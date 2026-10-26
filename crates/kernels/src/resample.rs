//! Stencil access and resampling (B1, E2; doc 02 §5.5).
//!
//! * [`bilinear`]: Draft renders.
//! * [`lanczos3`]: Refined/Full. Lanczos-3 reconstruction whose support is widened by the
//!   local footprint `k` (source px per output px, from the Jacobian of the B1 map) for
//!   minification, so the kernel also low-passes; `k` is clamped to [1, [`MAX_FOOTPRINT`]]
//!   because B1 always samples a pyramid level at most 2× finer than the grid. The result is
//!   clamped to the range of the 2×2 nearest samples (anti-ringing), so edges stay sharp
//!   without halos.

use crate::Rgb;

/// Read-only 2D RGB accessor. Implemented by host tiles (f16/f32) and, later, GPU textures.
pub trait Rgb2D {
    fn width(&self) -> u32;
    fn height(&self) -> u32;
    /// `x < width`, `y < height`.
    fn load(&self, x: u32, y: u32) -> Rgb;
}

/// Symmetric (mirror) boundary: … 1 0 | 0 1 2 … w-1 | w-1 w-2 …
#[inline]
pub fn mirror(i: i32, n: u32) -> u32 {
    let n = n as i32;
    if n == 1 {
        return 0;
    }
    let period = 2 * n;
    let mut m = i.rem_euclid(period);
    if m >= n {
        m = period - 1 - m;
    }
    m as u32
}

/// Bilinear sample at continuous pixel-centre coordinates (`u = 0` is the centre of column 0).
#[inline]
pub fn bilinear<S: Rgb2D>(src: &S, u: f32, v: f32) -> Rgb {
    let x0 = crate::math::floor(u);
    let y0 = crate::math::floor(v);
    let fx = u - x0;
    let fy = v - y0;
    let (w, h) = (src.width(), src.height());
    let xa = mirror(x0 as i32, w);
    let xb = mirror(x0 as i32 + 1, w);
    let ya = mirror(y0 as i32, h);
    let yb = mirror(y0 as i32 + 1, h);
    let p00 = src.load(xa, ya);
    let p10 = src.load(xb, ya);
    let p01 = src.load(xa, yb);
    let p11 = src.load(xb, yb);
    let mut out = [0.0; 3];
    for c in 0..3 {
        let top = p00[c] + (p10[c] - p00[c]) * fx;
        let bot = p01[c] + (p11[c] - p01[c]) * fx;
        out[c] = top + (bot - top) * fy;
    }
    out
}

/// Largest footprint the Lanczos kernel is widened to.
pub const MAX_FOOTPRINT: f32 = 2.0;
/// Weights per axis at `MAX_FOOTPRINT`: 2·3·2 + 1.
const MAX_TAPS: usize = 13;

/// Lanczos-3 kernel.
#[inline]
pub fn lanczos(x: f32) -> f32 {
    let ax = if x < 0.0 { -x } else { x };
    if ax < 1e-5 {
        return 1.0;
    }
    if ax >= 3.0 {
        return 0.0;
    }
    let px = core::f32::consts::PI * x;
    3.0 * crate::math::sin(px) * crate::math::sin(px / 3.0) / (px * px)
}

/// Source support radius (px) of [`lanczos3`] at footprint `k`, for ROI computation.
pub fn lanczos_radius(k: f32) -> u32 {
    let k = k.max(1.0).min(MAX_FOOTPRINT);
    (3.0 * k) as u32 + 1
}

