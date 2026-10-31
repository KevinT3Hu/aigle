//! Tables that are colour data by definition (doc 02 §6.2 step 5, §6.4).
//!
//! * [`hsv_table`]: DNG / DCP HueSatMap and LookTable: hue shift, saturation scale and value
//!   scale indexed by HSV, interpolated as in the DNG SDK reference code
//!   (`RefBaselineHueSatMap`): bilinear in hue × saturation, trilinear when there are value
//!   divisions; hue wraps. The input is linear ProPhoto RGB.
//! * [`cube`]: a user look `.cube` 3D LUT, tetrahedral, through an sRGB shaper on sRGB
//!   primaries.
//!
//! Tables live in an `f32` slice at an offset, header first, so that the same code reads them
//! from host memory and from a GPU storage buffer.
//!
//! Scene-referred input (doc 02 §6.1): the DNG reference pins value to [0, 1]. Here the value
//! scale is applied as a ratio evaluated at `min(v, 1)`, so values above 1 keep their
//! highlight headroom and the result is continuous at 1.

use crate::Rgb;
use crate::math::{clamp01, floor, mat3_mul, srgb_eotf, srgb_oetf};

/// HSV table header: `[hue_div, sat_div, val_div, srgb_value]`, then
/// `val_div · hue_div · sat_div` entries of `[hue_shift_deg, sat_scale, val_scale]`, value
/// outermost, saturation innermost (DNG spec order).
pub const HSV_HEADER: usize = 4;

/// RGB → (h ∈ [0, 6), s, v), DNG SDK convention.
#[inline]
fn rgb_to_hsv(c: Rgb) -> (f32, f32, f32) {
    let v = c[0].max(c[1]).max(c[2]);
    let gap = v - c[0].min(c[1]).min(c[2]);
    if gap <= 0.0 || v <= 0.0 {
        return (0.0, 0.0, v);
    }
    let mut h = if c[0] == v {
        let h = (c[1] - c[2]) / gap;
        if h < 0.0 { h + 6.0 } else { h }
    } else if c[1] == v {
        2.0 + (c[2] - c[0]) / gap
    } else {
        4.0 + (c[0] - c[1]) / gap
    };
    if h >= 6.0 {
        h -= 6.0;
    }
    (h, gap / v, v)
}

#[inline]
fn hsv_to_rgb(h: f32, s: f32, v: f32) -> Rgb {
    if s <= 0.0 {
        return [v, v, v];
    }
    let mut h = h;
    while h < 0.0 {
        h += 6.0;
    }
    while h >= 6.0 {
        h -= 6.0;
    }
    let i = floor(h);
    let f = h - i;
    let p = v * (1.0 - s);
    let q = v * (1.0 - s * f);
    let t = v * (1.0 - s * (1.0 - f));
    let i = i as u32;
    if i == 0 {
        [v, t, p]
    } else if i == 1 {
        [q, v, p]
    } else if i == 2 {
        [p, v, t]
    } else if i == 3 {
        [p, q, v]
    } else if i == 4 {
        [t, p, v]
    } else {
        [v, p, q]
    }
}

/// Table entry `k` at (value, hue, saturation) indices; `dims = [hue_div, sat_div]`.
#[inline]
#[allow(clippy::too_many_arguments)]
fn hsv_at(t: &[f32], data: usize, dims: [u32; 2], vi: u32, hi: u32, si: u32, k: usize) -> f32 {
    t[data + (((vi * dims[0] + hi) * dims[1] + si) as usize) * 3 + k]
}

