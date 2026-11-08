use std::path::PathBuf;
pub(super) fn cache_dir() -> Option<PathBuf> {
    // TODO(windows): resolve the application's LocalAppData cache directory.
    // AIGLE_CACHE_DIR remains an explicit override.
    None
}
pub(super) fn data_dir() -> Option<PathBuf> {
    // TODO(windows): resolve the application's native persistent data directory.
    // AIGLE_DATA_DIR remains an explicit override.
    None
}
