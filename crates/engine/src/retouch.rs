//! Spot removal (heal / clone) and red-eye removal on the S-processed source pixels of a
//! pyramid level: after the S tier, before B1 geometry and the local adjustments of C1, so
//! retouching stays on its subject under any crop, rotation or lens correction and is seen
//! by every later stage.
//!
//! Positions are continuous level coordinates (pixel `i` covers `[i, i + 1)`).
//!
//! * Clone copies the source disc.
//! * Heal adds a membrane to the source disc: the harmonic function on the disc whose boundary
//!   values are the target − source difference around the rim (the Poisson integral of the
//!   disc, which solves the seamless-cloning equation exactly for a circle). It works on
//!   square-root encoded values so that a bright source over a dark target cannot go negative.
//! * Red-eye detects red pixels in the disc (red against the larger of green and blue) and
//!   replaces red by the mean of green and blue, darkened.
//!
//! Every spot is rendered as one whole-disc [`Patch`] from the un-retouched pixels, so tiles
//! that cut through a spot agree, and each spot's source never sees another spot.

use aigle_core::EditParams;
use aigle_core::geometry::Orientation;
use aigle_core::masks::Pt;
use aigle_core::retouch::SpotMode;
use aigle_kernels::Rgb;
use rayon::prelude::*;

use crate::backend::{Px, PxVec};
use crate::masks::LevelMap;
use crate::source::SourceImage;

/// Bump when the rendered result changes.
pub const RETOUCH_IMPL: u32 = 1;

/// `(x0, y0, w, h)` in level pixels.
pub type Rect = (u32, u32, u32, u32);

/// Pixels read around the disc beyond its radius (rim samples and bilinear support).
const RIM: f64 = 2.5;
/// Membrane rim samples spacing in level px.
const RIM_STEP: f64 = 1.5;
const MAX_RIM_SAMPLES: usize = 192;
/// Nodes across the membrane grid; the membrane is smooth, so large spots evaluate it coarsely.
const MEMBRANE_NODES: f64 = 48.0;
/// Soft edge of a red-eye disc, as a fraction of its radius.
const EYE_FEATHER: f64 = 0.25;

/// A spot in level pixels.
#[derive(Clone, Debug, PartialEq)]
pub struct LevelSpot {
    pub heal: bool,
    pub target: (f64, f64),
    pub source: (f64, f64),
    pub radius: f64,
    /// Radius of the fully replaced core.
    pub inner: f64,
    pub opacity: f32,
}

/// A red-eye correction in level pixels.
#[derive(Clone, Debug, PartialEq)]
pub struct LevelEye {
    pub center: (f64, f64),
    pub radius: f64,
    /// Minimum redness `(r − max(g, b)) / r` counted as pupil.
    pub threshold: f32,
    /// Gain applied to the corrected pupil.
    pub gain: f32,
}

/// The retouching of one edit on one level.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Retouch {
    pub spots: Vec<LevelSpot>,
    pub eyes: Vec<LevelEye>,
}

pub fn is_active(p: &EditParams) -> bool {
    !p.spots.is_empty() || !p.red_eye.is_empty()
}

/// Normalised Source point → continuous level coordinates (inverse of `map`).
pub fn to_level(map: &LevelMap, p: Pt) -> (f64, f64) {
    let m = &map.m;
    let (nx, ny) = (p.x as f64 - m[2], p.y as f64 - m[5]);
    let det = m[0] * m[4] - m[1] * m[3];
    ((m[4] * nx - m[1] * ny) / det, (m[0] * ny - m[3] * nx) / det)
}

/// Continuous level coordinates → normalised Source point.
pub fn to_norm(map: &LevelMap, (x, y): (f64, f64)) -> Pt {
    let m = &map.m;
    Pt {
        x: (m[0] * x + m[1] * y + m[2]) as f32,
        y: (m[3] * x + m[4] * y + m[5]) as f32,
    }
}

/// Level pixels per unit of a radius given as a fraction of the source long edge.
pub fn radius_scale(map: &LevelMap, level: usize) -> f64 {
    map.source.w.max(map.source.h) as f64 / 2f64.powi(level as i32)
}

