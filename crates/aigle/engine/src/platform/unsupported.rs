//! Resource fallbacks until native platform backends are implemented.
pub fn physical_ram() -> u64 {
    // TODO(windows): query physical memory with GlobalMemoryStatusEx.
    16 * crate::budget::GIB
}
pub(crate) fn cgroup_limit() -> Option<u64> {
    None
}
pub(crate) fn memory_pressure() -> Option<String> {
    // TODO(windows): implement native memory-pressure notifications.
    None
}
#[cfg(not(unix))]
pub(crate) fn free_space(_: &std::path::Path) -> Option<u64> {
    // TODO(windows): query available bytes with GetDiskFreeSpaceExW.
    None
}
#[cfg(not(unix))]
pub(crate) fn map_file(
    _: &std::path::Path,
    _: &[[half::f16; 3]],
) -> std::io::Result<memmap2::MmapMut> {
    // TODO(windows): use delete-on-close backing files; Unix unlink semantics do not apply.
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "image-buffer spilling is not implemented on this platform",
    ))
}
