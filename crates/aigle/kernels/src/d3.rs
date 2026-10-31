//! D3: effects (doc 02 §3) — post-crop vignette and film grain, on the bounded
//! display-referred D2 output (linear Rec.2020, 0..1).
//!
//! * **Vignette** is defined on the crop: an elliptical (roundness 0) to circular (roundness
//!   +100) or more rectangular (−100) distance from the crop centre, with a smooth falloff
//!   between `midpoint` and `midpoint + feather`. Negative amounts darken towards the edge
//!   (multiplicative), positive amounts lighten towards white.
//! * **Grain** is two-octave value noise anchored to Crop-space source pixels, so it is the
//!   same at 1:1 preview and in export. `size` sets the cell size (0.6–3 source px). Below
//!   1:1, when a cell is smaller than a grid pixel, the noise is evaluated on the grid lattice
//!   and its amplitude scaled by the expected standard deviation of the average of the cells
//!   that fall into one grid pixel (perceptual approximation, doc 02 §8). Grain is applied
//!   to luminance only, weighted towards the midtones.

use crate::Rgb;
use crate::dither::hash32;
use crate::math::{clamp01, exp2, floor, luminance, smoothstep, sqrt};

pub const VIGNETTE: u32 = 1;
pub const GRAIN: u32 = 2;

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct D3Params {
    /// Bit set of [`VIGNETTE`] and [`GRAIN`].
    pub active: u32,
    /// Crop centre in grid px and inverse half-extents (roundness folded in).
    pub cx: f32,
    pub cy: f32,
    pub inv_rx: f32,
    pub inv_ry: f32,
    /// −1..1.
    pub vig_amount: f32,
    /// Normalised distance where the falloff starts, and its width.
    pub vig_mid: f32,
    pub vig_width: f32,
    /// Effective amplitude (already scaled for the render scale).
    pub grain_amount: f32,
    /// Lattice cell in grid px (≥ 1).
    pub grain_cell: f32,
    /// 0..1: weight of the fine octave.
    pub grain_rough: f32,
}

impl D3Params {
    /// `s` = render scale (grid px per source px); `gw`, `gh` = crop size in grid px.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        amount: f32,
        midpoint: f32,
        roundness: f32,
        feather: f32,
        grain: f32,
        size: f32,
        roughness: f32,
        s: f32,
        gw: f32,
        gh: f32,
    ) -> D3Params {
        let mut active = 0;
        if amount != 0.0 {
            active |= VIGNETTE;
        }
        if grain != 0.0 {
            active |= GRAIN;
        }
        let (rx, ry) = (gw * 0.5, gh * 0.5);
        // Roundness +1: circle of the short half-extent; −1: stretched past the frame.
        let r = roundness / 100.0;
        let m = rx.min(ry);
        let (ex, ey) = if r >= 0.0 {
            (rx + (m - rx) * r, ry + (m - ry) * r)
        } else {
            (rx * (1.0 - 0.5 * r), ry * (1.0 - 0.5 * r))
        };
        let cell_src = 0.6 + 2.4 * size / 100.0;
        let cell_grid = cell_src * s;
        D3Params {
            active,
            cx: rx,
            cy: ry,
            inv_rx: 1.0 / ex.max(1e-3),
            inv_ry: 1.0 / ey.max(1e-3),
            vig_amount: amount / 100.0,
            vig_mid: 0.2 + 0.8 * midpoint / 100.0,
            vig_width: 0.05 + 1.2 * feather / 100.0,
            grain_amount: grain / 100.0 * 1.0 * cell_grid.min(1.0),
            grain_cell: cell_grid.max(1.0),
            grain_rough: roughness / 100.0,
        }
    }
}

#[inline]
fn lattice(ix: i32, iy: i32, seed: u32) -> f32 {
    let h = hash32(ix as u32 ^ hash32(iy as u32 ^ hash32(seed)));
    (h >> 8) as f32 * (2.0 / 16_777_216.0) - 1.0
}

