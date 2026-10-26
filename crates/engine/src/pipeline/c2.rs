//! C2 coarse field (doc 02 §3, §5.4, §8): the large-support part of dehaze, highlights/shadows
//! and clarity, computed once per (source, C1/C2 params) on a coarse pyramid level covering
//! the whole source, and sampled per pixel by the C1 tile kernel (`aigle_kernels::c2`).
//!
//! The field level is the finest one with a long edge ≤ 1024 px, so its cost is a few ms
//! regardless of sensor size, and — unlike a render-resolution evaluation — it does not depend
//! on the render scale: preview at any zoom and export see the same field. Edge-awareness
//! comes from the fast guided filter: the coarse level only carries the `a, b` coefficients;
//! the full-resolution pixel is the guide.
//!
//! Radii are fractions of the crop's long edge (doc 02 §8): tone 3 %, clarity 0.8 %, dehaze
//! dark-channel patch 0.4 % and transmission refinement 1.5 %, floored at one coarse pixel.
//! Dehaze airlight is a statistic over the crop (doc 02 §5.1): the mean colour of the 0.1 %
//! brightest dark-channel pixels inside the crop, on the field level.

use aigle_core::geometry::Point;
use aigle_kernels::Rgb;
use aigle_kernels::c2::{C2Params, FIELD_CH, dehaze_px, log_lum};
use rayon::prelude::*;

use super::Prepared;
use crate::backend::Aux;
use crate::detail::guided_coeffs;
use crate::source::SourceImage;

/// Implementation version of the field (doc 02 §13).
pub const C2_IMPL: u32 = 1;
const FIELD_MAX_EDGE: u32 = 1024;

pub struct C2Field {
    /// Pyramid level the field samples.
    pub level: usize,
    pub w: u32,
    pub h: u32,
    /// `FIELD_CH` floats per pixel: a_tone, b_tone, a_clarity, b_clarity, transmission.
    pub aux: Aux,
    pub airlight: Rgb,
}

impl C2Field {
    pub fn bytes(&self) -> u64 {
        self.aux.data.len() as u64 * 4
    }
}

fn field_level(src: &SourceImage) -> usize {
    let levels = &src.pyramid.levels;
    levels
        .iter()
        .position(|l| l.width.max(l.height) <= FIELD_MAX_EDGE)
        .unwrap_or(levels.len() - 1)
}

/// Separable min filter of radius `r`, window clipped at the edges.
fn min_filter(v: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let mut t = vec![0f32; w * h];
    t.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for (x, o) in row.iter_mut().enumerate() {
            let (a, b) = (x.saturating_sub(r), (x + r + 1).min(w));
            *o = v[y * w + a..y * w + b]
                .iter()
                .cloned()
                .fold(f32::MAX, f32::min);
        }
    });
    let mut out = vec![0f32; w * h];
    out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let (a, b) = (y.saturating_sub(r), (y + r + 1).min(h));
        for (x, o) in row.iter_mut().enumerate() {
            *o = (a..b).map(|yy| t[yy * w + x]).fold(f32::MAX, f32::min);
        }
    });
    out
}

/// `c2` holds the global params; `prep.uses` adds what local adjustments need.
pub fn compute(src: &SourceImage, prep: &Prepared, c2: &C2Params) -> C2Field {
    let u = prep.uses;
    let need_dehaze = c2.dehaze != 0.0 || u.dehaze;
    let need_tone = c2.shadows != 0.0 || c2.highlights != 0.0 || u.tone;
    let need_clarity = c2.clarity != 0.0 || u.clarity;
    let level = field_level(src);
    let lvl = &src.pyramid.levels[level];
    let (w, h) = (lvl.width as usize, lvl.height as usize);
    let step = 2f64.powi(level as i32);
    let (cw, ch) = prep.crop_extent();
    let long = cw.max(ch) / step;
    let rad = |frac: f64| ((frac * long).round() as usize).max(1);

    let mut rgb: Vec<Rgb> = lvl
        .data
        .par_iter()
        .map(|p| prep.c1.apply([p[0].to_f32(), p[1].to_f32(), p[2].to_f32()]))
        .collect();

    let mut t = vec![1.0f32; w * h];
    let mut airlight = [1.0f32; 3];
    if need_dehaze {
        let in_crop: Vec<bool> = (0..w * h)
            .into_par_iter()
            .map(|i| {
                let q = prep.stored_to_crop(Point::new(
                    ((i % w) as f64 + 0.5) * step,
                    ((i / w) as f64 + 0.5) * step,
                ));
                q.x >= 0.0 && q.y >= 0.0 && q.x < cw && q.y < ch
            })
            .collect();
        let dark: Vec<f32> = rgb.iter().map(|p| p[0].min(p[1]).min(p[2])).collect();
        let mut idx: Vec<usize> = (0..w * h).filter(|&i| in_crop[i]).collect();
        if idx.is_empty() {
            idx = (0..w * h).collect();
        }
        idx.sort_unstable_by(|&a, &b| dark[b].total_cmp(&dark[a]).then(a.cmp(&b)));
        let n = (idx.len() / 1000).max(1);
        let mut acc = [0f64; 3];
        for &i in &idx[..n] {
            for c in 0..3 {
                acc[c] += rgb[i][c] as f64;
            }
        }
        airlight = acc.map(|v| ((v / n as f64) as f32).max(1e-4));
        let dn: Vec<f32> = rgb
            .iter()
            .map(|p| {
                (p[0] / airlight[0])
                    .min(p[1] / airlight[1])
                    .min(p[2] / airlight[2])
            })
            .collect();
        let dmin = min_filter(&dn, w, h, rad(0.004));
        let traw: Vec<f32> = dmin.iter().map(|d| 1.0 - 0.95 * d).collect();
        let guide: Vec<f32> = rgb.iter().map(|p| log_lum(*p)).collect();
        let (a, b) = guided_coeffs(&guide, &traw, w, h, rad(0.015), 1e-2);
        t = (0..w * h)
            .map(|i| (a[i] * guide[i] + b[i]).clamp(0.05, 1.0))
            .collect();
        let mut cp = *c2;
        cp.airlight = airlight;
        rgb.par_iter_mut()
            .zip(&t)
            .for_each(|(p, &ti)| *p = dehaze_px(*p, ti, &cp));
    }

    let l: Vec<f32> = rgb.par_iter().map(|p| log_lum(*p)).collect();
    let zero = || (vec![0f32; w * h], vec![0f32; w * h]);
    let (at, bt) = if need_tone {
        guided_coeffs(&l, &l, w, h, rad(0.03), 0.05)
    } else {
        zero()
    };
    let (ac, bc) = if need_clarity {
        guided_coeffs(&l, &l, w, h, rad(0.008), 0.02)
    } else {
        zero()
    };
    let mut data = vec![0f32; w * h * FIELD_CH];
    data.par_chunks_mut(FIELD_CH)
        .enumerate()
        .for_each(|(i, o)| {
            o.copy_from_slice(&[at[i], bt[i], ac[i], bc[i], t[i]]);
        });
    C2Field {
        level,
        w: w as u32,
        h: h as u32,
        aux: Aux::new(data),
        airlight,
    }
}
