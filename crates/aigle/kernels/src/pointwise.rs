//! Pointwise kernels: C1, D1, D2, E1 (doc 02 §3, §6).
//!
//! Each kernel has a `#[repr(C)]` params struct (host-side constructors may do
//! transcendental setup once per render) and a per-pixel function.

use crate::math::{
    atan2, cbrt, clamp01, cos, exp2, mat3_mul, powf, sin, smoothstep, sqrt, srgb_eotf, srgb_oetf,
};
use crate::{Mat3f, Rgb};

// ---------------------------------------------------------------------------------------------
// C1: input profile + WB + exposure, folded into one matrix (doc 02 §6.2).

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct C1Params {
    /// camera RGB (reference WB applied) → linear Rec.2020, exposure gain folded in.
    pub matrix: Mat3f,
}

#[inline]
pub fn c1(px: Rgb, p: &C1Params) -> Rgb {
    mat3_mul(&p.matrix, px)
}

/// C1 colour block (DCP tables, doc 02 §6.2 step 5) at `t[base]`:
/// `[flags, exposure, m2 (9), hsm_offset, look_offset]` with table offsets relative to `base`.
/// `flags == 0`: no block, [`C1Params::matrix`] is the whole of C1. Otherwise the matrix maps
/// to linear ProPhoto without exposure; then HueSatMap ([`C1_HSM`]), exposure, LookTable
/// ([`C1_LOOK`]) and `m2` (ProPhoto → Rec.2020).
pub const C1_BLOCK: usize = 13;
pub const C1_HSM: u32 = 1;
pub const C1_LOOK: u32 = 2;

#[inline]
pub fn c1_color(px: Rgb, p: &C1Params, t: &[f32], base: usize) -> Rgb {
    let v = mat3_mul(&p.matrix, px);
    let flags = t[base] as u32;
    if flags == 0 {
        return v;
    }
    let mut v = v;
    if flags & C1_HSM != 0 {
        v = crate::luts::hsv_table(v, t, base + t[base + 11] as usize);
    }
    let k = t[base + 1];
    v = [v[0] * k, v[1] * k, v[2] * k];
    if flags & C1_LOOK != 0 {
        v = crate::luts::hsv_table(v, t, base + t[base + 12] as usize);
    }
    let o = base + 2;
    let m2 = [
        [t[o], t[o + 1], t[o + 2]],
        [t[o + 3], t[o + 4], t[o + 5]],
        [t[o + 6], t[o + 7], t[o + 8]],
    ];
    mat3_mul(&m2, v)
}

// ---------------------------------------------------------------------------------------------
// D1: rendering transform (doc 02 §6.3).
//
// 1. Gamut compression ([`gamut_compress`]): scene values outside Rec.2020 (negative
//    components from camera matrices on saturated lights) are pulled in smoothly along lines
//    of constant max(R,G,B), instead of clipping the negative component.
// 2. Log-logistic sigmoid on the norm max(R,G,B), anchored so scene mid-grey (0.18) maps to
//    display-linear mid-grey (0.18). `contrast` scales the slope, `whites` the asymptote,
//    `blacks` lifts/crushes the toe.
// 3. Hue-preserving below the shoulder (channel ratios kept), with a path to white: ratios
//    blend towards 1 as the mapped norm approaches the asymptote ([`PATH_TO_WHITE`]), so
//    over-exposed saturated colours brighten to white instead of staying flat and saturated.
//
// E1 then fits the result into the output gamut ([`fit_output`]).
//
// TODO(process version 1): not frozen yet; the curve, the compressor parameters and the
// path-to-white exponent become part of process version 1 at the first release.

pub const MID_GREY: f32 = 0.18;

/// Exponent of the path to white: ratios move `(mapped / white)^PATH_TO_WHITE` of the way to
/// neutral (0.4 % at half the asymptote, 17 % at 80 %, 43 % at 90 %, 66 % at 95 %).
/// Exponent 4 was tried and made +2.5 EV oranges and blues visibly pastel.
pub const PATH_TO_WHITE: f32 = 8.0;

