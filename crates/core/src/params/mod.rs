//! `EditParams` and its parameter-path, validation and schema machinery (doc 02 §4).

mod groups;
pub(crate) mod spec;
pub mod tone;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub use groups::*;
pub use spec::{FieldSpec, Scalars, round_to_step};

use crate::masks::{LocalAdjustment, MaskShape, Pt};
use crate::retouch::{RedEye, Spot};
use spec::join;

/// Process version (doc 02 §13). Stages pick their implementation from it.
#[derive(
    Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
pub struct ProcessVersion(pub u32);

impl ProcessVersion {
    pub const V1: ProcessVersion = ProcessVersion(1);
    pub const LATEST: ProcessVersion = ProcessVersion::V1;
}

impl Default for ProcessVersion {
    fn default() -> Self {
        ProcessVersion::LATEST
    }
}

/// Dotted parameter path, e.g. `light.exposure`, `mixer.blue.saturation`.
#[derive(
    Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(transparent)]
pub struct ParamPath(pub String);

impl ParamPath {
    pub fn new(s: impl Into<String>) -> Self {
        ParamPath(s.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ParamPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for ParamPath {
    fn from(s: &str) -> Self {
        ParamPath(s.to_string())
    }
}

/// The complete, non-destructive edit of one photo.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct EditParams {
    pub process_version: ProcessVersion,
    pub raw: RawParams,
    pub lens: LensParams,
    pub geometry: GeometryParams,
    pub profile: InputProfile,
    pub wb: WhiteBalance,
    pub light: LightParams,
    pub presence: PresenceParams,
    pub curve: ToneCurve,
    pub mixer: ColorMixer,
    pub bw: BwParams,
    pub grading: ColorGrading,
    pub detail: DetailParams,
    pub effects: EffectsParams,
    pub masks: Vec<LocalAdjustment>,
    /// Applied in order; each source reads the image before any spot.
    pub spots: Vec<Spot>,
    pub red_eye: Vec<RedEye>,
}

impl Scalars for EditParams {
    fn visit(&self, prefix: &str, f: &mut dyn FnMut(&str, &FieldSpec, f32)) {
        self.raw.visit(&join(prefix, "raw"), f);
        self.lens.visit(&join(prefix, "lens"), f);
        self.geometry.visit(&join(prefix, "geometry"), f);
        self.profile.visit(&join(prefix, "profile"), f);
        self.wb.visit(&join(prefix, "wb"), f);
        self.light.visit(&join(prefix, "light"), f);
        self.presence.visit(&join(prefix, "presence"), f);
        self.curve.visit(&join(prefix, "curve"), f);
        self.mixer.visit(&join(prefix, "mixer"), f);
        self.bw.visit(&join(prefix, "bw"), f);
        self.grading.visit(&join(prefix, "grading"), f);
        self.detail.visit(&join(prefix, "detail"), f);
        self.effects.visit(&join(prefix, "effects"), f);
    }

    fn visit_mut(&mut self, prefix: &str, f: &mut dyn FnMut(&str, &FieldSpec, &mut f32)) {
        self.raw.visit_mut(&join(prefix, "raw"), f);
        self.lens.visit_mut(&join(prefix, "lens"), f);
        self.geometry.visit_mut(&join(prefix, "geometry"), f);
        self.profile.visit_mut(&join(prefix, "profile"), f);
        self.wb.visit_mut(&join(prefix, "wb"), f);
        self.light.visit_mut(&join(prefix, "light"), f);
        self.presence.visit_mut(&join(prefix, "presence"), f);
        self.curve.visit_mut(&join(prefix, "curve"), f);
        self.mixer.visit_mut(&join(prefix, "mixer"), f);
        self.bw.visit_mut(&join(prefix, "bw"), f);
        self.grading.visit_mut(&join(prefix, "grading"), f);
        self.detail.visit_mut(&join(prefix, "detail"), f);
        self.effects.visit_mut(&join(prefix, "effects"), f);
    }
}

/// A value that was outside its range and was clamped (reported back to agents/profiles).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ClampReport {
    pub path: String,
    pub requested: f32,
    pub applied: f32,
}

#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum ParamsError {
    #[error("value at `{0}` is not finite")]
    NotFinite(String),
    #[error("unknown parameter path `{0}`")]
    UnknownPath(String),
}

