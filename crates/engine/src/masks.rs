//! M: compiles `EditParams::masks` into the per-tile mask programs that the kernels evaluate
//! (`aigle_kernels::masks`, doc 02 §9).
//!
//! Two programs per edit:
//! * the **C program** (12 parameters, `aigle_kernels::tiles::local`) for the C1 tile kernel;
//! * the **S program** (texture, sharpness, noise) for the S tier on the source grid.
//!
//! Geometry conventions (all in normalised oriented Source coordinates, doc 02 §2):
//! * brush `size` is the stroke radius as a fraction of the source long edge;
//! * radial `radii` are fractions of the source width and height, `angle` in degrees,
//!   `feather` 0..100;
//! * linear: α = 1 at `from`, 0 at `to`;
//! * luminance range `low`/`high` are sRGB-encoded luminance (0..1); colour range samples are
//!   scene-linear Rec.2020 RGB as returned by `Session::sample(SceneLinear)`, `tolerance`
//!   0..100; `smoothness` 0..100 widens both falloffs.
//!
//! Range components see the pixel after the global C1 (doc 02 §9.3).
//!
//! Semantic masks need the AI mask assets (doc 06 §5), which are not available yet; they
//! compile to α = 0 and are reported once through the log.

use aigle_core::EditParams;
use aigle_core::color::{self, Mat3};
use aigle_core::geometry::{Orientation, Point, Size};
use aigle_core::masks::{CombineOp, LocalAdjustment, LocalParams, MaskShape, RangeKind};
use aigle_kernels::masks::*;
use aigle_kernels::tiles::local;

/// Which local parameters a program carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tier {
    C,
    S,
}

pub const S_TEXTURE: usize = 0;
pub const S_SHARPNESS: usize = 1;
pub const S_NOISE: usize = 2;

fn deltas(a: &LocalParams, tier: Tier) -> Vec<f32> {
    let g = |v: Option<f32>| v.unwrap_or(0.0);
    match tier {
        Tier::C => {
            let mut d = vec![0f32; local::N];
            d[local::EXPOSURE] = g(a.exposure);
            d[local::TEMPERATURE] = g(a.temperature);
            d[local::TINT] = g(a.tint);
            d[local::HIGHLIGHTS] = g(a.highlights);
            d[local::SHADOWS] = g(a.shadows);
            d[local::CLARITY] = g(a.clarity);
            d[local::DEHAZE] = g(a.dehaze);
            d[local::CONTRAST] = g(a.contrast);
            d[local::WHITES] = g(a.whites);
            d[local::BLACKS] = g(a.blacks);
            d[local::SATURATION] = g(a.saturation);
            d[local::HUE] = g(a.hue);
            d
        }
        Tier::S => vec![g(a.texture), g(a.sharpness), g(a.noise)],
    }
}

/// Masks that touch any parameter of `tier`.
pub fn relevant(p: &EditParams, tier: Tier) -> Vec<&LocalAdjustment> {
    p.masks
        .iter()
        .filter(|m| m.opacity > 0.0 && deltas(&m.adjust, tier).iter().any(|v| *v != 0.0))
        .collect()
}

/// Which C-tier parameter groups any mask uses: (C1 wb/exposure, C2 tone, C2 clarity,
/// C2 dehaze, D fields).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Uses {
    pub any: bool,
    pub tone: bool,
    pub clarity: bool,
    pub dehaze: bool,
    pub d_fields: bool,
}

pub fn uses(p: &EditParams) -> Uses {
    let mut u = Uses::default();
    for m in relevant(p, Tier::C) {
        let d = deltas(&m.adjust, Tier::C);
        u.any = true;
        u.tone |= d[local::HIGHLIGHTS] != 0.0 || d[local::SHADOWS] != 0.0;
        u.clarity |= d[local::CLARITY] != 0.0;
        u.dehaze |= d[local::DEHAZE] != 0.0;
        u.d_fields |= [
            local::CONTRAST,
            local::WHITES,
            local::BLACKS,
            local::SATURATION,
            local::HUE,
        ]
        .iter()
        .any(|&i| d[i] != 0.0);
    }
    u
}

/// Region in Source px, for culling geometry; `None` = no culling.
#[derive(Clone, Copy, Debug)]
pub struct Bounds {
    pub x0: f64,
    pub y0: f64,
    pub x1: f64,
    pub y1: f64,
}

impl Bounds {
    fn hits(&self, ax: f64, ay: f64, bx: f64, by: f64, r: f64) -> bool {
        ax.min(bx) - r <= self.x1
            && ax.max(bx) + r >= self.x0
            && ay.min(by) - r <= self.y1
            && ay.max(by) + r >= self.y0
    }
}

