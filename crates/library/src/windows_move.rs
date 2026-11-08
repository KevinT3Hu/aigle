//! Atomic Windows renames that refuse to replace an existing destination.
use std::{io, os::windows::ffi::OsStrExt, path::Path};

#[allow(unsafe_code)]
pub(crate) fn rename(from: &Path, to: &Path) -> io::Result<()> {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(from: *const u16, to: *const u16, flags: u32) -> i32;
    }
    fn wide(path: &Path) -> io::Result<Vec<u16>> {
        let mut value: Vec<_> = path.as_os_str().encode_wide().collect();
        if value.contains(&0) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "NUL in file path",
            ));
        }
        value.push(0);
        Ok(value)
    }
    let from = wide(from)?;
    let to = wide(to)?;
    // SAFETY: both buffers are NUL-terminated and live through the call. Zero
    // flags prohibit overwriting and copying between volumes; callers handle
    // ERROR_NOT_SAME_DEVICE with their verified copy path.
    if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), 0) } == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn moves_unicode_paths_without_replacing_existing_files() {
        let dir = crate::sidecar::tests::temp_dir("windows-rename");
        let from = dir.join("原图.dng");
        let to = dir.join("目标.dng");
        std::fs::write(&from, b"source").unwrap();
        std::fs::write(&to, b"existing").unwrap();
        assert_eq!(
            super::rename(&from, &to).unwrap_err().kind(),
            std::io::ErrorKind::AlreadyExists
        );
        assert_eq!(std::fs::read(&from).unwrap(), b"source");
        assert_eq!(std::fs::read(&to).unwrap(), b"existing");
        std::fs::remove_file(&to).unwrap();
        super::rename(&from, &to).unwrap();
        assert!(!from.exists());
        assert_eq!(std::fs::read(&to).unwrap(), b"source");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
