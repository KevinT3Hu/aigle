//! Local adjustments (doc 02 §9) and semantic mask targets (doc 06 §5.1).
//!
//! All shapes are anchored in Source space with coordinates normalised to the oriented source
//! size, so crop/rotation/lens changes never move a mask relative to its subject.

use std::fmt;
use std::str::FromStr;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::ids::{AssetId, MaskId};
use crate::params::FieldSpec;

/// Normalised Source-space point.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Pt {
    pub x: f32,
    pub y: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Stroke {
    pub points: Vec<Pt>,
    /// Brush radius as a fraction of the source long edge.
    pub size: f32,
    /// 0..100.
    pub feather: f32,
    /// 0..100.
    pub flow: f32,
    /// Erase instead of paint.
    #[serde(default)]
    pub erase: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum RangeKind {
    Luminance {
        low: f32,
        high: f32,
    },
    Color {
        samples: Vec<[f32; 3]>,
        tolerance: f32,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CombineOp {
    Add,
    Subtract,
    Intersect,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct EdgeRefine {
    /// 0..100; guided-filter strength at sampling time (doc 06 §5.3).
    pub amount: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum MaskShape {
    Brush {
        strokes: Vec<Stroke>,
    },
    Linear {
        from: Pt,
        to: Pt,
    },
    Radial {
        center: Pt,
        radii: [f32; 2],
        angle: f32,
        feather: f32,
    },
    Semantic {
        asset: AssetId,
        target: MaskTarget,
        refine: EdgeRefine,
    },
    Range {
        range: RangeKind,
        smoothness: f32,
    },
    Combine {
        op: CombineOp,
        parts: Vec<MaskShape>,
    },
}

/// Declares the sparse local parameter set once, with the spec of each field.
macro_rules! local_params {
    ($( $field:ident : [$min:expr, $max:expr], step $step:expr, $unit:literal; )*) => {
        /// Sparse per-mask deltas (doc 02 §9). `None` = this mask does not touch the parameter.
        #[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
        #[serde(default)]
        pub struct LocalParams {
            $(
                #[serde(skip_serializing_if = "Option::is_none")]
                pub $field: Option<f32>,
            )*
        }

        impl LocalParams {
            pub const SPECS: &'static [FieldSpec] = &[
                $( FieldSpec { name: stringify!($field), min: $min, max: $max, step: $step,
                               default: 0.0, unit: $unit, doc: "" }, )*
            ];

            /// Visit the parameters that are set.
            #[allow(unused_assignments)]
            pub fn visit_mut(&mut self, f: &mut dyn FnMut(&FieldSpec, &mut f32)) {
                let mut i = 0usize;
                $(
                    if let Some(v) = self.$field.as_mut() { f(&Self::SPECS[i], v); }
                    i += 1;
                )*
            }

            #[allow(unused_assignments)]
            pub fn visit(&self, f: &mut dyn FnMut(&FieldSpec, f32)) {
                let mut i = 0usize;
                $(
                    if let Some(v) = self.$field { f(&Self::SPECS[i], v); }
                    i += 1;
                )*
            }
        }
    };
}

local_params! {
    exposure: [-4.0, 4.0], step 0.01, "EV";
    temperature: [-100.0, 100.0], step 1.0, "";
    tint: [-100.0, 100.0], step 1.0, "";
    contrast: [-100.0, 100.0], step 1.0, "";
    highlights: [-100.0, 100.0], step 1.0, "";
    shadows: [-100.0, 100.0], step 1.0, "";
    whites: [-100.0, 100.0], step 1.0, "";
    blacks: [-100.0, 100.0], step 1.0, "";
    texture: [-100.0, 100.0], step 1.0, "";
    clarity: [-100.0, 100.0], step 1.0, "";
    dehaze: [-100.0, 100.0], step 1.0, "";
    saturation: [-100.0, 100.0], step 1.0, "";
    hue: [-180.0, 180.0], step 1.0, "deg";
    sharpness: [-100.0, 100.0], step 1.0, "";
    noise: [-100.0, 100.0], step 1.0, "";
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct LocalAdjustment {
    pub id: MaskId,
    pub name: String,
    pub shape: MaskShape,
    #[serde(default)]
    pub invert: bool,
    /// 0..1.
    pub opacity: f32,
    #[serde(default)]
    pub adjust: LocalParams,
}

// ---------------------------------------------------------------------------------------------
// Semantic targets (doc 06 §5.1)

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PersonSel {
    All,
    Index(u32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PersonPart {
    Whole,
    FaceSkin,
    BodySkin,
    Hair,
    Eyes,
    Lips,
    Clothes,
}

impl PersonPart {
    const ALL: [(PersonPart, &'static str); 6] = [
        (PersonPart::FaceSkin, "face_skin"),
        (PersonPart::BodySkin, "body_skin"),
        (PersonPart::Hair, "hair"),
        (PersonPart::Eyes, "eyes"),
        (PersonPart::Lips, "lips"),
        (PersonPart::Clothes, "clothes"),
    ];
}

/// Semantic mask target. Textual form (profiles, agents): `subject`, `sky`, `background`,
/// `people`, `people.hair`, `person[1].face_skin`, …
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(try_from = "String", into = "String")]
#[schemars(with = "String")]
pub enum MaskTarget {
    Subject,
    Sky,
    Background,
    People { which: PersonSel, part: PersonPart },
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid mask target `{0}`")]
pub struct ParseMaskTargetError(pub String);

impl FromStr for MaskTarget {
    type Err = ParseMaskTargetError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let err = || ParseMaskTargetError(s.to_string());
        match s {
            "subject" => return Ok(MaskTarget::Subject),
            "sky" => return Ok(MaskTarget::Sky),
            "background" => return Ok(MaskTarget::Background),
            _ => {}
        }
        let (head, part) = match s.split_once('.') {
            Some((h, p)) => (h, Some(p)),
            None => (s, None),
        };
        let which = if head == "people" {
            PersonSel::All
        } else if let Some(idx) = head
            .strip_prefix("person[")
            .and_then(|r| r.strip_suffix(']'))
        {
            PersonSel::Index(idx.parse().map_err(|_| err())?)
        } else {
            return Err(err());
        };
        let part = match part {
            None => PersonPart::Whole,
            Some(p) => PersonPart::ALL
                .iter()
                .find(|(_, n)| *n == p)
                .map(|(v, _)| *v)
                .ok_or_else(err)?,
        };
        Ok(MaskTarget::People { which, part })
    }
}

impl fmt::Display for MaskTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MaskTarget::Subject => f.write_str("subject"),
            MaskTarget::Sky => f.write_str("sky"),
            MaskTarget::Background => f.write_str("background"),
            MaskTarget::People { which, part } => {
                match which {
                    PersonSel::All => f.write_str("people")?,
                    PersonSel::Index(i) => write!(f, "person[{i}]")?,
                }
                if let Some((_, n)) = PersonPart::ALL.iter().find(|(p, _)| p == part) {
                    write!(f, ".{n}")?;
                }
                Ok(())
            }
        }
    }
}

impl TryFrom<String> for MaskTarget {
    type Error = ParseMaskTargetError;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        s.parse()
    }
}

impl From<MaskTarget> for String {
    fn from(t: MaskTarget) -> String {
        t.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mask_target_round_trip() {
        for s in [
            "subject",
            "sky",
            "background",
            "people",
            "people.hair",
            "person[1].face_skin",
            "person[0]",
        ] {
            let t: MaskTarget = s.parse().unwrap();
            assert_eq!(t.to_string(), s);
        }
        assert!("people.nose".parse::<MaskTarget>().is_err());
        assert!("person[x]".parse::<MaskTarget>().is_err());
    }
}