/// Mapping from pyramid-level pixel-centre coordinates to normalised Source coordinates.
pub struct LevelMap {
    /// `[a, b, c, d, e, f]`: n = (a·(u+½) + b·(v+½) + c, d·(u+½) + e·(v+½) + f).
    pub m: [f64; 6],
    /// Oriented Source size (incl. `orientation_delta`).
    pub source: Size,
}

impl LevelMap {
    pub fn new(stored: Size, delta: Orientation, level: usize) -> LevelMap {
        let source = delta.apply_size(stored);
        let step = 2f64.powi(level as i32);
        let f = |x: f64, y: f64| {
            let p = delta.apply_point(Point::new(x * step, y * step), stored);
            (p.x / source.w as f64, p.y / source.h as f64)
        };
        let (c, f0) = f(0.0, 0.0);
        let (ax, dx) = f(1.0, 0.0);
        let (bx, ey) = f(0.0, 1.0);
        LevelMap {
            m: [ax - c, bx - c, c, dx - f0, ey - f0, f0],
            source,
        }
    }

    /// Source-px bounds of a level rectangle.
    pub fn bounds(&self, x0: f64, y0: f64, x1: f64, y1: f64) -> Bounds {
        let m = &self.m;
        let (sw, sh) = (self.source.w as f64, self.source.h as f64);
        let pts = [(x0, y0), (x1, y0), (x0, y1), (x1, y1)].map(|(u, v)| {
            (
                (m[0] * u + m[1] * v + m[2]) * sw,
                (m[3] * u + m[4] * v + m[5]) * sh,
            )
        });
        Bounds {
            x0: pts.iter().map(|p| p.0).fold(f64::MAX, f64::min),
            y0: pts.iter().map(|p| p.1).fold(f64::MAX, f64::min),
            x1: pts.iter().map(|p| p.0).fold(f64::MIN, f64::max),
            y1: pts.iter().map(|p| p.1).fold(f64::MIN, f64::max),
        }
    }
}

fn header(n: usize, np: usize, map: &LevelMap) -> Vec<f32> {
    let mut h = Vec::with_capacity(HEADER);
    h.extend_from_slice(&[
        n as f32,
        np as f32,
        map.source.w as f32,
        map.source.h as f32,
    ]);
    h.extend(map.m.iter().map(|&v| v as f32));
    let (lms, lab) = oklab();
    let cat = color::mul(&color::CAT16, &color::REC2020_TO_XYZ);
    let cat_inv = color::invert(&cat).expect("invertible");
    for m in [lms, lab, cat, cat_inv] {
        h.extend(m.iter().flatten().map(|&v| v as f32));
    }
    debug_assert_eq!(h.len(), HEADER);
    h
}

fn oklab() -> (Mat3, Mat3) {
    const M1: Mat3 = [
        [0.412_221_470_8, 0.536_332_536_3, 0.051_445_992_9],
        [0.211_903_498_2, 0.680_699_545_1, 0.107_396_956_6],
        [0.088_302_461_9, 0.281_718_837_6, 0.629_978_700_5],
    ];
    const M2: Mat3 = [
        [0.210_454_255_3, 0.793_617_785_0, -0.004_072_046_8],
        [1.977_998_495_1, -2.428_592_205_0, 0.450_593_709_9],
        [0.025_904_037_1, 0.782_771_766_2, -0.808_675_766_0],
    ];
    let to_srgb = color::invert(&color::SRGB_TO_XYZ).expect("invertible");
    (
        color::mul(&M1, &color::mul(&to_srgb, &color::REC2020_TO_XYZ)),
        M2,
    )
}

fn to_lab(rgb: [f32; 3]) -> [f32; 3] {
    let (m1, m2) = oklab();
    let lms = color::mul_vec(&m1, rgb.map(|v| v as f64));
    color::mul_vec(&m2, lms.map(f64::cbrt)).map(|v| v as f32)
}

