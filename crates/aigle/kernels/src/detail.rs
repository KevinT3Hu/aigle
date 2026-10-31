//! S tier passes (doc 02 §8): one function per pixel and pass, shared by the CPU executor
//! (`aigle_engine::detail`) and the `s_pass` entry point.
//!
//! A source block is processed in a workspace of `f32` planes of `rw × rh` pixels (the block
//! plus its margin). The host builds a *plan*, a sequence of passes; each pass reads up to
//! four planes and writes up to four, described by a `u32` header of [`UP_LEN`] words and an
//! `f32` parameter list. Stencil passes never write a plane they read; pointwise passes may
//! write in place (each pixel only reads its own position).
//!
//! The rebuild pass writes the block interior (`gw × gh`, offset by the margin) as
//! interleaved RGB into three consecutive planes starting at `out[0]`.

use crate::masks;
use crate::math::{atan2, exp2, log2, smoothstep, sqrt};
use crate::pointwise::{C1Params, c1_color};
use crate::resample::{Rgb2D, mirror};

/// `u32` words per pass header.
pub const UP_LEN: usize = 24;
/// No plane.
pub const NONE: u32 = u32::MAX;
/// [`out_pos`]: the slot is not written.
pub const NO_POS: usize = usize::MAX;

pub const U_OP: usize = 0;
pub const U_RW: usize = 1;
pub const U_RH: usize = 2;
/// Four input plane indices.
pub const U_IN: usize = 3;
/// Four output plane indices.
pub const U_OUT: usize = 7;
/// Dispatch grid.
pub const U_GW: usize = 11;
pub const U_GH: usize = 12;
/// Op-specific words.
pub const U_A: usize = 13;
pub const U_B: usize = 14;
pub const U_C: usize = 15;
/// Source ROI on the device (x0, y0, w, h, full_w, full_h, f32 flag); set by the GPU
/// executor.
pub const U_ROI: usize = 16;

/// Regularisation of ratios and logarithms.
pub const E: f32 = 1e-5;

/// Source → `Y, Cr, Cb, z = √(Y + off)`. `a, b`: level position of region pixel (0, 0) as
/// `i32`. fp: `[off]`.
pub const OP_SPLIT: u32 = 0;
/// Source → S-tier local fields `[texture, sharpness, noise]` (`masks::eval` slots 0–2,
/// clamped to ±100). `a, b` as for split; `c`: colour block offset. fp: C1 matrix (9),
/// mask program at 9, colour block at `c`.
pub const OP_FIELDS: u32 = 1;
/// 5×5 bilateral on `z` (in0) with the local noise field (in1) → `[z', max(z'² − off, 0)]`.
/// fp: `[σ_z, luma, detail, keep, off, 25 spatial weights]`.
pub const OP_BILATERAL: u32 = 2;
/// Box mean of radius `a` over `b` planes (horizontal / vertical), window clipped to the
/// region.
pub const OP_BOX_H: u32 = 3;
pub const OP_BOX_V: u32 = 4;
/// `[I·p1, I·p2, I·I]` for `I` = in0, `p1, p2` = in1, in2.
pub const OP_PROD: u32 = 5;
/// Guided-filter coefficients from means `[I, p, Ip, II]` → `[a, b]`. fp: `[ε]`.
pub const OP_COEF: u32 = 6;
/// `p + (ā·I + b̄ − p)·w` for in = `[I, ā, b̄, p]`. fp: `[w]`.
pub const OP_APPLY: u32 = 7;
/// Separable Gaussian, clamp-to-edge, radius `a`, weights `fp[0..=a]`; post-op `b`
/// ([`POST_NONE`], [`POST_RATIO`], [`POST_MUL`]) with in1.
pub const OP_GAUSS_H: u32 = 8;
pub const OP_GAUSS_V: u32 = 9;
/// `max(in0, 0)`.
pub const OP_MAX0: u32 = 10;
/// Capture-sharpening combine: in = `[y, rl estimate, z, sharpness field]`.
/// fp: `[amount, fade, limit, mask threshold, σ_z]`.
pub const OP_SHARPEN: u32 = 11;
/// `log2(max(in0, 0) + E)`.
pub const OP_LOG: u32 = 12;
/// Texture: in = `[log y, its low pass, texture field]` → y. fp: `[texture]`.
pub const OP_TEXTURE: u32 = 13;
/// RGB from in = `[y before, y after sharpening/texture, Cr, Cb]`; margin `a`.
pub const OP_REBUILD: u32 = 14;
/// Gradient magnitude (central differences, clamp-to-edge) of in0.
pub const OP_GRAD: u32 = 15;
/// Defringe: in = `[mean gradient of z, Cr, Cb]` → `[Cr, Cb]`.
/// fp: `[purple amount, purple hue, green amount, green hue, edge threshold]` (amounts in
/// 0..1, hues in degrees of `atan2(Cb, Cr)`).
pub const OP_DEFRINGE: u32 = 16;
/// Half width (degrees) of the defringe hue windows.
pub const DEFRINGE_WIDTH: f32 = 40.0;

