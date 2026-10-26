//! M: local-adjustment fields (doc 02 §9), evaluated per pixel from a compiled *mask program*.
//!
//! The host (`aigle-engine` `masks`) compiles the masks of an edit into a flat `f32` program
//! for one tile or block: geometry is culled to the region, so the per-pixel cost is
//! O(strokes touching the region). CPU and GPU evaluate the same [`eval`]; brushes are SDF
//! segments, as doc 04 §5 prescribes.
//!
//! Layout (offsets into the program slice, starting at `base`):
//!
//! ```text
//! header (HEADER floats):
//!   n_masks, n_params, sw, sh,              source size in px (normalised coords × (sw, sh))
//!   map[6]                                  level px (u, v) → normalised Source coords
//!   ok_lms[9], ok_lab[9]                    linear Rec.2020 → OkLab (colour range)
//!   cat[9], cat_inv[9]                      linear Rec.2020 ↔ CAT16 LMS (local WB)
//! per mask:
//!   opacity, invert, prog_len, delta[n_params], prog[prog_len]
//! ```
//!
//! Programs are postfix; every shape pushes one alpha, combinators pop two:
//!
//! ```text
//! RADIAL cx cy rx ry cos sin inner         (source px; inner = 1 − feather)
//! LINEAR x0 y0 dx dy                       (dx, dy = (to − from) / |to − from|²)
//! BRUSH  n_strokes { n_segs r inner flow erase { ax ay bx by }* }*
//! LUM    low high width                    (encoded luminance of the global-C1 pixel)
//! COLOR  n tol width { L a b }*            (OkLab)
//! CONST  v
//! MAX | SUB | MUL                          (Add | Subtract | Intersect, doc 02 §9.1)
//! ```
//!
//! Fields are `Σᵢ αᵢ·Δᵢ` per parameter, in the order the host chose (≤ [`MAX_PARAMS`]).

use crate::Mat3f;
use crate::Rgb;
use crate::math::{cbrt, clamp01, luminance, mat3_mul, smoothstep, sqrt, srgb_oetf};

pub const MAX_PARAMS: usize = 12;
pub const HEADER: usize = 4 + 6 + 36;
const STACK: usize = 8;

pub const OP_RADIAL: u32 = 1;
pub const OP_LINEAR: u32 = 2;
pub const OP_BRUSH: u32 = 3;
pub const OP_LUM: u32 = 4;
pub const OP_COLOR: u32 = 5;
pub const OP_CONST: u32 = 6;
pub const OP_MAX: u32 = 7;
pub const OP_SUB: u32 = 8;
pub const OP_MUL: u32 = 9;

#[inline]
fn mat(p: &[f32], o: usize) -> Mat3f {
    [
        [p[o], p[o + 1], p[o + 2]],
        [p[o + 3], p[o + 4], p[o + 5]],
        [p[o + 6], p[o + 7], p[o + 8]],
    ]
}

/// Squared distance from (x, y) to segment a–b.
#[inline]
fn seg_dist2(x: f32, y: f32, ax: f32, ay: f32, bx: f32, by: f32) -> f32 {
    let (dx, dy) = (bx - ax, by - ay);
    let l2 = dx * dx + dy * dy;
    let t = if l2 > 0.0 {
        clamp01(((x - ax) * dx + (y - ay) * dy) / l2)
    } else {
        0.0
    };
    let (px, py) = (ax + t * dx - x, ay + t * dy - y);
    px * px + py * py
}

/// (Rec.2020 → CAT16 LMS, inverse) from the program header.
#[inline]
pub fn cat(p: &[f32], base: usize) -> (Mat3f, Mat3f) {
    (mat(p, base + 28), mat(p, base + 37))
}

/// Pixel quantities the range components need, computed once per pixel.
struct Px {
    x: f32,
    y: f32,
    lum: f32,
    lab: [f32; 3],
}

