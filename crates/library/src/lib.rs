//! Folders, sidecars and the catalog (doc 01 §9).
//!
//! * [`scan`] lists the photos of a folder without reading them.
//! * [`Store`] reads and writes the XMP sidecar next to each photo, the only source of truth
//!   for its edits, rating and color label. Writes are atomic and refuse to overwrite a
//!   sidecar that changed on disk since aigle last read or wrote it.
//! * [`Writer`] debounces writes on a background thread.
//! * [`Catalog`] keeps edit history and virtual copies in SQLite.

mod catalog;
mod scan;
mod sidecar;
mod writer;

pub use catalog::{Catalog, CatalogError, CopyId, PhotoKey, VirtualCopy};
pub use scan::{PhotoEntry, scan};
pub use sidecar::{Error, Loaded, PhotoMeta, SCHEMA_VERSION, Snapshot, Store, sidecar_candidates};
pub use writer::{WriteReport, Writer};
