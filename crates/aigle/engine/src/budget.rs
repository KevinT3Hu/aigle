//! Engine memory budget (doc 03 §2.1).
//!
//! The configured budget `B` comes from the formula (or an override), capped below a cgroup
//! limit. A [`PressureMonitor`] thread polls Linux PSI (`/proc/pressure/memory`): while the
//! "some" stall share over 10 s exceeds [`PSI_ENTER`] %, the ledger's effective budget is
//! [`crate::ledger::PRESSURE_SHARE`] of `B` and the cache is trimmed to it; it returns to `B`
//! once the share stays below [`PSI_LEAVE`] %. The GPU's own budget (`VK_EXT_memory_budget`)
//! is read by the GPU backend.

use std::sync::{Arc, Weak};
use std::time::Duration;

use aigle_engine_api::{EngineEvent, ProcessKind, broadcast};

use crate::cache::TileCache;
use crate::ledger::MemLedger;

pub const GIB: u64 = 1 << 30;

/// `clamp(0.25 × RAM, 3, 24) GiB` for the GUI, `clamp(0.5 × RAM, 3, 48) GiB` for the CLI.
pub fn budget_for(ram: u64, kind: ProcessKind) -> u64 {
    match kind {
        ProcessKind::Gui => (ram / 4).clamp(3 * GIB, 24 * GIB),
        ProcessKind::Cli => (ram / 2).clamp(3 * GIB, 48 * GIB),
    }
}

/// Effective budget: override, else the formula, capped below a cgroup memory limit.
pub fn engine_budget(kind: ProcessKind, override_bytes: Option<u64>) -> u64 {
    let b = override_bytes.unwrap_or_else(|| budget_for(physical_ram(), kind));
    match cgroup_limit() {
        Some(limit) => b.min(limit / 10 * 8),
        None => b,
    }
}

/// Physical RAM from `/proc/meminfo`; 16 GiB (the minimum spec) if unavailable.
pub fn physical_ram() -> u64 {
    std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("MemTotal:"))
                .and_then(|l| {
                    l.split_whitespace()
                        .nth(1)
                        .and_then(|kb| kb.parse::<u64>().ok())
                        .map(|kb| kb * 1024)
                })
        })
        .unwrap_or(16 * GIB)
}

fn cgroup_limit() -> Option<u64> {
    ["/sys/fs/cgroup/memory.high", "/sys/fs/cgroup/memory.max"]
        .iter()
        .filter_map(|p| std::fs::read_to_string(p).ok())
        .filter_map(|s| s.trim().parse::<u64>().ok())
        .min()
}

/// PSI "some avg10" share (%) that enters the pressure state.
pub const PSI_ENTER: f64 = 10.0;
/// Share below which the pressure state is left (hysteresis).
pub const PSI_LEAVE: f64 = 5.0;
const PSI_PATH: &str = "/proc/pressure/memory";
const POLL: Duration = Duration::from_secs(1);

/// `avg10` of the `some` line of a PSI file.
pub fn parse_psi_some_avg10(text: &str) -> Option<f64> {
    let line = text.lines().find(|l| l.starts_with("some "))?;
    line.split_whitespace()
        .find_map(|f| f.strip_prefix("avg10="))
        .and_then(|v| v.parse().ok())
}

/// Pressure state with hysteresis.
#[derive(Default)]
pub struct PressureState {
    pub on: bool,
}

impl PressureState {
    /// Feed one sample; returns the new state.
    pub fn update(&mut self, avg10: f64) -> bool {
        if !self.on && avg10 > PSI_ENTER {
            self.on = true;
        } else if self.on && avg10 < PSI_LEAVE {
            self.on = false;
        }
        self.on
    }
}

/// Polls PSI and the ledger; reports `EngineEvent::MemoryPressure` when pressure starts and
/// when the pinned working set exceeds the effective budget (doc 03 §5.3, "low memory").
pub struct PressureMonitor {
    ledger: Arc<MemLedger>,
    cache: Weak<TileCache>,
    events: broadcast::Sender<EngineEvent>,
    state: PressureState,
    over: bool,
}

