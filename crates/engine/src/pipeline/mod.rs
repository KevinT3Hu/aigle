//! Demand-driven, tiled evaluation of tiers B–E (doc 02 §3, §5).
//!
//! A render request is a Crop-space region and an output size. The engine picks a quantised
//! render scale `s_k = 2^(k/2)` ≥ the zoom (one step lower per Draft offset), renders the
//! Crop-grid tiles that intersect the region (cached by content key), and E2 resamples them to
//! the exact viewport.
//!
//! Products, as in doc 03 §4.3:
//! * **S blocks** (`crate::detail`): NR, capture sharpening and texture on the source level,
//!   cached per block and assembled into the source ROI each C1 tile samples;
//! * **C2 field** ([`c2`]): coarse large-support coefficients, cached per source and params;
//! * **C1 tile** (B1 resample ∘ C1 ∘ C2), RGB f16, cached — the base for most slider changes;
//! * **output tile** (D1 ∘ D2 ∘ D3 ∘ E1), BGRA8, cached.
//!
//! Local adjustments (M) are compiled per tile from the edit (`crate::masks`).
//!
//! Not implemented yet: margins/adaptive tile size (§5.3; C2 needs no margins because its
//! support lives in the coarse field).

pub mod c2;
pub mod color;
pub mod curves;
pub mod keys;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use aigle_core::EditParams;
use aigle_core::geometry::{GeometryMap, ImagePoint, ImageRect, Orientation, Point, Size};
use aigle_core::params::EffectsParams;
use aigle_engine_api::{CancelToken, DisplayProfileId, Overlays};
use aigle_kernels::Rgb;
use aigle_kernels::c2::C2Params;
use aigle_kernels::d3::D3Params;
use aigle_kernels::pointwise::{D1Params, D2Params, E1Params};
use aigle_kernels::resample::bilinear;
use aigle_kernels::tiles::{self, Affine, CLIP_GAMUT, CLIP_HIGH, CLIP_LOW, OutParams};
use rayon::prelude::*;

use crate::backend::{
    Aux, Backend, C1Data, C1Job, DeviceCell, DeviceSrc, FieldRef, LevelView, OutJob, PxVec, SJob,
    StaleDeviceData, TileGeo,
};
use crate::cache::{CacheKey, KeyBuilder, TileCache};
use crate::detail::{self, SParams};
use crate::ledger::MemClass;
use crate::masks;
use crate::retouch;
use crate::source::SourceImage;

/// Output-kernel aux data: the blue-noise dither texture, then the curve tables if any.
pub fn output_aux(curves: Option<&[f32]>) -> Vec<f32> {
    static BLUE: std::sync::OnceLock<Vec<f32>> = std::sync::OnceLock::new();
    let blue = BLUE.get_or_init(|| aigle_kernels::dither::blue_noise().to_vec());
    let mut v = blue.clone();
    if let Some(c) = curves {
        v.extend_from_slice(c);
    }
    v
}

/// Crop-grid tile edge in grid px (doc 02 §5.2).
pub const TILE: u32 = 512;

/// Warp-grid node spacing in grid px (doc 02 §5.5).
pub const WARP_CELL: u32 = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cancelled;

pub struct RenderCtx<'a> {
    pub cache: &'a TileCache,
    pub backend: &'a Backend,
    /// Cache owner (session id) for release on close.
    pub owner: u64,
    /// Full-precision level 0 (16-bit export, doc 03 §9): level-0 renders read it, and every
    /// product is computed and stored in f32, with 16-bit output tiles.
    pub full: Option<&'a crate::source::Level32>,
}

impl RenderCtx<'_> {
    fn full_at(&self, level: usize) -> bool {
        self.full.is_some() && level == 0
    }

    /// The source pixels of `level`.
    fn view<'s>(&'s self, src: &'s SourceImage, level: usize) -> LevelView<'s> {
        match self.full {
            Some(l) if level == 0 => LevelView::whole32(l),
            _ => LevelView::whole(&src.pyramid.levels[level]),
        }
    }
}

/// Everything derived once from (source, params, display) for a render.
pub struct Prepared {
    pub gm: GeometryMap,
    /// Stored (EXIF-oriented) pyramid → Source space including `orientation_delta`.
    delta: Orientation,
    stored_size: Size,
    c1: color::C1Color,
    c2: C2Params,
    /// Which local-adjustment groups the masks use.
    pub uses: masks::Uses,
    /// The edit, for per-tile mask compilation.
    edit: EditParams,
    s: SParams,
    d1: D1Params,
    d2: D2Params,
    e1: E1Params,
    effects: EffectsParams,
    /// Curve tables (`3 × CURVE_N`), or a 1-element placeholder.
    lut: Aux,
    s_key: CacheKey,
    /// Retouching (spots, red-eye) on top of the S tier, if any.
    r_key: Option<CacheKey>,
    c1_key: CacheKey,
    out_key: CacheKey,
    seed: u32,
    /// False when the edit references a pinned lens profile that cannot be found.
    pub lens_ok: bool,
    /// All referenced data (lens profile, DCP, look) was found. Missing data renders without
    /// it and never shares tiles with the complete render.
    pub data_ok: bool,
}

impl Prepared {
    /// `params` must already be sanitised.
    pub fn new(src: &SourceImage, params: &EditParams, display: &DisplayProfileId) -> Prepared {
        let stored_size = src.size();
        let delta = Orientation::from_delta(params.geometry.orientation_delta);
        let source_size = delta.apply_size(stored_size);
        let (optics, lens_ok) = crate::lens::optics(src.info.camera.as_ref(), params, source_size);
        let gm = GeometryMap::with_optics(source_size, &params.geometry, &optics);
        let l = &params.light;
        let pr = &params.presence;
        let lut = curves::build(&params.curve);
        let c1 = color::c1(&src.color, params);
        let (look, look_ok) = color::look(params);
        let mut d2 = color::d2_params(params, lut.is_some());
        let mut aux = output_aux(lut.as_deref());
        if let Some(l) = &look {
            d2.look = aux.len() as u32;
            aux.extend_from_slice(l);
        }
        let e1 = crate::output_space::e1_params(display, &mut aux);
        let s_key = keys::s_key(&src.a_key, params);
        Prepared {
            gm,
            delta,
            stored_size,
            c1_key: KeyBuilder::new("C1L")
                .key(&keys::c1_key(&src.a_key, params))
                .u32(lens_ok as u32)
                .u32(c1.dcp_ok as u32)
                .finish(),
            out_key: KeyBuilder::new("OUTL")
                .key(&keys::out_key(params, display))
                .u32(look_ok as u32)
                .finish(),
            data_ok: lens_ok && c1.dcp_ok && look_ok,
            c1,
            c2: {
                let mut c = C2Params::new(pr.dehaze, l.shadows, l.highlights, pr.clarity);
                let u = masks::uses(params);
                c.active |= (u.tone || u.clarity || u.dehaze) as u32;
                c
            },
            uses: masks::uses(params),
            edit: params.clone(),
            s: SParams::of(params),
            effects: params.effects.clone(),
            d1: D1Params::new(l.contrast, l.whites, l.blacks),
            d2,
            lut: Aux::new(aux),
            r_key: crate::retouch::is_active(params).then(|| keys::r_key(&s_key, params)),
            s_key,
            e1,
            lens_ok,
            seed: u32::from_le_bytes(src.a_key.0[..4].try_into().expect("4 bytes")),
        }
    }

    /// Crop-space extent in level-0 source px.
    pub fn crop_extent(&self) -> (f64, f64) {
        self.gm.crop_extent()
    }

    /// Soft-proof the output for `proof` (the view renders to `display`).
    pub fn set_proof(&mut self, display: &DisplayProfileId, proof: Option<&DisplayProfileId>) {
        let Some(proof) = proof else { return };
        let mut aux = std::mem::take(&mut self.lut.data);
        self.e1.proof = aux.len() as u32;
        aux.extend(crate::output_space::proof_matrices(display, proof));
        self.lut = Aux::new(aux);
        self.out_key = KeyBuilder::new("PROOF")
            .key(&self.out_key)
            .str(&format!("{proof:?}"))
            .finish();
    }

