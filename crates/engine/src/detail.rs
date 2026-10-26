//! S tier: source-grid detail (doc 02 §3, §8) — noise reduction, capture sharpening, texture.
//!
//! Runs on the pyramid level that B1 samples, in reference-WB camera RGB, before geometry.
//! The level is processed in square *blocks* (doc 02 §5.2 "source ROIs"); each block reads a
//! margin that covers the total support of the active ops, with mirror boundaries at the
//! level edges, so a block's interior equals the result of processing the whole (mirror-
//! extended) level. Blocks are cached by content key and shared by every tile and view.
//!
//! Per block, in order:
//! 1. Opponent split: `Y = (R + 2G + B)/4`, `Cr = R − G`, `Cb = B − G` (near-neutral under the
//!    reference WB, so independent of later user WB, doc 02 §8).
//! 2. **Luma NR**: 5×5 bilateral on the variance-stabilised luma `z = √(Y + c)`. The range
//!    scale is the level's noise σ in `z`; `c` and σ come from the source's noise model
//!    (`crate::noise`: DNG NoiseProfile, else measured from the level), which also makes the
//!    strength scale with the pyramid level.
//! 3. **Chroma NR**: guided filter of `Cr`, `Cb` with `z` as guide (edges follow luma).
//! 4. **Capture sharpening**: Richardson–Lucy deconvolution of `Y` with a Gaussian PSF
//!    (4 iterations), halo-limited by `detail`, gated by a noise-normalised edge mask.
//! 5. **Texture**: log-luminance band boost around a 4 px Gaussian.
//!
//! Scale awareness: radii are in level-0 source px and shrink by `2^-L`. Below about half a
//! pixel, sharpening fades out and texture/chroma radii are floored, so renders below 1:1 are
//! approximate by design (doc 02 §8).
//!
//! Execution: [`plan`] turns a block into a sequence of passes over `f32` planes whose
//! per-pixel functions live in `aigle_kernels::detail`; [`run_cpu`] runs them with rayon and
//! the GPU backend as `s_*` SPIR-V entry points (`Backend::s_blocks`, which shares a batch
//! between CPU and GPU). Box means are direct sums in `f32` so that both agree.
//!
//! TODO(doc 02 §8): raw-domain chroma NR.

use half::f16;
use rayon::prelude::*;

use crate::backend::LevelView;
use crate::noise::NoiseLevel;
use crate::pipeline::color::C1Color;
use aigle_core::EditParams;
use aigle_kernels::detail::*;
use aigle_kernels::resample::{Rgb2D, mirror};

use crate::source::Level;

/// Implementation version of the S block product (doc 02 §13).
pub const S_IMPL: u32 = 3;
const RL_ITER: u32 = 4;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SParams {
    pub luma: f32,
    pub luma_detail: f32,
    pub luma_contrast: f32,
    pub chroma: f32,
    pub chroma_detail: f32,
    pub sharpen: f32,
    pub radius: f32,
    pub sharpen_detail: f32,
    pub masking: f32,
    pub texture: f32,
    /// Largest local deltas any pixel can receive from masks: positive noise, |sharpness|,
    /// |texture| (doc 02 §9.2: `noise`/`sharpness` fields live on the Source grid).
    pub local: [f32; 3],
}

impl SParams {
    pub fn of(p: &EditParams) -> SParams {
        let n = &p.detail.noise;
        let s = &p.detail.sharpen;
        SParams {
            luma: n.luma,
            luma_detail: n.detail,
            luma_contrast: n.contrast,
            chroma: n.chroma,
            chroma_detail: n.chroma_detail,
            sharpen: s.amount,
            radius: s.radius,
            sharpen_detail: s.detail,
            masking: s.masking,
            texture: p.presence.texture,
            local: {
                let mut l = [0f32; 3];
                for m in crate::masks::relevant(p, crate::masks::Tier::S) {
                    let a = &m.adjust;
                    l[0] += a.noise.unwrap_or(0.0).max(0.0) * m.opacity;
                    l[1] += a.sharpness.unwrap_or(0.0).abs() * m.opacity;
                    l[2] += a.texture.unwrap_or(0.0).abs() * m.opacity;
                }
                l
            },
        }
    }

