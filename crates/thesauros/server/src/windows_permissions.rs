//! Private Windows objects, with inherited access for newly created children.
use std::{ffi::c_void, io, os::windows::ffi::OsStrExt, path::Path};

#[allow(unsafe_code)]
pub(crate) fn make_private(path: &Path) -> io::Result<()> {
    #[link(name = "advapi32")]
    unsafe extern "system" {
        fn ConvertStringSecurityDescriptorToSecurityDescriptorW(
            text: *const u16,
            revision: u32,
            descriptor: *mut *mut c_void,
            size: *mut u32,
        ) -> i32;
        fn SetFileSecurityW(path: *const u16, information: u32, descriptor: *const c_void) -> i32;
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn LocalFree(memory: *mut c_void) -> *mut c_void;
    }
    let mut name: Vec<_> = path.as_os_str().encode_wide().collect();
    if name.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "NUL in file path",
        ));
    }
    name.push(0);
    // Protected DACL: owner, administrators and SYSTEM only. Child objects
    // inherit these entries. Administrators retain recovery access, like root
    // on Unix. Existing descendants retain their own Windows ACLs.
    let sddl: Vec<_> = "D:P(A;OICI;FA;;;OW)(A;OICI;FA;;;BA)(A;OICI;FA;;;SY)"
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let mut descriptor = std::ptr::null_mut();
    // SAFETY: inputs are NUL-terminated UTF-16 buffers. The descriptor is
    // allocated by Windows, used synchronously, and freed with LocalFree.
    unsafe {
        if ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            1,
            &mut descriptor,
            std::ptr::null_mut(),
        ) == 0
        {
            return Err(io::Error::last_os_error());
        }
        let result = if SetFileSecurityW(name.as_ptr(), 0x8000_0004, descriptor) == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        };
        LocalFree(descriptor);
        result
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn private_objects_remain_accessible_to_the_owner() {
        let dir = tempfile::tempdir().unwrap();
        super::make_private(dir.path()).unwrap();
        let file = dir.path().join("token");
        std::fs::write(&file, b"secret").unwrap();
        super::make_private(&file).unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"secret");
    }
}
