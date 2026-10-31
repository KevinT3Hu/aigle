//! Auto white balance (doc 06 §3.1): a parameter producer. Deterministic, CPU only, so the
//! result is identical on every backend.
//!
//! Input: camera RGB with the reference WB undone, from the finest pyramid level of at most
//! [`MAX_PIXELS`]. Excluded pixels: sensor-clipped (A2 mask, dilated), within 2 % of black,
//! or saturated after the camera matrix (outside Rec.2020 under the as-shot white).
//!
//! 1. Candidates, each a camera-space illuminant: shades-of-grey (p = 6), first-order
//!    grey-edge (p = 1), white-patch (mean of the brightest 1 % by luminance) and a
//!    near-neutral vote (luminance-weighted mode of per-pixel chromaticities on the Planckian
//!    band, in mired × tint bins).
//! 2. Each is converted to (CCT, tint) under the calibration C1 uses and constrained to
//!    2000–15000 K, |Duv| ≤ 0.02 (|tint| ≤ 60).
//! 3. Fusion in (mired, tint): an iteratively reweighted (Cauchy) mean of the candidates and
//!    the as-shot prior. Maker-note colour temperatures are not exposed by LibRaw, so the
//!    as-shot white is the only prior.
//! 4. Confidence = candidate agreement (mean Cauchy weight) × usable-pixel share × colour
//!    diversity (spread of log-chromaticities; a scene of one colour is indistinguishable
//!    from a coloured illuminant; not applied to a user-picked region, which is meant to be
//!    neutral). Below [`MIN_CONFIDENCE`] the result is As Shot, with the
//!    reason.
//!
//! Output temperature and tint are rounded to the param steps (1 K, 1).

use aigle_core::color;
use aigle_core::geometry::{ImageRect, Point};
use aigle_engine_api::WbEstimate;

use crate::profile::CameraCalibration;
use crate::source::SourceImage;

/// Analysis level budget (≈ 2 MP).
pub const MAX_PIXELS: u64 = 2_200_000;
pub const MIN_CONFIDENCE: f32 = 0.3;
const T_RANGE: (f64, f64) = (2000.0, 15000.0);
/// |Duv| ≤ 0.02 in tint units (tint = Duv · −3000).
const TINT_MAX: f64 = 60.0;
/// Fusion scales: candidates this far apart weigh ½.
const MIRED_SCALE: f64 = 25.0;
const TINT_SCALE: f64 = 12.0;
/// Weight of the as-shot prior relative to one candidate.
const PRIOR_WEIGHT: f64 = 0.5;
/// Pixels sampled for the near-neutral vote.
const VOTE_SAMPLES: usize = 20_000;

/// A source region in level-0 stored-pixel coordinates `[x0, x1) × [y0, y1)`.
pub type SourceRegion = (f64, f64, f64, f64);

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Candidate {
    pub name: &'static str,
    pub temperature: f64,
    pub tint: f64,
}

/// Full result, with the candidates for diagnostics.
#[derive(Clone, Debug, PartialEq)]
pub struct AwbResult {
    pub estimate: WbEstimate,
    pub candidates: Vec<Candidate>,
    pub usable_share: f32,
}

/// The finest pyramid level within [`MAX_PIXELS`].
fn analysis_level(src: &SourceImage) -> usize {
    let lv = &src.pyramid.levels;
    (0..lv.len())
        .find(|&l| lv[l].width as u64 * lv[l].height as u64 <= MAX_PIXELS)
        .unwrap_or(lv.len() - 1)
}

/// Constrain `xy` to the locus band; returns `(T, tint)`.
fn constrain(xy: (f64, f64)) -> (f64, f64) {
    let (t, tint) = color::xy_to_cct_tint(xy.0, xy.1);
    (
        t.clamp(T_RANGE.0, T_RANGE.1),
        tint.clamp(-TINT_MAX, TINT_MAX),
    )
}

fn mired(t: f64) -> f64 {
    1e6 / t
}

