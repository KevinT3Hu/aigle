//! A4 highlight reconstruction (doc 02 §7), sparse and keyed on the user's WB.
//!
//! A2 records which CFA sites clipped; after demosaic and orientation that becomes a
//! per-pixel, per-channel mask, dilated twice: radius [`CLIP_R`] marks values contaminated by
//! a clipped site ("clipped"), radius [`RING_R`] marks the surrounding ring used to sample
//! scene chroma. The pre-A4 data of every [`TILE`]² tile touching the mask is kept in
//! [`HlBase`], so A4 can be re-run for another WB or algorithm without re-decoding: the
//! output only differs from the pre-A4 data at clipped pixels, and inputs are always read
//! from the pre-A4 data, which makes the result independent of what was applied before.
//!
//! Reconstruction runs in user-WB camera space `u_c = s_c · user_c / ref_c` (stored data `s`
//! carries the reference WB), where channel `c` clips at `user_c` (green = 1), with the
//! user's multipliers quantised to 1 % ([`HlKey`]). Algorithms (versioned with the decoder):
//! * `Off`: clipped pixels are clipped to the lowest channel clip level (neutral white).
//! * `InpaintOpposed`: each clipped channel becomes at least the cube-root mean of the two
//!   other channels plus the image's mean chroma of that channel, sampled in the ring.
//! * `Guided`: per clipped channel, a local linear model `c ≈ a·g + b` of the channel on the
//!   opposed mean `g`, fitted on unclipped pixels within [`GUIDED_R`]; falls back to the
//!   opposed estimate where too few unclipped pixels are near.
//! * `ColorPropagation`: librtprocess' inpainting, on crops of connected tile clusters with a
//!   [`PROPAGATION_MARGIN`] context margin.
//!
//! Reconstructed values only ever increase a clipped channel (`max`), except `Off`.

use std::collections::HashMap;

use aigle_core::geometry::Size;
use aigle_core::params::HighlightRecon;
use aigle_engine_api::CancelToken;
use aigle_kernels::Rgb;
use aigle_rtprocess_sys::{self as rp, Planes};
use half::f16;
use rayon::prelude::*;
use serde::Serialize;

use super::LoadError;
use super::pyramid::Level;

/// Tile edge of the sparse pre-A4 store.
pub const TILE: u32 = 64;
/// Dilation radius of the clipped mask (demosaic contamination).
pub const CLIP_R: usize = 2;
/// Dilation radius of the chroma-sampling ring.
pub const RING_R: usize = 5;
/// Guided-filter window radius.
pub const GUIDED_R: u32 = 12;
/// Context margin for colour propagation.
pub const PROPAGATION_MARGIN: u32 = 64;

/// Clipped bits (one per channel) of a mask byte.
pub const CLIPPED: u8 = 0b111;
const RING_SHIFT: u8 = 3;

/// Key of an A4 result: algorithm and the user's WB multipliers (R, B; green = 1) in 1 %
/// steps.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct HlKey {
    pub recon: HighlightRecon,
    pub wb: [u32; 2],
}

impl HlKey {
    /// Key for camera multipliers `mult` (green-normalised).
    pub fn new(recon: HighlightRecon, mult: [f64; 3]) -> HlKey {
        let q = |v: f64| (v / mult[1] * 100.0).round().clamp(1.0, 1e6) as u32;
        HlKey {
            recon,
            wb: [q(mult[0]), q(mult[2])],
        }
    }
}

/// Dilate the per-channel site bits (bits 0..2) in place: afterwards bits 0..2 are the
/// clipped mask (radius [`CLIP_R`]) and bits 3..5 the ring (radius [`RING_R`]).
pub fn dilate(mask: &mut [u8], size: Size) {
    let clipped = dilate_box(mask, size, CLIP_R);
    let ring = dilate_box(&clipped, size, RING_R - CLIP_R);
    mask.par_iter_mut()
        .zip(clipped.par_iter().zip(ring.par_iter()))
        .for_each(|(m, (c, r))| *m = (c & CLIPPED) | ((r & CLIPPED) << RING_SHIFT));
}

