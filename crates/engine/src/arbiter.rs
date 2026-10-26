//! Shared-resource arbitration by scheduler class (doc 05 §5, §6).
//!
//! Two primitives, both pure policy over task metadata and a clock ([`Policy`],
//! [`GatePolicy`], tested with a simulated clock) wrapped in a mutex and condvar:
//!
//! - [`Arbiter`]: a resource with `capacity` holders (the GPU queue: 1; native decoders;
//!   I/O). Waiters are granted by strict priority — class, then FIFO — except that a
//!   non-interactive job that has made no progress for [`GUARD`] is granted first, once
//!   (starvation guard, doc 05 Q33).
//! - [`CpuGate`]: CPU tokens between the classes. Every running task registers its class;
//!   a task calls [`CpuGate::checkpoint`] at its chunk boundaries and waits there while a
//!   task of a higher class runs, unless it is starved. So an interactive render takes the
//!   CPU from an export at the export's next chunk boundary. All classes share one rayon
//!   pool; within one chunk they share the CPU.
//!
//! The current task's class and job id come from a thread-local [`TaskCtx`] that the
//! scheduler sets around each task; work outside the scheduler counts as interactive.

use std::cell::Cell;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use aigle_engine_api::CancelToken;
use parking_lot::{Condvar, Mutex};

use crate::scheduler::Class;

/// A job that made no progress for this long is granted ahead of higher classes once.
pub const GUARD: Duration = Duration::from_secs(2);
/// Re-evaluation period of waiters (for the guard).
const POLL: Duration = Duration::from_millis(20);

/// Class and job of the running task.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TaskCtx {
    pub class: Class,
    pub job: u64,
}

thread_local! {
    static CURRENT: Cell<Option<TaskCtx>> = const { Cell::new(None) };
}

/// The current task, or an interactive pseudo-task (job 0) outside the scheduler.
pub fn current() -> TaskCtx {
    CURRENT.with(|c| c.get()).unwrap_or(TaskCtx {
        class: Class::Interactive,
        job: 0,
    })
}

/// Run `f` as task `ctx` on this thread (also used to carry the context into helper threads).
pub fn with_task<R>(ctx: TaskCtx, f: impl FnOnce() -> R) -> R {
    struct Reset(Option<TaskCtx>);
    impl Drop for Reset {
        fn drop(&mut self) {
            CURRENT.with(|c| c.set(self.0));
        }
    }
    let _reset = Reset(CURRENT.with(|c| c.replace(Some(ctx))));
    f()
}

// ---------------------------------------------------------------------------------------------
// Process-wide resources (the CPU and the native decoders are shared by every engine)

/// CPU tokens between classes.
pub fn cpu_gate() -> &'static Arc<CpuGate> {
    static G: std::sync::OnceLock<Arc<CpuGate>> = std::sync::OnceLock::new();
    G.get_or_init(Default::default)
}

/// Concurrent native decodes (LibRaw + librtprocess, single-threaded each): half the
/// hardware threads; memory admission (doc 03 §6) bounds them further.
pub fn decoders() -> &'static Arbiter {
    static A: std::sync::OnceLock<Arbiter> = std::sync::OnceLock::new();
    A.get_or_init(|| Arbiter::new("decode", (hw_threads() / 2).max(1)))
}

/// File writes (doc 05 §5: IO pool of 2).
pub fn io() -> &'static Arbiter {
    static A: std::sync::OnceLock<Arbiter> = std::sync::OnceLock::new();
    A.get_or_init(|| Arbiter::new("io", 2))
}

pub fn hw_threads() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
}

// ---------------------------------------------------------------------------------------------
// Arbiter

#[derive(Clone, Copy, Debug)]
struct Waiter {
    ticket: u64,
    ctx: TaskCtx,
    since: Instant,
}

/// Grant policy of an [`Arbiter`].
#[derive(Debug, Default)]
pub struct Policy {
    capacity: usize,
    held: usize,
    seq: u64,
    waiting: Vec<Waiter>,
    /// Last grant per job, for the starvation guard.
    progress: HashMap<u64, Instant>,
}

impl Policy {
    pub fn new(capacity: usize) -> Policy {
        Policy {
            capacity: capacity.max(1),
            ..Default::default()
        }
    }

    pub fn enqueue(&mut self, ctx: TaskCtx, now: Instant) -> u64 {
        self.seq += 1;
        self.waiting.push(Waiter {
            ticket: self.seq,
            ctx,
            since: now,
        });
        self.seq
    }

