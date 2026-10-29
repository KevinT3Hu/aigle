//! Safe wrappers over the aigle Exiv2 shim (`shim/aigle_exiv2.cpp`): read the XMP of a
//! sidecar or image, look up properties, rewrite text, array and language-alternative
//! properties of a packet while keeping every other property (doc 01 §9.2), and read the
//! camera, lens and capture date from an image's EXIF.
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
    fn aigle_exiv2_read_facts(
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
        kinds: *const c_int,
        values: *const *const c_char,
        out: *mut *mut c_char,
        err: *mut *mut c_char,
    ) -> c_int;
}

/// Separates the items of an array property in [`get`] and [`update`] values.
pub const ITEM_SEP: char = '\u{1f}';

/// The XMP form of a property written by [`update`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Text = 0,
    /// Unordered array (`dc:subject`); items separated by [`ITEM_SEP`].
    Bag = 1,
    /// Ordered array (`dc:creator`); items separated by [`ITEM_SEP`].
    Seq = 2,
    /// Language alternative (`dc:title`): sets the `x-default` text, keeps other languages.
    LangAlt = 3,
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

/// What the EXIF of an image says about how it was taken; fields are `None` when unknown.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExifFacts {
    pub make: Option<String>,
    pub model: Option<String>,
    /// Exiv2's lens name, decoded from maker notes where needed.
    pub lens: Option<String>,
    /// `DateTimeOriginal` as recorded (`YYYY:MM:DD HH:MM:SS`).
    pub date_time: Option<String>,
}

/// Camera, lens and capture date from the EXIF of the image at `path`.
pub fn read_facts(path: &Path) -> Result<ExifFacts, Error> {
    let c = CString::new(path.as_os_str().as_bytes()).map_err(|_| Error("path has NUL".into()))?;
    locked(|| {
        let mut out = [std::ptr::null_mut(); 4];
        let mut err = std::ptr::null_mut();
        let rc = unsafe { aigle_exiv2_read_facts(c.as_ptr(), out.as_mut_ptr(), &mut err) };
        let [make, model, lens, date_time] = out.map(|p| {
            take(p)
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        });
        check(rc, err)?;
        Ok(ExifFacts {
            make,
            model,
            lens,
            date_time,
        })
    })
}

/// Values of `keys` (`Xmp.xmp.Rating`, `Xmp.aigle.Edits`, …) in `packet`, in order. The
/// items of an array are joined with [`ITEM_SEP`]; a language alternative gives its
/// `x-default` text.
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
    let typed: Vec<(&str, Kind, Option<&str>)> =
        changes.iter().map(|&(k, v)| (k, Kind::Text, v)).collect();
    update_typed(packet, &typed)
}

/// Like [`update`], writing each property in the given [`Kind`].
pub fn update_typed(
    packet: Option<&str>,
    changes: &[(&str, Kind, Option<&str>)],
) -> Result<String, Error> {
    let packet = packet.map(cstring).transpose()?;
    let keys = changes
        .iter()
        .map(|(k, _, _)| cstring(k))
        .collect::<Result<Vec<_>, _>>()?;
    let kinds: Vec<c_int> = changes.iter().map(|(_, k, _)| *k as c_int).collect();
    let values = changes
        .iter()
        .map(|(_, _, v)| v.map(cstring).transpose())
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
                kinds.as_ptr(),
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
    fn arrays_and_language_alternatives() {
        let sep = ITEM_SEP.to_string();
        let keywords = ["sea", "boat, red"].join(&sep);
        let p = update_typed(
            None,
            &[
                ("Xmp.dc.subject", Kind::Bag, Some(&keywords)),
                ("Xmp.dc.creator", Kind::Seq, Some("Jane")),
                ("Xmp.dc.title", Kind::LangAlt, Some("Harbour")),
            ],
        )
        .unwrap();
        assert!(p.contains("rdf:Bag") && p.contains("rdf:Seq") && p.contains("rdf:Alt"));
        let keys = ["Xmp.dc.subject", "Xmp.dc.creator", "Xmp.dc.title"];
        assert_eq!(
            get(&p, &keys).unwrap(),
            vec![
                Some(keywords.clone()),
                Some("Jane".into()),
                Some("Harbour".into())
            ]
        );

        // A title in two languages: only x-default changes.
        let fr = p.replace(
            "<rdf:li xml:lang=\"x-default\">Harbour</rdf:li>",
            "<rdf:li xml:lang=\"x-default\">Harbour</rdf:li><rdf:li xml:lang=\"fr\">Port</rdf:li>",
        );
        assert_ne!(fr, p);
        let q = update_typed(Some(&fr), &[("Xmp.dc.title", Kind::LangAlt, Some("Dock"))]).unwrap();
        assert_eq!(
            get(&q, &["Xmp.dc.title"]).unwrap(),
            vec![Some("Dock".into())]
        );
        assert!(q.contains("Port"), "{q}");
        let q = update_typed(Some(&q), &[("Xmp.dc.subject", Kind::Bag, None)]).unwrap();
        assert_eq!(get(&q, &["Xmp.dc.subject"]).unwrap(), vec![None]);
    }

    /// A JPEG with only an EXIF segment: Make, Model and DateTimeOriginal.
    fn exif_jpeg() -> Vec<u8> {
        let (make, model, date) = (
            &b"Canon\0"[..],
            &b"Canon EOS 6D\0"[..],
            &b"2017:01:08 11:18:13\0"[..],
        );
        let data = 8 + 42 + 18;
        let mut t: Vec<u8> = b"II*\0".to_vec();
        t.extend(8u32.to_le_bytes());
        let entry = |t: &mut Vec<u8>, tag: u16, ty: u16, n: u32, v: u32| {
            t.extend(tag.to_le_bytes());
            t.extend(ty.to_le_bytes());
            t.extend(n.to_le_bytes());
            t.extend(v.to_le_bytes());
        };
        t.extend(3u16.to_le_bytes());
        entry(&mut t, 0x010f, 2, make.len() as u32, data);
        entry(
            &mut t,
            0x0110,
            2,
            model.len() as u32,
            data + make.len() as u32,
        );
        entry(&mut t, 0x8769, 4, 1, 8 + 42);
        t.extend(0u32.to_le_bytes());
        t.extend(1u16.to_le_bytes());
        let at = data + (make.len() + model.len()) as u32;
        entry(&mut t, 0x9003, 2, date.len() as u32, at);
        t.extend(0u32.to_le_bytes());
        for d in [make, model, date] {
            t.extend(d);
        }
        let mut app1 = b"Exif\0\0".to_vec();
        app1.extend(t);
        let mut j = vec![0xff, 0xd8, 0xff, 0xe1];
        j.extend(((app1.len() + 2) as u16).to_be_bytes());
        j.extend(app1);
        j.extend([0xff, 0xd9]);
        j
    }

    #[test]
    fn reads_exif_facts() {
        let dir = std::env::temp_dir().join(format!("aigle-exiv2-facts-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.jpg");
        std::fs::write(&path, exif_jpeg()).unwrap();
        assert_eq!(
            read_facts(&path).unwrap(),
            ExifFacts {
                make: Some("Canon".into()),
                model: Some("Canon EOS 6D".into()),
                lens: None,
                date_time: Some("2017:01:08 11:18:13".into()),
            }
        );
        std::fs::write(&path, b"not an image").unwrap();
        assert!(read_facts(&path).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
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
