//! `MemLedger`: the single source of truth for engine memory (doc 03 §2.2). Every engine
//! allocation is accounted through an RAII [`Allocation`]; admission control (doc 03 §6)
//! reserves a job's declared peak through a [`Reservation`].
//!
//! Two pools:
//! - **host**: the engine budget `B` (doc 03 §2.1). Its *effective* value shrinks under
//!   system memory pressure ([`MemLedger::set_pressure`], doc 03 §2.1 PSI).
//! - **device**: only with a discrete GPU, whose memory is not system RAM. The budget is the
//!   driver's (`VK_EXT_memory_budget`). Without a separate device pool (CPU backend,
//!   integrated GPU on unified memory) device classes count against the host pool.
//!
//! Before admission fails, the ledger asks a *reclaimer* (the stage-tile cache) to evict
//! unpinned entries (doc 03 §6 "after evicting what it can").

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use aigle_engine_api::CancelToken;
use parking_lot::{Condvar, Mutex};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MemClass {
    SourceCpu,
    /// GPU source tile atlas.
    SourceGpu,
    StageTile,
    /// Stage tiles resident on the GPU.
    DeviceTile,
    Field,
    Asset,
    Transient,
    Frame,
}

impl MemClass {
    fn on_device(self) -> bool {
        matches!(self, MemClass::SourceGpu | MemClass::DeviceTile)
    }
}

/// Effective budget under memory pressure, as a share of `B` (doc 03 §2.1).
pub const PRESSURE_SHARE: f64 = 0.75;

#[derive(Default, Debug)]
struct State {
    used: u64,
    reserved: u64,
    device_used: u64,
    by_class: HashMap<MemClass, u64>,
}

/// Frees memory on request: called with the number of host bytes wanted, returns the number
/// freed. Must not call back into admission.
pub type Reclaimer = Box<dyn Fn(u64) -> u64 + Send + Sync>;

pub struct MemLedger {
    budget: u64,
    effective: AtomicU64,
    pressure: AtomicBool,
    device_budget: AtomicU64,
    state: Mutex<State>,
    freed: Condvar,
    reclaimer: OnceLock<Reclaimer>,
    spill_dir: OnceLock<Option<std::path::PathBuf>>,
}

impl std::fmt::Debug for MemLedger {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemLedger")
            .field("budget", &self.budget)
            .field("effective", &self.effective())
            .field("state", &*self.state.lock())
            .finish()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LedgerSnapshot {
    pub budget: u64,
    pub effective: u64,
    pub used: u64,
    pub reserved: u64,
    /// Device pool (0 / 0 without a separate pool).
    pub device_budget: u64,
    pub device_used: u64,
    pub by_class: Vec<(MemClass, u64)>,
}

impl MemLedger {
    pub fn new(budget: u64) -> Arc<MemLedger> {
        Arc::new(MemLedger {
            budget,
            effective: AtomicU64::new(budget),
            pressure: AtomicBool::new(false),
            device_budget: AtomicU64::new(0),
            state: Mutex::new(State::default()),
            freed: Condvar::new(),
            reclaimer: OnceLock::new(),
            spill_dir: OnceLock::new(),
        })
    }

    /// The configured budget `B`.
    pub fn budget(&self) -> u64 {
        self.budget
    }

    /// `B`, or [`PRESSURE_SHARE`] of it under memory pressure.
    pub fn effective(&self) -> u64 {
        self.effective.load(Ordering::Acquire)
    }

    pub fn under_pressure(&self) -> bool {
        self.pressure.load(Ordering::Acquire)
    }

    /// Enter or leave the pressure state. Entering asks the reclaimer to get back under the
    /// reduced budget. Returns whether the state changed.
    pub fn set_pressure(&self, on: bool) -> bool {
        if self.pressure.swap(on, Ordering::AcqRel) == on {
            return false;
        }
        let eff = if on {
            (self.budget as f64 * PRESSURE_SHARE) as u64
        } else {
            self.budget
        };
        self.effective.store(eff, Ordering::Release);
        if on {
            let over = self.used().saturating_sub(eff);
            if over > 0 {
                self.reclaim(over);
            }
        }
        self.freed.notify_all();
        true
    }