    pub fn withdraw(&mut self, ticket: u64) {
        self.waiting.retain(|w| w.ticket != ticket);
    }

    fn starved(&self, w: &Waiter, now: Instant) -> bool {
        w.ctx.class != Class::Interactive
            && now.duration_since(*self.progress.get(&w.ctx.job).unwrap_or(&w.since)) >= GUARD
    }

    /// The waiter that is next, regardless of capacity.
    pub fn best(&self, now: Instant) -> Option<u64> {
        if let Some(w) = self.waiting.iter().find(|w| self.starved(w, now)) {
            return Some(w.ticket);
        }
        self.waiting
            .iter()
            .max_by_key(|w| (w.ctx.class, std::cmp::Reverse(w.ticket)))
            .map(|w| w.ticket)
    }

    /// Grant `ticket` if it is next and a slot is free.
    pub fn try_grant(&mut self, ticket: u64, now: Instant) -> bool {
        if self.held >= self.capacity || self.best(now) != Some(ticket) {
            return false;
        }
        let i = self
            .waiting
            .iter()
            .position(|w| w.ticket == ticket)
            .expect("queued");
        let w = self.waiting.remove(i);
        self.held += 1;
        self.progress.insert(w.ctx.job, now);
        if self.progress.len() > 256 {
            let live: Vec<u64> = self.waiting.iter().map(|w| w.ctx.job).collect();
            self.progress
                .retain(|j, t| live.contains(j) || now.duration_since(*t) < GUARD);
        }
        true
    }

    pub fn release(&mut self) {
        self.held -= 1;
    }
}

/// A class-arbitrated resource with a fixed number of holders.
pub struct Arbiter {
    name: &'static str,
    state: Mutex<Policy>,
    cv: Condvar,
}

/// Held while using the resource.
pub struct Grant<'a> {
    arb: &'a Arbiter,
}

impl Drop for Grant<'_> {
    fn drop(&mut self) {
        self.arb.state.lock().release();
        self.arb.cv.notify_all();
    }
}

impl Arbiter {
    pub fn new(name: &'static str, capacity: usize) -> Arbiter {
        Arbiter {
            name,
            state: Mutex::new(Policy::new(capacity)),
            cv: Condvar::new(),
        }
    }