impl Retouch {
    pub fn new(p: &EditParams, map: &LevelMap, level: usize) -> Retouch {
        let k = radius_scale(map, level);
        Retouch {
            spots: p
                .spots
                .iter()
                .map(|s| {
                    let radius = (s.radius as f64 * k).max(0.75);
                    LevelSpot {
                        heal: s.mode == SpotMode::Heal,
                        target: to_level(map, s.target),
                        source: to_level(map, s.source),
                        radius,
                        inner: radius * (1.0 - s.feather as f64 / 100.0),
                        opacity: s.opacity,
                    }
                })
                .collect(),
            eyes: p
                .red_eye
                .iter()
                .map(|e| LevelEye {
                    center: to_level(map, e.center),
                    radius: (e.radius as f64 * k).max(0.75),
                    threshold: 0.6 - 0.5 * e.pupil / 100.0,
                    gain: 1.0 - 0.85 * e.darken / 100.0,
                })
                .collect(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.spots.is_empty() && self.eyes.is_empty()
    }

    /// Whether any spot or eye changes pixels of rectangle `r` of the `lw × lh` level.
    pub fn touches(&self, r: Rect, lw: u32, lh: u32) -> bool {
        self.spots
            .iter()
            .filter_map(|s| s.rect(lw, lh))
            .chain(
                self.eyes
                    .iter()
                    .filter_map(|e| clip(disc_rect(e.center, e.radius, 0.0), lw, lh)),
            )
            .any(|a| intersects(a, r))
    }

    /// Red-eye correction of `data`, the pixels of level rectangle `r`.
    pub fn apply_eyes(&self, r: Rect, data: &mut PxVec) {
        for e in &self.eyes {
            let Some((x0, y0, x1, y1)) = overlap(disc_rect(e.center, e.radius, 0.0), r) else {
                continue;
            };
            let inner = e.radius * (1.0 - EYE_FEATHER);
            let px = data.as_px();
            let w = (x1 - x0) as usize;
            let fixed: Vec<(usize, Rgb)> = (0..w * (y1 - y0) as usize)
                .into_par_iter()
                .filter_map(|k| {
                    let (x, y) = (x0 + (k % w) as u32, y0 + (k / w) as u32);
                    let d = dist((x as f64 + 0.5, y as f64 + 0.5), e.center);
                    let edge = ramp(d, inner, e.radius);
                    if edge <= 0.0 {
                        return None;
                    }
                    let i = ((y - r.1) * r.2 + x - r.0) as usize;
                    let p = px.get(i);
                    let (fixed, m) = e.correct(p);
                    let a = m * edge;
                    (a > 0.0).then(|| (i, std::array::from_fn(|c| p[c] + (fixed[c] - p[c]) * a)))
                })
                .collect();
            for (i, v) in fixed {
                data.set(i, v);
            }
        }
    }
}

impl LevelEye {
    /// The corrected pixel and how strongly it counts as red pupil.
    fn correct(&self, p: Rgb) -> (Rgb, f32) {
        let [r, g, b] = p.map(|v| v.max(0.0));
        if r <= 1e-6 {
            return (p, 0.0);
        }
        let redness = (r - g.max(b)) / r;
        let m = smoothstep(self.threshold, self.threshold + 0.15, redness);
        let n = 0.5 * (g + b);
        ([n * self.gain, g * self.gain, b * self.gain], m)
    }
}

impl LevelSpot {
    /// Pixels the spot replaces, clipped to the `lw × lh` level.
    pub fn rect(&self, lw: u32, lh: u32) -> Option<Rect> {
        clip(disc_rect(self.target, self.radius, 0.0), lw, lh)
    }

    /// Pixels read around the target (the rim, for healing).
    pub fn target_read(&self, lw: u32, lh: u32) -> Option<Rect> {
        clip(disc_rect(self.target, self.radius, RIM), lw, lh)
    }

    /// Pixels read around the source, clamped into the level.
    pub fn source_read(&self, lw: u32, lh: u32) -> Rect {
        let (lo, hi) = disc_rect(self.source, self.radius, RIM);
        let cl = |v: i64, m: u32| v.clamp(0, m as i64 - 1) as u32;
        let (x0, y0) = (cl(lo.0, lw), cl(lo.1, lh));
        let (x1, y1) = (cl(hi.0 - 1, lw) + 1, cl(hi.1 - 1, lh) + 1);
        (x0, y0, x1 - x0, y1 - y0)
    }

    /// The whole retouched disc. `target` must cover [`Self::target_read`] (or the whole
    /// rect when healing is off it may be [`Self::rect`]), `source` [`Self::source_read`].
    pub fn patch(&self, rect: Rect, target: &Plane<'_>, source: &Plane<'_>) -> Patch {
        let (x0, y0, w, h) = rect;
        let (dx, dy) = (self.source.0 - self.target.0, self.source.1 - self.target.1);
        let membrane = self.heal.then(|| Membrane::new(self, rect, target, source));
        let (rgb, alpha) = (0..w as usize * h as usize)
            .into_par_iter()
            .map(|k| {
                let (x, y) = (x0 + (k % w as usize) as u32, y0 + (k / w as usize) as u32);
                let c = (x as f64 + 0.5, y as f64 + 0.5);
                let a = ramp(dist(c, self.target), self.inner, self.radius) * self.opacity;
                if a <= 0.0 {
                    return ([0.0; 3], 0.0);
                }
                let s = source.sample(c.0 + dx, c.1 + dy);
                let v = match &membrane {
                    Some(m) => {
                        let u = m.at(c);
                        std::array::from_fn(|k| decode(encode(s[k]) + u[k]))
                    }
                    None => s,
                };
                (v, a)
            })
            .unzip();
        Patch { rect, rgb, alpha }
    }
}

/// The retouched pixels of one spot and their blend weights.
pub struct Patch {
    pub rect: Rect,
    pub rgb: Vec<Rgb>,
    pub alpha: Vec<f32>,
}

impl Patch {
    pub fn bytes(&self) -> u64 {
        self.rgb.len() as u64 * 16
    }

    /// Blend into `data`, the pixels of level rectangle `r`.
    pub fn apply(&self, r: Rect, data: &mut PxVec) {
        let p = self.rect;
        let Some((x0, y0, x1, y1)) = overlap(
            (
                (p.0 as i64, p.1 as i64),
                ((p.0 + p.2) as i64, (p.1 + p.3) as i64),
            ),
            r,
        ) else {
            return;
        };
        for y in y0..y1 {
            for x in x0..x1 {
                let k = ((y - p.1) * p.2 + x - p.0) as usize;
                let a = self.alpha[k];
                if a <= 0.0 {
                    continue;
                }
                let i = ((y - r.1) * r.2 + x - r.0) as usize;
                let o = data.as_px().get(i);
                let v = self.rgb[k];
                data.set(i, std::array::from_fn(|c| o[c] + (v[c] - o[c]) * a));
            }
        }
    }
}

/// Pixels of a level rectangle, read with clamping.
#[derive(Clone, Copy)]
pub struct Plane<'a> {
    pub rect: Rect,
    pub data: Px<'a>,
}

impl Plane<'_> {
    fn at(&self, x: i64, y: i64) -> Rgb {
        let (x0, y0, w, h) = self.rect;
        let lx = (x - x0 as i64).clamp(0, w as i64 - 1);
        let ly = (y - y0 as i64).clamp(0, h as i64 - 1);
        self.data.get((ly * w as i64 + lx) as usize)
    }