/// Interpolated `[hue_shift_deg, sat_scale, val_scale]` at (h, s, encoded v).
#[inline]
fn hsv_lookup(t: &[f32], base: usize, h: f32, s: f32, ve: f32) -> [f32; 3] {
    let hd = t[base] as u32;
    let sd = t[base + 1] as u32;
    let vd = t[base + 2] as u32;
    let data = base + HSV_HEADER;
    let hs = if hd < 2 { 0.0 } else { h * (hd as f32 / 6.0) };
    let ss = clamp01(s) * (sd - 1) as f32;
    let mut h0 = floor(hs) as u32;
    let mut s0 = floor(ss) as u32;
    if s0 + 2 > sd {
        // `saturating_sub` does not compile with rust-gpu.
        #[allow(clippy::implicit_saturating_sub)]
        {
            s0 = if sd >= 2 { sd - 2 } else { 0 };
        }
    }
    let mut h1 = h0 + 1;
    if h0 + 1 >= hd {
        h0 = hd - 1;
        h1 = 0;
    }
    let hf = hs - h0 as f32;
    let sf = if sd >= 2 { ss - s0 as f32 } else { 0.0 };
    let s1 = if sd >= 2 { s0 + 1 } else { s0 };
    let (v0, v1, vf) = if vd < 2 {
        (0u32, 0u32, 0.0f32)
    } else {
        let vs = clamp01(ve) * (vd - 1) as f32;
        let mut v0 = floor(vs) as u32;
        if v0 + 2 > vd {
            v0 = vd - 2;
        }
        (v0, v0 + 1, vs - v0 as f32)
    };
    let dims = [hd, sd];
    let mut out = [0f32; 3];
    let mut k = 0;
    while k < 3 {
        let a = (hsv_at(t, data, dims, v0, h0, s0, k) * (1.0 - hf)
            + hsv_at(t, data, dims, v0, h1, s0, k) * hf)
            * (1.0 - sf)
            + (hsv_at(t, data, dims, v0, h0, s1, k) * (1.0 - hf)
                + hsv_at(t, data, dims, v0, h1, s1, k) * hf)
                * sf;
        let b = (hsv_at(t, data, dims, v1, h0, s0, k) * (1.0 - hf)
            + hsv_at(t, data, dims, v1, h1, s0, k) * hf)
            * (1.0 - sf)
            + (hsv_at(t, data, dims, v1, h0, s1, k) * (1.0 - hf)
                + hsv_at(t, data, dims, v1, h1, s1, k) * hf)
                * sf;
        out[k] = a * (1.0 - vf) + b * vf;
        k += 1;
    }
    out
}

/// Apply an HSV table (header at `t[base]`) to linear ProPhoto RGB.
#[inline]
pub fn hsv_table(c: Rgb, t: &[f32], base: usize) -> Rgb {
    let (h, s, v) = rgb_to_hsv(c);
    if v <= 0.0 {
        return c;
    }
    let srgb = t[base + 3] != 0.0;
    let vc = v.min(1.0);
    let ve = if srgb { srgb_oetf(vc) } else { vc };
    let d = hsv_lookup(t, base, h, s, ve);
    let h = h + d[0] * (6.0 / 360.0);
    let s = (s * d[1]).min(1.0).max(0.0);
    let ratio = if srgb {
        srgb_eotf((ve * d[2]).max(0.0)) / vc
    } else {
        d[2].max(0.0)
    };
    hsv_to_rgb(h, s, v * ratio)
}

/// `.cube` header: `[n, dmin_r, dmin_g, dmin_b, dmax_r, dmax_g, dmax_b]`, then the
/// linear-Rec.2020 → linear-sRGB-primaries matrix and its inverse (9 + 9), then `n³` RGB
/// entries, red fastest (the `.cube` order).
pub const CUBE_HEADER: usize = 7 + 18;

#[inline]
fn cube_at(t: &[f32], data: usize, n: u32, r: u32, g: u32, b: u32) -> Rgb {
    let i = data + (((b * n + g) * n + r) as usize) * 3;
    [t[i], t[i + 1], t[i + 2]]
}

