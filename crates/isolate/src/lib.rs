//! Untrusted-file parsers in worker processes.
//!
//! LibRaw, libvips and Exiv2 parse photo files in C and C++. A crash, abort or endless loop
//! in one of them on a bad file must fail that file, not take the app down. Callers send
//! such work to a worker process with [`Call`] (or [`call`]): the app's own executable,
//! started with [`WORKER_ARG`], whose `main` hands over to [`serve`]. A worker that dies or
//! stops responding is replaced and the request fails with [`Error::Crashed`].
//!
//! Isolation is off until [`enable`] is called, so tests, examples and the CLI parse
//! in-process; callers check [`enabled`] and otherwise run the parser directly.
//!
//! Each worker runs one request at a time. Frames are `tag u8, length u32, payload`
//! (little-endian):
//! * parent → worker: `REQUEST seq op body`, `ADMITTED seq` / `REFUSED seq` (the answer to
//!   `ADMIT`), `CANCEL seq`;
//! * worker → parent: `ADMIT peak` (the request needs `peak` bytes; wait for the answer),
//!   then `DONE header blob-lengths` followed by the blob bytes, `FAILED message` or
//!   `CANCELLED`.

use std::any::Any;
use std::collections::VecDeque;
use std::ffi::OsString;
use std::io::{self, Read, Write};
use std::os::unix::process::ExitStatusExt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, mpsc};
use std::time::{Duration, Instant};

use aigle_engine_api::CancelToken;
use bytemuck::Pod;
use rustix::event::{PollFd, PollFlags, Timespec};
use serde::Serialize;
use serde::de::DeserializeOwned;

/// First argument of a worker process.
pub const WORKER_ARG: &str = "--aigle-worker";

const REQUEST: u8 = 1;
const ADMITTED: u8 = 2;
const REFUSED: u8 = 3;
const CANCEL: u8 = 4;
const ADMIT: u8 = 10;
const DONE: u8 = 11;
const FAILED: u8 = 12;
const CANCELLED: u8 = 13;

/// How long a worker may stay silent before it counts as hung.
const SILENCE: Duration = Duration::from_secs(300);
/// How long a cancelled request may take to stop before its worker is killed.
const GRACE: Duration = Duration::from_secs(2);
const TICK: Duration = Duration::from_millis(25);
const MAX_FRAME: u32 = 64 << 20;
const MAX_BLOB: u64 = 1 << 37;
const MAX_IDLE: usize = 4;

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("cancelled")]
    Cancelled,
    /// The operation reported an error.
    #[error("{0}")]
    Failed(String),
    /// The worker died or stopped responding; the input most likely crashes its parser.
    #[error("{0}")]
    Crashed(String),
}

struct Spawn {
    exe: PathBuf,
    env: Vec<(OsString, OsString)>,
}

static EXE: OnceLock<Spawn> = OnceLock::new();
static SEQ: AtomicU64 = AtomicU64::new(1);

/// Run calls in worker processes started from `exe` (`/proc/self/exe` for the app, which
/// keeps working when the binary is replaced while the app runs) with `env` added to their
/// environment. `AIGLE_ISOLATE=0` keeps parsing in-process. Returns whether isolation is on.
pub fn enable(exe: PathBuf, env: Vec<(OsString, OsString)>) -> bool {
    if std::env::var_os("AIGLE_ISOLATE").is_some_and(|v| v == "0") {
        tracing::warn!("AIGLE_ISOLATE=0: parsing photo files in-process");
        return false;
    }
    let _ = EXE.set(Spawn { exe, env });
    true
}

pub fn enabled() -> bool {
    EXE.get().is_some()
}

/// Whether this process was started as a worker.
pub fn is_worker() -> bool {
    std::env::args_os().nth(1).is_some_and(|a| a == WORKER_ARG)
}

/// The text of a panic payload.
pub fn panic_message(p: &(dyn Any + Send)) -> String {
    if let Some(s) = p.downcast_ref::<&str>() {
        s.to_string()
    } else if let Some(s) = p.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic".into()
    }
}

// ---------------------------------------------------------------------------------------
// Frames

fn write_frame(w: &mut impl Write, tag: u8, payload: &[u8]) -> io::Result<()> {
    let mut buf = Vec::with_capacity(5 + payload.len());
    buf.push(tag);
    buf.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    buf.extend_from_slice(payload);
    w.write_all(&buf)?;
    w.flush()
}

