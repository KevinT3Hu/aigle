//! Host-side setup of the colour kernels (C1, E1).

use std::sync::Arc;

use aigle_core::EditParams;
use aigle_core::color::{self, Mat3};
use aigle_core::params::{CameraProfile, WbMode};
use aigle_engine_api::DisplayProfileId;
use aigle_kernels::Rgb;
use aigle_kernels::pointwise::{C1_BLOCK, C1_HSM, C1_LOOK, C1Params, D2Params, c1_color};

use crate::data::{self, FileKind};
use crate::profile::{self, CameraCalibration, DngProfile};
use crate::source::SourceColor;

/// Host side of C1 (doc 02 §6.2): the matrix and the optional colour block of DCP tables
/// (`aigle_kernels::pointwise::c1_color`).
#[derive(Clone, Debug, PartialEq)]
pub struct C1Color {
    pub params: C1Params,
    /// `[0.0]` (no tables), or the colour block followed by its tables.
    pub block: Vec<f32>,
    /// False when the edit references a DCP that cannot be found (rendered with the
    /// camera's own matrices).
    pub dcp_ok: bool,
}

impl C1Color {
    /// C1 for one pixel on the host.
    #[inline]
    pub fn apply(&self, px: Rgb) -> Rgb {
        c1_color(px, &self.params, &self.block, 0)
    }
}

/// The DCP selected by `profile.camera`, if any; the bool is false when it is referenced
/// but missing.
pub fn dcp(p: &EditParams) -> (Option<Arc<DngProfile>>, bool) {
    match &p.profile.camera {
        CameraProfile::Matrix(_) => (None, true),
        CameraProfile::Dcp(r) => match data::file(FileKind::Dcp, r, profile::parse) {
            Some(d) => (Some(d), true),
            None => {
                tracing::warn!(data_ref = %r, "DCP not found; rendering with the camera matrix");
                (None, false)
            }
        },
    }
}

/// Scene illuminant chromaticity selected by the WB params, under calibration `cal`.
pub fn scene_illuminant(src: &SourceColor, cal: &CameraCalibration, p: &EditParams) -> (f64, f64) {
    match p.wb.mode {
        WbMode::AsShot if std::ptr::eq(cal, &*src.calibration) => src.as_shot_xy,
        WbMode::AsShot => cal.neutral_to_xy(src.as_shot_neutral()),
        WbMode::Auto | WbMode::Custom => {
            color::cct_tint_to_xy(p.wb.values.temperature as f64, p.wb.values.tint as f64)
        }
    }
}

/// Camera multipliers (green = 1) that make the scene white of `p` neutral, under the
/// calibration C1 uses (A4 reconstructs in this space, doc 02 §7).
pub fn user_multipliers(src: &SourceColor, p: &EditParams) -> [f64; 3] {
    if p.wb.mode == WbMode::AsShot && matches!(p.profile.camera, CameraProfile::Matrix(_)) {
        return src.reference_wb;
    }
    let (dcp, _) = dcp(p);
    let cal: &CameraCalibration = dcp.as_ref().map_or(&src.calibration, |d| &d.calibration);
    let n = cal.neutral(scene_illuminant(src, cal, p));
    [1.0 / n[0], 1.0, 1.0 / n[2]]
}

/// ProPhoto RGB (ROMM, D50): the space of DCP HueSatMap / LookTable.
fn prophoto_to_xyz() -> Mat3 {
    color::primaries_to_xyz(
        (0.7347, 0.2653),
        (0.1596, 0.8404),
        (0.0366, 0.0001),
        profile::D50_XY,
    )
}

