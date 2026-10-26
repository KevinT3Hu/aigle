//! Per-pixel functions of the two fused tile products (doc 03 §4.3) and their GPU ABI.
//!
//! * **C1 tile** = B1 resample ∘ C1 ∘ C2: read the (S-processed) source level through the
//!   tile's [`Geo`] (affine map, or a warp grid for lens / perspective correction with
//!   per-channel positions and vignetting gain), bilinear or Lanczos-3, apply the C1 matrix,
//!   then C2 with the coarse field.
//! * **Output tile** = D1 ∘ D2 ∘ D3 ∘ E1 ∘ dithered quantisation to BGRA8.
//!
//! Local adjustments (doc 02 §9): the C1 kernel evaluates the mask program once per pixel,
//! applies the C-tier fields and, when any D-tier field is used, stores contrast, whites,
//! blacks, saturation and hue next to the pixel (C1 tile stride 5 words instead of 2).
//!
//! Each kernel also takes one auxiliary `f32` buffer per batch: the C2 field for `c1_tile`,
//! the three curve tables for `out_tile`.
//!
//! The CPU backend calls [`c1_px`] / [`out_px`] directly; the SPIR-V entry points
//! (`crates/kernels-build/shader`) call the same functions after unpacking their parameters
//! with [`unpack_c1`] / [`unpack_out`]. The host packs with [`pack_c1`] / [`pack_out`], so the
//! layout is defined in exactly one place.
//!
//! GPU buffers: source and C1 tiles are RGBA f16, two `u32` per pixel (low half = R / B);
//! output tiles are BGRA8, one `u32` per pixel (B in the low byte).

use crate::c2::{C2Params, FieldView, c2_px};
use crate::d3::{D3Params, d3};
use crate::dither::{blue, quantize8};
use crate::masks;
use crate::math::exp2;
use crate::math::{luminance, mat3_mul, srgb_oetf};
use crate::pointwise::{
    C1Params, D1Params, D2Params, E1Params, c1_color, d1, d2_local, fit_output,
};
use crate::resample::{Rgb2D, bilinear, lanczos3};
use crate::{Mat3f, Rgb};

/// Tile-local grid pixel (x, y) → source-level pixel-centre coordinates:
/// `u = a0·x + a1·y + a2`, `v = a3·x + a4·y + a5`, evaluated at pixel centres (x + 0.5).
/// The host builds it in f64 relative to the tile origin, so values stay small in f32.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Affine {
    pub a: [f32; 6],
}

/// B1 sampling geometry of one C1 tile (doc 02 §5.5).
///
/// * Affine (`wn == 0`): exact for orientation, rotation and crop.
/// * Warp grid (`wn > 0`): lens distortion, TCA, vignetting and perspective. Nodes every
///   `cell` tile px (node `i` at tile-local position `i·cell`), [`WARP_NODE`] floats each:
///   level coordinates of R, G, B (pixel-centre convention), vignetting gain, footprint.
///   Bilinear between nodes; the host spaces them so the error is ≪ 0.01 px.
#[derive(Clone, Copy)]
pub struct Geo<'a> {
    pub affine: Affine,
    pub warp: &'a [f32],
    pub wbase: usize,
    /// Nodes per row; 0 = affine.
    pub wn: u32,
    pub cell: f32,
    pub flags: u32,
}

pub const GEO_LANCZOS: u32 = 1;
pub const GEO_TCA: u32 = 2;
/// Samples whose green position falls outside the level are black (lens / perspective
/// correction without the constrain crop).
pub const GEO_CLIP: u32 = 4;
pub const WARP_NODE: usize = 8;

impl Geo<'static> {
    pub fn affine(a: Affine, flags: u32) -> Geo<'static> {
        Geo {
            affine: a,
            warp: &[0.0],
            wbase: 0,
            wn: 0,
            cell: 1.0,
            flags,
        }
    }
}

#[inline]
fn sample_at<S: Rgb2D>(src: &S, u: f32, v: f32, k: f32, lanczos: bool) -> Rgb {
    if lanczos {
        lanczos3(src, u, v, k)
    } else {
        bilinear(src, u, v)
    }
}