impl EditParams {
    /// The unedited photo framed like `self`: defaults with this edit's geometry and lens
    /// correction, so that a before/after comparison shows the same region.
    pub fn original_framing(&self) -> EditParams {
        EditParams {
            process_version: self.process_version,
            lens: self.lens.clone(),
            geometry: self.geometry.clone(),
            ..EditParams::default()
        }
    }

    /// The same edit with the crop removed: the whole straightened frame (crop tool).
    pub fn uncropped(&self) -> EditParams {
        let mut p = self.clone();
        p.geometry.crop = CropRect::default();
        p
    }

    /// Reject NaN/Inf, clamp every value to its range and round it to its step.
    ///
    /// `doc` calls this at commit so that the stored edit is exactly what was rendered; the
    /// engine calls it defensively on every `set_edits`. Idempotent.
    pub fn sanitize(&mut self) -> Result<Vec<ClampReport>, ParamsError> {
        self.check_finite()?;
        let mut reports = Vec::new();

        self.visit_mut("", &mut |path, spec, v| {
            let n = spec.normalize(*v);
            if *v < spec.min || *v > spec.max {
                reports.push(ClampReport {
                    path: path.to_string(),
                    requested: *v,
                    applied: n,
                });
            }
            *v = n;
        });

        // Crop must stay inside the frame.
        let crop = &mut self.geometry.crop;
        for (pos, len, name) in [(&crop.x, &mut crop.w, "w"), (&crop.y, &mut crop.h, "h")] {
            if *pos + *len > 1.0 {
                let fitted = round_down_to_step(1.0 - *pos, GEOMETRY_STEP).max(0.001);
                reports.push(ClampReport {
                    path: format!("geometry.crop.{name}"),
                    requested: *len,
                    applied: fitted,
                });
                *len = fitted;
            }
        }
        // Normalise the crop origin too in case the length fit moved it.
        let crop = &mut self.geometry.crop;
        crop.x = crop.x.min(1.0 - crop.w).max(0.0);
        crop.y = crop.y.min(1.0 - crop.h).max(0.0);

        self.geometry.orientation_delta.quarter_turns %= 4;

        for curve in self.curve.point.all_mut() {
            for p in curve.iter_mut() {
                p.x = round_to_step(p.x.clamp(0.0, 1.0), GEOMETRY_STEP) + 0.0;
                p.y = round_to_step(p.y.clamp(0.0, 1.0), GEOMETRY_STEP) + 0.0;
            }
            curve.sort_by(|a, b| a.x.total_cmp(&b.x));
            curve.dedup_by(|a, b| a.x == b.x);
        }

        for (i, m) in self.masks.iter_mut().enumerate() {
            m.opacity = m.opacity.clamp(0.0, 1.0) + 0.0;
            m.adjust.visit_mut(&mut |spec, v| {
                let n = spec.normalize(*v);
                if *v < spec.min || *v > spec.max {
                    reports.push(ClampReport {
                        path: format!("masks[{i}].adjust.{}", spec.name),
                        requested: *v,
                        applied: n,
                    });
                }
                *v = n;
            });
            visit_shape_points(&mut m.shape, &mut |p| {
                p.x = round_to_step(p.x, GEOMETRY_STEP) + 0.0;
                p.y = round_to_step(p.y, GEOMETRY_STEP) + 0.0;
            });
        }

        let round = |p: &mut Pt| {
            p.x = round_to_step(p.x, GEOMETRY_STEP) + 0.0;
            p.y = round_to_step(p.y, GEOMETRY_STEP) + 0.0;
        };
        for s in &mut self.spots {
            s.clamp();
            round(&mut s.target);
            round(&mut s.source);
        }
        for r in &mut self.red_eye {
            r.clamp();
            round(&mut r.center);
        }

        Ok(reports)
    }

    fn check_finite(&self) -> Result<(), ParamsError> {
        let mut bad: Option<String> = None;
        self.visit("", &mut |path, _, v| {
            if !v.is_finite() && bad.is_none() {
                bad = Some(path.to_string());
            }
        });
        if let Some(p) = bad {
            return Err(ParamsError::NotFinite(p));
        }
        for (name, curve) in ["rgb", "red", "green", "blue"]
            .iter()
            .zip(self.curve.point.all())
        {
            if curve.iter().any(|p| !p.x.is_finite() || !p.y.is_finite()) {
                return Err(ParamsError::NotFinite(format!("curve.point.{name}")));
            }
        }
        for (i, m) in self.masks.iter().enumerate() {
            let mut ok = m.opacity.is_finite();
            m.adjust.visit(&mut |_, v| ok &= v.is_finite());
            let mut shape = m.shape.clone();
            visit_shape_points(&mut shape, &mut |p| {
                ok &= p.x.is_finite() && p.y.is_finite()
            });
            if !ok {
                return Err(ParamsError::NotFinite(format!("masks[{i}]")));
            }
        }
        if let Some(i) = self.spots.iter().position(|s| !s.is_finite()) {
            return Err(ParamsError::NotFinite(format!("spots[{i}]")));
        }
        if let Some(i) = self.red_eye.iter().position(|r| !r.is_finite()) {
            return Err(ParamsError::NotFinite(format!("red_eye[{i}]")));
        }
        Ok(())
    }

