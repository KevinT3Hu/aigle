//! XMP sidecars (doc 01 §9.1–§9.3).

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use aigle_core::EditParams;
use aigle_engine_api::{is_photo_path, is_raw_path};

/// Version of the `aigle:` properties written by this build.
pub const SCHEMA_VERSION: u32 = 1;

const RATING: &str = "Xmp.xmp.Rating";
const LABEL: &str = "Xmp.xmp.Label";
const SCHEMA: &str = "Xmp.aigle.SchemaVersion";
const EDITS: &str = "Xmp.aigle.Edits";

/// What aigle keeps per photo.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PhotoMeta {
    /// −1 (rejected) to 5; 0 is unrated.
    pub rating: i8,
    /// `xmp:Label`, e.g. `Red`.
    pub label: Option<String>,
    /// `None` for an unedited photo.
    pub edits: Option<EditParams>,
}

/// The result of [`Store::read`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Loaded {
    pub meta: PhotoMeta,
    /// The sidecar that was read, if there is one.
    pub sidecar: Option<PathBuf>,
    /// Problems that did not stop the read (unreadable edits, unknown schema version).
    pub warnings: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{}: {source}", path.display())]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("{}: {message}", path.display())]
    Xmp { path: PathBuf, message: String },
    /// The sidecar changed on disk since aigle last read or wrote it; nothing was written.
    #[error("{} was changed by another program", .0.display())]
    Conflict(PathBuf),
}

impl Error {
    /// The folder cannot be written (doc 01 §9.4).
    pub fn is_read_only(&self) -> bool {
        matches!(self, Error::Io { source, .. } if matches!(
            source.kind(),
            std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::ReadOnlyFilesystem
        ))
    }
}

fn io_err(path: &Path) -> impl FnOnce(std::io::Error) -> Error + '_ {
    move |source| Error::Io {
        path: path.to_path_buf(),
        source,
    }
}

fn xmp_err(path: &Path) -> impl FnOnce(aigle_exiv2_sys::Error) -> Error + '_ {
    move |e| Error::Xmp {
        path: path.to_path_buf(),
        message: e.0,
    }
}

/// The two sidecar names of `photo`, in read order: `IMG_0001.CR3.xmp`, `IMG_0001.xmp`.
pub fn sidecar_candidates(photo: &Path) -> [PathBuf; 2] {
    let mut full = photo.as_os_str().to_owned();
    full.push(".xmp");
    [PathBuf::from(full), photo.with_extension("xmp")]
}

/// Another photo in the same folder has the same stem (`IMG_0001.CR3` + `IMG_0001.JPG`).
fn has_sibling(photo: &Path) -> bool {
    let (Some(dir), Some(stem)) = (photo.parent(), photo.file_stem()) else {
        return false;
    };
    let Ok(rd) = std::fs::read_dir(dir) else {
        return false;
    };
    rd.flatten().any(|e| {
        let p = e.path();
        p != photo && p.file_stem() == Some(stem) && is_photo_path(&p)
    })
}

/// The sidecar to read (if one exists) and the one to write. The short name is shared by
/// photos with the same stem, so it is only attributed to a raw or to a photo without such
/// a sibling.
fn resolve(photo: &Path) -> (Option<PathBuf>, PathBuf) {
    let [full, short] = sidecar_candidates(photo);
    if full.is_file() {
        return (Some(full.clone()), full);
    }
    let sibling = has_sibling(photo);
    if short.is_file() && (is_raw_path(photo) || !sibling) {
        return (Some(short.clone()), short);
    }
    (None, if sibling { full } else { short })
}

struct Known {
    sidecar: PathBuf,
    /// Content hash of the sidecar as last read or written; `None` if it did not exist.
    hash: Option<blake3::Hash>,
    /// The stored edits could not be read; they are never overwritten.
    edits_locked: bool,
}

/// Reads and writes sidecars and remembers what it last saw of each, so that external
/// changes are never overwritten (doc 01 §9.3).
#[derive(Default)]
pub struct Store {
    known: Mutex<HashMap<PathBuf, Known>>,
}

