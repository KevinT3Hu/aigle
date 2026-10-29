//! Importing photos from a memory card or another folder: each photo is copied with its
//! sidecars into a destination folder, into subfolders and under file names made from
//! templates, and optionally into a backup folder too.
//!
//! Nothing is overwritten. A photo whose target already holds the same bytes counts as a
//! duplicate and is skipped; a different file there makes the copy take a `-2`, `-3`, …
//! name. Every copy is written under a temporary name, synced, read back and compared
//! with the source before it gets its name.

use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::SystemTime;

use crate::facts::Facts;
use crate::files::sidecars_of;
use crate::scan::PhotoEntry;
use crate::sidecar::sidecar_candidates;

/// Default subfolder template: one folder per capture day, inside one per year.
pub const DEFAULT_FOLDERS: &str = "{year}/{year}-{month}-{day}";
/// Default file name template: the name the camera gave.
pub const DEFAULT_NAMES: &str = "{name}";

const CHUNK: usize = 1 << 20;
const REPORT_EVERY: u64 = 16 << 20;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportOptions {
    pub dest: PathBuf,
    /// Subfolders below `dest`, separated by `/`; empty for none.
    pub folders: String,
    /// The file name without its extension, which is kept.
    pub names: String,
    /// A second folder that receives the same files under the same subfolders.
    pub backup: Option<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum TemplateError {
    #[error("unknown field {{{0}}}")]
    Unknown(String),
    #[error("a {{ is not closed")]
    Unclosed,
    #[error("the file name cannot contain /")]
    Separator,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Token {
    Name,
    Year,
    Month,
    Day,
    Hour,
    Minute,
    Second,
    Seq(usize),
    Camera,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Part {
    Text(String),
    Token(Token),
}

/// The fields a template may use: `{name}` (the original file name without extension),
/// `{year}`, `{month}`, `{day}`, `{hour}`, `{minute}`, `{second}` (capture time, else the
/// file's modification time), `{camera}`, and `{seq}` (1-based number of the photo in
/// capture order, 4 digits; `{seq:N}` for N digits).
pub const FIELDS: &[&str] = &[
    "name", "year", "month", "day", "hour", "minute", "second", "camera", "seq",
];

fn parse(template: &str) -> Result<Vec<Part>, TemplateError> {
    let mut parts = Vec::new();
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        if open > 0 {
            parts.push(Part::Text(rest[..open].to_string()));
        }
        let close = rest[open..].find('}').ok_or(TemplateError::Unclosed)? + open;
        let field = &rest[open + 1..close];
        let token = match field {
            "name" => Token::Name,
            "year" => Token::Year,
            "month" => Token::Month,
            "day" => Token::Day,
            "hour" => Token::Hour,
            "minute" => Token::Minute,
            "second" => Token::Second,
            "camera" => Token::Camera,
            "seq" => Token::Seq(4),
            f => match f.strip_prefix("seq:").and_then(|n| n.parse::<usize>().ok()) {
                Some(n) if (1..=9).contains(&n) => Token::Seq(n),
                _ => return Err(TemplateError::Unknown(f.to_string())),
            },
        };
        parts.push(Part::Token(token));
        rest = &rest[close + 1..];
    }
    if !rest.is_empty() {
        parts.push(Part::Text(rest.to_string()));
    }
    Ok(parts)
}

/// Whether the templates of `options` can be used; the first problem otherwise.
pub fn check(options: &ImportOptions) -> Result<(), TemplateError> {
    parse(&options.folders)?;
    if options.names.contains('/') {
        return Err(TemplateError::Separator);
    }
    parse(&options.names).map(drop)
}

/// Whether the templates use the capture time or the camera, which are read from the
/// photo's EXIF.
pub fn needs_facts(options: &ImportOptions) -> bool {
    [&options.folders, &options.names].iter().any(|t| {
        parse(t).is_ok_and(|parts| {
            parts.iter().any(|p| {
                matches!(
                    p,
                    Part::Token(
                        Token::Year
                            | Token::Month
                            | Token::Day
                            | Token::Hour
                            | Token::Minute
                            | Token::Second
                            | Token::Camera
                    )
                )
            })
        })
    })
}

/// A photo to import.
#[derive(Clone, Debug)]
pub struct Source {
    pub entry: PhotoEntry,
    /// From its EXIF; `None` when not read or unknown.
    pub facts: Option<Facts>,
}

/// The photos in `dir` and its subfolders, with their EXIF facts when `facts` (see
/// [`needs_facts`]). Stops early with what was found when `cancel` is set.
pub fn read_sources(dir: &Path, facts: bool, cancel: &AtomicBool) -> io::Result<Vec<Source>> {
    let mut out = Vec::new();
    for entry in crate::scan_recursive(dir)? {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let facts = facts.then(|| crate::read_facts(&entry.path));
        out.push(Source { entry, facts });
    }
    Ok(out)
}

/// `YYYY-MM-DD HH:MM:SS` local time.
fn when(source: &Source) -> String {
    if let Some(c) = source.facts.as_ref().and_then(|f| f.captured.clone()) {
        return c;
    }
    let t: chrono::DateTime<chrono::Local> = source
        .entry
        .modified
        .unwrap_or(SystemTime::UNIX_EPOCH)
        .into();
    t.format("%Y-%m-%d %H:%M:%S").to_string()
}

fn render(parts: &[Part], source: &Source, when: &str, seq: usize) -> String {
    let stem = Path::new(&source.entry.name)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut out = String::new();
    for p in parts {
        match p {
            Part::Text(t) => out.push_str(t),
            Part::Token(t) => {
                let v = match t {
                    Token::Name => stem.clone(),
                    Token::Year => when[..4].to_string(),
                    Token::Month => when[5..7].to_string(),
                    Token::Day => when[8..10].to_string(),
                    Token::Hour => when[11..13].to_string(),
                    Token::Minute => when[14..16].to_string(),
                    Token::Second => when[17..19].to_string(),
                    Token::Seq(w) => format!("{seq:0w$}"),
                    Token::Camera => source
                        .facts
                        .as_ref()
                        .and_then(|f| f.camera.clone())
                        .unwrap_or_else(|| "Unknown".into()),
                };
                out.push_str(&v.replace(['/', '\\', '\0'], "_"));
            }
        }
    }
    out
}

/// A path component made from a template: never empty, hidden, `.` or `..`.
fn component(s: &str, fallback: &str) -> String {
    let s = s.trim().replace('\0', "_");
    match s.as_str() {
        "" | "." | ".." => fallback.to_string(),
        s if s.starts_with('.') => format!("_{}", &s[1..]),
        s => s.to_string(),
    }
}

/// Where one photo goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Planned {
    pub source: PathBuf,
    pub bytes: u64,
    /// Below the destination (and the backup) folder.
    pub relative: PathBuf,
}

/// The targets of `sources`, in capture order. `{seq}` numbers photos that share a folder
/// and a file stem (RAW + JPEG) alike, so that they keep sharing a stem.
pub fn plan(sources: &[Source], options: &ImportOptions) -> Result<Vec<Planned>, TemplateError> {
    check(options)?;
    let folders = parse(&options.folders)?;
    let names = parse(&options.names)?;
    let mut order: Vec<(String, &Source)> = sources.iter().map(|s| (when(s), s)).collect();
    order.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then_with(|| a.1.entry.path.cmp(&b.1.entry.path))
    });
    let mut groups: Vec<(PathBuf, usize)> = Vec::new();
    let mut out = Vec::with_capacity(order.len());
    for (when, s) in order {
        let path = &s.entry.path;
        let group = path.with_extension("");
        let seq = match groups.iter().find(|(g, _)| *g == group) {
            Some((_, n)) => *n,
            None => {
                groups.push((group, groups.len() + 1));
                groups.len()
            }
        };
        let stem = Path::new(&s.entry.name)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "photo".into());
        let mut relative: PathBuf = render(&folders, s, &when, seq)
            .split('/')
            .filter(|c| !c.trim().is_empty())
            .map(|c| component(c, "_"))
            .collect();
        let name = component(&render(&names, s, &when, seq), &stem);
        relative.push(match path.extension() {
            Some(ext) => format!("{name}.{}", ext.to_string_lossy()),
            None => name,
        });
        out.push(Planned {
            source: path.clone(),
            bytes: s.entry.bytes,
            relative,
        });
    }
    Ok(out)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Progress {
    pub files: usize,
    pub total_files: usize,
    pub bytes: u64,
    pub total_bytes: u64,
}