impl PressureMonitor {
    /// Start the monitor thread; it exits when the cache (the engine) is dropped. Without
    /// PSI (non-Linux, old kernels, some containers) only the over-budget check runs.
    pub fn spawn(
        ledger: Arc<MemLedger>,
        cache: Weak<TileCache>,
        events: broadcast::Sender<EngineEvent>,
    ) {
        let mut m = PressureMonitor {
            ledger,
            cache,
            events,
            state: PressureState::default(),
            over: false,
        };
        let _ = std::thread::Builder::new()
            .name("aigle-memory".into())
            .spawn(move || {
                while m.tick(std::fs::read_to_string(PSI_PATH).ok().as_deref()) {
                    std::thread::sleep(POLL);
                }
            });
    }

    /// One poll with the PSI file contents. Returns false once the engine is gone.
    fn tick(&mut self, psi: Option<&str>) -> bool {
        let Some(cache) = self.cache.upgrade() else {
            return false;
        };
        if let Some(avg) = psi.and_then(parse_psi_some_avg10) {
            let on = self.state.update(avg);
            if self.ledger.set_pressure(on) {
                if on {
                    tracing::warn!(avg10 = avg, "memory pressure: shrinking the engine budget");
                    cache.trim();
                    self.emit();
                } else {
                    tracing::info!(avg10 = avg, "memory pressure cleared");
                }
            }
        }
        let over = self.ledger.used() > self.ledger.effective();
        if over && !self.over {
            cache.trim();
            if self.ledger.used() > self.ledger.effective() {
                tracing::warn!("pinned working set exceeds the memory budget");
                self.emit();
            }
        }
        self.over = over;
        true
    }

    fn emit(&self) {
        let _ = self.events.send(EngineEvent::MemoryPressure {
            used: self.ledger.used(),
            budget: self.ledger.effective(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn psi_parse_and_hysteresis() {
        let t = "some avg10=12.50 avg60=3.00 avg300=1.00 total=123\nfull avg10=1.00 avg60=0.00 avg300=0.00 total=4\n";
        assert_eq!(parse_psi_some_avg10(t), Some(12.5));
        assert_eq!(parse_psi_some_avg10("garbage"), None);
        let mut s = PressureState::default();
        assert!(!s.update(8.0));
        assert!(s.update(12.0));
        assert!(s.update(7.0), "stays on above the leave threshold");
        assert!(!s.update(4.0));
    }

    #[test]
    fn monitor_shrinks_budget_and_reports() {
        let ledger = MemLedger::new(1000);
        let cache = Arc::new(TileCache::new(ledger.clone(), 0.5));
        let (tx, mut rx) = broadcast::channel(8);
        let mut m = PressureMonitor {
            ledger: ledger.clone(),
            cache: Arc::downgrade(&cache),
            events: tx,
            state: PressureState::default(),
            over: false,
        };
        let hot = "some avg10=30.00 avg60=0 avg300=0 total=0";
        let calm = "some avg10=0.00 avg60=0 avg300=0 total=0";
        assert!(m.tick(Some(hot)));
        assert_eq!(ledger.effective(), 750);
        assert!(matches!(
            rx.try_recv(),
            Ok(EngineEvent::MemoryPressure { budget: 750, .. })
        ));
        // A pinned allocation over the budget is reported once.
        let a = ledger.alloc(crate::ledger::MemClass::SourceCpu, 800);
        m.tick(Some(hot));
        m.tick(Some(hot));
        assert!(rx.try_recv().is_ok());
        assert!(rx.try_recv().is_err());
        drop(a);
        m.tick(Some(calm));
        assert_eq!(ledger.effective(), 1000);
        drop(cache);
        assert!(!m.tick(None), "exits with the engine");
    }

    #[test]
    fn formula_matches_doc() {
        assert_eq!(budget_for(16 * GIB, ProcessKind::Gui), 4 * GIB);
        assert_eq!(budget_for(32 * GIB, ProcessKind::Gui), 8 * GIB);
        assert_eq!(budget_for(32 * GIB, ProcessKind::Cli), 16 * GIB);
        assert_eq!(budget_for(8 * GIB, ProcessKind::Gui), 3 * GIB);
        assert_eq!(budget_for(256 * GIB, ProcessKind::Gui), 24 * GIB);
    }
}