fn read_bytes(path: &Path) -> Result<Option<Vec<u8>>, Error> {
    match std::fs::read(path) {
        Ok(b) => Ok(Some(b)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(io_err(path)(e)),
    }
}

fn parse_rating(s: &str) -> Option<i8> {
    let v: f64 = s.trim().parse().ok()?;
    Some(v.round().clamp(-1.0, 5.0) as i8)
}

impl Store {
    pub fn new() -> Store {
        Store::default()
    }

    /// Rating, label and edits of `photo`, from its sidecar, or from the XMP embedded in
    /// the photo when it has no sidecar (embedded XMP is never written).
    pub fn read(&self, photo: &Path) -> Result<Loaded, Error> {
        let (existing, target) = resolve(photo);
        let bytes = match &existing {
            Some(s) => read_bytes(s)?,
            None => None,
        };
        let hash = bytes.as_deref().map(blake3::hash);
        let (packet, source) = match &bytes {
            Some(b) => (
                Some(String::from_utf8_lossy(b).into_owned()),
                existing.clone(),
            ),
            None => {
                let embedded = match aigle_exiv2_sys::read_file(photo) {
                    Ok(p) => p,
                    Err(e) => {
                        tracing::debug!(path = %photo.display(), "no embedded XMP: {}", e.0);
                        None
                    }
                };
                (embedded, None)
            }
        };

        let mut loaded = Loaded {
            sidecar: existing.clone(),
            ..Loaded::default()
        };
        let mut edits_locked = false;
        if let Some(packet) = &packet {
            let at = source.as_deref().unwrap_or(photo);
            let v = aigle_exiv2_sys::get(packet, &[RATING, LABEL, SCHEMA, EDITS])
                .map_err(xmp_err(at))?;
            let [rating, label, schema, edits] = <[Option<String>; 4]>::try_from(v)
                .map_err(|_| xmp_err(at)(aigle_exiv2_sys::Error("short result".into())))?;
            loaded.meta.rating = rating.as_deref().and_then(parse_rating).unwrap_or(0);
            loaded.meta.label = label.filter(|l| !l.trim().is_empty());
            if source.is_some()
                && let Some(json) = edits
            {
                let version = schema.as_deref().and_then(|s| s.trim().parse::<u32>().ok());
                match parse_edits(&json, version) {
                    Ok(e) => loaded.meta.edits = Some(e),
                    Err(w) => {
                        edits_locked = true;
                        loaded.warnings.push(w);
                    }
                }
            }
        }

        self.known.lock().unwrap_or_else(|e| e.into_inner()).insert(
            photo.to_path_buf(),
            Known {
                sidecar: existing.unwrap_or(target),
                hash,
                edits_locked,
            },
        );
        Ok(loaded)
    }

    /// Write `meta` to the sidecar of `photo`, keeping every property aigle does not own.
    /// Returns the sidecar path, or `None` when there was nothing to store and no sidecar
    /// existed. Fails with [`Error::Conflict`] if the sidecar changed on disk since it was
    /// last read or written here.
    pub fn write(&self, photo: &Path, meta: &PhotoMeta) -> Result<Option<PathBuf>, Error> {
        let mut known = self.known.lock().unwrap_or_else(|e| e.into_inner());
        if !known.contains_key(photo) {
            drop(known);
            self.read(photo)?;
            known = self.known.lock().unwrap_or_else(|e| e.into_inner());
        }
        let k = known.get_mut(photo).expect("read above");
        let current = read_bytes(&k.sidecar)?;
        if current.as_deref().map(blake3::hash) != k.hash {
            return Err(Error::Conflict(k.sidecar.clone()));
        }

        let rating = (meta.rating != 0).then(|| meta.rating.to_string());
        let label = meta.label.as_deref().filter(|l| !l.is_empty());
        let edits = meta
            .edits
            .as_ref()
            .filter(|e| **e != EditParams::default())
            .map(serde_json::to_string)
            .transpose()
            .map_err(|e| Error::Xmp {
                path: k.sidecar.clone(),
                message: e.to_string(),
            })?;
        let schema = SCHEMA_VERSION.to_string();
        let mut changes: Vec<(&str, Option<&str>)> =
            vec![(RATING, rating.as_deref()), (LABEL, label)];
        if !k.edits_locked {
            changes.push((EDITS, edits.as_deref()));
            changes.push((SCHEMA, edits.as_ref().map(|_| schema.as_str())));
        }
        if current.is_none() && changes.iter().all(|(_, v)| v.is_none()) {
            return Ok(None);
        }

        let old = current.as_deref().map(String::from_utf8_lossy);
        let packet =
            aigle_exiv2_sys::update(old.as_deref(), &changes).map_err(xmp_err(&k.sidecar))?;
        write_atomic(&k.sidecar, packet.as_bytes())?;
        k.hash = Some(blake3::hash(packet.as_bytes()));
        Ok(Some(k.sidecar.clone()))
    }

    /// Forget what was read for `photo`; the next write reads the sidecar again first.
    pub fn forget(&self, photo: &Path) {
        self.known
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(photo);
    }
}

fn parse_edits(json: &str, version: Option<u32>) -> Result<EditParams, String> {
    match version {
        Some(v) if v > SCHEMA_VERSION => {
            return Err(format!(
                "edits were saved by a newer aigle (schema {v}); they are kept but not shown"
            ));
        }
        None => return Err("edits have no schema version; they are kept but not shown".into()),
        _ => {}
    }
    let mut p: EditParams =
        serde_json::from_str(json).map_err(|e| format!("edits are unreadable ({e}); kept"))?;
    p.sanitize()
        .map_err(|e| format!("edits are invalid ({e}); kept"))?;
    Ok(p)
}

/// Write a temp file in the same folder, sync it, rename it over `path` and sync the folder.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), Error> {
    let dir = path.parent().unwrap_or(Path::new("."));
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let tmp = dir.join(format!(".{name}.{}.tmp", std::process::id()));
    let r = (|| {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        std::fs::rename(&tmp, path)?;
        std::fs::File::open(dir)?.sync_all()
    })();
    if r.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    r.map_err(io_err(path))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn temp_dir(tag: &str) -> PathBuf {
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let d = std::env::temp_dir().join(format!("aigle-lib-{tag}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn edited() -> EditParams {
        let mut p = EditParams::default();
        p.set("light.exposure", 1.25).unwrap();
        p
    }

    #[test]
    fn round_trip_keeps_foreign_properties() {
        let dir = temp_dir("rt");
        let photo = dir.join("IMG_1.NEF");
        std::fs::write(&photo, b"raw").unwrap();
        let foreign =
            aigle_exiv2_sys::update(None, &[("Xmp.dc.format", Some("image/x-nikon-nef"))]).unwrap();
        std::fs::write(dir.join("IMG_1.xmp"), foreign).unwrap();

        let store = Store::new();
        let loaded = store.read(&photo).unwrap();
        assert_eq!(loaded.meta, PhotoMeta::default());
        assert_eq!(loaded.sidecar, Some(dir.join("IMG_1.xmp")));

        let meta = PhotoMeta {
            rating: 4,
            label: Some("Red".into()),
            edits: Some(edited()),
        };
        assert_eq!(
            store.write(&photo, &meta).unwrap(),
            Some(dir.join("IMG_1.xmp"))
        );
        let text = std::fs::read_to_string(dir.join("IMG_1.xmp")).unwrap();
        assert!(text.contains("image/x-nikon-nef"));
        assert_eq!(Store::new().read(&photo).unwrap().meta, meta);

        // Back to unedited and unrated: aigle's properties go, the foreign one stays.
        store.write(&photo, &PhotoMeta::default()).unwrap();
        let text = std::fs::read_to_string(dir.join("IMG_1.xmp")).unwrap();
        assert!(text.contains("image/x-nikon-nef"));
        assert!(!text.contains("Edits"));
        assert_eq!(
            Store::new().read(&photo).unwrap().meta,
            PhotoMeta::default()
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn nothing_to_store_creates_no_file() {
        let dir = temp_dir("empty");
        let photo = dir.join("a.dng");
        std::fs::write(&photo, b"raw").unwrap();
        let store = Store::new();
        assert_eq!(store.write(&photo, &PhotoMeta::default()).unwrap(), None);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn same_stem_photos_get_their_own_sidecars() {
        let dir = temp_dir("stem");
        let raw = dir.join("IMG_2.CR3");
        let jpg = dir.join("IMG_2.JPG");
        std::fs::write(&raw, b"raw").unwrap();
        std::fs::write(&jpg, b"jpg").unwrap();
        let store = Store::new();
        let rated = |r| PhotoMeta {
            rating: r,
            ..PhotoMeta::default()
        };
        assert_eq!(
            store.write(&raw, &rated(2)).unwrap(),
            Some(dir.join("IMG_2.CR3.xmp"))
        );
        assert_eq!(
            store.write(&jpg, &rated(3)).unwrap(),
            Some(dir.join("IMG_2.JPG.xmp"))
        );
        assert_eq!(store.read(&raw).unwrap().meta.rating, 2);
        assert_eq!(store.read(&jpg).unwrap().meta.rating, 3);

        // A Lightroom-style short sidecar belongs to the raw, not to the JPEG.
        let dir2 = temp_dir("stem2");
        let raw2 = dir2.join("IMG_3.CR3");
        let jpg2 = dir2.join("IMG_3.JPG");
        std::fs::write(&raw2, b"raw").unwrap();
        std::fs::write(&jpg2, b"jpg").unwrap();
        let short = aigle_exiv2_sys::update(None, &[(RATING, Some("5"))]).unwrap();
        std::fs::write(dir2.join("IMG_3.xmp"), short).unwrap();
        assert_eq!(store.read(&raw2).unwrap().meta.rating, 5);
        assert_eq!(store.read(&jpg2).unwrap().meta.rating, 0);
        std::fs::remove_dir_all(&dir).unwrap();
        std::fs::remove_dir_all(&dir2).unwrap();
    }

    #[test]
    fn external_changes_are_not_overwritten() {
        let dir = temp_dir("conflict");
        let photo = dir.join("b.nef");
        std::fs::write(&photo, b"raw").unwrap();
        let store = Store::new();
        let meta = PhotoMeta {
            rating: 1,
            ..PhotoMeta::default()
        };
        let sidecar = store.write(&photo, &meta).unwrap().unwrap();
        let other = aigle_exiv2_sys::update(None, &[(RATING, Some("5"))]).unwrap();
        std::fs::write(&sidecar, &other).unwrap();
        assert!(matches!(
            store.write(&photo, &meta),
            Err(Error::Conflict(p)) if p == sidecar
        ));
        assert_eq!(std::fs::read_to_string(&sidecar).unwrap(), other);
        // After reading again the write goes through.
        assert_eq!(store.read(&photo).unwrap().meta.rating, 5);
        store.write(&photo, &meta).unwrap();
        assert_eq!(store.read(&photo).unwrap().meta.rating, 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn unreadable_edits_are_kept() {
        let dir = temp_dir("future");
        let photo = dir.join("c.nef");
        std::fs::write(&photo, b"raw").unwrap();
        let packet = aigle_exiv2_sys::update(
            None,
            &[(SCHEMA, Some("999")), (EDITS, Some("{\"future\":true}"))],
        )
        .unwrap();
        std::fs::write(dir.join("c.xmp"), packet).unwrap();
        let store = Store::new();
        let loaded = store.read(&photo).unwrap();
        assert_eq!(loaded.meta.edits, None);
        assert_eq!(loaded.warnings.len(), 1);
        let meta = PhotoMeta {
            rating: 3,
            edits: Some(edited()),
            ..PhotoMeta::default()
        };
        store.write(&photo, &meta).unwrap();
        let text = std::fs::read_to_string(dir.join("c.xmp")).unwrap();
        assert!(text.contains("future"), "{text}");
        assert_eq!(store.read(&photo).unwrap().meta.rating, 3);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn ratings_parse_leniently() {
        assert_eq!(parse_rating("3"), Some(3));
        assert_eq!(parse_rating(" 4.0 "), Some(4));
        assert_eq!(parse_rating("-1"), Some(-1));
        assert_eq!(parse_rating("9"), Some(5));
        assert_eq!(parse_rating("x"), None);
    }
}