/// `[uR, vR, uG, vG, uB, vB, gain, footprint]` at tile pixel (x, y).
#[inline]
pub fn locate(g: &Geo<'_>, x: u32, y: u32) -> [f32; WARP_NODE] {
    let fx = x as f32 + 0.5;
    let fy = y as f32 + 0.5;
    if g.wn == 0 {
        let a = &g.affine.a;
        let u = a[0] * fx + a[1] * fy + a[2];
        let v = a[3] * fx + a[4] * fy + a[5];
        let det = a[0] * a[4] - a[1] * a[3];
        let k = crate::math::sqrt(if det < 0.0 { -det } else { det });
        return [u, v, u, v, u, v, 1.0, k];
    }
    let gx = fx / g.cell;
    let gy = fy / g.cell;
    let mut i = crate::math::floor(gx) as u32;
    if i + 1 >= g.wn {
        i = g.wn - 2;
    }
    let j = crate::math::floor(gy) as u32;
    let (tx, ty) = (gx - i as f32, gy - j as f32);
    let n00 = g.wbase + ((j * g.wn + i) as usize) * WARP_NODE;
    let n10 = n00 + WARP_NODE;
    let n01 = n00 + g.wn as usize * WARP_NODE;
    let n11 = n01 + WARP_NODE;
    let w = g.warp;
    let mut out = [0f32; WARP_NODE];
    let mut k = 0;
    while k < WARP_NODE {
        let top = w[n00 + k] + (w[n10 + k] - w[n00 + k]) * tx;
        let bot = w[n01 + k] + (w[n11 + k] - w[n01 + k]) * tx;
        out[k] = top + (bot - top) * ty;
        k += 1;
    }
    out
}

/// Index of each local parameter in the C-tier mask program (doc 02 §9).
pub mod local {
    pub const EXPOSURE: usize = 0;
    pub const TEMPERATURE: usize = 1;
    pub const TINT: usize = 2;
    pub const HIGHLIGHTS: usize = 3;
    pub const SHADOWS: usize = 4;
    pub const CLARITY: usize = 5;
    pub const DEHAZE: usize = 6;
    pub const CONTRAST: usize = 7;
    pub const WHITES: usize = 8;
    pub const BLACKS: usize = 9;
    pub const SATURATION: usize = 10;
    pub const HUE: usize = 11;
    pub const N: usize = 12;
    /// Field clamp per parameter (doc 02 §9.1 `range_p`).
    pub const LIMIT: [f32; N] = [
        4.0, 100.0, 100.0, 100.0, 100.0, 100.0, 100.0, 100.0, 100.0, 100.0, 100.0, 180.0,
    ];
    /// D-tier fields carried in the C1 tile: contrast, whites, blacks, saturation, hue.
    pub const D_FIELDS: usize = 5;
}

/// C1 output plus the D-tier local fields of the pixel.
pub type C1Px = (Rgb, [f32; local::D_FIELDS]);

/// Per-tile tables of the C1 kernel in one slice: the compiled mask program (`masks`) at
/// `prog`, and the C1 colour block (`pointwise::C1_BLOCK`) at `color`. A program with zero
/// masks and an empty colour block cost one load each.
#[derive(Clone, Copy)]
pub struct C1Tables<'a> {
    pub data: &'a [f32],
    pub prog: usize,
    pub color: usize,
}

/// No masks, no colour block.
pub const NO_TABLES: C1Tables<'static> = C1Tables {
    data: &[0.0],
    prog: 0,
    color: 0,
};