/// Square bitwise-OR dilation of radius `r`, separable.
fn dilate_box(src: &[u8], size: Size, r: usize) -> Vec<u8> {
    let (w, h) = (size.w as usize, size.h as usize);
    let mut tmp = vec![0u8; src.len()];
    tmp.par_chunks_mut(w)
        .zip(src.par_chunks(w))
        .for_each(|(out, row)| {
            if row.iter().all(|&v| v == 0) {
                return;
            }
            for (x, &v) in row.iter().enumerate() {
                if v != 0 {
                    for o in &mut out[x.saturating_sub(r)..(x + r + 1).min(w)] {
                        *o |= v;
                    }
                }
            }
        });
    let mut out = vec![0u8; src.len()];
    out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for yy in y.saturating_sub(r)..(y + r + 1).min(h) {
            let s = &tmp[yy * w..(yy + 1) * w];
            for (o, v) in row.iter_mut().zip(s) {
                *o |= v;
            }
        }
    });
    out
}

struct HlTile {
    tx: u32,
    ty: u32,
    w: u32,
    h: u32,
    /// Pre-A4 stored values.
    rgb: Vec<[f16; 3]>,
    mask: Vec<u8>,
}

impl HlTile {
    fn x0(&self) -> u32 {
        self.tx * TILE
    }
    fn y0(&self) -> u32 {
        self.ty * TILE
    }
}

/// Pre-A4 data and mask of the tiles touching the dilated clip mask.
pub struct HlBase {
    size: Size,
    /// Reference WB (the stored clip level per channel).
    ref_wb: [f32; 3],
    tiles: Vec<HlTile>,
    index: HashMap<(u32, u32), usize>,
}

/// Axis-aligned rectangle `[x0, x1) × [y0, y1)` in level-0 pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x0: u32,
    pub y0: u32,
    pub x1: u32,
    pub y1: u32,
}

impl Rect {
    fn w(&self) -> u32 {
        self.x1 - self.x0
    }
    fn h(&self) -> u32 {
        self.y1 - self.y0
    }
}

impl HlBase {
    /// Build from level 0 (pre-A4) and the dilated mask; `None` if nothing is clipped.
    pub fn build(level0: &Level, mask: &[u8], ref_wb: [f64; 3]) -> Option<HlBase> {
        let size = Size::new(level0.width, level0.height);
        let (tw, th) = (size.w.div_ceil(TILE), size.h.div_ceil(TILE));
        let w = size.w as usize;
        let tiles: Vec<HlTile> = (0..th)
            .into_par_iter()
            .flat_map_iter(|ty| {
                (0..tw).filter_map(move |tx| {
                    let (x0, y0) = (tx * TILE, ty * TILE);
                    let (cw, ch) = (TILE.min(size.w - x0), TILE.min(size.h - y0));
                    let rows = (y0..y0 + ch).map(|y| y as usize * w + x0 as usize);
                    let any = rows
                        .clone()
                        .any(|i| mask[i..i + cw as usize].iter().any(|&m| m != 0));
                    any.then(|| HlTile {
                        tx,
                        ty,
                        w: cw,
                        h: ch,
                        rgb: rows
                            .clone()
                            .flat_map(|i| level0.data[i..i + cw as usize].iter().copied())
                            .collect(),
                        mask: rows
                            .flat_map(|i| mask[i..i + cw as usize].iter().copied())
                            .collect(),
                    })
                })
            })
            .collect();
        if tiles.is_empty() {
            return None;
        }
        let index = tiles
            .iter()
            .enumerate()
            .map(|(i, t)| ((t.tx, t.ty), i))
            .collect();
        Some(HlBase {
            size,
            ref_wb: ref_wb.map(|v| v as f32),
            tiles,
            index,
        })
    }

    pub fn bytes(&self) -> u64 {
        self.tiles
            .iter()
            .map(|t| t.rgb.len() as u64 * 7)
            .sum::<u64>()
    }

