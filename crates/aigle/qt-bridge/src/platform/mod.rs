//! GUI desktop integration. Never imported by the headless engine.
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
mod colord;
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
pub(super) use colord::monitor_profile;
#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
pub(super) use windows::monitor_profile;
#[cfg(not(any(target_os = "linux", target_os = "freebsd", target_os = "windows")))]
pub(super) async fn monitor_profile(_: &str) -> anyhow::Result<std::path::PathBuf> {
    anyhow::bail!("Automatic monitor detection unavailable; set AIGLE_DISPLAY_ICC")
}
