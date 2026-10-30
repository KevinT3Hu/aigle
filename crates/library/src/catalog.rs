//! The catalog: an SQLite database of what does not belong in a photo's sidecar, namely
//! saved edit history and virtual copies.
//!
//! Sidecars stay the source of truth for a photo's own edits, rating and label. A virtual
//! copy exists only here: its edits, rating, label and snapshots are stored in its row.
//! Photos are identified by their path; moving a photo outside aigle loses its history
//! and copies.

use std::collections::{BTreeMap, HashMap};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use aigle_core::EditParams;
use aigle_doc::{Origin, SavedHistory};
use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::facts::Facts;
use crate::scan::PhotoEntry;
use crate::sidecar::{Field, PhotoMeta, Snapshot};

pub type CopyId = i64;

/// A photo, or one of its virtual copies.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PhotoKey {
    pub path: PathBuf,
    pub copy: Option<CopyId>,
}

impl PhotoKey {
    pub fn master(path: impl Into<PathBuf>) -> PhotoKey {
        PhotoKey {
            path: path.into(),
            copy: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct VirtualCopy {
    pub id: CopyId,
    /// The photo it is a copy of.
    pub path: PathBuf,
    pub name: String,
    pub meta: PhotoMeta,
}

#[derive(Debug, thiserror::Error)]
pub enum CatalogError {
    #[error("catalog: {0}")]
    Sql(#[from] rusqlite::Error),
    #[error("catalog: {0}")]
    Json(#[from] serde_json::Error),
    #[error("catalog: {0}")]
    Io(#[from] std::io::Error),
    #[error("catalog was written by a newer aigle (schema {0})")]
    Newer(i64),
}

const SCHEMA: i64 = 3;

/// [`PhotoMeta`] as stored for a virtual copy.
#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct StoredMeta {
    rating: i8,
    label: Option<String>,
    edits: Option<EditParams>,
    snapshots: Vec<Snapshot>,
    keywords: Vec<String>,
    hierarchy: Vec<String>,
    iptc: BTreeMap<Field, String>,
}

impl From<&PhotoMeta> for StoredMeta {
    fn from(m: &PhotoMeta) -> StoredMeta {
        StoredMeta {
            rating: m.rating,
            label: m.label.clone(),
            edits: m.edits.clone(),
            snapshots: m.snapshots.clone(),
            keywords: m.keywords.clone(),
            hierarchy: m.hierarchy.clone(),
            iptc: m.iptc.clone(),
        }
    }
}

impl From<StoredMeta> for PhotoMeta {
    fn from(m: StoredMeta) -> PhotoMeta {
        PhotoMeta {
            rating: m.rating,
            label: m.label,
            edits: m.edits,
            snapshots: m.snapshots,
            keywords: m.keywords,
            hierarchy: m.hierarchy,
            iptc: m.iptc,
        }
    }
}

fn bytes(s: &OsStr) -> &[u8] {
    s.as_encoded_bytes()
}

/// A modification time as stored: nanoseconds since the Unix epoch.
fn nanos(t: Option<std::time::SystemTime>) -> Option<i64> {
    let d = t?.duration_since(std::time::UNIX_EPOCH).ok()?;
    i64::try_from(d.as_nanos()).ok()
}

/// `(folder, file name)` columns of `path`.
fn split(path: &Path) -> (&[u8], &[u8]) {
    let dir = path.parent().map_or(&[][..], |d| bytes(d.as_os_str()));
    let file = path.file_name().map_or(&[][..], bytes);
    (dir, file)
}

pub struct Catalog {
    db: Mutex<Connection>,
}

impl Catalog {
    /// Open or create the catalog at `path`.
    pub fn open(path: &Path) -> Result<Catalog, CatalogError> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        Catalog::init(Connection::open(path)?)
    }

    /// A catalog that lives only as long as the process (no catalog file available).
    pub fn in_memory() -> Catalog {
        Catalog::init(Connection::open_in_memory().expect("in-memory SQLite"))
            .expect("fresh schema")
    }

    fn init(db: Connection) -> Result<Catalog, CatalogError> {
        db.pragma_update(None, "journal_mode", "WAL")?;
        db.pragma_update(None, "synchronous", "NORMAL")?;
        db.pragma_update(None, "foreign_keys", true)?;
        db.busy_timeout(std::time::Duration::from_secs(5))?;
        let version: i64 = db.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if version > SCHEMA {
            return Err(CatalogError::Newer(version));
        }
        db.execute_batch(
            "CREATE TABLE IF NOT EXISTS copies (
                 id INTEGER PRIMARY KEY,
                 dir BLOB NOT NULL,
                 file BLOB NOT NULL,
                 name TEXT NOT NULL,
                 meta TEXT NOT NULL,
                 created INTEGER NOT NULL DEFAULT (unixepoch())
             );
             CREATE INDEX IF NOT EXISTS copies_by_dir ON copies (dir, file);
             CREATE TABLE IF NOT EXISTS history (
                 dir BLOB NOT NULL,
                 file BLOB NOT NULL,
                 copy INTEGER NOT NULL DEFAULT 0,
                 data TEXT NOT NULL,
                 updated INTEGER NOT NULL DEFAULT (unixepoch()),
                 PRIMARY KEY (dir, file, copy)
             );
             CREATE TABLE IF NOT EXISTS albums (
                 id INTEGER PRIMARY KEY,
                 name TEXT NOT NULL UNIQUE COLLATE NOCASE
             );
             CREATE TABLE IF NOT EXISTS album_photos (
                 album INTEGER NOT NULL REFERENCES albums(id) ON DELETE CASCADE,
                 path BLOB NOT NULL,
                 PRIMARY KEY (album, path)
             );
             CREATE TABLE IF NOT EXISTS facts (
                 dir BLOB NOT NULL,
                 file BLOB NOT NULL,
                 bytes INTEGER NOT NULL,
                 modified INTEGER NOT NULL,
                 camera TEXT,
                 lens TEXT,
                 captured TEXT,
                 PRIMARY KEY (dir, file)
             );",
        )?;
        db.pragma_update(None, "user_version", SCHEMA)?;
        Ok(Catalog { db: Mutex::new(db) })
    }

    /// Consistent SQLite snapshot including WAL changes. Originals and XMP are separate.
    pub fn backup(&self, path: &Path) -> Result<(), CatalogError> {
        if path.exists() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "Backup already exists",
            )
            .into());
        }
        self.db
            .lock()
            .backup(rusqlite::MAIN_DB, path, rusqlite::backup::NO_PROGRESS)?;
        Ok(())
    }

