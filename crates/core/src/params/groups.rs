//! Parameter groups of `EditParams` (doc 02 §4). Ranges follow Lightroom-like user units;
//! the exact values are provisional until round 4 (doc 01 §12).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::spec::{FieldSpec, Scalars, join, nested_group, scalar_group};
use crate::ids::{AssetId, DataRef};

/// Steps for normalised geometry: 6 decimals (doc 02 §4).
pub const GEOMETRY_STEP: f32 = 0.000_001;

// ---------------------------------------------------------------------------------------------
// raw

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Demosaic {
    /// RCD for Bayer, Markesteijn 3-pass for X-Trans, passthrough for linear DNG.
    #[default]
    Auto,
    Rcd,
    Amaze,
    Markesteijn3,
    Passthrough,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HighlightRecon {
    Off,
    #[default]
    ColorPropagation,
    InpaintOpposed,
    Guided,
}

scalar_group! {
    /// Blend of the AI-denoise asset with the normal demosaic output (S1, doc 06 §4.1).
    pub struct AiDenoiseAmount {
        /// Strength of AI denoise.
        amount: 100.0, [0.0, 100.0], step 1.0, "%";
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AiDenoise {
    pub asset: AssetId,
    #[serde(flatten)]
    pub strength: AiDenoiseAmount,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct RawParams {
    pub demosaic: Demosaic,
    pub ca_correction: bool,
    pub hot_pixels: bool,
    pub highlight_recon: HighlightRecon,
    pub ai_denoise: Option<AiDenoise>,
}

impl Default for RawParams {
    fn default() -> Self {
        Self {
            demosaic: Demosaic::Auto,
            ca_correction: true,
            hot_pixels: true,
            highlight_recon: HighlightRecon::default(),
            ai_denoise: None,
        }
    }
}

impl Scalars for RawParams {
    fn visit(&self, prefix: &str, f: &mut dyn FnMut(&str, &FieldSpec, f32)) {
        if let Some(ai) = &self.ai_denoise {
            ai.strength.visit(&join(prefix, "ai_denoise"), f);
        }
    }
    fn visit_mut(&mut self, prefix: &str, f: &mut dyn FnMut(&str, &FieldSpec, &mut f32)) {
        if let Some(ai) = &mut self.ai_denoise {
            ai.strength.visit_mut(&join(prefix, "ai_denoise"), f);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// lens

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", tag = "kind", content = "data")]
pub enum LensProfile {
    #[default]
    Off,
    Resolved(DataRef),
}

scalar_group! {
    pub struct LensAmounts {
        /// Amount of the profile's distortion correction.
        distortion: 100.0, [0.0, 200.0], step 1.0, "%";
        /// Amount of the profile's transverse chromatic aberration correction.
        tca: 100.0, [0.0, 200.0], step 1.0, "%";
        /// Amount of the profile's vignetting correction.
        vignetting: 100.0, [0.0, 200.0], step 1.0, "%";
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct LensParams {
    pub profile: LensProfile,
    #[serde(flatten)]
    pub amounts: LensAmounts,
}

impl Scalars for LensParams {
    fn visit(&self, prefix: &str, f: &mut dyn FnMut(&str, &FieldSpec, f32)) {
        self.amounts.visit(prefix, f);
    }
    fn visit_mut(&mut self, prefix: &str, f: &mut dyn FnMut(&str, &FieldSpec, &mut f32)) {
        self.amounts.visit_mut(prefix, f);
    }
}

// ---------------------------------------------------------------------------------------------
// geometry

/// A user 90° rotation / flip on top of the EXIF orientation (doc 02 §4). Part of the Source
/// space definition, not a resampling parameter.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct OrientationDelta {
    /// Clockwise quarter turns, 0..=3.
    pub quarter_turns: u8,
    /// Horizontal mirror, applied before the rotation.
    pub flip: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Aspect {
    #[default]
    Free,
    Original,
    Ratio {
        w: u32,
        h: u32,
    },
}

scalar_group! {
    /// Crop rectangle, normalised to the oriented source size, in the straightened frame
    /// (see `geometry::GeometryMap`).
    pub struct CropRect {
        x: 0.0, [0.0, 1.0], step GEOMETRY_STEP, "";
        y: 0.0, [0.0, 1.0], step GEOMETRY_STEP, "";
        w: 1.0, [0.001, 1.0], step GEOMETRY_STEP, "";
        h: 1.0, [0.001, 1.0], step GEOMETRY_STEP, "";
    }
}

scalar_group! {
    pub struct GeometryScalars {
        /// Straightening angle, clockwise positive.
        angle: 0.0, [-45.0, 45.0], step 0.01, "deg";
        /// Vertical perspective correction.
        perspective_vertical: 0.0, [-100.0, 100.0], step 1.0, "";
        /// Horizontal perspective correction.
        perspective_horizontal: 0.0, [-100.0, 100.0], step 1.0, "";
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct GeometryParams {
    pub orientation_delta: OrientationDelta,
    pub aspect: Aspect,
    pub crop: CropRect,
    /// Zoom the lens/perspective-corrected frame so that no empty areas appear.
    pub constrain: bool,
    #[serde(flatten)]
    pub scalars: GeometryScalars,
}

impl Default for GeometryParams {
    fn default() -> Self {
        GeometryParams {
            orientation_delta: OrientationDelta::default(),
            aspect: Aspect::default(),
            crop: CropRect::default(),
            constrain: true,
            scalars: GeometryScalars::default(),
        }
    }
}

impl Scalars for GeometryParams {
    fn visit(&self, prefix: &str, f: &mut dyn FnMut(&str, &FieldSpec, f32)) {
        self.scalars.visit(prefix, f);
        self.crop.visit(&join(prefix, "crop"), f);
    }
    fn visit_mut(&mut self, prefix: &str, f: &mut dyn FnMut(&str, &FieldSpec, &mut f32)) {
        self.scalars.visit_mut(prefix, f);
        self.crop.visit_mut(&join(prefix, "crop"), f);
    }
}

// ---------------------------------------------------------------------------------------------
// input profile

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", tag = "kind", content = "data")]
pub enum CameraProfile {
    /// Standard colour matrix (LibRaw / DNG data). `None` until resolved and pinned.
    Matrix(Option<DataRef>),
    /// User-imported DCP.
    Dcp(DataRef),
}

impl Default for CameraProfile {
    fn default() -> Self {
        CameraProfile::Matrix(None)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct InputProfile {
    pub camera: CameraProfile,
    pub look: Option<DataRef>,
}

impl Scalars for InputProfile {
    fn visit(&self, _: &str, _: &mut dyn FnMut(&str, &FieldSpec, f32)) {}
    fn visit_mut(&mut self, _: &str, _: &mut dyn FnMut(&str, &FieldSpec, &mut f32)) {}
}

// ---------------------------------------------------------------------------------------------
// white balance

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WbMode {
    /// Use the camera's as-shot white exactly; `temperature`/`tint` are informational.
    #[default]
    AsShot,
    /// Values were produced by Auto WB (provenance only, doc 06 §3).
    Auto,
    Custom,
}

scalar_group! {
    pub struct WbValues {
        /// Scene illuminant correlated colour temperature.
        temperature: 5500.0, [2000.0, 50000.0], step 1.0, "K";
        /// Offset along the isotherm; positive is magenta.
        tint: 0.0, [-150.0, 150.0], step 1.0, "";
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct WhiteBalance {
    pub mode: WbMode,
    #[serde(flatten)]
    pub values: WbValues,
}

impl Scalars for WhiteBalance {
    fn visit(&self, prefix: &str, f: &mut dyn FnMut(&str, &FieldSpec, f32)) {
        self.values.visit(prefix, f);
    }
    fn visit_mut(&mut self, prefix: &str, f: &mut dyn FnMut(&str, &FieldSpec, &mut f32)) {
        self.values.visit_mut(prefix, f);
    }
}

// ---------------------------------------------------------------------------------------------
// light / presence

scalar_group! {
    pub struct LightParams {
        /// Exposure compensation.
        exposure: 0.0, [-5.0, 5.0], step 0.01, "EV";
        contrast: 0.0, [-100.0, 100.0], step 1.0, "";
        highlights: 0.0, [-100.0, 100.0], step 1.0, "";
        shadows: 0.0, [-100.0, 100.0], step 1.0, "";
        whites: 0.0, [-100.0, 100.0], step 1.0, "";
        blacks: 0.0, [-100.0, 100.0], step 1.0, "";
    }
}

scalar_group! {
    pub struct PresenceParams {
        texture: 0.0, [-100.0, 100.0], step 1.0, "";
        clarity: 0.0, [-100.0, 100.0], step 1.0, "";
        dehaze: 0.0, [-100.0, 100.0], step 1.0, "";
        vibrance: 0.0, [-100.0, 100.0], step 1.0, "";
        saturation: 0.0, [-100.0, 100.0], step 1.0, "";
    }
}

// ---------------------------------------------------------------------------------------------
// curves

scalar_group! {
    pub struct ParametricCurve {
        highlights: 0.0, [-100.0, 100.0], step 1.0, "";
        lights: 0.0, [-100.0, 100.0], step 1.0, "";
        darks: 0.0, [-100.0, 100.0], step 1.0, "";
        shadows: 0.0, [-100.0, 100.0], step 1.0, "";
    }
}

/// A control point of a point curve, both coordinates in [0, 1] of the rendered image.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CurvePoint {
    pub x: f32,
    pub y: f32,
}

/// Point curves. An empty list is the identity.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct PointCurves {
    pub rgb: Vec<CurvePoint>,
    pub red: Vec<CurvePoint>,
    pub green: Vec<CurvePoint>,
    pub blue: Vec<CurvePoint>,
}

impl PointCurves {
    pub fn all_mut(&mut self) -> [&mut Vec<CurvePoint>; 4] {
        [
            &mut self.rgb,
            &mut self.red,
            &mut self.green,
            &mut self.blue,
        ]
    }
    pub fn all(&self) -> [&Vec<CurvePoint>; 4] {
        [&self.rgb, &self.red, &self.green, &self.blue]
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct ToneCurve {
    pub parametric: ParametricCurve,
    pub point: PointCurves,
}

impl Scalars for ToneCurve {
    fn visit(&self, prefix: &str, f: &mut dyn FnMut(&str, &FieldSpec, f32)) {
        self.parametric.visit(&join(prefix, "parametric"), f);
    }
    fn visit_mut(&mut self, prefix: &str, f: &mut dyn FnMut(&str, &FieldSpec, &mut f32)) {
        self.parametric.visit_mut(&join(prefix, "parametric"), f);
    }
}

// ---------------------------------------------------------------------------------------------
// colour mixer / grading

scalar_group! {
    pub struct HslBand {
        hue: 0.0, [-100.0, 100.0], step 1.0, "";
        saturation: 0.0, [-100.0, 100.0], step 1.0, "";
        luminance: 0.0, [-100.0, 100.0], step 1.0, "";
    }
}

nested_group! {
    /// 8 hue bands × hue/saturation/luminance.
    pub struct ColorMixer {
        red: HslBand,
        orange: HslBand,
        yellow: HslBand,
        green: HslBand,
        aqua: HslBand,
        blue: HslBand,
        purple: HslBand,
        magenta: HslBand,
    }
}

scalar_group! {
    /// Black & white mix: lightness of colours of each hue in the monochrome conversion.
    pub struct BwMix {
        red: 0.0, [-100.0, 100.0], step 1.0, "";
        orange: 0.0, [-100.0, 100.0], step 1.0, "";
        yellow: 0.0, [-100.0, 100.0], step 1.0, "";
        green: 0.0, [-100.0, 100.0], step 1.0, "";
        aqua: 0.0, [-100.0, 100.0], step 1.0, "";
        blue: 0.0, [-100.0, 100.0], step 1.0, "";
        purple: 0.0, [-100.0, 100.0], step 1.0, "";
        magenta: 0.0, [-100.0, 100.0], step 1.0, "";
    }
}

impl BwMix {
    pub fn bands(&self) -> [f32; 8] {
        [
            self.red,
            self.orange,
            self.yellow,
            self.green,
            self.aqua,
            self.blue,
            self.purple,
            self.magenta,
        ]
    }
}

/// Black & white mode. When on, saturation, vibrance and the colour mixer are replaced by
/// the black & white mix; colour grading still applies (split toning).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct BwParams {
    pub monochrome: bool,
    #[serde(flatten)]
    pub mix: BwMix,
}

impl Scalars for BwParams {
    fn visit(&self, prefix: &str, f: &mut dyn FnMut(&str, &FieldSpec, f32)) {
        self.mix.visit(prefix, f);
    }
    fn visit_mut(&mut self, prefix: &str, f: &mut dyn FnMut(&str, &FieldSpec, &mut f32)) {
        self.mix.visit_mut(prefix, f);
    }
}

scalar_group! {
    pub struct Wheel {
        hue: 0.0, [0.0, 360.0], step 1.0, "deg";
        saturation: 0.0, [0.0, 100.0], step 1.0, "";
        luminance: 0.0, [-100.0, 100.0], step 1.0, "";
    }
}

scalar_group! {
    pub struct GradingBlend {
        blending: 50.0, [0.0, 100.0], step 1.0, "";
        balance: 0.0, [-100.0, 100.0], step 1.0, "";
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct ColorGrading {
    pub shadows: Wheel,
    pub midtones: Wheel,
    pub highlights: Wheel,
    pub global: Wheel,
    #[serde(flatten)]
    pub blend: GradingBlend,
}

impl Scalars for ColorGrading {
    fn visit(&self, prefix: &str, f: &mut dyn FnMut(&str, &FieldSpec, f32)) {
        self.shadows.visit(&join(prefix, "shadows"), f);
        self.midtones.visit(&join(prefix, "midtones"), f);
        self.highlights.visit(&join(prefix, "highlights"), f);
        self.global.visit(&join(prefix, "global"), f);
        self.blend.visit(prefix, f);
    }
    fn visit_mut(&mut self, prefix: &str, f: &mut dyn FnMut(&str, &FieldSpec, &mut f32)) {
        self.shadows.visit_mut(&join(prefix, "shadows"), f);
        self.midtones.visit_mut(&join(prefix, "midtones"), f);
        self.highlights.visit_mut(&join(prefix, "highlights"), f);
        self.global.visit_mut(&join(prefix, "global"), f);
        self.blend.visit_mut(prefix, f);
    }
}

// ---------------------------------------------------------------------------------------------
// detail / effects

scalar_group! {
    pub struct Sharpen {
        amount: 40.0, [0.0, 150.0], step 1.0, "";
        /// Radius in level-0 source pixels (doc 02 §8).
        radius: 1.0, [0.5, 3.0], step 0.1, "px";
        detail: 25.0, [0.0, 100.0], step 1.0, "";
        masking: 0.0, [0.0, 100.0], step 1.0, "";
    }
}

scalar_group! {
    pub struct NoiseReduction {
        luma: 0.0, [0.0, 100.0], step 1.0, "";
        detail: 50.0, [0.0, 100.0], step 1.0, "";
        contrast: 0.0, [0.0, 100.0], step 1.0, "";
        chroma: 25.0, [0.0, 100.0], step 1.0, "";
        chroma_detail: 50.0, [0.0, 100.0], step 1.0, "";
    }
}

scalar_group! {
    /// Removal of purple and green fringes (axial chromatic aberration) along high-contrast
    /// edges.
    pub struct Defringe {
        purple_amount: 0.0, [0.0, 100.0], step 1.0, "";
        /// Shift of the purple hue window, from magenta (−) to violet (+).
        purple_hue: 0.0, [-100.0, 100.0], step 1.0, "";
        green_amount: 0.0, [0.0, 100.0], step 1.0, "";
        /// Shift of the green hue window, from yellow-green (−) to cyan-green (+).
        green_hue: 0.0, [-100.0, 100.0], step 1.0, "";
    }
}

nested_group! {
    pub struct DetailParams {
        sharpen: Sharpen,
        noise: NoiseReduction,
        defringe: Defringe,
    }
}

scalar_group! {
    /// Post-crop vignette.
    pub struct Vignette {
        amount: 0.0, [-100.0, 100.0], step 1.0, "";
        midpoint: 50.0, [0.0, 100.0], step 1.0, "";
        roundness: 0.0, [-100.0, 100.0], step 1.0, "";
        feather: 50.0, [0.0, 100.0], step 1.0, "";
    }
}

scalar_group! {
    pub struct Grain {
        amount: 0.0, [0.0, 100.0], step 1.0, "";
        size: 25.0, [0.0, 100.0], step 1.0, "";
        roughness: 50.0, [0.0, 100.0], step 1.0, "";
    }
}

nested_group! {
    pub struct EffectsParams {
        vignette: Vignette,
        grain: Grain,
    }
}
