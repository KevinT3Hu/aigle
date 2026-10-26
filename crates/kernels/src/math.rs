//! Scalar math shared by the host and SPIR-V builds.
//!
//! Transcendentals come from `libm` on the host (bitwise reproducible across machines, doc 02
//! §14) and from GLSL.std.450 on the GPU (within the GPU tolerance of the CPU reference).
//! No `f32::clamp`: it asserts `min <= max`, which adds a panic path to every shader.

#[cfg(target_arch = "spirv")]
use spirv_std::num_traits::Float;

use crate::{Mat3f, Rgb};

/// Rec.2020 luminance weights.
pub const REC2020_LUMA: Rgb = [0.2627, 0.6780, 0.0593];

#[inline]
pub fn mat3_mul(m: &Mat3f, v: Rgb) -> Rgb {
    [
        m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
        m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
        m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2],
    ]
}

#[inline]
pub fn luminance(v: Rgb) -> f32 {
    REC2020_LUMA[0] * v[0] + REC2020_LUMA[1] * v[1] + REC2020_LUMA[2] * v[2]
}

/// Clamp to [0, 1]; NaN maps to 0.
#[inline]
pub fn clamp01(v: f32) -> f32 {
    v.max(0.0).min(1.0)
}

#[inline]
pub fn powf(x: f32, y: f32) -> f32 {
    #[cfg(not(target_arch = "spirv"))]
    {
        libm::powf(x, y)
    }
    #[cfg(target_arch = "spirv")]
    {
        x.powf(y)
    }
}

#[inline]
pub fn exp2(x: f32) -> f32 {
    #[cfg(not(target_arch = "spirv"))]
    {
        libm::exp2f(x)
    }
    #[cfg(target_arch = "spirv")]
    {
        x.exp2()
    }
}

#[inline]
pub fn floor(x: f32) -> f32 {
    #[cfg(not(target_arch = "spirv"))]
    {
        libm::floorf(x)
    }
    #[cfg(target_arch = "spirv")]
    {
        x.floor()
    }
}

macro_rules! unary {
    ($name:ident, $libm:ident, $m:ident) => {
        #[inline]
        pub fn $name(x: f32) -> f32 {
            #[cfg(not(target_arch = "spirv"))]
            {
                libm::$libm(x)
            }
            #[cfg(target_arch = "spirv")]
            {
                x.$m()
            }
        }
    };
}

unary!(log2, log2f, log2);
unary!(sqrt, sqrtf, sqrt);
unary!(sin, sinf, sin);
unary!(cos, cosf, cos);

#[inline]
pub fn atan2(y: f32, x: f32) -> f32 {
    #[cfg(not(target_arch = "spirv"))]
    {
        libm::atan2f(y, x)
    }
    #[cfg(target_arch = "spirv")]
    {
        y.atan2(x)
    }
}

/// Hermite smoothstep; `e0 < e1`.
#[inline]
pub fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = clamp01((x - e0) / (e1 - e0));
    t * t * (3.0 - 2.0 * t)
}

/// Real cube root, sign-preserving.
#[inline]
pub fn cbrt(x: f32) -> f32 {
    if x < 0.0 {
        -powf(-x, 1.0 / 3.0)
    } else {
        powf(x, 1.0 / 3.0)
    }
}

/// sRGB transfer function, linear → encoded.
#[inline]
pub fn srgb_oetf(v: f32) -> f32 {
    if v <= 0.003_130_8 {
        12.92 * v
    } else {
        1.055 * powf(v, 1.0 / 2.4) - 0.055
    }
}

/// sRGB transfer function, encoded → linear.
#[inline]
pub fn srgb_eotf(v: f32) -> f32 {
    if v <= 0.040_45 {
        v / 12.92
    } else {
        powf((v + 0.055) / 1.055, 2.4)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn srgb_round_trip() {
        for i in 0..=100 {
            let v = i as f32 / 100.0;
            assert!((srgb_eotf(srgb_oetf(v)) - v).abs() < 1e-6);
        }
    }

    #[test]
    fn clamp01_handles_nan() {
        assert_eq!(clamp01(f32::NAN), 0.0);
        assert_eq!(clamp01(2.0), 1.0);
        assert_eq!(clamp01(-1.0), 0.0);
    }
}
