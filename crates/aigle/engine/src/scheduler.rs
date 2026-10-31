//! Four-class priority scheduler (doc 05 §2, §3).
//!
//! Worker threads drain a strict-priority ready queue (class, then in-class priority, then
//! FIFO), with delayed tasks (the Draft → Refined settle timer), cooperative cancellation and
//! memory admission (doc 03 §6).
//!
//! Each task runs with its [`crate::arbiter::TaskCtx`] (class, job id) set on the worker,
//! and registered with the CPU gate. Shared resources are arbitrated by class where they are used:
//! the GPU queue (`backend::gpu`), native decoders and file writes ([`crate::arbiter`]).
//! Long lower-class work (exports, thumbnails, agent renders) is cut into chunks with a
//! [`crate::arbiter::CpuGate::checkpoint`] between them, and identical products are
//! computed once (`TileCache::claim`).

use std::cmp::{Ordering, Reverse};
use std::collections::BinaryHeap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::time::{Duration, Instant};

use aigle_engine_api::CancelToken;
use parking_lot::{Condvar, Mutex};

use crate::ledger::MemLedger;

/// Scheduler classes, highest last so that the derived `Ord` ranks `Interactive` highest.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Class {
    Background,
    Batch,
    Agent,
    Interactive,
}

#[derive(Clone, Debug)]
pub struct TaskSpec {
    pub name: &'static str,
    pub class: Class,
    pub prio: i32,
    /// Declared peak memory for admission; 0 = negligible.
    pub mem_peak: u64,
    pub cancel: Option<CancelToken>,
    pub not_before: Option<Instant>,
}

impl TaskSpec {
    pub fn new(name: &'static str, class: Class) -> Self {
        TaskSpec {
            name,
            class,
            prio: 0,
            mem_peak: 0,
            cancel: None,
            not_before: None,
        }
    }
    pub fn prio(mut self, p: i32) -> Self {
        self.prio = p;
        self
    }
    pub fn cancel(mut self, c: CancelToken) -> Self {
        self.cancel = Some(c);
        self
    }
    pub fn mem_peak(mut self, bytes: u64) -> Self {
        self.mem_peak = bytes;
        self
    }
    pub fn delay(mut self, d: Duration) -> Self {
        self.not_before = Some(Instant::now() + d);
        self
    }
}

type TaskFn = Box<dyn FnOnce() + Send + 'static>;

struct Queued {
    spec: TaskSpec,
    seq: u64,
    run: TaskFn,
}

impl Queued {
    fn rank(&self) -> (Class, i32, Reverse<u64>) {
        (self.spec.class, self.spec.prio, Reverse(self.seq))
    }
}

impl PartialEq for Queued {
    fn eq(&self, o: &Self) -> bool {
        self.rank() == o.rank()
    }
}
impl Eq for Queued {}
impl PartialOrd for Queued {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for Queued {
    fn cmp(&self, o: &Self) -> Ordering {
        self.rank().cmp(&o.rank())
    }
}

struct Delayed {
    at: Instant,
    task: Queued,
}
impl PartialEq for Delayed {
    fn eq(&self, o: &Self) -> bool {
        (self.at, self.task.seq) == (o.at, o.task.seq)
    }
}
impl Eq for Delayed {}
impl PartialOrd for Delayed {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for Delayed {
    fn cmp(&self, o: &Self) -> Ordering {
        (self.at, self.task.seq).cmp(&(o.at, o.task.seq))
    }
}

#[derive(Default)]
struct State {
    ready: BinaryHeap<Queued>,
    delayed: BinaryHeap<Reverse<Delayed>>,
    seq: u64,
    running: usize,
    shutdown: bool,
}

struct Shared {
    state: Mutex<State>,
    cv: Condvar,
    ledger: Arc<MemLedger>,
}

/// Cheap-to-clone handle. Worker threads exit after [`Scheduler::shutdown`].
#[derive(Clone)]
pub struct Scheduler {
    shared: Arc<Shared>,
}

impl Scheduler {
    pub fn new(workers: usize, ledger: Arc<MemLedger>) -> Scheduler {
        let shared = Arc::new(Shared {
            state: Mutex::new(State::default()),
            cv: Condvar::new(),
            ledger,
        });
        for i in 0..workers.max(1) {
            let s = shared.clone();
            std::thread::Builder::new()
                .name(format!("aigle-worker-{i}"))
                .spawn(move || worker(s))
                .expect("spawn engine worker");
        }
        Scheduler { shared }
    }