/// Estimate the scene white of `src` (optionally only inside `region`) under `cal`.
pub fn estimate(
    src: &SourceImage,
    cal: &CameraCalibration,
    region: Option<SourceRegion>,
) -> AwbResult {
    let l = analysis_level(src);
    let lvl = &src.pyramid.levels[l];
    let (w, h) = (lvl.width as usize, lvl.height as usize);
    let clip = src.clip_mask(l);
    let undo = src.color.reference_wb.map(|m| 1.0 / m as f32);
    let as_shot = src.color.as_shot_xy;
    // Camera → Rec.2020 under the as-shot white, for the saturation test.
    let to_2020 = color::to_f32(&color::mul(
        &color::invert(&color::REC2020_TO_XYZ).expect("invertible"),
        &cal.camera_to_xyz(as_shot, color::D65_XY),
    ));
    let scale = (1u64 << l) as f64;
    let (rx0, ry0, rx1, ry1) = match region {
        Some((x0, y0, x1, y1)) => (
            (x0 / scale).floor().max(0.0) as usize,
            (y0 / scale).floor().max(0.0) as usize,
            ((x1 / scale).ceil() as usize).min(w),
            ((y1 / scale).ceil() as usize).min(h),
        ),
        None => (0, 0, w, h),
    };
    let cam = |x: usize, y: usize| -> [f32; 3] {
        let p = lvl.data[y * w + x];
        [
            p[0].to_f32() * undo[0],
            p[1].to_f32() * undo[1],
            p[2].to_f32() * undo[2],
        ]
    };
    let usable = |x: usize, y: usize, c: [f32; 3]| -> bool {
        if clip.as_ref().is_some_and(|m| m[y * w + x]) {
            return false;
        }
        let mx = c[0].max(c[1]).max(c[2]);
        if mx < 0.02 || c.iter().any(|v| !v.is_finite()) {
            return false;
        }
        let r =
            [0, 1, 2].map(|i| to_2020[i][0] * c[0] + to_2020[i][1] * c[1] + to_2020[i][2] * c[2]);
        let rm = r[0].max(r[1]).max(r[2]);
        r.iter().all(|&v| v >= -0.02 * rm)
    };
    let lum = |c: [f32; 3]| 0.25 * c[0] + 0.5 * c[1] + 0.25 * c[2];

    let mut sog = [0f64; 3];
    let mut edge = [0f64; 3];
    let (mut n, mut n_edge, mut total) = (0usize, 0usize, 0usize);
    let mut lums: Vec<f32> = Vec::new();
    let mut px: Vec<[f32; 3]> = Vec::new();
    for y in ry0..ry1 {
        for x in rx0..rx1 {
            total += 1;
            let c = cam(x, y);
            if !usable(x, y, c) {
                continue;
            }
            n += 1;
            for k in 0..3 {
                sog[k] += (c[k].max(0.0) as f64).powi(6);
            }
            lums.push(lum(c));
            px.push(c);
            if x + 1 < rx1 && y + 1 < ry1 {
                let (cx, cy) = (cam(x + 1, y), cam(x, y + 1));
                if usable(x + 1, y, cx) && usable(x, y + 1, cy) {
                    n_edge += 1;
                    for k in 0..3 {
                        let g = ((cx[k] - c[k]).powi(2) + (cy[k] - c[k]).powi(2)).sqrt() as f64;
                        edge[k] += g;
                    }
                }
            }
        }
    }
    let share = if total > 0 {
        n as f32 / total as f32
    } else {
        0.0
    };
    let (prior_t, prior_tint) = color::xy_to_cct_tint(as_shot.0, as_shot.1);
    let as_shot_result = |conf: f32, reason: &str| AwbResult {
        estimate: WbEstimate {
            temperature: prior_t.round() as f32,
            tint: prior_tint.round() as f32,
            confidence: conf,
            low_confidence_reason: Some(reason.into()),
        },
        candidates: Vec::new(),
        usable_share: share,
    };
    if n < 256 {
        return as_shot_result(0.0, "too few usable pixels");
    }

    let mut cands: Vec<Candidate> = Vec::new();
    let mut push = |name: &'static str, e: [f64; 3]| {
        if e.iter().all(|v| v.is_finite() && *v > 0.0) {
            let (t, tint) = constrain(cal.neutral_to_xy([e[0] / e[1], 1.0, e[2] / e[1]]));
            cands.push(Candidate {
                name,
                temperature: t,
                tint,
            });
        }
    };
    push(
        "shades_of_grey",
        sog.map(|v| (v / n as f64).powf(1.0 / 6.0)),
    );
    if n_edge > 256 {
        push(
            "grey_edge",
            edge.map(|v| (v / n_edge as f64).powf(1.0 / 6.0)),
        );
    }
    // White patch: brightest 1 %.
    let mut sorted = lums.clone();
    let k = sorted.len() - 1 - sorted.len() / 100;
    let (_, &mut thr, _) = sorted.select_nth_unstable_by(k, f32::total_cmp);
    let mut wp = [0f64; 3];
    for (c, &y) in px.iter().zip(&lums) {
        if y >= thr {
            for i in 0..3 {
                wp[i] += c[i] as f64;
            }
        }
    }
    push("white_patch", wp);
    // Near-neutral vote: per-pixel chromaticity as if the pixel were neutral, inside the
    // band, luminance-weighted histogram in 10 mired × 4 tint bins; the mode bin's mean.
    let stride = (px.len() / VOTE_SAMPLES).max(1);
    let mut bins: std::collections::BTreeMap<(i32, i32), (f64, f64, f64)> = Default::default();
    for (c, &y) in px.iter().zip(&lums).step_by(stride) {
        if c.iter().any(|&v| v <= 1e-4) {
            continue;
        }
        let xy = cal.neutral_to_xy([(c[0] / c[1]) as f64, 1.0, (c[2] / c[1]) as f64]);
        let (t, tint) = color::xy_to_cct_tint(xy.0, xy.1);
        if !(T_RANGE.0..=T_RANGE.1).contains(&t) || tint.abs() > TINT_MAX {
            continue;
        }
        let m = mired(t);
        let b = bins
            .entry(((m / 10.0).floor() as i32, (tint / 4.0).floor() as i32))
            .or_default();
        b.0 += y as f64;
        b.1 += y as f64 * m;
        b.2 += y as f64 * tint;
    }
    if let Some((_, b)) = bins
        .iter()
        .max_by(|a, b| a.1.0.total_cmp(&b.1.0).then(b.0.cmp(a.0)))
        && b.0 > 0.0
    {
        cands.push(Candidate {
            name: "neutral_vote",
            temperature: 1e6 / (b.1 / b.0),
            tint: b.2 / b.0,
        });
    }
    if cands.is_empty() {
        return as_shot_result(0.0, "no usable estimator");
    }

    // Fusion (IRLS, Cauchy weights) with the as-shot prior.
    let pt = (
        mired(prior_t.clamp(T_RANGE.0, T_RANGE.1)),
        prior_tint.clamp(-TINT_MAX, TINT_MAX),
    );
    let pts: Vec<(f64, f64, f64)> = cands
        .iter()
        .map(|c| (mired(c.temperature), c.tint, 1.0))
        .chain(std::iter::once((pt.0, pt.1, PRIOR_WEIGHT)))
        .collect();
    let d2 = |p: &(f64, f64, f64), m: (f64, f64)| {
        ((p.0 - m.0) / MIRED_SCALE).powi(2) + ((p.1 - m.1) / TINT_SCALE).powi(2)
    };
    let mut est = {
        let s: f64 = pts.iter().map(|p| p.2).sum();
        (
            pts.iter().map(|p| p.0 * p.2).sum::<f64>() / s,
            pts.iter().map(|p| p.1 * p.2).sum::<f64>() / s,
        )
    };
    for _ in 0..20 {
        let (mut s, mut a, mut b) = (0.0, 0.0, 0.0);
        for p in &pts {
            let wt = p.2 / (1.0 + d2(p, est));
            s += wt;
            a += wt * p.0;
            b += wt * p.1;
        }
        est = (a / s, b / s);
    }
    // Agreement of the candidates (not the prior) around the estimate.
    let agree = (cands
        .iter()
        .map(|c| 1.0 / (1.0 + d2(&(mired(c.temperature), c.tint, 1.0), est)))
        .sum::<f64>()
        / cands.len() as f64) as f32;
    let enough = (share / 0.2).min(1.0);
    // Colour diversity: RMS spread of (ln r/g, ln b/g) over the usable pixels.
    let lc: Vec<(f64, f64)> = px
        .iter()
        .step_by(stride)
        .filter(|c| c.iter().all(|&v| v > 1e-4))
        .map(|c| (((c[0] / c[1]) as f64).ln(), ((c[2] / c[1]) as f64).ln()))
        .collect();
    // A user-picked region is meant to be neutral: no diversity requirement there.
    let diverse = if region.is_some() {
        1.0
    } else if lc.len() < 2 {
        0.0
    } else {
        let m = lc.iter().fold((0.0, 0.0), |a, p| (a.0 + p.0, a.1 + p.1));
        let m = (m.0 / lc.len() as f64, m.1 / lc.len() as f64);
        let v = lc
            .iter()
            .map(|p| (p.0 - m.0).powi(2) + (p.1 - m.1).powi(2))
            .sum::<f64>()
            / lc.len() as f64;
        (v.sqrt() / 0.3).min(1.0) as f32
    };
    let conf = agree * enough * diverse;
    let t = (1e6 / est.0).clamp(T_RANGE.0, T_RANGE.1);
    if conf < MIN_CONFIDENCE {
        let worst = agree.min(enough).min(diverse);
        let reason = if worst == diverse {
            "dominant single colour"
        } else if worst == enough {
            "too few usable pixels"
        } else {
            "estimators disagree"
        };
        let mut r = as_shot_result(conf, reason);
        r.candidates = cands;
        return r;
    }
    AwbResult {
        estimate: WbEstimate {
            temperature: t.round() as f32,
            tint: est.1.round() as f32,
            confidence: conf,
            low_confidence_reason: None,
        },
        candidates: cands,
        usable_share: share,
    }
}

