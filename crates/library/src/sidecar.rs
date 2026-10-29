//! XMP sidecars (doc 01 §9.1–§9.3).

use std::collections::{BTreeMap, HashMap};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use aigle_core::EditParams;
use aigle_engine_api::{is_photo_path, is_raw_path};
use aigle_exiv2_sys::{ITEM_SEP, Kind};

/// Version of the `aigle:` properties written by this build.
pub const SCHEMA_VERSION: u32 = 1;

const RATING: &str = "Xmp.xmp.Rating";
const LABEL: &str = "Xmp.xmp.Label";
const SCHEMA: &str = "Xmp.aigle.SchemaVersion";
const EDITS: &str = "Xmp.aigle.Edits";
const SNAPSHOTS: &str = "Xmp.aigle.Snapshots";
const KEYWORDS: &str = "Xmp.dc.subject";
const HIERARCHY: &str = "Xmp.lr.hierarchicalSubject";

/// Descriptive IPTC Core fields, stored in their standard XMP properties.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum Field {
    Title,
    Caption,
    Headline,
    /// Several creators are shown and typed separated by `; `.
    Creator,
    CreatorJobTitle,
    Copyright,
    CopyrightUrl,
    Credit,
    Source,
    Instructions,
    JobId,
    Sublocation,
    City,
    State,
    Country,
    CountryCode,
}

impl Field {
    pub const ALL: [Field; 16] = [
        Field::Title,
        Field::Caption,
        Field::Headline,
        Field::Creator,
        Field::CreatorJobTitle,
        Field::Copyright,
        Field::CopyrightUrl,
        Field::Credit,
        Field::Source,
        Field::Instructions,
        Field::JobId,
        Field::Sublocation,
        Field::City,
        Field::State,
        Field::Country,
        Field::CountryCode,
    ];

    /// `snake_case` name, as in serialized form.
    pub fn name(self) -> &'static str {
        match self {
            Field::Title => "title",
            Field::Caption => "caption",
            Field::Headline => "headline",
            Field::Creator => "creator",
            Field::CreatorJobTitle => "creator_job_title",
            Field::Copyright => "copyright",
            Field::CopyrightUrl => "copyright_url",
            Field::Credit => "credit",
            Field::Source => "source",
            Field::Instructions => "instructions",
            Field::JobId => "job_id",
            Field::Sublocation => "sublocation",
            Field::City => "city",
            Field::State => "state",
            Field::Country => "country",
            Field::CountryCode => "country_code",
        }
    }

    /// The XMP property and how it is stored.
    pub fn property(self) -> (&'static str, Kind) {
        match self {
            Field::Title => ("Xmp.dc.title", Kind::LangAlt),
            Field::Caption => ("Xmp.dc.description", Kind::LangAlt),
            Field::Headline => ("Xmp.photoshop.Headline", Kind::Text),
            Field::Creator => ("Xmp.dc.creator", Kind::Seq),
            Field::CreatorJobTitle => ("Xmp.photoshop.AuthorsPosition", Kind::Text),
            Field::Copyright => ("Xmp.dc.rights", Kind::LangAlt),
            Field::CopyrightUrl => ("Xmp.xmpRights.WebStatement", Kind::Text),
            Field::Credit => ("Xmp.photoshop.Credit", Kind::Text),
            Field::Source => ("Xmp.photoshop.Source", Kind::Text),
            Field::Instructions => ("Xmp.photoshop.Instructions", Kind::Text),
            Field::JobId => ("Xmp.photoshop.TransmissionReference", Kind::Text),
            Field::Sublocation => ("Xmp.Iptc4xmpCore.Location", Kind::Text),
            Field::City => ("Xmp.photoshop.City", Kind::Text),
            Field::State => ("Xmp.photoshop.State", Kind::Text),
            Field::Country => ("Xmp.photoshop.Country", Kind::Text),
            Field::CountryCode => ("Xmp.Iptc4xmpCore.CountryCode", Kind::Text),
        }
    }

    /// The value as shown from what [`aigle_exiv2_sys::get`] returned.
    fn shown_value(self, raw: &str) -> String {
        match self.property().1 {
            Kind::Seq | Kind::Bag => raw
                .split(ITEM_SEP)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join("; "),
            _ => raw.trim().to_string(),
        }
    }

    fn stored_value(self, value: &str) -> String {
        match self.property().1 {
            Kind::Seq | Kind::Bag => value
                .split(';')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(&ITEM_SEP.to_string()),
            _ => value.to_string(),
        }
    }
}