pub const POST_NONE: u32 = 0;
/// `(max(in1, 0) + E) / (v + E)`.
pub const POST_RATIO: u32 = 1;
/// `in1 · v`.
pub const POST_MUL: u32 = 2;

#[inline]
fn fmin(a: f32, b: f32) -> f32 {
    if a < b { a } else { b }
}

#[inline]
fn fmax(a: f32, b: f32) -> f32 {
    if a > b { a } else { b }
}

#[inline]
fn clampf(v: f32, lo: f32, hi: f32) -> f32 {
    fmin(fmax(v, lo), hi)
}

#[inline]
fn clampi(v: i32, hi: u32) -> u32 {
    if v < 0 {
        0
    } else if v as u32 > hi {
        hi
    } else {
        v as u32
    }
}

#[inline]
fn plane(up: &[u32], slot: usize) -> u32 {
    up[U_IN + slot]
}

/// Plane value at region pixel (x, y); 0 for [`NONE`].
#[inline]
fn rd(w: &[f32], up: &[u32], slot: usize, x: u32, y: u32) -> f32 {
    let p = plane(up, slot);
    if p == NONE {
        return 0.0;
    }
    let rw = up[U_RW];
    let n = (rw * up[U_RH]) as usize;
    w[p as usize * n + (y * rw + x) as usize]
}

/// Workspace index written by output `k` of the pass at grid position (x, y).
#[inline]
pub fn out_pos(up: &[u32], k: u32, x: u32, y: u32) -> usize {
    let rw = up[U_RW];
    let n = (rw * up[U_RH]) as usize;
    if up[U_OP] == OP_REBUILD {
        if k >= 3 {
            return NO_POS;
        }
        return up[U_OUT] as usize * n + ((y * up[U_GW] + x) * 3 + k) as usize;
    }
    let p = up[U_OUT + k as usize];
    if p == NONE {
        return NO_POS;
    }
    p as usize * n + (y * rw + x) as usize
}

/// Level pixel under region pixel (x, y), mirrored at the level edges.
#[inline]
fn src_px<R: Rgb2D>(src: &R, up: &[u32], x: u32, y: u32) -> (u32, u32) {
    let sx = mirror(up[U_A] as i32 + x as i32, src.width());
    let sy = mirror(up[U_B] as i32 + y as i32, src.height());
    (sx, sy)
}

/// GPU entry point that runs `op` ([`ENTRY_SPLIT`] …): one per op group, so that each
/// pipeline only carries the code (and register pressure) of its own ops.
pub fn entry(op: u32) -> u32 {
    if op == OP_SPLIT {
        ENTRY_SPLIT
    } else if op == OP_FIELDS {
        ENTRY_FIELDS
    } else if op == OP_BILATERAL {
        ENTRY_BILATERAL
    } else if op == OP_BOX_H || op == OP_BOX_V {
        ENTRY_BOX
    } else if op == OP_GAUSS_H || op == OP_GAUSS_V {
        ENTRY_GAUSS
    } else {
        ENTRY_POINT
    }
}

pub const ENTRY_SPLIT: u32 = 0;
pub const ENTRY_FIELDS: u32 = 1;
pub const ENTRY_BILATERAL: u32 = 2;
pub const ENTRY_BOX: u32 = 3;
pub const ENTRY_GAUSS: u32 = 4;
pub const ENTRY_POINT: u32 = 5;
/// Entry point names, indexed by `ENTRY_*`.
pub const ENTRY_NAMES: [&str; 6] = [
    "s_split",
    "s_fields",
    "s_bilateral",
    "s_box",
    "s_gauss",
    "s_point",
];