/// C1 (doc 02 §6.2). Without DCP tables, one matrix:
/// `XYZ→Rec2020 · camera→XYZ(D65, CAT16 from the scene white) · diag(1/reference WB) · 2^EV`.
/// With HueSatMap / LookTable the matrix stops in linear ProPhoto (D50) and the block does
/// HueSatMap → exposure → LookTable → Rec.2020.
pub fn c1(src: &SourceColor, p: &EditParams) -> C1Color {
    let (dcp, dcp_ok) = dcp(p);
    let cal: &CameraCalibration = dcp.as_ref().map_or(&src.calibration, |d| &d.calibration);
    let xy = scene_illuminant(src, cal, p);
    let undo_ref = color::diag(src.reference_wb.map(|m| 1.0 / m));
    let ev = p.light.exposure as f64
        + src.baseline_exposure
        + dcp.as_ref().map_or(0.0, |d| d.baseline_exposure_offset);
    let k = 2f64.powf(ev);
    let xyz_to_2020 = color::invert(&color::REC2020_TO_XYZ).expect("invertible");
    let hsm = dcp.as_ref().and_then(|d| d.hue_sat_at(xy));
    let look = dcp.as_ref().and_then(|d| d.look.clone());
    if hsm.is_none() && look.is_none() {
        let m = color::mul(
            &xyz_to_2020,
            &color::mul(&cal.camera_to_xyz(xy, color::D65_XY), &undo_ref),
        );
        return C1Color {
            params: C1Params {
                matrix: color::to_f32(&color::scale(&m, k)),
            },
            block: vec![0.0],
            dcp_ok,
        };
    }
    let pp = prophoto_to_xyz();
    let m = color::mul(
        &color::invert(&pp).expect("invertible"),
        &color::mul(&cal.camera_to_xyz(xy, profile::D50_XY), &undo_ref),
    );
    let d50 = color::xy_to_xyz(profile::D50_XY.0, profile::D50_XY.1);
    let d65 = color::xy_to_xyz(color::D65_XY.0, color::D65_XY.1);
    let m2 = color::mul(&xyz_to_2020, &color::mul(&color::cat16(d50, d65), &pp));
    let mut flags = 0;
    let mut block = vec![0.0; C1_BLOCK];
    block[1] = k as f32;
    for (i, v) in color::to_f32(&m2).iter().flatten().enumerate() {
        block[2 + i] = *v;
    }
    if let Some(h) = &hsm {
        flags |= C1_HSM;
        block[11] = block.len() as f32;
        h.pack(&mut block);
    }
    if let Some(l) = &look {
        flags |= C1_LOOK;
        block[12] = block.len() as f32;
        l.pack(&mut block);
    }
    block[0] = flags as f32;
    C1Color {
        params: C1Params {
            matrix: color::to_f32(&m),
        },
        block,
        dcp_ok,
    }
}

/// The user look (`profile.look`) packed for the output aux buffer, if set and found; the
/// bool is false when it is referenced but missing.
pub fn look(p: &EditParams) -> (Option<Vec<f32>>, bool) {
    let Some(r) = &p.profile.look else {
        return (None, true);
    };
    let parse = |b: &[u8]| crate::look::parse(&String::from_utf8_lossy(b));
    match data::file(FileKind::Look, r, parse) {
        Some(l) => {
            let mut v = Vec::new();
            l.pack(&mut v);
            (Some(v), true)
        }
        None => {
            tracing::warn!(data_ref = %r, "look LUT not found; rendering without it");
            (None, false)
        }
    }
}

/// E1 matrix: linear Rec.2020 → linear display/output primaries (doc 02 §6.5); see
/// [`crate::output_space`]. An unresolved ICC id falls back to sRGB.
pub fn e1_matrix(display: &DisplayProfileId) -> Mat3 {
    crate::output_space::e1_matrix(display)
}

// ---------------------------------------------------------------------------------------------
// D2 (doc 02 §6.4)

/// OkLab (Ottosson 2020): linear sRGB → LMS, and LMS^(1/3) → Lab.
const OKLAB_M1: Mat3 = [
    [0.412_221_470_8, 0.536_332_536_3, 0.051_445_992_9],
    [0.211_903_498_2, 0.680_699_545_1, 0.107_396_956_6],
    [0.088_302_461_9, 0.281_718_837_6, 0.629_978_700_5],
];
const OKLAB_M2: Mat3 = [
    [0.210_454_255_3, 0.793_617_785_0, -0.004_072_046_8],
    [1.977_998_495_1, -2.428_592_205_0, 0.450_593_709_9],
    [0.025_904_037_1, 0.782_771_766_2, -0.808_675_766_0],
];

