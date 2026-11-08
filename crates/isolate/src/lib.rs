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
//!   then `DONE header blob-lengths`, `FAILED message` or `CANCELLED`. Ordinary
//!   blobs follow as bytes. A high length bit marks an immutable shared payload:
//!   one marker byte carries a sealed memfd using SCM_RIGHTS on the output socket.
//!   The stdin control channel remains a pipe.

#[cfg(any(unix, windows))]
mod unix;
#[cfg(any(unix, windows))]
pub use unix::*;

// Windows uses bounded anonymous pipes and owned pixel buffers; Linux can also
// transfer sealed shared buffers over its Unix socket.
#[cfg(not(any(unix, windows)))]
compile_error!("Parser isolation is not implemented on this platform");
