//! Pinned external data (doc 02 §13.2): content-addressed by a BLAKE3 hash of the canonical
//! JSON, held in memory and persisted under `<data dir>/<kind>/<hash>.json`, where the data
//! dir is `$AIGLE_DATA_DIR`, else `$XDG_DATA_HOME/aigle/data`, else
//! `~/.local/share/aigle/data`.
//!
//! An edit references data only by `DataRef`, so updating or removing the lensfun database
//! never changes an existing edit. A missing item is reported, not silently replaced.
//!
//! **Reference data** (camconst white levels, the lensfun database) ships with aigle as plain files, read at runtime, never compiled in. Their
//! upstream, pinned revision and licence are recorded in `data/manifest.toml` (refreshed by
//! `cargo xtask data --update`). Each item is looked up, first hit wins, as
//! 1. its own environment variable (e.g. `$AIGLE_CAMCONST`),
//! 2. the same relative path under the user data dir (a user-updated copy, e.g.
//!    `~/.local/share/aigle/data/camconst/camconst.json`),
//! 3. the bundled data dir ([`bundled_dir`]).
//!
//! Nothing is read from other applications' installations.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use aigle_core::DataRef;
use aigle_core::lens::LensProfileData;
use parking_lot::Mutex;

/// The bundled reference data: the first of `$AIGLE_BUNDLED_DATA`, `<exe dir>/../share/aigle`
/// (installed layout), `<exe dir>/data`, and the source tree's `data/` that contains
/// `manifest.toml`.
pub fn bundled_dir() -> Option<&'static Path> {
    static D: OnceLock<Option<PathBuf>> = OnceLock::new();
    D.get_or_init(|| {
        let mut c: Vec<PathBuf> = Vec::new();
        if let Some(d) = std::env::var_os("AIGLE_BUNDLED_DATA") {
            c.push(d.into());
        }
        if let Some(exe) = std::env::current_exe().ok().and_then(|e| {
            e.canonicalize()
                .ok()
                .and_then(|e| e.parent().map(Path::to_path_buf))
        }) {
            c.push(exe.join("../share/aigle"));
            c.push(exe.join("data"));
        }
        c.push(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data"));
        c.into_iter()
            .find(|d| d.join("manifest.toml").is_file())
            .map(|d| d.canonicalize().unwrap_or(d))
    })
    .as_deref()
}

/// A reference data file or directory `rel` (relative to the data roots), overridable by
/// `env` (see the module docs).
pub fn reference(env: &str, rel: &str) -> Option<PathBuf> {
    if let Some(p) = std::env::var_os(env) {
        return Some(p.into());
    }
    [dir(), bundled_dir().map(Path::to_path_buf)]
        .into_iter()
        .flatten()
        .map(|d| d.join(rel))
        .find(|p| p.exists())
}

/// Disposable cache dir (doc 03 §7): `$AIGLE_CACHE_DIR`, else `$XDG_CACHE_HOME/aigle`, else
/// `~/.cache/aigle`.
pub(crate) fn cache_dir() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("AIGLE_CACHE_DIR") {
        return Some(PathBuf::from(d));
    }
    let home = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))?;
    Some(home.join("aigle"))
}

pub(crate) fn dir() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("AIGLE_DATA_DIR") {
        return Some(PathBuf::from(d));
    }
    let home = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))?;
    Some(home.join("aigle/data"))
}

fn lenses() -> &'static Mutex<HashMap<DataRef, Arc<LensProfileData>>> {
    static M: OnceLock<Mutex<HashMap<DataRef, Arc<LensProfileData>>>> = OnceLock::new();
    M.get_or_init(Default::default)
}

/// Content hash of a lens profile.
pub fn lens_ref(p: &LensProfileData) -> DataRef {
    let json = serde_json::to_vec(p).expect("lens profile serialises");
    let mut h = blake3::Hasher::new();
    h.update(b"aigle.lens.v1\0");
    h.update(&json);
    DataRef(h.finalize().to_hex().to_string())
}

/// Pin a lens profile: remember it and write it to the data dir (best effort).
pub fn pin_lens(p: &LensProfileData) -> DataRef {
    let r = lens_ref(p);
    let mut m = lenses().lock();
    if !m.contains_key(&r) {
        if let Some(d) = dir() {
            let path = d.join("lens").join(format!("{}.json", r.as_str()));
            if !path.exists() {
                let res =
                    std::fs::create_dir_all(path.parent().expect("has parent")).and_then(|_| {
                        crate::encode::write_atomic(
                            &path,
                            &serde_json::to_vec_pretty(p).expect("serialises"),
                        )
                        .map_err(std::io::Error::other)
                    });
                if let Err(e) = res {
                    tracing::warn!(path = %path.display(), "cannot persist lens profile: {e}");
                }
            }
        }
        m.insert(r.clone(), Arc::new(p.clone()));
    }
    r
}

/// Look up a pinned lens profile (memory, then data dir). The file content must hash to
/// its name, so a corrupted or edited file is rejected.
pub fn lens(r: &DataRef) -> Option<Arc<LensProfileData>> {
    if let Some(p) = lenses().lock().get(r) {
        return Some(p.clone());
    }
    if !r.as_str().chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let path = dir()?.join("lens").join(format!("{}.json", r.as_str()));
    let bytes = std::fs::read(&path).ok()?;
    let p: LensProfileData = serde_json::from_slice(&bytes).ok()?;
    if &lens_ref(&p) != r {
        tracing::warn!(path = %path.display(), "pinned lens profile does not match its hash; ignored");
        return None;
    }
    let p = Arc::new(p);
    lenses().lock().insert(r.clone(), p.clone());
    Some(p)
}