/// Pixels the picker averages at most; it uses the finest level where the region fits.
const PICK_PIXELS: f64 = 4096.0;

/// White balance picker: the white that makes the mean camera colour of `region` neutral,
/// without the AWB band constraint, estimators or prior. `None` when the region has no
/// usable pixels (sensor-clipped or near black). Clamped to the `wb.*` param ranges; then
/// `low_confidence_reason` says so.
pub fn neutral(
    src: &SourceImage,
    cal: &CameraCalibration,
    region: SourceRegion,
) -> Option<WbEstimate> {
    let (x0, y0, x1, y1) = region;
    let levels = &src.pyramid.levels;
    let area = (x1 - x0).max(1.0) * (y1 - y0).max(1.0);
    let l = (0..levels.len())
        .find(|&l| area / (1u64 << (2 * l)) as f64 <= PICK_PIXELS)
        .unwrap_or(levels.len() - 1);
    let lvl = &levels[l];
    let (w, h) = (lvl.width as usize, lvl.height as usize);
    let scale = (1u64 << l) as f64;
    let cx = ((x0 + x1) / 2.0 / scale).floor().clamp(0.0, (w - 1) as f64) as usize;
    let cy = ((y0 + y1) / 2.0 / scale).floor().clamp(0.0, (h - 1) as f64) as usize;
    let rx0 = ((x0 / scale).floor().max(0.0) as usize).min(cx);
    let ry0 = ((y0 / scale).floor().max(0.0) as usize).min(cy);
    let rx1 = ((x1 / scale).ceil() as usize).clamp(cx + 1, w);
    let ry1 = ((y1 / scale).ceil() as usize).clamp(cy + 1, h);
    let clip = src.clip_mask(l);
    let undo = src.color.reference_wb.map(|m| 1.0 / m);

    let mut sum = [0f64; 3];
    let mut n = 0usize;
    for y in ry0..ry1 {
        for x in rx0..rx1 {
            if clip.as_ref().is_some_and(|m| m[y * w + x]) {
                continue;
            }
            let p = lvl.data[y * w + x];
            let c = [0, 1, 2].map(|k| p[k].to_f32() as f64 * undo[k]);
            if c.iter().any(|v| !v.is_finite()) || c[0].max(c[1]).max(c[2]) < 0.005 {
                continue;
            }
            for k in 0..3 {
                sum[k] += c[k];
            }
            n += 1;
        }
    }
    if n == 0 || sum.iter().any(|&v| v <= 0.0) {
        return None;
    }
    let xy = cal.neutral_to_xy([sum[0] / sum[1], 1.0, sum[2] / sum[1]]);
    let (t, tint) = color::xy_to_cct_tint(xy.0, xy.1);
    if !(t.is_finite() && tint.is_finite()) {
        return None;
    }
    let params = aigle_core::EditParams::default();
    let spec = |p: &str| params.spec(p).expect("white balance params exist");
    let (ts, is) = (spec("wb.temperature"), spec("wb.tint"));
    let clamped = (
        t.clamp(ts.min as f64, ts.max as f64),
        tint.clamp(is.min as f64, is.max as f64),
    );
    let in_range = clamped == (t, tint);
    Some(WbEstimate {
        temperature: ts.normalize(clamped.0 as f32),
        tint: is.normalize(clamped.1 as f32),
        confidence: if in_range { 1.0 } else { 0.0 },
        low_confidence_reason: (!in_range).then(|| "outside the white balance range".into()),
    })
}