/// Tetrahedral interpolation of the cube at `base` for shaper-encoded `e` (in the domain).
#[inline]
fn tetra(t: &[f32], base: usize, e: Rgb) -> Rgb {
    let n = t[base] as u32;
    let data = base + CUBE_HEADER;
    let mut f = [0f32; 3];
    let mut i0 = [0u32; 3];
    let mut c = 0;
    while c < 3 {
        let lo = t[base + 1 + c];
        let hi = t[base + 4 + c];
        let x = clamp01((e[c] - lo) / (hi - lo)) * (n - 1) as f32;
        let mut i = floor(x) as u32;
        if i + 1 >= n {
            i = n - 2;
        }
        i0[c] = i;
        f[c] = x - i as f32;
        c += 1;
    }
    let (r0, g0, b0) = (i0[0], i0[1], i0[2]);
    let (fr, fg, fb) = (f[0], f[1], f[2]);
    let c000 = cube_at(t, data, n, r0, g0, b0);
    let c111 = cube_at(t, data, n, r0 + 1, g0 + 1, b0 + 1);
    // The tetrahedron containing the point is chosen by the order of the fractions.
    let (a, b, wa, wb, w0, w1);
    if fr >= fg {
        if fg >= fb {
            a = cube_at(t, data, n, r0 + 1, g0, b0);
            b = cube_at(t, data, n, r0 + 1, g0 + 1, b0);
            (w0, wa, wb, w1) = (1.0 - fr, fr - fg, fg - fb, fb);
        } else if fr >= fb {
            a = cube_at(t, data, n, r0 + 1, g0, b0);
            b = cube_at(t, data, n, r0 + 1, g0, b0 + 1);
            (w0, wa, wb, w1) = (1.0 - fr, fr - fb, fb - fg, fg);
        } else {
            a = cube_at(t, data, n, r0, g0, b0 + 1);
            b = cube_at(t, data, n, r0 + 1, g0, b0 + 1);
            (w0, wa, wb, w1) = (1.0 - fb, fb - fr, fr - fg, fg);
        }
    } else if fb >= fg {
        a = cube_at(t, data, n, r0, g0, b0 + 1);
        b = cube_at(t, data, n, r0, g0 + 1, b0 + 1);
        (w0, wa, wb, w1) = (1.0 - fb, fb - fg, fg - fr, fr);
    } else if fb >= fr {
        a = cube_at(t, data, n, r0, g0 + 1, b0);
        b = cube_at(t, data, n, r0, g0 + 1, b0 + 1);
        (w0, wa, wb, w1) = (1.0 - fg, fg - fb, fb - fr, fr);
    } else {
        a = cube_at(t, data, n, r0, g0 + 1, b0);
        b = cube_at(t, data, n, r0 + 1, g0 + 1, b0);
        (w0, wa, wb, w1) = (1.0 - fg, fg - fr, fr - fb, fb);
    }
    [
        w0 * c000[0] + wa * a[0] + wb * b[0] + w1 * c111[0],
        w0 * c000[1] + wa * a[1] + wb * b[1] + w1 * c111[1],
        w0 * c000[2] + wa * a[2] + wb * b[2] + w1 * c111[2],
    ]
}

#[inline]
fn mat_at(t: &[f32], o: usize) -> [[f32; 3]; 3] {
    [
        [t[o], t[o + 1], t[o + 2]],
        [t[o + 3], t[o + 4], t[o + 5]],
        [t[o + 6], t[o + 7], t[o + 8]],
    ]
}

