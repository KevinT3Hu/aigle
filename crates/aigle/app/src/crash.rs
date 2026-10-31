//! Log file and crash reports, under `$XDG_STATE_HOME/aigle` (`~/.local/state/aigle`).
//!
//! * `logs/aigle-<UTC time>-<pid>.log`: one log per run, the newest [`LOGS_KEPT`] kept.
//!   Worker processes append to their app's log.
//! * `crashes/aigle-<UTC time>-<pid>.txt`: written by the panic hook (message, backtrace,
//!   the end of the log), or at the next start for a run that ended without a clean exit or
//!   a termination signal: a crash in native code, or the out-of-memory killer.
//! * `sessions/<pid>`: exists while a run is going; records its log, its crash reports and
//!   how it ended.

use std::backtrace::Backtrace;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

const LOGS_KEPT: usize = 10;
const REPORTS_KEPT: usize = 20;
/// Panic reports written by one run.
const REPORTS_PER_RUN: usize = 3;
const LOG_TAIL_LINES: usize = 200;
/// Tells a worker process which log to append to.
const LOG_ENV: &str = "AIGLE_LOG_FILE";

/// `$XDG_STATE_HOME/aigle`, else `~/.local/state/aigle`.
pub fn state_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_STATE_HOME")
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".local/state")))
        .map(|p| p.join("aigle"))
}

fn filter() -> EnvFilter {
    EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn,aigle=info".into())
}

/// `YYYYMMDD-HHMMSS` (UTC) of `t`.
fn stamp(t: SystemTime) -> String {
    let secs = t.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let (days, rem) = ((secs / 86_400) as i64, secs % 86_400);
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}{m:02}{d:02}-{:02}{:02}{:02}",
        rem / 3600,
        rem / 60 % 60,
        rem % 60
    )
}

/// Delete the oldest `aigle-*` files of `dir` beyond `keep` (names sort by time).
fn prune(dir: &Path, keep: usize) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut names: Vec<PathBuf> = rd
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with("aigle-"))
        })
        .collect();
    names.sort();
    let n = names.len().saturating_sub(keep);
    for p in &names[..n] {
        let _ = std::fs::remove_file(p);
    }
}

/// Log to stderr and to a new log file; returns the file's path.
pub fn init_logging() -> Option<PathBuf> {
    let file = state_dir().map(|d| d.join("logs")).and_then(|dir| {
        std::fs::create_dir_all(&dir).ok()?;
        prune(&dir, LOGS_KEPT - 1);
        let path = dir.join(format!(
            "aigle-{}-{}.log",
            stamp(SystemTime::now()),
            std::process::id()
        ));
        let f = File::create(&path).ok()?;
        Some((path, f))
    });
    let stderr = tracing_subscriber::fmt::layer().with_writer(std::io::stderr);
    let registry = tracing_subscriber::registry().with(filter()).with(stderr);
    match file {
        Some((path, f)) => {
            registry
                .with(
                    tracing_subscriber::fmt::layer()
                        .with_ansi(false)
                        .with_writer(Mutex::new(f)),
                )
                .init();
            tracing::info!(
                "aigle {} logging to {}",
                env!("CARGO_PKG_VERSION"),
                path.display()
            );
            Some(path)
        }
        None => {
            registry.init();
            tracing::warn!("no log file: cannot create it under the state directory");
            None
        }
    }
}

/// Logging of a worker process: stderr, and the app's log if it has one.
pub fn init_worker_logging() {
    let file = std::env::var_os(LOG_ENV).and_then(|p| OpenOptions::new().append(true).open(p).ok());
    let stderr = tracing_subscriber::fmt::layer().with_writer(std::io::stderr);
    let registry = tracing_subscriber::registry().with(filter()).with(stderr);
    match file {
        Some(f) => registry
            .with(
                tracing_subscriber::fmt::layer()
                    .with_ansi(false)
                    .with_writer(Mutex::new(f)),
            )
            .init(),
        None => registry.init(),
    }
}