/// B1 ∘ C1 ∘ (local C1) ∘ C2 for one pixel.
#[allow(clippy::too_many_arguments)]
#[inline]
pub fn c1_px<S: Rgb2D>(
    src: &S,
    g: &Geo<'_>,
    x: u32,
    y: u32,
    p: &C1Params,
    c2: &C2Params,
    field: &FieldView<'_>,
    tab: &C1Tables<'_>,
) -> C1Px {
    let prog = tab.data;
    let pbase = tab.prog;
    let l = locate(g, x, y);
    let (u, v) = (l[2], l[3]);
    let lz = g.flags & GEO_LANCZOS != 0;
    let raw = if g.flags & GEO_TCA != 0 {
        [
            sample_at(src, l[0], l[1], l[7], lz)[0],
            sample_at(src, u, v, l[7], lz)[1],
            sample_at(src, l[4], l[5], l[7], lz)[2],
        ]
    } else {
        sample_at(src, u, v, l[7], lz)
    };
    let mut gain = l[6];
    if g.flags & GEO_CLIP != 0 {
        let (lw, lh) = (src.width() as f32, src.height() as f32);
        if u < -0.5 || v < -0.5 || u > lw - 0.5 || v > lh - 0.5 {
            gain = 0.0;
        }
    }
    let raw = [raw[0] * gain, raw[1] * gain, raw[2] * gain];
    let mut px = c1_color(raw, p, tab.data, tab.color);
    let mut d = [0.0; local::D_FIELDS];
    let mut lc2 = *c2;
    if prog[pbase] != 0.0 {
        let mut f = masks::eval(prog, pbase, u, v, px);
        // Index loop: iterator adaptors over arrays do not compile with rust-gpu.
        #[allow(clippy::needless_range_loop)]
        for j in 0..local::N {
            f[j] = f[j].max(-local::LIMIT[j]).min(local::LIMIT[j]);
        }
        let k = exp2(f[local::EXPOSURE]);
        px = [px[0] * k, px[1] * k, px[2] * k];
        let (t, g) = (f[local::TEMPERATURE] / 100.0, f[local::TINT] / 100.0);
        if t != 0.0 || g != 0.0 {
            let cat = masks::cat(prog, pbase);
            let lms = mat3_mul(&cat.0, px);
            let lms = [
                lms[0] * exp2(0.35 * t),
                lms[1] * exp2(-0.2 * g),
                lms[2] * exp2(-0.35 * t),
            ];
            px = mat3_mul(&cat.1, lms);
        }
        lc2.highlights = (c2.highlights + f[local::HIGHLIGHTS] / 100.0)
            .max(-1.0)
            .min(1.0);
        lc2.shadows = (c2.shadows + f[local::SHADOWS] / 100.0).max(-1.0).min(1.0);
        lc2.clarity = (c2.clarity + f[local::CLARITY] / 100.0).max(-1.0).min(1.0);
        lc2.dehaze = (c2.dehaze + f[local::DEHAZE] / 100.0).max(-1.0).min(1.0);
        d = [
            f[local::CONTRAST],
            f[local::WHITES],
            f[local::BLACKS],
            f[local::SATURATION],
            f[local::HUE],
        ];
    }
    if lc2.active == 0 {
        return (px, d);
    }
    let f = field.sample((u + 0.5) * c2.fscale - 0.5, (v + 0.5) * c2.fscale - 0.5);
    (c2_px(px, f, &lc2), d)
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OutParams {
    pub d1: D1Params,
    pub d2: D2Params,
    pub e1: E1Params,
    pub d3: D3Params,
    /// Global contrast, whites, blacks sliders, for per-pixel D1 with local fields.
    pub d1_sliders: [f32; 3],
    /// Per-photo dither seed (doc 02 §14).
    pub seed: u32,
}

/// Scene-linear C1 output → BGRA8 packed into a `u32` (B in the low byte, i.e. the bytes
/// of `to_le_bytes` are B, G, R, A). `gx`, `gy` are absolute Crop-grid coordinates (dither
/// and grain position).
#[inline]
/// `aux` = blue-noise texture followed by the curve tables (see `pointwise::CURVE_OFFSET`).
pub fn out_px(px: Rgb, gx: u32, gy: u32, p: &OutParams, aux: &[f32]) -> u32 {
    out_px_local(px, None, gx, gy, p, aux)
}

/// [`out_px`] with the D-tier local fields of the pixel (contrast, whites, blacks,
/// saturation, hue; see [`local`]).
#[inline]
pub fn out_px_local(
    px: Rgb,
    d: Option<[f32; local::D_FIELDS]>,
    gx: u32,
    gy: u32,
    p: &OutParams,
    aux: &[f32],
) -> u32 {
    encode_out(
        out_lin_local(px, d, gx, gy, p, aux),
        gx,
        gy,
        &p.e1,
        p.seed,
        aux,
    )
}

/// D1 ∘ D2 ∘ D3 of [`out_px_local`]: display-referred linear RGB before E1.
#[inline]
pub fn out_lin_local(
    px: Rgb,
    d: Option<[f32; local::D_FIELDS]>,
    gx: u32,
    gy: u32,
    p: &OutParams,
    aux: &[f32],
) -> Rgb {
    let (v, dsat, dhue) = match d {
        Some(d) => {
            let s = p.d1_sliders;
            let c = (s[0] + d[0]).max(-100.0).min(100.0);
            let w = (s[1] + d[1]).max(-100.0).min(100.0);
            let b = (s[2] + d[2]).max(-100.0).min(100.0);
            let d1p = if d[0] == 0.0 && d[1] == 0.0 && d[2] == 0.0 {
                p.d1
            } else {
                D1Params::new(c, w, b)
            };
            (d1(px, &d1p), d[3] / 100.0, d[4])
        }
        None => (d1(px, &p.d1), 0.0, 0.0),
    };
    let v = if p.d2.is_identity() && dsat == 0.0 && dhue == 0.0 {
        v
    } else {
        d2_local(v, &p.d2, aux, dsat, dhue)
    };
    if p.d3.active == 0 {
        v
    } else {
        d3(v, gx, gy, &p.d3, p.seed)
    }
}

/// [`out_px`] without D2. The GPU uses it when D2 is the identity, so that it never unpacks
/// the large D2 parameter block into per-invocation memory.
#[inline]
#[allow(clippy::too_many_arguments)]
pub fn out_px_base(
    px: Rgb,
    gx: u32,
    gy: u32,
    d1p: &D1Params,
    e1: &E1Params,
    d3p: &D3Params,
    seed: u32,
    aux: &[f32],
) -> u32 {
    encode_out(
        out_lin_base(px, gx, gy, d1p, d3p, seed),
        gx,
        gy,
        e1,
        seed,
        aux,
    )
}

/// D1 ∘ D3 of [`out_px_base`].
#[inline]
pub fn out_lin_base(px: Rgb, gx: u32, gy: u32, d1p: &D1Params, d3p: &D3Params, seed: u32) -> Rgb {
    let v = d1(px, d1p);
    if d3p.active == 0 {
        v
    } else {
        d3(v, gx, gy, d3p, seed)
    }
}

/// Alpha of an output pixel: 255 minus clip flags (doc 02 §6.5), evaluated in the target
/// gamut before clamping. E2 reads the flags for the clipping overlay and restores 255.
pub const CLIP_HIGH: u32 = 1;
pub const CLIP_LOW: u32 = 2;

#[inline]
fn encode_out(v: Rgb, gx: u32, gy: u32, e1p: &E1Params, seed: u32, tex: &[f32]) -> u32 {
    let (e, a) = encode_target(v, e1p);
    let b = quantize8(e[2], blue(tex, gx, gy, seed, 2));
    let g = quantize8(e[1], blue(tex, gx, gy, seed, 1));
    let r = quantize8(e[0], blue(tex, gx, gy, seed, 0));
    b | (g << 8) | (r << 16) | ((255 - a) << 24)
}

/// E1 to the encoded target values in [0, 1] and the clip flags.
#[inline]
fn encode_target(v: Rgb, e1p: &E1Params) -> (Rgb, u32) {
    let l = mat3_mul(&e1p.matrix, v);
    // Clip flags describe the target gamut before fitting (doc 02 §6.5).
    let hi = l[0].max(l[1]).max(l[2]) >= 1.0;
    let lo = l[0].min(l[1]).min(l[2]) <= 0.0;
    let l = fit_output(l, luminance(v));
    let e = [srgb_oetf(l[0]), srgb_oetf(l[1]), srgb_oetf(l[2])];
    (
        e,
        if hi { CLIP_HIGH } else { 0 } + if lo { CLIP_LOW } else { 0 },
    )
}

/// 16-bit output (doc 03 §9): RGBA16 as two words, `R | G << 16`, `B | A << 16`, with
/// `A = 65535 − clip flags`. Rounded, not dithered: one 16-bit step is far below visibility.
#[inline]
pub fn encode_out16(v: Rgb, e1p: &E1Params) -> [u32; 2] {
    let (e, a) = encode_target(v, e1p);
    let q = |x: f32| (x * 65535.0 + 0.5).max(0.0).min(65535.0) as u32;
    [q(e[0]) | (q(e[1]) << 16), q(e[2]) | ((65535 - a) << 16)]
}

// ---------------------------------------------------------------------------------------------
// f16 packing (host side; the GPU uses UnpackHalf2x16 / PackHalf2x16).

/// IEEE binary16 bits → f32, exact.
pub fn f16_to_f32(h: u32) -> f32 {
    let h = h & 0xffff;
    let sign = (h & 0x8000) << 16;
    let exp = (h >> 10) & 0x1f;
    let man = h & 0x3ff;
    let bits = if exp == 0 {
        if man == 0 {
            sign
        } else {
            // Subnormal: value = man · 2^-24.
            let v = man as f32 * (1.0 / 16_777_216.0);
            return if sign != 0 { -v } else { v };
        }
    } else if exp == 31 {
        sign | 0x7f80_0000 | (man << 13)
    } else {
        sign | ((exp + 112) << 23) | (man << 13)
    };
    f32::from_bits(bits)
}

/// f32 → binary16 bits, round to nearest even (matches PackHalf2x16 on conformant drivers).
pub fn f32_to_f16(f: f32) -> u32 {
    let x = f.to_bits();
    let sign = (x >> 16) & 0x8000;
    let exp = ((x >> 23) & 0xff) as i32;
    let man = x & 0x7f_ffff;
    if exp == 255 {
        return sign | 0x7c00 | if man != 0 { 0x200 } else { 0 };
    }
    let e = exp - 127 + 15;
    if e >= 31 {
        return sign | 0x7c00;
    }
    if e <= 0 {
        if e < -10 {
            return sign;
        }
        let m = man | 0x80_0000;
        let shift = (14 - e) as u32;
        let half = 1u32 << (shift - 1);
        let rem = m & ((1 << shift) - 1);
        let mut r = m >> shift;
        if rem > half || (rem == half && (r & 1) == 1) {
            r += 1;
        }
        return sign | r;
    }
    let rem = man & 0x1fff;
    let mut r = ((e as u32) << 10) | (man >> 13);
    if rem > 0x1000 || (rem == 0x1000 && (r & 1) == 1) {
        r += 1;
    }
    sign | r
}

#[inline]
pub fn pack_rgb(px: Rgb) -> [u32; 2] {
    [
        f32_to_f16(px[0]) | (f32_to_f16(px[1]) << 16),
        f32_to_f16(px[2]),
    ]
}

#[inline]
pub fn unpack_rgb(p: [u32; 2]) -> Rgb {
    [f16_to_f32(p[0]), f16_to_f32(p[0] >> 16), f16_to_f32(p[1])]
}

/// Edge of a GPU source atlas tile (doc 03 §4.2), in level px.
pub const ATLAS_TILE: u32 = 256;
const ATLAS_SHIFT: u32 = 8;
/// `u32` words of one atlas tile (RGBA f16).
pub const ATLAS_TILE_WORDS: usize = (ATLAS_TILE * ATLAS_TILE * 2) as usize;

/// A region of interest of one source level, RGBA f16 packed, addressed in full-level
/// coordinates so that mirror boundaries apply at the level edges (doc 02 §5.1).
///
/// Two layouts:
/// - contiguous (`atlas == false`): `w × h` pixels of the rectangle at (`x0`, `y0`);
/// - atlas (doc 03 §4.2): `data` is a pool of [`ATLAS_TILE`]² tiles; the ROI is the tile
///   rectangle at tile (`x0`, `y0`) of `w × h` tiles, and `table[tbase + j·w + i]` is the
///   pool slot of tile (`x0 + i`, `y0 + j`).
pub struct PackedRoi<'a> {
    pub data: &'a [u32],
    pub x0: u32,
    pub y0: u32,
    pub w: u32,
    pub h: u32,
    pub full_w: u32,
    pub full_h: u32,
    pub atlas: bool,
    pub table: &'a [u32],
    pub tbase: usize,
    /// f32 pixels, 3 words each (full-precision export), else RGBA f16 in 2 words.
    pub full: bool,
}

