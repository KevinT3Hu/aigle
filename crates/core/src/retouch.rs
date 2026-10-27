//! Retouching: spot removal (heal / clone) and red-eye removal.
//!
//! Like masks, every spot is anchored in Source space with points normalised to the oriented
//! source size; radii are fractions of the source long edge. The engine applies them to the
//! S-processed source pixels, before geometry and before local adjustments.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::masks::Pt;

pub const MIN_RADIUS: f32 = 0.0005;
pub const MAX_RADIUS: f32 = 0.25;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SpotMode {
    /// Source texture, blended into the target's surroundings (a membrane fitted to the
    /// target's boundary).
    #[default]
    Heal,
    /// Source pixels copied as they are.
    Clone,
}

/// One circular spot: the disc at `target` is replaced from the disc at `source`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Spot {
    #[serde(default)]
    pub mode: SpotMode,
    pub target: Pt,
    pub source: Pt,
    /// Fraction of the source long edge.
    pub radius: f32,
    /// 0..100: width of the soft edge, as a percentage of the radius.
    #[serde(default = "default_feather")]
    pub feather: f32,
    /// 0..1.
    #[serde(default = "default_opacity")]
    pub opacity: f32,
}

/// One eye: red pixels in the disc are desaturated and darkened.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RedEye {
    pub center: Pt,
    /// Fraction of the source long edge.
    pub radius: f32,
    /// 0..100: how far from pure red a pixel may be and still count as part of the pupil.
    #[serde(default = "default_pupil")]
    pub pupil: f32,
    /// 0..100: how much the corrected pupil is darkened.
    #[serde(default = "default_darken")]
    pub darken: f32,
}

fn default_feather() -> f32 {
    50.0
}

fn default_opacity() -> f32 {
    1.0
}

fn default_pupil() -> f32 {
    50.0
}

fn default_darken() -> f32 {
    50.0
}

impl Spot {
    pub fn new(mode: SpotMode, target: Pt, source: Pt, radius: f32) -> Spot {
        Spot {
            mode,
            target,
            source,
            radius,
            feather: default_feather(),
            opacity: default_opacity(),
        }
    }

    pub(crate) fn clamp(&mut self) {
        self.radius = self.radius.clamp(MIN_RADIUS, MAX_RADIUS);
        self.feather = self.feather.clamp(0.0, 100.0).round() + 0.0;
        self.opacity = self.opacity.clamp(0.0, 1.0) + 0.0;
    }

    pub(crate) fn is_finite(&self) -> bool {
        [
            self.target.x,
            self.target.y,
            self.source.x,
            self.source.y,
            self.radius,
            self.feather,
            self.opacity,
        ]
        .iter()
        .all(|v| v.is_finite())
    }
}

impl RedEye {
    pub fn new(center: Pt, radius: f32) -> RedEye {
        RedEye {
            center,
            radius,
            pupil: default_pupil(),
            darken: default_darken(),
        }
    }

    pub(crate) fn clamp(&mut self) {
        self.radius = self.radius.clamp(MIN_RADIUS, MAX_RADIUS);
        self.pupil = self.pupil.clamp(0.0, 100.0).round() + 0.0;
        self.darken = self.darken.clamp(0.0, 100.0).round() + 0.0;
    }

    pub(crate) fn is_finite(&self) -> bool {
        [
            self.center.x,
            self.center.y,
            self.radius,
            self.pupil,
            self.darken,
        ]
        .iter()
        .all(|v| v.is_finite())
    }
}