/// Reference gamut compression (ACES 1.3 RGC: thresholds, limits, power), per channel.
const RGC_THRESHOLD: Rgb = [0.815, 0.803, 0.880];
const RGC_LIMIT: Rgb = [1.147, 1.264, 1.312];
const RGC_POWER: f32 = 1.2;

/// Compress one distance `d` (0 = achromatic, 1 = on the gamut boundary) so that `lim` maps
/// to 1; identity below `thr`.
#[inline]
fn rgc_distance(d: f32, thr: f32, lim: f32) -> f32 {
    if d < thr {
        return d;
    }
    let p = RGC_POWER;
    let scale = (lim - thr) / powf(powf((1.0 - thr) / (lim - thr), -p) - 1.0, 1.0 / p);
    let x = (d - thr) / scale;
    thr + scale * x / powf(1.0 + powf(x, p), 1.0 / p)
}

/// Distance-based gamut compression (ACES RGC) towards the achromatic axis at constant
/// max(R,G,B): components below 0 come in smoothly; colours well inside are unchanged.
#[inline]
pub fn gamut_compress(px: Rgb) -> Rgb {
    let ac = px[0].max(px[1]).max(px[2]);
    if ac <= 1e-10 {
        return [0.0; 3];
    }
    let mut out = [0.0; 3];
    for c in 0..3 {
        let d = (ac - px[c]) / ac;
        out[c] = if d < RGC_THRESHOLD[c] {
            px[c]
        } else {
            ac - rgc_distance(d, RGC_THRESHOLD[c], RGC_LIMIT[c]) * ac
        };
    }
    out
}

