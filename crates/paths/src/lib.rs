//! Application directory policy, kept outside the core image engine.
use std::path::PathBuf;
#[cfg(unix)]
#[path = "xdg.rs"]
mod platform;
#[cfg(not(unix))]
#[path = "unsupported.rs"]
mod platform;

/// Disposable cache dir (doc 03 §7): `$AIGLE_CACHE_DIR`, else `$XDG_CACHE_HOME/aigle`, else
/// `~/.cache/aigle`.
pub fn cache_dir() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("AIGLE_CACHE_DIR") {
        return Some(PathBuf::from(d));
    }
    platform::cache_dir()
}

pub fn data_dir() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("AIGLE_DATA_DIR") {
        return Some(PathBuf::from(d));
    }
    platform::data_dir()
}