    /// Give device classes their own pool of `bytes` (discrete GPU).
    pub fn set_device_budget(&self, bytes: u64) {
        self.device_budget.store(bytes, Ordering::Release);
    }

    /// Separate device pool budget, if any.
    pub fn device_budget(&self) -> Option<u64> {
        Some(self.device_budget.load(Ordering::Acquire)).filter(|b| *b > 0)
    }

    pub fn device_used(&self) -> u64 {
        self.state.lock().device_used
    }

    /// Register the reclaimer (once).
    pub fn set_reclaimer(&self, r: Reclaimer) {
        let _ = self.reclaimer.set(r);
    }

    /// Where source pyramids too large for the budget are spilled (doc 03 §7.3): set once by
    /// the engine, else `<default cache dir>/source`.
    pub fn set_spill_dir(&self, dir: Option<std::path::PathBuf>) {
        let _ = self.spill_dir.set(dir);
    }

    pub fn spill_dir(&self) -> Option<&std::path::Path> {
        self.spill_dir
            .get_or_init(|| crate::data::cache_dir().map(|d| d.join("source")))
            .as_deref()
    }

    fn reclaim(&self, bytes: u64) -> u64 {
        self.reclaimer.get().map_or(0, |r| r(bytes))
    }

    /// Host bytes in use.
    pub fn used(&self) -> u64 {
        self.state.lock().used
    }

    pub fn reserved(&self) -> u64 {
        self.state.lock().reserved
    }

    fn device_pool(&self, class: MemClass) -> bool {
        class.on_device() && self.device_budget().is_some()
    }

    /// Record an allocation. Never fails: callers that must respect the budget go through
    /// admission ([`MemLedger::try_reserve`]) or cache eviction first.
    pub fn alloc(self: &Arc<Self>, class: MemClass, size: u64) -> Allocation {
        let device = self.device_pool(class);
        let mut s = self.state.lock();
        if device {
            s.device_used += size;
        } else {
            s.used += size;
        }
        *s.by_class.entry(class).or_default() += size;
        Allocation {
            ledger: self.clone(),
            class,
            size,
            device,
        }
    }

    fn try_reserve_once(self: &Arc<Self>, peak: u64) -> Result<Reservation, u64> {
        let mut s = self.state.lock();
        let eff = self.effective();
        if s.used + s.reserved + peak <= eff {
            s.reserved += peak;
            Ok(Reservation {
                ledger: self.clone(),
                size: peak,
            })
        } else {
            Err(s.used + s.reserved + peak - eff)
        }
    }

    /// Admission: `used + reserved + peak ≤ effective budget`, after asking the reclaimer
    /// for the shortfall.
    pub fn try_reserve(self: &Arc<Self>, peak: u64) -> Option<Reservation> {
        match self.try_reserve_once(peak) {
            Ok(r) => Some(r),
            Err(short) => {
                if self.reclaim(short) == 0 {
                    return None;
                }
                self.try_reserve_once(peak).ok()
            }
        }
    }

    /// A job larger than what is free runs anyway when nothing else runs (doc 03 §6, Q22).
    pub fn reserve_forced(self: &Arc<Self>, peak: u64) -> Reservation {
        self.state.lock().reserved += peak;
        Reservation {
            ledger: self.clone(),
            size: peak,
        }
    }

    /// Admission from inside a running task (the peak is only known once it has started, e.g.
    /// after reading a raw header): wait until the peak fits, or run over budget when no other
    /// reservation is outstanding (doc 03 §6, Q22). `None` if cancelled while waiting.
    pub fn admit(self: &Arc<Self>, peak: u64, cancel: Option<&CancelToken>) -> Option<Reservation> {
        loop {
            if let Some(r) = self.try_reserve(peak) {
                return Some(r);
            }
            let mut s = self.state.lock();
            if s.reserved == 0 {
                tracing::warn!(
                    peak,
                    used = s.used,
                    effective = self.effective(),
                    "admitting a job over budget"
                );
                s.reserved += peak;
                return Some(Reservation {
                    ledger: self.clone(),
                    size: peak,
                });
            }
            if cancel.is_some_and(|c| c.is_cancelled()) {
                return None;
            }
            self.freed.wait_for(&mut s, Duration::from_millis(50));
        }
    }