#[derive(Debug, Default)]
pub struct ImportReport {
    /// The photos now in the destination folder.
    pub imported: Vec<PathBuf>,
    /// Photos whose target already held the same file.
    pub duplicates: usize,
    pub failed: Vec<(PathBuf, String)>,
    /// Photos imported, but not (fully) copied to the backup folder.
    pub backup_failed: Vec<(PathBuf, String)>,
    pub cancelled: bool,
}

enum Placed {
    New(PathBuf),
    Duplicate,
}

struct Copier<'a> {
    cancel: &'a AtomicBool,
    progress: &'a mut dyn FnMut(Progress),
    state: Progress,
    reported: u64,
}

impl Copier<'_> {
    fn advance(&mut self, bytes: u64) {
        self.state.bytes += bytes;
        if self.state.bytes - self.reported >= REPORT_EVERY {
            self.reported = self.state.bytes;
            (self.progress)(self.state);
        }
    }

    fn cancelled(&self) -> io::Result<()> {
        if self.cancel.load(Ordering::Relaxed) {
            Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"))
        } else {
            Ok(())
        }
    }

    /// Copy `from` to `want`, or to the first free `-N` name after it; a target holding the
    /// same bytes is a duplicate. `count` adds the bytes read from `from` to the progress.
    fn place(&mut self, from: &Path, want: &Path, count: bool) -> io::Result<Placed> {
        let dir = want.parent().unwrap_or(Path::new("."));
        std::fs::create_dir_all(dir)?;
        let src_len = std::fs::metadata(from)?.len();
        for n in 1.. {
            let target = numbered(want, n);
            match std::fs::symlink_metadata(&target) {
                Ok(m) => {
                    if m.is_file() && m.len() == src_len && hash(&target)? == hash(from)? {
                        if count {
                            self.advance(src_len);
                        }
                        return Ok(Placed::Duplicate);
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => {
                    match self.copy_verified(from, &target, count) {
                        Ok(()) => return Ok(Placed::New(target)),
                        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
                        Err(e) => return Err(e),
                    }
                }
                Err(e) => return Err(e),
            }
        }
        unreachable!()
    }

    /// Copy through a temporary file in the target folder, compare what was written with
    /// what was read, then give it its name without replacing anything.
    fn copy_verified(&mut self, from: &Path, to: &Path, count: bool) -> io::Result<()> {
        let name = to.file_name().unwrap_or_default().to_string_lossy();
        let tmp = to.with_file_name(format!(".{name}.aigle-import"));
        let r = (|| {
            let mut src = std::fs::File::open(from)?;
            let meta = src.metadata()?;
            let mut dst = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&tmp)?;
            let mut hasher = blake3::Hasher::new();
            let mut buf = vec![0u8; CHUNK];
            let mut done = 0u64;
            loop {
                self.cancelled()?;
                let n = src.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                hasher.update(&buf[..n]);
                dst.write_all(&buf[..n])?;
                done += n as u64;
                if count {
                    self.advance(n as u64);
                }
            }
            if done != meta.len() {
                return Err(io::Error::other("the file changed while it was copied"));
            }
            let mut times = std::fs::FileTimes::new();
            if let Ok(m) = meta.modified() {
                times = times.set_modified(m);
            }
            dst.set_times(times)?;
            dst.sync_all()?;
            let _ = rustix::fs::fadvise(&dst, 0, None, rustix::fs::Advice::DontNeed);
            drop(dst);
            if hash(&tmp)? != hasher.finalize() {
                return Err(io::Error::other("the copy differs from the original"));
            }
            give_name(&tmp, to)
        })();
        if r.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        r
    }

    /// Copy the photo and its sidecars to `want`. Sidecars of a duplicate are left alone:
    /// the existing ones may hold later edits.
    fn import_one(&mut self, from: &Path, want: &Path) -> io::Result<Placed> {
        let placed = self.place(from, want, true)?;
        if let Placed::New(photo) = &placed {
            let [full_from, _] = sidecar_candidates(from);
            let [full, short] = sidecar_candidates(photo);
            let mut copied = Vec::new();
            for sidecar in sidecars_of(from) {
                let to = if sidecar == full_from { &full } else { &short };
                match self.place_exact(&sidecar, to) {
                    Ok(true) => copied.push(to.clone()),
                    Ok(false) => {}
                    Err(e) => {
                        for p in copied.iter().chain([photo]) {
                            let _ = std::fs::remove_file(p);
                        }
                        return Err(io::Error::new(
                            e.kind(),
                            format!("{}: {e}", sidecar.display()),
                        ));
                    }
                }
            }
        }
        Ok(placed)
    }

    /// Copy `from` to exactly `to`; false when `to` already holds the same bytes, an error
    /// when it holds others.
    fn place_exact(&mut self, from: &Path, to: &Path) -> io::Result<bool> {
        if to.exists() {
            if hash(to)? == hash(from)? {
                return Ok(false);
            }
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("{} already exists", to.display()),
            ));
        }
        self.copy_verified(from, to, false).map(|()| true)
    }
}