    pub fn is_identity(&self) -> bool {
        !self.run_luma() && self.chroma == 0.0 && !self.run_sharpen() && !self.run_texture()
    }

    pub fn has_locals(&self) -> bool {
        self.local.iter().any(|v| *v != 0.0)
    }

    fn run_luma(&self) -> bool {
        self.luma > 0.0 || self.local[0] > 0.0
    }

    fn run_sharpen(&self) -> bool {
        self.sharpen > 0.0 || self.local[1] > 0.0
    }

    fn run_texture(&self) -> bool {
        self.texture != 0.0 || self.local[2] > 0.0
    }

    fn scale(level: usize) -> f32 {
        0.5f32.powi(level as i32)
    }

    fn chroma_radius(&self, level: usize) -> u32 {
        let r0 = 2.0 + 6.0 * self.chroma / 100.0;
        ((r0 * Self::scale(level)).round() as u32).max(1)
    }

    fn sharpen_sigma(&self, level: usize) -> Option<f32> {
        let s = self.radius * Self::scale(level);
        (self.run_sharpen() && s >= 0.25).then_some(s)
    }

    fn texture_sigma(level: usize) -> f32 {
        (4.0 * Self::scale(level)).max(0.6)
    }

    /// Total support of the active ops at `level`, in level px.
    pub fn margin(&self, level: usize) -> u32 {
        let mut m = 1;
        if self.run_luma() {
            m += 2;
        }
        if self.chroma > 0.0 {
            m += 2 * self.chroma_radius(level);
        }
        if let Some(s) = self.sharpen_sigma(level) {
            m += 2 * RL_ITER * gauss_radius(s) as u32 + 1;
        }
        if self.run_texture() {
            m += gauss_radius(Self::texture_sigma(level)) as u32;
        }
        m
    }

    /// Block edge (doc 02 §5.3): the smallest power of two ≥ 256 whose margin overhead
    /// `(T + 2m)² / T²` is at most 2, as long as the block's workspace stays within
    /// [`MAX_BLOCK_WORK`]; above that the overhead is accepted.
    ///
    /// The doc's bound is 1.6. Blocks also cost coverage: a viewport needs whole blocks, so
    /// larger blocks compute more pixels outside it. At 1:1 on a 24 MP raw with the default
    /// detail settings (margin 36 px), 512 px blocks made the S tier 20 % slower than 256 px
    /// blocks at overhead 1.64; the bound of 2 keeps 256 there.
    pub fn block_size(&self, level: usize) -> u32 {
        let m = self.margin(level) as u64;
        let fits = |t: u64| (t + 2 * m).pow(2) * PLANES_EST * 4 <= MAX_BLOCK_WORK;
        let mut t = 256u64;
        while (t + 2 * m).pow(2) > 2 * t * t && fits(2 * t) {
            t *= 2;
        }
        t as u32
    }
}

/// Upper bound of the planes a block plan holds at once (checked by a test).
const PLANES_EST: u64 = 24;
/// Workspace bytes per block that the block size may grow to.
pub const MAX_BLOCK_WORK: u64 = 48 << 20;

fn gauss_radius(sigma: f32) -> usize {
    (3.0 * sigma).ceil().max(1.0) as usize
}

/// One pass of a block plan (`aigle_kernels::detail`).
#[derive(Clone, Debug)]
pub struct SPass {
    pub up: [u32; UP_LEN],
    /// Never empty (bound as a storage buffer on the GPU).
    pub fp: Vec<f32>,
}