/// Lanczos-3 sample at pixel-centre coordinates (`u = 0` is the centre of column 0) with
/// footprint `k`, mirror boundary, anti-ringing clamp.
#[inline]
pub fn lanczos3<S: Rgb2D>(src: &S, u: f32, v: f32, k: f32) -> Rgb {
    let k = k.max(1.0).min(MAX_FOOTPRINT);
    let r = 3.0 * k;
    let xs = crate::math::floor(u - r) as i32 + 1;
    let ys = crate::math::floor(v - r) as i32 + 1;
    let mut nx = (crate::math::floor(u + r) as i32 - xs + 1) as usize;
    let mut ny = (crate::math::floor(v + r) as i32 - ys + 1) as usize;
    if nx > MAX_TAPS {
        nx = MAX_TAPS;
    }
    if ny > MAX_TAPS {
        ny = MAX_TAPS;
    }
    let inv = 1.0 / k;
    let mut wx = [0f32; MAX_TAPS];
    let mut wy = [0f32; MAX_TAPS];
    let mut i = 0;
    while i < nx {
        wx[i] = lanczos(((xs + i as i32) as f32 - u) * inv);
        i += 1;
    }
    let mut j = 0;
    while j < ny {
        wy[j] = lanczos(((ys + j as i32) as f32 - v) * inv);
        j += 1;
    }
    let (w, h) = (src.width(), src.height());
    let mut acc = [0f32; 3];
    let mut wsum = 0f32;
    j = 0;
    while j < ny {
        let yy = mirror(ys + j as i32, h);
        let mut row = [0f32; 3];
        let mut rs = 0f32;
        i = 0;
        while i < nx {
            let p = src.load(mirror(xs + i as i32, w), yy);
            let wt = wx[i];
            row[0] += wt * p[0];
            row[1] += wt * p[1];
            row[2] += wt * p[2];
            rs += wt;
            i += 1;
        }
        acc[0] += wy[j] * row[0];
        acc[1] += wy[j] * row[1];
        acc[2] += wy[j] * row[2];
        wsum += wy[j] * rs;
        j += 1;
    }
    let inv_w = if wsum != 0.0 { 1.0 / wsum } else { 0.0 };
    // Anti-ringing: clamp to the 2×2 neighbourhood.
    let x0 = crate::math::floor(u) as i32;
    let y0 = crate::math::floor(v) as i32;
    let a = src.load(mirror(x0, w), mirror(y0, h));
    let b = src.load(mirror(x0 + 1, w), mirror(y0, h));
    let c = src.load(mirror(x0, w), mirror(y0 + 1, h));
    let d = src.load(mirror(x0 + 1, w), mirror(y0 + 1, h));
    let mut out = [0f32; 3];
    let mut ch = 0;
    while ch < 3 {
        let lo = a[ch].min(b[ch]).min(c[ch]).min(d[ch]);
        let hi = a[ch].max(b[ch]).max(c[ch]).max(d[ch]);
        out[ch] = (acc[ch] * inv_w).max(lo).min(hi);
        ch += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Ramp;
    impl Rgb2D for Ramp {
        fn width(&self) -> u32 {
            4
        }
        fn height(&self) -> u32 {
            2
        }
        fn load(&self, x: u32, y: u32) -> Rgb {
            [x as f32, y as f32, 0.0]
        }
    }

    #[test]
    fn mirror_boundary() {
        let got: [u32; 8] = core::array::from_fn(|i| mirror(i as i32 - 2, 3));
        assert_eq!(got, [1, 0, 0, 1, 2, 2, 1, 0]);
    }

    #[test]
    fn lanczos_is_interpolating_and_bounded() {
        // On sample positions Lanczos returns the sample; on a ramp it is exact in the interior.
        assert_eq!(lanczos3(&Ramp, 2.0, 1.0, 1.0), [2.0, 1.0, 0.0]);
        let p = lanczos3(&Ramp, 1.5, 0.5, 1.0);
        assert!(
            (p[0] - 1.5).abs() < 0.05 && (p[1] - 0.5).abs() < 0.05,
            "{p:?}"
        );
        // Widened kernel smooths but stays within the 2×2 range.
        let q = lanczos3(&Ramp, 1.3, 0.2, 2.0);
        assert!((1.0..=2.0).contains(&q[0]) && (0.0..=1.0).contains(&q[1]));
        assert_eq!(lanczos_radius(1.0), 4);
        assert_eq!(lanczos_radius(5.0), 7);
    }

    #[test]
    fn bilinear_interpolates() {
        let p = bilinear(&Ramp, 1.5, 0.25);
        assert!((p[0] - 1.5).abs() < 1e-6 && (p[1] - 0.25).abs() < 1e-6);
        assert_eq!(bilinear(&Ramp, 2.0, 1.0), [2.0, 1.0, 0.0]);
    }
}