/// Fit linear output-primary values `l` (whose Rec.2020 luminance is `y`) into [0, 1]:
/// gamut compression for negatives, then, if a component still exceeds 1, a blend towards
/// the grey of the same luminance just far enough to bring it to 1 (hue and luminance kept;
/// white if `y ≥ 1`). Values inside the gamut are unchanged.
#[inline]
pub fn fit_output(l: Rgb, y: f32) -> Rgb {
    let c = gamut_compress(l);
    let mx = c[0].max(c[1]).max(c[2]);
    if mx <= 1.0 {
        return [clamp01(c[0]), clamp01(c[1]), clamp01(c[2])];
    }
    if y >= 1.0 {
        return [1.0; 3];
    }
    let y = y.max(0.0);
    let t = (mx - 1.0) / (mx - y);
    [
        clamp01(c[0] + (y - c[0]) * t),
        clamp01(c[1] + (y - c[1]) * t),
        clamp01(c[2] + (y - c[2]) * t),
    ]
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct D1Params {
    pub slope: f32,
    /// `k^slope` of `x^a / (x^a + k^a)`.
    pub k_pow: f32,
    pub white: f32,
    pub blacks: f32,
}

impl D1Params {
    /// Slider values in user units (−100..100).
    pub fn new(contrast: f32, whites: f32, blacks: f32) -> D1Params {
        let slope = 1.5 * exp2(contrast / 100.0);
        let white = 1.0 + 0.5 * (whites / 100.0);
        // L · g^a / (g^a + k^a) = g  ⇒  k^a = g^a (L/g − 1)
        let k_pow = powf(MID_GREY, slope) * (white / MID_GREY - 1.0);
        D1Params {
            slope,
            k_pow,
            white,
            blacks: blacks / 100.0,
        }
    }
}

#[inline]
pub fn d1(px: Rgb, p: &D1Params) -> Rgb {
    let px = gamut_compress(px);
    let n = px[0].max(px[1]).max(px[2]);
    let mapped = if n > 1e-10 {
        let xa = powf(n, p.slope);
        p.white * xa / (xa + p.k_pow)
    } else {
        0.0
    };
    let q = mapped / p.white;
    let q2 = q * q;
    let q4 = q2 * q2;
    let toward_white = q4 * q4; // q^PATH_TO_WHITE
    let mut out = [0.0; 3];
    for c in 0..3 {
        let ratio = if n > 1e-10 { (px[c] / n).max(0.0) } else { 0.0 };
        let ratio = ratio + (1.0 - ratio) * toward_white;
        let y = mapped * ratio;
        let t = 1.0 - clamp01(y);
        let toe = p.blacks * 0.05 * (t * t) * (t * t);
        out[c] = clamp01(y + toe);
    }
    out
}

// ---------------------------------------------------------------------------------------------
// D2: creative colour, fused (doc 02 §6.4), on the bounded display-referred D1 output.
//
// 1. Tone curves: one 4096-entry table per channel (parametric ∘ point-rgb ∘ point-channel,
//    built by the host), indexed and valued in the sRGB encoding so that curves match what the
//    user sees in the histogram.
// 2. In OkLab: vibrance/saturation on chroma, colour mixer (8 hue bands, smooth periodic
//    weights), colour grading (shadows/midtones/highlights/global wheels by lightness).
//
// 0. Look LUT (`profile.look`, `.cube`, tetrahedral; `luts::cube`), first, so that the
//    user's D2 edits act on top of the look. It lives in the aux buffer at `look`.

pub const CURVE_N: usize = 4096;
/// The curve tables follow the blue-noise texture in the output kernel's aux buffer.
pub const CURVE_OFFSET: usize = crate::dither::BLUE_LEN;

/// OkLab hue (degrees) of the eight mixer band centres: red, orange, yellow, green, aqua,
/// blue, purple, magenta (the hues of the sRGB primaries/secondaries, orange in between).
pub const MIXER_HUES: [f32; 8] = [29.0, 65.0, 110.0, 142.0, 195.0, 264.0, 305.0, 328.0];

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct D2Params {
    /// Non-zero: apply the curve tables.
    pub curve: u32,
    /// Non-zero: run the OkLab colour block.
    pub color: u32,
    /// Non-zero: some mixer band is set.
    pub mixer_on: u32,
    /// Offset of the look cube in the aux buffer; 0 = none.
    pub look: u32,
    /// Non-zero: black & white; replaces saturation, vibrance and the mixer by `bw_mix`.
    pub bw: u32,
    /// Black & white: per mixer band, lightness EV of colours of that hue.
    pub bw_mix: [f32; 8],
    /// Chroma gain.
    pub saturation: f32,
    /// Extra chroma gain for low-chroma colours.
    pub vibrance: f32,
    /// Per band: hue shift (deg), chroma delta, lightness EV.
    pub mixer: [[f32; 3]; 8],
    /// Shadows, midtones, highlights, global: Δa, Δb, ΔL.
    pub wheels: [[f32; 3]; 4],
    /// Lightness of the shadows/highlights crossover and its half width.
    pub grade_mid: f32,
    pub grade_width: f32,
    /// linear Rec.2020 → LMS, LMS^(1/3) → Lab and the inverses (OkLab, Ottosson 2020).
    pub to_lms: Mat3f,
    pub lms_to_lab: Mat3f,
    pub lab_to_lms: Mat3f,
    pub lms_to_rgb: Mat3f,
}

impl D2Params {
    pub fn is_identity(&self) -> bool {
        self.curve == 0 && self.color == 0 && self.look == 0
    }
}

/// Sample curve channel `c` at encoded value `v` in [0, 1], linear interpolation.
#[inline]
pub fn curve_sample(lut: &[f32], c: usize, v: f32) -> f32 {
    let f = clamp01(v) * (CURVE_N - 1) as f32;
    let i = f as u32;
    let i = if i as usize >= CURVE_N - 1 {
        CURVE_N as u32 - 2
    } else {
        i
    };
    let t = f - i as f32;
    let o = CURVE_OFFSET + c * CURVE_N + i as usize;
    lut[o] + (lut[o + 1] - lut[o]) * t
}

#[inline]
fn curve_ch(lut: &[f32], c: usize, x: f32) -> f32 {
    srgb_eotf(clamp01(curve_sample(lut, c, srgb_oetf(clamp01(x)))))
}

/// Mixer adjustment at OkLab hue `h` (degrees, 0..360): weighted blend of the two
/// neighbouring bands with a raised-cosine partition of unity.
#[inline]
pub fn mixer_at(h: f32, m: &[[f32; 3]; 8]) -> [f32; 3] {
    let (i, j, w0) = band_weights(h);
    let mut out = [0.0; 3];
    for k in 0..3 {
        out[k] = w0 * m[i][k] + (1.0 - w0) * m[j][k];
    }
    out
}

/// The two mixer bands around OkLab hue `h` (degrees, 0..360) and the weight of the first.
#[inline]
fn band_weights(h: f32) -> (usize, usize, f32) {
    let hh = if h < MIXER_HUES[0] { h + 360.0 } else { h };
    let mut out = (7, 0, 1.0);
    for i in 0..8 {
        let c0 = MIXER_HUES[i];
        let c1 = if i == 7 {
            MIXER_HUES[0] + 360.0
        } else {
            MIXER_HUES[i + 1]
        };
        if hh >= c0 && hh < c1 {
            let t = (hh - c0) / (c1 - c0);
            let w0 = 0.5 * (1.0 + cos(core::f32::consts::PI * t));
            out = (i, if i == 7 { 0 } else { i + 1 }, w0);
        }
    }
    out
}

#[inline]
fn hue_deg(a: f32, b: f32) -> f32 {
    let h = atan2(b, a) * (180.0 / core::f32::consts::PI);
    if h < 0.0 { h + 360.0 } else { h }
}

#[inline]
fn color_px(v: Rgb, p: &D2Params, dsat: f32, dhue: f32) -> Rgb {
    let lms = mat3_mul(&p.to_lms, v);
    let lab = mat3_mul(&p.lms_to_lab, [cbrt(lms[0]), cbrt(lms[1]), cbrt(lms[2])]);
    let (mut l, mut a, mut b) = (lab[0], lab[1], lab[2]);
    let c = sqrt(a * a + b * b);
    if p.bw != 0 {
        if c > 1e-5 {
            let (i, j, w0) = band_weights(hue_deg(a, b));
            let ev = w0 * p.bw_mix[i] + (1.0 - w0) * p.bw_mix[j];
            l *= exp2(ev * smoothstep(0.0, 0.12, c));
        }
        return grade(l, 0.0, 0.0, p);
    }
    let g = ((p.saturation + dsat) * (1.0 + p.vibrance * (1.0 - smoothstep(0.0, 0.2, c)))).max(0.0);
    a *= g;
    b *= g;
    let c = c * g;
    if dhue != 0.0 {
        let ang = dhue * (core::f32::consts::PI / 180.0);
        let (ca, sa) = (cos(ang), sin(ang));
        let (na, nb) = (a * ca - b * sa, a * sa + b * ca);
        a = na;
        b = nb;
    }
    if p.mixer_on != 0 && c > 1e-5 {
        let m = mixer_at(hue_deg(a, b), &p.mixer);
        let ang = m[0] * (core::f32::consts::PI / 180.0);
        let (ca, sa) = (cos(ang), sin(ang));
        let s = (1.0 + m[1]).max(0.0);
        let (na, nb) = (a * ca - b * sa, a * sa + b * ca);
        a = na * s;
        b = nb * s;
        l *= exp2(m[2] * smoothstep(0.02, 0.12, c));
    }
    grade(l, a, b, p)
}

/// Colour grading wheels by lightness, then OkLab → linear RGB.
#[inline]
fn grade(mut l: f32, mut a: f32, mut b: f32, p: &D2Params) -> Rgb {
    let wh = smoothstep(p.grade_mid - p.grade_width, p.grade_mid + p.grade_width, l);
    let ws = 1.0 - wh;
    let wm = 4.0 * ws * wh;
    let w = &p.wheels;
    a += ws * w[0][0] + wm * w[1][0] + wh * w[2][0] + w[3][0];
    b += ws * w[0][1] + wm * w[1][1] + wh * w[2][1] + w[3][1];
    l += ws * w[0][2] + wm * w[1][2] + wh * w[2][2] + w[3][2];
    let lp = mat3_mul(&p.lab_to_lms, [l, a, b]);
    let lms = [
        lp[0] * lp[0] * lp[0],
        lp[1] * lp[1] * lp[1],
        lp[2] * lp[2] * lp[2],
    ];
    mat3_mul(&p.lms_to_rgb, lms)
}

#[inline]
pub fn d2(px: Rgb, p: &D2Params, lut: &[f32]) -> Rgb {
    d2_local(px, p, lut, 0.0, 0.0)
}

/// D2 with local saturation (gain delta) and hue (degrees) fields (doc 02 §9).
#[inline]
pub fn d2_local(px: Rgb, p: &D2Params, lut: &[f32], dsat: f32, dhue: f32) -> Rgb {
    let mut v = px;
    if p.look != 0 {
        v = crate::luts::cube(v, lut, p.look as usize);
        v = [clamp01(v[0]), clamp01(v[1]), clamp01(v[2])];
    }
    if p.curve != 0 {
        v = [
            curve_ch(lut, 0, v[0]),
            curve_ch(lut, 1, v[1]),
            curve_ch(lut, 2, v[2]),
        ];
    }
    if p.color != 0 || dsat != 0.0 || dhue != 0.0 {
        v = color_px(v, p, dsat, dhue);
    }
    [clamp01(v[0]), clamp01(v[1]), clamp01(v[2])]
}

// ---------------------------------------------------------------------------------------------
// E1: output transform (doc 02 §6.5). Matrix/TRC profiles are exact; this covers sRGB.

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct E1Params {
    /// linear Rec.2020 → linear output primaries.
    pub matrix: Mat3f,
    /// Output transfer function, `math::TRANSFER_*`.
    pub transfer: u32,
    /// [`TRANSFER_TABLE`](crate::math::TRANSFER_TABLE): offset of the tables in the aux
    /// buffer.
    pub table: u32,
    /// Soft proof: offset in the aux buffer of two matrices, linear Rec.2020 → linear proof
    /// RGB and linear proof RGB → linear output primaries, replacing `matrix` so that colours
    /// are fitted to the proof gamut; 0 = off. (In the aux buffer rather than here to keep
    /// the output kernel's register use down.)
    pub proof: u32,
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::math::luminance;

    #[test]
    fn d1_anchors_grey_and_is_bounded_and_monotone() {
        for contrast in [-100.0, 0.0, 100.0] {
            let p = D1Params::new(contrast, 0.0, 0.0);
            let g = d1([MID_GREY; 3], &p);
            assert!((g[0] - MID_GREY).abs() < 1e-5, "contrast {contrast}: {g:?}");
            let mut prev = -1.0;
            for i in 0..200 {
                let x = 1e-4 * libm::powf(1.1, i as f32);
                let y = d1([x; 3], &p)[0];
                assert!((0.0..=1.0).contains(&y));
                assert!(y >= prev);
                prev = y;
            }
        }
        // Hue preservation: channel ratios are (nearly) kept below the shoulder.
        let p = D1Params::new(0.0, 0.0, 0.0);
        let o = d1([0.2, 0.1, 0.05], &p);
        assert!((o[1] / o[0] - 0.5).abs() < 2e-3, "{o:?}");
    }

    #[test]
    fn saturated_highlights_go_to_white() {
        let p = D1Params::new(0.0, 0.0, 0.0);
        let sat = |v: Rgb| (v[0] - v[2]) / v[0];
        // The same orange at increasing exposure desaturates towards white.
        let mut prev = 1.0;
        for ev in [0.0f32, 2.0, 4.0, 6.0, 8.0] {
            let k = libm::exp2f(ev);
            let o = d1([0.2 * k, 0.1 * k, 0.02 * k], &p);
            assert!(sat(o) < prev + 1e-6, "{ev}: {o:?}");
            prev = sat(o);
        }
        assert!(prev < 0.05, "{prev}");
        // Mid-tones keep their saturation.
        let o = d1([0.2, 0.1, 0.02], &p);
        assert!(sat(o) > 0.85, "{o:?}");
    }

    #[test]
    fn gamut_compression_is_smooth_and_identity_inside() {
        let v = [0.5, 0.4, 0.3];
        assert_eq!(gamut_compress(v), v);
        // Decreasing input blue gives decreasing (continuous) output blue, the max is kept,
        // and everything down to the limit lands inside the gamut.
        let mut prev = 0.21;
        for i in 0..=46 {
            let b = 0.2 - 0.01 * i as f32;
            let o = gamut_compress([1.0, 0.5, b]);
            assert_eq!(o[0], 1.0);
            assert!(o[2] <= prev + 1e-6 && prev - o[2] < 0.011, "{b}: {o:?}");
            if b >= 1.0 - RGC_LIMIT[2] {
                assert!(o[2] >= -1e-5, "{b}: {o:?}");
            }
            prev = o[2];
        }
        let at_lim = gamut_compress([1.0, 0.5, 1.0 - RGC_LIMIT[2]]);
        assert!(at_lim[2].abs() < 1e-4, "{at_lim:?}");
    }

    #[test]
    fn fit_output_keeps_luminance_and_bounds() {
        // Over 1 in one channel: pulled to 1 at the same luminance.
        let l = [1.4, 0.6, 0.2];
        let y = luminance(l);
        let o = fit_output(l, y);
        let yo = luminance(o);
        assert!((o[0] - 1.0).abs() < 1e-5 && (yo - y).abs() < 1e-4, "{o:?}");
        assert!(o[0] > o[1] && o[1] > o[2]);
        // In gamut: unchanged.
        assert_eq!(fit_output([0.5, 0.3, 0.1], 0.3), [0.5, 0.3, 0.1]);
        assert_eq!(fit_output([2.0, 2.0, 2.0], 2.0), [1.0; 3]);
    }

    /// OkLab matrices from Ottosson (linear sRGB); good enough for kernel-level tests.
    #[allow(clippy::excessive_precision)]
    pub(crate) fn test_d2(color: bool) -> D2Params {
        let to_lms = [
            [0.4122214708, 0.5363325363, 0.0514459929],
            [0.2119034982, 0.6806995451, 0.1073969566],
            [0.0883024619, 0.2817188376, 0.6299787005],
        ];
        let lms_to_lab = [
            [0.2104542553, 0.7936177850, -0.0040720468],
            [1.9779984951, -2.4285922050, 0.4505937099],
            [0.0259040371, 0.7827717662, -0.8086757660],
        ];
        let lab_to_lms = [
            [1.0, 0.3963377774, 0.2158037573],
            [1.0, -0.1055613458, -0.0638541728],
            [1.0, -0.0894841775, -1.2914855480],
        ];
        let lms_to_rgb = [
            [4.0767416621, -3.3077115913, 0.2309699292],
            [-1.2684380046, 2.6097574011, -0.3413193965],
            [-0.0041960863, -0.7034186147, 1.7076147010],
        ];
        D2Params {
            curve: 0,
            color: color as u32,
            mixer_on: 0,
            look: 0,
            bw: 0,
            bw_mix: [0.0; 8],
            saturation: 1.0,
            vibrance: 0.0,
            mixer: [[0.0; 3]; 8],
            wheels: [[0.0; 3]; 4],
            grade_mid: 0.55,
            grade_width: 0.2,
            to_lms,
            lms_to_lab,
            lab_to_lms,
            lms_to_rgb,
        }
    }

    #[test]
    fn d2_neutral_is_identity_and_greys_stay_grey() {
        let lut: Vec<f32> = (0..CURVE_OFFSET + 3 * CURVE_N)
            .map(|i| (i.saturating_sub(CURVE_OFFSET) % CURVE_N) as f32 / (CURVE_N - 1) as f32)
            .collect();
        let px = [0.3, 0.5, 0.7];
        // OkLab round trip and an identity curve table are both near-exact.
        let mut p = test_d2(true);
        p.curve = 1;
        let o = d2(px, &p, &lut);
        for c in 0..3 {
            assert!((o[c] - px[c]).abs() < 2e-4, "{o:?}");
        }
        p.saturation = 1.8;
        p.vibrance = 0.5;
        p.mixer_on = 1;
        p.mixer[5] = [20.0, -0.5, 0.3];
        let g = d2([0.4; 3], &p, &lut);
        assert!(
            (g[0] - g[1]).abs() < 1e-4 && (g[2] - g[1]).abs() < 1e-4,
            "{g:?}"
        );
    }

    #[test]
    fn bw_is_grey_and_the_mix_moves_lightness_by_hue() {
        let lut = vec![0.0; CURVE_OFFSET];
        let mut p = test_d2(true);
        p.bw = 1;
        let red = [0.6, 0.1, 0.08];
        let blue = [0.08, 0.12, 0.6];
        let grey = |v: Rgb| (v[0] - v[1]).abs() < 1e-4 && (v[2] - v[1]).abs() < 1e-4;
        let base_r = d2(red, &p, &lut);
        let base_b = d2(blue, &p, &lut);
        assert!(grey(base_r) && grey(base_b), "{base_r:?} {base_b:?}");
        // Neutral input keeps its value.
        let g = d2([0.4; 3], &p, &lut);
        assert!((g[0] - 0.4).abs() < 2e-4, "{g:?}");
        // Brightening reds leaves blues alone.
        p.bw_mix[0] = 1.0;
        let r = d2(red, &p, &lut);
        let b = d2(blue, &p, &lut);
        assert!(grey(r) && r[0] > base_r[0] * 1.5, "{r:?} {base_r:?}");
        assert!((b[0] - base_b[0]).abs() < 1e-4, "{b:?} {base_b:?}");
        // Grading still tints the monochrome image.
        p.wheels[3] = [0.02, 0.03, 0.0];
        let t = d2([0.4; 3], &p, &lut);
        assert!(t[0] > t[2], "{t:?}");
    }

    #[test]
    fn mixer_weights_are_a_partition_of_unity() {
        let mut m = [[0.0; 3]; 8];
        for b in m.iter_mut() {
            b[1] = 1.0;
        }
        for h in 0..360 {
            let w = mixer_at(h as f32, &m);
            assert!((w[1] - 1.0).abs() < 1e-5, "{h}: {w:?}");
        }
        let mut m = [[0.0; 3]; 8];
        m[5][1] = 1.0; // blue band
        assert!((mixer_at(264.0, &m)[1] - 1.0).abs() < 1e-6);
        assert_eq!(mixer_at(29.0, &m)[1], 0.0);
    }

    #[test]
    fn saturation_and_grading_move_colour() {
        let lut = [0.0f32; 1];
        let mut p = test_d2(true);
        p.saturation = 0.0;
        let grey = d2([0.6, 0.3, 0.1], &p, &lut);
        assert!((grey[0] - grey[2]).abs() < 1e-3, "{grey:?}");
        let mut p = test_d2(true);
        p.wheels[0] = [0.0, -0.05, 0.0]; // shadows towards blue (−b)
        let o = d2([0.02; 3], &p, &lut);
        assert!(o[2] > o[0], "{o:?}");
    }
}