    pub fn snapshot(&self) -> LedgerSnapshot {
        let s = self.state.lock();
        let mut by_class: Vec<_> = s.by_class.iter().map(|(k, v)| (*k, *v)).collect();
        by_class.sort_by_key(|(k, _)| format!("{k:?}"));
        LedgerSnapshot {
            budget: self.budget,
            effective: self.effective(),
            used: s.used,
            reserved: s.reserved,
            device_budget: self.device_budget().unwrap_or(0),
            device_used: s.device_used,
            by_class,
        }
    }
}

#[derive(Debug)]
pub struct Allocation {
    ledger: Arc<MemLedger>,
    class: MemClass,
    size: u64,
    device: bool,
}

impl Allocation {
    pub fn size(&self) -> u64 {
        self.size
    }

    /// Accounted in the separate device pool.
    pub fn on_device_pool(&self) -> bool {
        self.device
    }
}

impl Drop for Allocation {
    fn drop(&mut self) {
        let mut s = self.ledger.state.lock();
        if self.device {
            s.device_used -= self.size;
        } else {
            s.used -= self.size;
        }
        if let Some(v) = s.by_class.get_mut(&self.class) {
            *v -= self.size;
        }
        drop(s);
        self.ledger.freed.notify_all();
    }
}

#[derive(Debug)]
pub struct Reservation {
    ledger: Arc<MemLedger>,
    size: u64,
}

impl Drop for Reservation {
    fn drop(&mut self) {
        self.ledger.state.lock().reserved -= self.size;
        self.ledger.freed.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accounting_and_admission() {
        let l = MemLedger::new(100);
        let a = l.alloc(MemClass::StageTile, 60);
        assert_eq!(l.used(), 60);
        assert!(l.try_reserve(50).is_none());
        let r = l.try_reserve(40).unwrap();
        assert!(l.try_reserve(1).is_none());
        drop(r);
        drop(a);
        let s = l.snapshot();
        assert_eq!((s.used, s.reserved), (0, 0));
    }

    #[test]
    fn pressure_shrinks_the_effective_budget() {
        let l = MemLedger::new(100);
        let _a = l.alloc(MemClass::StageTile, 70);
        assert!(l.try_reserve(30).is_some());
        assert!(l.set_pressure(true));
        assert!(!l.set_pressure(true));
        assert_eq!(l.effective(), 75);
        assert!(l.try_reserve(10).is_none());
        assert!(l.try_reserve(5).is_some());
        l.set_pressure(false);
        assert_eq!(l.effective(), 100);
    }

    #[test]
    fn reclaimer_runs_before_admission_fails() {
        let l = MemLedger::new(100);
        let held = Arc::new(Mutex::new(vec![l.alloc(MemClass::StageTile, 80)]));
        let h = held.clone();
        l.set_reclaimer(Box::new(move |_| {
            let mut v = h.lock();
            let freed = v.iter().map(|a| a.size()).sum();
            v.clear();
            freed
        }));
        assert!(l.try_reserve(50).is_some());
        assert!(held.lock().is_empty());
    }

    #[test]
    fn device_pool_is_separate() {
        let l = MemLedger::new(100);
        let a = l.alloc(MemClass::DeviceTile, 50);
        assert_eq!(
            l.used(),
            50,
            "unified memory: device classes use the host pool"
        );
        drop(a);
        l.set_device_budget(1000);
        let _b = l.alloc(MemClass::DeviceTile, 500);
        assert_eq!((l.used(), l.device_used()), (0, 500));
        assert!(l.try_reserve(100).is_some());
    }

    #[test]
    fn admit_waits_then_forces_when_alone() {
        let l = MemLedger::new(100);
        let r = l.try_reserve(80).unwrap();
        let l2 = l.clone();
        let t = std::thread::spawn(move || l2.admit(50, None).map(|r| r.size));
        std::thread::sleep(Duration::from_millis(80));
        assert!(!t.is_finished(), "must wait while another reservation runs");
        drop(r);
        assert_eq!(t.join().unwrap(), Some(50));
        // Larger than the whole budget, nothing else reserved: admitted over budget.
        assert!(l.admit(500, None).is_some());
        // Cancelled while waiting.
        let _r = l.try_reserve(60).unwrap();
        let c = CancelToken::new();
        c.cancel();
        assert!(l.admit(60, Some(&c)).is_none());
    }
}