fn compile_shape(s: &MaskShape, map: &LevelMap, b: Option<&Bounds>, out: &mut Vec<f32>) {
    let (sw, sh) = (map.source.w as f64, map.source.h as f64);
    let long = sw.max(sh);
    match s {
        MaskShape::Radial {
            center,
            radii,
            angle,
            feather,
        } => {
            let a = (*angle as f64).to_radians();
            out.extend_from_slice(&[
                OP_RADIAL as f32,
                (center.x as f64 * sw) as f32,
                (center.y as f64 * sh) as f32,
                (radii[0] as f64 * sw).max(1e-3) as f32,
                (radii[1] as f64 * sh).max(1e-3) as f32,
                a.cos() as f32,
                a.sin() as f32,
                1.0 - feather.clamp(0.0, 100.0) / 100.0,
            ]);
        }
        MaskShape::Linear { from, to } => {
            let (fx, fy) = (from.x as f64 * sw, from.y as f64 * sh);
            let (dx, dy) = (to.x as f64 * sw - fx, to.y as f64 * sh - fy);
            let l2 = (dx * dx + dy * dy).max(1e-9);
            out.extend_from_slice(&[
                OP_LINEAR as f32,
                fx as f32,
                fy as f32,
                (dx / l2) as f32,
                (dy / l2) as f32,
            ]);
        }
        MaskShape::Brush { strokes } => {
            let mut body = Vec::new();
            let mut n = 0;
            for st in strokes {
                let r = (st.size as f64 * long).max(0.5);
                let pts: Vec<(f64, f64)> = st
                    .points
                    .iter()
                    .map(|p| (p.x as f64 * sw, p.y as f64 * sh))
                    .collect();
                let segs: Vec<[f64; 4]> = match pts.len() {
                    0 => Vec::new(),
                    1 => vec![[pts[0].0, pts[0].1, pts[0].0, pts[0].1]],
                    _ => pts
                        .windows(2)
                        .map(|w| [w[0].0, w[0].1, w[1].0, w[1].1])
                        .collect(),
                };
                let segs: Vec<[f64; 4]> = segs
                    .into_iter()
                    .filter(|g| b.is_none_or(|b| b.hits(g[0], g[1], g[2], g[3], r)))
                    .collect();
                if segs.is_empty() {
                    continue;
                }
                n += 1;
                body.extend_from_slice(&[
                    segs.len() as f32,
                    r as f32,
                    1.0 - st.feather.clamp(0.0, 100.0) / 100.0,
                    st.flow.clamp(0.0, 100.0) / 100.0,
                    st.erase as u32 as f32,
                ]);
                for g in segs {
                    body.extend(g.iter().map(|&v| v as f32));
                }
            }
            out.extend_from_slice(&[OP_BRUSH as f32, n as f32]);
            out.extend(body);
        }
        MaskShape::Range { range, smoothness } => {
            let w = 0.02 + 0.3 * smoothness.clamp(0.0, 100.0) / 100.0;
            match range {
                RangeKind::Luminance { low, high } => {
                    out.extend_from_slice(&[OP_LUM as f32, *low, *high, w])
                }
                RangeKind::Color { samples, tolerance } => {
                    out.extend_from_slice(&[
                        OP_COLOR as f32,
                        samples.len() as f32,
                        tolerance.clamp(0.0, 100.0) / 100.0 * 0.15,
                        w * 0.5,
                    ]);
                    for s in samples {
                        out.extend_from_slice(&to_lab(*s));
                    }
                }
            }
        }
        MaskShape::Semantic { target, .. } => {
            tracing::warn!(%target, "semantic mask assets are not available; mask renders as empty");
            out.extend_from_slice(&[OP_CONST as f32, 0.0]);
        }
        MaskShape::Combine { op, parts } => {
            if parts.is_empty() {
                out.extend_from_slice(&[OP_CONST as f32, 0.0]);
                return;
            }
            let code = match op {
                CombineOp::Add => OP_MAX,
                CombineOp::Subtract => OP_SUB,
                CombineOp::Intersect => OP_MUL,
            } as f32;
            compile_shape(&parts[0], map, b, out);
            for p in &parts[1..] {
                compile_shape(p, map, b, out);
                out.push(code);
            }
        }
    }
}

/// Compile the masks relevant to `tier`, culled to `bounds`. Always non-empty (`[0.0]`
/// means no masks).
pub fn compile(p: &EditParams, tier: Tier, map: &LevelMap, bounds: Option<&Bounds>) -> Vec<f32> {
    let ms = relevant(p, tier);
    if ms.is_empty() {
        return vec![0.0];
    }
    let np = if tier == Tier::C { local::N } else { 3 };
    let mut out = header(ms.len(), np, map);
    for m in ms {
        let mut prog = Vec::new();
        compile_shape(&m.shape, map, bounds, &mut prog);
        out.extend_from_slice(&[
            m.opacity.clamp(0.0, 1.0),
            m.invert as u32 as f32,
            prog.len() as f32,
        ]);
        out.extend(deltas(&m.adjust, tier));
        out.extend(prog);
    }
    out
}

/// Single-mask program for the mask overlay of mask `id` (C-tier layout, one mask, zero
/// deltas), or `None` if there is no such mask.
pub fn overlay(p: &EditParams, id: aigle_core::MaskId, map: &LevelMap) -> Option<Vec<f32>> {
    let m = p.masks.iter().find(|m| m.id == id)?;
    let mut out = header(1, 0, map);
    let mut prog = Vec::new();
    compile_shape(&m.shape, map, None, &mut prog);
    out.extend_from_slice(&[
        m.opacity.clamp(0.0, 1.0),
        m.invert as u32 as f32,
        prog.len() as f32,
    ]);
    out.extend(prog);
    Some(out)
}
