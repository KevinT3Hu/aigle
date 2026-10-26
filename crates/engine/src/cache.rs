//! Cache keys (doc 02 §11) and the in-RAM stage-tile cache (doc 03 §4.3, §5).

use std::any::Any;
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use aigle_engine_api::CancelToken;
use parking_lot::{Condvar, Mutex};
use serde::Serialize;

use crate::ledger::{Allocation, MemClass, MemLedger};
use crate::scheduler::Class;

/// BLAKE3 hash over a canonical serialisation.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CacheKey(pub [u8; 32]);

impl CacheKey {
    pub fn hex(&self) -> String {
        self.0.iter().map(|b| format!("{b:02x}")).collect()
    }
}

impl fmt::Debug for CacheKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "CacheKey({}…)", &self.hex()[..12])
    }
}

/// Builds a [`CacheKey`]. Variable-length fields are length-prefixed; floats are hashed by bit
/// pattern after normalising `-0.0` (NaN is rejected earlier by params validation).
pub struct KeyBuilder(blake3::Hasher);

impl KeyBuilder {
    pub fn new(domain: &str) -> Self {
        let mut k = KeyBuilder(blake3::Hasher::new());
        k.str(domain);
        k
    }
    pub fn bytes(&mut self, b: &[u8]) -> &mut Self {
        self.0.update(&(b.len() as u64).to_le_bytes());
        self.0.update(b);
        self
    }
    pub fn str(&mut self, s: &str) -> &mut Self {
        self.bytes(s.as_bytes())
    }
    pub fn u32(&mut self, v: u32) -> &mut Self {
        self.0.update(&v.to_le_bytes());
        self
    }
    pub fn u64(&mut self, v: u64) -> &mut Self {
        self.0.update(&v.to_le_bytes());
        self
    }
    pub fn i32(&mut self, v: i32) -> &mut Self {
        self.0.update(&v.to_le_bytes());
        self
    }
    pub fn f32(&mut self, v: f32) -> &mut Self {
        self.0.update(&(v + 0.0).to_bits().to_le_bytes());
        self
    }
    pub fn f64(&mut self, v: f64) -> &mut Self {
        self.0.update(&(v + 0.0).to_bits().to_le_bytes());
        self
    }
    pub fn key(&mut self, k: &CacheKey) -> &mut Self {
        self.0.update(&k.0);
        self
    }
    /// Canonical serialisation of a params slice. serde_json output is deterministic for our
    /// types (struct fields in declaration order, no maps with random order).
    pub fn json<T: Serialize>(&mut self, v: &T) -> &mut Self {
        let s = serde_json::to_vec(v).expect("params serialise");
        self.bytes(&s)
    }
    pub fn finish(&self) -> CacheKey {
        CacheKey(*self.0.finalize().as_bytes())
    }
}

struct Entry {
    value: Arc<dyn Any + Send + Sync>,
    size: u64,
    /// Estimated time to recompute, in seconds.
    cost: f64,
    last_used: u64,
    owner: u64,
    /// Accounted in the ledger's separate device pool.
    device: bool,
    _alloc: Allocation,
}

#[derive(Default)]
struct State {
    map: HashMap<CacheKey, Entry>,
    /// Host-pool bytes held.
    used: u64,
    /// Device-pool bytes held.
    device_used: u64,
    tick: u64,
    /// Keys being computed: highest claimant class and claim count.
    inflight: HashMap<CacheKey, (Class, usize)>,
}

impl State {
    fn remove(&mut self, k: &CacheKey) -> Option<Entry> {
        let e = self.map.remove(k)?;
        if e.device {
            self.device_used -= e.size;
        } else {
            self.used -= e.size;
        }
        Some(e)
    }
}

/// Content-addressed cache of stage products. Entries in use (an `Arc` held outside the
/// cache) are pinned (doc 03 §5.1). Eviction removes the lowest
/// `recompute_cost / size × recency` first (doc 03 §5.2).
///
/// Capacity follows the ledger: the cache may hold `share` of the effective budget, and
/// never more than what the rest of the engine (other allocations and admitted
/// reservations) leaves free, so it shrinks under memory pressure and before admission
/// ([`TileCache::reclaim`] is the ledger's reclaimer). Entries in the device pool (discrete
/// GPU) are capped by the device budget the same way.
pub struct TileCache {
    ledger: Arc<MemLedger>,
    share: f64,
    state: Mutex<State>,
    /// Signalled on inserts and released claims.
    changed: Condvar,
}