/// The environment worker processes need to log to `log`.
pub fn worker_env(log: Option<&Path>) -> Vec<(std::ffi::OsString, std::ffi::OsString)> {
    log.map(|l| (LOG_ENV.into(), l.as_os_str().to_owned()))
        .into_iter()
        .collect()
}

fn tail(log: &Path, lines: usize) -> String {
    let Ok(text) = std::fs::read(log) else {
        return String::new();
    };
    let text = String::from_utf8_lossy(&text);
    let all: Vec<&str> = text.lines().collect();
    all[all.len().saturating_sub(lines)..].join("\n")
}

fn system() -> String {
    let read = |p: &str| {
        std::fs::read_to_string(p)
            .map(|s| s.trim().to_string())
            .unwrap_or_default()
    };
    let var = |k: &str| std::env::var(k).unwrap_or_default();
    format!(
        "kernel: {} {}\ndesktop: {} (session {}, Wayland display {})",
        read("/proc/sys/kernel/ostype"),
        read("/proc/sys/kernel/osrelease"),
        var("XDG_CURRENT_DESKTOP"),
        var("XDG_SESSION_TYPE"),
        if std::env::var_os("WAYLAND_DISPLAY").is_some() {
            "set"
        } else {
            "unset"
        },
    )
}

/// Write a crash report about process `pid` (whose log is `log`); returns its path.
fn write_report(pid: u32, what: &str, log: Option<&Path>) -> Option<PathBuf> {
    let dir = state_dir()?.join("crashes");
    std::fs::create_dir_all(&dir).ok()?;
    prune(&dir, REPORTS_KEPT - 1);
    let now = stamp(SystemTime::now());
    let mut path = dir.join(format!("aigle-{now}-{pid}.txt"));
    let mut n = 1;
    while path.exists() {
        n += 1;
        path = dir.join(format!("aigle-{now}-{pid}-{n}.txt"));
    }
    let mut text = format!(
        "aigle crash report\nversion: {}\nwritten: {now} UTC\nprocess: {pid}\n{}\n\n{what}\n",
        env!("CARGO_PKG_VERSION"),
        system()
    );
    if let Some(log) = log {
        text.push_str(&format!(
            "\nend of the log ({}):\n{}\n",
            log.display(),
            tail(log, LOG_TAIL_LINES)
        ));
    }
    std::fs::write(&path, text).ok()?;
    Some(path)
}

fn sessions_dir() -> Option<PathBuf> {
    state_dir().map(|d| d.join("sessions"))
}

fn append(file: &Path, line: &str) {
    if let Ok(mut f) = OpenOptions::new().append(true).open(file) {
        let _ = writeln!(f, "{line}");
    }
}

/// Whether `pid` is a running aigle.
fn alive(pid: u32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/comm")).is_ok_and(|c| c.starts_with("aigle"))
}

/// Crash reports of earlier runs not shown yet; writes one for each run that ended without
/// a clean exit, a termination signal or a report of its own.
pub fn previous_reports() -> Vec<PathBuf> {
    let Some(dir) = sessions_dir() else {
        return Vec::new();
    };
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let me = std::process::id();
    let mut out = Vec::new();
    for e in rd.filter_map(Result::ok) {
        let Some(pid) = e.file_name().to_str().and_then(|n| n.parse::<u32>().ok()) else {
            continue;
        };
        if pid == me || alive(pid) {
            continue;
        }
        let text = std::fs::read_to_string(e.path()).unwrap_or_default();
        let mut log = None;
        let mut ended = false;
        let mut reports = Vec::new();
        for line in text.lines() {
            match line.split_once(' ') {
                Some(("log", p)) => log = Some(PathBuf::from(p)),
                Some(("report", p)) => reports.push(PathBuf::from(p)),
                Some(("signal", _)) => ended = true,
                _ if line == "clean" => ended = true,
                _ => {}
            }
        }
        if reports.is_empty() && !ended {
            let what = format!(
                "aigle quit unexpectedly. No Rust panic was recorded, so it most likely \
                 crashed in native code (a C/C++ library or the graphics driver) or was killed, \
                 for example by the out-of-memory killer. The system's crash handler \
                 (`coredumpctl info {pid}` with systemd-coredump, or /var/crash with apport) \
                 may have more."
            );
            tracing::warn!(pid, "the previous run of aigle quit unexpectedly");
            reports.extend(write_report(pid, &what, log.as_deref()));
        }
        let _ = std::fs::remove_file(e.path());
        out.extend(reports.into_iter().filter(|r| r.exists()));
    }
    out
}

