//! Exact file duplicates: group by size before streaming BLAKE3 fingerprints.
//! Never deletes files and never mistakes virtual copies for separate files.
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Read,
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
};

#[derive(Default, Debug)]
pub struct Report {
    pub groups: Vec<Vec<PathBuf>>,
    pub failed: Vec<(PathBuf, String)>,
}

pub fn find(paths: Vec<PathBuf>, cancel: &AtomicBool) -> Report {
    let mut report = Report::default();
    let mut by_size: BTreeMap<u64, Vec<PathBuf>> = BTreeMap::new();
    for path in paths.into_iter().collect::<BTreeSet<_>>() {
        if cancel.load(Ordering::Relaxed) {
            return report;
        }
        match std::fs::metadata(&path) {
            Ok(m) if m.is_file() => by_size.entry(m.len()).or_default().push(path),
            Ok(_) => {}
            Err(e) => report.failed.push((path, e.to_string())),
        }
    }
    let mut buffer = vec![0u8; 128 * 1024];
    for (size, paths) in by_size {
        if paths.len() < 2 {
            continue;
        }
        let mut by_hash: BTreeMap<[u8; 32], Vec<PathBuf>> = BTreeMap::new();
        for path in paths {
            if cancel.load(Ordering::Relaxed) {
                return report;
            }
            let hash = (|| -> std::io::Result<Option<[u8; 32]>> {
                let mut file = std::fs::File::open(&path)?;
                let before = file.metadata()?;
                let mut hash = blake3::Hasher::new();
                let mut read = 0;
                loop {
                    if cancel.load(Ordering::Relaxed) {
                        return Ok(None);
                    }
                    let n = file.read(&mut buffer)?;
                    if n == 0 {
                        break;
                    }
                    read += n as u64;
                    hash.update(&buffer[..n]);
                }
                let after = file.metadata()?;
                if read != size
                    || before.modified().ok() != after.modified().ok()
                    || after.len() != size
                {
                    return Err(std::io::Error::other("File changed during duplicate scan"));
                }
                Ok(Some(*hash.finalize().as_bytes()))
            })();
            match hash {
                Ok(Some(hash)) => by_hash.entry(hash).or_default().push(path),
                Ok(None) => return report,
                Err(e) => report.failed.push((path, e.to_string())),
            }
        }
        report
            .groups
            .extend(by_hash.into_values().filter(|group| group.len() > 1));
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_identical_content_is_grouped_and_repeated_paths_are_ignored() {
        let dir = crate::sidecar::tests::temp_dir("duplicates");
        for (name, bytes) in [("a.nef", b"abc"), ("b.nef", b"abc"), ("c.nef", b"xyz")] {
            std::fs::write(dir.join(name), bytes).unwrap();
        }
        let a = dir.join("a.nef");
        let b = dir.join("b.nef");
        let report = find(
            vec![
                a.clone(),
                b.clone(),
                a.clone(),
                dir.join("c.nef"),
                dir.join("missing.nef"),
            ],
            &AtomicBool::new(false),
        );
        assert_eq!(report.groups, [vec![a, b]]);
        assert_eq!(report.failed.len(), 1);
        assert!(
            find(
                vec![dir.join("a.nef"), dir.join("b.nef")],
                &AtomicBool::new(true)
            )
            .groups
            .is_empty()
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}
