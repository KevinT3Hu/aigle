//! Auto-tone: pick exposure, contrast, highlights, shadows, whites and blacks from a
//! histogram so a flat or dull capture lands in a pleasing range. It reads the image with
//! its tone sliders zeroed (see [`neutral`]), measures luma percentiles and clipping, and
//! aims the midtone at a target while protecting the ends.
//!
//! This is intentionally conservative and mirrors the look of Lightroom's Auto: it never
//! clips more than the source already does, and it leaves a neutral image neutral.

use aigle_core::EditParams;
use aigle_doc::ParamOp;
use aigle_engine_api::{ChannelStats, ClipStats, HistogramStats};

/// Target median luma (sRGB-encoded) for the adjusted image.
const TARGET_MID: f32 = 0.45;
/// Where the bright/dark anchors should sit.
const TARGET_HIGH: f32 = 0.92;
const TARGET_LOW: f32 = 0.05;
/// Already-clipped fraction (per mille of the histogram) we treat as "touching" an end.
const CLIP_TOLERANCE: f32 = 0.2;

/// The params used to measure the base image: tone sliders and tone curve cleared, the rest
/// (white balance, crop, lens, …) kept so the measurement matches what the user sees.
pub fn neutral(params: &EditParams) -> EditParams {
    let mut p = params.clone();
    p.light = Default::default();
    p.curve = Default::default();
    p
}

fn percentile(c: &ChannelStats, p: f32) -> f32 {
    c.percentiles
        .iter()
        .find(|(q, _)| (*q - p).abs() < 1e-3)
        .map(|(_, v)| *v)
        .unwrap_or(0.5)
}

/// Auto-tone slider values (`light.*`) for an image whose neutral histogram is `hist`, with
/// `clip` its clipping. Values are rounded to the slider step. `None` when the image is
/// already well placed and no change helps.
pub fn adjustments(hist: &HistogramStats, clip: &ClipStats) -> Vec<(&'static str, f64)> {
    let luma = &hist.luma;
    let (p1, mid, p99) = (
        percentile(luma, 1.0),
        percentile(luma, 50.0),
        percentile(luma, 99.0),
    );

    // Exposure: shift the median toward the target in stops, from the ratio of linear
    // luminances, then damp so a big gap does not blow out.
    let lin = |v: f32| ((v + 0.055) / 1.055).max(1e-4).powf(2.4);
    let stops = (lin(TARGET_MID) / lin(mid.clamp(0.02, 0.98))).log2();
    let exposure = (stops * 0.6).clamp(-2.0, 2.0);

    // Whites/blacks stretch the ends toward the anchors without adding clipping.
    let head_room = CLIP_TOLERANCE - clip.highlights_pct.iter().copied().fold(0.0, f32::max);
    let foot_room = CLIP_TOLERANCE - clip.shadows_pct.iter().copied().fold(0.0, f32::max);
    let whites = if head_room > 0.0 {
        ((TARGET_HIGH - p99) * 180.0).clamp(0.0, 60.0)
    } else {
        0.0
    };
    let blacks = if foot_room > 0.0 {
        ((TARGET_LOW - p1) * 180.0).clamp(-60.0, 0.0)
    } else {
        0.0
    };

    // Highlights/shadows recover whatever is already piled up at the ends.
    let highlights = -(clip.highlights_pct.iter().copied().fold(0.0, f32::max) * 6.0).min(80.0);
    let shadows = (clip.shadows_pct.iter().copied().fold(0.0, f32::max) * 6.0).min(80.0);

    // Contrast only when the tones are compressed around the midtone.
    let spread = p99 - p1;
    let contrast = if spread < 0.55 {
        ((0.6 - spread) * 120.0).clamp(0.0, 35.0)
    } else {
        0.0
    };

    let step = |v: f32| (v as f64).round();
    [
        ("light.exposure", (exposure as f64 * 100.0).round() / 100.0),
        ("light.contrast", step(contrast)),
        ("light.highlights", step(highlights)),
        ("light.shadows", step(shadows)),
        ("light.whites", step(whites)),
        ("light.blacks", step(blacks)),
    ]
    .into_iter()
    .filter(|(_, v)| *v != 0.0)
    .collect()
}

/// [`adjustments`] as ops that set each slider, resetting the ones left at their default so
/// a second auto-tone replaces the first.
pub fn ops(hist: &HistogramStats, clip: &ClipStats) -> Vec<ParamOp> {
    let chosen = adjustments(hist, clip);
    [
        "exposure",
        "contrast",
        "highlights",
        "shadows",
        "whites",
        "blacks",
    ]
    .into_iter()
    .map(|name| {
        let path = format!("light.{name}");
        match chosen.iter().find(|(p, _)| p.ends_with(name)) {
            Some((_, v)) => ParamOp::set(&path, *v),
            None => ParamOp::Reset {
                path: path.as_str().into(),
            },
        }
    })
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn channel(percentiles: &[(f32, f32)]) -> ChannelStats {
        ChannelStats {
            mean: 0.5,
            percentiles: percentiles.to_vec(),
            counts: Vec::new(),
        }
    }

    fn stats(p1: f32, mid: f32, p99: f32) -> HistogramStats {
        let c = channel(&[(1.0, p1), (50.0, mid), (99.0, p99)]);
        HistogramStats {
            red: c.clone(),
            green: c.clone(),
            blue: c.clone(),
            luma: c,
        }
    }

    fn clip(h: f32, s: f32) -> ClipStats {
        ClipStats {
            highlights_pct: [h; 3],
            shadows_pct: [s; 3],
            sensor_clipped_pct: None,
        }
    }

    fn value(v: &[(&str, f64)], path: &str) -> f64 {
        v.iter().find(|(p, _)| *p == path).map_or(0.0, |(_, x)| *x)
    }

    #[test]
    fn dark_flat_image_is_brightened_and_expanded() {
        let a = adjustments(&stats(0.2, 0.25, 0.4), &clip(0.0, 0.0));
        assert!(value(&a, "light.exposure") > 0.3, "{a:?}");
        assert!(value(&a, "light.whites") > 0.0);
        assert!(value(&a, "light.blacks") < 0.0);
        assert!(value(&a, "light.contrast") > 0.0);
    }

    #[test]
    fn well_exposed_image_is_left_alone() {
        let a = adjustments(&stats(0.04, 0.45, 0.93), &clip(0.0, 0.0));
        assert!(value(&a, "light.exposure").abs() <= 0.05, "{a:?}");
        assert_eq!(value(&a, "light.whites"), 0.0);
        assert_eq!(value(&a, "light.blacks"), 0.0);
    }

    #[test]
    fn clipping_is_recovered_not_increased() {
        let a = adjustments(&stats(0.02, 0.5, 0.99), &clip(5.0, 3.0));
        assert!(value(&a, "light.highlights") < 0.0);
        assert!(value(&a, "light.shadows") > 0.0);
        // Ends already clipped: do not push whites/blacks further out.
        assert_eq!(value(&a, "light.whites"), 0.0);
        assert_eq!(value(&a, "light.blacks"), 0.0);
    }

    #[test]
    fn ops_reset_sliders_that_auto_leaves_default() {
        let ops = ops(&stats(0.04, 0.45, 0.93), &clip(0.0, 0.0));
        assert_eq!(ops.len(), 6);
        assert!(
            ops.iter()
                .any(|o| matches!(o, ParamOp::Reset { path } if path.as_str() == "light.whites"))
        );
    }
}