    /// Wait for a slot as the current task. `None` if `cancel` fires while waiting.
    pub fn acquire(&self, cancel: Option<&CancelToken>) -> Option<Grant<'_>> {
        let ctx = current();
        let mut st = self.state.lock();
        let t0 = Instant::now();
        let ticket = st.enqueue(ctx, t0);
        loop {
            if st.try_grant(ticket, Instant::now()) {
                drop(st);
                let waited = t0.elapsed();
                if waited > Duration::from_millis(5) {
                    tracing::trace!(arbiter = self.name, ?ctx, ?waited, "granted after waiting");
                }
                // Others may be grantable too (capacity > 1).
                self.cv.notify_all();
                return Some(Grant { arb: self });
            }
            if cancel.is_some_and(|c| c.is_cancelled()) {
                st.withdraw(ticket);
                drop(st);
                self.cv.notify_all();
                return None;
            }
            self.cv.wait_for(&mut st, POLL);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// CPU gate

/// Policy of the [`CpuGate`].
#[derive(Debug, Default)]
pub struct GatePolicy {
    running: HashMap<Class, usize>,
    progress: HashMap<u64, Instant>,
}

impl GatePolicy {
    pub fn start(&mut self, ctx: TaskCtx, now: Instant) {
        *self.running.entry(ctx.class).or_default() += 1;
        self.progress.insert(ctx.job, now);
    }

    pub fn finish(&mut self, ctx: TaskCtx) {
        if let Some(n) = self.running.get_mut(&ctx.class) {
            *n -= 1;
        }
        self.progress.remove(&ctx.job);
    }

    /// Whether `ctx` may run its next chunk now; passing counts as progress.
    pub fn pass(&mut self, ctx: TaskCtx, now: Instant) -> bool {
        let higher = self.running.iter().any(|(c, n)| *c > ctx.class && *n > 0);
        let last = *self.progress.get(&ctx.job).unwrap_or(&now);
        if !higher || now.duration_since(last) >= GUARD {
            self.progress.insert(ctx.job, now);
            return true;
        }
        false
    }
}

/// CPU tokens between classes (see the module docs).
#[derive(Default)]
pub struct CpuGate {
    state: Mutex<GatePolicy>,
    cv: Condvar,
}

/// Registration of a running task; dropping it wakes yielding lower-class tasks.
pub struct Running {
    gate: Arc<CpuGate>,
    ctx: TaskCtx,
}

impl Drop for Running {
    fn drop(&mut self) {
        self.gate.state.lock().finish(self.ctx);
        self.gate.cv.notify_all();
    }
}

impl CpuGate {
    pub fn start(self: &Arc<Self>, ctx: TaskCtx) -> Running {
        self.state.lock().start(ctx, Instant::now());
        Running {
            gate: self.clone(),
            ctx,
        }
    }

    /// Chunk boundary of the current task: wait while higher classes run (or until starved).
    /// Returns false if `cancel` fired while waiting.
    pub fn checkpoint(&self, cancel: Option<&CancelToken>) -> bool {
        let ctx = current();
        if ctx.class == Class::Interactive {
            return true;
        }
        let mut st = self.state.lock();
        loop {
            if st.pass(ctx, Instant::now()) {
                return true;
            }
            if cancel.is_some_and(|c| c.is_cancelled()) {
                return false;
            }
            self.cv.wait_for(&mut st, POLL);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(class: Class, job: u64) -> TaskCtx {
        TaskCtx { class, job }
    }

    #[test]
    fn strict_priority_then_fifo() {
        let now = Instant::now();
        let mut p = Policy::new(1);
        let bg = p.enqueue(t(Class::Background, 1), now);
        let ag = p.enqueue(t(Class::Agent, 2), now);
        let ia1 = p.enqueue(t(Class::Interactive, 3), now);
        let ia2 = p.enqueue(t(Class::Interactive, 4), now);
        assert!(!p.try_grant(bg, now));
        assert!(p.try_grant(ia1, now));
        assert!(!p.try_grant(ia2, now), "capacity 1");
        p.release();
        assert!(p.try_grant(ia2, now));
        p.release();
        assert!(p.try_grant(ag, now));
        p.release();
        assert!(p.try_grant(bg, now));
    }

    #[test]
    fn starved_job_gets_one_chunk_per_guard() {
        // Continuous interactive load on a simulated clock; one background job.
        let t0 = Instant::now();
        let mut p = Policy::new(1);
        let mut bg = p.enqueue(t(Class::Background, 9), t0);
        let mut bg_grants = Vec::new();
        let mut ms = 0u64;
        while ms < 10_000 {
            let now = t0 + Duration::from_millis(ms);
            let ia = p.enqueue(t(Class::Interactive, 1), now);
            if p.try_grant(bg, now) {
                bg_grants.push(ms);
                p.release();
                bg = p.enqueue(t(Class::Background, 9), now);
            }
            assert!(p.try_grant(ia, now));
            p.release();
            ms += 10;
        }
        // Invariant: never starved longer than the guard plus one chunk (10 ms here).
        let mut last = 0;
        for &g in &bg_grants {
            assert!(g - last <= 2010, "{bg_grants:?}");
            last = g;
        }
        assert!((4..=5).contains(&bg_grants.len()), "{bg_grants:?}");
    }

    #[test]
    fn gate_yields_to_higher_classes() {
        let t0 = Instant::now();
        let mut g = GatePolicy::default();
        let ex = t(Class::Background, 7);
        g.start(ex, t0);
        assert!(g.pass(ex, t0));
        let ia = t(Class::Interactive, 8);
        g.start(ia, t0);
        assert!(!g.pass(ex, t0 + Duration::from_millis(100)));
        assert!(
            g.pass(ex, t0 + GUARD),
            "starved: one chunk after the guard interval"
        );
        assert!(!g.pass(ex, t0 + GUARD + Duration::from_millis(10)));
        g.finish(ia);
        assert!(g.pass(ex, t0 + GUARD + Duration::from_millis(20)));
    }

    #[test]
    fn arbiter_threads_and_cancel() {
        let a = Arc::new(Arbiter::new("t", 1));
        let g = a.acquire(None).unwrap();
        let c = CancelToken::new();
        let (a2, c2) = (a.clone(), c.clone());
        let h = std::thread::spawn(move || a2.acquire(Some(&c2)).is_some());
        std::thread::sleep(Duration::from_millis(50));
        c.cancel();
        assert!(!h.join().unwrap());
        drop(g);
        let a3 = a.clone();
        let h = std::thread::spawn(move || {
            with_task(t(Class::Batch, 3), || a3.acquire(None).is_some())
        });
        assert!(h.join().unwrap());
    }
}