/// The S tier of one block as passes over a workspace of `planes` planes of `rw × rh`
/// pixels. Built by [`plan`]; run by [`run_cpu`] or the GPU backend, which execute the same
/// per-pixel functions.
#[derive(Clone, Debug)]
pub struct SPlan {
    pub rw: u32,
    pub rh: u32,
    pub planes: u32,
    pub passes: Vec<SPass>,
    /// First of three planes holding the interleaved RGB of the `bw × bh` interior.
    pub out: u32,
    pub bw: u32,
    pub bh: u32,
    /// Level rectangle (x0, y0, w, h) read by the source passes, mirror boundaries folded in.
    pub src_rect: (u32, u32, u32, u32),
}

impl SPlan {
    pub fn plane_len(&self) -> usize {
        self.rw as usize * self.rh as usize
    }

    /// Workspace size in `f32`s.
    pub fn work_len(&self) -> usize {
        self.planes as usize * self.plane_len()
    }

    /// Output (start, len) in the workspace, in `f32`s.
    pub fn out_range(&self) -> (usize, usize) {
        (
            self.out as usize * self.plane_len(),
            3 * self.bw as usize * self.bh as usize,
        )
    }
}

/// Plane allocator and pass recorder.
struct Builder {
    rw: u32,
    rh: u32,
    next: u32,
    free: Vec<u32>,
    passes: Vec<SPass>,
}

impl Builder {
    fn alloc(&mut self) -> u32 {
        self.free.pop().unwrap_or_else(|| {
            self.next += 1;
            self.next - 1
        })
    }

