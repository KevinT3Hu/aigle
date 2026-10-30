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
        scope: Default::default(),
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

/// HDR histograms use PQ normalized to the 1000-nit mastering peak, with luma
/// computed in linear Rec.2020 before encoding. Diffuse white is not clipping.
pub fn hdr_histogram(rgba16f: &[u8]) -> Histogram {
    let mut h = Histogram {
        red: vec![0; 256],
        green: vec![0; 256],
        blue: vec![0; 256],
        luma: vec![0; 256],
        scope: Default::default(),
    };
    let peak = crate::hdr::pq_encode(crate::hdr::PEAK_NITS);
    let bin = |v: f32| {
        (crate::hdr::pq_encode(v * crate::hdr::REFERENCE_WHITE) / peak * 255.0)
            .round()
            .clamp(0.0, 255.0) as usize
    };
    for p in rgba16f.as_chunks::<8>().0 {
        let rgba: [f32; 4] = std::array::from_fn(|c| {
            half::f16::from_bits(u16::from_le_bytes([p[c * 2], p[c * 2 + 1]])).to_f32()
        });
        if rgba[3] <= 0.0 {
            continue;
        }
        h.red[bin(rgba[0])] += 1;
        h.green[bin(rgba[1])] += 1;
        h.blue[bin(rgba[2])] += 1;
        h.luma[bin(aigle_kernels::math::luminance([rgba[0], rgba[1], rgba[2]]))] += 1;
    }
    h
}

pub fn rendered_histogram(r: &crate::pipeline::Rendered) -> Histogram {
    let mut h = match r.format {
        aigle_engine_api::PixelFormat::Bgra8 => histogram(&r.data),
        aigle_engine_api::PixelFormat::Rgba16F => hdr_histogram(&r.data),
    };
    let width = r.width.min(128);
    let height = r.height.min(128);
    let hdr = r.format == aigle_engine_api::PixelFormat::Rgba16F;
    let mut pixels = Vec::with_capacity((width * height) as usize);
    for y in 0..height {
        for x in 0..width {
            let sx = ((2 * x + 1) as u64 * r.width as u64 / (2 * width) as u64) as usize;
            let sy = ((2 * y + 1) as u64 * r.height as u64 / (2 * height) as u64) as usize;
            let i = sy * r.width as usize + sx;
            let pixel = if hdr {
                std::array::from_fn(|c| {
                    let j = i * 8 + c * 2;
                    half::f16::from_bits(u16::from_le_bytes([r.data[j], r.data[j + 1]])).to_f32()
                })
            } else {
                let p = &r.data[i * 4..i * 4 + 4];
                let linear = |v: u8| {
                    let v = v as f32 / 255.0;
                    if v <= 0.04045 {
                        v / 12.92
                    } else {
                        ((v + 0.055) / 1.055).powf(2.4)
                    }
                };
                let (r, g, b) = (linear(p[2]), linear(p[1]), linear(p[0]));
                [
                    0.627404 * r + 0.329283 * g + 0.043313 * b,
                    0.069097 * r + 0.919540 * g + 0.011362 * b,
                    0.016391 * r + 0.088013 * g + 0.895595 * b,
                    p[3] as f32 / 255.0,
                ]
            };
            pixels.push(pixel);
        }
    }
    h.scope = aigle_engine_api::ScopeSamples {
        width,
        height,
        hdr,
        pixels,
    };
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
    fn spatial_samples_preserve_position_and_linear_values() {
        let r = crate::pipeline::Rendered {
            width: 2,
            height: 1,
            data: vec![0, 0, 255, 255, 255, 255, 255, 0],
            format: aigle_engine_api::PixelFormat::Bgra8,
            render_scale: 1.0,
            elapsed: std::time::Duration::ZERO,
        };
        let h = rendered_histogram(&r);
        assert_eq!((h.scope.width, h.scope.height), (2, 1));
        assert!(!h.scope.hdr);
        assert!((h.scope.pixels[0][0] - 0.627404).abs() < 1e-6);
        assert_eq!(h.scope.pixels[1][3], 0.0);
        assert_eq!(h.red.iter().sum::<u32>(), 1);
        let r = crate::pipeline::Rendered {
            width: 256,
            height: 256,
            data: vec![255; 256 * 256 * 4],
            ..r
        };
        let h = rendered_histogram(&r);
        assert_eq!(h.scope.pixels.len(), 128 * 128);
        assert_eq!(h.red.iter().sum::<u32>(), 256 * 256);
    }

    #[test]
    fn hdr_spatial_samples_keep_superwhite_and_transparency() {
        let pixels = [[2.0, 0.5, 0.25, 1.0], [0.0, 0.0, 0.0, 0.0]];
        let data = pixels
            .into_iter()
            .flatten()
            .flat_map(|v| half::f16::from_f32(v).to_bits().to_le_bytes())
            .collect();
        let r = crate::pipeline::Rendered {
            width: 2,
            height: 1,
            data,
            format: aigle_engine_api::PixelFormat::Rgba16F,
            render_scale: 1.0,
            elapsed: std::time::Duration::ZERO,
        };
        let h = rendered_histogram(&r);
        assert!(h.scope.hdr);
        assert_eq!(h.scope.pixels[0], [2.0, 0.5, 0.25, 1.0]);
        assert_eq!(h.scope.pixels[1][3], 0.0);
    }

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