    /// Stable key describing the full render state (for `Analysis::rev_key`).
    pub fn state_key(&self) -> CacheKey {
        KeyBuilder::new("state")
            .key(&self.c1_key)
            .key(&self.out_key)
            .finish()
    }

    /// Output-tile parameters at render scale `s` (D3 is defined on the crop in grid px).
    pub fn out_params(&self, s: f64) -> OutParams {
        let (cw, ch) = self.crop_extent();
        let e = &self.effects;
        let (v, g) = (&e.vignette, &e.grain);
        let d3 = D3Params::new(
            v.amount,
            v.midpoint,
            v.roundness,
            v.feather,
            g.amount,
            g.size,
            g.roughness,
            s as f32,
            (cw * s) as f32,
            (ch * s) as f32,
        );
        let l = &self.edit.light;
        OutParams {
            d1: self.d1,
            d2: self.d2,
            e1: self.e1,
            d3,
            d1_sliders: [l.contrast, l.whites, l.blacks],
            seed: self.seed,
        }
    }

    /// Affine map from tile-local grid pixels of the tile at grid (`x0`, `y0`) to pixel-centre
    /// coordinates of pyramid `level` (see `aigle_kernels::tiles::Affine`). Exact when B1 is
    /// rotation + crop + orientation only; built in f64 relative to the tile origin.
    pub fn tile_affine(&self, s: f64, level: usize, x0: u32, y0: u32) -> Affine {
        let inv_l = 0.5f64.powi(level as i32);
        let at = |fx: f64, fy: f64| {
            let p = self.crop_to_stored(Point::new((x0 as f64 + fx) / s, (y0 as f64 + fy) / s));
            (p.x * inv_l - 0.5, p.y * inv_l - 0.5)
        };
        let (cu, cv) = at(0.0, 0.0);
        let (xu, xv) = at(1.0, 0.0);
        let (yu, yv) = at(0.0, 1.0);
        Affine {
            a: [
                (xu - cu) as f32,
                (yu - cu) as f32,
                cu as f32,
                (xv - cv) as f32,
                (yv - cv) as f32,
                cv as f32,
            ],
        }
    }

    /// Pyramid level to sample at render scale `s`: the correction's fill zoom magnifies the
    /// source, so a finer level is needed.
    pub fn level_for_scale(&self, src: &SourceImage, s: f64) -> usize {
        src.pyramid.level_for_scale(s * self.gm.fill())
    }

    /// B1 geometry of the `w × h` tile at grid (`x0`, `y0`), scale `s`, sampling `level`
    /// (doc 02 §5.5). Affine when there is no lens or perspective correction; otherwise a warp
    /// grid with a node every [`WARP_CELL`] grid px holding the R, G, B level positions, the
    /// vignetting gain and the sampling footprint (√|det J| in level px per grid px).
    pub fn tile_geo(
        &self,
        s: f64,
        level: usize,
        (x0, y0): (u32, u32),
        (w, h): (u32, u32),
        lanczos: bool,
    ) -> TileGeo {
        let affine = self.tile_affine(s, level, x0, y0);
        let mut flags = if lanczos { tiles::GEO_LANCZOS } else { 0 };
        if self.gm.is_affine() {
            return TileGeo::affine(affine, flags);
        }
        if self.gm.has_tca() {
            flags |= tiles::GEO_TCA;
        }
        if self.gm.may_leave_source() {
            flags |= tiles::GEO_CLIP;
        }
        let inv_l = 0.5f64.powi(level as i32);
        let inv = self.delta.inverse();
        let size = self.gm.source_size();
        let (warp, wn) = warp_grid(w, h, |gx, gy| {
            let (ps, gain) = self
                .gm
                .crop_to_source_rgb(Point::new((x0 as f64 + gx) / s, (y0 as f64 + gy) / s));
            let lv = ps.map(|p| {
                let q = inv.apply_point(p, size);
                (q.x * inv_l - 0.5, q.y * inv_l - 0.5)
            });
            (lv, gain)
        });
        TileGeo {
            affine,
            warp,
            wn,
            flags,
        }
    }

    /// Whether C1 must read S ROIs instead of the source level.
    fn s_on(&self) -> bool {
        !self.s.is_identity() || self.r_key.is_some()
    }

    /// Level → normalised Source map for mask programs.
    pub fn level_map(&self, level: usize) -> masks::LevelMap {
        masks::LevelMap::new(self.stored_size, self.delta, level)
    }