    pub fn spawn(&self, spec: TaskSpec, f: impl FnOnce() + Send + 'static) {
        let mut st = self.shared.state.lock();
        if st.shutdown {
            return;
        }
        st.seq += 1;
        let q = Queued {
            seq: st.seq,
            run: Box::new(f),
            spec,
        };
        match q.spec.not_before {
            Some(at) if at > Instant::now() => st.delayed.push(Reverse(Delayed { at, task: q })),
            _ => st.ready.push(q),
        }
        drop(st);
        self.shared.cv.notify_one();
    }

    pub fn shutdown(&self) {
        let mut st = self.shared.state.lock();
        st.shutdown = true;
        st.ready.clear();
        st.delayed.clear();
        drop(st);
        self.shared.cv.notify_all();
    }

    /// Number of queued (ready + delayed) tasks.
    pub fn queued(&self) -> usize {
        let st = self.shared.state.lock();
        st.ready.len() + st.delayed.len()
    }
}

fn worker(s: Arc<Shared>) {
    let mut st = s.state.lock();
    loop {
        if st.shutdown {
            return;
        }
        let now = Instant::now();
        while st.delayed.peek().is_some_and(|d| d.0.at <= now) {
            let Reverse(d) = st.delayed.pop().expect("peeked");
            st.ready.push(d.task);
        }

        let Some(top) = st.ready.peek() else {
            match st.delayed.peek().map(|d| d.0.at) {
                Some(at) => {
                    s.cv.wait_until(&mut st, at);
                }
                None => s.cv.wait(&mut st),
            }
            continue;
        };

        if top.spec.cancel.as_ref().is_some_and(|c| c.is_cancelled()) {
            st.ready.pop();
            continue;
        }

        // Admission (doc 03 §6): wait for memory unless nothing else runs.
        let peak = top.spec.mem_peak;
        let reservation = if peak == 0 {
            None
        } else {
            match s.ledger.try_reserve(peak) {
                Some(r) => Some(r),
                None if st.running == 0 => Some(s.ledger.reserve_forced(peak)),
                None => {
                    s.cv.wait_for(&mut st, Duration::from_millis(50));
                    continue;
                }
            }
        };

        let task = st.ready.pop().expect("peeked");
        st.running += 1;
        drop(st);

        let name = task.spec.name;
        let ctx = crate::arbiter::TaskCtx {
            class: task.spec.class,
            job: task.seq,
        };
        let run = task.run;
        // Task bodies stay off rayon threads: they block (claims, gates, admission), and a
        // rayon thread may run them nested inside another task's join.
        let _running = crate::arbiter::cpu_gate().start(ctx);
        let ok = crate::arbiter::with_task(ctx, || catch_unwind(AssertUnwindSafe(run)).is_ok());
        if !ok {
            tracing::error!(task = name, "engine task panicked");
        }
        drop(reservation);

        st = s.state.lock();
        st.running -= 1;
        s.cv.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn strict_priority_then_fifo() {
        let ledger = MemLedger::new(1 << 30);
        let sched = Scheduler::new(1, ledger);
        let (tx, rx) = mpsc::channel();
        // Block the single worker while we queue.
        let (gate_tx, gate_rx) = mpsc::channel::<()>();
        sched.spawn(TaskSpec::new("gate", Class::Interactive), move || {
            gate_rx.recv().ok();
        });
        std::thread::sleep(Duration::from_millis(20));
        for (name, class) in [
            ("bg", Class::Background),
            ("agent1", Class::Agent),
            ("ia", Class::Interactive),
            ("agent2", Class::Agent),
            ("batch", Class::Batch),
        ] {
            let tx = tx.clone();
            sched.spawn(TaskSpec::new("t", class), move || tx.send(name).unwrap());
        }
        let cancelled = CancelToken::new();
        cancelled.cancel();
        let txc = tx.clone();
        sched.spawn(
            TaskSpec::new("c", Class::Interactive).cancel(cancelled),
            move || txc.send("cancelled").unwrap(),
        );
        gate_tx.send(()).unwrap();
        let order: Vec<_> = (0..5)
            .map(|_| rx.recv_timeout(Duration::from_secs(2)).unwrap())
            .collect();
        assert_eq!(order, ["ia", "agent1", "agent2", "batch", "bg"]);
        assert!(rx.recv_timeout(Duration::from_millis(50)).is_err());
        sched.shutdown();
    }

    #[test]
    fn delayed_tasks_run_later() {
        let sched = Scheduler::new(2, MemLedger::new(1 << 30));
        let (tx, rx) = mpsc::channel();
        let t0 = Instant::now();
        sched.spawn(
            TaskSpec::new("d", Class::Interactive).delay(Duration::from_millis(60)),
            move || {
                tx.send(Instant::now()).unwrap();
            },
        );
        let ran = rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(ran - t0 >= Duration::from_millis(60));
        sched.shutdown();
    }
}