impl<'a> PackedRoi<'a> {
    /// A contiguous ROI.
    pub fn contiguous(
        data: &'a [u32],
        rect: (u32, u32, u32, u32),
        full_w: u32,
        full_h: u32,
    ) -> PackedRoi<'a> {
        PackedRoi {
            data,
            x0: rect.0,
            y0: rect.1,
            w: rect.2,
            h: rect.3,
            full_w,
            full_h,
            atlas: false,
            table: data,
            tbase: 0,
            full: false,
        }
    }
}

#[inline]
fn clamp_span(v: u32, lo: u32, n: u32) -> u32 {
    if v < lo {
        0
    } else if v - lo >= n {
        n - 1
    } else {
        v - lo
    }
}

impl Rgb2D for PackedRoi<'_> {
    fn width(&self) -> u32 {
        self.full_w
    }
    fn height(&self) -> u32 {
        self.full_h
    }
    #[inline]
    fn load(&self, x: u32, y: u32) -> Rgb {
        // Clamp into the ROI; the host sizes it so this only matters for degenerate input.
        let i = if self.atlas {
            let tx = clamp_span(x >> ATLAS_SHIFT, self.x0, self.w);
            let ty = clamp_span(y >> ATLAS_SHIFT, self.y0, self.h);
            let slot = self.table[self.tbase + (ty * self.w + tx) as usize] as usize;
            let (lx, ly) = (x & (ATLAS_TILE - 1), y & (ATLAS_TILE - 1));
            slot * ATLAS_TILE_WORDS + ((ly * ATLAS_TILE + lx) * 2) as usize
        } else {
            let lx = clamp_span(x, self.x0, self.w);
            let ly = clamp_span(y, self.y0, self.h);
            if self.full {
                let i = ((ly * self.w + lx) * 3) as usize;
                return [
                    f32::from_bits(self.data[i]),
                    f32::from_bits(self.data[i + 1]),
                    f32::from_bits(self.data[i + 2]),
                ];
            }
            ((ly * self.w + lx) * 2) as usize
        };
        unpack_rgb_any([self.data[i], self.data[i + 1]])
    }
}