fn read_frame(r: &mut impl Read) -> io::Result<(u8, Vec<u8>)> {
    let mut head = [0u8; 5];
    r.read_exact(&mut head)?;
    let len = u32::from_le_bytes(head[1..5].try_into().expect("4 bytes"));
    if len > MAX_FRAME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "frame too large",
        ));
    }
    let mut payload = vec![0u8; len as usize];
    r.read_exact(&mut payload)?;
    Ok((head[0], payload))
}

/// Reads little-endian fields off the front of a payload.
struct Fields<'a>(&'a [u8]);

impl<'a> Fields<'a> {
    fn take(&mut self, n: usize) -> io::Result<&'a [u8]> {
        if self.0.len() < n {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "short frame"));
        }
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Ok(a)
    }
    fn u16(&mut self) -> io::Result<u16> {
        Ok(u16::from_le_bytes(
            self.take(2)?.try_into().expect("2 bytes"),
        ))
    }
    fn u32(&mut self) -> io::Result<u32> {
        Ok(u32::from_le_bytes(
            self.take(4)?.try_into().expect("4 bytes"),
        ))
    }
    fn u64(&mut self) -> io::Result<u64> {
        Ok(u64::from_le_bytes(
            self.take(8)?.try_into().expect("8 bytes"),
        ))
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

// ---------------------------------------------------------------------------------------
// Parent side

struct Worker {
    child: Child,
    stdin: ChildStdin,
    stdout: ChildStdout,
}

struct Pool {
    idle: Vec<Worker>,
    live: usize,
}

static POOL: Mutex<Pool> = Mutex::new(Pool {
    idle: Vec::new(),
    live: 0,
});
static FREED: Condvar = Condvar::new();

fn max_live() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .clamp(2, 8)
        + 2
}

fn spawn() -> io::Result<Worker> {
    let spawn = EXE
        .get()
        .ok_or_else(|| io::Error::other("worker processes are not enabled"))?;
    let mut child = Command::new(&spawn.exe)
        .arg(WORKER_ARG)
        .envs(spawn.env.iter().map(|(k, v)| (k, v)))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()?;
    let stdin = child.stdin.take().expect("piped");
    let stdout = child.stdout.take().expect("piped");
    // Decoded images are hundreds of MB; a larger pipe means fewer context switches.
    let _ = rustix::pipe::fcntl_setpipe_size(&stdout, 1 << 20);
    tracing::debug!(pid = child.id(), "worker process started");
    Ok(Worker {
        child,
        stdin,
        stdout,
    })
}

fn is_cancelled(c: Option<&CancelToken>) -> bool {
    c.is_some_and(|c| c.is_cancelled())
}

/// An idle worker, or a new one while fewer than [`max_live`] run.
fn acquire(cancel: Option<&CancelToken>) -> Result<Worker, Error> {
    let mut p = lock(&POOL);
    loop {
        while let Some(mut w) = p.idle.pop() {
            if matches!(w.child.try_wait(), Ok(None)) {
                return Ok(w);
            }
            p.live -= 1;
        }
        if p.live < max_live() {
            p.live += 1;
            drop(p);
            return spawn().map_err(|e| {
                lock(&POOL).live -= 1;
                FREED.notify_one();
                Error::Failed(format!("cannot start a worker process: {e}"))
            });
        }
        if is_cancelled(cancel) {
            return Err(Error::Cancelled);
        }
        p = FREED
            .wait_timeout(p, TICK)
            .unwrap_or_else(|e| e.into_inner())
            .0;
    }
}

fn release(w: Worker) {
    let mut p = lock(&POOL);
    if p.idle.len() < MAX_IDLE {
        p.idle.push(w);
    } else {
        p.live -= 1;
        drop(p);
        let _ = end(w);
    }
    FREED.notify_one();
}

/// Kill `w` (if still running) and reap it.
fn end(mut w: Worker) -> io::Result<ExitStatus> {
    drop(w.stdin);
    let _ = w.child.kill();
    w.child.wait()
}

fn discard(w: Worker) -> io::Result<ExitStatus> {
    let status = end(w);
    lock(&POOL).live -= 1;
    FREED.notify_one();
    status
}

fn signal_name(sig: i32) -> &'static str {
    match sig {
        4 => "SIGILL",
        5 => "SIGTRAP",
        6 => "SIGABRT",
        7 => "SIGBUS",
        8 => "SIGFPE",
        9 => "SIGKILL, often the out-of-memory killer",
        11 => "SIGSEGV",
        15 => "SIGTERM",
        _ => "signal",
    }
}