    /// Read a scalar by path.
    pub fn get(&self, path: &str) -> Option<f32> {
        let mut out = None;
        self.visit("", &mut |p, _, v| {
            if p == path {
                out = Some(v);
            }
        });
        out
    }

    /// The spec of a scalar path, if it exists in these params.
    pub fn spec(&self, path: &str) -> Option<FieldSpec> {
        let mut out = None;
        self.visit("", &mut |p, s, _| {
            if p == path {
                out = Some(*s);
            }
        });
        out
    }

    /// Set a scalar by path, clamping and rounding it. Returns a report if it was clamped.
    pub fn set(&mut self, path: &str, value: f32) -> Result<Option<ClampReport>, ParamsError> {
        if !value.is_finite() {
            return Err(ParamsError::NotFinite(path.to_string()));
        }
        let mut found = false;
        let mut report = None;
        self.visit_mut("", &mut |p, spec, v| {
            if p == path {
                found = true;
                let n = spec.normalize(value);
                if value < spec.min || value > spec.max {
                    report = Some(ClampReport {
                        path: p.to_string(),
                        requested: value,
                        applied: n,
                    });
                }
                *v = n;
            }
        });
        if found {
            Ok(report)
        } else {
            Err(ParamsError::UnknownPath(path.to_string()))
        }
    }

    /// Reset a scalar to its default.
    pub fn reset(&mut self, path: &str) -> Result<(), ParamsError> {
        let mut found = false;
        self.visit_mut("", &mut |p, spec, v| {
            if p == path {
                found = true;
                *v = spec.default;
            }
        });
        if found {
            Ok(())
        } else {
            Err(ParamsError::UnknownPath(path.to_string()))
        }
    }

    /// All scalar paths with their specs (the always-present ones; optional groups such as
    /// `raw.ai_denoise` only appear when set).
    pub fn scalar_specs() -> Vec<(String, FieldSpec)> {
        let mut out = Vec::new();
        EditParams::default().visit("", &mut |p, s, _| out.push((p.to_string(), *s)));
        out
    }

    /// Scalars that differ from their default, for the agent's context summary (doc 01 §7).
    pub fn non_default(&self) -> Vec<(String, f32)> {
        let mut out = Vec::new();
        self.visit("", &mut |p, s, v| {
            if v != s.default {
                out.push((p.to_string(), v));
            }
        });
        out
    }

    /// JSON Schema of `EditParams`, annotated with the range, step, unit and description of
    /// every scalar. The single source of truth for agent docs and validation (doc 01 §7).
    pub fn json_schema() -> serde_json::Value {
        let generator = schemars::generate::SchemaSettings::draft2020_12()
            .with(|s| s.inline_subschemas = true)
            .into_generator();
        let schema = generator.into_root_schema_for::<EditParams>();
        let mut value = serde_json::to_value(&schema).expect("schema serialises");
        for (path, spec) in Self::scalar_specs() {
            if let Some(node) = schema_node_mut(&mut value, &path)
                && let Some(obj) = node.as_object_mut()
            {
                obj.insert("minimum".into(), spec.min.into());
                obj.insert("maximum".into(), spec.max.into());
                obj.insert("default".into(), spec.default.into());
                obj.insert("x-step".into(), spec.step.into());
                if !spec.unit.is_empty() {
                    obj.insert("x-unit".into(), spec.unit.into());
                }
                if !spec.doc.is_empty() {
                    obj.insert("description".into(), spec.doc.trim().into());
                }
            }
        }
        value
    }
}

fn round_down_to_step(v: f32, step: f32) -> f32 {
    let r = round_to_step(v, step);
    if r > v {
        round_to_step(v - step, step)
    } else {
        r
    }
}