/// `IMG_1.CR3` for 1, `IMG_1-2.CR3` for 2.
fn numbered(path: &Path, n: usize) -> PathBuf {
    if n == 1 {
        return path.to_path_buf();
    }
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    let name = match path.extension() {
        Some(ext) => format!("{stem}-{n}.{}", ext.to_string_lossy()),
        None => format!("{stem}-{n}"),
    };
    path.with_file_name(name)
}

fn hash(path: &Path) -> io::Result<blake3::Hash> {
    let mut h = blake3::Hasher::new();
    h.update_reader(std::fs::File::open(path)?)?;
    Ok(h.finalize())
}

fn give_name(tmp: &Path, to: &Path) -> io::Result<()> {
    use rustix::fs::{CWD, RenameFlags, renameat_with};
    match renameat_with(CWD, tmp, CWD, to, RenameFlags::NOREPLACE) {
        Ok(()) => Ok(()),
        Err(rustix::io::Errno::INVAL) | Err(rustix::io::Errno::NOSYS) => {
            std::fs::hard_link(tmp, to)?;
            std::fs::remove_file(tmp)
        }
        Err(e) => Err(e.into()),
    }
}

/// Copy the planned photos into `options.dest` (and the backup folder). `progress` is
/// called now and then from this thread; setting `cancel` stops after the current file
/// (a partly copied file is removed).
pub fn run(
    planned: &[Planned],
    options: &ImportOptions,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(Progress),
) -> ImportReport {
    let mut report = ImportReport::default();
    let mut c = Copier {
        cancel,
        progress,
        state: Progress {
            total_files: planned.len(),
            total_bytes: planned.iter().map(|p| p.bytes).sum(),
            ..Progress::default()
        },
        reported: 0,
    };
    (c.progress)(c.state);
    for p in planned {
        if cancel.load(Ordering::Relaxed) {
            report.cancelled = true;
            break;
        }
        let before = c.state.bytes;
        match c.import_one(&p.source, &options.dest.join(&p.relative)) {
            Ok(Placed::New(path)) => {
                if let Some(b) = &options.backup
                    && let Err(e) = c.import_one(&p.source, &b.join(&p.relative))
                {
                    if c.cancelled().is_err() {
                        report.cancelled = true;
                    }
                    report.backup_failed.push((p.source.clone(), e.to_string()));
                }
                report.imported.push(path);
            }
            Ok(Placed::Duplicate) => report.duplicates += 1,
            Err(_) if c.cancelled().is_err() => {
                report.cancelled = true;
                break;
            }
            Err(e) => report.failed.push((p.source.clone(), e.to_string())),
        }
        c.state.bytes = before + p.bytes;
        c.state.files += 1;
        c.reported = c.state.bytes;
        (c.progress)(c.state);
    }
    report
}