    /// Clipped mask (any channel, dilated) at pyramid level `level` of size `w × h`: a level
    /// pixel is clipped if any level-0 pixel it covers is.
    pub fn mask_at_level(&self, level: usize, w: u32, h: u32) -> Vec<bool> {
        let mut out = vec![false; w as usize * h as usize];
        for t in &self.tiles {
            for y in 0..t.h {
                for x in 0..t.w {
                    if t.mask[(y * t.w + x) as usize] & CLIPPED != 0 {
                        let (lx, ly) = ((t.x0() + x) >> level, (t.y0() + y) >> level);
                        if lx < w && ly < h {
                            out[(ly * w + lx) as usize] = true;
                        }
                    }
                }
            }
        }
        out
    }

    /// Number of clipped pixels (any channel).
    pub fn clipped_pixels(&self) -> usize {
        self.tiles
            .iter()
            .map(|t| t.mask.iter().filter(|&&m| m & CLIPPED != 0).count())
            .sum()
    }

    /// Pre-A4 values and mask of `r`.
    fn extract(&self, level0: &Level, r: Rect) -> (Vec<Rgb>, Vec<u8>) {
        let (w, sw) = (r.w() as usize, self.size.w as usize);
        let mut rgb = vec![[0f32; 3]; w * r.h() as usize];
        let mut mask = vec![0u8; rgb.len()];
        for (yy, row) in rgb.chunks_mut(w).enumerate() {
            let s = (r.y0 as usize + yy) * sw + r.x0 as usize;
            for (o, p) in row.iter_mut().zip(&level0.data[s..s + w]) {
                *o = p.map(f16::to_f32);
            }
        }
        for ty in r.y0 / TILE..r.y1.div_ceil(TILE) {
            for tx in r.x0 / TILE..r.x1.div_ceil(TILE) {
                let Some(&i) = self.index.get(&(tx, ty)) else {
                    continue;
                };
                let t = &self.tiles[i];
                let (ax, bx) = (t.x0().max(r.x0), (t.x0() + t.w).min(r.x1));
                let (ay, by) = (t.y0().max(r.y0), (t.y0() + t.h).min(r.y1));
                for y in ay..by {
                    for x in ax..bx {
                        let ti = ((y - t.y0()) * t.w + (x - t.x0())) as usize;
                        let oi = ((y - r.y0) as usize) * w + (x - r.x0) as usize;
                        rgb[oi] = t.rgb[ti].map(f16::to_f32);
                        mask[oi] = t.mask[ti];
                    }
                }
            }
        }
        (rgb, mask)
    }

    fn tile_rect(&self, t: &HlTile, margin: u32) -> Rect {
        Rect {
            x0: t.x0().saturating_sub(margin),
            y0: t.y0().saturating_sub(margin),
            x1: (t.x0() + t.w + margin).min(self.size.w),
            y1: (t.y0() + t.h + margin).min(self.size.h),
        }
    }
}

/// Reconstruction space for one key.
#[derive(Clone, Copy, Debug)]
struct Space {
    /// stored → user-WB space.
    ratio: [f32; 3],
    /// Clip level per channel in user-WB space.
    clip: [f32; 3],
}

impl Space {
    fn new(base: &HlBase, key: &HlKey) -> Space {
        let r = base.ref_wb;
        let ref_key = HlKey::new(key.recon, r.map(f64::from));
        let ratio = if ref_key.wb == key.wb {
            [1.0; 3]
        } else {
            let user = [key.wb[0] as f32 / 100.0, 1.0, key.wb[1] as f32 / 100.0];
            [user[0] / r[0], user[1] / r[1], user[2] / r[2]]
        };
        Space {
            ratio,
            clip: [r[0] * ratio[0], r[1] * ratio[1], r[2] * ratio[2]],
        }
    }
    fn to_user(self, p: Rgb) -> Rgb {
        [
            p[0] * self.ratio[0],
            p[1] * self.ratio[1],
            p[2] * self.ratio[2],
        ]
    }
    fn to_stored(self, p: Rgb) -> Rgb {
        [
            p[0] / self.ratio[0],
            p[1] / self.ratio[1],
            p[2] / self.ratio[2],
        ]
    }
}