fn describe(status: io::Result<ExitStatus>) -> String {
    match status {
        Ok(s) => match s.signal() {
            Some(sig) => format!("the worker process crashed ({}, {sig})", signal_name(sig)),
            None => format!("the worker process quit ({s})"),
        },
        Err(e) => format!("the worker process was lost: {e}"),
    }
}

/// What a worker sent before its result.
pub enum Event<T> {
    /// The request needs this many bytes; answer with [`Call::admitted`].
    Admit(u64),
    /// The result header; its blobs follow ([`Call::blob`]).
    Done(T),
}

/// One request to a worker. Dropping an unfinished call kills its worker.
pub struct Call {
    worker: Option<Worker>,
    seq: u64,
    op: &'static str,
    cancel: Option<CancelToken>,
    cancel_sent: Option<Instant>,
    blobs: VecDeque<u64>,
}

impl Call {
    pub fn start(
        op: &'static str,
        request: &impl Serialize,
        cancel: Option<&CancelToken>,
    ) -> Result<Call, Error> {
        let body = serde_json::to_vec(request)
            .map_err(|e| Error::Failed(format!("{op}: cannot encode the request: {e}")))?;
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let mut payload = Vec::with_capacity(10 + op.len() + body.len());
        payload.extend_from_slice(&seq.to_le_bytes());
        payload.extend_from_slice(&(op.len() as u16).to_le_bytes());
        payload.extend_from_slice(op.as_bytes());
        payload.extend_from_slice(&body);
        // An idle worker may have been killed from outside; retry once on a fresh one.
        let mut tries = 0;
        loop {
            let mut w = acquire(cancel)?;
            match write_frame(&mut w.stdin, REQUEST, &payload) {
                Ok(()) => {
                    return Ok(Call {
                        worker: Some(w),
                        seq,
                        op,
                        cancel: cancel.cloned(),
                        cancel_sent: None,
                        blobs: VecDeque::new(),
                    });
                }
                Err(e) => {
                    let _ = discard(w);
                    tries += 1;
                    if tries == 2 {
                        return Err(Error::Failed(format!("{op}: worker unavailable: {e}")));
                    }
                }
            }
        }
    }

    fn worker(&mut self) -> &mut Worker {
        self.worker.as_mut().expect("call already finished")
    }

    fn crashed(&mut self, why: Option<&str>) -> Error {
        let status = self.worker.take().map(discard);
        let msg = match (why, status) {
            (Some(why), _) => why.to_string(),
            (None, Some(s)) => describe(s),
            (None, None) => "the worker process was lost".into(),
        };
        tracing::warn!(op = self.op, "{msg}");
        Error::Crashed(msg)
    }

    fn done(&mut self) {
        if let Some(w) = self.worker.take() {
            release(w);
        }
    }

    /// Wait for the next frame, forwarding cancellation; a worker that ignores it for
    /// [`GRACE`] or is silent for [`SILENCE`] is killed.
    fn read(&mut self) -> Result<(u8, Vec<u8>), Error> {
        let since = Instant::now();
        loop {
            let ready = {
                let w = self.worker();
                let mut fds = [PollFd::new(&w.stdout, PollFlags::IN)];
                let t = Timespec {
                    tv_sec: 0,
                    tv_nsec: TICK.as_nanos() as _,
                };
                match rustix::event::poll(&mut fds, Some(&t)) {
                    Ok(n) => n > 0,
                    Err(rustix::io::Errno::INTR) => false,
                    Err(e) => return Err(self.crashed(Some(&format!("poll: {e}")))),
                }
            };
            if ready {
                break;
            }
            if is_cancelled(self.cancel.as_ref()) {
                match self.cancel_sent {
                    None => {
                        let seq = self.seq.to_le_bytes();
                        let _ = write_frame(&mut self.worker().stdin, CANCEL, &seq);
                        self.cancel_sent = Some(Instant::now());
                    }
                    Some(at) if at.elapsed() > GRACE => {
                        if let Some(w) = self.worker.take() {
                            let _ = discard(w);
                        }
                        return Err(Error::Cancelled);
                    }
                    Some(_) => {}
                }
            }
            if since.elapsed() > SILENCE {
                let msg = format!(
                    "the worker process stopped responding for {} s",
                    SILENCE.as_secs()
                );
                return Err(self.crashed(Some(&msg)));
            }
        }
        read_frame(&mut self.worker().stdout).map_err(|_| self.crashed(None))
    }