/// Outputs of one pass at grid position (x, y); see the `OP_*` constants.
pub fn s_px<R: Rgb2D>(w: &[f32], src: &R, fp: &[f32], up: &[u32], x: u32, y: u32) -> [f32; 4] {
    let e = entry(up[U_OP]);
    if e == ENTRY_SPLIT {
        split_px(src, fp, up, x, y)
    } else if e == ENTRY_FIELDS {
        fields_px(src, fp, up, x, y)
    } else if e == ENTRY_BILATERAL {
        bilateral_px(w, fp, up, x, y)
    } else if e == ENTRY_BOX {
        box_px(w, up, x, y)
    } else if e == ENTRY_GAUSS {
        gauss_px(w, fp, up, x, y)
    } else {
        point_px(w, fp, up, x, y)
    }
}

/// [`OP_SPLIT`].
pub fn split_px<R: Rgb2D>(src: &R, fp: &[f32], up: &[u32], x: u32, y: u32) -> [f32; 4] {
    let (sx, sy) = src_px(src, up, x, y);
    let p = src.load(sx, sy);
    let l = (p[0] + 2.0 * p[1] + p[2]) * 0.25;
    [l, p[0] - p[1], p[2] - p[1], sqrt(fmax(l, 0.0) + fp[0])]
}

/// [`OP_FIELDS`].
pub fn fields_px<R: Rgb2D>(src: &R, fp: &[f32], up: &[u32], x: u32, y: u32) -> [f32; 4] {
    let (sx, sy) = src_px(src, up, x, y);
    let p = src.load(sx, sy);
    let c1 = C1Params {
        matrix: [
            [fp[0], fp[1], fp[2]],
            [fp[3], fp[4], fp[5]],
            [fp[6], fp[7], fp[8]],
        ],
    };
    let rgb = c1_color(p, &c1, fp, up[U_C] as usize);
    let f = masks::eval(fp, 9, sx as f32, sy as f32, rgb);
    [
        clampf(f[0], -100.0, 100.0),
        clampf(f[1], -100.0, 100.0),
        clampf(f[2], -100.0, 100.0),
        0.0,
    ]
}

/// [`OP_BILATERAL`].
pub fn bilateral_px(w: &[f32], fp: &[f32], up: &[u32], x: u32, y: u32) -> [f32; 4] {
    let (rw, rh) = (up[U_RW], up[U_RH]);
    let c = rd(w, up, 0, x, y);
    let luma = clampf(fp[1] + rd(w, up, 1, x, y), 0.0, 100.0);
    let srk = fp[0] * (0.5 + 3.0 * luma / 100.0) * (1.5 - fp[2] / 100.0);
    let mut f = c;
    if luma > 0.0 && srk > 0.0 {
        let inv = 1.0 / (srk * srk);
        let (mut acc, mut ws) = (0f32, 0f32);
        let mut j = 0u32;
        while j < 5 {
            let yy = clampi(y as i32 + j as i32 - 2, rh - 1);
            let mut i = 0u32;
            while i < 5 {
                let xx = clampi(x as i32 + i as i32 - 2, rw - 1);
                let v = rd(w, up, 0, xx, yy);
                let d = (v - c) * (v - c) * inv;
                let r = 1.0 / (1.0 + d);
                let wt = fp[5 + (j * 5 + i) as usize] * r * r;
                acc += wt * v;
                ws += wt;
                i += 1;
            }
            j += 1;
        }
        f = acc / ws;
    }
    let keep = fp[3];
    let z = f * (1.0 - keep) + c * keep;
    [z, fmax(z * z - fp[4], 0.0), 0.0, 0.0]
}

/// [`OP_BOX_H`], [`OP_BOX_V`].
pub fn box_px(w: &[f32], up: &[u32], x: u32, y: u32) -> [f32; 4] {
    let (op, rw, rh) = (up[U_OP], up[U_RW], up[U_RH]);
    let r = up[U_A] as i32;
    let np = up[U_B] as usize;
    let horiz = op == OP_BOX_H;
    let (pos, len) = if horiz { (x, rw) } else { (y, rh) };
    let a = clampi(pos as i32 - r, len - 1);
    let b = clampi(pos as i32 + r, len - 1);
    let inv = 1.0 / (b - a + 1) as f32;
    let mut out = [0f32; 4];
    let mut s = 0usize;
    while s < np {
        let mut acc = 0f32;
        let mut t = a;
        while t <= b {
            acc += if horiz {
                rd(w, up, s, t, y)
            } else {
                rd(w, up, s, x, t)
            };
            t += 1;
        }
        let v = acc * inv;
        if s == 0 {
            out[0] = v;
        } else if s == 1 {
            out[1] = v;
        } else if s == 2 {
            out[2] = v;
        } else {
            out[3] = v;
        }
        s += 1;
    }
    out
}