/// Keywords as typed or read: trimmed, without empty or repeated (case-insensitive) ones,
/// in their first order.
pub fn clean_keywords<S: AsRef<str>>(keywords: impl IntoIterator<Item = S>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    keywords
        .into_iter()
        .map(|k| k.as_ref().trim().to_string())
        .filter(|k| !k.is_empty() && seen.insert(k.to_lowercase()))
        .collect()
}

/// Separates the levels of a keyword as typed and shown: `Places > France > Paris`.
pub const LEVEL_SEP: &str = " > ";

/// The levels of a typed keyword (`Places > France`, or `Places|France` as stored), trimmed,
/// without empty ones.
fn levels(tag: &str) -> Vec<&str> {
    tag.split(['>', '|'])
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect()
}

/// A typed keyword in the form [`PhotoMeta::tags`] shows it; `None` when blank.
pub fn normalize_tag(tag: &str) -> Option<String> {
    let l = levels(tag);
    (!l.is_empty()).then(|| l.join(LEVEL_SEP))
}

/// A named, saved state of a photo's edits.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Snapshot {
    pub name: String,
    pub params: EditParams,
}

/// What aigle keeps per photo.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PhotoMeta {
    /// −1 (rejected) to 5; 0 is unrated.
    pub rating: i8,
    /// `xmp:Label`, e.g. `Red`.
    pub label: Option<String>,
    /// `None` for an unedited photo.
    pub edits: Option<EditParams>,
    /// Oldest first.
    pub snapshots: Vec<Snapshot>,
    /// `dc:subject`, see [`clean_keywords`]. Holds every level of the hierarchical keywords.
    pub keywords: Vec<String>,
    /// `lr:hierarchicalSubject`: keyword paths with levels separated by `|`
    /// (`Places|France|Paris`).
    pub hierarchy: Vec<String>,
    /// Descriptive fields; absent and empty are the same.
    pub iptc: BTreeMap<Field, String>,
}

impl PhotoMeta {
    /// The keywords as the user sees them: each path (`Places > France > Paris`), then the
    /// flat keywords that are not a level of one.
    pub fn tags(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .hierarchy
            .iter()
            .filter_map(|p| normalize_tag(p))
            .collect();
        let in_paths: std::collections::HashSet<String> = self
            .hierarchy
            .iter()
            .flat_map(|p| levels(p))
            .map(str::to_lowercase)
            .collect();
        out.extend(
            self.keywords
                .iter()
                .filter(|k| !in_paths.contains(&k.to_lowercase()))
                .cloned(),
        );
        clean_keywords(out)
    }

    /// Set the keywords from what the user typed. A keyword with levels becomes a path, and
    /// each of its levels a flat keyword, as Lightroom writes them.
    pub fn set_tags<S: AsRef<str>>(&mut self, tags: impl IntoIterator<Item = S>) {
        let mut flat = Vec::new();
        let mut paths = Vec::new();
        for t in tags {
            let l = levels(t.as_ref());
            match l.len() {
                0 => {}
                1 => flat.push(l[0].to_string()),
                _ => {
                    flat.extend(l.iter().map(|s| s.to_string()));
                    paths.push(l.join("|"));
                }
            }
        }
        self.keywords = clean_keywords(flat);
        self.hierarchy = clean_keywords(paths);
    }

    /// Every keyword at any level: the flat ones, then path levels missing from them.
    pub fn all_keywords(&self) -> Vec<String> {
        clean_keywords(
            self.keywords
                .iter()
                .map(String::as_str)
                .chain(self.hierarchy.iter().flat_map(|p| levels(p))),
        )
    }

