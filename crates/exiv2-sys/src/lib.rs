//! Safe wrappers over the aigle Exiv2 shim (`shim/aigle_exiv2.cpp`): read the XMP of a
//! sidecar or image, look up properties, and rewrite simple properties of a packet while
//! keeping every other property (doc 01 §9.2).
//!
//! Exiv2's XMP toolkit is not re-entrant, so every call takes one process-wide lock.

use std::ffi::{CStr, CString, c_char, c_int};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::sync::Mutex;

/// Namespace of aigle's own properties (`Xmp.aigle.*`).
pub const NS_URI: &str = "http://ns.aigle.invalid/1.0/";
pub const NS_PREFIX: &str = "aigle";

unsafe extern "C" {
    fn aigle_exiv2_init(ns_uri: *const c_char, ns_prefix: *const c_char) -> c_int;
    fn aigle_exiv2_free(p: *mut c_char);
    fn aigle_exiv2_read_file(
        path: *const c_char,
        out: *mut *mut c_char,
        err: *mut *mut c_char,
    ) -> c_int;
    fn aigle_exiv2_get(
        packet: *const c_char,
        n: usize,
        keys: *const *const c_char,
        values: *mut *mut c_char,
        err: *mut *mut c_char,
    ) -> c_int;
    fn aigle_exiv2_update(
        packet: *const c_char,
        n: usize,
        keys: *const *const c_char,
        values: *const *const c_char,
        out: *mut *mut c_char,
        err: *mut *mut c_char,
    ) -> c_int;
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct Error(pub String);

static LOCK: Mutex<Option<bool>> = Mutex::new(None);

/// Run `f` under the lock, initialising Exiv2 on first use.
fn locked<T>(f: impl FnOnce() -> Result<T, Error>) -> Result<T, Error> {
    let mut g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let ok = *g.get_or_insert_with(|| {
        let uri = CString::new(NS_URI).expect("no NUL");
        let prefix = CString::new(NS_PREFIX).expect("no NUL");
        unsafe { aigle_exiv2_init(uri.as_ptr(), prefix.as_ptr()) != 0 }
    });
    if !ok {
        return Err(Error("Exiv2 XMP support failed to initialise".into()));
    }
    f()
}

/// Take ownership of a shim string.
fn take(p: *mut c_char) -> Option<String> {
    if p.is_null() {
        return None;
    }
    let s = unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned();
    unsafe { aigle_exiv2_free(p) };
    Some(s)
}

fn check(rc: c_int, err: *mut c_char) -> Result<(), Error> {
    let msg = take(err);
    if rc == 0 {
        Ok(())
    } else {
        Err(Error(msg.unwrap_or_else(|| "Exiv2 error".into())))
    }
}

fn cstring(s: &str) -> Result<CString, Error> {
    CString::new(s).map_err(|_| Error("string contains NUL".into()))
}

/// The XMP packet of `path`: a sidecar's content or an image's embedded XMP; `None` if the
/// file has none.
pub fn read_file(path: &Path) -> Result<Option<String>, Error> {
    let c = CString::new(path.as_os_str().as_bytes()).map_err(|_| Error("path has NUL".into()))?;
    locked(|| {
        let mut out = std::ptr::null_mut();
        let mut err = std::ptr::null_mut();
        let rc = unsafe { aigle_exiv2_read_file(c.as_ptr(), &mut out, &mut err) };
        let packet = take(out);
        check(rc, err)?;
        Ok(packet)
    })
}

/// Values of `keys` (`Xmp.xmp.Rating`, `Xmp.aigle.Edits`, …) in `packet`, in order.
pub fn get(packet: &str, keys: &[&str]) -> Result<Vec<Option<String>>, Error> {
    let packet = cstring(packet)?;
    let keys = keys
        .iter()
        .map(|k| cstring(k))
        .collect::<Result<Vec<_>, _>>()?;
    let ptrs: Vec<*const c_char> = keys.iter().map(|k| k.as_ptr()).collect();
    locked(|| {
        let mut values = vec![std::ptr::null_mut(); keys.len()];
        let mut err = std::ptr::null_mut();
        let rc = unsafe {
            aigle_exiv2_get(
                packet.as_ptr(),
                ptrs.len(),
                ptrs.as_ptr(),
                values.as_mut_ptr(),
                &mut err,
            )
        };
        let values: Vec<Option<String>> = values.into_iter().map(take).collect();
        check(rc, err)?;
        Ok(values)
    })
}

/// `packet` (or an empty one) with each `(key, value)` set as a text property, or removed
/// when `value` is `None`. Every other property is kept.
pub fn update(packet: Option<&str>, changes: &[(&str, Option<&str>)]) -> Result<String, Error> {
    let packet = packet.map(cstring).transpose()?;
    let keys = changes
        .iter()
        .map(|(k, _)| cstring(k))
        .collect::<Result<Vec<_>, _>>()?;
    let values = changes
        .iter()
        .map(|(_, v)| v.map(cstring).transpose())
        .collect::<Result<Vec<_>, _>>()?;
    let key_ptrs: Vec<*const c_char> = keys.iter().map(|k| k.as_ptr()).collect();
    let value_ptrs: Vec<*const c_char> = values
        .iter()
        .map(|v| v.as_ref().map_or(std::ptr::null(), |v| v.as_ptr()))
        .collect();
    locked(|| {
        let mut out = std::ptr::null_mut();
        let mut err = std::ptr::null_mut();
        let rc = unsafe {
            aigle_exiv2_update(
                packet.as_ref().map_or(std::ptr::null(), |p| p.as_ptr()),
                key_ptrs.len(),
                key_ptrs.as_ptr(),
                value_ptrs.as_ptr(),
                &mut out,
                &mut err,
            )
        };
        let encoded = take(out);
        check(rc, err)?;
        encoded.ok_or_else(|| Error("Exiv2 returned no packet".into()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_keeps_foreign_properties() {
        let p = update(
            None,
            &[
                ("Xmp.xmp.Rating", Some("3")),
                ("Xmp.dc.format", Some("image/x-nikon-nef")),
            ],
        )
        .unwrap();
        let p = update(
            Some(&p),
            &[
                ("Xmp.xmp.Rating", Some("5")),
                ("Xmp.aigle.Edits", Some("{\"a\":1}")),
            ],
        )
        .unwrap();
        assert_eq!(
            get(
                &p,
                &[
                    "Xmp.xmp.Rating",
                    "Xmp.dc.format",
                    "Xmp.aigle.Edits",
                    "Xmp.xmp.Label"
                ]
            )
            .unwrap(),
            vec![
                Some("5".into()),
                Some("image/x-nikon-nef".into()),
                Some("{\"a\":1}".into()),
                None
            ]
        );
        assert!(p.contains(NS_URI));
        let p = update(Some(&p), &[("Xmp.xmp.Rating", None)]).unwrap();
        assert_eq!(get(&p, &["Xmp.xmp.Rating"]).unwrap(), vec![None]);
    }

    #[test]
    fn reads_a_sidecar_file() {
        let dir = std::env::temp_dir().join(format!("aigle-exiv2-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.xmp");
        let p = update(None, &[("Xmp.xmp.Label", Some("Red"))]).unwrap();
        std::fs::write(&path, &p).unwrap();
        let read = read_file(&path).unwrap().unwrap();
        assert_eq!(
            get(&read, &["Xmp.xmp.Label"]).unwrap(),
            vec![Some("Red".into())]
        );
        assert!(get("<not xmp", &["Xmp.xmp.Label"]).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