/// Keys this task computes (doc 05 §3.1: work is deduplicated by cache key). Dropping it,
/// with or without having inserted the products, wakes the tasks waiting for them.
pub struct Claim<'a> {
    cache: &'a TileCache,
    keys: Vec<CacheKey>,
}

impl Drop for Claim<'_> {
    fn drop(&mut self) {
        let mut s = self.cache.state.lock();
        for k in &self.keys {
            if let Some(e) = s.inflight.get_mut(k) {
                e.1 -= 1;
                if e.1 == 0 {
                    s.inflight.remove(k);
                }
            }
        }
        drop(s);
        self.cache.changed.notify_all();
    }
}

/// Released entries' memory is not returned to the OS at once (allocator, driver pools);
/// keep this much of the device budget free.
const DEVICE_HEADROOM: f64 = 0.1;

impl TileCache {
    pub fn new(ledger: Arc<MemLedger>, share: f64) -> Self {
        TileCache {
            ledger,
            share,
            state: Mutex::new(State::default()),
            changed: Condvar::new(),
        }
    }

    /// Claim `keys` for computation by the current task. Returns the claim and the indices
    /// of keys another task of the same or a higher class is already computing: wait for
    /// those with [`TileCache::wait`]. Keys held only by lower classes are computed again
    /// rather than waited for, so a waiting task never depends on work that yields to it
    /// (no priority inversion).
    pub fn claim(&self, keys: &[CacheKey]) -> (Claim<'_>, Vec<usize>) {
        let class = crate::arbiter::current().class;
        let mut s = self.state.lock();
        let mut mine = Vec::with_capacity(keys.len());
        let mut theirs = Vec::new();
        for (i, k) in keys.iter().enumerate() {
            match s.inflight.get_mut(k) {
                Some(e) if e.0 >= class => theirs.push(i),
                Some(e) => {
                    e.0 = class;
                    e.1 += 1;
                    mine.push(*k);
                }
                None => {
                    s.inflight.insert(*k, (class, 1));
                    mine.push(*k);
                }
            }
        }
        (
            Claim {
                cache: self,
                keys: mine,
            },
            theirs,
        )
    }

    /// Wait for a key claimed by another task: its product, or `None` if the claim was
    /// released without one (cancelled) or `cancel` fired.
    pub fn wait<T: Any + Send + Sync>(
        &self,
        key: &CacheKey,
        cancel: Option<&CancelToken>,
    ) -> Option<Arc<T>> {
        let mut s = self.state.lock();
        loop {
            if let Some(e) = s.map.get(key) {
                return e.value.clone().downcast::<T>().ok();
            }
            if !s.inflight.contains_key(key) || cancel.is_some_and(|c| c.is_cancelled()) {
                return None;
            }
            self.changed.wait_for(&mut s, Duration::from_millis(50));
        }
    }

    /// Current (host, device) capacity in bytes; device is `u64::MAX` without a device pool.
    fn capacity(&self, s: &State) -> (u64, u64) {
        let snap = self.ledger.snapshot();
        let others = snap.used.saturating_sub(s.used) + snap.reserved;
        let host = ((snap.effective as f64 * self.share) as u64)
            .min(snap.effective.saturating_sub(others));
        let device = match self.ledger.device_budget() {
            None => u64::MAX,
            Some(b) => {
                let others = snap.device_used.saturating_sub(s.device_used);
                ((b as f64 * (1.0 - DEVICE_HEADROOM)) as u64).saturating_sub(others)
            }
        };
        (host, device)
    }

    pub fn get<T: Any + Send + Sync>(&self, key: &CacheKey) -> Option<Arc<T>> {
        let mut s = self.state.lock();
        s.tick += 1;
        let tick = s.tick;
        let e = s.map.get_mut(key)?;
        e.last_used = tick;
        e.value.clone().downcast::<T>().ok()
    }

