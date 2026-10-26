//! Noise model of a source (doc 02 §8, doc 06 §2).
//!
//! The raw noise variance of the normalised signal `x` (1.0 = white level) is `S·x + O`.
//! `(S, O)` come from, in order:
//! 1. the file's DNG NoiseProfile (per CFA plane, [`NoiseSource::Vendor`]);
//! 2. else a measurement from the image ([`NoiseSource::Measured`]).
//!
//! TODO(doc 02 §8): per-camera profiles for raws without a NoiseProfile. darktable's
//! `noiseprofiles.json` was evaluated and not bundled: on the green CFA sites of a D3200
//! (ISO 800), an EOS 6D (ISO 100) and a K-5 (ISO 160) the measured variance slope is a
//! consistent 8.5–9.1× its green `a`, and the cause (its profiling pipeline) is not
//! established.
//!
//! The model is
//! carried into the stored data `s_c = x_c · wb_c` (reference WB, green = 1) and into the S
//! tier's luma `Y = (R + 2G + B)/4` assuming independent channels and a near-neutral pixel
//! (`s_c ≈ Y`): `var(Y) ≈ a·Y + b`. The matching variance-stabilising transform is
//! `z = √(Y + b/a)`, with a constant `σ_z = √a / 2`. A 2×2 box level divides the variance by
//! four, so `σ_z` halves per pyramid level and the offset stays.
//!
//! Without either profile (rendered images, unlisted cameras) the level is measured from
//! the image ([`measure`], doc 06 §2 "measured from flat regions"), with a pure-Poisson VST
//! (offset [`E`]).

use aigle_engine_api::NoiseEstimate;
use rayon::prelude::*;

use crate::source::Level;

/// VST offset of the measured fallback.
pub const E: f32 = 1e-5;
/// Mid-grey in stored units, where [`NoiseEstimate`] is reported.
pub const MID_GREY: f64 = 0.18;
const LUMA_W: [f64; 3] = [0.25, 0.5, 0.25];

/// Noise of one pyramid level for the S tier: `σ` of `z = √(Y + offset)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NoiseLevel {
    pub sigma: f32,
    pub offset: f32,
}

/// Where a source's noise numbers come from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoiseSource {
    /// DNG NoiseProfile.
    Vendor,
    /// Measured from the image (no vendor data).
    Measured,
}

/// Level-0 noise model in stored units: `var(Y) = luma[0]·Y + luma[1]`, and the mean of the
/// chroma (`R − G`, `B − G`) variances `chroma[0]·Y + chroma[1]`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NoiseModel {
    pub luma: [f64; 2],
    pub chroma: [f64; 2],
    pub source: NoiseSource,
}

impl NoiseModel {
    /// From NoiseProfile pairs (one for all planes, or one per R, G, B plane) and the
    /// reference WB multipliers premultiplied into the stored data.
    pub fn from_profile(
        planes: &[(f64, f64)],
        wb: [f64; 3],
        source: NoiseSource,
    ) -> Option<NoiseModel> {
        let p: [(f64, f64); 3] = match planes {
            [a] => [*a; 3],
            [r, g, b] => [*r, *g, *b],
            _ => return None,
        };
        // var(s_c) = wb_c²·(S_c·x_c + O_c) = wb_c·S_c·s_c + wb_c²·O_c, with s_c ≈ Y.
        let ch = |c: usize| [wb[c] * p[c].0, wb[c] * wb[c] * p[c].1];
        let v = [ch(0), ch(1), ch(2)];
        let lin = |w: [f64; 3], k: usize| (0..3).map(|c| w[c] * w[c] * v[c][k]).sum::<f64>();
        let luma = [lin(LUMA_W, 0), lin(LUMA_W, 1)];
        let chroma = [0, 1].map(|k| v[1][k] + 0.5 * (v[0][k] + v[2][k]));
        // O may be slightly negative in fitted profiles; the VST offset and the
        // reported σ are clamped.
        (luma[0] > 0.0 && luma.iter().chain(&chroma).all(|x| x.is_finite())).then_some(NoiseModel {
            luma,
            chroma,
            source,
        })
    }

    /// S-tier noise of pyramid level `l`.
    pub fn level(&self, l: usize) -> NoiseLevel {
        let a = self.luma[0] / 4f64.powi(l as i32);
        NoiseLevel {
            sigma: (a.sqrt() / 2.0) as f32,
            offset: ((self.luma[1] / self.luma[0]) as f32).max(E),
        }
    }

    /// Level-0 standard deviations at mid-grey.
    pub fn estimate(&self) -> NoiseEstimate {
        let sd = |m: [f64; 2]| (m[0] * MID_GREY + m[1]).max(0.0).sqrt() as f32;
        NoiseEstimate {
            luma_sigma: sd(self.luma),
            chroma_sigma: sd(self.chroma),
        }
    }
}

/// The noise model of a raw: its DNG NoiseProfile (`wb` = the reference WB premultiplied
/// into the stored data).
pub fn raw_model(dng_profile: &[(f64, f64)], wb: [f64; 3]) -> Option<NoiseModel> {
    NoiseModel::from_profile(dng_profile, wb, NoiseSource::Vendor)
}