#[cfg(not(target_arch = "spirv"))]
#[inline]
fn unpack_rgb_any(p: [u32; 2]) -> Rgb {
    unpack_rgb(p)
}

#[cfg(target_arch = "spirv")]
#[inline]
fn unpack_rgb_any(p: [u32; 2]) -> Rgb {
    let rg = spirv_std::float::f16x2_to_vec2(p[0]);
    let b = spirv_std::float::f16x2_to_vec2(p[1]);
    [rg.x, rg.y, b.x]
}

// ---------------------------------------------------------------------------------------------
// Parameter ABI: one `f32` storage buffer and one `u32` storage buffer per dispatch.

pub const C1_F32: usize = 6 + 9 + 8;
/// `[tile_w, tile_h, roi_x0, roi_y0, roi_w, roi_h, level_w, level_h, field_w, field_h,
/// c2_active, d_fields, geo_flags, warp_offset, warp_nodes_per_row, color_offset, atlas,
/// full]`, then (if `atlas != 0`) the [`PackedRoi`] slot table; the ROI is then in tiles.
/// `full`: f32 input pixels and f32 output (3 words, plus 5 for the local fields). The mask
/// program follows the C1 floats in the same buffer, then (if `warp_offset != 0`) the warp
/// grid as `[cell, nodes…]` starting at `warp_offset`, then the C1 colour block at
/// `color_offset` (its table offsets are relative to the block).
pub const C1_U32: usize = 18;
pub const OUT_F32: usize = 4 + 9 + 10 + 3 + 2 + 24 + 12 + 2 + 36;
/// `[tile_w, tile_h, grid_x0, grid_y0, seed, curve, color, mixer_on, d3_active, d_fields,
/// look_offset, full]`; `full`: f32 input and 16-bit output ([`encode_out16`], 2 words).
pub const OUT_U32: usize = 12;