    pub fn insert<T: Any + Send + Sync>(
        &self,
        key: CacheKey,
        value: Arc<T>,
        size: u64,
        class: MemClass,
        cost: Duration,
        owner: u64,
    ) {
        let alloc = self.ledger.alloc(class, size);
        let device = alloc.on_device_pool();
        let mut s = self.state.lock();
        s.tick += 1;
        let entry = Entry {
            value,
            size,
            cost: cost.as_secs_f64(),
            last_used: s.tick,
            owner,
            device,
            _alloc: alloc,
        };
        if device {
            s.device_used += size;
        } else {
            s.used += size;
        }
        let old = s.map.insert(key, entry);
        if let Some(old) = old {
            if old.device {
                s.device_used -= old.size;
            } else {
                s.used -= old.size;
            }
        }
        let (host, dev) = self.capacity(&s);
        Self::evict_locked(&mut s, host, dev);
        drop(s);
        self.changed.notify_all();
    }

    /// Evict unpinned entries, lowest score first, until host use ≤ `host` and device use
    /// ≤ `device`. Returns the host bytes freed.
    fn evict_locked(s: &mut State, host: u64, device: u64) -> u64 {
        if s.used <= host && s.device_used <= device {
            return 0;
        }
        let tick = s.tick;
        let mut candidates: Vec<(f64, CacheKey)> = s
            .map
            .iter()
            .filter(|(_, e)| Arc::strong_count(&e.value) == 1)
            .map(|(k, e)| {
                let age = (tick - e.last_used) as f64;
                let score = (e.cost.max(1e-6) / e.size.max(1) as f64) / (1.0 + age);
                (score, *k)
            })
            .collect();
        candidates.sort_by(|a, b| a.0.total_cmp(&b.0));
        let before = s.used;
        for (_, k) in candidates {
            let (h_over, d_over) = (s.used > host, s.device_used > device);
            if !h_over && !d_over {
                break;
            }
            // Only evict from a pool that is over its capacity.
            let dev = s.map.get(&k).is_some_and(|e| e.device);
            if (dev && d_over) || (!dev && h_over) {
                drop(s.remove(&k));
            }
        }
        before - s.used
    }

    /// Ledger reclaimer: free at least `bytes` of host memory if unpinned entries allow.
    /// Returns the bytes freed.
    pub fn reclaim(&self, bytes: u64) -> u64 {
        let mut s = self.state.lock();
        let target = s.used.saturating_sub(bytes);
        let freed = Self::evict_locked(&mut s, target, u64::MAX);
        if freed > 0 {
            tracing::debug!(bytes, freed, "cache reclaimed for admission / pressure");
        }
        freed
    }

    /// Evict down to the current capacity (after the budget shrank).
    pub fn trim(&self) -> u64 {
        let mut s = self.state.lock();
        let (host, dev) = self.capacity(&s);
        Self::evict_locked(&mut s, host, dev)
    }

    /// Remove one entry (it stays alive while referenced elsewhere).
    pub fn remove(&self, key: &CacheKey) {
        let e = self.state.lock().remove(key);
        drop(e);
    }

    /// Drop everything a closed session owned (doc 01 §3: dropping a Session releases caches).
    pub fn remove_owner(&self, owner: u64) {
        let mut s = self.state.lock();
        let keys: Vec<_> = s
            .map
            .iter()
            .filter(|(_, e)| e.owner == owner)
            .map(|(k, _)| *k)
            .collect();
        for k in keys {
            drop(s.remove(&k));
        }
    }

    /// Host-pool bytes held.
    pub fn used(&self) -> u64 {
        self.state.lock().used
    }

    /// Device-pool bytes held.
    pub fn device_used(&self) -> u64 {
        self.state.lock().device_used
    }