    /// Bilinear sample at continuous coordinates.
    pub fn sample(&self, x: f64, y: f64) -> Rgb {
        let (u, v) = (x - 0.5, y - 0.5);
        let (i, j) = (u.floor(), v.floor());
        let (fx, fy) = ((u - i) as f32, (v - j) as f32);
        let (i, j) = (i as i64, j as i64);
        let (a, b, c, d) = (
            self.at(i, j),
            self.at(i + 1, j),
            self.at(i, j + 1),
            self.at(i + 1, j + 1),
        );
        std::array::from_fn(|k| {
            let top = a[k] + (b[k] - a[k]) * fx;
            let bot = c[k] + (d[k] - c[k]) * fx;
            top + (bot - top) * fy
        })
    }
}

/// The healing offset (encoded domain) on a node grid over the spot.
struct Membrane {
    x0: f64,
    y0: f64,
    step: f64,
    nx: usize,
    ny: usize,
    nodes: Vec<Rgb>,
}

impl Membrane {
    fn new(s: &LevelSpot, rect: Rect, target: &Plane<'_>, source: &Plane<'_>) -> Membrane {
        let r = s.radius;
        let n = ((std::f64::consts::TAU * r / RIM_STEP).ceil() as usize).clamp(12, MAX_RIM_SAMPLES);
        let dirs: Vec<(f64, f64)> = (0..n)
            .map(|k| {
                let a = std::f64::consts::TAU * k as f64 / n as f64;
                (a.cos(), a.sin())
            })
            .collect();
        // Target − source around the rim, averaged across it (just inside and just outside).
        let raw: Vec<[f64; 3]> = dirs
            .iter()
            .map(|&(c, sn)| {
                let mut d = [0.0f64; 3];
                for rho in [r - 0.5, r + 0.5, r + 1.5] {
                    let rho = rho.max(0.5);
                    let t = target.sample(s.target.0 + c * rho, s.target.1 + sn * rho);
                    let q = source.sample(s.source.0 + c * rho, s.source.1 + sn * rho);
                    for k in 0..3 {
                        d[k] += (encode(t[k]) - encode(q[k])) as f64 / 3.0;
                    }
                }
                d
            })
            .collect();
        let rim: Vec<[f64; 3]> = (0..n)
            .map(|k| {
                let (a, b, c) = (raw[(k + n - 1) % n], raw[k], raw[(k + 1) % n]);
                std::array::from_fn(|i| 0.25 * a[i] + 0.5 * b[i] + 0.25 * c[i])
            })
            .collect();
        // Keep the Poisson kernel wider than the rim spacing.
        let a_max = 1.0 - std::f64::consts::TAU / n as f64;
        let step = (2.0 * r / MEMBRANE_NODES).max(1.0);
        let (x0, y0) = (rect.0 as f64 + 0.5, rect.1 as f64 + 0.5);
        let nx = ((rect.2 as f64 - 1.0) / step).ceil() as usize + 2;
        let ny = ((rect.3 as f64 - 1.0) / step).ceil() as usize + 2;
        let nodes = (0..nx * ny)
            .into_par_iter()
            .map(|n| {
                let (i, j) = (n % nx, n / nx);
                let (mut dx, mut dy) = (
                    x0 + i as f64 * step - s.target.0,
                    y0 + j as f64 * step - s.target.1,
                );
                let rho = (dx * dx + dy * dy).sqrt();
                if rho > a_max * r {
                    let f = a_max * r / rho;
                    dx *= f;
                    dy *= f;
                }
                let a2 = (dx * dx + dy * dy) / (r * r);
                let mut acc = [0.0f64; 3];
                let mut wsum = 0.0;
                for (d, &(c, sn)) in rim.iter().zip(&dirs) {
                    let w = (1.0 - a2) / (1.0 - 2.0 * (dx * c + dy * sn) / r + a2);
                    wsum += w;
                    for k in 0..3 {
                        acc[k] += w * d[k];
                    }
                }
                acc.map(|v| (v / wsum) as f32)
            })
            .collect();
        Membrane {
            x0,
            y0,
            step,
            nx,
            ny,
            nodes,
        }
    }