    /// The next event; errors end the call.
    pub fn event<T: DeserializeOwned>(&mut self) -> Result<Event<T>, Error> {
        let (tag, payload) = self.read()?;
        let mut f = Fields(&payload);
        match tag {
            ADMIT => match f.u64() {
                Ok(peak) => Ok(Event::Admit(peak)),
                Err(_) => Err(self.crashed(Some("protocol error"))),
            },
            DONE => {
                let parsed = (|| -> io::Result<(T, VecDeque<u64>)> {
                    let n = f.u32()? as usize;
                    let header = serde_json::from_slice(f.take(n)?)?;
                    let count = f.u32()?;
                    let blobs = (0..count).map(|_| f.u64()).collect::<io::Result<_>>()?;
                    Ok((header, blobs))
                })();
                match parsed {
                    Ok((header, blobs)) => {
                        self.blobs = blobs;
                        if self.blobs.is_empty() {
                            self.done();
                        }
                        Ok(Event::Done(header))
                    }
                    Err(e) => Err(self.crashed(Some(&format!("{}: bad reply: {e}", self.op)))),
                }
            }
            FAILED => {
                self.done();
                Err(Error::Failed(
                    String::from_utf8_lossy(&payload).into_owned(),
                ))
            }
            CANCELLED => {
                self.done();
                Err(Error::Cancelled)
            }
            _ => Err(self.crashed(Some("protocol error"))),
        }
    }

    /// Answer an [`Event::Admit`]: `false` cancels the request.
    pub fn admitted(&mut self, ok: bool) -> Result<(), Error> {
        let seq = self.seq.to_le_bytes();
        let tag = if ok { ADMITTED } else { REFUSED };
        write_frame(&mut self.worker().stdin, tag, &seq).map_err(|_| self.crashed(None))
    }

    /// Number of blobs still to read.
    pub fn blobs_left(&self) -> usize {
        self.blobs.len()
    }

    /// The next blob of the result as `T`s.
    pub fn blob<T: Pod>(&mut self) -> Result<Vec<T>, Error> {
        let Some(len) = self.blobs.pop_front() else {
            return Err(Error::Failed(format!("{}: missing result data", self.op)));
        };
        let size = size_of::<T>() as u64;
        if len % size != 0 || len > MAX_BLOB {
            return Err(self.crashed(Some(&format!("{}: bad result data", self.op))));
        }
        let mut v = vec![T::zeroed(); (len / size) as usize];
        if self
            .worker()
            .stdout
            .read_exact(bytemuck::cast_slice_mut(&mut v))
            .is_err()
        {
            return Err(self.crashed(None));
        }
        if self.blobs.is_empty() {
            self.done();
        }
        Ok(v)
    }
}

impl Drop for Call {
    fn drop(&mut self) {
        if let Some(w) = self.worker.take() {
            let _ = discard(w);
        }
    }
}

/// A request without blobs whose memory needs are admitted unconditionally.
pub fn call<T: DeserializeOwned>(
    op: &'static str,
    request: &impl Serialize,
    cancel: Option<&CancelToken>,
) -> Result<T, Error> {
    let mut c = Call::start(op, request, cancel)?;
    loop {
        match c.event()? {
            Event::Admit(_) => c.admitted(true)?,
            Event::Done(r) => return Ok(r),
        }
    }
}

// ---------------------------------------------------------------------------------------
// Worker side

/// An operation of [`serve`]: the request body (JSON) and the task.
pub type Handler = fn(&[u8], &mut Task) -> Result<Reply, Failure>;

#[derive(Debug)]
pub enum Failure {
    Cancelled,
    Failed(String),
}

impl From<String> for Failure {
    fn from(s: String) -> Self {
        Failure::Failed(s)
    }
}

trait Blob {
    fn bytes(&self) -> &[u8];
}

impl<T: Pod> Blob for Vec<T> {
    fn bytes(&self) -> &[u8] {
        bytemuck::cast_slice(self)
    }
}

/// A result: a JSON header and raw data blobs.
pub struct Reply {
    header: Vec<u8>,
    blobs: Vec<Box<dyn Blob>>,
}

impl Reply {
    pub fn new(header: &impl Serialize) -> Result<Reply, Failure> {
        Ok(Reply {
            header: serde_json::to_vec(header)
                .map_err(|e| Failure::Failed(format!("cannot encode the reply: {e}")))?,
            blobs: Vec::new(),
        })
    }

    pub fn blob<T: Pod>(mut self, data: Vec<T>) -> Reply {
        self.blobs.push(Box::new(data));
        self
    }
}

/// The request being served.
pub struct Task<'a> {
    seq: u64,
    cancel: CancelToken,
    out: &'a mut std::fs::File,
    control: &'a mpsc::Receiver<(u64, bool)>,
}