    /// Restore only a validated catalog from this schema version.
    pub fn restore(&self, path: &Path) -> Result<(), CatalogError> {
        let source = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let version: i64 = source.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if version != SCHEMA {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("Backup schema {version}; this build requires schema {SCHEMA}"),
            )
            .into());
        }
        let integrity: String = source.query_row("PRAGMA integrity_check", [], |r| r.get(0))?;
        if integrity != "ok" {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, integrity).into());
        }
        for table in ["copies", "history", "facts", "albums", "album_photos"] {
            source.prepare(&format!("SELECT * FROM {table} LIMIT 0"))?;
        }
        self.db
            .lock()
            .restore(rusqlite::MAIN_DB, path, rusqlite::backup::NO_PROGRESS)?;
        Ok(())
    }

    pub fn albums(&self) -> Result<Vec<(i64, String)>, CatalogError> {
        let db = self.db.lock();
        let mut stmt = db.prepare("SELECT id, name FROM albums ORDER BY name COLLATE NOCASE")?;
        Ok(stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<Result<_, _>>()?)
    }

    pub fn create_album(&self, name: &str, paths: &[PathBuf]) -> Result<i64, CatalogError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Album name cannot be empty",
            )
            .into());
        }
        let mut db = self.db.lock();
        let tx = db.transaction()?;
        tx.execute("INSERT INTO albums (name) VALUES (?1)", [name])?;
        let id = tx.last_insert_rowid();
        for path in paths {
            tx.execute(
                "INSERT OR IGNORE INTO album_photos (album, path) VALUES (?1, ?2)",
                params![id, bytes(path.as_os_str())],
            )?;
        }
        tx.commit()?;
        Ok(id)
    }

    pub fn update_album(&self, id: i64, paths: &[PathBuf], add: bool) -> Result<(), CatalogError> {
        let mut db = self.db.lock();
        let tx = db.transaction()?;
        for path in paths {
            tx.execute(
                if add {
                    "INSERT OR IGNORE INTO album_photos (album, path) VALUES (?1, ?2)"
                } else {
                    "DELETE FROM album_photos WHERE album = ?1 AND path = ?2"
                },
                params![id, bytes(path.as_os_str())],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn album_paths(&self, id: i64) -> Result<Vec<PathBuf>, CatalogError> {
        use std::os::unix::ffi::OsStringExt;
        let db = self.db.lock();
        let mut stmt =
            db.prepare("SELECT path FROM album_photos WHERE album = ?1 ORDER BY path")?;
        Ok(stmt
            .query_map([id], |r| {
                let bytes: Vec<u8> = r.get(0)?;
                Ok(PathBuf::from(std::ffi::OsString::from_vec(bytes)))
            })?
            .collect::<Result<_, _>>()?)
    }

    pub fn delete_album(&self, id: i64) -> Result<(), CatalogError> {
        self.db
            .lock()
            .execute("DELETE FROM albums WHERE id = ?1", [id])?;
        Ok(())
    }

    /// The virtual copies of `photos`, grouped by photo in `photos` order, oldest first.
    pub fn copies_of(&self, photos: &[PathBuf]) -> Result<Vec<VirtualCopy>, CatalogError> {
        let db = self.db.lock();
        let mut by_dir: HashMap<&[u8], HashMap<&[u8], usize>> = HashMap::new();
        for (i, p) in photos.iter().enumerate() {
            let (dir, file) = split(p);
            by_dir.entry(dir).or_default().insert(file, i);
        }
        let mut stmt = db
            .prepare_cached("SELECT id, file, name, meta FROM copies WHERE dir = ?1 ORDER BY id")?;
        let mut found = Vec::new();
        for (dir, files) in &by_dir {
            let rows = stmt.query_map(params![dir], |r| {
                Ok((
                    r.get::<_, CopyId>(0)?,
                    r.get::<_, Vec<u8>>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                ))
            })?;
            for row in rows {
                let (id, file, name, meta) = row?;
                let Some(&i) = files.get(&file[..]) else {
                    continue;
                };
                let meta = match serde_json::from_str::<StoredMeta>(&meta) {
                    Ok(m) => m.into(),
                    Err(e) => {
                        tracing::warn!("virtual copy {id}: {e}");
                        PhotoMeta::default()
                    }
                };
                found.push((
                    i,
                    VirtualCopy {
                        id,
                        path: photos[i].clone(),
                        name,
                        meta,
                    },
                ));
            }
        }
        found.sort_by_key(|(i, c)| (*i, c.id));
        Ok(found.into_iter().map(|(_, c)| c).collect())
    }

    pub fn create_copy(
        &self,
        photo: &Path,
        name: &str,
        meta: &PhotoMeta,
    ) -> Result<CopyId, CatalogError> {
        let (dir, file) = split(photo);
        let json = serde_json::to_string(&StoredMeta::from(meta))?;
        let db = self.db.lock();
        db.execute(
            "INSERT INTO copies (dir, file, name, meta) VALUES (?1, ?2, ?3, ?4)",
            params![dir, file, name, json],
        )?;
        Ok(db.last_insert_rowid())
    }

    pub fn copy_meta(&self, id: CopyId) -> Result<Option<PhotoMeta>, CatalogError> {
        let json: Option<String> = self
            .db
            .lock()
            .query_row("SELECT meta FROM copies WHERE id = ?1", params![id], |r| {
                r.get(0)
            })
            .optional()?;
        Ok(json
            .map(|j| serde_json::from_str::<StoredMeta>(&j))
            .transpose()?
            .map(Into::into))
    }

    pub fn set_copy_meta(&self, id: CopyId, meta: &PhotoMeta) -> Result<(), CatalogError> {
        let json = serde_json::to_string(&StoredMeta::from(meta))?;
        self.db.lock().execute(
            "UPDATE copies SET meta = ?2 WHERE id = ?1",
            params![id, json],
        )?;
        Ok(())
    }

    /// The stored facts of `entries` whose file has not changed size or modification time
    /// since they were stored.
    pub fn facts_of(
        &self,
        entries: &[PhotoEntry],
    ) -> Result<HashMap<PathBuf, Facts>, CatalogError> {
        let db = self.db.lock();
        let mut stmt = db.prepare_cached(
            "SELECT file, bytes, modified, camera, lens, captured FROM facts WHERE dir = ?1",
        )?;
        let mut by_dir: HashMap<&[u8], HashMap<&[u8], &PhotoEntry>> = HashMap::new();
        for e in entries {
            let (dir, file) = split(&e.path);
            by_dir.entry(dir).or_default().insert(file, e);
        }
        let mut out = HashMap::new();
        for (dir, files) in &by_dir {
            let rows = stmt.query_map(params![dir], |r| {
                Ok((
                    r.get::<_, Vec<u8>>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, i64>(2)?,
                    Facts {
                        camera: r.get(3)?,
                        lens: r.get(4)?,
                        captured: r.get(5)?,
                    },
                ))
            })?;
            for row in rows {
                let (file, size, modified, facts) = row?;
                if let Some(e) = files.get(&file[..])
                    && i64::try_from(e.bytes).ok() == Some(size)
                    && nanos(e.modified) == Some(modified)
                {
                    out.insert(e.path.clone(), facts);
                }
            }
        }
        Ok(out)
    }

    /// Store the facts read from these files. Files without a modification time are
    /// skipped, since a later change could not be noticed.
    pub fn set_facts(&self, read: &[(PhotoEntry, Facts)]) -> Result<(), CatalogError> {
        let mut db = self.db.lock();
        let tx = db.transaction()?;
        {
            let mut stmt = tx.prepare_cached(
                "INSERT OR REPLACE INTO facts (dir, file, bytes, modified, camera, lens, captured)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )?;
            for (e, f) in read {
                let (Some(modified), Ok(size)) = (nanos(e.modified), i64::try_from(e.bytes)) else {
                    continue;
                };
                let (dir, file) = split(&e.path);
                stmt.execute(params![
                    dir, file, size, modified, f.camera, f.lens, f.captured
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn rename_copy(&self, id: CopyId, name: &str) -> Result<(), CatalogError> {
        self.db.lock().execute(
            "UPDATE copies SET name = ?2 WHERE id = ?1",
            params![id, name],
        )?;
        Ok(())
    }

    /// The photo at `from` is now at `to`: its virtual copies and history follow it. Rows
    /// left for an earlier photo at `to` are dropped.
    pub fn move_photo(&self, from: &Path, to: &Path) -> Result<(), CatalogError> {
        self.move_photo_impl(from, to, true)
    }

    pub fn relink_photo(&self, from: &Path, to: &Path) -> Result<(), CatalogError> {
        self.move_photo_impl(from, to, false)
    }

    fn move_photo_impl(&self, from: &Path, to: &Path, replace: bool) -> Result<(), CatalogError> {
        let (fd, ff) = split(from);
        let (td, tf) = split(to);
        let mut db = self.db.lock();
        let tx = db.transaction()?;
        if !replace {
            let occupied: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM history WHERE dir = ?1 AND file = ?2 UNION ALL SELECT 1 FROM copies WHERE dir = ?1 AND file = ?2)",
                params![td, tf], |r| r.get(0))?;
            if occupied {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::AlreadyExists,
                    "The replacement already has catalog edits or virtual copies",
                )
                .into());
            }
        }
        tx.execute(
            "DELETE FROM history WHERE dir = ?1 AND file = ?2",
            params![td, tf],
        )?;
        tx.execute(
            "DELETE FROM copies WHERE dir = ?1 AND file = ?2",
            params![td, tf],
        )?;
        tx.execute(
            "UPDATE history SET dir = ?3, file = ?4 WHERE dir = ?1 AND file = ?2",
            params![fd, ff, td, tf],
        )?;
        tx.execute(
            "UPDATE copies SET dir = ?3, file = ?4 WHERE dir = ?1 AND file = ?2",
            params![fd, ff, td, tf],
        )?;
        tx.execute(
            "DELETE FROM facts WHERE dir = ?1 AND file = ?2",
            params![td, tf],
        )?;
        tx.execute(
            "UPDATE facts SET dir = ?3, file = ?4 WHERE dir = ?1 AND file = ?2",
            params![fd, ff, td, tf],
        )?;
        tx.execute("INSERT OR IGNORE INTO album_photos (album, path) SELECT album, ?2 FROM album_photos WHERE path = ?1",
            params![bytes(from.as_os_str()), bytes(to.as_os_str())])?;
        tx.execute(
            "DELETE FROM album_photos WHERE path = ?1",
            [bytes(from.as_os_str())],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Drop everything stored for the photo at `path`: its history, its virtual copies and
    /// theirs.
    pub fn forget_photo(&self, path: &Path) -> Result<(), CatalogError> {
        let (dir, file) = split(path);
        let mut db = self.db.lock();
        let tx = db.transaction()?;
        tx.execute(
            "DELETE FROM history WHERE dir = ?1 AND file = ?2",
            params![dir, file],
        )?;
        tx.execute(
            "DELETE FROM copies WHERE dir = ?1 AND file = ?2",
            params![dir, file],
        )?;
        tx.execute(
            "DELETE FROM facts WHERE dir = ?1 AND file = ?2",
            params![dir, file],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Delete a virtual copy and its history.
    pub fn remove_copy(&self, id: CopyId) -> Result<(), CatalogError> {
        let mut db = self.db.lock();
        let tx = db.transaction()?;
        tx.execute("DELETE FROM history WHERE copy = ?1", params![id])?;
        tx.execute("DELETE FROM copies WHERE id = ?1", params![id])?;
        tx.commit()?;
        Ok(())
    }

    pub fn history(&self, key: &PhotoKey) -> Result<Option<SavedHistory>, CatalogError> {
        let (dir, file) = split(&key.path);
        let data: Option<String> = self
            .db
            .lock()
            .query_row(
                "SELECT data FROM history WHERE dir = ?1 AND file = ?2 AND copy = ?3",
                params![dir, file, key.copy.unwrap_or(0)],
                |r| r.get(0),
            )
            .optional()?;
        Ok(data.map(|d| serde_json::from_str(&d)).transpose()?)
    }

    pub fn set_history(&self, key: &PhotoKey, h: &SavedHistory) -> Result<(), CatalogError> {
        let (dir, file) = split(&key.path);
        let data = serde_json::to_string(h)?;
        self.db.lock().execute(
            "INSERT INTO history (dir, file, copy, data) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (dir, file, copy) DO UPDATE SET data = ?4, updated = unixepoch()",
            params![dir, file, key.copy.unwrap_or(0), data],
        )?;
        Ok(())
    }

    /// Add an entry to the saved history of `key` for a change made without opening it.
    /// `before` starts the history when none is saved.
    pub fn push_history(
        &self,
        key: &PhotoKey,
        before: &EditParams,
        label: &str,
        origin: Origin,
        after: &EditParams,
    ) -> Result<(), CatalogError> {
        let mut h = match self.history(key) {
            Ok(Some(h)) if h.head() == Some(before) => h,
            _ => SavedHistory::new(before.clone()),
        };
        h.push(label, origin, after.clone());
        self.set_history(key, &h)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(exposure: f32, rating: i8) -> PhotoMeta {
        let mut p = EditParams::default();
        p.light.exposure = exposure;
        PhotoMeta {
            rating,
            edits: Some(p),
            ..PhotoMeta::default()
        }
    }

    #[test]
    fn relink_refuses_to_replace_existing_virtual_copies() {
        let catalog = Catalog::in_memory();
        let from = PathBuf::from("/old/a.nef");
        let to = PathBuf::from("/new/a.nef");
        let id = catalog
            .create_copy(&to, "Keep", &PhotoMeta::default())
            .unwrap();
        assert!(catalog.relink_photo(&from, &to).is_err());
        assert!(catalog.copy_meta(id).unwrap().is_some());
    }

    #[test]
    fn catalog_backup_restore_is_consistent_and_rejects_invalid_inputs() {
        let dir = crate::sidecar::tests::temp_dir("catalog-backup");
        let catalog = Catalog::open(&dir.join("live.sqlite3")).unwrap();
        let a = catalog.create_album("Before", &[]).unwrap();
        let backup = dir.join("backup.sqlite3");
        catalog.backup(&backup).unwrap();
        assert!(catalog.backup(&backup).is_err());
        catalog.delete_album(a).unwrap();
        catalog.create_album("After", &[]).unwrap();
        catalog.restore(&backup).unwrap();
        assert_eq!(catalog.albums().unwrap(), [(a, "Before".into())]);
        std::fs::write(dir.join("invalid"), "not a database").unwrap();
        assert!(catalog.restore(&dir.join("invalid")).is_err());
        assert_eq!(catalog.albums().unwrap(), [(a, "Before".into())]);
        drop(catalog);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn album_membership_survives_moves_and_catalog_reopen() {
        let dir = crate::sidecar::tests::temp_dir("albums");
        let db = dir.join("catalog.sqlite3");
        let from = dir.join("a.nef");
        let to = dir.join("b.nef");
        let catalog = Catalog::open(&db).unwrap();
        let album = catalog
            .create_album("Favourites", &[from.clone(), from.clone()])
            .unwrap();
        assert!(catalog.create_album("favourites", &[]).is_err());
        assert!(catalog.create_album("  ", &[]).is_err());
        catalog.move_photo(&from, &to).unwrap();
        drop(catalog);
        let catalog = Catalog::open(&db).unwrap();
        assert_eq!(catalog.album_paths(album).unwrap(), [to.clone()]);
        assert_eq!(catalog.albums().unwrap(), [(album, "Favourites".into())]);
        catalog.update_album(album, &[to], false).unwrap();
        assert!(catalog.album_paths(album).unwrap().is_empty());
        catalog.delete_album(album).unwrap();
        assert!(catalog.albums().unwrap().is_empty());
        drop(catalog);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn copies_round_trip_per_photo() {
        let dir = crate::sidecar::tests::temp_dir("catalog");
        let path = dir.join("catalog.sqlite3");
        let (a, b, other) = (
            dir.join("a.nef"),
            dir.join("b.nef"),
            PathBuf::from("/elsewhere/a.nef"),
        );
        let c = Catalog::open(&path).unwrap();
        let b1 = c.create_copy(&b, "Copy 1", &meta(1.0, 2)).unwrap();
        let a1 = c.create_copy(&a, "Copy 1", &meta(0.5, 0)).unwrap();
        c.create_copy(&other, "Copy 1", &PhotoMeta::default())
            .unwrap();
        c.set_copy_meta(a1, &meta(0.7, 4)).unwrap();
        assert_eq!(c.copy_meta(a1).unwrap(), Some(meta(0.7, 4)));
        assert_eq!(c.copy_meta(999).unwrap(), None);
        drop(c);

        let c = Catalog::open(&path).unwrap();
        let copies = c.copies_of(&[a.clone(), b.clone()]).unwrap();
        assert_eq!(
            copies
                .iter()
                .map(|c| (c.id, c.path.clone()))
                .collect::<Vec<_>>(),
            [(a1, a.clone()), (b1, b.clone())]
        );
        assert_eq!(copies[0].meta, meta(0.7, 4));
        c.remove_copy(a1).unwrap();
        assert_eq!(c.copies_of(&[a.clone(), b]).unwrap().len(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn moved_photos_keep_copies_and_history() {
        let c = Catalog::in_memory();
        let (a, b) = (PathBuf::from("/p/a.nef"), PathBuf::from("/q/b.nef"));
        let id = c.create_copy(&a, "Copy 1", &meta(1.0, 3)).unwrap();
        let d = EditParams::default();
        let e = meta(1.0, 0).edits.unwrap();
        c.push_history(&PhotoKey::master(&a), &d, "x", Origin::User, &e)
            .unwrap();
        // Leftovers of an earlier photo at the destination are dropped.
        c.create_copy(&b, "Old", &PhotoMeta::default()).unwrap();
        c.move_photo(&a, &b).unwrap();
        let copies = c.copies_of(&[a.clone(), b.clone()]).unwrap();
        assert_eq!(
            copies.iter().map(|c| (c.id, &c.path)).collect::<Vec<_>>(),
            [(id, &b)]
        );
        assert!(c.history(&PhotoKey::master(&a)).unwrap().is_none());
        assert!(c.history(&PhotoKey::master(&b)).unwrap().is_some());
        c.rename_copy(id, "Warm").unwrap();
        assert_eq!(
            c.copies_of(std::slice::from_ref(&b)).unwrap()[0].name,
            "Warm"
        );
        c.forget_photo(&b).unwrap();
        assert!(c.copies_of(std::slice::from_ref(&b)).unwrap().is_empty());
        assert!(c.history(&PhotoKey::master(&b)).unwrap().is_none());
    }

    #[test]
    fn facts_are_cached_while_the_file_is_unchanged() {
        let c = Catalog::in_memory();
        let t = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        let entry = |name: &str, bytes: u64, modified| PhotoEntry {
            path: PathBuf::from("/p").join(name),
            name: name.into(),
            bytes,
            modified,
        };
        let facts = Facts {
            camera: Some("Canon EOS 6D".into()),
            lens: None,
            captured: Some("2017-01-08 11:18:13".into()),
        };
        let (a, b) = (entry("a.cr2", 10, Some(t)), entry("b.cr2", 10, None));
        c.set_facts(&[(a.clone(), facts.clone()), (b.clone(), facts.clone())])
            .unwrap();
        let found = c.facts_of(&[a.clone(), b.clone()]).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[&a.path], facts);
        let grown = entry("a.cr2", 11, Some(t));
        assert!(c.facts_of(&[grown]).unwrap().is_empty());
        let touched = entry("a.cr2", 10, Some(t + std::time::Duration::from_nanos(1)));
        assert!(c.facts_of(&[touched]).unwrap().is_empty());

        c.move_photo(&a.path, Path::new("/q/a.cr2")).unwrap();
        let moved = PhotoEntry {
            path: PathBuf::from("/q/a.cr2"),
            ..a.clone()
        };
        assert_eq!(c.facts_of(std::slice::from_ref(&moved)).unwrap().len(), 1);
        c.forget_photo(&moved.path).unwrap();
        assert!(c.facts_of(&[moved]).unwrap().is_empty());
    }

    #[test]
    fn history_per_photo_and_copy() {
        let c = Catalog::in_memory();
        let master = PhotoKey::master("/p/a.nef");
        let copy = PhotoKey {
            copy: Some(3),
            ..master.clone()
        };
        assert_eq!(c.history(&master).unwrap(), None);
        let e = meta(1.0, 0).edits.unwrap();
        let d = EditParams::default();
        c.push_history(&master, &d, "Paste", Origin::User, &e)
            .unwrap();
        c.push_history(&copy, &d, "Preset", Origin::Preset, &e)
            .unwrap();
        let h = c.history(&master).unwrap().unwrap();
        assert_eq!((h.entries.len(), h.head()), (1, Some(&e)));
        // A history ending elsewhere is restarted from `before`.
        let mut f = e.clone();
        f.light.contrast = 10.0;
        c.push_history(&master, &f, "Sync", Origin::User, &d)
            .unwrap();
        let h = c.history(&master).unwrap().unwrap();
        assert_eq!((h.entries.len(), &h.states[0]), (1, &f));
        assert_eq!(
            c.history(&copy).unwrap().unwrap().entries[0].label,
            "Preset"
        );
    }
}