/// Apply the look cube at `base` to bounded linear Rec.2020 (D1 output). The part of the
/// colour outside the sRGB gamut is carried around the LUT unchanged.
#[inline]
pub fn cube(px: Rgb, t: &[f32], base: usize) -> Rgb {
    let lin = mat3_mul(&mat_at(t, base + 7), px);
    let cl = [clamp01(lin[0]), clamp01(lin[1]), clamp01(lin[2])];
    let e = [srgb_oetf(cl[0]), srgb_oetf(cl[1]), srgb_oetf(cl[2])];
    let o = tetra(t, base, e);
    let back = [
        srgb_eotf(clamp01(o[0])) + (lin[0] - cl[0]),
        srgb_eotf(clamp01(o[1])) + (lin[1] - cl[1]),
        srgb_eotf(clamp01(o[2])) + (lin[2] - cl[2]),
    ];
    mat3_mul(&mat_at(t, base + 16), back)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: [f32; 9] = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];

    fn cube_table(n: u32, f: impl Fn(Rgb) -> Rgb) -> Vec<f32> {
        let mut t = vec![n as f32, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0];
        t.extend_from_slice(&ID);
        t.extend_from_slice(&ID);
        for b in 0..n {
            for g in 0..n {
                for r in 0..n {
                    let s = (n - 1) as f32;
                    t.extend_from_slice(&f([r as f32 / s, g as f32 / s, b as f32 / s]));
                }
            }
        }
        t
    }

    #[test]
    fn identity_cube_is_identity_and_affine_cubes_are_exact() {
        let id = cube_table(17, |c| c);
        // Tetrahedral interpolation reproduces any affine function exactly.
        let aff = cube_table(9, |c| {
            [0.8 * c[0] + 0.1 * c[1], c[1] * 0.5 + 0.2, 1.0 - c[2]]
        });
        for px in [
            [0.0, 0.0, 0.0],
            [0.2, 0.5, 0.9],
            [1.0, 1.0, 1.0],
            [0.05, 0.6, 0.3],
        ] {
            let o = cube(px, &id, 0);
            for c in 0..3 {
                assert!((o[c] - px[c]).abs() < 1e-5, "{px:?} → {o:?}");
            }
            let e = px.map(srgb_oetf);
            let want = [0.8 * e[0] + 0.1 * e[1], e[1] * 0.5 + 0.2, 1.0 - e[2]];
            let got = tetra(&aff, 0, e);
            for c in 0..3 {
                assert!(
                    (got[c] - want[c]).abs() < 1e-5,
                    "{e:?}: {got:?} vs {want:?}"
                );
            }
        }
        // Out of gamut and above 1: the excess passes through.
        let o = cube([1.5, -0.1, 0.4], &id, 0);
        assert!(
            (o[0] - 1.5).abs() < 1e-5 && (o[1] + 0.1).abs() < 1e-5,
            "{o:?}"
        );
    }

    fn hsv_tab(
        hd: u32,
        sd: u32,
        vd: u32,
        srgb: bool,
        f: impl Fn(u32, u32, u32) -> [f32; 3],
    ) -> Vec<f32> {
        let mut t = vec![hd as f32, sd as f32, vd as f32, srgb as u32 as f32];
        for v in 0..vd {
            for h in 0..hd {
                for s in 0..sd {
                    t.extend_from_slice(&f(v, h, s));
                }
            }
        }
        t
    }

    #[test]
    fn hsv_round_trip_and_identity_table() {
        for c in [
            [0.2, 0.5, 0.9],
            [0.9, 0.1, 0.3],
            [0.4, 0.4, 0.4],
            [2.5, 1.0, 0.2],
        ] {
            let (h, s, v) = rgb_to_hsv(c);
            let o = hsv_to_rgb(h, s, v);
            for k in 0..3 {
                assert!((o[k] - c[k]).abs() < 1e-5, "{c:?} → {o:?}");
            }
        }
        for (vd, srgb) in [(1, false), (8, true)] {
            let t = hsv_tab(90, 30, vd, srgb, |_, _, _| [0.0, 1.0, 1.0]);
            for c in [[0.2, 0.5, 0.9], [3.0, 1.0, 0.5], [0.01, 0.02, 0.0]] {
                let o = hsv_table(c, &t, 0);
                for k in 0..3 {
                    assert!((o[k] - c[k]).abs() < 1e-4 * c[k].max(1.0), "{c:?} → {o:?}");
                }
            }
        }
    }

    #[test]
    fn hsv_table_shifts_hue_and_scales() {
        // +60° everywhere: red → yellow. Saturation halved, value doubled (linear).
        let t = hsv_tab(6, 2, 1, false, |_, _, _| [60.0, 0.5, 2.0]);
        let o = hsv_table([0.8, 0.0, 0.0], &t, 0);
        let (h, s, v) = rgb_to_hsv(o);
        assert!(
            (h - 1.0).abs() < 1e-4 && (s - 0.5).abs() < 1e-4 && (v - 1.6).abs() < 1e-4,
            "{o:?}"
        );
        // Hue wraps: a magenta-ish red near h = 5.9 shifted by +60° lands near h = 0.9.
        let o = hsv_table(hsv_to_rgb(5.9, 0.8, 0.5), &t, 0);
        assert!((rgb_to_hsv(o).0 - 0.9).abs() < 1e-3);
    }
}