/// Sequential writer/reader so that pack and unpack share one field order.
struct W<'a> {
    f: &'a mut [f32],
    i: usize,
}

impl W<'_> {
    fn put(&mut self, v: f32) {
        self.f[self.i] = v;
        self.i += 1;
    }
    fn mat(&mut self, m: &Mat3f) {
        for r in m {
            for &v in r {
                self.put(v);
            }
        }
    }
}

struct R<'a> {
    f: &'a [f32],
    i: usize,
}

impl R<'_> {
    #[inline]
    fn get(&mut self) -> f32 {
        let v = self.f[self.i];
        self.i += 1;
        v
    }
    #[inline]
    fn v3(&mut self) -> [f32; 3] {
        [self.get(), self.get(), self.get()]
    }
    #[inline]
    fn mat(&mut self) -> Mat3f {
        [self.v3(), self.v3(), self.v3()]
    }
}

pub fn pack_c1(m: &Affine, p: &C1Params, c2: &C2Params) -> [f32; C1_F32] {
    let mut f = [0.0; C1_F32];
    let mut w = W { f: &mut f, i: 0 };
    for &v in &m.a {
        w.put(v);
    }
    w.mat(&p.matrix);
    for v in [
        c2.dehaze,
        c2.airlight[0],
        c2.airlight[1],
        c2.airlight[2],
        c2.shadows,
        c2.highlights,
        c2.clarity,
        c2.fscale,
    ] {
        w.put(v);
    }
    f
}