/// Alpha of one program `p[o..o + len]` at pixel `px`.
fn run(p: &[f32], o: usize, len: usize, px: &Px) -> f32 {
    let mut st = [0f32; STACK];
    let mut sp = 0usize;
    let mut i = o;
    let end = o + len;
    while i < end {
        let op = p[i] as u32;
        i += 1;
        let push;
        if op == OP_RADIAL {
            let (cx, cy, rx, ry, c, s, inner) = (
                p[i],
                p[i + 1],
                p[i + 2],
                p[i + 3],
                p[i + 4],
                p[i + 5],
                p[i + 6],
            );
            i += 7;
            let (dx, dy) = (px.x - cx, px.y - cy);
            let u = (dx * c + dy * s) / rx.max(1e-6);
            let v = (-dx * s + dy * c) / ry.max(1e-6);
            let d = sqrt(u * u + v * v);
            push = Some(1.0 - smoothstep(inner.min(0.999), 1.0, d));
        } else if op == OP_LINEAR {
            let t = (px.x - p[i]) * p[i + 2] + (px.y - p[i + 1]) * p[i + 3];
            i += 4;
            push = Some(1.0 - smoothstep(0.0, 1.0, t));
        } else if op == OP_BRUSH {
            let n = p[i] as u32;
            i += 1;
            let mut a = 0.0f32;
            let mut k = 0;
            while k < n {
                let (segs, r, inner, flow, erase) =
                    (p[i] as u32, p[i + 1], p[i + 2], p[i + 3], p[i + 4]);
                i += 5;
                let mut d2 = f32::MAX;
                let mut j = 0;
                while j < segs {
                    d2 = d2.min(seg_dist2(px.x, px.y, p[i], p[i + 1], p[i + 2], p[i + 3]));
                    i += 4;
                    j += 1;
                }
                let d = sqrt(d2) / r.max(1e-6);
                let w = (1.0 - smoothstep(inner.min(0.999), 1.0, d)) * flow;
                a = if erase != 0.0 {
                    a * (1.0 - w)
                } else {
                    a + w - a * w
                };
                k += 1;
            }
            push = Some(a);
        } else if op == OP_LUM {
            let (lo, hi, w) = (p[i], p[i + 1], p[i + 2]);
            i += 3;
            let rise = if lo <= 0.0 {
                1.0
            } else {
                smoothstep(lo - w, lo + 1e-6, px.lum)
            };
            let fall = if hi >= 1.0 {
                1.0
            } else {
                1.0 - smoothstep(hi - 1e-6, hi + w, px.lum)
            };
            push = Some(rise * fall);
        } else if op == OP_COLOR {
            let (n, tol, w) = (p[i] as u32, p[i + 1], p[i + 2]);
            i += 3;
            let mut best = f32::MAX;
            let mut k = 0;
            while k < n {
                let (dl, da, db) = (px.lab[0] - p[i], px.lab[1] - p[i + 1], px.lab[2] - p[i + 2]);
                best = best.min(sqrt(0.25 * dl * dl + da * da + db * db));
                i += 3;
                k += 1;
            }
            push = Some(1.0 - smoothstep(tol, tol + w, best));
        } else if op == OP_CONST {
            push = Some(p[i]);
            i += 1;
        } else if (op == OP_MAX || op == OP_SUB || op == OP_MUL) && sp >= 2 {
            let b = st[sp - 1];
            let a = st[sp - 2];
            sp -= 2;
            push = Some(if op == OP_MAX {
                a.max(b)
            } else if op == OP_SUB {
                a * (1.0 - b)
            } else {
                a * b
            });
        } else {
            // Malformed program: stop rather than read garbage.
            break;
        }
        if let Some(v) = push
            && sp < STACK
        {
            st[sp] = v;
            sp += 1;
        }
    }
    if sp > 0 { clamp01(st[sp - 1]) } else { 0.0 }
}

fn prepare(p: &[f32], base: usize, u: f32, v: f32, rgb: Rgb) -> Px {
    let m = base + 4;
    let (fu, fv) = (u + 0.5, v + 0.5);
    let nx = p[m] * fu + p[m + 1] * fv + p[m + 2];
    let ny = p[m + 3] * fu + p[m + 4] * fv + p[m + 5];
    let lms = mat3_mul(&mat(p, base + 10), rgb);
    let lab = mat3_mul(
        &mat(p, base + 19),
        [cbrt(lms[0]), cbrt(lms[1]), cbrt(lms[2])],
    );
    Px {
        x: nx * p[base + 2],
        y: ny * p[base + 3],
        lum: srgb_oetf(clamp01(luminance(rgb))),
        lab,
    }
}

