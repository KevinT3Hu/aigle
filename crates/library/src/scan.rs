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

/// The photos directly in `dir` (not recursive), sorted by name (case-insensitive). Hidden
/// files and files the engine cannot open are skipped; sidecars are never listed.
pub fn scan(dir: &Path) -> std::io::Result<Vec<PhotoEntry>> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir)? {
        let Ok(e) = e else { continue };
        let path = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || !aigle_engine_api::is_photo_path(&path) {
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
    out.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.name.cmp(&b.name))
    });
    Ok(out)
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
}