    fn at(&self, (x, y): (f64, f64)) -> Rgb {
        let u = ((x - self.x0) / self.step).clamp(0.0, (self.nx - 1) as f64);
        let v = ((y - self.y0) / self.step).clamp(0.0, (self.ny - 1) as f64);
        let (i, j) = ((u as usize).min(self.nx - 2), (v as usize).min(self.ny - 2));
        let (fx, fy) = ((u - i as f64) as f32, (v - j as f64) as f32);
        let n = |i: usize, j: usize| self.nodes[j * self.nx + i];
        let (a, b, c, d) = (n(i, j), n(i + 1, j), n(i, j + 1), n(i + 1, j + 1));
        std::array::from_fn(|k| {
            let top = a[k] + (b[k] - a[k]) * fx;
            let bot = c[k] + (d[k] - c[k]) * fx;
            top + (bot - top) * fy
        })
    }
}

fn encode(v: f32) -> f32 {
    v.max(0.0).sqrt()
}

fn decode(v: f32) -> f32 {
    let v = v.max(0.0);
    v * v
}

fn dist(a: (f64, f64), b: (f64, f64)) -> f64 {
    ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
}

/// 1 inside `inner`, smoothly 0 at `outer`.
fn ramp(d: f64, inner: f64, outer: f64) -> f32 {
    if d >= outer {
        0.0
    } else if d <= inner {
        1.0
    } else {
        smoothstep(0.0, 1.0, ((outer - d) / (outer - inner)) as f32)
    }
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

type IRect = ((i64, i64), (i64, i64));

/// Pixel bounds `[lo, hi)` of the disc grown by `margin`.
fn disc_rect(c: (f64, f64), r: f64, margin: f64) -> IRect {
    let e = r + margin;
    (
        ((c.0 - e).floor() as i64, (c.1 - e).floor() as i64),
        ((c.0 + e).ceil() as i64, (c.1 + e).ceil() as i64),
    )
}

fn clip(((x0, y0), (x1, y1)): IRect, lw: u32, lh: u32) -> Option<Rect> {
    let (x0, y0) = (x0.max(0), y0.max(0));
    let (x1, y1) = (x1.min(lw as i64), y1.min(lh as i64));
    (x1 > x0 && y1 > y0).then(|| (x0 as u32, y0 as u32, (x1 - x0) as u32, (y1 - y0) as u32))
}

/// Intersection `(x0, y0, x1, y1)` of a pixel range and a rectangle.
fn overlap(((ax0, ay0), (ax1, ay1)): IRect, r: Rect) -> Option<(u32, u32, u32, u32)> {
    let x0 = ax0.max(r.0 as i64);
    let y0 = ay0.max(r.1 as i64);
    let x1 = ax1.min((r.0 + r.2) as i64);
    let y1 = ay1.min((r.1 + r.3) as i64);
    (x1 > x0 && y1 > y0).then_some((x0 as u32, y0 as u32, x1 as u32, y1 as u32))
}

/// Whether two rectangles overlap.
pub fn intersects(a: Rect, b: Rect) -> bool {
    a.0 < b.0 + b.2 && b.0 < a.0 + a.2 && a.1 < b.1 + b.3 && b.1 < a.1 + a.3
}

/// Level radius the source search works at.
const SEARCH_RADIUS: f64 = 12.0;
const SEARCH_ANGLES: usize = 24;

/// A source for a new spot at `target` (radius as a fraction of the long edge): the nearby
/// disc whose surroundings best match the target's surroundings and whose interior has no
/// blob of its own, away from the image edge and from other spots' targets.
pub fn find_source(src: &SourceImage, p: &EditParams, target: Pt, radius: f32) -> Pt {
    let delta = Orientation::from_delta(p.geometry.orientation_delta);
    let base = LevelMap::new(src.size(), delta, 0);
    let r0 = radius as f64 * radius_scale(&base, 0);
    let levels = &src.pyramid.levels;
    let level = ((r0 / SEARCH_RADIUS).log2().floor().max(0.0) as usize).min(levels.len() - 1);
    let lvl = &levels[level];
    let img = crate::backend::LevelView::whole(lvl);
    let map = LevelMap::new(src.size(), delta, level);
    let k = radius_scale(&map, level);
    let r = (radius as f64 * k).max(1.0);
    let t = to_level(&map, target);
    let (w, h) = (lvl.width as f64, lvl.height as f64);
    let luma = |x: f64, y: f64| {
        let v = aigle_kernels::resample::bilinear(&img, (x - 0.5) as f32, (y - 0.5) as f32);
        ((v[0] + v[1] + v[2]) / 3.0).max(0.0).sqrt() as f64
    };
    let ring = |c: (f64, f64), radii: &[f64]| -> Vec<f64> {
        radii
            .iter()
            .flat_map(|&f| {
                (0..SEARCH_ANGLES).map(move |a| {
                    let th = std::f64::consts::TAU * a as f64 / SEARCH_ANGLES as f64;
                    (c.0 + f * r * th.cos(), c.1 + f * r * th.sin())
                })
            })
            .map(|(x, y)| luma(x, y))
            .collect()
    };
    let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len() as f64;
    let std = |v: &[f64]| {
        let m = mean(v);
        (v.iter().map(|x| (x - m).powi(2)).sum::<f64>() / v.len() as f64).sqrt()
    };
    let outer = [1.2, 1.5];
    let ring_t = ring(t, &outer);
    let (mt, st) = (mean(&ring_t), std(&ring_t));
    let others: Vec<((f64, f64), f64)> = p
        .spots
        .iter()
        .map(|s| (to_level(&map, s.target), s.radius as f64 * k))
        .collect();
    let mut best: Option<(f64, (f64, f64))> = None;
    for d in [2.2, 3.0, 4.0, 5.5, 7.5] {
        for a in 0..16 {
            let th = std::f64::consts::TAU * (a as f64 + 0.5 * (d > 3.5) as u8 as f64) / 16.0;
            let c = (t.0 + d * r * th.cos(), t.1 + d * r * th.sin());
            let m = 1.6 * r;
            if c.0 < m || c.1 < m || c.0 > w - m || c.1 > h - m {
                continue;
            }
            if others.iter().any(|&(o, ro)| dist(c, o) < r + ro) {
                continue;
            }
            let ring_c = ring(c, &outer);
            let inner = ring(c, &[0.0, 0.35, 0.7]);
            let mc = mean(&ring_c);
            let shape = ring_t
                .iter()
                .zip(&ring_c)
                .map(|(a, b)| (a - mt - (b - mc)).powi(2))
                .sum::<f64>()
                / ring_t.len() as f64;
            let level_diff = (mt - mc).powi(2);
            let blob = (mean(&inner) - mc).powi(2);
            let texture = (std(&inner) - st).powi(2);
            let score =
                (shape + 0.5 * level_diff + 2.0 * blob + texture) * (1.0 + 0.04 * (d - 2.2));
            if best.is_none_or(|(b, _)| score < b) {
                best = Some((score, c));
            }
        }
    }
    let c = best.map(|b| b.1).unwrap_or_else(|| {
        // Towards the image centre.
        let (dx, dy) = (w / 2.0 - t.0, h / 2.0 - t.1);
        let n = (dx * dx + dy * dy).sqrt().max(1e-9);
        (t.0 + 2.5 * r * dx / n, t.1 + 2.5 * r * dy / n)
    });
    let q = to_norm(&map, c);
    Pt {
        x: q.x.clamp(0.0, 1.0),
        y: q.y.clamp(0.0, 1.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aigle_core::geometry::{Orientation, Size};
    use aigle_core::retouch::{RedEye, Spot};

    fn plane(w: u32, h: u32, f: impl Fn(u32, u32) -> Rgb) -> Vec<Rgb> {
        (0..h)
            .flat_map(|y| (0..w).map(move |x| (x, y)))
            .map(|(x, y)| f(x, y))
            .collect()
    }

    fn render(p: &EditParams, w: u32, h: u32, img: &[Rgb]) -> Vec<Rgb> {
        let map = LevelMap::new(Size::new(w, h), Orientation::default(), 0);
        let rt = Retouch::new(p, &map, 0);
        let full = (0, 0, w, h);
        let src = Plane {
            rect: full,
            data: Px::F(img),
        };
        let mut data = PxVec::F(img.to_vec());
        for s in &rt.spots {
            let rect = s.rect(w, h).unwrap();
            s.patch(rect, &src, &src).apply(full, &mut data);
        }
        rt.apply_eyes(full, &mut data);
        match data {
            PxVec::F(v) => v,
            PxVec::H(_) => unreachable!(),
        }
    }

    fn pt(x: f32, y: f32) -> Pt {
        Pt { x, y }
    }

    #[test]
    fn touches_only_rects_the_discs_reach() {
        let map = LevelMap::new(Size::new(400, 200), Orientation::default(), 0);
        let mut p = EditParams::default();
        p.spots.push(Spot::new(
            SpotMode::Heal,
            pt(0.25, 0.5),
            pt(0.75, 0.5),
            10.0 / 400.0,
        ));
        p.red_eye.push(RedEye::new(pt(0.9, 0.1), 5.0 / 400.0));
        let rt = Retouch::new(&p, &map, 0);
        assert!(rt.touches((95, 95, 10, 10), 400, 200));
        assert!(rt.touches((355, 15, 10, 10), 400, 200));
        // The spot's source is read, not changed.
        assert!(!rt.touches((295, 95, 10, 10), 400, 200));
        assert!(!rt.touches((0, 0, 50, 50), 400, 200));
    }

    #[test]
    fn level_coordinates_round_trip() {
        let stored = Size::new(300, 200);
        for q in 0..4 {
            let o = Orientation::from_delta(aigle_core::params::OrientationDelta {
                quarter_turns: q,
                flip: q % 2 == 1,
            });
            let map = LevelMap::new(stored, o, 1);
            let p = pt(0.3, 0.8);
            let l = to_level(&map, p);
            let back = to_norm(&map, l);
            assert!((back.x - p.x).abs() < 1e-6 && (back.y - p.y).abs() < 1e-6);
        }
    }

    /// A dark blob on a horizontal gradient healed from a clean area takes on the target's
    /// local brightness; the source texture survives; outside the disc nothing changes.
    #[test]
    fn heal_matches_the_surroundings() {
        let (w, h) = (120, 80);
        let img = plane(w, h, |x, y| {
            let base = 0.1 + 0.004 * x as f32;
            let tex = if (x + y) % 2 == 0 { 0.01 } else { -0.01 };
            let d = ((x as f32 - 30.0).powi(2) + (y as f32 - 40.0).powi(2)).sqrt();
            let blob = if d < 5.0 { 0.2 } else { 1.0 };
            [base * blob + tex; 3]
        });
        let mut p = EditParams::default();
        let mut s = Spot::new(
            SpotMode::Heal,
            pt(30.0 / 120.0, 0.5),
            pt(90.0 / 120.0, 0.5),
            8.0 / 120.0,
        );
        s.feather = 0.0;
        p.spots.push(s);
        let out = render(&p, w, h, &img);
        let at = |x: u32, y: u32| out[(y * w + x) as usize][0];
        let expect = |x: u32| 0.1 + 0.004 * x as f32;
        for x in 26..35 {
            let avg = 0.5 * (at(x, 40) + at(x, 41));
            assert!(
                (avg - expect(x)).abs() < 0.02,
                "x {x}: {avg} vs {}",
                expect(x)
            );
        }
        // Texture (checkerboard) is kept.
        assert!((at(30, 40) - at(31, 40)).abs() > 0.005);
        assert_eq!(out[(10 * w + 10) as usize], img[(10 * w + 10) as usize]);
        assert_eq!(out[(40 * w + 45) as usize], img[(40 * w + 45) as usize]);
    }

    #[test]
    fn clone_copies_the_source() {
        let (w, h) = (64, 64);
        let img = plane(w, h, |x, y| [x as f32 / 64.0, y as f32 / 64.0, 0.5]);
        let mut p = EditParams::default();
        let mut s = Spot::new(SpotMode::Clone, pt(0.25, 0.25), pt(0.75, 0.5), 6.0 / 64.0);
        s.feather = 0.0;
        p.spots.push(s);
        let out = render(&p, w, h, &img);
        let o = out[(16 * w + 16) as usize];
        let i = img[(32 * w + 48) as usize];
        assert!((0..3).all(|k| (o[k] - i[k]).abs() < 1e-5), "{o:?} vs {i:?}");
    }

    #[test]
    fn red_eye_fixes_red_pupils_only() {
        let (w, h) = (40, 40);
        let img = plane(w, h, |x, y| {
            if (18..22).contains(&x) && (18..22).contains(&y) {
                [0.6, 0.08, 0.1]
            } else if x < 20 {
                [0.3, 0.25, 0.2]
            } else {
                [0.1, 0.2, 0.4]
            }
        });
        let mut p = EditParams::default();
        p.red_eye.push(RedEye::new(pt(0.5, 0.5), 8.0 / 40.0));
        let out = render(&p, w, h, &img);
        let pupil = out[(20 * w + 20) as usize];
        assert!(pupil[0] < 0.1 && pupil[0] <= pupil[2] + 0.01, "{pupil:?}");
        assert_eq!(out[(15 * w + 15) as usize], img[(15 * w + 15) as usize]);
        assert_eq!(out[(15 * w + 24) as usize], img[(15 * w + 24) as usize]);
    }
}
