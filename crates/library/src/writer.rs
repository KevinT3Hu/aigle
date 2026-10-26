//! Debounced sidecar writes on a background thread (doc 01 §9.3).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::sidecar::{Error, PhotoMeta, Store};

/// Outcome of one write: the photo and the sidecar written (`None`: nothing to store).
pub type WriteReport = (PathBuf, Result<Option<PathBuf>, Error>);

enum Msg {
    Schedule {
        photo: PathBuf,
        meta: Box<PhotoMeta>,
        delay: Duration,
    },
    Flush(mpsc::Sender<()>),
}

/// Writes the latest scheduled [`PhotoMeta`] of each photo once no newer one has arrived for
/// the debounce delay. Dropping the writer flushes.
pub struct Writer {
    tx: Option<mpsc::Sender<Msg>>,
    thread: Option<JoinHandle<()>>,
    delay: Duration,
}

impl Writer {
    /// `report` is called on the writer thread after every write.
    pub fn new(
        store: Arc<Store>,
        delay: Duration,
        report: impl Fn(WriteReport) + Send + 'static,
    ) -> Writer {
        let (tx, rx) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("aigle-sidecar".into())
            .spawn(move || run(&store, &rx, &report))
            .expect("spawn sidecar writer");
        Writer {
            tx: Some(tx),
            thread: Some(thread),
            delay,
        }
    }

    /// Write `meta` after the debounce delay, replacing any pending write of `photo`.
    pub fn schedule(&self, photo: PathBuf, meta: PhotoMeta) {
        self.send(photo, meta, self.delay);
    }

    /// Write `meta` without waiting (closing a photo).
    pub fn write_now(&self, photo: PathBuf, meta: PhotoMeta) {
        self.send(photo, meta, Duration::ZERO);
    }

    fn send(&self, photo: PathBuf, meta: PhotoMeta, delay: Duration) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(Msg::Schedule {
                photo,
                meta: Box::new(meta),
                delay,
            });
        }
    }

    /// Write everything pending now and wait for it.
    pub fn flush(&self) {
        let (done, wait) = mpsc::channel();
        if let Some(tx) = &self.tx
            && tx.send(Msg::Flush(done)).is_ok()
        {
            let _ = wait.recv();
        }
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        drop(self.tx.take());
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn run(store: &Store, rx: &mpsc::Receiver<Msg>, report: &dyn Fn(WriteReport)) {
    let mut pending: HashMap<PathBuf, (PhotoMeta, Instant)> = HashMap::new();
    let write = |photo: PathBuf, meta: PhotoMeta| {
        let r = store.write(&photo, &meta);
        if let Err(e) = &r {
            tracing::warn!("sidecar not written: {e}");
        }
        report((photo, r));
    };
    loop {
        let next = pending.values().map(|(_, due)| *due).min();
        let msg = match next {
            Some(due) => rx.recv_timeout(due.saturating_duration_since(Instant::now())),
            None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
        };
        match msg {
            Ok(Msg::Schedule { photo, meta, delay }) => {
                pending.insert(photo, (*meta, Instant::now() + delay));
            }
            Ok(Msg::Flush(done)) => {
                for (photo, (meta, _)) in pending.drain() {
                    write(photo, meta);
                }
                let _ = done.send(());
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                for (photo, (meta, _)) in pending.drain() {
                    write(photo, meta);
                }
                return;
            }
        }
        let now = Instant::now();
        let due: Vec<PathBuf> = pending
            .iter()
            .filter(|(_, (_, at))| *at <= now)
            .map(|(p, _)| p.clone())
            .collect();
        for photo in due {
            let (meta, _) = pending.remove(&photo).expect("listed above");
            write(photo, meta);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn coalesces_debounces_and_flushes() {
        let dir = crate::sidecar::tests::temp_dir("writer");
        let photo = dir.join("d.nef");
        std::fs::write(&photo, b"raw").unwrap();
        let store = Arc::new(Store::new());
        let reports = Arc::new(Mutex::new(Vec::new()));
        let sink = reports.clone();
        let writer = Writer::new(store.clone(), Duration::from_millis(200), move |r| {
            sink.lock().unwrap().push(r)
        });
        let rated = |r| PhotoMeta {
            rating: r,
            ..PhotoMeta::default()
        };
        for r in 1..=4 {
            writer.schedule(photo.clone(), rated(r));
        }
        std::thread::sleep(Duration::from_millis(50));
        assert!(reports.lock().unwrap().is_empty(), "debounced");
        let deadline = Instant::now() + Duration::from_secs(5);
        while reports.lock().unwrap().is_empty() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(reports.lock().unwrap().len(), 1, "coalesced");
        assert_eq!(Store::new().read(&photo).unwrap().meta.rating, 4);

        writer.schedule(photo.clone(), rated(2));
        writer.flush();
        assert_eq!(reports.lock().unwrap().len(), 2);
        assert_eq!(Store::new().read(&photo).unwrap().meta.rating, 2);

        writer.schedule(photo.clone(), rated(5));
        drop(writer);
        assert_eq!(Store::new().read(&photo).unwrap().meta.rating, 5);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