/// Mean of the cube roots of the two channels other than `c`.
#[inline]
fn opposed_cbrt(p: Rgb, c: usize) -> f32 {
    0.5 * (p[(c + 1) % 3].max(0.0).cbrt() + p[(c + 2) % 3].max(0.0).cbrt())
}

/// Mean chroma `∛c − mean(∛others)` per channel over unclipped ring pixels (a cube-root
/// difference, so that it scales roughly with brightness). A channel whose own ring has too
/// few unclipped pixels (it lies inside another channel's clipped area) uses the union of
/// all rings.
fn chroma(base: &HlBase, sp: &Space) -> [f32; 3] {
    const MIN_SAMPLES: u64 = 16;
    type Acc = ([f64; 3], [u64; 3], [f64; 3], u64);
    let add = |a: Acc, b: Acc| -> Acc {
        (
            [0, 1, 2].map(|c| a.0[c] + b.0[c]),
            [0, 1, 2].map(|c| a.1[c] + b.1[c]),
            [0, 1, 2].map(|c| a.2[c] + b.2[c]),
            a.3 + b.3,
        )
    };
    let (own, own_n, any, any_n) = base
        .tiles
        .par_iter()
        .map(|t| {
            let mut acc: Acc = ([0.0; 3], [0; 3], [0.0; 3], 0);
            for (p, &m) in t.rgb.iter().zip(&t.mask) {
                if m & CLIPPED != 0 || m == 0 {
                    continue;
                }
                let u = sp.to_user(p.map(f16::to_f32));
                if (0..3).any(|k| u[k] >= 0.98 * sp.clip[k]) {
                    continue;
                }
                acc.3 += 1;
                for c in 0..3 {
                    let d = (u[c].max(0.0).cbrt() - opposed_cbrt(u, c)) as f64;
                    acc.2[c] += d;
                    if m & (1 << (c as u8 + RING_SHIFT)) != 0 {
                        acc.0[c] += d;
                        acc.1[c] += 1;
                    }
                }
            }
            acc
        })
        .reduce(|| ([0.0; 3], [0; 3], [0.0; 3], 0), add);
    [0, 1, 2].map(|c| {
        if own_n[c] >= MIN_SAMPLES {
            (own[c] / own_n[c] as f64) as f32
        } else if any_n > 0 {
            (any[c] / any_n as f64) as f32
        } else {
            0.0
        }
    })
}

/// Opposed estimate of one user-space pixel.
#[inline]
fn opposed_px(u: Rgb, m: u8, chroma: &[f32; 3]) -> Rgb {
    let mut o = u;
    for c in 0..3 {
        if m & (1 << c) != 0 {
            o[c] = u[c].max((opposed_cbrt(u, c) + chroma[c]).max(0.0).powi(3));
        }
    }
    o
}

/// Separable box sum of radius `r` over a `w × h` plane (clamped at the borders).
fn box_sum(src: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let mut tmp = vec![0f32; src.len()];
    for y in 0..h {
        let row = &src[y * w..(y + 1) * w];
        let mut acc: f32 = row[..(r + 1).min(w)].iter().sum();
        for x in 0..w {
            tmp[y * w + x] = acc;
            if x + r + 1 < w {
                acc += row[x + r + 1];
            }
            if x >= r {
                acc -= row[x - r];
            }
        }
    }
    let mut out = vec![0f32; src.len()];
    for x in 0..w {
        let mut acc: f32 = (0..(r + 1).min(h)).map(|y| tmp[y * w + x]).sum();
        for y in 0..h {
            out[y * w + x] = acc;
            if y + r + 1 < h {
                acc += tmp[(y + r + 1) * w + x];
            }
            if y >= r {
                acc -= tmp[(y - r) * w + x];
            }
        }
    }
    out
}