/// [`OP_GAUSS_H`], [`OP_GAUSS_V`].
pub fn gauss_px(w: &[f32], fp: &[f32], up: &[u32], x: u32, y: u32) -> [f32; 4] {
    let (op, rw, rh) = (up[U_OP], up[U_RW], up[U_RH]);
    let r = up[U_A];
    let horiz = op == OP_GAUSS_H;
    let mut acc = fp[0] * rd(w, up, 0, x, y);
    let mut i = 1u32;
    while i <= r {
        let (a, b) = if horiz {
            (
                rd(w, up, 0, clampi(x as i32 - i as i32, rw - 1), y),
                rd(w, up, 0, clampi((x + i) as i32, rw - 1), y),
            )
        } else {
            (
                rd(w, up, 0, x, clampi(y as i32 - i as i32, rh - 1)),
                rd(w, up, 0, x, clampi((y + i) as i32, rh - 1)),
            )
        };
        acc += fp[i as usize] * (a + b);
        i += 1;
    }
    let post = up[U_B];
    let v = if post == POST_RATIO {
        (fmax(rd(w, up, 1, x, y), 0.0) + E) / (acc + E)
    } else if post == POST_MUL {
        rd(w, up, 1, x, y) * acc
    } else {
        acc
    };
    [v, 0.0, 0.0, 0.0]
}

/// The pointwise ops and the sharpening combine.
pub fn point_px(w: &[f32], fp: &[f32], up: &[u32], x: u32, y: u32) -> [f32; 4] {
    let (op, rw, rh) = (up[U_OP], up[U_RW], up[U_RH]);
    if op == OP_PROD {
        let i = rd(w, up, 0, x, y);
        return [i * rd(w, up, 1, x, y), i * rd(w, up, 2, x, y), i * i, 0.0];
    }
    if op == OP_COEF {
        let (mi, mp) = (rd(w, up, 0, x, y), rd(w, up, 1, x, y));
        let (mip, mii) = (rd(w, up, 2, x, y), rd(w, up, 3, x, y));
        let var = fmax(mii - mi * mi, 0.0);
        let cov = mip - mi * mp;
        let a = cov / (var + fp[0]);
        return [a, mp - a * mi, 0.0, 0.0];
    }
    if op == OP_APPLY {
        let i = rd(w, up, 0, x, y);
        let p = rd(w, up, 3, x, y);
        let f = rd(w, up, 1, x, y) * i + rd(w, up, 2, x, y);
        return [p + (f - p) * fp[0], 0.0, 0.0, 0.0];
    }
    if op == OP_MAX0 {
        return [fmax(rd(w, up, 0, x, y), 0.0), 0.0, 0.0, 0.0];
    }
    if op == OP_SHARPEN {
        let amount = clampf((fp[0] + 1.5 * rd(w, up, 3, x, y)) / 100.0, -1.0, 1.5) * fp[1];
        let base = rd(w, up, 0, x, y);
        let lim = fp[2] * if base < 0.0 { -base } else { base };
        let d = clampf(rd(w, up, 1, x, y) - base, -lim, lim);
        let t = fp[3];
        let mask = if t > 0.0 {
            let xl = clampi(x as i32 - 1, rw - 1);
            let xr = clampi(x as i32 + 1, rw - 1);
            let yu = clampi(y as i32 - 1, rh - 1);
            let yd = clampi(y as i32 + 1, rh - 1);
            let gx = (rd(w, up, 2, xr, y) - rd(w, up, 2, xl, y)) * 0.5;
            let gy = (rd(w, up, 2, x, yd) - rd(w, up, 2, x, yu)) * 0.5;
            smoothstep(0.5 * t, t, sqrt(gx * gx + gy * gy) / fp[4])
        } else {
            1.0
        };
        return [base + amount * mask * d, 0.0, 0.0, 0.0];
    }
    if op == OP_LOG {
        return [log2(fmax(rd(w, up, 0, x, y), 0.0) + E), 0.0, 0.0, 0.0];
    }
    if op == OP_GRAD {
        let xl = clampi(x as i32 - 1, rw - 1);
        let xr = clampi(x as i32 + 1, rw - 1);
        let yu = clampi(y as i32 - 1, rh - 1);
        let yd = clampi(y as i32 + 1, rh - 1);
        let gx = (rd(w, up, 0, xr, y) - rd(w, up, 0, xl, y)) * 0.5;
        let gy = (rd(w, up, 0, x, yd) - rd(w, up, 0, x, yu)) * 0.5;
        return [sqrt(gx * gx + gy * gy), 0.0, 0.0, 0.0];
    }
    if op == OP_DEFRINGE {
        let (cr, cb) = (rd(w, up, 1, x, y), rd(w, up, 2, x, y));
        let k = defringe_gain(rd(w, up, 0, x, y), cr, cb, fp);
        return [cr * k, cb * k, 0.0, 0.0];
    }
    if op == OP_TEXTURE {
        let ly = rd(w, up, 0, x, y);
        let low = rd(w, up, 1, x, y);
        let kk = clampf((fp[0] + rd(w, up, 2, x, y)) / 100.0, -1.0, 1.0) * 0.8;
        let v = exp2(ly + kk * clampf(ly - low, -1.0, 1.0)) - E;
        return [fmax(v, 0.0), 0.0, 0.0, 0.0];
    }
    if op == OP_REBUILD {
        let m = up[U_A];
        let (xx, yy) = (x + m, y + m);
        let base = rd(w, up, 0, xx, yy);
        let yf = rd(w, up, 1, xx, yy);
        let ratio = if base > E { yf / base } else { 1.0 };
        let crk = rd(w, up, 2, xx, yy) * ratio;
        let cbk = rd(w, up, 3, xx, yy) * ratio;
        let g = yf - (crk + cbk) * 0.25;
        return [crk + g, g, cbk + g, 0.0];
    }
    [0.0; 4]
}