/// Robust σ of a stabilised signal: 1.4826 · median |v − mean of 4 neighbours| / 1.118 (the
/// Laplacian stencil's gain on white noise), over a subsampled interior grid.
fn mad(level: &Level, f: impl Fn([f32; 3]) -> f32 + Sync) -> f32 {
    let (w, h) = (level.width as usize, level.height as usize);
    if w < 3 || h < 3 {
        return 0.0;
    }
    let v = |x: usize, y: usize| {
        let p = level.data[y * w + x];
        f([p[0].to_f32(), p[1].to_f32(), p[2].to_f32()])
    };
    let n = (w - 2) * (h - 2);
    let stride = ((n as f64 / 2.0e6).sqrt().ceil() as usize).max(1);
    let mut d: Vec<f32> = (1..h - 1)
        .step_by(stride)
        .collect::<Vec<_>>()
        .par_iter()
        .flat_map_iter(|&y| {
            let v = &v;
            (1..w - 1).step_by(stride).map(move |x| {
                (v(x, y) - 0.25 * (v(x - 1, y) + v(x + 1, y) + v(x, y - 1) + v(x, y + 1))).abs()
            })
        })
        .collect();
    if d.is_empty() {
        return 0.0;
    }
    let mid = d.len() / 2;
    let (_, m, _) = d.select_nth_unstable_by(mid, f32::total_cmp);
    1.4826 * *m / 1.118
}

fn luma(p: [f32; 3]) -> f32 {
    (p[0] + 2.0 * p[1] + p[2]) * 0.25
}

/// Measured noise of a level (no vendor model): `σ` of `z = √(Y + E)`.
pub fn measure(level: &Level) -> NoiseLevel {
    NoiseLevel {
        sigma: mad(level, |p| (luma(p).max(0.0) + E).sqrt()),
        offset: E,
    }
}

/// Measured level-0 standard deviations at mid-grey (Poisson-scaled from the stabilised
/// luma and chroma).
pub fn measure_estimate(level: &Level) -> NoiseEstimate {
    let k = 2.0 * (MID_GREY as f32 + E).sqrt();
    let stab = |p: [f32; 3], c: usize| (p[c] - p[1]) / (2.0 * (luma(p).max(0.0) + E).sqrt());
    let cr = mad(level, |p| stab(p, 0));
    let cb = mad(level, |p| stab(p, 2));
    NoiseEstimate {
        luma_sigma: k * measure(level).sigma,
        chroma_sigma: k * 0.5 * (cr + cb),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Flat patches with Poisson-Gaussian noise of the given NoiseProfile, independent per
    /// channel, in stored units (`wb` premultiplied).
    fn level(profile: (f64, f64), wb: [f64; 3]) -> Level {
        let (w, h) = (256u32, 256u32);
        let mut seed = 0x1234_5678_9abc_def0u64;
        let mut gauss = move || {
            // Box–Muller on xorshift.
            let mut u = || {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                ((seed >> 11) as f64 + 0.5) / (1u64 << 53) as f64
            };
            let (a, b) = (u(), u());
            (-2.0 * a.ln()).sqrt() * (std::f64::consts::TAU * b).cos()
        };
        let rgb: Vec<[f32; 3]> = (0..w * h)
            .map(|i| {
                let (x, y) = (i % w, i / w);
                // Neutral patches (s_c equal) at a few brightnesses.
                let y_s = [0.05, 0.18, 0.4, 0.7][((x / 64 + y / 64) % 4) as usize];
                [0, 1, 2].map(|c| {
                    let xr = y_s / wb[c];
                    let sd = (profile.0 * xr + profile.1).sqrt();
                    ((xr + sd * gauss()) * wb[c]) as f32
                })
            })
            .collect();
        Level::from_f32(w, h, &rgb)
    }

    #[test]
    fn vendor_model_matches_the_measured_noise() {
        let (prof, wb) = ((4e-5, 2e-7), [2.0, 1.0, 1.6]);
        let lvl = level(prof, wb);
        let m = NoiseModel::from_profile(&[prof], wb, NoiseSource::Vendor).unwrap();
        // σ in the matched VST, measured on the synthetic data.
        let nl = m.level(0);
        let meas = mad(&lvl, |p| (luma(p).max(0.0) + nl.offset).sqrt());
        assert!(
            (meas / nl.sigma - 1.0).abs() < 0.15,
            "{meas} vs {}",
            nl.sigma
        );
        // Mid-grey estimate agrees with the measured fallback.
        let (e, f) = (m.estimate(), measure_estimate(&lvl));
        assert!(
            (f.luma_sigma / e.luma_sigma - 1.0).abs() < 0.2,
            "{e:?} {f:?}"
        );
        assert!(
            (f.chroma_sigma / e.chroma_sigma - 1.0).abs() < 0.25,
            "{e:?} {f:?}"
        );
        // One level down: half the σ.
        assert!((m.level(1).sigma * 2.0 - nl.sigma).abs() < 1e-7);
    }

    #[test]
    fn rejects_bad_profiles() {
        let v = NoiseSource::Vendor;
        assert!(NoiseModel::from_profile(&[], [1.0; 3], v).is_none());
        assert!(NoiseModel::from_profile(&[(1e-5, 0.0); 2], [1.0; 3], v).is_none());
        assert!(NoiseModel::from_profile(&[(0.0, 0.0)], [1.0; 3], v).is_none());
    }

    #[test]
    fn negative_offset_still_gives_a_usable_model() {
        let m = NoiseModel::from_profile(&[(1e-6, -1e-8)], [2.0, 1.0, 1.5], NoiseSource::Vendor)
            .unwrap();
        assert!(m.level(0).offset > 0.0 && m.estimate().luma_sigma > 0.0);
    }
}
