//! Folders, sidecars and the catalog (doc 01 §9).
//!
//! * [`scan`] lists the photos of a folder without reading them; [`scan_recursive`] those
//!   of its subfolders too.
//! * [`import`] copies photos from a memory card or folder into the library's folders.
//! * [`Store`] reads and writes the XMP sidecar next to each photo, the only source of truth
//!   for its edits, rating, color label, keywords and IPTC fields. Writes are atomic and
//!   refuse to overwrite a sidecar that changed on disk since aigle last read or wrote it.
//! * [`Writer`] debounces writes on a background thread.
//! * [`Catalog`] keeps edit history and virtual copies in SQLite.
//! * [`read_facts`] reads the camera, lens and capture date that the library filters by.
//! * [`move_photo`] and [`trash_photo`] take a photo's sidecars along.

mod catalog;
mod facts;
mod files;
pub mod import;
mod scan;
mod sidecar;
mod worker;
mod writer;

pub use catalog::{Catalog, CatalogError, CopyId, PhotoKey, VirtualCopy};
pub use facts::{Facts, camera_name, read_facts};
pub use files::{FileError, move_photo, sidecars_of, trash_photo};
pub use scan::{MAX_DEPTH, PhotoEntry, scan, scan_recursive};
pub use sidecar::{
    Error, Field, LEVEL_SEP, Loaded, PhotoMeta, SCHEMA_VERSION, Snapshot, Store, clean_keywords,
    normalize_tag, sidecar_candidates, write_dng_sidecar,
};
pub use worker::OPS as WORKER_OPS;
pub use writer::{WriteReport, Writer};