/// Offset from the mixer/grading user hue (0 = red) to OkLab hue.
const HUE_OFFSET_DEG: f64 = 29.0;

/// D2 parameters from `EditParams`; `curve` tells whether curve tables are bound.
pub fn d2_params(p: &EditParams, curve: bool) -> D2Params {
    let to_lms = color::mul(
        &OKLAB_M1,
        &color::mul(
            &color::invert(&color::SRGB_TO_XYZ).expect("invertible"),
            &color::REC2020_TO_XYZ,
        ),
    );
    let lms_to_rgb = color::invert(&to_lms).expect("invertible");
    let lab_to_lms = color::invert(&OKLAB_M2).expect("invertible");
    let mx = &p.mixer;
    let bands = [
        &mx.red,
        &mx.orange,
        &mx.yellow,
        &mx.green,
        &mx.aqua,
        &mx.blue,
        &mx.purple,
        &mx.magenta,
    ];
    let mixer = bands.map(|b| {
        [
            b.hue / 100.0 * 30.0,
            b.saturation / 100.0,
            b.luminance / 100.0 * 0.5,
        ]
    });
    let mixer_on = bands
        .iter()
        .any(|b| b.hue != 0.0 || b.saturation != 0.0 || b.luminance != 0.0);
    let g = &p.grading;
    let wheel = |w: &aigle_core::params::Wheel| {
        let h = (w.hue as f64 + HUE_OFFSET_DEG).to_radians();
        let s = w.saturation as f64 / 100.0 * 0.06;
        [
            (s * h.cos()) as f32,
            (s * h.sin()) as f32,
            w.luminance / 100.0 * 0.1,
        ]
    };
    let wheels = [
        wheel(&g.shadows),
        wheel(&g.midtones),
        wheel(&g.highlights),
        wheel(&g.global),
    ];
    let pr = &p.presence;
    let bw = p.bw.monochrome;
    let color = bw
        || pr.saturation != 0.0
        || pr.vibrance != 0.0
        || mixer_on
        || wheels.iter().flatten().any(|v| *v != 0.0);
    D2Params {
        curve: curve as u32,
        color: color as u32,
        mixer_on: mixer_on as u32,
        look: 0,
        bw: bw as u32,
        bw_mix: p.bw.mix.bands().map(|v| v / 100.0),
        saturation: 1.0 + pr.saturation / 100.0,
        vibrance: pr.vibrance / 100.0,
        mixer,
        wheels,
        grade_mid: 0.55 - g.blend.balance / 100.0 * 0.15,
        grade_width: 0.05 + 0.3 * g.blend.blending / 100.0,
        to_lms: color::to_f32(&to_lms),
        lms_to_lab: color::to_f32(&OKLAB_M2),
        lab_to_lms: color::to_f32(&lab_to_lms),
        lms_to_rgb: color::to_f32(&lms_to_rgb),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn as_shot_working_space_is_identity() {
        let c = c1(&SourceColor::working_space(), &EditParams::default());
        assert_eq!(c.block, vec![0.0]);
        let m = c.params.matrix;
        for (i, row) in m.iter().enumerate() {
            for (j, v) in row.iter().enumerate() {
                assert!((v - if i == j { 1.0 } else { 0.0 }).abs() < 1e-6, "{m:?}");
            }
        }
    }

    #[test]
    fn lower_temperature_cools_the_image() {
        let mut p = EditParams::default();
        p.wb.mode = WbMode::Custom;
        p.wb.values.temperature = 3000.0;
        p.wb.values.tint = 0.0;
        let w = c1(&SourceColor::working_space(), &p).apply([1.0, 1.0, 1.0]);
        assert!(w[2] > w[0], "{w:?}");
    }
}
