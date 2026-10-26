use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Monotonic revision of a photo's edit state, assigned by `doc` (doc 01 §5).
#[derive(
    Copy,
    Clone,
    Debug,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    JsonSchema,
)]
pub struct Revision(pub u64);

impl Revision {
    pub const ZERO: Revision = Revision(0);

    pub fn next(self) -> Revision {
        Revision(self.0 + 1)
    }
}

/// Stable id of a local adjustment inside one photo's params (doc 02 §9).
#[derive(
    Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
pub struct MaskId(pub u32);

macro_rules! hash_id {
    ($(#[$m:meta])* $name:ident) => {
        $(#[$m])*
        #[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema)]
        #[serde(transparent)]
        pub struct $name(pub String);

        impl $name {
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

hash_id!(
    /// Content-addressed result of a heavy job (AI denoise, semantic mask) (doc 02 §10).
    /// Hex-encoded BLAKE3 hash of the asset key.
    AssetId
);
hash_id!(
    /// Id of a semantic/AI mask asset persisted in the photo's `.aigle-masks` file.
    MaskAssetId
);
hash_id!(
    /// Content hash of resolved, pinned external data: lens profile, camera matrix, DCP, look
    /// (doc 02 §13.2).
    DataRef
);
