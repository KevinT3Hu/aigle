//! Durable catalog and job storage. Native parsers never run under a database transaction.
use crate::{
    config::Config,
    error::{Fault, invalid},
};
use aigle_engine_api::is_raw_path;
use aigle_library::{Store, sidecar_candidates};
use anyhow::{Context, Result, ensure};
use fs2::FileExt;
use parking_lot::{Condvar, Mutex};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[derive(Clone)]
pub struct Library {
    pub root: PathBuf,
    roots: Arc<Vec<PathBuf>>,
    pub config: Arc<Config>,
    pub stopping: tokio_util::sync::CancellationToken,
    _lock: Arc<fs::File>,
    mutation: Arc<Mutex<()>>,
    parsers: Arc<ParserBudget>,
}
struct ParserBudget {
    active: Mutex<usize>,
    changed: Condvar,
    limit: usize,
}
struct ParserPermit<'a>(&'a ParserBudget);
impl Drop for ParserPermit<'_> {
    fn drop(&mut self) {
        *self.0.active.lock() -= 1;
        self.0.changed.notify_one();
    }
}
impl ParserBudget {
    fn acquire(&self) -> ParserPermit<'_> {
        let mut active = self.active.lock();
        while *active >= self.limit {
            self.changed.wait(&mut active);
        }
        *active += 1;
        ParserPermit(self)
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Photo {
    pub id: String,
    pub name: String,
    pub size: u64,
    pub captured: Option<String>,
    pub camera: Option<String>,
    pub lens: Option<String>,
    pub rating: i8,
    pub revision: String,
    pub error: Option<String>,
    pub status: String,
    pub has_preview: bool,
    #[serde(skip)]
    pub path: PathBuf,
    #[serde(skip)]
    pub source_mtime: Option<i64>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Job {
    pub id: String,
    pub kind: String,
    #[serde(skip)]
    pub path: PathBuf,
    pub photo_id: Option<String>,
    pub state: String,
    pub attempts: u32,
    pub processed: u64,
    pub failed: u64,
    pub error: Option<String>,
    pub cancel_requested: bool,
    pub created: i64,
    pub updated: i64,
}
#[derive(Clone, Default, Debug, Deserialize)]
pub struct Search {
    #[serde(default)]
    pub q: String,
    pub rating: Option<i8>,
    pub camera: Option<String>,
    pub lens: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
}
#[derive(Clone, Default)]
pub struct FilterOptions {
    pub cameras: Vec<String>,
    pub lenses: Vec<String>,
}
impl Search {
    pub fn camera_value(&self) -> Option<&str> {
        self.camera.as_deref().filter(|v| !v.is_empty())
    }
    pub fn lens_value(&self) -> Option<&str> {
        self.lens.as_deref().filter(|v| !v.is_empty())
    }
    pub fn limit(&self) -> u32 {
        self.limit.unwrap_or(60).clamp(1, 500)
    }
    pub fn offset(&self) -> u32 {
        self.offset.unwrap_or(0).min(10_000_000)
    }
}
#[derive(Serialize, Deserialize)]
pub struct Inspection {
    pub captured: Option<String>,
    pub camera: Option<String>,
    pub lens: Option<String>,
    pub rating: i8,
    pub revision: String,
    pub preview: Option<Vec<u8>>,
    pub warning: Option<String>,
}
pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
pub fn secret() -> String {
    rand::random::<[u8; 32]>()
        .iter()
        .map(|v| format!("{v:02x}"))
        .collect()
}
pub fn hash(path: &Path) -> Result<String> {
    let mut h = blake3::Hasher::new();
    let mut f = fs::File::open(path)?;
    let mut b = [0; 128 * 1024];
    loop {
        let n = f.read(&mut b)?;
        if n == 0 {
            break;
        }
        h.update(&b[..n]);
    }
    Ok(h.finalize().to_hex().to_string())
}
pub fn sidecar(path: &Path) -> PathBuf {
    let candidates = sidecar_candidates(path);
    candidates
        .iter()
        .find(|p| p.exists())
        .cloned()
        .unwrap_or_else(|| candidates[0].clone())
}
pub fn revision(path: &Path) -> Result<String> {
    match hash(path) {
        Ok(hash) => Ok(hash),
        Err(e)
            if e.downcast_ref::<std::io::Error>()
                .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) =>
        {
            Ok("none".into())
        }
        Err(e) => Err(e),
    }
}
fn mtime(path: &Path) -> Result<i64> {
    Ok(path
        .metadata()?
        .modified()?
        .duration_since(UNIX_EPOCH)?
        .as_nanos()
        .min(i64::MAX as u128) as i64)
}
pub fn validate_id(id: &str) -> Result<()> {
    if id.len() != 64
        || !id
            .bytes()
            .all(|v| v.is_ascii_digit() || (b'a'..=b'f').contains(&v))
    {
        return Err(invalid("Invalid photo ID"));
    }
    Ok(())
}
pub fn validate_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > 240
        || name
            .chars()
            .any(|c| c.is_control() || c == '/' || c == '\\')
        || !is_raw_path(Path::new(name))
    {
        return Err(invalid(
            "Filename must be a RAW basename, at most 240 bytes",
        ));
    }
    Ok(())
}

