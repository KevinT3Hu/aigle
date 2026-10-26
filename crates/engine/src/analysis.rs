//! Display-referred histogram and analysis statistics (doc 01 §3, doc 06 §2), measured in
//! the output profile the caller names (`AnalysisSet::output`). Sensor clipping comes from
//! the A2 clip count, the WB and noise estimates from `crate::awb` and `crate::noise`.

use aigle_engine_api::{ChannelStats, ClipStats, Histogram, HistogramStats};

const PERCENTILES: [f32; 9] = [0.1, 1.0, 5.0, 25.0, 50.0, 75.0, 95.0, 99.0, 99.9];

/// 256-bin histogram of BGRA8 pixels with alpha > 0. Integer bins, no float atomics (doc 02 §14).
pub fn histogram(bgra: &[u8]) -> Histogram {
    let mut h = Histogram {
        red: vec![0; 256],
        green: vec![0; 256],
        blue: vec![0; 256],
        luma: vec![0; 256],
    };
    for px in bgra.as_chunks::<4>().0 {
        if px[3] == 0 {
            continue;
        }
        let (b, g, r) = (px[0] as u32, px[1] as u32, px[2] as u32);
        h.red[r as usize] += 1;
        h.green[g as usize] += 1;
        h.blue[b as usize] += 1;
        // Rec.709 weights on encoded values, ×10000 fixed point.
        let y = (2126 * r + 7152 * g + 722 * b + 5000) / 10000;
        h.luma[y.min(255) as usize] += 1;
    }
    h
}

fn channel(counts: &[u32]) -> ChannelStats {
    let total: u64 = counts.iter().map(|&c| c as u64).sum();
    let mean = if total == 0 {
        0.0
    } else {
        counts
            .iter()
            .enumerate()
            .map(|(i, &c)| i as f64 * c as f64)
            .sum::<f64>()
            / total as f64
            / 255.0
    };
    let percentiles = PERCENTILES
        .iter()
        .map(|&p| {
            let target = (p as f64 / 100.0 * total as f64).ceil().max(1.0) as u64;
            let mut acc = 0u64;
            let mut bin = 255;
            for (i, &c) in counts.iter().enumerate() {
                acc += c as u64;
                if acc >= target {
                    bin = i;
                    break;
                }
            }
            (p, bin as f32 / 255.0)
        })
        .collect();
    ChannelStats {
        mean: mean as f32,
        percentiles,
        counts: counts.to_vec(),
    }
}

pub fn stats(h: &Histogram) -> (HistogramStats, ClipStats) {
    let pct = |counts: &[u32], bin: usize| {
        let total: u64 = counts.iter().map(|&c| c as u64).sum();
        if total == 0 {
            0.0
        } else {
            counts[bin] as f32 * 100.0 / total as f32
        }
    };
    let hs = HistogramStats {
        red: channel(&h.red),
        green: channel(&h.green),
        blue: channel(&h.blue),
        luma: channel(&h.luma),
    };
    let clip = ClipStats {
        highlights_pct: [pct(&h.red, 255), pct(&h.green, 255), pct(&h.blue, 255)],
        shadows_pct: [pct(&h.red, 0), pct(&h.green, 0), pct(&h.blue, 0)],
        sensor_clipped_pct: None,
    };
    (hs, clip)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stats_of_two_values() {
        let mut px = Vec::new();
        for i in 0..100 {
            let v = if i < 50 { 0 } else { 255 };
            px.extend_from_slice(&[v, v, v, 255]);
        }
        px.extend_from_slice(&[9, 9, 9, 0]); // transparent: ignored
        let h = histogram(&px);
        let (s, c) = stats(&h);
        assert!((s.luma.mean - 0.5).abs() < 1e-6);
        assert_eq!(c.highlights_pct, [50.0; 3]);
        assert_eq!(c.shadows_pct, [50.0; 3]);
        assert_eq!(s.red.percentiles[4], (50.0, 0.0));
        assert_eq!(s.red.percentiles[8], (99.9, 1.0));
    }
}