#[inline]
pub fn unpack_c1(f: &[f32], c2_active: u32) -> (Affine, C1Params, C2Params) {
    let mut r = R { f, i: 0 };
    let a = [r.get(), r.get(), r.get(), r.get(), r.get(), r.get()];
    let matrix = r.mat();
    let c2 = C2Params {
        active: c2_active,
        dehaze: r.get(),
        airlight: r.v3(),
        shadows: r.get(),
        highlights: r.get(),
        clarity: r.get(),
        fscale: r.get(),
    };
    (Affine { a }, C1Params { matrix }, c2)
}

pub fn pack_out(p: &OutParams) -> [f32; OUT_F32] {
    let mut f = [0.0; OUT_F32];
    let mut w = W { f: &mut f, i: 0 };
    for v in [p.d1.slope, p.d1.k_pow, p.d1.white, p.d1.blacks] {
        w.put(v);
    }
    w.mat(&p.e1.matrix);
    let e = &p.d3;
    for v in [
        e.cx,
        e.cy,
        e.inv_rx,
        e.inv_ry,
        e.vig_amount,
        e.vig_mid,
        e.vig_width,
        e.grain_amount,
        e.grain_cell,
        e.grain_rough,
    ] {
        w.put(v);
    }
    for v in p.d1_sliders {
        w.put(v);
    }
    let d = &p.d2;
    w.put(d.saturation);
    w.put(d.vibrance);
    for b in d.mixer.iter().chain(d.wheels.iter()) {
        for &v in b {
            w.put(v);
        }
    }
    w.put(d.grade_mid);
    w.put(d.grade_width);
    for m in [&d.to_lms, &d.lms_to_lab, &d.lab_to_lms, &d.lms_to_rgb] {
        w.mat(m);
    }
    debug_assert_eq!(w.i, OUT_F32);
    f
}

pub fn pack_out_u32(
    p: &OutParams,
    w: u32,
    h: u32,
    gx0: u32,
    gy0: u32,
    d_fields: bool,
    full: bool,
) -> [u32; OUT_U32] {
    [
        w,
        h,
        gx0,
        gy0,
        p.seed,
        p.d2.curve,
        p.d2.color,
        p.d2.mixer_on,
        p.d3.active,
        d_fields as u32,
        p.d2.look,
        full as u32,
    ]
}

#[inline]
fn read_head(r: &mut R<'_>, up: &[u32]) -> (D1Params, E1Params, D3Params) {
    let d1 = D1Params {
        slope: r.get(),
        k_pow: r.get(),
        white: r.get(),
        blacks: r.get(),
    };
    let e1 = E1Params { matrix: r.mat() };
    let d3 = D3Params {
        active: up[8],
        cx: r.get(),
        cy: r.get(),
        inv_rx: r.get(),
        inv_ry: r.get(),
        vig_amount: r.get(),
        vig_mid: r.get(),
        vig_width: r.get(),
        grain_amount: r.get(),
        grain_cell: r.get(),
        grain_rough: r.get(),
    };
    (d1, e1, d3)
}