/// Bounding box in level-0 stored pixels of a crop-space rectangle, through `to_stored`.
pub fn region_to_source(r: ImageRect, to_stored: impl Fn(Point) -> Point) -> Option<SourceRegion> {
    let mut b = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for j in 0..=4 {
        for i in 0..=4 {
            let p = to_stored(Point::new(
                r.x + r.w * i as f64 / 4.0,
                r.y + r.h * j as f64 / 4.0,
            ));
            b = (b.0.min(p.x), b.1.min(p.y), b.2.max(p.x), b.3.max(p.y));
        }
    }
    (b.2 > b.0 && b.3 > b.1 && b.0.is_finite()).then_some(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::source::{SourceColor, test_source};
    use std::sync::Arc;

    /// A varied scene of reflectances lit by an illuminant of camera neutral `n`, stored with
    /// reference WB `ref_wb`. The camera is Rec.2020 (working-space calibration).
    fn scene(n: [f64; 3], ref_wb: [f64; 3], single_colour: bool) -> SourceImage {
        let (w, h) = (320u32, 240u32);
        let mut rgb = Vec::new();
        for y in 0..h {
            for x in 0..w {
                let (i, j) = (x / 40, y / 40);
                let refl = if single_colour {
                    [0.6, 0.15, 0.08]
                } else {
                    let k = (i * 7 + j * 3) % 12;
                    [
                        [0.6, 0.6, 0.6],
                        [0.2, 0.2, 0.2],
                        [0.5, 0.3, 0.2],
                        [0.2, 0.4, 0.2],
                        [0.15, 0.2, 0.45],
                        [0.7, 0.65, 0.4],
                        [0.4, 0.25, 0.4],
                        [0.3, 0.45, 0.5],
                        [0.85, 0.85, 0.85],
                        [0.1, 0.1, 0.1],
                        [0.45, 0.35, 0.3],
                        [0.35, 0.35, 0.3],
                    ][k as usize]
                };
                // Mild in-patch shading so grey-edge sees edges everywhere.
                let s = 0.8 + 0.2 * ((x % 40) as f64 / 40.0);
                rgb.push([0, 1, 2].map(|c| (refl[c] * n[c] * s * ref_wb[c] * 0.9) as f32));
            }
        }
        let ws = SourceColor::working_space();
        let color = SourceColor::new(ref_wb, (*ws.calibration).clone(), 0.0);
        test_source(w, h, &rgb, color)
    }

    fn neutral_of(t: f64, tint: f64, cal: &CameraCalibration) -> [f64; 3] {
        cal.neutral(color::cct_tint_to_xy(t, tint))
    }

    #[test]
    fn recovers_the_illuminant_and_ignores_the_as_shot_offset() {
        let cal = Arc::clone(&SourceColor::working_space().calibration);
        for (t, tint) in [(3200.0, 0.0), (5000.0, 10.0), (7500.0, -8.0)] {
            let n = neutral_of(t, tint, &cal);
            // As-shot deliberately off (D65 neutral).
            let ref_wb = neutral_of(6500.0, 0.0, &cal).map(|v| 1.0 / v);
            let src = scene(n, ref_wb, false);
            let r = estimate(&src, &cal, None);
            let e = &r.estimate;
            assert!(e.low_confidence_reason.is_none(), "{t}: {r:?}");
            let dm = (1e6 / e.temperature as f64 - 1e6 / t).abs();
            assert!(dm < 12.0, "{t}/{tint}: got {e:?} ({:?})", r.candidates);
            assert!((e.tint as f64 - tint).abs() < 8.0, "{t}/{tint}: got {e:?}");
        }
    }

    #[test]
    fn single_colour_scene_falls_back_to_as_shot() {
        let cal = Arc::clone(&SourceColor::working_space().calibration);
        let n = neutral_of(5000.0, 0.0, &cal);
        let src = scene(n, n.map(|v| 1.0 / v), true);
        let r = estimate(&src, &cal, None);
        let e = &r.estimate;
        assert!(e.low_confidence_reason.is_some(), "{r:?}");
        let (t, tint) = color::xy_to_cct_tint(src.color.as_shot_xy.0, src.color.as_shot_xy.1);
        assert_eq!(
            (e.temperature, e.tint),
            (t.round() as f32, tint.round() as f32)
        );
    }

    #[test]
    fn region_on_a_grey_patch_gives_its_white() {
        let cal = Arc::clone(&SourceColor::working_space().calibration);
        let n = neutral_of(4000.0, 0.0, &cal);
        let ref_wb = neutral_of(6500.0, 0.0, &cal).map(|v| 1.0 / v);
        let src = scene(n, ref_wb, false);
        // Patch (0, 0) is grey.
        let r = estimate(&src, &cal, Some((2.0, 2.0, 38.0, 38.0)));
        let e = &r.estimate;
        assert!(e.low_confidence_reason.is_none(), "{r:?}");
        assert!(
            (e.temperature - 4000.0).abs() < 100.0 && e.tint.abs() <= 2.0,
            "{r:?}"
        );
    }

    #[test]
    fn picker_neutralises_the_patch_and_rejects_dark_or_clipped() {
        let cal = Arc::clone(&SourceColor::working_space().calibration);
        let ref_wb = neutral_of(6500.0, 0.0, &cal).map(|v| 1.0 / v);
        for (t, tint) in [(3000.0, 0.0), (5600.0, 12.0), (9000.0, -20.0)] {
            let src = scene(neutral_of(t, tint, &cal), ref_wb, false);
            // Patch (0, 0) is grey; a 1 px region still works.
            for r in [(5.0, 5.0, 35.0, 35.0), (20.0, 20.0, 20.5, 20.5)] {
                let e = neutral(&src, &cal, r).unwrap();
                assert!(e.low_confidence_reason.is_none(), "{e:?}");
                let dm = (1e6 / e.temperature as f64 - 1e6 / t).abs();
                assert!(
                    dm < 1.0 && (e.tint as f64 - tint).abs() <= 1.0,
                    "{t}: {e:?}"
                );
            }
        }
        // A coloured patch is taken at face value (no locus band), and clamped at the range.
        let src = scene(neutral_of(5000.0, 0.0, &cal), ref_wb, false);
        let e = neutral(&src, &cal, (45.0, 5.0, 75.0, 35.0)).unwrap();
        assert_ne!(e.temperature.round(), 5000.0);
        let mut rgb = vec![[0.0f32, 0.0, 0.0]; 64 * 64];
        rgb[0] = [0.9, 0.001, 0.001];
        let dark = test_source(64, 64, &rgb, SourceColor::working_space());
        assert!(neutral(&dark, &cal, (10.0, 10.0, 20.0, 20.0)).is_none());
        let e = neutral(&dark, &cal, (0.0, 0.0, 1.0, 1.0)).unwrap();
        assert!(e.low_confidence_reason.is_some(), "{e:?}");
    }

    #[test]
    fn constrain_clamps_to_the_band() {
        let xy = color::cct_tint_to_xy(1500.0, 200.0);
        let (t, tint) = constrain(xy);
        assert_eq!(t, 2000.0);
        assert_eq!(tint, TINT_MAX);
    }
}