    fn release(&mut self, p: u32) {
        if p != NONE {
            debug_assert!(!self.free.contains(&p), "plane {p} released twice");
            self.free.push(p);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn pass_grid(
        &mut self,
        op: u32,
        ins: &[u32],
        outs: &[u32],
        abc: [u32; 3],
        fp: Vec<f32>,
        gw: u32,
        gh: u32,
    ) {
        let mut up = [NONE; UP_LEN];
        up[U_OP] = op;
        up[U_RW] = self.rw;
        up[U_RH] = self.rh;
        up[U_IN..U_IN + ins.len()].copy_from_slice(ins);
        up[U_OUT..U_OUT + outs.len()].copy_from_slice(outs);
        up[U_GW] = gw;
        up[U_GH] = gh;
        up[U_A] = abc[0];
        up[U_B] = abc[1];
        up[U_C] = abc[2];
        up[U_ROI..U_ROI + 7].fill(0);
        let fp = if fp.is_empty() { vec![0.0] } else { fp };
        self.passes.push(SPass { up, fp });
    }

    fn pass(&mut self, op: u32, ins: &[u32], outs: &[u32], abc: [u32; 3], fp: Vec<f32>) {
        let (w, h) = (self.rw, self.rh);
        self.pass_grid(op, ins, outs, abc, fp, w, h);
    }

    /// Gaussian of `src` into `dst` with a post-op reading `post_in` (see `OP_GAUSS_V`).
    fn gauss(&mut self, src: u32, dst: u32, sigma: f32, post: u32, post_in: u32) {
        debug_assert_ne!(src, dst);
        let r = gauss_radius(sigma);
        let k: Vec<f32> = (0..=r)
            .map(|i| (-(i as f32).powi(2) / (2.0 * sigma * sigma)).exp())
            .collect();
        let norm = k[0] + 2.0 * k[1..].iter().sum::<f32>();
        let k: Vec<f32> = k.iter().map(|v| v / norm).collect();
        let t = self.alloc();
        self.pass(
            OP_GAUSS_H,
            &[src],
            &[t],
            [r as u32, POST_NONE, 0],
            k.clone(),
        );
        self.pass(OP_GAUSS_V, &[t, post_in], &[dst], [r as u32, post, 0], k);
        self.release(t);
    }

    /// Box means of radius `r` of up to four planes, into new planes.
    fn box_means(&mut self, ins: &[u32], r: u32) -> Vec<u32> {
        let n = ins.len() as u32;
        let t: Vec<u32> = ins.iter().map(|_| self.alloc()).collect();
        self.pass(OP_BOX_H, ins, &t, [r, n, 0], Vec::new());
        let out: Vec<u32> = ins.iter().map(|_| self.alloc()).collect();
        self.pass(OP_BOX_V, &t, &out, [r, n, 0], Vec::new());
        for p in t {
            self.release(p);
        }
        out
    }
}

/// Smallest range covering `mirror(a .. a + len)` on an axis of `n` px: (start, len).
fn fold(a: i32, len: u32, n: u32) -> (u32, u32) {
    let (mut lo, mut hi) = (u32::MAX, 0);
    for i in a..a + len as i32 {
        let v = mirror(i, n);
        lo = lo.min(v);
        hi = hi.max(v);
    }
    (lo, hi - lo + 1)
}

/// Plan the S tier for the `bw × bh` block at (`bx0`, `by0`) of a `lw × lh` level (index `l`)
/// with the level's noise `noise` (`crate::noise`) and, with local adjustments, the S-tier
/// mask program and C1 colour (the program is evaluated on C1 colour at level positions).
#[allow(clippy::too_many_arguments)]
pub fn plan(
    lw: u32,
    lh: u32,
    l: usize,
    sp: &SParams,
    noise: NoiseLevel,
    bx0: u32,
    by0: u32,
    bw: u32,
    bh: u32,
    local: Option<(&[f32], &C1Color)>,
) -> SPlan {
    let m = sp.margin(l);
    let (rw, rh) = (bw + 2 * m, bh + 2 * m);
    let (ox, oy) = (bx0 as i32 - m as i32, by0 as i32 - m as i32);
    let mut b = Builder {
        rw,
        rh,
        next: 0,
        free: Vec::new(),
        passes: Vec::new(),
    };
    let (y, cr, cb, z0) = (b.alloc(), b.alloc(), b.alloc(), b.alloc());
    let (oxu, oyu) = (ox as u32, oy as u32);
    b.pass(
        OP_SPLIT,
        &[],
        &[y, cr, cb, z0],
        [oxu, oyu, 0],
        vec![noise.offset],
    );
    // Local fields, indexed by `crate::masks::S_*`.
    let mut field = [NONE; 3];
    if let Some((prog, c1)) = local {
        field = [b.alloc(), b.alloc(), b.alloc()];
        let mut fp: Vec<f32> = c1.params.matrix.iter().flatten().copied().collect();
        fp.extend_from_slice(prog);
        let c = fp.len() as u32;
        fp.extend_from_slice(&c1.block);
        b.pass(OP_FIELDS, &[], &field, [oxu, oyu, c], fp);
    }
    use crate::masks::{S_NOISE, S_SHARPNESS, S_TEXTURE};
    let sz = noise.sigma.max(1e-4);
    let off = noise.offset;

    // Luma NR.
    let (z, ynr) = if sp.run_luma() {
        let mut fp = vec![
            sz,
            sp.luma,
            sp.luma_detail,
            0.5 * sp.luma_contrast / 100.0,
            off,
        ];
        for j in 0..5 {
            for i in 0..5 {
                let d2 = ((i as f32 - 2.0).powi(2) + (j as f32 - 2.0).powi(2)) / (2.0 * 1.2 * 1.2);
                fp.push((-d2).exp());
            }
        }
        let (z2, yn) = (b.alloc(), b.alloc());
        b.pass(OP_BILATERAL, &[z0, field[S_NOISE]], &[z2, yn], [0; 3], fp);
        b.release(z0);
        b.release(y);
        (z2, yn)
    } else {
        (z0, y)
    };
    b.release(field[S_NOISE]);

    // Chroma NR: guided filter of Cr, Cb with z as guide.
    if sp.chroma > 0.0 {
        let rad = sp.chroma_radius(l);
        let eps = (sz * (1.0 + 3.0 * (1.0 - sp.chroma_detail / 100.0))).powi(2);
        let wc = (sp.chroma / 25.0).min(1.0);
        let p = [b.alloc(), b.alloc(), b.alloc()];
        b.pass(OP_PROD, &[z, cr, cb], &p, [0; 3], Vec::new());
        let m1 = b.box_means(&[z, cr, cb], rad);
        let m2 = b.box_means(&p, rad);
        for q in p {
            b.release(q);
        }
        let (ar, br) = (b.alloc(), b.alloc());
        b.pass(
            OP_COEF,
            &[m1[0], m1[1], m2[0], m2[2]],
            &[ar, br],
            [0; 3],
            vec![eps],
        );
        let (ab, bb) = (b.alloc(), b.alloc());
        b.pass(
            OP_COEF,
            &[m1[0], m1[2], m2[1], m2[2]],
            &[ab, bb],
            [0; 3],
            vec![eps],
        );
        for q in m1.into_iter().chain(m2) {
            b.release(q);
        }
        let mc = b.box_means(&[ar, br, ab, bb], rad);
        for q in [ar, br, ab, bb] {
            b.release(q);
        }
        b.pass(OP_APPLY, &[z, mc[0], mc[1], cr], &[cr], [0; 3], vec![wc]);
        b.pass(OP_APPLY, &[z, mc[2], mc[3], cb], &[cb], [0; 3], vec![wc]);
        for q in mc {
            b.release(q);
        }
    }

    // Capture sharpening: Richardson–Lucy, then the halo-limited, edge-masked combine.
    let mut ys = ynr;
    if let Some(s) = sp.sharpen_sigma(l) {
        let fade = (s / 0.5).min(1.0);
        let est = b.alloc();
        b.pass(OP_MAX0, &[ynr], &[est], [0; 3], Vec::new());
        let ratio = b.alloc();
        for _ in 0..RL_ITER {
            b.gauss(est, ratio, s, POST_RATIO, ynr);
            b.gauss(ratio, est, s, POST_MUL, est);
        }
        b.release(ratio);
        let lim = 0.1 + 0.9 * sp.sharpen_detail / 100.0;
        let t = sp.masking / 100.0 * 8.0;
        let out = b.alloc();
        b.pass(
            OP_SHARPEN,
            &[ynr, est, z, field[S_SHARPNESS]],
            &[out],
            [0; 3],
            vec![sp.sharpen, fade, lim, t, sz],
        );
        b.release(est);
        ys = out;
    }
    b.release(field[S_SHARPNESS]);

    // Texture: log-luminance band boost.
    if sp.run_texture() {
        let ly = b.alloc();
        b.pass(OP_LOG, &[ys], &[ly], [0; 3], Vec::new());
        let low = b.alloc();
        b.gauss(ly, low, SParams::texture_sigma(l), POST_NONE, NONE);
        let out = b.alloc();
        b.pass(
            OP_TEXTURE,
            &[ly, low, field[S_TEXTURE]],
            &[out],
            [0; 3],
            vec![sp.texture],
        );
        b.release(ly);
        b.release(low);
        if ys != ynr {
            b.release(ys);
        }
        ys = out;
    }

    // Rebuild RGB; chroma follows the luma change of sharpening/texture.
    let out = b.next;
    b.next += 3;
    b.pass_grid(
        OP_REBUILD,
        &[ynr, ys, cr, cb],
        &[out],
        [m, 0, 0],
        Vec::new(),
        bw,
        bh,
    );
    let (x0, w) = fold(ox, rw, lw);
    let (y0, h) = fold(oy, rh, lh);
    SPlan {
        rw,
        rh,
        planes: b.next,
        passes: b.passes,
        out,
        bw,
        bh,
        src_rect: (x0, y0, w, h),
    }
}

/// Run `plan` on the CPU over the level `src` (addressed in full-level coordinates); the
/// block interior in f32.
pub fn run_cpu<R: Rgb2D + Sync>(plan: &SPlan, src: &R) -> Vec<[f32; 3]> {
    let n = plan.plane_len();
    let mut work = vec![0f32; plan.work_len()];
    for p in &plan.passes {
        let (gw, gh) = (p.up[U_GW] as usize, p.up[U_GH] as usize);
        let res: Vec<[f32; 4]> = (0..gw * gh)
            .into_par_iter()
            .map(|i| s_px(&work, src, &p.fp, &p.up, (i % gw) as u32, (i / gw) as u32))
            .collect();
        if p.up[U_OP] == OP_REBUILD {
            let o = p.up[U_OUT] as usize * n;
            work[o..o + 3 * gw * gh]
                .par_chunks_mut(3)
                .zip(&res)
                .for_each(|(d, v)| d.copy_from_slice(&v[..3]));
            continue;
        }
        for k in 0..4 {
            let pl = p.up[U_OUT + k];
            if pl == NONE {
                continue;
            }
            let o = pl as usize * n;
            work[o..o + n]
                .par_iter_mut()
                .zip(&res)
                .for_each(|(d, v)| *d = v[k]);
        }
    }
    let (o, len) = plan.out_range();
    work[o..o + len].as_chunks::<3>().0.to_vec()
}

/// Process the `bw × bh` block at (`bx0`, `by0`) of `level` (index `l`) on the CPU.
#[allow(clippy::too_many_arguments)]
pub fn process_block(
    level: &Level,
    l: usize,
    sp: &SParams,
    noise: NoiseLevel,
    bx0: u32,
    by0: u32,
    bw: u32,
    bh: u32,
    local: Option<(&[f32], &C1Color)>,
) -> Vec<[f16; 3]> {
    let p = plan(
        level.width,
        level.height,
        l,
        sp,
        noise,
        bx0,
        by0,
        bw,
        bh,
        local,
    );
    run_cpu(&p, &LevelView::whole(level))
        .into_iter()
        .map(|c| c.map(f16::from_f32))
        .collect()
}

/// Box mean of radius `r`, window clipped at the region edges (prefix sums in f64).
pub(crate) fn box_mean(v: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let mut rows = vec![0f32; w * h];
    rows.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let src = &v[y * w..(y + 1) * w];
        let mut pre = vec![0f64; w + 1];
        for x in 0..w {
            pre[x + 1] = pre[x] + src[x] as f64;
        }
        for (x, o) in row.iter_mut().enumerate() {
            let a = x.saturating_sub(r);
            let b = (x + r + 1).min(w);
            *o = ((pre[b] - pre[a]) / (b - a) as f64) as f32;
        }
    });
    let mut out = vec![0f32; w * h];
    // Column pass, processed in column strips for parallelism.
    let cols: Vec<Vec<f32>> = (0..w)
        .into_par_iter()
        .map(|x| {
            let mut pre = vec![0f64; h + 1];
            for y in 0..h {
                pre[y + 1] = pre[y] + rows[y * w + x] as f64;
            }
            (0..h)
                .map(|y| {
                    let a = y.saturating_sub(r);
                    let b = (y + r + 1).min(h);
                    ((pre[b] - pre[a]) / (b - a) as f64) as f32
                })
                .collect()
        })
        .collect();
    for (x, col) in cols.iter().enumerate() {
        for (y, &v) in col.iter().enumerate() {
            out[y * w + x] = v;
        }
    }
    out
}