// ---------------------------------------------------------------------------------------------
// Imported files (DCPs, look LUTs): pinned as the original bytes, parsed on load.

/// A kind of imported file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FileKind {
    Dcp,
    Look,
    DisplayIcc,
}

impl FileKind {
    fn dir(self) -> &'static str {
        match self {
            FileKind::Dcp => "dcp",
            FileKind::Look => "look",
            FileKind::DisplayIcc => "display",
        }
    }
    fn ext(self) -> &'static str {
        match self {
            FileKind::Dcp => "dcp",
            FileKind::Look => "cube",
            FileKind::DisplayIcc => "icc",
        }
    }
}

/// Content hash of an imported file.
pub fn file_ref(kind: FileKind, bytes: &[u8]) -> DataRef {
    let mut h = blake3::Hasher::new();
    h.update(format!("aigle.{}.v1\0", kind.dir()).as_bytes());
    h.update(bytes);
    DataRef(h.finalize().to_hex().to_string())
}

fn file_path(kind: FileKind, r: &DataRef) -> Option<PathBuf> {
    if !r.as_str().chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    Some(
        dir()?
            .join(kind.dir())
            .join(format!("{}.{}", r.as_str(), kind.ext())),
    )
}

/// Parsed imported files by (kind, hash); `None` = known missing or invalid.
type Parsed = HashMap<(FileKind, DataRef), Option<Arc<dyn std::any::Any + Send + Sync>>>;

fn parsed() -> &'static Mutex<Parsed> {
    static M: OnceLock<Mutex<Parsed>> = OnceLock::new();
    M.get_or_init(Default::default)
}

/// Pin an imported file: persist it to the data dir (required: the edit must find it again)
/// and remember the parsed value.
pub fn pin_file<T: Send + Sync + 'static>(
    kind: FileKind,
    bytes: &[u8],
    value: T,
) -> Result<DataRef, String> {
    let r = file_ref(kind, bytes);
    let path = file_path(kind, &r).ok_or("no data directory (set AIGLE_DATA_DIR or HOME)")?;
    if !path.exists() {
        std::fs::create_dir_all(path.parent().expect("has parent"))
            .map_err(|e| format!("{}: {e}", path.display()))?;
        crate::encode::write_atomic(&path, bytes)
            .map_err(|e| format!("{}: {e}", path.display()))?;
    }
    parsed()
        .lock()
        .insert((kind, r.clone()), Some(Arc::new(value)));
    Ok(r)
}

/// Look up a pinned file and parse it with `parse` (once per process). The bytes must hash to
/// the name.
pub fn file<T: Send + Sync + 'static>(
    kind: FileKind,
    r: &DataRef,
    parse: impl FnOnce(&[u8]) -> Result<T, String>,
) -> Option<Arc<T>> {
    let key = (kind, r.clone());
    if let Some(v) = parsed().lock().get(&key) {
        return v.clone().and_then(|a| a.downcast::<T>().ok());
    }
    let load = || -> Option<T> {
        let path = file_path(kind, r)?;
        let bytes = std::fs::read(&path).ok()?;
        if &file_ref(kind, &bytes) != r {
            tracing::warn!(path = %path.display(), "pinned file does not match its hash; ignored");
            return None;
        }
        parse(&bytes)
            .map_err(|e| tracing::warn!(path = %path.display(), "cannot parse pinned file: {e}"))
            .ok()
    };
    let v: Option<Arc<dyn std::any::Any + Send + Sync>> = load().map(|t| Arc::new(t) as _);
    parsed().lock().insert(key, v.clone());
    v.and_then(|a| a.downcast::<T>().ok())
}

/// Validate and pin a user DCP or look LUT.
pub fn import(
    kind: aigle_engine_api::ImportKind,
    bytes: &[u8],
) -> Result<aigle_engine_api::Imported, String> {
    use aigle_engine_api::{ImportKind, Imported};
    match kind {
        ImportKind::Dcp => {
            let p = crate::profile::parse(bytes)?;
            let (name, model) = (p.name.clone(), p.camera_model.clone());
            let r = pin_file(FileKind::Dcp, bytes, p)?;
            Ok(Imported {
                data_ref: r,
                name,
                camera_model: (!model.is_empty()).then_some(model),
            })
        }
        ImportKind::LookLut => {
            let text =
                std::str::from_utf8(bytes).map_err(|_| "a .cube file must be text".to_string())?;
            let l = crate::look::parse(text)?;
            let name = l.title.clone();
            let r = pin_file(FileKind::Look, bytes, l)?;
            Ok(Imported {
                data_ref: r,
                name,
                camera_model: None,
            })
        }
        ImportKind::DisplayProfile => {
            let p = crate::icc::parse(bytes)?;
            let name = p.name.clone();
            let r = pin_file(FileKind::DisplayIcc, bytes, p)?;
            Ok(Imported {
                data_ref: r,
                name,
                camera_model: None,
            })
        }
    }
}
