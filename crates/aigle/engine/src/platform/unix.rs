//! Unix backing storage for compute caches and spilled image buffers.
use half::f16;
use std::path::Path;

/// Free bytes on the filesystem of `path`.
#[allow(unsafe_code)]
pub(crate) fn free_space(path: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    // SAFETY: `statvfs` only writes the struct it is given; zeroed is a valid value for it.
    let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: valid NUL-terminated path and out-pointer.
    let rc = unsafe { libc::statvfs(c.as_ptr(), &mut s) };
    (rc == 0).then(|| s.f_bavail as u64 * s.f_frsize as u64)
}

/// Move `px` into an unlinked, memory-mapped file in `dir`.
#[allow(unsafe_code)]
pub(crate) fn map_file(dir: &Path, px: &[[f16; 3]]) -> std::io::Result<memmap2::MmapMut> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    std::fs::create_dir_all(dir)?;
    let path = dir.join(format!(
        "level-{}-{}.f16",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&path)?;
    let bytes: &[u8] = bytemuck::cast_slice(px);
    let res = (|| {
        file.set_len(bytes.len().max(1) as u64)?;
        // SAFETY: the file was created exclusively by this process (`create_new`) and is
        // unlinked below before anyone else can open it by name, so no other process or
        // mapping can modify it while this mapping lives.
        let mut m = unsafe {
            memmap2::MmapOptions::new()
                .len(bytes.len())
                .map_mut(&file)?
        };
        m.copy_from_slice(bytes);
        Ok(m)
    })();
    let _ = std::fs::remove_file(&path);
    res
}
