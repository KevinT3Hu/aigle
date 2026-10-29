//! File decoding in worker processes ([`aigle_isolate`]): LibRaw, librtprocess and libvips
//! run there, so a file that crashes or hangs them fails alone.

use std::path::{Path, PathBuf};

use aigle_engine_api::CancelToken;
use aigle_isolate::{Call, Error, Event, Failure, Handler, Reply, Task};
use serde::{Deserialize, Serialize};

use super::{ATier, FileDecoded, FileHeader, LoadError};

const DECODE: &str = "engine.decode";
const PREVIEW: &str = "engine.preview";

/// The engine's worker operations, for [`aigle_isolate::serve`].
pub const OPS: &[(&str, Handler)] = &[(DECODE, serve_decode), (PREVIEW, serve_preview)];

#[derive(Serialize, Deserialize)]
struct DecodeRequest {
    path: PathBuf,
    raw: bool,
    a: ATier,
}

fn load_error(path: &Path, e: Error) -> LoadError {
    match e {
        Error::Cancelled => LoadError::Cancelled,
        Error::Failed(m) => LoadError::Failed(m),
        Error::Crashed(m) => {
            tracing::warn!(path = %path.display(), "decoding crashed: {m}");
            LoadError::Failed(format!(
                "{}: the decoder crashed on this file ({m})",
                path.display()
            ))
        }
    }
}

pub(super) fn decode(
    path: &Path,
    raw: bool,
    a: &ATier,
    admit: &mut dyn FnMut(u64) -> Result<(), LoadError>,
    cancel: Option<&CancelToken>,
) -> Result<FileDecoded, LoadError> {
    let req = DecodeRequest {
        path: path.to_path_buf(),
        raw,
        a: *a,
    };
    let err = |e| load_error(path, e);
    let mut call = Call::start(DECODE, &req, cancel).map_err(err)?;
    let header: FileHeader = loop {
        match call.event().map_err(err)? {
            Event::Admit(peak) => {
                let r = admit(peak);
                call.admitted(r.is_ok()).map_err(err)?;
                if let Err(e) = r {
                    // Collect the worker's CANCELLED so it goes back to the pool.
                    let _ = call.event::<FileHeader>();
                    return Err(e);
                }
            }
            Event::Done(h) => break h,
        }
    };
    let rgb = call.blob().map_err(err)?;
    let mask = if header.has_mask {
        Some(call.blob().map_err(err)?)
    } else {
        None
    };
    let n = header.size.area() as usize;
    if rgb.len() != n || mask.as_ref().is_some_and(|m: &Vec<u8>| m.len() != n) {
        return Err(LoadError::Failed(format!(
            "{}: the decoder returned inconsistent data",
            path.display()
        )));
    }
    Ok(FileDecoded { header, rgb, mask })
}

fn failure(e: LoadError) -> Failure {
    match e {
        LoadError::Cancelled => Failure::Cancelled,
        LoadError::Failed(m) => Failure::Failed(m),
    }
}

fn request<T: for<'de> Deserialize<'de>>(body: &[u8]) -> Result<T, Failure> {
    serde_json::from_slice(body).map_err(|e| Failure::Failed(format!("bad request: {e}")))
}

fn serve_decode(body: &[u8], task: &mut Task) -> Result<Reply, Failure> {
    let req: DecodeRequest = request(body)?;
    let cancel = task.cancel_token().clone();
    let mut admit = |peak: u64| {
        task.admit(peak).map_err(|f| match f {
            Failure::Cancelled => LoadError::Cancelled,
            Failure::Failed(m) => LoadError::Failed(m),
        })
    };
    let d = super::decode_file_here(&req.path, req.raw, &req.a, &mut admit, Some(&cancel))
        .map_err(failure)?;
    let reply = Reply::new(&d.header)?.blob(d.rgb);
    Ok(match d.mask {
        Some(m) => reply.blob(m),
        None => reply,
    })
}

pub(super) fn preview(path: &Path) -> Result<(u32, u32, Vec<u8>), String> {
    let run = || -> Result<(u32, u32, Vec<u8>), Error> {
        let mut call = Call::start(PREVIEW, &path, None)?;
        let (w, h): (u32, u32) = loop {
            match call.event()? {
                Event::Admit(_) => call.admitted(true)?,
                Event::Done(size) => break size,
            }
        };
        let rgb: Vec<u8> = call.blob()?;
        if rgb.len() != w as usize * h as usize * 3 {
            return Err(Error::Failed("inconsistent preview".into()));
        }
        Ok((w, h, rgb))
    };
    run().map_err(|e| e.to_string())
}

fn serve_preview(body: &[u8], _: &mut Task) -> Result<Reply, Failure> {
    let path: PathBuf = request(body)?;
    let (w, h, rgb) = super::raw::embedded_preview(&path)?;
    Ok(Reply::new(&(w, h))?.blob(rgb))
}