/// Weight of hue `h` in the window centred on `c` (degrees, wrapping).
#[inline]
fn hue_window(h: f32, c: f32) -> f32 {
    let mut d = h - c;
    if d > 180.0 {
        d -= 360.0;
    } else if d < -180.0 {
        d += 360.0;
    }
    let d = if d < 0.0 { -d } else { d };
    1.0 - smoothstep(0.5 * DEFRINGE_WIDTH, DEFRINGE_WIDTH, d)
}

/// [`OP_DEFRINGE`] chroma gain at mean gradient `g` and chroma (`cr`, `cb`).
#[inline]
pub fn defringe_gain(g: f32, cr: f32, cb: f32, fp: &[f32]) -> f32 {
    if cr * cr + cb * cb < E * E {
        return 1.0;
    }
    let t = fp[4];
    let edge = smoothstep(t, 3.0 * t, g);
    let mut h = atan2(cb, cr) * (180.0 / core::f32::consts::PI);
    if h < 0.0 {
        h += 360.0;
    }
    let k = fp[0] * hue_window(h, fp[1]) + fp[2] * hue_window(h, fp[3]);
    1.0 - clampf(k, 0.0, 1.0) * edge
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defringe_only_touches_its_hues_near_edges() {
        let fp = [1.0, 45.0, 0.5, 225.0, 0.01];
        // Purple (R, B > G) at an edge is removed; away from edges it stays.
        assert!(defringe_gain(0.1, 0.2, 0.2, &fp) < 1e-6);
        assert_eq!(defringe_gain(0.001, 0.2, 0.2, &fp), 1.0);
        // Green at half amount.
        assert!((defringe_gain(0.1, -0.2, -0.2, &fp) - 0.5).abs() < 1e-6);
        // Red / yellow / blue are untouched.
        for (cr, cb) in [(0.3, -0.05), (0.05, -0.3), (-0.2, 0.3)] {
            assert_eq!(defringe_gain(0.1, cr, cb, &fp), 1.0, "{cr} {cb}");
        }
        // Neutral stays neutral.
        assert_eq!(defringe_gain(0.1, 0.0, 0.0, &fp), 1.0);
    }
}