/// Accumulate all masks' fields at level coordinates (`u`, `v`) (pixel-centre convention, as
/// sampled by B1) for the global-C1 pixel `rgb`.
pub fn eval(p: &[f32], base: usize, u: f32, v: f32, rgb: Rgb) -> [f32; MAX_PARAMS] {
    let mut out = [0f32; MAX_PARAMS];
    let n = p[base] as u32;
    let np = p[base + 1] as usize;
    if n == 0 {
        return out;
    }
    let px = prepare(p, base, u, v, rgb);
    let mut o = base + HEADER;
    let mut k = 0;
    while k < n {
        let (opacity, invert, len) = (p[o], p[o + 1], p[o + 2] as usize);
        let deltas = o + 3;
        let prog = deltas + np;
        let mut a = run(p, prog, len, &px);
        if invert != 0.0 {
            a = 1.0 - a;
        }
        a *= opacity;
        let mut j = 0;
        while j < np && j < MAX_PARAMS {
            out[j] += a * p[deltas + j];
            j += 1;
        }
        o = prog + len;
        k += 1;
    }
    out
}

/// Alpha of mask number `index` alone (mask overlay, CPU).
pub fn alpha_of(p: &[f32], base: usize, index: u32, u: f32, v: f32, rgb: Rgb) -> f32 {
    let n = p[base] as u32;
    let np = p[base + 1] as usize;
    let px = prepare(p, base, u, v, rgb);
    let mut o = base + HEADER;
    let mut k = 0;
    while k < n {
        let (opacity, invert, len) = (p[o], p[o + 1], p[o + 2] as usize);
        let prog = o + 3 + np;
        if k == index {
            let a = run(p, prog, len, &px);
            return opacity * if invert != 0.0 { 1.0 - a } else { a };
        }
        o = prog + len;
        k += 1;
    }
    0.0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Header with identity mapping (level px = normalised × 100) and no OkLab.
    fn header(n: usize, np: usize) -> Vec<f32> {
        let mut h = vec![
            n as f32, np as f32, 100.0, 100.0, 0.01, 0.0, 0.0, 0.0, 0.01, 0.0,
        ];
        let id = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];
        for _ in 0..4 {
            h.extend_from_slice(&id);
        }
        h
    }

    #[test]
    fn shapes_and_combination() {
        let mut p = header(2, 1);
        // Mask 0: radial at (50, 50), radius 20, hard edge, Δ = 2.
        p.extend_from_slice(&[
            1.0,
            0.0,
            8.0,
            2.0,
            OP_RADIAL as f32,
            50.0,
            50.0,
            20.0,
            20.0,
            1.0,
            0.0,
            0.9,
        ]);
        // Mask 1: brush stroke x 0..100 at y = 10, radius 5, minus a linear gradient; Δ = 1.
        p.extend_from_slice(&[1.0, 0.0, 17.0, 1.0]);
        p.extend_from_slice(&[
            OP_BRUSH as f32,
            1.0,
            1.0,
            5.0,
            0.5,
            1.0,
            0.0,
            0.0,
            10.0,
            100.0,
            10.0,
        ]);
        p.extend_from_slice(&[OP_LINEAR as f32, 50.0, 0.0, 0.0, 0.0, OP_SUB as f32]);
        // Linear with zero direction → t = 0 → α = 1, so SUB removes the brush entirely…
        assert_eq!(eval(&p, 0, 49.5, 49.5, [0.2; 3])[0], 2.0);
        assert_eq!(eval(&p, 0, 9.5, 9.5, [0.2; 3])[0], 0.0);
        // …and with a real gradient (from x = 50 to x = 100) only the left half is removed.
        let n = p.len();
        p[n - 3] = 1.0 / 50.0;
        assert!((eval(&p, 0, 79.5, 9.5, [0.2; 3])[0] - 0.648).abs() < 0.01);
        assert_eq!(eval(&p, 0, 29.5, 9.5, [0.2; 3])[0], 0.0);
        assert_eq!(alpha_of(&p, 0, 0, 49.5, 49.5, [0.2; 3]), 1.0);
    }

    #[test]
    fn luminance_range_and_invert() {
        let mut p = header(1, 2);
        p.extend_from_slice(&[0.5, 1.0, 4.0, 1.0, -3.0, OP_LUM as f32, 0.6, 1.0, 0.1]);
        // Inverted "highlights only" at half opacity: dark pixel gets 0.5·Δ.
        let dark = eval(&p, 0, 0.0, 0.0, [0.01; 3]);
        assert!((dark[0] - 0.5).abs() < 1e-6 && (dark[1] + 1.5).abs() < 1e-6);
        let bright = eval(&p, 0, 0.0, 0.0, [0.9; 3]);
        assert!(bright[0].abs() < 1e-6);
    }
}