/// Guided filter (He et al.) of `p` with guide `i`: returns the smoothed (mean a, mean b).
pub(crate) fn guided_coeffs(
    i: &[f32],
    p: &[f32],
    w: usize,
    h: usize,
    r: usize,
    eps: f32,
) -> (Vec<f32>, Vec<f32>) {
    let mi = box_mean(i, w, h, r);
    let mp = box_mean(p, w, h, r);
    let ip: Vec<f32> = i.iter().zip(p).map(|(a, b)| a * b).collect();
    let ii: Vec<f32> = i.iter().map(|a| a * a).collect();
    let mip = box_mean(&ip, w, h, r);
    let mii = box_mean(&ii, w, h, r);
    let (a, b): (Vec<f32>, Vec<f32>) = (0..w * h)
        .into_par_iter()
        .map(|k| {
            let var = (mii[k] - mi[k] * mi[k]).max(0.0);
            let cov = mip[k] - mi[k] * mp[k];
            let a = cov / (var + eps);
            (a, mp[k] - a * mi[k])
        })
        .unzip();
    (box_mean(&a, w, h, r), box_mean(&b, w, h, r))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn noisy_level(w: u32, h: u32, sigma: f32) -> Level {
        let rgb: Vec<[f32; 3]> = (0..w * h)
            .map(|i| {
                let (x, y) = (i % w, i / w);
                let base = if (x / 16 + y / 16) % 2 == 0 { 0.1 } else { 0.4 };
                let n = |s: u32| {
                    let h = aigle_kernels::dither::noise(x, y, s)
                        + aigle_kernels::dither::noise(x, y, s + 7)
                        - 1.0;
                    h * sigma * 2.45
                };
                [base + n(1), base + n(2), base + n(3)]
            })
            .collect();
        Level::from_f32(w, h, &rgb)
    }

    fn estimate_noise(l: &Level) -> NoiseLevel {
        crate::noise::measure(l)
    }

    fn params() -> SParams {
        SParams::of(&EditParams::default())
    }

    #[test]
    fn noise_estimate_tracks_noise() {
        let quiet = estimate_noise(&noisy_level(256, 256, 0.002)).sigma;
        let loud = estimate_noise(&noisy_level(256, 256, 0.02)).sigma;
        assert!(loud > 4.0 * quiet, "{quiet} {loud}");
    }

    #[test]
    fn blocks_are_seam_free() {
        // Processing two adjacent blocks equals processing one block covering both.
        let lvl = noisy_level(200, 120, 0.01);
        let mut sp = params();
        sp.luma = 60.0;
        sp.texture = 40.0;
        sp.masking = 20.0;
        let sigma = estimate_noise(&lvl);
        let whole = process_block(&lvl, 0, &sp, sigma, 0, 0, 200, 120, None);
        let left = process_block(&lvl, 0, &sp, sigma, 0, 0, 100, 120, None);
        let right = process_block(&lvl, 0, &sp, sigma, 100, 0, 100, 120, None);
        for y in 0..120usize {
            for x in 0..200usize {
                let b = if x < 100 {
                    left[y * 100 + x]
                } else {
                    right[y * 100 + x - 100]
                };
                assert_eq!(whole[y * 200 + x], b, "({x},{y})");
            }
        }
    }

    #[test]
    fn nr_reduces_noise_and_keeps_mean() {
        let lvl = noisy_level(128, 128, 0.02);
        let sigma = estimate_noise(&lvl);
        let mut sp = params();
        sp.sharpen = 0.0;
        sp.luma = 80.0;
        sp.chroma = 50.0;
        let out = process_block(&lvl, 0, &sp, sigma, 0, 0, 128, 128, None);
        // Variance inside one flat 16×16 square (excluding its border).
        let stats = |f: &dyn Fn(usize) -> f32| {
            let px: Vec<f32> = (2..14)
                .flat_map(|y| (2..14).map(move |x| y * 128 + x))
                .map(f)
                .collect();
            let m = px.iter().sum::<f32>() / px.len() as f32;
            (
                m,
                px.iter().map(|v| (v - m).powi(2)).sum::<f32>() / px.len() as f32,
            )
        };
        let (m0, v0) = stats(&|k| lvl.data[k][1].to_f32());
        let (m1, v1) = stats(&|k| out[k][1].to_f32());
        assert!(v1 < 0.3 * v0, "{v0} → {v1}");
        assert!((m1 - m0).abs() < 0.01, "{m0} → {m1}");
    }

    #[test]
    fn plans_read_only_written_planes() {
        // The GPU workspace is pooled and not cleared, and stencil passes must not write a
        // plane they read at neighbouring pixels.
        let mut sp = params();
        sp.luma = 50.0;
        sp.chroma = 40.0;
        sp.sharpen = 60.0;
        sp.masking = 30.0;
        sp.texture = 20.0;
        sp.local = [10.0, 10.0, 10.0];
        let noise = NoiseLevel {
            sigma: 0.01,
            offset: 1e-3,
        };
        let c1 = C1Color {
            params: aigle_kernels::pointwise::C1Params {
                matrix: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            },
            block: vec![0.0],
            dcp_ok: true,
        };
        let prog = [0.0, 0.0];
        for local in [None, Some((&prog[..], &c1))] {
            let p = plan(300, 200, 0, &sp, noise, 256, 0, 44, 200, local);
            let mut written = std::collections::HashSet::new();
            for pass in &p.passes {
                let up = &pass.up;
                let ins: Vec<u32> = up[U_IN..U_IN + 4]
                    .iter()
                    .copied()
                    .filter(|&v| v != NONE)
                    .collect();
                for i in &ins {
                    assert!(
                        written.contains(i),
                        "op {} reads unwritten plane {i}",
                        up[U_OP]
                    );
                }
                let stencil: &[u32] = match up[U_OP] {
                    OP_BILATERAL | OP_GAUSS_H | OP_GAUSS_V => &ins[..1],
                    OP_BOX_H | OP_BOX_V => &ins,
                    OP_SHARPEN => &ins[2..3],
                    _ => &[],
                };
                let n = if up[U_OP] == OP_REBUILD { 3 } else { 4 };
                for k in 0..n {
                    let o = if up[U_OP] == OP_REBUILD {
                        up[U_OUT] + k
                    } else {
                        up[U_OUT + k as usize]
                    };
                    if o == NONE {
                        continue;
                    }
                    assert!(
                        !stencil.contains(&o),
                        "op {} writes its stencil input",
                        up[U_OP]
                    );
                    assert!(o < p.planes);
                    written.insert(o);
                }
            }
            assert_eq!(p.passes.last().unwrap().up[U_OP], OP_REBUILD);
            assert!(p.planes as u64 <= PLANES_EST, "{} planes", p.planes);
        }
    }

    #[test]
    fn block_size_follows_margin() {
        let mut sp = params();
        sp.sharpen = 0.0;
        sp.chroma = 0.0;
        sp.texture = 0.0;
        sp.luma = 0.0;
        assert_eq!(sp.block_size(0), 256);
        sp.sharpen = 100.0;
        sp.radius = 3.0;
        sp.chroma = 100.0;
        sp.texture = 50.0;
        let m = sp.margin(0);
        let t = sp.block_size(0);
        assert!(m > 53 && t == 512, "margin {m} → block {t}");
        // Default settings at 1:1 stay at 256.
        let mut d = params();
        d.luma = 20.0;
        assert_eq!(d.block_size(0), 256, "margin {}", d.margin(0));
        // Coarser levels shrink the radii and the block.
        assert_eq!(sp.block_size(3), 256);
    }

    #[test]
    fn sharpening_increases_edge_contrast() {
        let (w, h) = (64u32, 32u32);
        let rgb: Vec<[f32; 3]> = (0..w * h)
            .map(|i| {
                let x = (i % w) as f32;
                let v = 0.1 + 0.3 / (1.0 + (-(x - 32.0) / 1.5).exp());
                [v; 3]
            })
            .collect();
        let lvl = Level::from_f32(w, h, &rgb);
        let mut sp = params();
        sp.chroma = 0.0;
        sp.sharpen = 100.0;
        sp.sharpen_detail = 100.0;
        let quiet = NoiseLevel {
            sigma: 0.0,
            offset: crate::noise::E,
        };
        let out = process_block(&lvl, 0, &sp, quiet, 0, 0, w, h, None);
        let at = |d: &dyn Fn(usize) -> f32, x: usize| d(16 * w as usize + x);
        let before = at(&|k| lvl.data[k][1].to_f32(), 33) - at(&|k| lvl.data[k][1].to_f32(), 31);
        let after = at(&|k| out[k][1].to_f32(), 33) - at(&|k| out[k][1].to_f32(), 31);
        assert!(after > before * 1.1, "{before} → {after}");
    }
}