impl Library {
    pub fn open(root: &Path, roots: &[PathBuf]) -> Result<Self> {
        Self::with_config(Config {
            data: root.into(),
            import_roots: roots.to_vec(),
            ..Config::default()
        })
    }
    pub fn with_config(mut config: Config) -> Result<Self> {
        config.validate()?;
        fs::create_dir_all(&config.data)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&config.data, fs::Permissions::from_mode(0o700))?;
        }
        #[cfg(windows)]
        crate::windows_permissions::make_private(&config.data).context("secure data directory")?;
        let root = config.data.canonicalize()?;
        ensure!(
            root.to_str().is_some(),
            "data directory must have a UTF-8 path"
        );
        let lock = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(root.join("server.lock"))?;
        lock.try_lock_exclusive()
            .context("another Thesauros process is using this data directory")?;
        let roots = config
            .import_roots
            .iter()
            .map(|p| p.canonicalize())
            .collect::<std::io::Result<Vec<_>>>()?;
        for p in &roots {
            ensure!(p.is_dir(), "import root must be a directory");
            ensure!(
                !root.starts_with(p) && !p.starts_with(&root),
                "import roots and server data directory must not overlap"
            );
        }
        for dir in ["originals", "previews", "tmp"] {
            fs::create_dir_all(root.join(dir))?;
        }
        let parsers = Arc::new(ParserBudget {
            active: Mutex::new(0),
            changed: Condvar::new(),
            limit: config.index_workers,
        });
        let lib = Self {
            root,
            roots: Arc::new(roots),
            config: Arc::new(config),
            stopping: tokio_util::sync::CancellationToken::new(),
            _lock: Arc::new(lock),
            mutation: Arc::new(Mutex::new(())),
            parsers,
        };
        let mut db = lib.db()?;
        db.pragma_update(None, "journal_mode", "WAL")?;
        db.pragma_update(None, "synchronous", "FULL")?;
        let version: u32 = db.pragma_query_value(None, "user_version", |r| r.get(0))?;
        ensure!(
            version <= 2,
            "catalog was written by a newer Thesauros version"
        );
        let migrations = [
            include_str!("../migrations/0001_initial.sql"),
            include_str!("../migrations/0002_service.sql"),
        ];
        for (index, sql) in migrations.iter().enumerate().skip(version as usize) {
            let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            tx.execute_batch(sql)?;
            tx.execute_batch("CREATE TABLE IF NOT EXISTS schema_migrations(version INTEGER PRIMARY KEY, applied INTEGER NOT NULL);")?;
            tx.execute(
                "INSERT INTO schema_migrations VALUES (?,?)",
                params![index as i64 + 1, now()],
            )?;
            tx.pragma_update(None, "user_version", index as i64 + 1)?;
            tx.commit()?;
        }
        // Only one instance owns this catalog. Work interrupted by a restart can safely be reclaimed.
        db.execute("UPDATE jobs SET state=CASE WHEN cancel_requested=1 THEN 'cancelled' WHEN attempts>=3 THEN 'failed' ELSE 'queued' END, updated=? WHERE state='running'",[now()])?;
        db.execute("DELETE FROM sessions WHERE expires<=?", [now()])?;
        db.execute("UPDATE photos SET status='failed',error='Indexing interrupted repeatedly; retry the failed job' WHERE status='pending' AND id IN (SELECT photo_id FROM jobs WHERE state='failed') AND id NOT IN (SELECT photo_id FROM jobs WHERE state IN ('queued','running') AND photo_id IS NOT NULL)",[])?;
        lib.recover_sidecars()?;
        lib.recover_originals()?;
        // Named temporary uploads have no committed catalog entries and are safe to discard after a crash.
        for item in fs::read_dir(lib.root.join("tmp"))? {
            let item = item?;
            if item.file_type()?.is_file()
                && item.file_name().to_string_lossy().starts_with("upload-")
            {
                fs::remove_file(item.path())?;
            }
        }
        Ok(lib)
    }
    pub fn db(&self) -> Result<Connection> {
        let db = Connection::open(self.root.join("catalog.sqlite3"))?;
        db.busy_timeout(Duration::from_secs(5))?;
        db.pragma_update(None, "foreign_keys", true)?;
        db.pragma_update(None, "synchronous", "FULL")?;
        Ok(db)
    }
    pub fn ready(&self) -> Result<()> {
        self.db()?.query_row("SELECT 1", [], |_| Ok(()))?;
        if fs2::available_space(&self.root)? < self.config.min_free_bytes {
            return Err(Fault::StorageFull.into());
        }
        Ok(())
    }
    fn from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Photo> {
        Ok(Photo {
            id: row.get("id")?,
            name: row.get("name")?,
            path: PathBuf::from(row.get::<_, String>("path")?),
            size: row.get::<_, i64>("size")? as u64,
            captured: row.get("captured")?,
            camera: row.get("camera")?,
            lens: row.get("lens")?,
            rating: row.get("rating")?,
            revision: row.get("revision")?,
            error: row.get("error")?,
            status: row.get("status")?,
            source_mtime: row.get("source_mtime")?,
            has_preview: false,
        })
    }
    fn with_preview(&self, mut p: Photo) -> Photo {
        p.has_preview = self
            .root
            .join("previews")
            .join(format!("{}.jpg", p.id))
            .is_file();
        p
    }
    pub fn photo(&self, id: &str) -> Result<Photo> {
        validate_id(id)?;
        let p = self
            .db()?
            .query_row("SELECT * FROM photos WHERE id=?", [id], Self::from_row)
            .optional()?
            .ok_or(Fault::NotFound)?;
        Ok(self.with_preview(p))
    }
    pub fn list(&self, q: &Search) -> Result<Vec<Photo>> {
        if q.q.len() > 256
            || q.camera.as_ref().is_some_and(|s| s.len() > 256)
            || q.lens.as_ref().is_some_and(|s| s.len() > 256)
            || q.rating.is_some_and(|r| !(-1..=5).contains(&r))
        {
            return Err(invalid("Invalid search filter"));
        }
        let escaped = q.q.replace('!', "!!").replace('%', "!%").replace('_', "!_");
        let db = self.db()?;
        let mut stmt=db.prepare("SELECT * FROM photos WHERE (name LIKE ?1 ESCAPE '!' OR camera LIKE ?1 ESCAPE '!' OR lens LIKE ?1 ESCAPE '!') AND (?2 IS NULL OR rating=?2) AND (?3 IS NULL OR camera=?3) AND (?4 IS NULL OR lens=?4) ORDER BY captured DESC,id LIMIT ?5 OFFSET ?6")?;
        let rows = stmt.query_map(
            params![
                format!("%{escaped}%"),
                q.rating,
                q.camera_value(),
                q.lens_value(),
                q.limit(),
                q.offset()
            ],
            Self::from_row,
        )?;
        rows.map(|r| Ok(self.with_preview(r?))).collect()
    }
    pub fn filter_options(&self) -> Result<FilterOptions> {
        let db = self.db()?;
        let values = |sql: &str| -> Result<Vec<String>> {
            let mut statement = db.prepare(sql)?;
            Ok(statement
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?)
        };
        Ok(FilterOptions {
            cameras: values(
                "SELECT DISTINCT camera FROM photos WHERE camera IS NOT NULL AND camera<>'' ORDER BY camera COLLATE NOCASE,camera",
            )?,
            lenses: values(
                "SELECT DISTINCT lens FROM photos WHERE lens IS NOT NULL AND lens<>'' ORDER BY lens COLLATE NOCASE,lens",
            )?,
        })
    }
    pub fn check_queue(&self) -> Result<()> {
        self.queue_room(&self.db()?)
    }
    fn queue_room(&self, db: &Connection) -> Result<()> {
        let count: u32 = db.query_row(
            "SELECT COUNT(*) FROM jobs WHERE state IN ('queued','running')",
            [],
            |r| r.get(0),
        )?;
        if count >= self.config.max_queued_jobs {
            return Err(Fault::QueueFull.into());
        }
        Ok(())
    }
    fn insert_job(
        &self,
        db: &Connection,
        kind: &str,
        path: &Path,
        photo: Option<&str>,
    ) -> Result<Job> {
        self.queue_room(db)?;
        let id = secret();
        let time = now();
        db.execute(
            "INSERT INTO jobs(id,kind,path,photo_id,created,updated) VALUES (?,?,?,?,?,?)",
            params![
                id,
                kind,
                path.to_str().context("non-UTF8 path")?,
                photo,
                time,
                time
            ],
        )?;
        Self::job_in(db, &id)
    }
    pub fn accept_upload(
        &self,
        temp: tempfile::NamedTempFile,
        name: &str,
        id: &str,
    ) -> Result<Photo> {
        validate_name(name)?;
        validate_id(id)?;
        let _guard = self.mutation.lock();
        let mut db = self.db()?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(p) = tx
            .query_row("SELECT * FROM photos WHERE id=?", [id], Self::from_row)
            .optional()?
        {
            return Ok(self.with_preview(p));
        }
        self.queue_room(&tx)?;
        let suffix = Path::new(name)
            .extension()
            .unwrap()
            .to_string_lossy()
            .to_ascii_lowercase();
        let dest = self.root.join("originals").join(format!("{id}.{suffix}"));
        temp.as_file().sync_all()?;
        match temp.persist_noclobber(&dest) {
            Ok(_) => {}
            Err(e) if e.error.kind() == std::io::ErrorKind::AlreadyExists => {
                ensure!(
                    hash(&dest)? == id,
                    "existing managed original checksum mismatch"
                );
            }
            Err(e) => return Err(e.into()),
        }
        fs::File::open(dest.parent().unwrap())?.sync_all()?;
        tx.execute("INSERT INTO photos(id,name,path,size,status,source_mtime) VALUES (?,?,?,?,'pending',?)",params![id,name,dest.to_str().unwrap(),i64::try_from(dest.metadata()?.len())?,mtime(&dest)?])?;
        self.insert_job(&tx, "index", &dest, Some(id))?;
        tx.commit()?;
        self.photo(id)
    }
    pub fn import_path(&self, path: &Path) -> Result<PathBuf> {
        let path = path.canonicalize().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                anyhow::Error::from(Fault::NotFound)
            } else {
                e.into()
            }
        })?;
        if !self.roots.iter().any(|r| path.starts_with(r)) {
            return Err(invalid("Path is outside configured import roots"));
        }
        Ok(path)
    }
    pub fn queue_import(&self, path: &Path) -> Result<Job> {
        let path = self.import_path(path)?;
        let mut db = self.db()?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let job = self.insert_job(&tx, "import", &path, None)?;
        tx.commit()?;
        Ok(job)
    }
    pub fn reindex(&self, id: &str) -> Result<Job> {
        let p = self.photo(id)?;
        let mut db = self.db()?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(active) = tx
            .query_row(
                "SELECT id FROM jobs WHERE photo_id=? AND state IN ('queued','running')",
                [id],
                |r| r.get::<_, String>(0),
            )
            .optional()?
        {
            return Self::job_in(&tx, &active);
        }
        let job = self.insert_job(&tx, "index", &p.path, Some(id))?;
        tx.execute("UPDATE photos SET status='pending' WHERE id=?", [id])?;
        tx.commit()?;
        Ok(job)
    }
    fn job_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Job> {
        Ok(Job {
            id: r.get("id")?,
            kind: r.get("kind")?,
            path: PathBuf::from(r.get::<_, String>("path")?),
            photo_id: r.get("photo_id")?,
            state: r.get("state")?,
            attempts: r.get("attempts")?,
            processed: r.get::<_, i64>("processed")? as u64,
            failed: r.get::<_, i64>("failed")? as u64,
            error: r.get("error")?,
            cancel_requested: r.get("cancel_requested")?,
            created: r.get("created")?,
            updated: r.get("updated")?,
        })
    }
    fn job_in(db: &Connection, id: &str) -> Result<Job> {
        validate_id(id)?;
        Ok(db
            .query_row("SELECT * FROM jobs WHERE id=?", [id], Self::job_row)
            .optional()?
            .ok_or(Fault::NotFound)?)
    }
    pub fn job(&self, id: &str) -> Result<Job> {
        Self::job_in(&self.db()?, id)
    }
    pub fn jobs(&self) -> Result<Vec<Job>> {
        let db = self.db()?;
        let mut stmt =
            db.prepare("SELECT * FROM jobs ORDER BY CASE state WHEN 'running' THEN 0 WHEN 'queued' THEN 1 ELSE 2 END,created DESC,rowid DESC LIMIT 50")?;
        Ok(stmt
            .query_map([], Self::job_row)?
            .collect::<rusqlite::Result<_>>()?)
    }
    pub fn claim(&self) -> Result<Option<Job>> {
        let mut db = self.db()?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let id: Option<String> = tx
            .query_row(
                "SELECT id FROM jobs WHERE state='queued' ORDER BY created,rowid LIMIT 1",
                [],
                |r| r.get(0),
            )
            .optional()?;
        let id = match id {
            Some(id) => id,
            None => {
                let pending=tx.query_row("SELECT id,path FROM photos WHERE status='pending' AND NOT EXISTS(SELECT 1 FROM jobs WHERE photo_id=photos.id AND state IN ('queued','running')) ORDER BY id LIMIT 1",[],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?))).optional()?;
                if let Some((photo, path)) = pending {
                    if self.queue_room(&tx).is_err() {
                        return Ok(None);
                    }
                    self.insert_job(&tx, "index", Path::new(&path), Some(&photo))?
                        .id
                } else {
                    return Ok(None);
                }
            }
        };
        tx.execute(
            "UPDATE jobs SET state='running',attempts=attempts+1,updated=? WHERE id=?",
            params![now(), id],
        )?;
        let job = Self::job_in(&tx, &id)?;
        tx.commit()?;
        Ok(Some(job))
    }
    pub fn cancel(&self, id: &str) -> Result<Job> {
        let mut db = self.db()?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        Self::job_in(&tx, id)?;
        tx.execute("UPDATE jobs SET cancel_requested=1,state=CASE WHEN state='queued' THEN 'cancelled' ELSE state END,updated=? WHERE id=? AND state IN ('queued','running')",params![now(),id])?;
        let job = Self::job_in(&tx, id)?;
        if job.state == "cancelled"
            && let Some(photo) = &job.photo_id
        {
            tx.execute(
                "UPDATE photos SET status='cancelled' WHERE id=? AND status='pending'",
                [photo],
            )?;
        }
        tx.commit()?;
        Ok(job)
    }
    pub fn retry(&self, id: &str) -> Result<Job> {
        let mut db = self.db()?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let job = Self::job_in(&tx, id)?;
        if !matches!(job.state.as_str(), "failed" | "cancelled") {
            return Err(invalid("Only failed or cancelled jobs can be retried"));
        }
        self.queue_room(&tx)?;
        if let Some(photo) = &job.photo_id
            && tx.query_row("SELECT EXISTS(SELECT 1 FROM jobs WHERE photo_id=? AND state IN ('queued','running'))",[photo],|r|r.get::<_,bool>(0))? {return Err(invalid("Photo already has an active job"))}
        tx.execute("UPDATE jobs SET state='queued',attempts=0,cancel_requested=0,processed=0,failed=0,error=NULL,updated=? WHERE id=?",params![now(),id])?;
        if let Some(photo) = &job.photo_id {
            tx.execute("UPDATE photos SET status='pending' WHERE id=?", [photo])?;
        }
        tx.commit()?;
        self.job(id)
    }
    pub fn run_job(&self, job: &Job) -> Result<()> {
        let result = if job.kind == "import" {
            self.run_import(job)
        } else {
            self.inspect_photo(job.photo_id.as_deref().context("index job without photo")?)
        };
        let current = self.job(&job.id)?;
        let (state, error) = if self.stopping.is_cancelled() && job.kind == "import" {
            ("queued", None)
        } else if current.cancel_requested {
            ("cancelled", None)
        } else {
            match result {
                Ok(()) => ("completed", None),
                Err(ref e) => {
                    tracing::error!(job_id=%job.id,error=?e,"index job failed");
                    ("failed", Some("Indexing failed; see server logs"))
                }
            }
        };
        self.db()?.execute(
            "UPDATE jobs SET state=?,error=COALESCE(?,error),updated=? WHERE id=?",
            params![state, error, now(), job.id],
        )?;
        if job.kind == "index" && state == "failed" {
            self.db()?.execute(
                "UPDATE photos SET status='failed',error=? WHERE id=?",
                params!["RAW indexing failed; see server logs", job.photo_id],
            )?;
        }
        Ok(())
    }
    fn run_import(&self, job: &Job) -> Result<()> {
        let root = self.import_path(&job.path)?;
        let mut progress = (0i64, 0i64);
        self.walk_import(&root, 0, job, &mut progress)?;
        self.db()?.execute("UPDATE jobs SET processed=?,failed=?,error=CASE WHEN ?>0 THEN 'Some files could not be indexed; see server logs' ELSE NULL END WHERE id=?",params![progress.0,progress.1,progress.1,job.id])?;
        Ok(())
    }
    // Depth-first traversal keeps at most 33 directory iterators open, independent
    // of collection size. Each file commits its own progress before continuing.
    fn walk_import(
        &self,
        path: &Path,
        depth: usize,
        job: &Job,
        progress: &mut (i64, i64),
    ) -> Result<()> {
        if self.stopping.is_cancelled() || self.job(&job.id)?.cancel_requested {
            return Ok(());
        }
        if depth > 32 {
            return Err(invalid("Import exceeds maximum folder depth of 32"));
        }
        if path.is_dir() {
            for entry in fs::read_dir(path)? {
                if self.stopping.is_cancelled() || self.job(&job.id)?.cancel_requested {
                    break;
                }
                let entry = match entry {
                    Ok(entry) => entry,
                    Err(error) => {
                        progress.1 += 1;
                        tracing::warn!(job_id=%job.id,?error,"could not read directory entry");
                        continue;
                    }
                };
                if entry.file_name().to_string_lossy().starts_with('.') {
                    continue;
                }
                let kind = entry.file_type()?;
                if kind.is_symlink() {
                    if is_raw_path(&entry.path()) {
                        progress.1 += 1;
                    }
                    continue;
                }
                if (kind.is_dir() || (kind.is_file() && is_raw_path(&entry.path())))
                    && let Err(error) = self.walk_import(&entry.path(), depth + 1, job, progress)
                {
                    progress.1 += 1;
                    tracing::warn!(job_id=%job.id,?error,"could not import folder");
                }
            }
            return Ok(());
        }
        if !is_raw_path(path) {
            return Ok(());
        }
        let result: Result<()> = (|| {
            let path = self.import_path(path)?;
            let before = mtime(&path)?;
            let id = hash(&path)?;
            ensure!(mtime(&path)? == before, "file changed while hashing");
            let db = self.db()?;
            let existing = db
                .query_row("SELECT status FROM photos WHERE id=?", [&id], |r| {
                    r.get::<_, String>(0)
                })
                .optional()?;
            if existing.as_deref() == Some("ready") {
                return Ok(());
            }
            db.execute("INSERT INTO photos(id,name,path,size,status,source_mtime) VALUES (?,?,?,?,'pending',?) ON CONFLICT(id) DO NOTHING",params![id,path.file_name().unwrap().to_string_lossy(),path.to_str().context("non-UTF8 path")?,i64::try_from(path.metadata()?.len())?,before])?;
            self.inspect_photo(&id)
        })();
        progress.0 += 1;
        if let Err(error) = result {
            progress.1 += 1;
            tracing::warn!(job_id=%job.id,path=%path.display(),?error,"import file failed");
        }
        self.db()?.execute(
            "UPDATE jobs SET processed=?,failed=?,updated=? WHERE id=?",
            params![progress.0, progress.1, now(), job.id],
        )?;
        Ok(())
    }
    fn inspect_photo(&self, id: &str) -> Result<()> {
        let p = self.photo(id)?;
        let result = (|| {
            self.check_source(&p)?;
            let source_mtime = mtime(&p.path)?;
            if p.source_mtime.is_none() {
                ensure!(
                    hash(&p.path)? == p.id,
                    "source content does not match catalog identity"
                );
            }
            let parser_permit = self.parsers.acquire();
            let data: Inspection = if aigle_isolate::enabled() {
                aigle_isolate::call("server.inspect", &p.path, None)?
            } else {
                inspect(&p.path)?
            };
            drop(parser_permit);
            if let Some(bytes) = data.preview {
                atomic(
                    &self.root.join("previews").join(format!("{id}.jpg")),
                    &bytes,
                )?;
            }
            self.check_source(&p)?;
            ensure!(
                mtime(&p.path)? == source_mtime,
                "source changed while parsing"
            );
            // Serialize the final metadata snapshot with sidecar writes, after parsing finishes.
            let _guard = self.mutation.lock();
            let current_revision = revision(&sidecar(&p.path))?;
            self.db()?.execute("UPDATE photos SET captured=?,camera=?,lens=?,rating=CASE WHEN ?= ? THEN ? ELSE rating END,revision=CASE WHEN ?=? THEN ? ELSE revision END,status='ready',error=?,source_mtime=? WHERE id=?",params![data.captured,data.camera,data.lens,current_revision,data.revision,data.rating,current_revision,data.revision,current_revision,data.warning,source_mtime,id])?;
            Ok(())
        })();
        if result.is_err() {
            self.db()?.execute("UPDATE photos SET status='failed',error='RAW indexing failed; see server logs' WHERE id=?",[id])?;
        }
        result
    }
    pub fn check_source(&self, p: &Photo) -> Result<()> {
        if fs::symlink_metadata(&p.path).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err(Fault::SourceChanged.into());
        }
        let meta = fs::metadata(&p.path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                anyhow::Error::from(Fault::NotFound)
            } else {
                e.into()
            }
        })?;
        if meta.len() != p.size
            || p.source_mtime
                .is_some_and(|m| mtime(&p.path).ok() != Some(m))
        {
            return Err(Fault::SourceChanged.into());
        }
        Ok(())
    }
    pub fn read_sidecar(&self, id: &str) -> Result<(Option<Vec<u8>>, String)> {
        let _guard = self.mutation.lock();
        let p = self.photo(id)?;
        let path = sidecar(&p.path);
        if !path.exists() {
            return Ok((None, "none".into()));
        }
        if fs::symlink_metadata(&path)?.file_type().is_symlink() {
            return Err(invalid("Symbolic-link sidecars are not supported"));
        }
        if path.metadata()?.len() > 8 * 1024 * 1024 {
            return Err(invalid("Sidecar exceeds the 8 MiB limit"));
        }
        let bytes = fs::read(path)?;
        let rev = blake3::hash(&bytes).to_hex().to_string();
        Ok((Some(bytes), rev))
    }
    pub fn rate(&self, id: &str, expected: &str, rating: i8) -> Result<String> {
        if !(-1..=5).contains(&rating) {
            return Err(invalid("Rating must be -1 through 5"));
        }
        let (bytes, revision) = self.read_sidecar(id)?;
        if format!("\"{revision}\"") != expected {
            return Err(Fault::Conflict.into());
        }
        let packet = bytes.map(String::from_utf8).transpose()?;
        let parser_permit = self.parsers.acquire();
        let updated: String = if aigle_isolate::enabled() {
            aigle_isolate::call("server.rate_xmp", &(packet, rating), None)?
        } else {
            update_rating(packet.as_deref(), rating)?
        };
        drop(parser_permit);
        self.write_sidecar(id, expected, updated.as_bytes())
    }
    pub fn write_sidecar(&self, id: &str, expected: &str, bytes: &[u8]) -> Result<String> {
        let p = self.photo(id)?;
        if bytes.len() > 8 * 1024 * 1024 {
            return Err(Fault::TooLarge.into());
        }
        let packet = std::str::from_utf8(bytes).map_err(|_| invalid("XMP must be UTF-8"))?;
        let parser_permit = self.parsers.acquire();
        let rating: i8 = if aigle_isolate::enabled() {
            aigle_isolate::call("server.validate_xmp", &packet, None)
                .map_err(|_| invalid("Invalid XMP packet or rating"))?
        } else {
            validate_xmp(packet)?
        };
        drop(parser_permit);
        let _guard = self.mutation.lock();
        let target = sidecar(&p.path);
        if fs::symlink_metadata(&target).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err(invalid("Symbolic-link sidecars cannot be overwritten"));
        }
        let previous = revision(&target)?;
        if format!("\"{previous}\"") != expected {
            return Err(Fault::Conflict.into());
        }
        let rev = blake3::hash(bytes).to_hex().to_string();
        // A FULL-synchronous journal makes file+catalog updates recoverable after interruption.
        self.db()?.execute(
            "INSERT INTO sidecar_journal VALUES (?,?,?,?,?,?)",
            params![
                id,
                target.to_str().context("non-UTF8 sidecar path")?,
                previous,
                rev,
                bytes,
                rating
            ],
        )?;
        atomic(&target, bytes)?;
        self.finish_sidecar(id, &rev, rating)?;
        Ok(rev)
    }
    fn finish_sidecar(&self, id: &str, rev: &str, rating: i8) -> Result<()> {
        let mut db = self.db()?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "UPDATE photos SET revision=?,rating=? WHERE id=?",
            params![rev, rating, id],
        )?;
        tx.execute("DELETE FROM sidecar_journal WHERE photo_id=?", [id])?;
        tx.commit()?;
        Ok(())
    }
    fn recover_sidecars(&self) -> Result<()> {
        let db = self.db()?;
        let mut stmt = db.prepare(
            "SELECT photo_id,target,previous_revision,revision,packet,rating FROM sidecar_journal",
        )?;
        let entries = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, Vec<u8>>(4)?,
                    r.get::<_, i8>(5)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for (id, path, previous, rev, bytes, rating) in entries {
            let path = Path::new(&path);
            let current = revision(path)?;
            if current != rev {
                ensure!(
                    current == previous,
                    "sidecar recovery conflict for {id}; preserve both catalog journal and external XMP before repair"
                );
                atomic(path, &bytes)?;
            }
            self.finish_sidecar(&id, &rev, rating)?;
        }
        Ok(())
    }
    fn recover_originals(&self) -> Result<()> {
        for entry in fs::read_dir(self.root.join("originals"))? {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            let path = entry.path();
            if !is_raw_path(&path) {
                continue;
            }
            let Some(id) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            if validate_id(id).is_err() {
                continue;
            }
            let mut db = self.db()?;
            if db.query_row(
                "SELECT EXISTS(SELECT 1 FROM photos WHERE id=?)",
                [id],
                |r| r.get::<_, bool>(0),
            )? {
                continue;
            }
            ensure!(
                hash(&path)? == id,
                "orphaned original checksum mismatch for {id}"
            );
            let name = format!(
                "Recovered-{}.{}",
                &id[..12],
                path.extension().unwrap().to_string_lossy()
            );
            let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            tx.execute("INSERT INTO photos(id,name,path,size,status,source_mtime) VALUES (?,?,?,?,'pending',?)",params![id,name,path.to_str().context("non-UTF8 path")?,i64::try_from(path.metadata()?.len())?,mtime(&path)?])?;
            self.insert_job(&tx, "index", &path, Some(id))?;
            tx.commit()?;
            tracing::warn!(photo_id=%id,"recovered original from interrupted registration; original display name was unavailable");
        }
        Ok(())
    }
    pub fn housekeeping(&self) -> Result<()> {
        let db = self.db()?;
        db.execute("DELETE FROM sessions WHERE expires<=?", [now()])?;
        db.execute(
            "DELETE FROM jobs WHERE state IN ('completed','cancelled') AND updated<?",
            [now() - 30 * 86400],
        )?;
        Ok(())
    }
}
pub fn atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let dir = path.parent().context("path has no parent")?;
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    tmp.write_all(bytes)?;
    tmp.as_file().sync_all()?;
    tmp.persist(path)?;
    fs::File::open(dir)?.sync_all()?;
    Ok(())
}
pub fn validate_xmp(packet: &str) -> Result<i8> {
    if packet.trim().is_empty() || packet.contains("<!DOCTYPE") || packet.contains("<!ENTITY") {
        return Err(invalid("Invalid XMP packet"));
    }
    let values = aigle_exiv2_sys::get(packet, &["Xmp.xmp.Rating"])
        .map_err(|_| invalid("Invalid XMP packet"))?;
    let rating = values[0]
        .as_ref()
        .map(|r| r.parse::<i8>())
        .transpose()
        .map_err(|_| invalid("Invalid rating"))?
        .unwrap_or(0);
    if !(-1..=5).contains(&rating) {
        return Err(invalid("Rating must be -1 through 5"));
    }
    Ok(rating)
}
pub fn inspect(path: &Path) -> Result<Inspection> {
    let mut raw = aigle_libraw_sys::Raw::open(path)?;
    let flip = raw.meta()?.flip;
    let facts = aigle_library::read_facts(path);
    let loaded = Store::new().read(path)?;
    let rev = revision(&sidecar(path))?;
    let thumbnail = (|| -> Result<Vec<u8>> {
        let thumb = raw.thumbnail()?;
        let image = match thumb {
            aigle_libraw_sys::Thumb::Jpeg(bytes) => {
                let mut reader =
                    image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format()?;
                let mut limits = image::Limits::default();
                limits.max_alloc = Some(128 * 1024 * 1024);
                limits.max_image_width = Some(16384);
                limits.max_image_height = Some(16384);
                reader.limits(limits);
                reader.decode()?
            }
            aigle_libraw_sys::Thumb::Rgb8 {
                width,
                height,
                data,
            } => {
                ensure!(
                    u64::from(width) * u64::from(height) <= 40_000_000,
                    "embedded preview too large"
                );
                image::DynamicImage::ImageRgb8(
                    image::RgbImage::from_raw(width, height, data)
                        .context("invalid RGB preview")?,
                )
            }
        };
        let image = match flip {
            1 => image.fliph(),
            2 => image.flipv(),
            3 => image.rotate180(),
            4 => image.rotate90().fliph(),
            5 => image.rotate270(),
            6 => image.rotate90(),
            7 => image.rotate90().flipv(),
            _ => image,
        };
        let thumb = image
            .thumbnail(image.width().min(1600), image.height().min(1600))
            .to_rgb8();
        let mut bytes = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 85).encode_image(&thumb)?;
        Ok(bytes)
    })();
    let (preview, warning) = match thumbnail {
        Ok(bytes) => (Some(bytes), None),
        Err(e) => {
            tracing::debug!(error=?e,"embedded preview unavailable");
            (None, Some("No usable embedded preview".into()))
        }
    };
    Ok(Inspection {
        captured: facts.captured,
        camera: facts.camera,
        lens: facts.lens,
        rating: loaded.meta.rating,
        revision: rev,
        preview,
        warning,
    })
}

pub fn update_rating(packet: Option<&str>, rating: i8) -> Result<String> {
    let rating = rating.to_string();
    Ok(aigle_exiv2_sys::update(
        packet,
        &[("Xmp.xmp.Rating", Some(rating.as_str()))],
    )?)
}