/// Guided estimate for the clipped pixels of a crop (user space `u`, opposed estimate
/// `est`). For channel `c`, three local linear models `c ≈ a·g + b` are fitted on unclipped
/// pixels, with guide `g` = each other channel alone or their cube-root mean; each clipped
/// pixel uses the model whose guide channels are unclipped there (the opposed estimate if
/// both are clipped, or where too few unclipped pixels are near).
fn guided(est: &[Rgb], u: &[Rgb], mask: &[u8], w: usize, h: usize, sp: &Space) -> Vec<Rgb> {
    const MIN_W: f32 = 16.0;
    let r = GUIDED_R as usize;
    let n = est.len();
    let mut out = est.to_vec();
    let eps = (2e-3 * sp.clip.iter().cloned().fold(0.0, f32::max)).powi(2);
    let valid: Vec<bool> = (0..n)
        .map(|i| mask[i] & CLIPPED == 0 && (0..3).all(|k| u[i][k] < 0.98 * sp.clip[k]))
        .collect();
    for c in 0..3 {
        if !mask.iter().any(|&m| m & (1 << c) != 0) {
            continue;
        }
        let (o1, o2) = ((c + 1) % 3, (c + 2) % 3);
        // Guide per model: 0 = o1, 1 = o2, 2 = cube-root mean of both.
        let guide = |p: Rgb, m: usize| match m {
            0 => p[o1].max(0.0),
            1 => p[o2].max(0.0),
            _ => opposed_cbrt(p, c).powi(3),
        };
        for model in 0..3 {
            let uses = |bits: u8| match model {
                0 => bits & (1 << o1) == 0 && bits & (1 << o2) != 0,
                1 => bits & (1 << o2) == 0 && bits & (1 << o1) != 0,
                _ => bits & ((1 << o1) | (1 << o2)) == 0,
            };
            if !(0..n).any(|i| mask[i] & (1 << c) != 0 && uses(mask[i])) {
                continue;
            }
            let mut planes = [(); 5].map(|_| vec![0f32; n]);
            for i in (0..n).filter(|&i| valid[i]) {
                let (g, v) = (guide(u[i], model), u[i][c]);
                planes[0][i] = 1.0;
                planes[1][i] = g;
                planes[2][i] = v;
                planes[3][i] = g * g;
                planes[4][i] = g * v;
            }
            let [wt, sg, sc, sgg, sgc] = planes.map(|p| box_sum(&p, w, h, r));
            for i in 0..n {
                if mask[i] & (1 << c) == 0 || !uses(mask[i]) || wt[i] < 1.0 {
                    continue;
                }
                let wsum = wt[i];
                let (mg, mc) = (sg[i] / wsum, sc[i] / wsum);
                let vg = (sgg[i] / wsum - mg * mg).max(0.0);
                let cov = sgc[i] / wsum - mg * mc;
                let a = (cov / (vg + eps)).clamp(0.0, 8.0);
                let pred = (a * guide(u[i], model) + mc - a * mg).min(16.0 * sp.clip[c]);
                let t = (wsum / MIN_W).min(1.0);
                out[i][c] = u[i][c].max(t * pred + (1.0 - t) * est[i][c]);
            }
        }
    }
    out
}

