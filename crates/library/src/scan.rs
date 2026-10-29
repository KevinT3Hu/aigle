use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// One photo of a folder, from the directory listing only.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PhotoEntry {
    pub path: PathBuf,
    pub name: String,
    pub bytes: u64,
    pub modified: Option<SystemTime>,
}

/// Subfolders deeper than this below the scanned folder are not listed.
pub const MAX_DEPTH: usize = 32;

/// The photos directly in `dir` (not recursive), sorted by name (case-insensitive). Hidden
/// files and files the engine cannot open are skipped; sidecars are never listed.
pub fn scan(dir: &Path) -> std::io::Result<Vec<PhotoEntry>> {
    let mut out = Vec::new();
    list(dir, &mut out, None)?;
    sort(&mut out);
    Ok(out)
}

/// The photos in `dir` and in its subfolders, sorted by name, then by path. Hidden
/// folders, symbolic links to folders and folders more than [`MAX_DEPTH`] levels down are
/// skipped, as are subfolders that cannot be read (logged).
pub fn scan_recursive(dir: &Path) -> std::io::Result<Vec<PhotoEntry>> {
    let mut out = Vec::new();
    let mut pending = Vec::new();
    list(dir, &mut out, Some(&mut pending))?;
    let mut level = 1;
    while !pending.is_empty() && level <= MAX_DEPTH {
        let mut next = Vec::new();
        for sub in std::mem::take(&mut pending) {
            if let Err(e) = list(&sub, &mut out, Some(&mut next)) {
                tracing::warn!(dir = %sub.display(), "not listed: {e}");
            }
        }
        pending = next;
        level += 1;
    }
    sort(&mut out);
    Ok(out)
}

/// Append the photos of `dir` to `out`, and its subfolders to `subdirs` when given.
fn list(
    dir: &Path,
    out: &mut Vec<PhotoEntry>,
    mut subdirs: Option<&mut Vec<PathBuf>>,
) -> std::io::Result<()> {
    for e in std::fs::read_dir(dir)? {
        let Ok(e) = e else { continue };
        let path = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        if let Some(subdirs) = subdirs.as_deref_mut()
            && e.file_type().is_ok_and(|t| t.is_dir())
        {
            subdirs.push(path);
            continue;
        }
        if !aigle_engine_api::is_photo_path(&path) {
            continue;
        }
        let Ok(meta) = std::fs::metadata(&path) else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        out.push(PhotoEntry {
            path,
            name,
            bytes: meta.len(),
            modified: meta.modified().ok(),
        });
    }
    Ok(())
}

fn sort(entries: &mut [PhotoEntry]) {
    entries.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.path.cmp(&b.path))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_photos_sorted_and_skips_the_rest() {
        let dir = crate::sidecar::tests::temp_dir("scan");
        for f in [
            "b.NEF",
            "A.jpg",
            "a.xmp",
            ".hidden.cr2",
            "notes.txt",
            "c.CR3",
        ] {
            std::fs::write(dir.join(f), b"x").unwrap();
        }
        std::fs::create_dir(dir.join("sub.dng")).unwrap();
        let names: Vec<String> = scan(&dir).unwrap().into_iter().map(|e| e.name).collect();
        assert_eq!(names, ["A.jpg", "b.NEF", "c.CR3"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn recursive_scan_lists_subfolders_but_not_hidden_or_linked_ones() {
        let dir = crate::sidecar::tests::temp_dir("scan-tree");
        for f in [
            "top.jpg",
            "2024/a.NEF",
            "2024/day/a.NEF",
            "2024/day/deep/z.dng",
            ".hidden/x.jpg",
            "2024/.cache/y.jpg",
        ] {
            let p = dir.join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, b"x").unwrap();
        }
        std::os::unix::fs::symlink(&dir, dir.join("2024/loop")).unwrap();
        let rel: Vec<String> = scan_recursive(&dir)
            .unwrap()
            .into_iter()
            .map(|e| e.path.strip_prefix(&dir).unwrap().display().to_string())
            .collect();
        assert_eq!(
            rel,
            [
                "2024/a.NEF",
                "2024/day/a.NEF",
                "top.jpg",
                "2024/day/deep/z.dng"
            ]
        );
        assert_eq!(scan(&dir).unwrap().len(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
