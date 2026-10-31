//! Disposable disk cache of rendered thumbnails and previews (doc 03 §7, doc 01 §6).
//!
//! Entries live in `<cache>/thumbs/` (long edge ≤ 256) or `<cache>/previews/`, named by the
//! hex cache key. Each file is a 32-byte BLAKE3 hash of the payload followed by the payload,
//! written atomically; a corrupt or truncated entry fails the hash check and is recomputed
//! (doc 03 §10). The total is kept below min(20 GiB, 10 % of the free space); when it exceeds
//! that by 10 %, the least recently used entries (file mtime, refreshed on hits) are removed
//! (doc 03 §7.4).

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use parking_lot::Mutex;

use crate::cache::CacheKey;

const GIB: u64 = 1 << 30;

pub struct DiskCache {
    dir: PathBuf,
    /// Bytes held, scanned on first use.
    used: Mutex<Option<u64>>,
}

/// Cache areas.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Area {
    Thumbs,
    Previews,
}

impl Area {
    fn dir(self) -> &'static str {
        match self {
            Area::Thumbs => "thumbs",
            Area::Previews => "previews",
        }
    }

    pub fn for_size(max_px: u32) -> Area {
        if max_px <= 256 {
            Area::Thumbs
        } else {
            Area::Previews
        }
    }
}

const AREAS: [Area; 2] = [Area::Thumbs, Area::Previews];

/// Free bytes on the filesystem of `path`.
#[allow(unsafe_code)]
fn free_space(path: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    // SAFETY: `statvfs` only writes the struct it is given; zeroed is a valid value for it.
    let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: valid NUL-terminated path and out-pointer.
    let rc = unsafe { libc::statvfs(c.as_ptr(), &mut s) };
    (rc == 0).then(|| s.f_bavail as u64 * s.f_frsize as u64)
}

impl DiskCache {
    pub fn new(dir: PathBuf) -> DiskCache {
        DiskCache {
            dir,
            used: Mutex::new(None),
        }
    }

    fn path(&self, area: Area, key: &CacheKey) -> PathBuf {
        self.dir.join(area.dir()).join(key.hex())
    }

    /// The payload of `key`, if present and intact.
    pub fn get(&self, area: Area, key: &CacheKey) -> Option<Vec<u8>> {
        let path = self.path(area, key);
        let bytes = std::fs::read(&path).ok()?;
        if bytes.len() < 32 || blake3::hash(&bytes[32..]).as_bytes() != &bytes[..32] {
            tracing::warn!(path = %path.display(), "corrupt disk-cache entry removed");
            let _ = std::fs::remove_file(&path);
            return None;
        }
        // Recency for eviction.
        if let Ok(f) = std::fs::File::options().write(true).open(&path) {
            let _ = f.set_modified(SystemTime::now());
        }
        Some(bytes[32..].to_vec())
    }

    pub fn put(&self, area: Area, key: &CacheKey, payload: &[u8]) {
        let path = self.path(area, key);
        let mut bytes = Vec::with_capacity(32 + payload.len());
        bytes.extend_from_slice(blake3::hash(payload).as_bytes());
        bytes.extend_from_slice(payload);
        let res = std::fs::create_dir_all(path.parent().expect("in an area"))
            .map_err(|e| e.to_string())
            .and_then(|()| crate::encode::write_atomic(&path, &bytes));
        if let Err(e) = res {
            tracing::debug!("disk cache write failed: {e}");
            return;
        }
        let mut used = self.used.lock();
        let total = used.get_or_insert_with(|| self.scan().iter().map(|e| e.1).sum());
        *total += bytes.len() as u64;
        let limit = self.limit();
        if *total > limit + limit / 10 {
            *total = self.evict(limit);
        }
    }

    /// min(20 GiB, 10 % of the free space).
    fn limit(&self) -> u64 {
        let free = free_space(&self.dir).unwrap_or(200 * GIB);
        (20 * GIB).min(free / 10)
    }

    fn scan(&self) -> Vec<(PathBuf, u64, SystemTime)> {
        let mut v = Vec::new();
        for a in AREAS {
            let Ok(rd) = std::fs::read_dir(self.dir.join(a.dir())) else {
                continue;
            };
            for e in rd.flatten() {
                if let Ok(m) = e.metadata()
                    && m.is_file()
                {
                    v.push((
                        e.path(),
                        m.len(),
                        m.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                    ));
                }
            }
        }
        v
    }

    /// Remove least recently used entries until at most `limit` bytes remain; returns the
    /// bytes left.
    fn evict(&self, limit: u64) -> u64 {
        let mut v = self.scan();
        v.sort_by_key(|e| e.2);
        let mut total: u64 = v.iter().map(|e| e.1).sum();
        for (p, n, _) in v {
            if total <= limit {
                break;
            }
            if std::fs::remove_file(&p).is_ok() {
                total -= n;
            }
        }
        total
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::KeyBuilder;

    #[test]
    fn round_trip_corruption_and_eviction() {
        let dir = std::env::temp_dir().join(format!("aigle-disk-{}", std::process::id()));
        let c = DiskCache::new(dir.clone());
        let k = |i: u32| KeyBuilder::new("t").u32(i).finish();
        c.put(Area::Thumbs, &k(1), b"hello");
        assert_eq!(c.get(Area::Thumbs, &k(1)).as_deref(), Some(&b"hello"[..]));
        assert!(c.get(Area::Previews, &k(1)).is_none());
        // Corruption is detected and the entry dropped.
        let p = c.path(Area::Thumbs, &k(1));
        let mut b = std::fs::read(&p).unwrap();
        *b.last_mut().unwrap() ^= 1;
        std::fs::write(&p, b).unwrap();
        assert!(c.get(Area::Thumbs, &k(1)).is_none());
        assert!(!p.exists());
        // LRU eviction keeps the most recent entries.
        for i in 0..10 {
            c.put(Area::Previews, &k(10 + i), &[0u8; 100]);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let _ = c.get(Area::Previews, &k(10));
        assert_eq!(c.evict(4 * 132), 4 * 132);
        assert!(c.get(Area::Previews, &k(10)).is_some(), "recently used");
        assert!(c.get(Area::Previews, &k(11)).is_none(), "oldest evicted");
        assert!(c.get(Area::Previews, &k(19)).is_some());
        std::fs::remove_dir_all(&dir).ok();
    }
}