/// D1, E1, D3 and seed only (the head of the layout).
#[inline]
pub fn unpack_out_base(f: &[f32], up: &[u32]) -> (D1Params, E1Params, D3Params, u32) {
    let mut r = R { f, i: 0 };
    let (d1, e1, d3) = read_head(&mut r, up);
    (d1, e1, d3, up[4])
}

#[inline]
pub fn unpack_out(f: &[f32], up: &[u32]) -> OutParams {
    let mut r = R { f, i: 0 };
    let (d1, e1, d3) = read_head(&mut r, up);
    let d1_sliders = r.v3();
    let saturation = r.get();
    let vibrance = r.get();
    let mixer = [
        r.v3(),
        r.v3(),
        r.v3(),
        r.v3(),
        r.v3(),
        r.v3(),
        r.v3(),
        r.v3(),
    ];
    let wheels = [r.v3(), r.v3(), r.v3(), r.v3()];
    let grade_mid = r.get();
    let grade_width = r.get();
    let d2 = D2Params {
        curve: up[5],
        color: up[6],
        mixer_on: up[7],
        look: up[10],
        saturation,
        vibrance,
        mixer,
        wheels,
        grade_mid,
        grade_width,
        to_lms: r.mat(),
        lms_to_lab: r.mat(),
        lab_to_lms: r.mat(),
        lms_to_rgb: r.mat(),
    };
    OutParams {
        d1,
        d2,
        e1,
        d3,
        d1_sliders,
        seed: up[4],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f16_round_trip_matches_half_semantics() {
        for &v in &[
            0.0f32, -0.0, 1.0, 0.18, 65504.0, 1e-5, 6.1e-5, 3.0e-8, -2.5, 1e6,
        ] {
            let back = f16_to_f32(f32_to_f16(v));
            let rel = if v == 0.0 {
                back.abs()
            } else {
                ((back - v) / v).abs()
            };
            if v.abs() <= 65504.0 && v.abs() >= 6.1e-5 {
                assert!(rel < 1e-3, "{v} → {back}");
            }
        }
        assert_eq!(f16_to_f32(f32_to_f16(1e6)), f32::INFINITY);
        // Exhaustive: every finite f16 survives f16→f32→f16.
        for h in 0u32..0x7c00 {
            assert_eq!(f32_to_f16(f16_to_f32(h)), h, "{h:#x}");
        }
    }

    #[test]
    fn packing_round_trips() {
        let mut d2 = crate::pointwise::tests::test_d2(true);
        d2.curve = 1;
        d2.mixer_on = 1;
        d2.mixer[3] = [5.0, 0.1, -0.2];
        d2.wheels[2] = [0.01, 0.02, 0.03];
        let p = OutParams {
            d1: D1Params::new(10.0, -5.0, 3.0),
            d2,
            e1: E1Params {
                matrix: [[1.0, 2.0, 3.0], [4.0, 5.0, 6.0], [7.0, 8.0, 9.0]],
            },
            d3: D3Params::new(-30.0, 40.0, 20.0, 60.0, 30.0, 50.0, 40.0, 0.5, 800.0, 600.0),
            d1_sliders: [10.0, -5.0, 3.0],
            seed: 42,
        };
        assert_eq!(
            unpack_out(&pack_out(&p), &pack_out_u32(&p, 1, 2, 3, 4, false, false)),
            p
        );
        let a = Affine {
            a: [1.0, 0.5, -3.0, 0.25, 2.0, 7.0],
        };
        let c = C1Params {
            matrix: [[0.5; 3]; 3],
        };
        let mut c2 = C2Params::new(10.0, 20.0, -30.0, 40.0);
        c2.airlight = [0.7, 0.8, 0.9];
        c2.fscale = 0.25;
        assert_eq!(unpack_c1(&pack_c1(&a, &c, &c2), 1), (a, c, c2));
    }
}