fn schema_node_mut<'a>(
    root: &'a mut serde_json::Value,
    path: &str,
) -> Option<&'a mut serde_json::Value> {
    let mut node = root;
    for seg in path.split('.') {
        node = node.get_mut("properties")?.get_mut(seg)?;
    }
    Some(node)
}

fn visit_shape_points(shape: &mut MaskShape, f: &mut dyn FnMut(&mut Pt)) {
    match shape {
        MaskShape::Brush { strokes } => strokes
            .iter_mut()
            .flat_map(|s| s.points.iter_mut())
            .for_each(f),
        MaskShape::Linear { from, to } => {
            f(from);
            f(to);
        }
        MaskShape::Radial { center, .. } => f(center),
        MaskShape::Semantic { .. } | MaskShape::Range { .. } => {}
        MaskShape::Combine { parts, .. } => parts.iter_mut().for_each(|p| visit_shape_points(p, f)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_within_spec_and_sanitize_is_noop() {
        let mut p = EditParams::default();
        let before = p.clone();
        assert!(p.sanitize().unwrap().is_empty());
        assert_eq!(p, before);
    }

    #[test]
    fn get_set_reset_by_path() {
        let mut p = EditParams::default();
        assert_eq!(p.set("light.exposure", 0.304).unwrap(), None);
        assert_eq!(p.get("light.exposure"), Some(0.3));
        let r = p.set("mixer.blue.saturation", 250.0).unwrap().unwrap();
        assert_eq!(r.applied, 100.0);
        assert_eq!(p.mixer.blue.saturation, 100.0);
        assert_eq!(p.set("wb.temperature", 4321.4).unwrap(), None);
        assert_eq!(p.wb.values.temperature, 4321.0);
        p.reset("light.exposure").unwrap();
        assert_eq!(p.light.exposure, 0.0);
        assert!(matches!(
            p.set("light.nope", 1.0),
            Err(ParamsError::UnknownPath(_))
        ));
        assert!(matches!(
            p.set("light.exposure", f32::NAN),
            Err(ParamsError::NotFinite(_))
        ));
    }

    #[test]
    fn paths_are_unique() {
        let specs = EditParams::scalar_specs();
        let mut paths: Vec<_> = specs.iter().map(|(p, _)| p.clone()).collect();
        paths.sort();
        let n = paths.len();
        paths.dedup();
        assert_eq!(n, paths.len());
        assert!(paths.contains(&"grading.blending".to_string()));
        assert!(paths.contains(&"geometry.crop.w".to_string()));
        assert!(paths.contains(&"geometry.angle".to_string()));
    }

    #[test]
    fn sanitize_rejects_nan_and_fits_crop() {
        let mut p = EditParams::default();
        p.light.contrast = f32::NAN;
        assert!(p.sanitize().is_err());

        let mut p = EditParams::default();
        p.geometry.crop.x = 0.5;
        p.geometry.crop.w = 0.8;
        let reports = p.sanitize().unwrap();
        assert_eq!(p.geometry.crop.w, 0.5);
        assert_eq!(reports.len(), 1);
        // Idempotent.
        let again = p.clone();
        assert!(p.sanitize().unwrap().is_empty());
        assert_eq!(p, again);
    }

    #[test]
    fn serde_round_trip_and_flattened_paths() {
        let mut p = EditParams::default();
        p.set("wb.tint", 12.0).unwrap();
        let json = serde_json::to_value(&p).unwrap();
        assert_eq!(json["wb"]["tint"], 12.0);
        assert_eq!(json["geometry"]["angle"], 0.0);
        let back: EditParams = serde_json::from_value(json).unwrap();
        assert_eq!(back, p);
        // Missing fields take defaults.
        let partial: EditParams = serde_json::from_str(r#"{"light":{"exposure":1.5}}"#).unwrap();
        assert_eq!(partial.light.exposure, 1.5);
        assert_eq!(partial.grading.blend.blending, 50.0);
    }

    #[test]
    fn schema_is_annotated() {
        let s = EditParams::json_schema();
        let exp = &s["properties"]["light"]["properties"]["exposure"];
        assert_eq!(exp["minimum"], -5.0);
        assert_eq!(exp["x-unit"], "EV");
        let temp = &s["properties"]["wb"]["properties"]["temperature"];
        assert_eq!(temp["maximum"], 50000.0);
        let band = &s["properties"]["mixer"]["properties"]["blue"]["properties"]["hue"];
        assert_eq!(band["minimum"], -100.0);
    }
}