/// Smooth value noise in [-1, 1] at lattice coordinates.
#[inline]
pub fn value_noise(x: f32, y: f32, seed: u32) -> f32 {
    let (x0, y0) = (floor(x), floor(y));
    let (fx, fy) = (x - x0, y - y0);
    let (sx, sy) = (fx * fx * (3.0 - 2.0 * fx), fy * fy * (3.0 - 2.0 * fy));
    let (ix, iy) = (x0 as i32, y0 as i32);
    let a = lattice(ix, iy, seed);
    let b = lattice(ix + 1, iy, seed);
    let c = lattice(ix, iy + 1, seed);
    let d = lattice(ix + 1, iy + 1, seed);
    let top = a + (b - a) * sx;
    let bot = c + (d - c) * sx;
    top + (bot - top) * sy
}

#[inline]
pub fn d3(px: Rgb, gx: u32, gy: u32, p: &D3Params, seed: u32) -> Rgb {
    let mut v = px;
    let (x, y) = (gx as f32 + 0.5, gy as f32 + 0.5);
    if p.active & VIGNETTE != 0 {
        let u = (x - p.cx) * p.inv_rx;
        let w = (y - p.cy) * p.inv_ry;
        let d = sqrt(u * u + w * w);
        let t = smoothstep(p.vig_mid, p.vig_mid + p.vig_width, d);
        if p.vig_amount < 0.0 {
            let k = exp2(3.0 * p.vig_amount * t);
            v = [v[0] * k, v[1] * k, v[2] * k];
        } else {
            let k = p.vig_amount * t;
            v = [
                v[0] + (1.0 - v[0]) * k,
                v[1] + (1.0 - v[1]) * k,
                v[2] + (1.0 - v[2]) * k,
            ];
        }
    }
    if p.active & GRAIN != 0 {
        let (gxc, gyc) = (x / p.grain_cell, y / p.grain_cell);
        let coarse = value_noise(gxc, gyc, seed ^ 0x6a09_e667);
        let fine = value_noise(gxc * 2.0 + 17.0, gyc * 2.0 + 31.0, seed ^ 0xbb67_ae85);
        let n = coarse * (1.0 - 0.5 * p.grain_rough) + fine * p.grain_rough * 0.7;
        let l = clamp01(luminance(v));
        let mid = 4.0 * l * (1.0 - l) + 0.15;
        let k = exp2(p.grain_amount * n * mid);
        v = [v[0] * k, v[1] * k, v[2] * k];
    }
    [clamp01(v[0]), clamp01(v[1]), clamp01(v[2])]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vignette_darkens_corners_only() {
        let p = D3Params::new(-80.0, 50.0, 0.0, 50.0, 0.0, 25.0, 50.0, 1.0, 1000.0, 600.0);
        let c = d3([0.5; 3], 500, 300, &p, 1);
        let e = d3([0.5; 3], 2, 2, &p, 1);
        assert!((c[0] - 0.5).abs() < 1e-6);
        assert!(e[0] < 0.3, "{e:?}");
        let p = D3Params::new(60.0, 50.0, 0.0, 50.0, 0.0, 25.0, 50.0, 1.0, 1000.0, 600.0);
        assert!(d3([0.5; 3], 2, 2, &p, 1)[0] > 0.6);
    }

    #[test]
    fn grain_is_zero_mean_and_bounded() {
        let p = D3Params::new(0.0, 50.0, 0.0, 50.0, 60.0, 25.0, 50.0, 1.0, 500.0, 500.0);
        let (mut sum, mut var) = (0f64, 0f64);
        for y in 0..200 {
            for x in 0..200 {
                let v = d3([0.18; 3], x, y, &p, 7)[1] as f64 - 0.18;
                sum += v;
                var += v * v;
            }
        }
        let n = 40000.0;
        assert!((sum / n).abs() < 0.004, "mean {}", sum / n);
        assert!((var / n).sqrt() > 0.01);
        // Below 1:1 the amplitude shrinks.
        let small = D3Params::new(0.0, 50.0, 0.0, 50.0, 60.0, 25.0, 50.0, 0.25, 500.0, 500.0);
        assert!(small.grain_amount < p.grain_amount * 0.5);
    }
}
