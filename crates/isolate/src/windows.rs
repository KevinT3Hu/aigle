//! Windows anonymous pipes. A bounded reader lets the parent poll cancellation
//! while a parser is silent, without an unbounded queue of image data.
use super::*;
use std::io::Cursor;
use std::os::windows::io::FromRawHandle;
use std::os::windows::process::CommandExt;

pub(super) struct PipeReader {
    rx: mpsc::Receiver<io::Result<Vec<u8>>>,
    pending: Cursor<Vec<u8>>,
    eof: bool,
}

impl PipeReader {
    pub(super) fn ready(&mut self, timeout: Duration) -> io::Result<bool> {
        if self.pending.position() < self.pending.get_ref().len() as u64 || self.eof {
            return Ok(true);
        }
        match self.rx.recv_timeout(timeout) {
            Ok(data) => {
                self.pending = Cursor::new(data?);
                Ok(true)
            }
            Err(mpsc::RecvTimeoutError::Timeout) => Ok(false),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                self.eof = true;
                Ok(true)
            }
        }
    }
}
impl Read for PipeReader {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if out.is_empty() {
            return Ok(0);
        }
        loop {
            let n = self.pending.read(out)?;
            if n != 0 || self.eof {
                return Ok(n);
            }
            self.ready(Duration::from_secs(1))?;
        }
    }
}

pub(super) fn spawn() -> io::Result<Worker> {
    let spawn = EXE
        .get()
        .ok_or_else(|| io::Error::other("workers are not enabled"))?;
    let mut child = Command::new(&spawn.exe)
        .arg(WORKER_ARG)
        .envs(spawn.env.iter().map(|(k, v)| (k, v)))
        .creation_flags(0x08000000) // CREATE_NO_WINDOW
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()?;
    let stdin = child.stdin.take().expect("piped");
    let mut output = child.stdout.take().expect("piped");
    let (tx, rx) = mpsc::sync_channel(1);
    std::thread::spawn(move || {
        loop {
            let mut bytes = vec![0; 64 * 1024];
            match output.read(&mut bytes) {
                Ok(0) => break,
                Ok(n) => {
                    bytes.truncate(n);
                    if tx.send(Ok(bytes)).is_err() {
                        break;
                    }
                }
                Err(e) => {
                    let _ = tx.send(Err(e));
                    break;
                }
            }
        }
    });
    Ok(Worker {
        child,
        stdin,
        stdout: PipeReader {
            rx,
            pending: Cursor::new(Vec::new()),
            eof: false,
        },
    })
}

#[allow(unsafe_code)]
pub(super) fn output() -> io::Result<std::fs::File> {
    unsafe extern "C" {
        fn _dup(fd: i32) -> i32;
        fn _dup2(src: i32, dst: i32) -> i32;
        fn _setmode(fd: i32, mode: i32) -> i32;
        fn _get_osfhandle(fd: i32) -> isize;
    }
    // SAFETY: called before parser threads start. Keep the duplicated CRT fd
    // open for the worker lifetime; File owns its handle until process exit.
    unsafe {
        let fd = _dup(1);
        if fd < 0 || _setmode(fd, 0x8000) < 0 || _setmode(0, 0x8000) < 0 || _dup2(2, 1) < 0 {
            return Err(io::Error::last_os_error());
        }
        let handle = _get_osfhandle(fd);
        if handle == -1 {
            return Err(io::Error::last_os_error());
        }
        Ok(std::fs::File::from_raw_handle(handle as *mut _))
    }
}