impl Task<'_> {
    /// Set when the parent cancels the request.
    pub fn cancel_token(&self) -> &CancelToken {
        &self.cancel
    }

    /// Wait until the parent admits `peak` bytes of memory for this request.
    pub fn admit(&mut self, peak: u64) -> Result<(), Failure> {
        if write_frame(self.out, ADMIT, &peak.to_le_bytes()).is_err() {
            gone();
        }
        loop {
            match self.control.recv() {
                Ok((seq, ok)) if seq == self.seq => {
                    return if ok { Ok(()) } else { Err(Failure::Cancelled) };
                }
                Ok(_) => {}
                Err(_) => gone(),
            }
        }
    }
}

/// The parent closed the pipes: it quit or crashed.
fn gone() -> ! {
    std::process::exit(0)
}

fn reply(out: &mut std::fs::File, r: Result<Reply, Failure>) -> io::Result<()> {
    match r {
        Ok(r) => {
            let mut p = Vec::with_capacity(8 + r.header.len() + 8 * r.blobs.len());
            p.extend_from_slice(&(r.header.len() as u32).to_le_bytes());
            p.extend_from_slice(&r.header);
            p.extend_from_slice(&(r.blobs.len() as u32).to_le_bytes());
            for b in &r.blobs {
                p.extend_from_slice(&(b.bytes().len() as u64).to_le_bytes());
            }
            write_frame(out, DONE, &p)?;
            for b in &r.blobs {
                out.write_all(b.bytes())?;
            }
            out.flush()
        }
        Err(Failure::Failed(m)) => write_frame(out, FAILED, m.as_bytes()),
        Err(Failure::Cancelled) => write_frame(out, CANCELLED, &[]),
    }
}

/// Serve requests on stdin/stdout until the parent goes away. Stdout is reserved for the
/// protocol: anything the libraries print goes to stderr.
pub fn serve(handlers: &[(&str, Handler)]) -> ! {
    let proto = rustix::io::dup(io::stdout()).and_then(|fd| {
        rustix::stdio::dup2_stdout(io::stderr())?;
        Ok(fd)
    });
    let mut out = match proto {
        Ok(fd) => std::fs::File::from(fd),
        Err(e) => {
            eprintln!("aigle worker: cannot set up stdout: {e}");
            std::process::exit(2);
        }
    };
    let current = Arc::new(Mutex::new((0u64, CancelToken::new())));
    let (req_tx, req_rx) = mpsc::channel::<(u64, String, Vec<u8>)>();
    let (ctl_tx, ctl_rx) = mpsc::channel::<(u64, bool)>();
    let reader_current = current.clone();
    std::thread::Builder::new()
        .name("worker-control".into())
        .spawn(move || {
            let mut stdin = io::stdin().lock();
            loop {
                let Ok((tag, payload)) = read_frame(&mut stdin) else {
                    gone();
                };
                let mut f = Fields(&payload);
                let Ok(seq) = f.u64() else { gone() };
                match tag {
                    REQUEST => {
                        let Ok(n) = f.u16() else { gone() };
                        let Ok(op) = f.take(n as usize) else { gone() };
                        let op = String::from_utf8_lossy(op).into_owned();
                        if req_tx.send((seq, op, f.0.to_vec())).is_err() {
                            gone();
                        }
                    }
                    ADMITTED | REFUSED => {
                        let _ = ctl_tx.send((seq, tag == ADMITTED));
                    }
                    CANCEL => {
                        let c = lock(&reader_current);
                        if c.0 == seq {
                            c.1.cancel();
                        }
                    }
                    _ => {}
                }
            }
        })
        .unwrap_or_else(|e| {
            eprintln!("aigle worker: {e}");
            std::process::exit(2)
        });

    for (seq, op, body) in req_rx {
        let cancel = CancelToken::new();
        *lock(&current) = (seq, cancel.clone());
        let mut task = Task {
            seq,
            cancel,
            out: &mut out,
            control: &ctl_rx,
        };
        let result = match handlers.iter().find(|(name, _)| *name == op) {
            Some((_, h)) => {
                catch_unwind(AssertUnwindSafe(|| h(&body, &mut task))).unwrap_or_else(|p| {
                    Err(Failure::Failed(format!(
                        "internal error: {}",
                        panic_message(p.as_ref())
                    )))
                })
            }
            None => Err(Failure::Failed(format!("unknown worker operation {op:?}"))),
        };
        if reply(&mut out, result).is_err() {
            gone();
        }
    }
    gone()
}
