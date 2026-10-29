//! Moving, renaming and trashing photos together with their sidecars (doc 01 §9.1: a photo
//! is the file plus its sidecar). The catalog is updated by the caller.

use std::ffi::OsStr;
use std::io;
use std::path::{Path, PathBuf};

use aigle_engine_api::{is_photo_path, is_raw_path};

use crate::sidecar::sidecar_candidates;

#[derive(Debug, thiserror::Error)]
pub enum FileError {
    #[error("{} already exists", .0.display())]
    Exists(PathBuf),
    #[error("{}: {source}", path.display())]
    Io { path: PathBuf, source: io::Error },
    #[error("{}: {message}", path.display())]
    Trash { path: PathBuf, message: String },
}

fn io_err(path: &Path) -> impl FnOnce(io::Error) -> FileError + '_ {
    move |source| FileError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// Another photo than `except` in `dir` has the file stem `stem`.
fn stem_taken(dir: &Path, stem: &OsStr, except: &[&Path]) -> bool {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return false;
    };
    rd.flatten().any(|e| {
        let p = e.path();
        p.file_stem() == Some(stem) && !except.contains(&p.as_path()) && is_photo_path(&p)
    })
}

/// The sidecars that belong to `photo`: its full-name sidecar (`IMG_1.CR3.xmp`), and the
/// short one (`IMG_1.xmp`) unless it belongs to another photo with the same stem.
pub fn sidecars_of(photo: &Path) -> Vec<PathBuf> {
    let [full, short] = sidecar_candidates(photo);
    let mut out = Vec::new();
    let has_full = full.is_file();
    if has_full {
        out.push(full);
    }
    if short.is_file() {
        let dir = photo.parent().unwrap_or(Path::new("."));
        let sibling = photo
            .file_stem()
            .is_some_and(|s| stem_taken(dir, s, &[photo]));
        if !sibling || (is_raw_path(photo) && !has_full) {
            out.push(short);
        }
    }
    out
}

/// Where the sidecar `sidecar` of `from` goes when the photo moves to `to`. A short name
/// becomes a full one when another photo at the destination has the same stem, so that it
/// is not attributed to that photo.
fn sidecar_target(from: &Path, to: &Path, sidecar: &Path, others: &[PathBuf]) -> PathBuf {
    let [full_from, _] = sidecar_candidates(from);
    let [full_to, short_to] = sidecar_candidates(to);
    if sidecar == full_from {
        return full_to;
    }
    let dir = to.parent().unwrap_or(Path::new("."));
    let shared = to
        .file_stem()
        .is_some_and(|s| stem_taken(dir, s, &[from, to]));
    if shared && !others.contains(&full_from) {
        full_to
    } else {
        short_to
    }
}

/// Rename without replacing an existing file; across file systems, copy then delete.
fn move_file(from: &Path, to: &Path) -> Result<(), FileError> {
    use rustix::fs::{CWD, RenameFlags, renameat_with};
    match renameat_with(CWD, from, CWD, to, RenameFlags::NOREPLACE) {
        Ok(()) => Ok(()),
        Err(rustix::io::Errno::EXIST) => Err(FileError::Exists(to.to_path_buf())),
        Err(rustix::io::Errno::XDEV) => copy_then_delete(from, to),
        // The file system does not support RENAME_NOREPLACE (some FUSE and network ones).
        Err(rustix::io::Errno::INVAL) | Err(rustix::io::Errno::NOSYS) => {
            if std::fs::symlink_metadata(to).is_ok() {
                return Err(FileError::Exists(to.to_path_buf()));
            }
            std::fs::rename(from, to).map_err(io_err(from))
        }
        Err(e) => Err(io_err(from)(e.into())),
    }
}

fn copy_then_delete(from: &Path, to: &Path) -> Result<(), FileError> {
    let r = (|| {
        let mut src = std::fs::File::open(from)?;
        let meta = src.metadata()?;
        let mut dst = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(to)?;
        io::copy(&mut src, &mut dst)?;
        let mut times = std::fs::FileTimes::new();
        if let Ok(m) = meta.modified() {
            times = times.set_modified(m);
        }
        if let Ok(a) = meta.accessed() {
            times = times.set_accessed(a);
        }
        dst.set_times(times)?;
        dst.set_permissions(meta.permissions())?;
        dst.sync_all()
    })();
    match r {
        Ok(()) => std::fs::remove_file(from).map_err(io_err(from)),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
            Err(FileError::Exists(to.to_path_buf()))
        }
        Err(e) => {
            let _ = std::fs::remove_file(to);
            Err(io_err(to)(e))
        }
    }
}