    /// The S-tier mask program for `level`, culled to the level rectangle.
    pub fn s_program(&self, level: usize, x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<f32> {
        let map = self.level_map(level);
        let b = map.bounds(x0, y0, x1, y1);
        masks::compile(&self.edit, masks::Tier::S, &map, Some(&b))
    }

    /// Stored (pyramid) coordinates → Crop space.
    pub fn stored_to_crop(&self, p: Point) -> Point {
        self.gm
            .source_to_crop(self.delta.apply_point(p, self.stored_size))
    }

    fn crop_to_stored(&self, q: ImagePoint) -> Point {
        let p = self.gm.crop_to_source(q);
        self.delta.inverse().apply_point(p, self.gm.source_size())
    }

    /// Stored points sampled for R, G, B at Crop point `q`, and the vignetting gain.
    pub(crate) fn crop_to_stored_rgb(&self, q: ImagePoint) -> ([Point; 3], f64) {
        let (ps, gain) = self.gm.crop_to_source_rgb(q);
        let inv = self.delta.inverse();
        (ps.map(|p| inv.apply_point(p, self.gm.source_size())), gain)
    }
}

#[derive(Clone, Debug)]
pub struct RenderTarget {
    /// Crop-space region shown by the output.
    pub region: ImageRect,
    pub out: Size,
    /// Quantised steps below the target render scale (Draft budget, doc 05 §4.2).
    pub scale_offset: i32,
    pub overlays: Overlays,
    /// Resample with Lanczos-3 (Refined / Full / export); bilinear otherwise (Draft).
    pub lanczos: bool,
}

pub struct Rendered {
    pub width: u32,
    pub height: u32,
    /// BGRA8, premultiplied-free; alpha 0 outside the crop.
    pub data: Vec<u8>,
    /// Render scale actually used (grid px per level-0 source px).
    pub render_scale: f64,
    pub elapsed: std::time::Duration,
}

/// Quantised render scale: smallest `2^(k/2)` ≥ `zoom`, minus `offset` steps.
pub fn quantized_scale(zoom: f64, offset: i32) -> f64 {
    let k = (2.0 * zoom.log2() - 1e-9).ceil() as i32 - offset;
    2f64.powf(k as f64 / 2.0)
}

struct C1Tile {
    w: u32,
    h: u32,
    data: C1Data,
}

struct OutTile {
    w: u32,
    /// BGRA8, or RGBA16 (little-endian, alpha = clip flags) for 16-bit output.
    data: Vec<u8>,
    bpp: usize,
}

impl OutTile {
    fn bytes_per_px(&self) -> usize {
        self.bpp
    }
}

/// One processed S block of a level.
struct SBlock {
    w: u32,
    data: PxVec,
}

/// S-processed pixels of a level rectangle: exactly the [`TileGeo::roi`] of one C1 tile.
struct SRoi {
    x0: u32,
    y0: u32,
    w: u32,
    h: u32,
    data: PxVec,
    device: DeviceCell,
}

/// Source ROIs for C1 tiles, after the S tier and retouching (doc 02 §5.2 "source ROIs").
fn s_rois(
    ctx: &RenderCtx<'_>,
    src: &SourceImage,
    prep: &Prepared,
    level: usize,
    rects: &[(u32, u32, u32, u32)],
) -> Vec<Arc<SRoi>> {
    let Some(r_key) = &prep.r_key else {
        return s_base(ctx, src, prep, level, rects, true);
    };
    let view = ctx.view(src, level);
    let (lw, lh) = (view.full_w, view.full_h);
    let rt = retouch::Retouch::new(&prep.edit, &prep.level_map(level), level);
    // Rectangles no spot or eye reaches are the S ROIs themselves.
    let touched: Vec<bool> = rects.iter().map(|&r| rt.touches(r, lw, lh)).collect();
    let plain: Vec<_> = (0..rects.len())
        .filter(|&i| !touched[i])
        .map(|i| rects[i])
        .collect();
    let mut plain = s_base(ctx, src, prep, level, &plain, true).into_iter();
    let full = ctx.full_at(level);
    let key = |r: &(u32, u32, u32, u32)| {
        KeyBuilder::new("Rroi")
            .key(r_key)
            .u32(level as u32)
            .u32(full as u32)
            .u32(r.0)
            .u32(r.1)
            .u32(r.2)
            .u32(r.3)
            .finish()
    };
    let mut out: Vec<Option<Arc<SRoi>>> = rects
        .iter()
        .zip(&touched)
        .map(|(r, &t)| {
            if t {
                ctx.cache.get::<SRoi>(&key(r))
            } else {
                plain.next()
            }
        })
        .collect();
    let miss: Vec<usize> = (0..rects.len()).filter(|&i| out[i].is_none()).collect();
    if miss.is_empty() {
        return out.into_iter().map(|o| o.expect("cached")).collect();
    }
    let mrects: Vec<_> = miss.iter().map(|&i| rects[i]).collect();
    let patches: Vec<Arc<retouch::Patch>> = rt
        .spots
        .iter()
        .filter_map(|sp| {
            let rect = sp.rect(lw, lh)?;
            mrects
                .iter()
                .any(|&r| retouch::intersects(rect, r))
                .then(|| spot_patch(ctx, src, prep, level, sp, rect))
        })
        .collect();
    let base = s_base(ctx, src, prep, level, &mrects, false);
    for ((&i, r), b) in miss.iter().zip(&mrects).zip(base) {
        let mut data = b.data.clone();
        for p in &patches {
            p.apply(*r, &mut data);
        }
        rt.apply_eyes(*r, &mut data);
        let roi = Arc::new(SRoi {
            x0: r.0,
            y0: r.1,
            w: r.2,
            h: r.3,
            data,
            device: DeviceCell::default(),
        });
        insert_roi(ctx, key(r), &roi);
        out[i] = Some(roi);
    }
    out.into_iter().map(|o| o.expect("filled")).collect()
}

/// One spot rendered over its whole disc from the un-retouched S pixels, cached per level.
fn spot_patch(
    ctx: &RenderCtx<'_>,
    src: &SourceImage,
    prep: &Prepared,
    level: usize,
    sp: &retouch::LevelSpot,
    rect: (u32, u32, u32, u32),
) -> Arc<retouch::Patch> {
    let full = ctx.full_at(level);
    let key = KeyBuilder::new("Rpatch")
        .u32(retouch::RETOUCH_IMPL)
        .key(&prep.s_key)
        .u32(level as u32)
        .u32(full as u32)
        .str(&format!("{sp:?}"))
        .finish();
    if let Some(p) = ctx.cache.get::<retouch::Patch>(&key) {
        return p;
    }
    let t0 = Instant::now();
    let view = ctx.view(src, level);
    let (lw, lh) = (view.full_w, view.full_h);
    let t = sp.target_read(lw, lh).unwrap_or(rect);
    let s = sp.source_read(lw, lh);
    let rois = s_base(ctx, src, prep, level, &[t, s], false);
    let plane = |i: usize| retouch::Plane {
        rect: (rois[i].x0, rois[i].y0, rois[i].w, rois[i].h),
        data: rois[i].data.as_px(),
    };
    let p = Arc::new(sp.patch(rect, &plane(0), &plane(1)));
    ctx.cache.insert(
        key,
        p.clone(),
        p.bytes(),
        MemClass::StageTile,
        t0.elapsed(),
        ctx.owner,
    );
    p
}

fn insert_roi(ctx: &RenderCtx<'_>, key: CacheKey, roi: &Arc<SRoi>) {
    let device_words = (roi.data.len() * roi.data.as_px().gpu_words() * 4) as u64;
    ctx.cache.insert(
        key,
        roi.clone(),
        // Host pixels, plus their device copy on the GPU (counted in the host pool also on a
        // discrete GPU: one entry has one pool).
        roi.data.bytes()
            + if ctx.backend.kind() == aigle_engine_api::BackendKind::Gpu {
                device_words
            } else {
                0
            },
        MemClass::StageTile,
        std::time::Duration::from_micros(50),
        ctx.owner,
    );
}

/// S-processed pixels of level rectangles. Blocks are cached and shared between neighbouring
/// tiles and views; the ROIs too when `keep`. Without S ops, the source pixels.
fn s_base(
    ctx: &RenderCtx<'_>,
    src: &SourceImage,
    prep: &Prepared,
    level: usize,
    rects: &[(u32, u32, u32, u32)],
    keep: bool,
) -> Vec<Arc<SRoi>> {
    let view = ctx.view(src, level);
    let lvl = (view.full_w, view.full_h);
    let full = ctx.full_at(level);
    if prep.s.is_identity() {
        return rects
            .iter()
            .map(|&(x0, y0, w, h)| {
                let mut data = PxVec::zeros(w as usize * h as usize, full);
                for j in 0..h {
                    let s0 = ((y0 + j) * view.w + x0) as usize;
                    data.copy_from_px((j * w) as usize, view.data, s0, w as usize);
                }
                Arc::new(SRoi {
                    x0,
                    y0,
                    w,
                    h,
                    data,
                    device: DeviceCell::default(),
                })
            })
            .collect();
    }
    let bs = prep.s.block_size(level);
    let roi_key = |r: &(u32, u32, u32, u32)| {
        KeyBuilder::new("Sroi")
            .u32(detail::S_IMPL)
            .key(&prep.s_key)
            .u32(level as u32)
            .u32(full as u32)
            .u32(r.0)
            .u32(r.1)
            .u32(r.2)
            .u32(r.3)
            .finish()
    };
    let block_key = |bx: u32, by: u32| {
        KeyBuilder::new("Sblock")
            .u32(detail::S_IMPL)
            .key(&prep.s_key)
            .u32(level as u32)
            .u32(full as u32)
            .u32(bs)
            .u32(bx)
            .u32(by)
            .finish()
    };
    let mut out: Vec<Option<Arc<SRoi>>> = rects
        .iter()
        .map(|r| ctx.cache.get::<SRoi>(&roi_key(r)))
        .collect();
    let mut want = std::collections::BTreeSet::new();
    for (r, o) in rects.iter().zip(&out) {
        if o.is_none() {
            for by in r.1 / bs..=(r.1 + r.3 - 1) / bs {
                for bx in r.0 / bs..=(r.0 + r.2 - 1) / bs {
                    want.insert((bx, by));
                }
            }
        }
    }
    if want.is_empty() {
        return out.into_iter().map(|o| o.expect("cached")).collect();
    }
    let mut blocks: HashMap<(u32, u32), Arc<SBlock>> = HashMap::new();
    let mut todo = Vec::new();
    for &(bx, by) in &want {
        match ctx.cache.get::<SBlock>(&block_key(bx, by)) {
            Some(b) => {
                blocks.insert((bx, by), b);
            }
            None => todo.push((bx, by)),
        }
    }
    // Blocks are computed once across concurrent renders (`TileCache::claim`).
    while !todo.is_empty() {
        let tkeys: Vec<CacheKey> = todo.iter().map(|&(bx, by)| block_key(bx, by)).collect();
        let (claim, theirs) = ctx.cache.claim(&tkeys);
        let theirs: Vec<(u32, u32)> = theirs.iter().map(|&j| todo[j]).collect();
        todo.retain(|b| !theirs.contains(b));
        if !todo.is_empty() {
            let noise = src.noise_level(level);
            let t0 = Instant::now();
            let plans: Vec<detail::SPlan> = todo
                .par_iter()
                .map(|&(bx, by)| {
                    let (x0, y0) = (bx * bs, by * bs);
                    let (w, h) = ((lvl.0 - x0).min(bs), (lvl.1 - y0).min(bs));
                    let prog = prep.s.has_locals().then(|| {
                        let mg = prep.s.margin(level) as f64;
                        prep.s_program(
                            level,
                            x0 as f64 - mg,
                            y0 as f64 - mg,
                            (x0 + w) as f64 + mg,
                            (y0 + h) as f64 + mg,
                        )
                    });
                    detail::plan(
                        lvl.0,
                        lvl.1,
                        level,
                        &prep.s,
                        noise,
                        x0,
                        y0,
                        w,
                        h,
                        prog.as_deref().map(|p| (p, &prep.c1)),
                    )
                })
                .collect();
            let jobs: Vec<SJob<'_>> = plans.iter().map(|plan| SJob { plan, src: view }).collect();
            let made: Vec<((u32, u32), Arc<SBlock>)> = ctx
                .backend
                .s_blocks(&jobs)
                .into_iter()
                .zip(&todo)
                .zip(&plans)
                .map(|((data, &k), p)| (k, Arc::new(SBlock { w: p.bw, data })))
                .collect();
            tracing::debug!(level, blocks = todo.len(), elapsed = ?t0.elapsed(), "S blocks");
            let cost = t0.elapsed() / todo.len() as u32;
            for ((bx, by), b) in made {
                ctx.cache.insert(
                    block_key(bx, by),
                    b.clone(),
                    b.data.bytes(),
                    MemClass::StageTile,
                    cost,
                    ctx.owner,
                );
                blocks.insert((bx, by), b);
            }
        }
        drop(claim);
        todo = theirs
            .into_iter()
            .filter(
                |&(bx, by)| match ctx.cache.wait::<SBlock>(&block_key(bx, by), None) {
                    Some(b) => {
                        blocks.insert((bx, by), b);
                        false
                    }
                    None => true,
                },
            )
            .collect();
    }
    for (r, o) in rects.iter().zip(out.iter_mut()) {
        if o.is_some() {
            continue;
        }
        let &(x0, y0, w, h) = r;
        let mut data = PxVec::zeros(w as usize * h as usize, full);
        for j in 0..h {
            let y = y0 + j;
            let mut x = x0;
            while x < x0 + w {
                let (bx, by) = (x / bs, y / bs);
                let b = &blocks[&(bx, by)];
                let (lx, ly) = (x - bx * bs, y - by * bs);
                let n = (b.w - lx).min(x0 + w - x);
                let s0 = (ly * b.w + lx) as usize;
                let d0 = (j * w + x - x0) as usize;
                data.copy_from(d0, &b.data, s0, n as usize);
                x += n;
            }
        }
        let roi = Arc::new(SRoi {
            x0,
            y0,
            w,
            h,
            data,
            device: DeviceCell::default(),
        });
        if keep {
            insert_roi(ctx, roi_key(r), &roi);
        }
        *o = Some(roi);
    }
    out.into_iter().map(|o| o.expect("filled")).collect()
}

/// The C2 field for this render, cached by the C1 key (which covers every input).
fn c2_field(ctx: &RenderCtx<'_>, src: &SourceImage, prep: &Prepared) -> Arc<c2::C2Field> {
    let key = KeyBuilder::new("C2field")
        .u32(c2::C2_IMPL)
        .key(&prep.c1_key)
        .finish();
    if let Some(f) = ctx.cache.get::<c2::C2Field>(&key) {
        return f;
    }
    let t0 = Instant::now();
    let f = Arc::new(c2::compute(src, prep, &prep.c2));
    ctx.cache.insert(
        key,
        f.clone(),
        f.bytes(),
        MemClass::Field,
        t0.elapsed(),
        ctx.owner,
    );
    f
}

/// Rows `y0 .. y0 + h` of the full-resolution render (render scale 1, no E2 resampling:
/// grid pixels are output pixels) as RGB8, or as RGB16 when the output tiles are 16-bit
/// (doc 02 §5.6 export). `w` ≤ the grid width.
pub fn export_rows(
    ctx: &RenderCtx<'_>,
    src: &SourceImage,
    prep: &Prepared,
    (y0, h, w): (u32, u32, u32),
    cancel: Option<&CancelToken>,
) -> Result<Vec<u8>, Cancelled> {
    let (cw, ch) = prep.crop_extent();
    let (gw, gh) = ((cw.ceil() as u32).max(1), (ch.ceil() as u32).max(1));
    let level = prep.level_for_scale(src, 1.0);
    let tiles: Vec<(u32, u32)> = (y0 / TILE..(y0 + h).div_ceil(TILE))
        .flat_map(|ty| (0..gw.div_ceil(TILE)).map(move |tx| (tx, ty)))
        .collect();
    let map = tiles_chunked(ctx, src, prep, 1.0, level, true, &tiles, gw, gh, cancel)?;
    let bpp_in = map.values().next().map_or(4, |t| t.bytes_per_px());
    let bpp_out = bpp_in / 4 * 3;
    let mut out = vec![0u8; w as usize * h as usize * bpp_out];
    out.par_chunks_mut(w as usize * bpp_out)
        .enumerate()
        .for_each(|(j, row)| {
            let gy = y0 + j as u32;
            let ty = gy / TILE;
            for tx in 0..w.div_ceil(TILE) {
                let t = &map[&(tx, ty)];
                let ly = (gy - ty * TILE) as usize;
                let x0 = tx * TILE;
                let n = (w - x0).min(t.w) as usize;
                let src_row = &t.data[ly * t.w as usize * bpp_in..];
                for i in 0..n {
                    let s = &src_row[i * bpp_in..(i + 1) * bpp_in];
                    let d = &mut row[(x0 as usize + i) * bpp_out..][..bpp_out];
                    if bpp_in == 4 {
                        d.copy_from_slice(&[s[2], s[1], s[0]]);
                    } else {
                        // RGBA16 little-endian, alpha carries the clip flags.
                        d.copy_from_slice(&s[..6]);
                    }
                }
            }
        });
    Ok(out)
}

/// Pre-render the one-tile ring around `target`'s tiles into the cache, so that short pans
/// hit it (doc 05 §4.3). Run at `Background` class after the visible frame.
pub fn prerender_ring(
    ctx: &RenderCtx<'_>,
    src: &SourceImage,
    prep: &Prepared,
    target: &RenderTarget,
    cancel: Option<&CancelToken>,
) -> Result<usize, Cancelled> {
    let region = target.region;
    let zoom = (target.out.w as f64 / region.w).min(target.out.h as f64 / region.h);
    let s = quantized_scale(zoom, target.scale_offset.max(0));
    let (cw, ch) = prep.crop_extent();
    let gw = ((cw * s).ceil() as u32).max(1);
    let gh = ((ch * s).ceil() as u32).max(1);
    let (ntx, nty) = (gw.div_ceil(TILE), gh.div_ceil(TILE));
    let t = |v: f64, n: u32| ((v * s).floor().max(0.0) as u32 / TILE).min(n - 1);
    let (tx0, ty0) = (t(region.x, ntx), t(region.y, nty));
    let (tx1, ty1) = (t(region.right(), ntx), t(region.bottom(), nty));
    let ring: Vec<(u32, u32)> = (ty0.saturating_sub(1)..=(ty1 + 1).min(nty - 1))
        .flat_map(|ty| (tx0.saturating_sub(1)..=(tx1 + 1).min(ntx - 1)).map(move |tx| (tx, ty)))
        .filter(|&(tx, ty)| !(tx0..=tx1).contains(&tx) || !(ty0..=ty1).contains(&ty))
        .collect();
    let ring = centre_out(
        ring,
        (region.x + region.w / 2.0) * s,
        (region.y + region.h / 2.0) * s,
    );
    let level = prep.level_for_scale(src, s);
    tiles_chunked(
        ctx,
        src,
        prep,
        s,
        level,
        target.lanczos,
        &ring,
        gw,
        gh,
        cancel,
    )
    .map(|m| m.len())
}

/// Render `target` (doc 02 §5.1). Checks `cancel` at tile boundaries.
pub fn render(
    ctx: &RenderCtx<'_>,
    src: &SourceImage,
    prep: &Prepared,
    target: &RenderTarget,
    cancel: Option<&CancelToken>,
) -> Result<Rendered, Cancelled> {
    let t0 = Instant::now();
    let region = target.region;
    let out = target.out;
    let zoom = (out.w as f64 / region.w).min(out.h as f64 / region.h);
    let s = quantized_scale(zoom, target.scale_offset.max(0));
    let (cw, ch) = prep.crop_extent();
    let gw = ((cw * s).ceil() as u32).max(1);
    let gh = ((ch * s).ceil() as u32).max(1);

    // Grid pixels needed by the bilinear compose, plus one pixel of support.
    let gx0 = ((region.x * s).floor() as i64 - 1).clamp(0, gw as i64 - 1) as u32;
    let gy0 = ((region.y * s).floor() as i64 - 1).clamp(0, gh as i64 - 1) as u32;
    let gx1 = ((region.right() * s).ceil() as i64 + 1).clamp(1, gw as i64) as u32;
    let gy1 = ((region.bottom() * s).ceil() as i64 + 1).clamp(1, gh as i64) as u32;

    let tiles: Vec<(u32, u32)> = (gy0 / TILE..gy1.div_ceil(TILE))
        .flat_map(|ty| (gx0 / TILE..gx1.div_ceil(TILE)).map(move |tx| (tx, ty)))
        .collect();

    let level = prep.level_for_scale(src, s);
    let tiles = centre_out(
        tiles,
        (region.x + region.w / 2.0) * s,
        (region.y + region.h / 2.0) * s,
    );
    let map = tiles_chunked(
        ctx,
        src,
        prep,
        s,
        level,
        target.lanczos,
        &tiles,
        gw,
        gh,
        cancel,
    )?;

    let t_tiles = t0.elapsed();
    // Assemble the needed grid rect contiguously.
    let (aw, ah) = (gx1 - gx0, gy1 - gy0);
    let mut grid = vec![0u8; aw as usize * ah as usize * 4];
    grid.par_chunks_mut(aw as usize * 4)
        .enumerate()
        .for_each(|(row, dst)| {
            let gy = gy0 + row as u32;
            let ty = gy / TILE;
            let mut gx = gx0;
            while gx < gx1 {
                let tx = gx / TILE;
                let tile = &map[&(tx, ty)];
                let lx = gx - tx * TILE;
                let ly = gy - ty * TILE;
                let n = (tile.w - lx).min(gx1 - gx);
                let s0 = ((ly * tile.w + lx) * 4) as usize;
                let d0 = ((gx - gx0) * 4) as usize;
                dst[d0..d0 + n as usize * 4].copy_from_slice(&tile.data[s0..s0 + n as usize * 4]);
                gx += n;
            }
        });

    let t_assemble = t0.elapsed();
    // E2: exact viewport resample.
    // Mask overlay (view only): the alpha of one mask, evaluated per output pixel on the
    // sampled level, tinted red at 50 %.
    let overlay = target
        .overlays
        .mask_overlay
        .and_then(|id| masks::overlay(&prep.edit, id, &prep.level_map(level)));
    let ov_lvl = &src.pyramid.levels[level];
    let ov_inv = 0.5f64.powi(level as i32);
    let mut data = vec![0u8; out.w as usize * out.h as usize * 4];
    let sx = region.w / out.w as f64;
    let sy = region.h / out.h as f64;
    data.par_chunks_mut(out.w as usize * 4)
        .enumerate()
        .for_each(|(oy, row)| {
            let qy = region.y + (oy as f64 + 0.5) * sy;
            for ox in 0..out.w as usize {
                let qx = region.x + (ox as f64 + 0.5) * sx;
                if qx < 0.0 || qy < 0.0 || qx >= cw || qy >= ch {
                    continue; // transparent outside the crop
                }
                let u = (qx * s - 0.5 - gx0 as f64).clamp(0.0, (aw - 1) as f64);
                let v = (qy * s - 0.5 - gy0 as f64).clamp(0.0, (ah - 1) as f64);
                let mut px = sample_bgra(&grid, aw, ah, u, v);
                // Clip flags travel in the alpha of output tiles; nearest sample.
                let (nx, ny) = ((u + 0.5) as u32, (v + 0.5) as u32);
                let flags =
                    255 - grid[((ny.min(ah - 1) * aw + nx.min(aw - 1)) * 4 + 3) as usize] as u32;
                px[3] = 255;
                if target.overlays.clipping {
                    if flags & CLIP_HIGH != 0 {
                        px = [0, 0, 255, 255];
                    } else if flags & CLIP_LOW != 0 {
                        px = [255, 0, 0, 255];
                    }
                }
                if target.overlays.gamut_warning && flags & CLIP_GAMUT != 0 {
                    px = [255, 0, 255, 255];
                }
                if let Some(prog) = &overlay {
                    let p = prep.crop_to_stored(Point::new(qx, qy));
                    let (u, v) = ((p.x * ov_inv - 0.5) as f32, (p.y * ov_inv - 0.5) as f32);
                    let rgb = prep.c1.apply(bilinear(ov_lvl, u, v));
                    let a = 0.5 * aigle_kernels::masks::alpha_of(prog, 0, 0, u, v, rgb);
                    px[0] = (px[0] as f32 * (1.0 - a)).round() as u8;
                    px[1] = (px[1] as f32 * (1.0 - a)).round() as u8;
                    px[2] = (px[2] as f32 * (1.0 - a) + 255.0 * a).round() as u8;
                }
                row[ox * 4..ox * 4 + 4].copy_from_slice(&px);
            }
        });

    tracing::debug!(
        tiles = tiles.len(),
        t_tiles = ?t_tiles,
        t_assemble = ?(t_assemble - t_tiles),
        t_e2 = ?(t0.elapsed() - t_assemble),
        "render"
    );
    Ok(Rendered {
        width: out.w,
        height: out.h,
        data,
        render_scale: s,
        elapsed: t0.elapsed(),
    })
}

/// Level position of R, G, B and the vignetting gain at a tile-local grid point.
type WarpFn<'a> = dyn Fn(f64, f64) -> ([(f64, f64); 3], f64) + 'a;

/// Warp grid `[cell, nodes…]` over a `w × h` tile (see `aigle_kernels::tiles::Geo`) and its
/// nodes per row. The footprint is √|det J| of the green position, by central differences
/// over half a grid px.
fn warp_grid(w: u32, h: u32, at: impl Fn(f64, f64) -> ([(f64, f64); 3], f64)) -> (Vec<f32>, u32) {
    let at: &WarpFn<'_> = &at;
    let cell = WARP_CELL;
    let (wn, ny) = (w.div_ceil(cell) + 1, h.div_ceil(cell) + 1);
    let mut warp = Vec::with_capacity(1 + (wn * ny) as usize * tiles::WARP_NODE);
    warp.push(cell as f32);
    let e = 0.5;
    for j in 0..ny {
        for i in 0..wn {
            let (gx, gy) = ((i * cell) as f64, (j * cell) as f64);
            let (lv, gain) = at(gx, gy);
            let (ax, bx) = (at(gx + e, gy).0[1], at(gx - e, gy).0[1]);
            let (ay, by) = (at(gx, gy + e).0[1], at(gx, gy - e).0[1]);
            let (jxu, jxv) = ((ax.0 - bx.0) / (2.0 * e), (ax.1 - bx.1) / (2.0 * e));
            let (jyu, jyv) = ((ay.0 - by.0) / (2.0 * e), (ay.1 - by.1) / (2.0 * e));
            let k = (jxu * jyv - jyu * jxv).abs().sqrt();
            for (u, v) in lv {
                warp.push(u as f32);
                warp.push(v as f32);
            }
            warp.push(gain as f32);
            warp.push(k as f32);
        }
    }
    (warp, wn)
}

/// (C1, output) cache keys of grid tiles.
fn tile_keys(
    prep: &Prepared,
    s: f64,
    level: usize,
    lanczos: bool,
    tiles: &[(u32, u32)],
    full: bool,
) -> Vec<(CacheKey, CacheKey)> {
    tiles
        .iter()
        .map(|&(tx, ty)| {
            let c1 = KeyBuilder::new("C1tile")
                .u32(keys::C1_IMPL)
                .key(&prep.c1_key)
                .f64(s)
                .u32(level as u32)
                .u32(lanczos as u32)
                .u32(full as u32)
                .u32(TILE)
                .u32(tx)
                .u32(ty)
                .finish();
            let out = KeyBuilder::new("OUTtile")
                .u32(keys::OUT_IMPL)
                .key(&c1)
                .key(&prep.out_key)
                .finish();
            (c1, out)
        })
        .collect()
}

/// Output tiles, computed once across concurrent renders (views, jobs) that need the same
/// tiles: tiles another task of the same or a higher class is computing are waited for
/// (`TileCache::claim`).
#[allow(clippy::too_many_arguments)]
fn output_tiles(
    ctx: &RenderCtx<'_>,
    src: &SourceImage,
    prep: &Prepared,
    s: f64,
    level: usize,
    lanczos: bool,
    tiles: &[(u32, u32)],
    gw: u32,
    gh: u32,
    cancel: Option<&CancelToken>,
) -> Result<HashMap<(u32, u32), Arc<OutTile>>, Cancelled> {
    let keys = tile_keys(prep, s, level, lanczos, tiles, ctx.full_at(level));
    let mut done = HashMap::with_capacity(tiles.len());
    let mut want = Vec::new();
    for (&t, k) in tiles.iter().zip(&keys) {
        match ctx.cache.get::<OutTile>(&k.1) {
            Some(o) => {
                done.insert(t, o);
            }
            None => want.push((t, k.1)),
        }
    }
    if want.is_empty() {
        return Ok(done);
    }
    let wkeys: Vec<CacheKey> = want.iter().map(|w| w.1).collect();
    let (claim, theirs) = ctx.cache.claim(&wkeys);
    let mine: Vec<(u32, u32)> = (0..want.len())
        .filter(|j| !theirs.contains(j))
        .map(|j| want[j].0)
        .collect();
    if !mine.is_empty() {
        done.extend(compute_tiles(
            ctx, src, prep, s, level, lanczos, &mine, gw, gh, cancel,
        )?);
    }
    drop(claim);
    let mut retry = Vec::new();
    for j in theirs {
        match ctx.cache.wait::<OutTile>(&want[j].1, cancel) {
            Some(o) => {
                done.insert(want[j].0, o);
            }
            None => retry.push(want[j].0),
        }
    }
    if !retry.is_empty() {
        if cancel.is_some_and(|c| c.is_cancelled()) {
            return Err(Cancelled);
        }
        done.extend(output_tiles(
            ctx, src, prep, s, level, lanczos, &retry, gw, gh, cancel,
        )?);
    }
    Ok(done)
}

/// Produce the output tiles `tiles` of grid scale `s` (doc 02 §5.1), reusing cached output and
/// C1 tiles, and batching the misses of each product into one backend call.
#[allow(clippy::too_many_arguments)]
fn compute_tiles(
    ctx: &RenderCtx<'_>,
    src: &SourceImage,
    prep: &Prepared,
    s: f64,
    level: usize,
    lanczos: bool,
    tiles: &[(u32, u32)],
    gw: u32,
    gh: u32,
    cancel: Option<&CancelToken>,
) -> Result<HashMap<(u32, u32), Arc<OutTile>>, Cancelled> {
    debug_assert_eq!(src.size(), prep.stored_size);
    let keys = tile_keys(prep, s, level, lanczos, tiles, ctx.full_at(level));
    let dims = |tx: u32, ty: u32| ((gw - tx * TILE).min(TILE), (gh - ty * TILE).min(TILE));

    let mut done: HashMap<(u32, u32), Arc<OutTile>> = HashMap::with_capacity(tiles.len());
    let mut missing = Vec::new();
    for (i, &t) in tiles.iter().enumerate() {
        match ctx.cache.get::<OutTile>(&keys[i].1) {
            Some(o) => {
                done.insert(t, o);
            }
            None => missing.push(i),
        }
    }
    if missing.is_empty() {
        return Ok(done);
    }
    if cancel.is_some_and(|c| c.is_cancelled()) {
        return Err(Cancelled);
    }

    // C1 tiles for the missing outputs.
    let t0 = Instant::now();
    let mut c1s: Vec<Option<Arc<C1Tile>>> = missing
        .iter()
        .map(|&i| ctx.cache.get::<C1Tile>(&keys[i].0))
        .collect();
    let need: Vec<usize> = (0..missing.len()).filter(|&j| c1s[j].is_none()).collect();
    if !need.is_empty() {
        let whole = ctx.view(src, level);
        let lvl = (whole.full_w, whole.full_h);
        let field = (prep.c2.active != 0).then(|| c2_field(ctx, src, prep));
        let mut c2p = prep.c2;
        if let Some(f) = &field {
            c2p.airlight = f.airlight;
            c2p.fscale = 2f32.powi(level as i32 - f.level as i32);
        }
        let geo: Vec<(TileGeo, u32, u32)> = need
            .par_iter()
            .map(|&j| {
                let (tx, ty) = tiles[missing[j]];
                let (w, h) = dims(tx, ty);
                (
                    prep.tile_geo(s, level, (tx * TILE, ty * TILE), (w, h), lanczos),
                    w,
                    h,
                )
            })
            .collect();
        let rects: Vec<_> = geo
            .iter()
            .map(|(g, w, h)| g.roi(*w, *h, lvl.0, lvl.1))
            .collect();
        // Tiles read S-processed (and retouched) ROIs only where S ops run or retouching
        // reaches them; the others sample the level directly.
        let s_all = !prep.s.is_identity();
        let rt = (!s_all && prep.r_key.is_some())
            .then(|| retouch::Retouch::new(&prep.edit, &prep.level_map(level), level));
        let roi_of: Vec<Option<usize>> = {
            let mut n = 0;
            rects
                .iter()
                .map(|&r| {
                    let on = s_all || rt.as_ref().is_some_and(|t| t.touches(r, lvl.0, lvl.1));
                    on.then(|| {
                        n += 1;
                        n - 1
                    })
                })
                .collect()
        };
        let roi_rects: Vec<_> = rects
            .iter()
            .zip(&roi_of)
            .filter(|(_, i)| i.is_some())
            .map(|(r, _)| *r)
            .collect();
        let rois = if roi_rects.is_empty() {
            Vec::new()
        } else {
            s_rois(ctx, src, prep, level, &roi_rects)
        };
        let roi_dev: Vec<_> = rois
            .iter()
            .map(|r| ctx.backend.resident_px(&r.device, r.data.as_px()))
            .collect();
        // Mask programs, culled to each tile's source ROI.
        let map = prep.level_map(level);
        let progs: Vec<Vec<f32>> = rects
            .iter()
            .map(|&(x0, y0, rw, rh)| {
                if !prep.uses.any {
                    return vec![0.0];
                }
                let b = map.bounds(x0 as f64, y0 as f64, (x0 + rw) as f64, (y0 + rh) as f64);
                masks::compile(&prep.edit, masks::Tier::C, &map, Some(&b))
            })
            .collect();
        let jobs: Vec<C1Job<'_>> = geo
            .iter()
            .enumerate()
            .map(|(k, (geo, w, h))| {
                let (w, h) = (*w, *h);
                let (view, device_src, atlas) = if let Some(i) = roi_of[k] {
                    let r = &rois[i];
                    let view = LevelView {
                        data: r.data.as_px(),
                        x0: r.x0,
                        y0: r.y0,
                        w: r.w,
                        h: r.h,
                        full_w: lvl.0,
                        full_h: lvl.1,
                    };
                    let dev = roi_dev[i].as_deref().map(|buf| DeviceSrc {
                        buf,
                        x0: r.x0,
                        y0: r.y0,
                        w: r.w,
                        h: r.h,
                    });
                    (view, dev, None)
                } else if ctx.full_at(level) {
                    (whole, None, None)
                } else {
                    (whole, None, Some((src.a_key, level as u32)))
                };
                C1Job {
                    src: view,
                    device_src,
                    atlas,
                    geo,
                    params: prep.c1.params,
                    color: &prep.c1.block,
                    c2: c2p,
                    prog: &progs[k],
                    fields: prep.uses.d_fields,
                    w,
                    h,
                }
            })
            .collect();
        let fref = field.as_ref().map(|f| FieldRef {
            aux: &f.aux,
            w: f.w,
            h: f.h,
        });
        let made = ctx.backend.c1_tiles(&jobs, fref);
        let cost = t0.elapsed() / need.len() as u32;
        for (&j, (job, data)) in need.iter().zip(jobs.iter().zip(made)) {
            let t = Arc::new(C1Tile {
                w: job.w,
                h: job.h,
                data,
            });
            ctx.cache.insert(
                keys[missing[j]].0,
                t.clone(),
                t.data.bytes(),
                match t.data {
                    C1Data::Host(_) => MemClass::StageTile,
                    C1Data::Device(_) => MemClass::DeviceTile,
                },
                cost,
                ctx.owner,
            );
            c1s[j] = Some(t);
        }
    }
    if cancel.is_some_and(|c| c.is_cancelled()) {
        return Err(Cancelled);
    }

    // Output tiles.
    let t1 = Instant::now();
    let c1s: Vec<Arc<C1Tile>> = c1s.into_iter().map(|c| c.expect("filled above")).collect();
    let params = prep.out_params(s);
    let jobs: Vec<OutJob<'_>> = missing
        .iter()
        .zip(&c1s)
        .map(|(&i, c1)| {
            let (tx, ty) = tiles[i];
            OutJob {
                c1: &c1.data,
                fields: prep.uses.d_fields,
                full: ctx.full_at(level),
                params,
                w: c1.w,
                h: c1.h,
                gx0: tx * TILE,
                gy0: ty * TILE,
            }
        })
        .collect();
    let made = match ctx.backend.out_tiles(&jobs, &prep.lut) {
        Ok(m) => m,
        Err(StaleDeviceData) => {
            // The GPU was abandoned after these C1 tiles were made on it: recompute them on the
            // CPU (the cache entries are replaced with host tiles).
            drop(jobs);
            drop(c1s);
            for &i in &missing {
                ctx.cache.remove(&keys[i].0);
            }
            return compute_tiles(ctx, src, prep, s, level, lanczos, tiles, gw, gh, cancel);
        }
    };
    let cost = t1.elapsed() / missing.len() as u32;
    for ((&i, c1), data) in missing.iter().zip(&c1s).zip(made) {
        let t = Arc::new(OutTile {
            w: c1.w,
            data,
            bpp: crate::backend::out_bpp(ctx.full_at(level)),
        });
        ctx.cache.insert(
            keys[i].1,
            t.clone(),
            t.data.len() as u64,
            MemClass::StageTile,
            cost,
            ctx.owner,
        );
        done.insert(tiles[i], t);
    }
    Ok(done)
}

/// Tiles sorted by the distance of their centre from grid point (`cx`, `cy`), so that the
/// visible centre is computed first (doc 05 §4.3).
fn centre_out(mut tiles: Vec<(u32, u32)>, cx: f64, cy: f64) -> Vec<(u32, u32)> {
    let d = |&(tx, ty): &(u32, u32)| {
        let (x, y) = (
            (tx as f64 + 0.5) * TILE as f64 - cx,
            (ty as f64 + 0.5) * TILE as f64 - cy,
        );
        x * x + y * y
    };
    tiles.sort_by(|a, b| d(a).total_cmp(&d(b)).then(a.cmp(b)));
    tiles
}

/// Output tiles per chunk for lower classes: ~1 Mpx, a few ms on the GPU and tens of ms on
/// the CPU (doc 05 §6).
const LOW_CHUNK_TILES: usize = 4;

/// [`output_tiles`] for the current task: in one batch for interactive work, else in chunks
/// with a CPU-gate checkpoint before each, so higher classes take over the CPU and GPU at
/// chunk boundaries (doc 05 §5–6).
#[allow(clippy::too_many_arguments)]
fn tiles_chunked(
    ctx: &RenderCtx<'_>,
    src: &SourceImage,
    prep: &Prepared,
    s: f64,
    level: usize,
    lanczos: bool,
    tiles: &[(u32, u32)],
    gw: u32,
    gh: u32,
    cancel: Option<&CancelToken>,
) -> Result<HashMap<(u32, u32), Arc<OutTile>>, Cancelled> {
    if crate::arbiter::current().class == crate::scheduler::Class::Interactive {
        return output_tiles(ctx, src, prep, s, level, lanczos, tiles, gw, gh, cancel);
    }
    let mut map = HashMap::with_capacity(tiles.len());
    for chunk in tiles.chunks(LOW_CHUNK_TILES) {
        if !crate::arbiter::cpu_gate().checkpoint(cancel) {
            return Err(Cancelled);
        }
        map.extend(output_tiles(
            ctx, src, prep, s, level, lanczos, chunk, gw, gh, cancel,
        )?);
    }
    Ok(map)
}

fn sample_bgra(grid: &[u8], w: u32, h: u32, u: f64, v: f64) -> [u8; 4] {
    let x0 = u.floor() as u32;
    let y0 = v.floor() as u32;
    let x1 = (x0 + 1).min(w - 1);
    let y1 = (y0 + 1).min(h - 1);
    let fx = (u - x0 as f64) as f32;
    let fy = (v - y0 as f64) as f32;
    let at = |x: u32, y: u32| ((y * w + x) * 4) as usize;
    let (a, b, c, d) = (at(x0, y0), at(x1, y0), at(x0, y1), at(x1, y1));
    let mut out = [0u8; 4];
    for k in 0..4 {
        let top = grid[a + k] as f32 + (grid[b + k] as f32 - grid[a + k] as f32) * fx;
        let bot = grid[c + k] as f32 + (grid[d + k] as f32 - grid[c + k] as f32) * fx;
        out[k] = (top + (bot - top) * fy).round().clamp(0.0, 255.0) as u8;
    }
    out
}

/// Mean scene-referred (post-C1) value in a disc of `radius` Crop-space px around `at`,
/// sampled at level 0, after the S tier when any S op is active (so it matches a 1:1 render).
pub fn sample_scene(
    ctx: &RenderCtx<'_>,
    src: &SourceImage,
    prep: &Prepared,
    at: ImagePoint,
    radius: f32,
) -> Rgb {
    let r = radius.max(0.0) as f64;
    let n: i32 = if r < 0.5 { 0 } else { 4 };
    // Taps: per-channel level-0 positions and vignetting gain.
    let mut taps = Vec::new();
    for j in -n..=n {
        for i in -n..=n {
            let (dx, dy) = if n == 0 {
                (0.0, 0.0)
            } else {
                (i as f64 * r / n as f64, j as f64 * r / n as f64)
            };
            if dx * dx + dy * dy > r * r + 1e-9 {
                continue;
            }
            taps.push(prep.crop_to_stored_rgb(Point::new(at.x + dx, at.y + dy)));
        }
    }
    let lvl = &src.pyramid.levels[0];
    // With S ops active, read the S-processed level-0 pixels around the taps, so the sample
    // matches what is rendered at 1:1.
    let roi = prep.s_on().then(|| {
        let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for (ps, _) in &taps {
            for p in ps {
                (x0, y0, x1, y1) = (x0.min(p.x), y0.min(p.y), x1.max(p.x), y1.max(p.y));
            }
        }
        let cl = |v: f64, m: u32| (v.max(0.0) as u32).min(m - 1);
        let (ax, ay) = (cl(x0 - 2.0, lvl.width), cl(y0 - 2.0, lvl.height));
        let (bx, by) = (cl(x1 + 2.0, lvl.width), cl(y1 + 2.0, lvl.height));
        let rect = (ax, ay, bx - ax + 1, by - ay + 1);
        s_rois(ctx, src, prep, 0, &[rect]).remove(0)
    });
    struct Window<'a> {
        w: u32,
        h: u32,
        roi: &'a SRoi,
    }
    impl aigle_kernels::resample::Rgb2D for Window<'_> {
        fn width(&self) -> u32 {
            self.w
        }
        fn height(&self) -> u32 {
            self.h
        }
        fn load(&self, x: u32, y: u32) -> Rgb {
            let r = self.roi;
            let lx = x.clamp(r.x0, r.x0 + r.w - 1) - r.x0;
            let ly = y.clamp(r.y0, r.y0 + r.h - 1) - r.y0;
            r.data.as_px().get((ly * r.w + lx) as usize)
        }
    }
    let mut acc = [0.0f64; 3];
    for (ps, gain) in &taps {
        let at = |c: usize| {
            let (u, v) = ((ps[c].x - 0.5) as f32, (ps[c].y - 0.5) as f32);
            match &roi {
                Some(roi) => bilinear(
                    &Window {
                        w: lvl.width,
                        h: lvl.height,
                        roi,
                    },
                    u,
                    v,
                )[c],
                None => bilinear(lvl, u, v)[c],
            }
        };
        let g = *gain as f32;
        let v = prep.c1.apply([at(0) * g, at(1) * g, at(2) * g]);
        for k in 0..3 {
            acc[k] += v[k] as f64;
        }
    }
    acc.map(|v| (v / taps.len() as f64) as f32)
}

