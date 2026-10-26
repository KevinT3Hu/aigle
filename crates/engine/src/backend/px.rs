//! Pixel storage of the two precisions (doc 04 §5): f16 for previews and 8-bit exports, f32
//! for 16-bit exports (doc 03 §9). Products of the stages hold one or the other.

use aigle_kernels::Rgb;
use half::f16;

/// RGB pixels, borrowed.
#[derive(Clone, Copy, Debug)]
pub enum Px<'a> {
    H(&'a [[f16; 3]]),
    F(&'a [[f32; 3]]),
}

impl<'a> Px<'a> {
    pub fn len(&self) -> usize {
        match self {
            Px::H(v) => v.len(),
            Px::F(v) => v.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn is_f32(&self) -> bool {
        matches!(self, Px::F(_))
    }

    #[inline]
    pub fn get(&self, i: usize) -> Rgb {
        match self {
            Px::H(v) => {
                let p = v[i];
                [p[0].to_f32(), p[1].to_f32(), p[2].to_f32()]
            }
            Px::F(v) => v[i],
        }
    }

    pub fn slice(&self, a: usize, b: usize) -> Px<'a> {
        match *self {
            Px::H(v) => Px::H(&v[a..b]),
            Px::F(v) => Px::F(&v[a..b]),
        }
    }

    /// Words per pixel on the GPU: RGBA f16 in 2, or f32 RGB in 3.
    pub fn gpu_words(&self) -> usize {
        if self.is_f32() { 3 } else { 2 }
    }

    /// Append the GPU representation.
    pub fn pack(&self, out: &mut Vec<u32>) {
        match self {
            Px::H(v) => {
                for p in *v {
                    out.push(p[0].to_bits() as u32 | (p[1].to_bits() as u32) << 16);
                    out.push(p[2].to_bits() as u32);
                }
            }
            Px::F(v) => {
                for p in *v {
                    out.extend(p.map(f32::to_bits));
                }
            }
        }
    }
}

/// RGB pixels, owned.
#[derive(Clone, Debug, PartialEq)]
pub enum PxVec {
    H(Vec<[f16; 3]>),
    F(Vec<[f32; 3]>),
}

impl PxVec {
    pub fn zeros(n: usize, f32: bool) -> PxVec {
        if f32 {
            PxVec::F(vec![[0.0; 3]; n])
        } else {
            PxVec::H(vec![[f16::ZERO; 3]; n])
        }
    }

    /// From f32 values, rounded to f16 unless `f32`.
    pub fn from_rgb(v: Vec<Rgb>, f32: bool) -> PxVec {
        if f32 {
            PxVec::F(v)
        } else {
            PxVec::H(v.into_iter().map(|p| p.map(f16::from_f32)).collect())
        }
    }

    pub fn as_px(&self) -> Px<'_> {
        match self {
            PxVec::H(v) => Px::H(v),
            PxVec::F(v) => Px::F(v),
        }
    }

    pub fn len(&self) -> usize {
        self.as_px().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn bytes(&self) -> u64 {
        match self {
            PxVec::H(v) => v.len() as u64 * 6,
            PxVec::F(v) => v.len() as u64 * 12,
        }
    }

    /// Copy `src[s..s + n]` to `self[d..d + n]` (same precision).
    pub fn copy_from(&mut self, d: usize, src: &PxVec, s: usize, n: usize) {
        match (self, src) {
            (PxVec::H(a), PxVec::H(b)) => a[d..d + n].copy_from_slice(&b[s..s + n]),
            (PxVec::F(a), PxVec::F(b)) => a[d..d + n].copy_from_slice(&b[s..s + n]),
            _ => panic!("pixel precision mismatch"),
        }
    }
}

/// D-tier local fields per pixel (contrast, whites, blacks, saturation, hue).
#[derive(Clone, Debug, PartialEq)]
pub enum Fields {
    H(Vec<[f16; 5]>),
    F(Vec<[f32; 5]>),
}

impl Fields {
    pub fn none(f32: bool) -> Fields {
        if f32 {
            Fields::F(Vec::new())
        } else {
            Fields::H(Vec::new())
        }
    }

    pub fn from_f32(v: Vec<[f32; 5]>, f32: bool) -> Fields {
        if f32 {
            Fields::F(v)
        } else {
            Fields::H(v.into_iter().map(|p| p.map(f16::from_f32)).collect())
        }
    }

    pub fn len(&self) -> usize {
        match self {
            Fields::H(v) => v.len(),
            Fields::F(v) => v.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    #[inline]
    pub fn get(&self, i: usize) -> [f32; 5] {
        match self {
            Fields::H(v) => v[i].map(|h| h.to_f32()),
            Fields::F(v) => v[i],
        }
    }

    pub fn bytes(&self) -> u64 {
        match self {
            Fields::H(v) => v.len() as u64 * 10,
            Fields::F(v) => v.len() as u64 * 20,
        }
    }
}