    pub fn len(&self) -> usize {
        self.state.lock().map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(i: u32) -> CacheKey {
        KeyBuilder::new("t").u32(i).finish()
    }

    #[test]
    fn keys_are_canonical() {
        assert_eq!(
            KeyBuilder::new("a").f32(-0.0).finish(),
            KeyBuilder::new("a").f32(0.0).finish()
        );
        assert_ne!(
            KeyBuilder::new("a").str("bc").finish(),
            KeyBuilder::new("ab").str("c").finish()
        );
    }

    #[test]
    fn evicts_to_capacity_but_keeps_pinned() {
        let ledger = MemLedger::new(500);
        let c = TileCache::new(ledger.clone(), 0.5);
        let pinned = Arc::new(1u32);
        c.insert(
            key(0),
            pinned.clone(),
            100,
            MemClass::StageTile,
            Duration::from_millis(1),
            1,
        );
        c.insert(
            key(1),
            Arc::new(2u32),
            100,
            MemClass::StageTile,
            Duration::from_millis(1),
            1,
        );
        c.insert(
            key(2),
            Arc::new(3u32),
            100,
            MemClass::StageTile,
            Duration::from_millis(1),
            1,
        );
        assert!(c.used() <= 250);
        assert_eq!(*c.get::<u32>(&key(0)).unwrap(), 1);
        assert_eq!(ledger.used(), c.used());
        c.remove_owner(1);
        assert!(c.is_empty());
        assert_eq!(ledger.used(), 0);
    }

    fn ins(c: &TileCache, i: u32, size: u64, class: MemClass) {
        c.insert(
            key(i),
            Arc::new(i),
            size,
            class,
            Duration::from_millis(1),
            1,
        );
    }

    #[test]
    fn yields_to_pressure_and_admission() {
        let ledger = MemLedger::new(1000);
        let c = Arc::new(TileCache::new(ledger.clone(), 0.5));
        let w = Arc::downgrade(&c);
        ledger.set_reclaimer(Box::new(move |b| w.upgrade().map_or(0, |c| c.reclaim(b))));
        for i in 0..5 {
            ins(&c, i, 100, MemClass::StageTile);
        }
        assert_eq!(c.used(), 500);
        // Pressure: effective 750, share 0.5 → 375 on the next trim.
        ledger.set_pressure(true);
        c.trim();
        assert!(c.used() <= 375, "{}", c.used());
        ledger.set_pressure(false);
        // Admission evicts the cache to make room.
        let other = ledger.alloc(MemClass::SourceCpu, 400);
        let r = ledger
            .try_reserve(450)
            .expect("cache evicted for the reservation");
        assert!(ledger.used() + 450 <= 1000);
        // While reserved, new inserts stay within what is left.
        ins(&c, 10, 100, MemClass::StageTile);
        assert!(ledger.used() + 450 <= 1000 || c.used() == 0);
        drop((r, other));
    }

    #[test]
    fn claims_dedup_without_priority_inversion() {
        use crate::arbiter::{TaskCtx, with_task};
        let c = Arc::new(TileCache::new(MemLedger::new(1 << 20), 0.5));
        let ks = [key(1), key(2)];
        let bg = TaskCtx {
            class: Class::Background,
            job: 1,
        };
        let ia = TaskCtx {
            class: Class::Interactive,
            job: 2,
        };
        let (claim_bg, theirs) = with_task(bg, || c.claim(&ks[..1]));
        assert!(theirs.is_empty());
        // Interactive does not wait for background work; it claims and computes itself.
        let (claim_ia, theirs) = with_task(ia, || c.claim(&ks));
        assert!(theirs.is_empty());
        // A second background claimant now waits for the interactive one.
        let (_c3, theirs) = with_task(bg, || c.claim(&ks));
        assert_eq!(theirs, [0, 1]);
        let c2 = c.clone();
        let waiter = std::thread::spawn(move || c2.wait::<u32>(&key(1), None).map(|v| *v));
        std::thread::sleep(Duration::from_millis(30));
        ins(&c, 1, 10, MemClass::StageTile);
        assert_eq!(waiter.join().unwrap(), Some(1));
        drop((claim_bg, claim_ia));
        // Released without a product: waiters stop waiting.
        assert!(c.wait::<u32>(&key(2), None).is_none());
    }

    #[test]
    fn device_tiles_have_their_own_cap() {
        let ledger = MemLedger::new(1000);
        ledger.set_device_budget(300);
        let c = TileCache::new(ledger.clone(), 0.5);
        for i in 0..5 {
            ins(&c, i, 100, MemClass::DeviceTile);
        }
        assert!(c.device_used() <= 270, "{}", c.device_used());
        assert_eq!(ledger.used(), 0);
        ins(&c, 10, 100, MemClass::StageTile);
        assert_eq!(c.used(), 100);
    }
}