/// Region that fits the whole crop into `size`, letterboxed and centred.
pub fn fit_region(crop: (f64, f64), size: Size) -> ImageRect {
    let (cw, ch) = crop;
    let scale = (size.w as f64 / cw).min(size.h as f64 / ch);
    let rw = size.w as f64 / scale;
    let rh = size.h as f64 / scale;
    ImageRect::new((cw - rw) / 2.0, (ch - rh) / 2.0, rw, rh)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_prerender_serves_a_one_tile_pan() {
        use crate::arbiter::{TaskCtx, with_task};
        let (w, h) = (3000u32, 2000u32);
        let rgb: Vec<aigle_kernels::Rgb> = (0..w * h)
            .map(|i| [(i % w) as f32 / w as f32, (i / w) as f32 / h as f32, 0.3])
            .collect();
        let src =
            crate::source::test_source(w, h, &rgb, crate::source::SourceColor::working_space());
        let ledger = crate::ledger::MemLedger::new(4 << 30);
        let cache = TileCache::new(ledger, 0.5);
        let backend = Backend::cpu("test");
        let ctx = RenderCtx {
            cache: &cache,
            backend: &backend,
            owner: 1,
            full: None,
        };
        let prep = Prepared::new(&src, &EditParams::default(), &DisplayProfileId::Srgb);
        let at = |x: f64| RenderTarget {
            region: ImageRect::new(x, 700.0, 600.0, 400.0),
            out: Size::new(600, 400),
            scale_offset: 0,
            overlays: Overlays::default(),
            lanczos: true,
        };
        render(&ctx, &src, &prep, &at(1100.0), None).unwrap();
        let bg = TaskCtx {
            class: crate::scheduler::Class::Background,
            job: 7,
        };
        let n = with_task(bg, || prerender_ring(&ctx, &src, &prep, &at(1100.0), None)).unwrap();
        assert!(n >= 8, "{n} ring tiles");
        let before = cache.len();
        render(&ctx, &src, &prep, &at(1100.0 + TILE as f64), None).unwrap();
        assert_eq!(cache.len(), before, "the pan computed new tiles");
    }

    /// The warp grid reproduces the exact B1 map to well under 0.05 px for strong lens
    /// distortion plus perspective, at 1:1 and at fit scale.
    #[test]
    fn warp_grid_is_accurate() {
        use aigle_core::geometry::Optics;
        use aigle_core::lens::*;
        let prof = LensProfileData {
            lens_maker: "T".into(),
            lens_model: "T".into(),
            calib_crop: 1.0,
            calib_aspect: 1.5,
            camera_crop: 1.0,
            min_focal: 16.0,
            max_focal: 16.0,
            distortion: vec![DistEntry {
                focal: 16.0,
                real_focal: None,
                model: DistModel::Ptlens {
                    a: 0.02,
                    b: -0.08,
                    c: 0.03,
                },
            }],
            tca: vec![TcaEntry {
                focal: 16.0,
                model: TcaModel::Linear {
                    kr: 1.0006,
                    kb: 0.9994,
                },
            }],
            vignetting: vec![],
        };
        let size = Size::new(6000, 4000);
        let shot = LensShot {
            focal: 16.0,
            aperture: 8.0,
            distance: 1000.0,
        };
        let optics = Optics {
            lens: Some(prof.model(&shot, size, [1.0; 3])),
            focal_35mm: Some(16.0),
        };
        let mut g = aigle_core::params::GeometryParams::default();
        g.scalars.perspective_vertical = 40.0;
        g.scalars.angle = 2.0;
        let gm = GeometryMap::with_optics(size, &g, &optics);
        assert!(!gm.is_affine());
        for (s, level, tx, ty) in [
            (1.0, 0usize, 0u32, 0u32),
            (1.0, 0, 2560, 1536),
            (0.125, 3, 0, 0),
        ] {
            let inv_l = 0.5f64.powi(level as i32);
            let exact = |gx: f64, gy: f64| {
                let (ps, gain) =
                    gm.crop_to_source_rgb(Point::new((tx as f64 + gx) / s, (ty as f64 + gy) / s));
                (ps.map(|p| (p.x * inv_l - 0.5, p.y * inv_l - 0.5)), gain)
            };
            let (tw, th) = (TILE, TILE.min(500));
            let (warp, wn) = warp_grid(tw, th, exact);
            let geo = tiles::Geo {
                affine: Affine { a: [0.0; 6] },
                warp: &warp,
                wbase: 1,
                wn,
                cell: warp[0],
                flags: 0,
            };
            let mut worst = 0f64;
            for y in (0..th).step_by(7) {
                for x in (0..tw).step_by(5) {
                    let l = tiles::locate(&geo, x, y);
                    let (e, gain) = exact(x as f64 + 0.5, y as f64 + 0.5);
                    for c in 0..3 {
                        worst = worst
                            .max((l[2 * c] as f64 - e[c].0).abs())
                            .max((l[2 * c + 1] as f64 - e[c].1).abs());
                    }
                    assert!((l[6] as f64 - gain).abs() < 1e-3);
                }
            }
            assert!(
                worst < 0.05,
                "s {s} tile ({tx},{ty}): warp error {worst} px"
            );
        }
    }

    #[test]
    fn quantized_scales() {
        assert_eq!(quantized_scale(1.0, 0), 1.0);
        assert!((quantized_scale(0.6, 0) - 0.5f64.sqrt()).abs() < 1e-12);
        assert_eq!(quantized_scale(0.5, 0), 0.5);
        assert!((quantized_scale(0.5, 1) - 0.125f64.sqrt()).abs() < 1e-12);
        assert_eq!(quantized_scale(1.5, 0), 2.0);
    }

    #[test]
    fn fit_letterboxes() {
        let r = fit_region((4000.0, 2000.0), Size::new(400, 400));
        assert_eq!((r.x, r.w), (0.0, 4000.0));
        assert_eq!((r.y, r.h), (-1000.0, 4000.0));
    }
}
