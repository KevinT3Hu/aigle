//! aigle core: the pure, UI- and engine-independent part of aigle.
//!
//! * [`params`]: `EditParams`, parameter paths, ranges/steps and the JSON Schema (doc 02 §4).
//! * [`masks`]: local adjustments and semantic mask targets (doc 02 §9, doc 06 §5).
//! * [`retouch`]: spot removal and red-eye removal.
//! * [`geometry`]: coordinate spaces and the forward/inverse geometry maps shared by the
//!   engine and the UI (doc 02 §2).
//! * [`color`]: colour-science helpers (matrices, CCT/tint, CAT16) (doc 02 §6).
//! * [`ids`]: revisions and content-addressed identifiers.

pub mod color;
pub mod geometry;
pub mod ids;
pub mod lens;
pub mod masks;
pub mod params;
pub mod retouch;

pub use ids::{AssetId, DataRef, MaskAssetId, MaskId, Revision};
pub use params::{EditParams, ParamPath, ProcessVersion};