/// Run A4 for `key` on `level0` (whose pixels outside the clipped mask must be pre-A4).
/// Every clipped pixel is rewritten. Returns the touched tile rectangles.
pub fn apply(
    level0: &mut Level,
    base: &HlBase,
    key: &HlKey,
    cancel: Option<&CancelToken>,
) -> Result<Vec<Rect>, LoadError> {
    let sp = Space::new(base, key);
    let cancelled = || cancel.is_some_and(|c| c.is_cancelled());
    // Per-tile results: (tile index, stored values of the tile's pixels).
    let results: Vec<(usize, Vec<Rgb>)> = match key.recon {
        HighlightRecon::Off => {
            let lim = sp.clip.iter().cloned().fold(f32::MAX, f32::min);
            base.tiles
                .par_iter()
                .enumerate()
                .map(|(i, t)| {
                    let v = t
                        .rgb
                        .iter()
                        .zip(&t.mask)
                        .map(|(p, &m)| {
                            let s = p.map(f16::to_f32);
                            if m & CLIPPED != 0 {
                                sp.to_stored(sp.to_user(s).map(|v| v.min(lim)))
                            } else {
                                s
                            }
                        })
                        .collect();
                    (i, v)
                })
                .collect()
        }
        HighlightRecon::InpaintOpposed => {
            let ch = chroma(base, &sp);
            base.tiles
                .par_iter()
                .enumerate()
                .map(|(i, t)| {
                    let v = t
                        .rgb
                        .iter()
                        .zip(&t.mask)
                        .map(|(p, &m)| {
                            let s = p.map(f16::to_f32);
                            sp.to_stored(opposed_px(sp.to_user(s), m, &ch))
                        })
                        .collect();
                    (i, v)
                })
                .collect()
        }
        HighlightRecon::Guided => {
            let ch = chroma(base, &sp);
            let lv: &Level = level0;
            base.tiles
                .par_iter()
                .enumerate()
                .map(|(i, t)| {
                    if cancelled() {
                        return (i, Vec::new());
                    }
                    let r = base.tile_rect(t, GUIDED_R);
                    let (rgb, mask) = base.extract(lv, r);
                    let u: Vec<Rgb> = rgb.iter().map(|&p| sp.to_user(p)).collect();
                    let est: Vec<Rgb> = u
                        .iter()
                        .zip(&mask)
                        .map(|(&p, &m)| opposed_px(p, m, &ch))
                        .collect();
                    let g = guided(&est, &u, &mask, r.w() as usize, r.h() as usize, &sp);
                    let (ox, oy) = (t.x0() - r.x0, t.y0() - r.y0);
                    let mut v = Vec::with_capacity(t.rgb.len());
                    for y in 0..t.h {
                        for x in 0..t.w {
                            let ci = ((y + oy) * r.w() + x + ox) as usize;
                            v.push(sp.to_stored(g[ci]));
                        }
                    }
                    (i, v)
                })
                .collect()
        }
        HighlightRecon::ColorPropagation => propagate(level0, base, &sp, &cancelled)?,
    };
    if cancelled() {
        return Err(LoadError::Cancelled);
    }
    let w = base.size.w as usize;
    let mut rects = Vec::new();
    for (i, v) in results {
        let t = &base.tiles[i];
        let mut any = false;
        for y in 0..t.h {
            for x in 0..t.w {
                let ti = (y * t.w + x) as usize;
                if t.mask[ti] & CLIPPED != 0 {
                    level0.data[(t.y0() + y) as usize * w + (t.x0() + x) as usize] =
                        v[ti].map(f16::from_f32);
                    any = true;
                }
            }
        }
        if any {
            rects.push(base.tile_rect(t, 0));
        }
    }
    Ok(rects)
}