/// This run's session file; removed on a clean exit.
pub struct Session {
    file: Option<PathBuf>,
}

static SESSION: OnceLock<(Option<PathBuf>, Option<PathBuf>)> = OnceLock::new();

impl Session {
    /// Record the run, and install the panic hook and the termination-signal handler.
    pub fn start(log: Option<PathBuf>) -> Session {
        let file = sessions_dir().and_then(|dir| {
            std::fs::create_dir_all(&dir).ok()?;
            let file = dir.join(std::process::id().to_string());
            let line = log
                .as_ref()
                .map(|l| format!("log {}\n", l.display()))
                .unwrap_or_default();
            std::fs::write(&file, line).ok()?;
            Some(file)
        });
        let _ = SESSION.set((file.clone(), log));
        install_panic_hook();
        watch_signals();
        Session { file }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if std::thread::panicking() {
            return;
        }
        if let Some(f) = &self.file {
            let reported = std::fs::read_to_string(f)
                .is_ok_and(|t| t.lines().any(|l| l.starts_with("report ")));
            if reported {
                append(f, "clean");
            } else {
                let _ = std::fs::remove_file(f);
            }
        }
    }
}

fn install_panic_hook() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        static WRITTEN: Mutex<usize> = Mutex::new(0);
        let thread = std::thread::current()
            .name()
            .unwrap_or("unnamed")
            .to_string();
        let msg = aigle_isolate::panic_message(info.payload());
        let at = info
            .location()
            .map(|l| l.to_string())
            .unwrap_or_else(|| "unknown location".into());
        tracing::error!(thread, "panic at {at}: {msg}");
        let mut written = WRITTEN.lock().unwrap_or_else(|e| e.into_inner());
        if *written < REPORTS_PER_RUN
            && let Some((file, log)) = SESSION.get()
        {
            *written += 1;
            let what = format!(
                "panic in thread '{thread}' at {at}:\n{msg}\n\nbacktrace:\n{}",
                Backtrace::force_capture()
            );
            if let Some(report) = write_report(std::process::id(), &what, log.as_deref()) {
                if let Some(f) = file {
                    append(f, &format!("report {}", report.display()));
                }
                eprintln!("aigle: crash report saved to {}", report.display());
            }
        }
        drop(written);
        default(info);
    }));
}

/// SIGTERM, SIGINT and SIGHUP end the run without a crash report.
fn watch_signals() {
    use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM};
    let mut signals = match signal_hook::iterator::Signals::new([SIGTERM, SIGINT, SIGHUP]) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!("cannot watch termination signals: {e}");
            return;
        }
    };
    let _ = std::thread::Builder::new()
        .name("signals".into())
        .spawn(move || {
            if let Some(sig) = signals.forever().next() {
                tracing::info!("terminated by signal {sig}");
                if let Some((Some(f), _)) = SESSION.get() {
                    append(f, &format!("signal {sig}"));
                }
                let _ = signal_hook::low_level::emulate_default_handler(sig);
                std::process::exit(128 + sig);
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stamps_are_utc_calendar_times() {
        assert_eq!(stamp(UNIX_EPOCH), "19700101-000000");
        let t = UNIX_EPOCH + std::time::Duration::from_secs(1_793_284_212);
        assert_eq!(stamp(t), "20261029-143012");
        let leap = UNIX_EPOCH + std::time::Duration::from_secs(951_782_400);
        assert_eq!(stamp(leap), "20000229-000000");
    }
}