/// A mounted removable volume (memory card, USB drive).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Volume {
    pub path: PathBuf,
    pub name: String,
    /// Has a `DCIM` folder, as camera cards do.
    pub camera: bool,
}

/// The volumes mounted below `/media` or `/run/media` (where desktop environments mount
/// removable media), cards with a `DCIM` folder first.
pub fn removable_volumes() -> Vec<Volume> {
    let mounts = std::fs::read_to_string("/proc/self/mounts").unwrap_or_default();
    let mut out: Vec<Volume> = volumes_in(&mounts)
        .into_iter()
        .filter(|p| p.is_dir())
        .map(|path| Volume {
            name: path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            camera: path.join("DCIM").is_dir(),
            path,
        })
        .collect();
    out.sort_by(|a, b| b.camera.cmp(&a.camera).then_with(|| a.name.cmp(&b.name)));
    out
}

fn volumes_in(mounts: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for line in mounts.lines() {
        let Some(point) = line.split(' ').nth(1) else {
            continue;
        };
        let point = unescape(point);
        if (point.starts_with("/media/") || point.starts_with("/run/media/"))
            && !out
                .iter()
                .any(|p: &PathBuf| p.as_os_str() == point.as_str())
        {
            out.push(PathBuf::from(point));
        }
    }
    out
}