    /// The photo has keyword `k` at any level (case-insensitive).
    pub fn has_keyword(&self, k: &str) -> bool {
        let k = k.to_lowercase();
        self.all_keywords().iter().any(|x| x.to_lowercase() == k)
    }

    pub fn field(&self, f: Field) -> &str {
        self.iptc.get(&f).map_or("", String::as_str)
    }

    /// Set `f`, removing it when `value` is blank.
    pub fn set_field(&mut self, f: Field, value: &str) {
        let value = value.trim();
        if value.is_empty() {
            self.iptc.remove(&f);
        } else {
            self.iptc.insert(f, value.to_string());
        }
    }
}

/// The descriptive metadata of a sidecar as it is on disk.
#[derive(Clone, Debug, Default, PartialEq)]
struct Described {
    keywords: Vec<String>,
    hierarchy: Vec<String>,
    iptc: BTreeMap<Field, String>,
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
    /// Likewise for the snapshots.
    snapshots_locked: bool,
    /// Keywords and fields in the sidecar; only the ones that change are written, so
    /// translations and other details aigle does not show stay as they are.
    described: Described,
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
                let embedded = match crate::worker::embedded_xmp(photo) {
                    Ok(p) => p,
                    Err(e) => {
                        tracing::debug!(path = %photo.display(), "no embedded XMP: {e}");
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
        let mut snapshots_locked = false;
        let mut described = Described::default();
        if let Some(packet) = &packet {
            let at = source.as_deref().unwrap_or(photo);
            let mut keys = vec![RATING, LABEL, SCHEMA, EDITS, SNAPSHOTS, KEYWORDS, HIERARCHY];
            keys.extend(Field::ALL.iter().map(|f| f.property().0));
            let mut v = aigle_exiv2_sys::get(packet, &keys).map_err(xmp_err(at))?;
            if v.len() != keys.len() {
                return Err(xmp_err(at)(aigle_exiv2_sys::Error("short result".into())));
            }
            let fields = v.split_off(7);
            let [rating, label, schema, edits, snapshots, keywords, hierarchy] =
                <[Option<String>; 7]>::try_from(v).expect("length checked");
            loaded.meta.rating = rating.as_deref().and_then(parse_rating).unwrap_or(0);
            loaded.meta.label = label.filter(|l| !l.trim().is_empty());
            loaded.meta.keywords = keywords
                .map(|k| clean_keywords(k.split(ITEM_SEP)))
                .unwrap_or_default();
            loaded.meta.hierarchy = hierarchy
                .map(|h| clean_keywords(h.split(ITEM_SEP).map(|p| levels(p).join("|"))))
                .unwrap_or_default();
            for (f, raw) in Field::ALL.into_iter().zip(fields) {
                if let Some(raw) = raw {
                    loaded.meta.set_field(f, &f.shown_value(&raw));
                }
            }
            if source.is_some() {
                described = Described {
                    keywords: loaded.meta.keywords.clone(),
                    hierarchy: loaded.meta.hierarchy.clone(),
                    iptc: loaded.meta.iptc.clone(),
                };
            }
            let version = schema.as_deref().and_then(|s| s.trim().parse::<u32>().ok());
            if source.is_some()
                && let Some(json) = edits
            {
                match parse_edits(&json, version) {
                    Ok(e) => loaded.meta.edits = Some(e),
                    Err(w) => {
                        edits_locked = true;
                        loaded.warnings.push(w);
                    }
                }
            }
            if source.is_some()
                && let Some(json) = snapshots
            {
                match parse_snapshots(&json, version) {
                    Ok(s) => loaded.meta.snapshots = s,
                    Err(w) => {
                        snapshots_locked = true;
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
                snapshots_locked,
                described,
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
        let snapshots = (!meta.snapshots.is_empty())
            .then(|| serde_json::to_string(&meta.snapshots))
            .transpose()
            .map_err(|e| Error::Xmp {
                path: k.sidecar.clone(),
                message: e.to_string(),
            })?;
        let schema = SCHEMA_VERSION.to_string();
        let mut changes: Vec<(&str, Kind, Option<&str>)> = vec![
            (RATING, Kind::Text, rating.as_deref()),
            (LABEL, Kind::Text, label),
        ];
        if !k.edits_locked {
            changes.push((EDITS, Kind::Text, edits.as_deref()));
        }
        if !k.snapshots_locked {
            changes.push((SNAPSHOTS, Kind::Text, snapshots.as_deref()));
        }
        // The schema version describes whatever locked data is kept, so it stays as is then.
        if !k.edits_locked && !k.snapshots_locked {
            let any = edits.is_some() || snapshots.is_some();
            changes.push((SCHEMA, Kind::Text, any.then_some(schema.as_str())));
        }
        let described = Described {
            keywords: clean_keywords(&meta.keywords),
            hierarchy: clean_keywords(meta.hierarchy.iter().map(|p| levels(p).join("|"))),
            iptc: meta
                .iptc
                .iter()
                .filter(|(_, v)| !v.trim().is_empty())
                .map(|(f, v)| (*f, v.trim().to_string()))
                .collect(),
        };
        let sep = ITEM_SEP.to_string();
        let keywords = described.keywords.join(&sep);
        if described.keywords != k.described.keywords {
            changes.push((
                KEYWORDS,
                Kind::Bag,
                (!keywords.is_empty()).then_some(keywords.as_str()),
            ));
        }
        let hierarchy = described.hierarchy.join(&sep);
        if described.hierarchy != k.described.hierarchy {
            changes.push((
                HIERARCHY,
                Kind::Bag,
                (!hierarchy.is_empty()).then_some(hierarchy.as_str()),
            ));
        }
        let fields: Vec<(Field, Option<String>)> = Field::ALL
            .into_iter()
            .filter(|f| described.iptc.get(f) != k.described.iptc.get(f))
            .map(|f| (f, described.iptc.get(&f).map(|v| f.stored_value(v))))
            .collect();
        for (f, v) in &fields {
            let (key, kind) = f.property();
            changes.push((key, kind, v.as_deref().filter(|v| !v.is_empty())));
        }
        if current.is_none() && changes.iter().all(|(_, _, v)| v.is_none()) {
            return Ok(None);
        }

        let old = current.as_deref().map(String::from_utf8_lossy);
        let packet =
            aigle_exiv2_sys::update_typed(old.as_deref(), &changes).map_err(xmp_err(&k.sidecar))?;
        write_atomic(&k.sidecar, packet.as_bytes())?;
        k.hash = Some(blake3::hash(packet.as_bytes()));
        k.described = described;
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
    check_version(version, "edits")?;
    let mut p: EditParams =
        serde_json::from_str(json).map_err(|e| format!("edits are unreadable ({e}); kept"))?;
    p.sanitize()
        .map_err(|e| format!("edits are invalid ({e}); kept"))?;
    Ok(p)
}

fn parse_snapshots(json: &str, version: Option<u32>) -> Result<Vec<Snapshot>, String> {
    check_version(version, "snapshots")?;
    let mut s: Vec<Snapshot> =
        serde_json::from_str(json).map_err(|e| format!("snapshots are unreadable ({e}); kept"))?;
    for snap in &mut s {
        snap.params
            .sanitize()
            .map_err(|e| format!("snapshot {:?} is invalid ({e}); kept", snap.name))?;
    }
    Ok(s)
}

fn check_version(version: Option<u32>, what: &str) -> Result<(), String> {
    match version {
        Some(v) if v > SCHEMA_VERSION => Err(format!(
            "{what} were saved by a newer aigle (schema {v}); they are kept but not shown"
        )),
        None => Err(format!(
            "{what} have no schema version; they are kept but not shown"
        )),
        _ => Ok(()),
    }
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
            snapshots: vec![Snapshot {
                name: "Warm".into(),
                params: edited(),
            }],
            keywords: vec!["sea".into(), "boat".into()],
            hierarchy: vec!["Places|Brest".into()],
            iptc: [
                (Field::Title, "Harbour".to_string()),
                (Field::Creator, "Jane; John".to_string()),
                (Field::City, "Brest".to_string()),
            ]
            .into(),
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
        assert!(!text.contains("Snapshots"));
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
    fn snapshots_without_edits_round_trip() {
        let dir = temp_dir("snap");
        let photo = dir.join("d.nef");
        std::fs::write(&photo, b"raw").unwrap();
        let store = Store::new();
        let meta = PhotoMeta {
            snapshots: vec![Snapshot {
                name: "a".into(),
                params: edited(),
            }],
            ..PhotoMeta::default()
        };
        store.write(&photo, &meta).unwrap().unwrap();
        let loaded = Store::new().read(&photo).unwrap();
        assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
        assert_eq!(loaded.meta, meta);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn untouched_descriptions_are_not_rewritten() {
        let dir = temp_dir("iptc");
        let photo = dir.join("e.nef");
        std::fs::write(&photo, b"raw").unwrap();
        let packet = aigle_exiv2_sys::update_typed(
            None,
            &[
                ("Xmp.dc.title", Kind::LangAlt, Some("Harbour")),
                ("Xmp.dc.subject", Kind::Bag, Some("sea")),
            ],
        )
        .unwrap()
        .replace(
            "<rdf:li xml:lang=\"x-default\">Harbour</rdf:li>",
            "<rdf:li xml:lang=\"x-default\">Harbour</rdf:li><rdf:li xml:lang=\"fr\">Port</rdf:li>",
        );
        std::fs::write(dir.join("e.xmp"), packet).unwrap();
        let store = Store::new();
        let mut meta = store.read(&photo).unwrap().meta;
        assert_eq!(meta.field(Field::Title), "Harbour");
        assert_eq!(meta.keywords, ["sea"]);

        meta.rating = 2;
        store.write(&photo, &meta).unwrap();
        let text = std::fs::read_to_string(dir.join("e.xmp")).unwrap();
        assert!(text.contains("Port"), "{text}");

        meta.keywords.push(" Boat ".into());
        meta.keywords.push("SEA".into());
        meta.set_field(Field::Title, "  ");
        meta.set_field(Field::Headline, "Night");
        store.write(&photo, &meta).unwrap();
        let back = Store::new().read(&photo).unwrap().meta;
        assert_eq!(back.keywords, ["sea", "Boat"]);
        assert_eq!(back.field(Field::Title), "");
        assert_eq!(back.field(Field::Headline), "Night");
        assert_eq!(back.rating, 2);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn hierarchical_keywords() {
        let dir = temp_dir("hier");
        let photo = dir.join("h.nef");
        std::fs::write(&photo, b"raw").unwrap();
        let mut meta = PhotoMeta::default();
        meta.set_tags(["Places > France > Paris", "sea", "france", " > "]);
        assert_eq!(meta.keywords, ["Places", "France", "Paris", "sea"]);
        assert_eq!(meta.hierarchy, ["Places|France|Paris"]);
        assert_eq!(meta.tags(), ["Places > France > Paris", "sea"]);
        assert!(meta.has_keyword("paris"));

        let store = Store::new();
        store.write(&photo, &meta).unwrap();
        let text = std::fs::read_to_string(dir.join("h.xmp")).unwrap();
        assert!(text.contains("Places|France|Paris"), "{text}");
        let back = Store::new().read(&photo).unwrap().meta;
        assert_eq!(back.tags(), ["Places > France > Paris", "sea"]);

        let mut less = back.clone();
        less.set_tags(["sea"]);
        store.write(&photo, &less).unwrap();
        let back = Store::new().read(&photo).unwrap().meta;
        assert_eq!(
            (back.keywords, back.hierarchy),
            (vec!["sea".to_string()], vec![])
        );
        assert_eq!(normalize_tag("a|b >c"), Some("a > b > c".into()));
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
