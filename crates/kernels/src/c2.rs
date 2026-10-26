//! C2: spatial tone and presence (doc 02 §3, §5.4), per-pixel part.
//!
//! The large-support part is precomputed by the host as a small *field* on a coarse source
//! pyramid level (see `aigle-engine` `pipeline::c2`): per coarse pixel,
//! `[a_tone, b_tone, a_clarity, b_clarity, transmission]`. The `a, b` pairs are the
//! coefficients of a self-guided filter on log2 luminance (fast guided filter, He & Sun 2015),
//! so that `base = a·log2(Y) + b` is evaluated with the full-resolution pixel as guide:
//! edge-aware, and with a cost per pixel independent of the radius.
//!
//! Order: dehaze → highlights/shadows (tone equaliser on the base) → clarity (local detail
//! around a midtone-weighted base). All in linear Rec.2020, scene-referred, unbounded.

use crate::math::{clamp01, exp2, log2, luminance};
use crate::resample::mirror;
use crate::{Rgb, math};

/// log2(0.18).
pub const MID_GREY_EV: f32 = -2.473_931_2;
pub const FIELD_CH: usize = 5;

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct C2Params {
    /// Non-zero when any C2 op is active.
    pub active: u32,
    /// −1..1.
    pub dehaze: f32,
    pub airlight: Rgb,
    /// −1..1 each.
    pub shadows: f32,
    pub highlights: f32,
    pub clarity: f32,
    /// Field coordinate = (sampled level coordinate + 0.5) · fscale − 0.5.
    pub fscale: f32,
}

impl C2Params {
    pub fn new(dehaze: f32, shadows: f32, highlights: f32, clarity: f32) -> C2Params {
        let active =
            (dehaze != 0.0 || shadows != 0.0 || highlights != 0.0 || clarity != 0.0) as u32;
        C2Params {
            active,
            dehaze: dehaze / 100.0,
            airlight: [1.0; 3],
            shadows: shadows / 100.0,
            highlights: highlights / 100.0,
            clarity: clarity / 100.0,
            fscale: 1.0,
        }
    }
}

/// Coarse field, `FIELD_CH` interleaved floats per pixel.
pub struct FieldView<'a> {
    pub data: &'a [f32],
    pub w: u32,
    pub h: u32,
}

impl FieldView<'_> {
    #[inline]
    fn load(&self, x: u32, y: u32) -> [f32; FIELD_CH] {
        let i = ((y * self.w + x) as usize) * FIELD_CH;
        [
            self.data[i],
            self.data[i + 1],
            self.data[i + 2],
            self.data[i + 3],
            self.data[i + 4],
        ]
    }

    /// Bilinear, mirror boundary; `u = 0` is the centre of column 0.
    #[inline]
    pub fn sample(&self, u: f32, v: f32) -> [f32; FIELD_CH] {
        let x0 = math::floor(u);
        let y0 = math::floor(v);
        let fx = u - x0;
        let fy = v - y0;
        let xa = mirror(x0 as i32, self.w);
        let xb = mirror(x0 as i32 + 1, self.w);
        let ya = mirror(y0 as i32, self.h);
        let yb = mirror(y0 as i32 + 1, self.h);
        let p00 = self.load(xa, ya);
        let p10 = self.load(xb, ya);
        let p01 = self.load(xa, yb);
        let p11 = self.load(xb, yb);
        let mut out = [0.0; FIELD_CH];
        for c in 0..FIELD_CH {
            let top = p00[c] + (p10[c] - p00[c]) * fx;
            let bot = p01[c] + (p11[c] - p01[c]) * fx;
            out[c] = top + (bot - top) * fy;
        }
        out
    }
}