/// `/proc/mounts` writes space, tab, newline and backslash as octal escapes.
fn unescape(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\'
            && i + 3 < b.len()
            && b[i + 1..i + 4].iter().all(|c| (b'0'..=b'7').contains(c))
        {
            let v = (b[i + 1] - b'0') * 64 + (b[i + 2] - b'0') * 8 + (b[i + 3] - b'0');
            out.push(v);
            i += 4;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sidecar::tests::temp_dir;

    fn source(path: &Path, captured: Option<&str>) -> Source {
        Source {
            entry: PhotoEntry {
                path: path.to_path_buf(),
                name: path.file_name().unwrap().to_string_lossy().into_owned(),
                bytes: std::fs::metadata(path).map_or(0, |m| m.len()),
                modified: None,
            },
            facts: Some(Facts {
                camera: Some("Canon EOS R6".into()),
                lens: None,
                captured: captured.map(str::to_string),
            }),
        }
    }

    fn options(dest: &Path, folders: &str, names: &str) -> ImportOptions {
        ImportOptions {
            dest: dest.to_path_buf(),
            folders: folders.into(),
            names: names.into(),
            backup: None,
        }
    }

    #[test]
    fn templates_parse_and_render() {
        assert_eq!(
            parse("{year}x{seq:2}").unwrap(),
            [
                Part::Token(Token::Year),
                Part::Text("x".into()),
                Part::Token(Token::Seq(2))
            ]
        );
        assert_eq!(parse("{nope}"), Err(TemplateError::Unknown("nope".into())));
        assert_eq!(parse("{year"), Err(TemplateError::Unclosed));
        assert_eq!(
            check(&options(Path::new("/d"), "", "a/b")),
            Err(TemplateError::Separator)
        );
        let o = options(Path::new("/d"), DEFAULT_FOLDERS, DEFAULT_NAMES);
        assert!(needs_facts(&o));
        assert!(!needs_facts(&options(Path::new("/d"), "", "{name}-{seq}")));

        let srcs = [
            source(Path::new("/card/DCIM/B_2.CR3"), Some("2024-05-02 08:00:00")),
            source(Path::new("/card/DCIM/A_1.CR3"), Some("2024-05-01 23:59:59")),
            source(Path::new("/card/DCIM/A_1.JPG"), Some("2024-05-01 23:59:59")),
        ];
        let p = plan(&srcs, &o).unwrap();
        let rel: Vec<_> = p.iter().map(|p| p.relative.display().to_string()).collect();
        assert_eq!(
            rel,
            [
                "2024/2024-05-01/A_1.CR3",
                "2024/2024-05-01/A_1.JPG",
                "2024/2024-05-02/B_2.CR3"
            ]
        );
        let p = plan(
            &srcs,
            &options(Path::new("/d"), "{camera}", "{month}{day}-{seq:3}"),
        )
        .unwrap();
        let rel: Vec<_> = p.iter().map(|p| p.relative.display().to_string()).collect();
        assert_eq!(
            rel,
            [
                "Canon EOS R6/0501-001.CR3",
                "Canon EOS R6/0501-001.JPG",
                "Canon EOS R6/0502-002.CR3"
            ]
        );
        let p = plan(&srcs[..1], &options(Path::new("/d"), "../{name}/./", " ")).unwrap();
        assert_eq!(p[0].relative, PathBuf::from("_/B_2/_/B_2.CR3"));
        let p = plan(&srcs[..1], &options(Path::new("/d"), "", ".x{name}")).unwrap();
        assert_eq!(p[0].relative, PathBuf::from("_xB_2.CR3"));
    }

    #[test]
    fn imports_with_sidecars_backup_and_duplicates() {
        let root = temp_dir("import");
        let card = root.join("card/DCIM/100CANON");
        std::fs::create_dir_all(&card).unwrap();
        std::fs::write(card.join("IMG_1.CR3"), b"raw one").unwrap();
        std::fs::write(card.join("IMG_1.JPG"), b"jpeg one").unwrap();
        std::fs::write(card.join("IMG_1.xmp"), b"<x/>").unwrap();
        std::fs::write(card.join("IMG_2.CR3"), b"raw two").unwrap();
        let dest = root.join("photos");
        let backup = root.join("backup");
        std::fs::create_dir_all(dest.join("2024")).unwrap();
        std::fs::write(dest.join("2024/IMG_2.CR3"), b"another photo").unwrap();

        let entries = crate::scan_recursive(&root.join("card")).unwrap();
        assert_eq!(entries.len(), 3);
        let srcs: Vec<Source> = entries
            .iter()
            .map(|e| source(&e.path, Some("2024-05-01 10:00:00")))
            .collect();
        let mut o = options(&dest, "{year}", "{name}");
        o.backup = Some(backup.clone());
        let planned = plan(&srcs, &o).unwrap();
        let mut seen = Vec::new();
        let report = run(&planned, &o, &AtomicBool::new(false), &mut |p| seen.push(p));
        assert!(report.failed.is_empty(), "{:?}", report.failed);
        assert!(
            report.backup_failed.is_empty(),
            "{:?}",
            report.backup_failed
        );
        assert_eq!(report.imported.len(), 3);
        assert_eq!(report.duplicates, 0);
        let last = seen.last().unwrap();
        assert_eq!((last.files, last.bytes), (3, 22));
        let read = |p: PathBuf| std::fs::read(p).unwrap();
        for d in [&dest, &backup] {
            assert_eq!(read(d.join("2024/IMG_1.CR3")), b"raw one");
            assert_eq!(read(d.join("2024/IMG_1.JPG")), b"jpeg one");
            assert_eq!(read(d.join("2024/IMG_1.xmp")), b"<x/>");
        }
        assert_eq!(read(dest.join("2024/IMG_2.CR3")), b"another photo");
        assert_eq!(read(dest.join("2024/IMG_2-2.CR3")), b"raw two");
        assert_eq!(read(backup.join("2024/IMG_2.CR3")), b"raw two");
        let leftovers = std::fs::read_dir(dest.join("2024"))
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with('.'))
            .count();
        assert_eq!(leftovers, 0);

        std::fs::write(dest.join("2024/IMG_1.xmp"), b"<edited/>").unwrap();
        let again = run(&planned, &o, &AtomicBool::new(false), &mut |_| {});
        assert_eq!(again.duplicates, 3, "{again:?}");
        assert!(again.imported.is_empty() && again.failed.is_empty());
        assert_eq!(read(dest.join("2024/IMG_1.xmp")), b"<edited/>");

        let cancelled = run(&planned, &o, &AtomicBool::new(true), &mut |_| {});
        assert!(cancelled.cancelled && cancelled.imported.is_empty());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn removable_volumes_come_from_media_mounts() {
        let mounts = "/dev/sda1 / ext4 rw 0 0\n\
                      /dev/sdb1 /media/me/EOS_DIGITAL vfat rw 0 0\n\
                      /dev/sdc1 /run/media/me/My\\040Card exfat rw 0 0\n\
                      tmpfs /run/user/1000 tmpfs rw 0 0\n";
        assert_eq!(
            volumes_in(mounts),
            [
                PathBuf::from("/media/me/EOS_DIGITAL"),
                PathBuf::from("/run/media/me/My Card")
            ]
        );
    }
}
