//! Apply a hard committed-memory ceiling to an isolated Windows parser.
use std::{
    ffi::c_void,
    io,
    os::windows::io::{FromRawHandle, OwnedHandle},
};

#[repr(C)]
#[derive(Default)]
struct BasicLimits {
    process_time: i64,
    job_time: i64,
    flags: u32,
    min_working_set: usize,
    max_working_set: usize,
    active_processes: u32,
    affinity: usize,
    priority: u32,
    scheduling: u32,
}
#[repr(C)]
#[derive(Default)]
struct ExtendedLimits {
    basic: BasicLimits,
    io: [u64; 6],
    process_memory: usize,
    job_memory: usize,
    peak_process_memory: usize,
    peak_job_memory: usize,
}

#[allow(unsafe_code)]
pub(super) fn parser_memory_limit(bytes: u64) -> io::Result<OwnedHandle> {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateJobObjectW(attributes: *const c_void, name: *const u16) -> *mut c_void;
        fn SetInformationJobObject(
            job: *mut c_void,
            class: u32,
            info: *const c_void,
            size: u32,
        ) -> i32;
        fn AssignProcessToJobObject(job: *mut c_void, process: *mut c_void) -> i32;
        fn GetCurrentProcess() -> *mut c_void;
    }
    let limits = ExtendedLimits {
        basic: BasicLimits {
            flags: 0x100,
            ..Default::default()
        }, // JOB_OBJECT_LIMIT_PROCESS_MEMORY
        process_memory: usize::try_from(bytes)
            .map_err(|_| io::Error::other("parser memory limit exceeds address space"))?,
        ..Default::default()
    };
    // SAFETY: repr(C) structures match JOBOBJECT_EXTENDED_LIMIT_INFORMATION.
    // The unnamed job handle is uniquely owned and returned to the worker,
    // which retains it throughout serve(). All pointers live through each call.
    unsafe {
        let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        let job = OwnedHandle::from_raw_handle(handle);
        if SetInformationJobObject(
            handle,
            9,
            (&limits as *const ExtendedLimits).cast(),
            size_of::<ExtendedLimits>() as u32,
        ) == 0
            || AssignProcessToJobObject(handle, GetCurrentProcess()) == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(job)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn extended_limit_layout_matches_windows_x64() {
        #[cfg(target_pointer_width = "64")]
        assert_eq!(size_of::<super::ExtendedLimits>(), 144);
    }
}
