pub(crate) async fn monitor_profile(_: &str) -> anyhow::Result<std::path::PathBuf> {
    // TODO(windows): discover the selected monitor's ICC profile through WCS.
    anyhow::bail!("Windows monitor profile discovery is not implemented; set AIGLE_DISPLAY_ICC")
}
