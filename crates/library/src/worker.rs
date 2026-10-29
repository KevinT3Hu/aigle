//! Exiv2 reads of photo files in worker processes ([`aigle_isolate`]), so a file that
//! crashes Exiv2 loses its metadata instead of taking the app down. Sidecars and XMP
//! packets, which aigle writes itself, are still parsed in-process.

use std::path::{Path, PathBuf};

use aigle_exiv2_sys::ExifFacts;
use aigle_isolate::{Failure, Handler, Reply, Task};

const FACTS: &str = "library.facts";
const XMP: &str = "library.xmp";

/// The library's worker operations, for [`aigle_isolate::serve`].
pub const OPS: &[(&str, Handler)] = &[(FACTS, serve_facts), (XMP, serve_xmp)];

type FactsWire = [Option<String>; 4];

/// [`aigle_exiv2_sys::read_facts`], in a worker when isolation is on.
pub(crate) fn exif_facts(path: &Path) -> Result<ExifFacts, String> {
    if !aigle_isolate::enabled() {
        return aigle_exiv2_sys::read_facts(path).map_err(|e| e.0);
    }
    let [make, model, lens, date_time] =
        aigle_isolate::call::<FactsWire>(FACTS, &path, None).map_err(|e| reported(path, e))?;
    Ok(ExifFacts {
        make,
        model,
        lens,
        date_time,
    })
}

/// [`aigle_exiv2_sys::read_file`] of a photo, in a worker when isolation is on.
pub(crate) fn embedded_xmp(path: &Path) -> Result<Option<String>, String> {
    if !aigle_isolate::enabled() {
        return aigle_exiv2_sys::read_file(path).map_err(|e| e.0);
    }
    aigle_isolate::call(XMP, &path, None).map_err(|e| reported(path, e))
}

fn reported(path: &Path, e: aigle_isolate::Error) -> String {
    if let aigle_isolate::Error::Crashed(m) = &e {
        tracing::warn!(path = %path.display(), "Exiv2 crashed on this file: {m}");
    }
    e.to_string()
}

fn path_of(body: &[u8]) -> Result<PathBuf, Failure> {
    serde_json::from_slice(body).map_err(|e| Failure::Failed(format!("bad request: {e}")))
}

fn serve_facts(body: &[u8], _: &mut Task) -> Result<Reply, Failure> {
    let f = aigle_exiv2_sys::read_facts(&path_of(body)?).map_err(|e| Failure::Failed(e.0))?;
    let wire: FactsWire = [f.make, f.model, f.lens, f.date_time];
    Reply::new(&wire)
}

fn serve_xmp(body: &[u8], _: &mut Task) -> Result<Reply, Failure> {
    let packet = aigle_exiv2_sys::read_file(&path_of(body)?).map_err(|e| Failure::Failed(e.0))?;
    Reply::new(&packet)
}