/// Move (or rename) the photo `from` to `to` with its sidecars. Nothing is overwritten: if
/// the photo or a sidecar would replace an existing file, nothing moves. If a sidecar
/// cannot be moved, the files already moved are moved back.
pub fn move_photo(from: &Path, to: &Path) -> Result<(), FileError> {
    if from == to {
        return Ok(());
    }
    let sidecars = sidecars_of(from);
    let mut plan = vec![(from.to_path_buf(), to.to_path_buf())];
    for s in &sidecars {
        plan.push((s.clone(), sidecar_target(from, to, s, &sidecars)));
    }
    // Renaming only the case or the extension can map a sidecar onto itself.
    plan.retain(|(a, b)| a != b);
    for (_, dest) in &plan {
        if std::fs::symlink_metadata(dest).is_ok() {
            return Err(FileError::Exists(dest.clone()));
        }
    }
    // A sidecar left at the destination would be taken for the moved photo's, unless the
    // short name belongs to a namesake there.
    let [full_to, short_to] = sidecar_candidates(to);
    let dir = to.parent().unwrap_or(Path::new("."));
    let shared = to
        .file_stem()
        .is_some_and(|s| stem_taken(dir, s, &[from, to]));
    for c in [&full_to, &short_to] {
        let ours = sidecars.contains(c);
        if c.is_file() && !ours && !(c == &short_to && shared) {
            return Err(FileError::Exists(c.clone()));
        }
    }
    let mut done: Vec<&(PathBuf, PathBuf)> = Vec::new();
    for step in &plan {
        if let Err(e) = move_file(&step.0, &step.1) {
            for (a, b) in done.into_iter().rev() {
                if let Err(back) = move_file(b, a) {
                    tracing::error!("cannot move {} back: {back}", b.display());
                }
            }
            return Err(e);
        }
        done.push(step);
    }
    Ok(())
}

/// Move `photo` and its sidecars to the desktop trash, where they can be restored.
pub fn trash_photo(photo: &Path) -> Result<(), FileError> {
    let mut items = vec![photo.to_path_buf()];
    items.extend(sidecars_of(photo));
    trash::delete_all(&items).map_err(|e| FileError::Trash {
        path: photo.to_path_buf(),
        message: e.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sidecar::tests::temp_dir;

    fn names(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    }

    fn touch(dir: &Path, files: &[&str]) {
        for f in files {
            std::fs::write(dir.join(f), f.as_bytes()).unwrap();
        }
    }

    #[test]
    fn sidecars_follow_their_photo() {
        let a = temp_dir("move-a");
        let b = temp_dir("move-b");
        touch(
            &a,
            &[
                "IMG_1.CR3",
                "IMG_1.xmp",
                "IMG_1.JPG",
                "IMG_1.JPG.xmp",
                "IMG_2.NEF",
            ],
        );
        touch(&a, &["IMG_2.NEF.xmp", "IMG_2.xmp"]);

        // The short sidecar is the raw's: it goes with it; the JPEG keeps its own.
        move_photo(&a.join("IMG_1.CR3"), &b.join("IMG_1.CR3")).unwrap();
        move_photo(&a.join("IMG_1.JPG"), &b.join("IMG_1.JPG")).unwrap();
        assert_eq!(
            names(&b),
            ["IMG_1.CR3", "IMG_1.JPG", "IMG_1.JPG.xmp", "IMG_1.xmp"]
        );

        // Renaming keeps both forms of sidecar name.
        move_photo(&a.join("IMG_2.NEF"), &a.join("sea.NEF")).unwrap();
        assert_eq!(names(&a), ["sea.NEF", "sea.NEF.xmp", "sea.xmp"]);
        assert_eq!(std::fs::read(a.join("sea.xmp")).unwrap(), b"IMG_2.xmp");
        std::fs::remove_dir_all(&a).unwrap();
        std::fs::remove_dir_all(&b).unwrap();
    }

    #[test]
    fn a_short_sidecar_gets_the_full_name_next_to_a_namesake() {
        let a = temp_dir("move-c");
        let b = temp_dir("move-d");
        touch(&a, &["P.JPG", "P.xmp"]);
        touch(&b, &["P.CR3"]);
        move_photo(&a.join("P.JPG"), &b.join("P.JPG")).unwrap();
        assert_eq!(names(&b), ["P.CR3", "P.JPG", "P.JPG.xmp"]);
        assert_eq!(std::fs::read(b.join("P.JPG.xmp")).unwrap(), b"P.xmp");
        std::fs::remove_dir_all(&a).unwrap();
        std::fs::remove_dir_all(&b).unwrap();
    }

    #[test]
    fn nothing_is_overwritten() {
        let a = temp_dir("move-e");
        touch(&a, &["x.dng", "x.xmp", "y.dng.xmp", "z.dng", "w.xmp"]);
        for to in ["y.dng", "z.dng", "w.dng"] {
            let r = move_photo(&a.join("x.dng"), &a.join(to));
            assert!(matches!(r, Err(FileError::Exists(_))), "{to}: {r:?}");
        }
        move_photo(&a.join("x.dng"), &a.join("v.dng")).unwrap();
        assert_eq!(names(&a), ["v.dng", "v.xmp", "w.xmp", "y.dng.xmp", "z.dng"]);
        // Only the case of the extension changes; the short sidecar stays.
        move_photo(&a.join("v.dng"), &a.join("v.DNG")).unwrap();
        assert_eq!(names(&a), ["v.DNG", "v.xmp", "w.xmp", "y.dng.xmp", "z.dng"]);
        std::fs::remove_dir_all(&a).unwrap();
    }

    #[test]
    fn copies_across_file_systems_keep_the_time() {
        let a = temp_dir("move-f");
        touch(&a, &["s.nef"]);
        let t = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000);
        std::fs::File::options()
            .write(true)
            .open(a.join("s.nef"))
            .unwrap()
            .set_modified(t)
            .unwrap();
        copy_then_delete(&a.join("s.nef"), &a.join("t.nef")).unwrap();
        assert_eq!(names(&a), ["t.nef"]);
        let m = std::fs::metadata(a.join("t.nef"))
            .unwrap()
            .modified()
            .unwrap();
        assert_eq!(m, t);
        std::fs::remove_dir_all(&a).unwrap();
    }
}
