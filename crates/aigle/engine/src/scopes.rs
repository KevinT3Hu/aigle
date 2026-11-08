//! Display-referred scope distributions shared by desktop frontends.
use aigle_engine_api::Histogram;
pub const N: usize = 64;
pub fn pq(v: f32) -> f32 {
    let p = (v.max(0.0) / 10000.0).powf(2610.0 / 16384.0);
    ((3424.0 / 4096.0 + 2413.0 / 128.0 * p) / (1.0 + 2392.0 / 128.0 * p)).powf(2523.0 / 32.0)
}
pub fn nits(v: f32) -> f32 {
    let p = (v * pq(1000.0)).max(0.0).powf(32.0 / 2523.0);
    10000.0
        * ((p - 3424.0 / 4096.0).max(0.0) / (2413.0 / 128.0 - 2392.0 / 128.0 * p))
            .powf(16384.0 / 2610.0)
}
pub fn srgb(v: f32) -> f32 {
    if v <= 0.0031308 {
        12.92 * v
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}
pub fn rgb709(p: [f32; 4]) -> [f32; 3] {
    [
        1.660491 * p[0] - 0.587641 * p[1] - 0.072850 * p[2],
        -0.124550 * p[0] + 1.132_9 * p[1] - 0.008349 * p[2],
        -0.018151 * p[0] - 0.100579 * p[1] + 1.118_73 * p[2],
    ]
}
pub fn from709(p: [f32; 3]) -> [f32; 4] {
    [
        0.627404 * p[0] + 0.329283 * p[1] + 0.043313 * p[2],
        0.069097 * p[0] + 0.919540 * p[1] + 0.011362 * p[2],
        0.016391 * p[0] + 0.088013 * p[1] + 0.895595 * p[2],
        1.0,
    ]
}
pub fn level(v: f32, hdr: bool) -> f32 {
    if hdr {
        pq(v * 203.0) / pq(1000.0)
    } else {
        srgb(v)
    }
}
pub fn uv(p: [f32; 4], hdr: bool) -> [f32; 2] {
    let rgb = if hdr { [p[0], p[1], p[2]] } else { rgb709(p) };
    let [r, g, b] = rgb.map(|v| level(v, hdr));
    let (kr, kb) = if hdr {
        (0.2627, 0.0593)
    } else {
        (0.2126, 0.0722)
    };
    let y = kr * r + (1.0 - kr - kb) * g + kb * b;
    [(b - y) / (2.0 * (1.0 - kb)), (r - y) / (2.0 * (1.0 - kr))]
}
#[derive(Clone, serde::Serialize)]
pub struct Data {
    pub bins: [Vec<u32>; 4],
    pub wave: [Vec<u32>; 4],
    pub vector: Vec<u32>,
    pub count: u64,
    pub low: u64,
    pub high: u64,
    pub gamut: u64,
    pub hdr: bool,
}
impl Default for Data {
    fn default() -> Self {
        Self {
            bins: std::array::from_fn(|_| vec![0; 256]),
            wave: std::array::from_fn(|_| vec![0; N * N]),
            vector: vec![0; N * N],
            count: 0,
            low: 0,
            high: 0,
            gamut: 0,
            hdr: false,
        }
    }
}
pub fn analyze(h: &Histogram, roi: Option<[f32; 4]>, target: usize) -> Data {
    let gamut_matrix = {
        use aigle_core::color::*;
        let profile = match target {
            1 => DISPLAY_P3_TO_XYZ,
            2 => REC2020_TO_XYZ,
            _ => SRGB_TO_XYZ,
        };
        to_f32(&mul(
            &invert(&profile).expect("standard RGB matrix is invertible"),
            &REC2020_TO_XYZ,
        ))
    };
    let mut d = Data {
        hdr: h.scope.hdr,
        ..Default::default()
    };
    let [x0, y0, x1, y1] = roi.unwrap_or([0.0, 0.0, 1.0, 1.0]);
    let w = h.scope.width as usize;
    if w == 0 {
        d.bins = [
            h.red.clone(),
            h.green.clone(),
            h.blue.clone(),
            h.luma.clone(),
        ];
        return d;
    }
    for (i, &p) in h.scope.pixels.iter().enumerate() {
        let x = ((i % w) as f32 + 0.5) / w as f32;
        let y = ((i / w) as f32 + 0.5) / h.scope.height as f32;
        if p[3] <= 0.0 || x < x0 || x > x1 || y < y0 || y > y1 {
            continue;
        }
        d.count += 1;
        let rgb = if d.hdr { [p[0], p[1], p[2]] } else { rgb709(p) };
        let lum = 0.2627 * p[0] + 0.6780 * p[1] + 0.0593 * p[2];
        let values = [
            level(rgb[0], d.hdr),
            level(rgb[1], d.hdr),
            level(rgb[2], d.hdr),
            level(lum, d.hdr),
        ];
        d.low += u64::from(rgb.iter().any(|&v| v <= 0.0));
        let ceiling = if d.hdr { 1000.0 / 203.0 } else { 1.0 };
        d.high += u64::from(
            rgb.iter()
                .any(|&v| v >= ceiling - if d.hdr { 0.002 } else { 0.00001 }),
        );
        let rgb_target: [f32; 3] = std::array::from_fn(|row| {
            gamut_matrix[row][0] * p[0] + gamut_matrix[row][1] * p[1] + gamut_matrix[row][2] * p[2]
        });
        // Test chromatic gamut independently of luminance/headroom.
        d.gamut += u64::from(rgb_target.iter().any(|&v| v < -0.0001));
        for (c, v) in values.into_iter().enumerate() {
            let v = v.clamp(0.0, 1.0);
            d.bins[c][(v * 255.0).round() as usize] += 1;
            d.wave[c][((1.0 - v) * (N - 1) as f32).round() as usize * N
                + (x * (N - 1) as f32).round() as usize] += 1;
        }
        let [u, v] = uv(p, d.hdr);
        let col = ((u + 0.5) * (N - 1) as f32)
            .round()
            .clamp(0.0, (N - 1) as f32) as usize;
        let row = ((0.5 - v) * (N - 1) as f32)
            .round()
            .clamp(0.0, (N - 1) as f32) as usize;
        d.vector[row * N + col] += 1;
    }
    // Preserve exact full-image distributions; only spatial/region plots are sampled.
    if roi.is_none() {
        d.bins = [
            h.red.clone(),
            h.green.clone(),
            h.blue.clone(),
            h.luma.clone(),
        ];
    }
    d
}

/// An SDR-tonemapped analysis aid; these pixels never replace the HDR preview.
pub fn thumbnail(h: &Histogram) -> Vec<[u8; 4]> {
    h.scope
        .pixels
        .iter()
        .map(|&p| {
            let rgb = rgb709(p).map(|v| {
                let v = if h.scope.hdr {
                    v.max(0.0) / (1.0 + v.max(0.0))
                } else {
                    v
                };
                (srgb(v).clamp(0.0, 1.0) * 255.0).round() as u8
            });
            [
                rgb[0],
                rgb[1],
                rgb[2],
                (p[3].clamp(0.0, 1.0) * 255.0).round() as u8,
            ]
        })
        .collect()
}
/// Primary/secondary reference targets. They do not prove gamut membership.
pub fn reference_targets(hdr: bool) -> Vec<[f32; 2]> {
    [
        [1., 0., 0.],
        [1., 1., 0.],
        [0., 1., 0.],
        [0., 1., 1.],
        [0., 0., 1.],
        [1., 0., 1.],
    ]
    .into_iter()
    .map(|p| {
        uv(
            if hdr {
                [
                    p[0] * 1000. / 203.,
                    p[1] * 1000. / 203.,
                    p[2] * 1000. / 203.,
                    1.,
                ]
            } else {
                from709(p)
            },
            hdr,
        )
    })
    .collect()
}
#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> Histogram {
        Histogram {
            red: vec![0; 256],
            green: vec![0; 256],
            blue: vec![0; 256],
            luma: vec![0; 256],
            scope: aigle_engine_api::ScopeSamples {
                width: 2,
                height: 2,
                hdr: true,
                pixels: vec![
                    [0., 0., 0., 1.],
                    [1., 1., 1., 1.],
                    [5., 5., 5., 1.],
                    [1., 0., 0., 0.],
                ],
            },
        }
    }
    #[test]
    fn region_excludes_transparent_pixels_and_counts_highlights() {
        let h = fixture();
        let d = analyze(&h, Some([0., 0., 1., 1.]), 0);
        assert_eq!((d.count, d.low, d.high), (3, 1, 1));
        assert_eq!(d.wave[3].iter().sum::<u32>(), 3);
        assert_eq!(d.vector.iter().sum::<u32>(), 3);
        let d = analyze(&h, Some([0.5, 0., 1., 1.]), 0);
        assert_eq!((d.count, d.low, d.high), (1, 0, 0));
    }
    #[test]
    fn whole_crop_preserves_exact_bins_and_thumbnail_is_sdr_only() {
        let mut h = fixture();
        h.red[12] = 12345;
        assert_eq!(analyze(&h, None, 0).bins[0][12], 12345);
        assert_eq!(
            analyze(&h, Some([0., 0., 1., 1.]), 0).bins[0]
                .iter()
                .sum::<u32>(),
            3
        );
        let thumbnail = thumbnail(&h);
        assert_eq!(thumbnail.len(), 4);
        assert_eq!(thumbnail[3][3], 0);
        assert_eq!(thumbnail[0], [0, 0, 0, 255]);
        assert!(thumbnail[1][0] > 0 && thumbnail[1][0] < 255);
        let targets = reference_targets(true);
        assert_eq!(targets.len(), 6);
        assert!(targets.iter().all(|p| p.iter().all(|v| v.is_finite())));
    }

    #[test]
    fn pq_guides_and_neutral_vectors() {
        assert!((nits(pq(203.) / pq(1000.)) - 203.).abs() < 0.1);
        assert!((nits(1.) - 1000.).abs() < 0.1);
        let [u, v] = uv([1., 1., 1., 1.], true);
        assert!(u.abs() < 1e-6 && v.abs() < 1e-6);
        let d = analyze(&fixture(), Some([0., 0., 1., 1.]), 2);
        assert_eq!(d.gamut, 0);
    }
    #[test]
    fn target_gamut_and_spatial_channel_locations() {
        let mut h = fixture();
        h.scope.width = 1;
        h.scope.height = 1;
        h.scope.pixels = vec![[1.0, 0.0, 0.0, 1.0]];
        let small = analyze(&h, Some([0., 0., 1., 1.]), 0);
        let wide = analyze(&h, Some([0., 0., 1., 1.]), 2);
        assert_eq!(small.gamut, 1);
        assert_eq!(wide.gamut, 0);
        assert_eq!(wide.bins[0].iter().sum::<u32>(), 1);
        assert_eq!(wide.bins[1][0], 1);
        assert!(
            wide.wave[0]
                .iter()
                .enumerate()
                .any(|(i, &n)| n == 1 && i % N == 32)
        );
        let [u, v] = uv(from709([1., 0., 0.]), false);
        assert!(u < 0.0 && (v - 0.5).abs() < 0.0001);
    }

    #[test]
    #[ignore = "manual scope analysis benchmark"]
    fn scope_analysis_benchmark() {
        let mut h = fixture();
        h.scope.width = 128;
        h.scope.height = 128;
        h.scope.pixels = (0..128 * 128)
            .map(|i| [(i % 128) as f32 / 32.0, (i / 128) as f32 / 32.0, 0.2, 1.0])
            .collect();
        let start = std::time::Instant::now();
        for _ in 0..100 {
            std::hint::black_box(analyze(std::hint::black_box(&h), None, 0));
        }
        eprintln!(
            "128x128 HDR scope analysis: {:.3} ms/update",
            start.elapsed().as_secs_f64() * 10.0
        );
    }
}