/// Colour propagation on clusters of tiles that are within two margins of each other.
fn propagate(
    level0: &Level,
    base: &HlBase,
    sp: &Space,
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<Vec<(usize, Vec<Rgb>)>, LoadError> {
    let reach = (2 * PROPAGATION_MARGIN).div_ceil(TILE) as i64 + 1;
    let mut cluster = vec![usize::MAX; base.tiles.len()];
    let mut clusters: Vec<Vec<usize>> = Vec::new();
    for start in 0..base.tiles.len() {
        if cluster[start] != usize::MAX {
            continue;
        }
        let id = clusters.len();
        let mut members = vec![start];
        cluster[start] = id;
        let mut k = 0;
        while k < members.len() {
            let t = &base.tiles[members[k]];
            for dy in -reach..=reach {
                for dx in -reach..=reach {
                    let (x, y) = (t.tx as i64 + dx, t.ty as i64 + dy);
                    if x < 0 || y < 0 {
                        continue;
                    }
                    if let Some(&j) = base.index.get(&(x as u32, y as u32))
                        && cluster[j] == usize::MAX
                    {
                        cluster[j] = id;
                        members.push(j);
                    }
                }
            }
            k += 1;
        }
        clusters.push(members);
    }
    let fac = 65535.0 / sp.clip.iter().cloned().fold(0.0, f32::max);
    let mut out = Vec::with_capacity(base.tiles.len());
    for members in clusters {
        if cancelled() {
            return Err(LoadError::Cancelled);
        }
        let mut r = base.tile_rect(&base.tiles[members[0]], PROPAGATION_MARGIN);
        for &m in &members[1..] {
            let q = base.tile_rect(&base.tiles[m], PROPAGATION_MARGIN);
            r = Rect {
                x0: r.x0.min(q.x0),
                y0: r.y0.min(q.y0),
                x1: r.x1.max(q.x1),
                y1: r.y1.max(q.y1),
            };
        }
        let (rgb, _) = base.extract(level0, r);
        let n = rgb.len();
        let mut p = Planes {
            r: Vec::with_capacity(n),
            g: Vec::with_capacity(n),
            b: Vec::with_capacity(n),
        };
        for &px in &rgb {
            let u = sp.to_user(px);
            p.r.push(u[0].max(0.0) * fac);
            p.g.push(u[1].max(0.0) * fac);
            p.b.push(u[2].max(0.0) * fac);
        }
        let chmax = [&p.r, &p.g, &p.b].map(|v| v.iter().cloned().fold(0.0, f32::max));
        let clip = sp.clip.map(|c| c * fac);
        if let Err(e) = rp::highlight_inpaint(r.w(), r.h(), &mut p, chmax, clip) {
            tracing::warn!("highlight reconstruction failed: {e}");
        }
        for &m in &members {
            let t = &base.tiles[m];
            let (ox, oy) = (t.x0() - r.x0, t.y0() - r.y0);
            let mut v = Vec::with_capacity(t.rgb.len());
            for y in 0..t.h {
                for x in 0..t.w {
                    let ci = ((y + oy) * r.w() + x + ox) as usize;
                    v.push(sp.to_stored([p.r[ci] / fac, p.g[ci] / fac, p.b[ci] / fac]));
                }
            }
            out.push((m, v));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A grey-ish orange disc (camera space, reference WB `ref_wb` premultiplied) whose red
    /// and green channels clip in the centre.
    fn scene(ref_wb: [f64; 3]) -> (Level, Vec<u8>, Vec<Rgb>) {
        let (w, h) = (160u32, 128u32);
        let mut truth = Vec::new();
        for y in 0..h {
            for x in 0..w {
                let d = ((x as f32 - 80.0).powi(2) + (y as f32 - 64.0).powi(2)).sqrt();
                let l = 1.6 * (-d * d / 1800.0).exp() + 0.05;
                // Scene colour (user/neutral space): orange.
                let c = [l * 1.0, l * 0.7, l * 0.45];
                truth.push([
                    c[0] * ref_wb[0] as f32 / 1.0,
                    c[1],
                    c[2] * ref_wb[2] as f32 / 1.0,
                ]);
            }
        }
        // Clip in camera space: camera = stored / ref; clips at 1.0.
        let clip = ref_wb.map(|v| v as f32);
        let mut mask = vec![0u8; truth.len()];
        let rgb: Vec<Rgb> = truth
            .iter()
            .zip(&mut mask)
            .map(|(p, m)| {
                let mut o = *p;
                for c in 0..3 {
                    if p[c] >= clip[c] {
                        o[c] = clip[c];
                        *m |= 1 << c;
                    }
                }
                o
            })
            .collect();
        let size = Size::new(w, h);
        dilate(&mut mask, size);
        (Level::from_f32(w, h, &rgb), mask, truth)
    }

    fn err(level: &Level, truth: &[Rgb], mask: &[u8]) -> f32 {
        let mut e = 0.0;
        let mut n = 0.0;
        for i in 0..truth.len() {
            if mask[i] & CLIPPED != 0 {
                let p = level.data[i].map(f16::to_f32);
                // Hue error: ratios to green.
                let t = truth[i];
                e += ((p[0] / p[1]) - (t[0] / t[1])).abs() + ((p[2] / p[1]) - (t[2] / t[1])).abs();
                n += 1.0;
            }
        }
        e / n
    }

    #[test]
    fn dilation_marks_clipped_and_ring() {
        let size = Size::new(20, 20);
        let mut m = vec![0u8; 400];
        m[10 * 20 + 10] = 0b001;
        dilate(&mut m, size);
        assert_eq!(m[10 * 20 + 12] & CLIPPED, 0b001);
        assert_eq!(m[10 * 20 + 13] & CLIPPED, 0);
        assert_ne!(m[10 * 20 + 15] & (CLIPPED << RING_SHIFT), 0);
        assert_eq!(m[10 * 20 + 16], 0);
    }

    #[test]
    fn algorithms_restore_hue_and_rerun_is_independent() {
        let ref_wb = [2.0, 1.0, 1.5];
        let (level, mask, truth) = scene(ref_wb);
        let base = HlBase::build(&level, &mask, ref_wb).unwrap();
        assert!(base.clipped_pixels() > 100);
        let clipped_err = err(&level, &truth, &mask);
        let mut errs = HashMap::new();
        for recon in [
            HighlightRecon::Off,
            HighlightRecon::InpaintOpposed,
            HighlightRecon::Guided,
            HighlightRecon::ColorPropagation,
        ] {
            let key = HlKey::new(recon, ref_wb);
            let mut a = Level {
                width: level.width,
                height: level.height,
                data: level.data.clone(),
            };
            let rects = apply(&mut a, &base, &key, None).unwrap();
            assert!(!rects.is_empty());
            // Unclipped pixels are untouched.
            for (i, m) in mask.iter().enumerate() {
                if m & CLIPPED == 0 {
                    assert_eq!(a.data[i], level.data[i]);
                }
            }
            // Re-running on a result (another key first) gives the same output.
            let mut b = Level {
                width: level.width,
                height: level.height,
                data: level.data.clone(),
            };
            apply(&mut b, &base, &HlKey::new(recon, [1.3, 1.0, 2.2]), None).unwrap();
            apply(&mut b, &base, &key, None).unwrap();
            assert!(a.data == b.data, "{recon:?} depends on history");
            errs.insert(recon, err(&a, &truth, &mask));
        }
        // Reconstruction beats plain clipping; opposed/guided recover the hue closely.
        for r in [HighlightRecon::InpaintOpposed, HighlightRecon::Guided] {
            assert!(
                errs[&r] < 0.5 * clipped_err,
                "{r:?}: {errs:?} vs {clipped_err}"
            );
        }
        assert!(
            errs[&HighlightRecon::ColorPropagation] < clipped_err,
            "{errs:?}"
        );
        assert!(
            errs[&HighlightRecon::Guided] <= errs[&HighlightRecon::InpaintOpposed] * 1.05,
            "{errs:?}"
        );
    }

    #[test]
    fn off_clips_to_neutral_under_the_user_wb() {
        let ref_wb = [2.0, 1.0, 1.5];
        let (level, mask, _) = scene(ref_wb);
        let base = HlBase::build(&level, &mask, ref_wb).unwrap();
        let user = [2.6, 1.0, 1.2];
        let key = HlKey::new(HighlightRecon::Off, user);
        let mut a = Level {
            width: level.width,
            height: level.height,
            data: level.data.clone(),
        };
        apply(&mut a, &base, &key, None).unwrap();
        // Centre pixel (red and green clipped): both at the lowest user-space clip level
        // (green, 1.0), blue unchanged.
        let i = 64 * 160 + 80;
        let p = a.data[i].map(f16::to_f32);
        let u = [0, 1, 2].map(|c| p[c] / ref_wb[c] as f32 * user[c] as f32);
        assert!(
            (u[0] - 1.0).abs() < 0.01 && (u[1] - 1.0).abs() < 0.01,
            "{u:?}"
        );
        assert_eq!(a.data[i][2], level.data[i][2]);
    }

    #[test]
    fn key_quantises_to_one_percent() {
        let a = HlKey::new(HighlightRecon::Guided, [2.004, 1.0, 1.496]);
        let b = HlKey::new(HighlightRecon::Guided, [4.0, 2.0, 3.0]);
        assert_eq!(a, b);
        assert_eq!(a.wb, [200, 150]);
    }
}