/// Dehaze with transmission `t` (raw dark-channel estimate, 0..1) and airlight `A`.
#[inline]
pub fn dehaze_px(px: Rgb, t: f32, p: &C2Params) -> Rgb {
    if p.dehaze == 0.0 {
        return px;
    }
    let a = p.airlight;
    if p.dehaze > 0.0 {
        let t = (1.0 - p.dehaze * (1.0 - clamp01(t))).max(0.1);
        [
            ((px[0] - a[0]) / t + a[0]).max(0.0),
            ((px[1] - a[1]) / t + a[1]).max(0.0),
            ((px[2] - a[2]) / t + a[2]).max(0.0),
        ]
    } else {
        let h = -p.dehaze * 0.5;
        [
            px[0] + (a[0] - px[0]) * h,
            px[1] + (a[1] - px[1]) * h,
            px[2] + (a[2] - px[2]) * h,
        ]
    }
}

/// log2 luminance used by the tone and clarity fields.
#[inline]
pub fn log_lum(px: Rgb) -> f32 {
    log2(luminance(px).max(1.0 / 65536.0))
}

/// Shadow and highlight zone weights on a base in EV.
#[inline]
pub fn tone_weights(base: f32) -> (f32, f32) {
    let ws = 1.0 - math::smoothstep(MID_GREY_EV - 3.5, MID_GREY_EV + 0.5, base);
    let wh = math::smoothstep(MID_GREY_EV, MID_GREY_EV + 3.0, base);
    (ws, wh)
}

/// Full C2 for one pixel given its field sample.
#[inline]
pub fn c2_px(px: Rgb, f: [f32; FIELD_CH], p: &C2Params) -> Rgb {
    let v = dehaze_px(px, f[4], p);
    if p.shadows == 0.0 && p.highlights == 0.0 && p.clarity == 0.0 {
        return v;
    }
    let l = log_lum(v);
    let mut gain = 0.0;
    if p.shadows != 0.0 || p.highlights != 0.0 {
        let base = f[0] * l + f[1];
        let (ws, wh) = tone_weights(base);
        gain += 1.8 * (p.shadows * ws + p.highlights * wh);
    }
    if p.clarity != 0.0 {
        let base = f[2] * l + f[3];
        let d = (l - base).max(-2.0).min(2.0);
        let m = (base - MID_GREY_EV) / 3.0;
        let mid = exp2(-core::f32::consts::LOG2_E * m * m);
        gain += p.clarity * 0.6 * d * mid;
    }
    let k = exp2(gain);
    [v[0] * k, v[1] * k, v[2] * k]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_and_tone_direction() {
        let f = [1.0, 0.0, 1.0, 0.0, 1.0];
        let px = [0.01, 0.02, 0.03];
        assert_eq!(c2_px(px, f, &C2Params::new(0.0, 0.0, 0.0, 0.0)), px);
        // Flat field (a=0, b=base): shadows lift dark bases, highlights lower bright ones.
        let dark = [0.0, MID_GREY_EV - 4.0, 0.0, 0.0, 1.0];
        let lifted = c2_px(px, dark, &C2Params::new(0.0, 50.0, 0.0, 0.0));
        assert!(lifted[0] > px[0] * 1.5);
        let bright = [0.0, MID_GREY_EV + 4.0, 0.0, 0.0, 1.0];
        let lowered = c2_px([2.0; 3], bright, &C2Params::new(0.0, 0.0, -50.0, 0.0));
        assert!(lowered[0] < 1.5);
        // Clarity amplifies deviation from the base.
        let base_mid = [0.0, 0.0, 0.0, MID_GREY_EV, 1.0];
        let up = c2_px([0.36; 3], base_mid, &C2Params::new(0.0, 0.0, 0.0, 100.0));
        assert!(up[0] > 0.36);
    }

    #[test]
    fn dehaze_removes_airlight() {
        let mut p = C2Params::new(100.0, 0.0, 0.0, 0.0);
        p.airlight = [0.8; 3];
        // A hazy pixel halfway to the airlight with t = 0.5 recovers its radiance.
        let j = [0.2, 0.3, 0.1];
        let i = [0.5 * j[0] + 0.4, 0.5 * j[1] + 0.4, 0.5 * j[2] + 0.4];
        let out = dehaze_px(i, 0.5, &p);
        for c in 0..3 {
            assert!((out[c] - j[c]).abs() < 1e-5, "{out:?}");
        }
    }
}
