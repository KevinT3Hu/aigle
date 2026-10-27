//! GPU vs CPU reference (doc 02 §14, doc 04 §10): the same render on both backends must agree
//! within ≤ 1 code value in 8-bit output. Skipped when no usable GPU is present.
//! With `AIGLE_TEST_RAWS` set, real raws are compared too.

use std::sync::Arc;
use std::time::{Duration, Instant};

use aigle_core::EditParams;
use aigle_core::geometry::ImageRect;
use aigle_engine_api::{
    BackendKind, BackendPreference, DisplayProfileId, Engine, EngineConfig, Frame, Interaction,
    LoadState, Session, SourceRef, SyntheticPattern, ViewSpec, Viewport,
};

mod common;

fn engine(backend: BackendPreference) -> Engine {
    aigle_engine::create(EngineConfig {
        backend,
        memory_budget: Some(4 << 30),
        ..Default::default()
    })
}

fn gpu_engine() -> Option<Engine> {
    let e = engine(BackendPreference::Gpu);
    match e.backend() {
        (BackendKind::Gpu, desc) => {
            eprintln!("{desc}");
            Some(e)
        }
        (_, why) => {
            // A device that exists but rejects the kernels or fails the self-test is a bug.
            assert!(
                !why.contains("loading kernels") && !why.contains("self-test"),
                "GPU present but unusable: {why}"
            );
            eprintln!("no usable GPU, skipping: {why}");
            None
        }
    }
}

fn wait_ready(s: &Session) {
    let mut rx = s.info();
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        match &*rx.borrow_and_update() {
            LoadState::Ready(_) => return,
            LoadState::Failed(e) => panic!("load failed: {e}"),
            LoadState::Loading => {}
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn render(
    e: &Engine,
    src: &SourceRef,
    params: &EditParams,
    region: Option<ImageRect>,
    size: (u32, u32),
) -> Arc<Frame> {
    render_in(e, src, params, region, size, Default::default())
}

fn render_in(
    e: &Engine,
    src: &SourceRef,
    params: &EditParams,
    region: Option<ImageRect>,
    size: (u32, u32),
    display: DisplayProfileId,
) -> Arc<Frame> {
    let s = e.open(src.clone());
    wait_ready(&s);
    s.set_edits(
        aigle_core::Revision(1),
        Arc::new(params.clone()),
        Interaction::Final,
    );
    let view = s.create_view(ViewSpec::fit(size.0, size.1));
    let fallback = ImageRect::new(0.0, 0.0, size.0 as f64, size.1 as f64);
    if region.is_some() || display != DisplayProfileId::Srgb {
        view.set_viewport(
            Viewport {
                region: region.unwrap_or(fallback),
                output_size: size,
                display: display.clone(),
            },
            Interaction::Final,
        );
    }
    let region = region.or((display != DisplayProfileId::Srgb).then_some(fallback));
    let mut frames = view.frames();
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        if let Some(f) = frames.borrow_and_update().clone()
            && f.rev == aigle_core::Revision(1)
            && f.quality != aigle_engine_api::Quality::Draft
            && f.viewport.output_size == size
            && region.is_none_or(|r| f.viewport.region == r)
        {
            return f;
        }
        assert!(Instant::now() < deadline, "no frame");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn srgb_to_lab(p: &[u8]) -> [f64; 3] {
    // BGRA8 sRGB → CIELAB (D65).
    let lin = |v: u8| {
        let c = v as f64 / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    let (r, g, b) = (lin(p[2]), lin(p[1]), lin(p[0]));
    let x = (0.4124 * r + 0.3576 * g + 0.1805 * b) / 0.95047;
    let y = 0.2126 * r + 0.7152 * g + 0.0722 * b;
    let z = (0.0193 * r + 0.1192 * g + 0.9505 * b) / 1.08883;
    let f = |t: f64| {
        if t > 216.0 / 24389.0 {
            t.cbrt()
        } else {
            (24389.0 / 27.0 * t + 16.0) / 116.0
        }
    };
    let (fx, fy, fz) = (f(x), f(y), f(z));
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

/// CIEDE2000 (Sharma, Wu, Dalal 2005).
fn de2000(a: [f64; 3], b: [f64; 3]) -> f64 {
    use std::f64::consts::PI;
    let (l1, a1, b1) = (a[0], a[1], a[2]);
    let (l2, a2, b2) = (b[0], b[1], b[2]);
    let c1 = (a1 * a1 + b1 * b1).sqrt();
    let c2 = (a2 * a2 + b2 * b2).sqrt();
    let cm = (c1 + c2) / 2.0;
    let g = 0.5 * (1.0 - (cm.powi(7) / (cm.powi(7) + 25f64.powi(7))).sqrt());
    let (a1p, a2p) = (a1 * (1.0 + g), a2 * (1.0 + g));
    let (c1p, c2p) = ((a1p * a1p + b1 * b1).sqrt(), (a2p * a2p + b2 * b2).sqrt());
    let hue = |b: f64, a: f64| {
        if a == 0.0 && b == 0.0 {
            0.0
        } else {
            let h = b.atan2(a).to_degrees();
            if h < 0.0 { h + 360.0 } else { h }
        }
    };
    let (h1, h2) = (hue(b1, a1p), hue(b2, a2p));
    let dl = l2 - l1;
    let dc = c2p - c1p;
    let dh = if c1p * c2p == 0.0 {
        0.0
    } else if (h2 - h1).abs() <= 180.0 {
        h2 - h1
    } else if h2 - h1 > 180.0 {
        h2 - h1 - 360.0
    } else {
        h2 - h1 + 360.0
    };
    let dhh = 2.0 * (c1p * c2p).sqrt() * (dh.to_radians() / 2.0).sin();
    let lm = (l1 + l2) / 2.0;
    let cmp = (c1p + c2p) / 2.0;
    let hm = if c1p * c2p == 0.0 {
        h1 + h2
    } else if (h1 - h2).abs() <= 180.0 {
        (h1 + h2) / 2.0
    } else if h1 + h2 < 360.0 {
        (h1 + h2 + 360.0) / 2.0
    } else {
        (h1 + h2 - 360.0) / 2.0
    };
    let t = 1.0 - 0.17 * (hm - 30.0).to_radians().cos()
        + 0.24 * (2.0 * hm).to_radians().cos()
        + 0.32 * (3.0 * hm + 6.0).to_radians().cos()
        - 0.20 * (4.0 * hm - 63.0).to_radians().cos();
    let sl = 1.0 + 0.015 * (lm - 50.0).powi(2) / (20.0 + (lm - 50.0).powi(2)).sqrt();
    let sc = 1.0 + 0.045 * cmp;
    let sh = 1.0 + 0.015 * cmp * t;
    let rt = -2.0
        * (cmp.powi(7) / (cmp.powi(7) + 25f64.powi(7))).sqrt()
        * (60.0 * (-((hm - 275.0) / 25.0).powi(2)).exp())
            .to_radians()
            .sin();
    let _ = PI;
    ((dl / sl).powi(2) + (dc / sc).powi(2) + (dhh / sh).powi(2) + rt * (dc / sc) * (dhh / sh))
        .sqrt()
}

/// GPU tolerance of doc 02 §14: (max code diff, fraction of channels > 1, ΔE2000 p99, max).
fn compare(a: &Frame, b: &Frame) -> (u8, f64, f64, f64) {
    assert_eq!(a.pixels.data.len(), b.pixels.data.len());
    let mut max = 0u8;
    let mut over = 0u64;
    let mut de: Vec<f64> = Vec::with_capacity(a.pixels.data.len() / 4);
    for (pa, pb) in a
        .pixels
        .data
        .as_chunks::<4>()
        .0
        .iter()
        .zip(b.pixels.data.as_chunks::<4>().0)
    {
        assert_eq!(pa[3], pb[3], "coverage differs");
        for c in 0..3 {
            let d = pa[c].abs_diff(pb[c]);
            max = max.max(d);
            over += (d > 1) as u64;
        }
        de.push(if pa == pb {
            0.0
        } else {
            de2000(srgb_to_lab(pa), srgb_to_lab(pb))
        });
    }
    de.sort_by(f64::total_cmp);
    let n = de.len();
    (
        max,
        over as f64 / (3 * n) as f64,
        de[n * 99 / 100],
        de[n - 1],
    )
}

fn edited() -> EditParams {
    let mut p = EditParams::default();
    p.light.exposure = 0.7;
    p.light.contrast = 30.0;
    p.light.whites = -20.0;
    p.light.blacks = 15.0;
    p.presence.saturation = 25.0;
    p.presence.vibrance = 30.0;
    p.geometry.scalars.angle = 3.5;
    p.geometry.crop.x = 0.05;
    p.geometry.crop.y = 0.05;
    p.geometry.crop.w = 0.9;
    p.geometry.crop.h = 0.9;
    p.geometry.orientation_delta.quarter_turns = 1;
    // S
    p.detail.noise.luma = 40.0;
    p.detail.sharpen.amount = 80.0;
    p.detail.sharpen.masking = 20.0;
    p.presence.texture = 30.0;
    p.detail.defringe.purple_amount = 60.0;
    p.detail.defringe.green_amount = 40.0;
    p.spots.push(aigle_core::retouch::Spot::new(
        aigle_core::retouch::SpotMode::Heal,
        aigle_core::masks::Pt { x: 0.3, y: 0.4 },
        aigle_core::masks::Pt { x: 0.6, y: 0.5 },
        0.05,
    ));
    p.red_eye.push(aigle_core::retouch::RedEye::new(
        aigle_core::masks::Pt { x: 0.1, y: 0.1 },
        0.04,
    ));
    // C2
    p.light.highlights = -60.0;
    p.light.shadows = 50.0;
    p.presence.clarity = 40.0;
    p.presence.dehaze = 30.0;
    // D2
    p.curve.parametric.darks = -20.0;
    p.curve.point.rgb = vec![
        aigle_core::params::CurvePoint { x: 0.0, y: 0.02 },
        aigle_core::params::CurvePoint { x: 0.5, y: 0.55 },
        aigle_core::params::CurvePoint { x: 1.0, y: 0.97 },
    ];
    p.mixer.blue.saturation = -40.0;
    p.mixer.orange.hue = 20.0;
    p.mixer.green.luminance = 30.0;
    p.grading.shadows.hue = 220.0;
    p.grading.shadows.saturation = 30.0;
    p.grading.highlights.hue = 40.0;
    p.grading.highlights.saturation = 20.0;
    p.grading.global.luminance = 10.0;
    // D3
    p.effects.vignette.amount = -40.0;
    p.effects.grain.amount = 30.0;
    // M
    use aigle_core::masks::*;
    let pt = |x, y| Pt { x, y };
    p.masks.push(LocalAdjustment {
        id: aigle_core::MaskId(1),
        name: "radial".into(),
        shape: MaskShape::Combine {
            op: CombineOp::Intersect,
            parts: vec![
                MaskShape::Radial {
                    center: pt(0.5, 0.5),
                    radii: [0.3, 0.25],
                    angle: 30.0,
                    feather: 60.0,
                },
                MaskShape::Range {
                    range: RangeKind::Luminance {
                        low: 0.1,
                        high: 0.7,
                    },
                    smoothness: 30.0,
                },
            ],
        },
        invert: false,
        opacity: 0.9,
        adjust: LocalParams {
            exposure: Some(0.6),
            temperature: Some(-30.0),
            shadows: Some(30.0),
            clarity: Some(-20.0),
            contrast: Some(25.0),
            saturation: Some(30.0),
            hue: Some(-10.0),
            texture: Some(40.0),
            noise: Some(30.0),
            ..Default::default()
        },
    });
    p.masks.push(LocalAdjustment {
        id: aigle_core::MaskId(2),
        name: "brush".into(),
        shape: MaskShape::Brush {
            strokes: vec![Stroke {
                points: vec![pt(0.1, 0.8), pt(0.5, 0.6), pt(0.9, 0.9)],
                size: 0.05,
                feather: 50.0,
                flow: 70.0,
                erase: false,
            }],
        },
        invert: false,
        opacity: 1.0,
        adjust: LocalParams {
            whites: Some(-30.0),
            dehaze: Some(30.0),
            tint: Some(20.0),
            sharpness: Some(-50.0),
            ..Default::default()
        },
    });
    p
}

/// `edited()` plus perspective and a synthetic lens profile with distortion, TCA and
/// vignetting (non-affine B1: warp grid, per-channel Lanczos).
fn with_lens() -> EditParams {
    use aigle_core::lens::*;
    let prof = LensProfileData {
        lens_maker: "Test".into(),
        lens_model: "Test 24mm".into(),
        calib_crop: 1.0,
        calib_aspect: 1.5,
        camera_crop: 1.0,
        min_focal: 24.0,
        max_focal: 24.0,
        distortion: vec![DistEntry {
            focal: 24.0,
            real_focal: None,
            model: DistModel::Ptlens {
                a: 0.01,
                b: -0.04,
                c: 0.02,
            },
        }],
        tca: vec![TcaEntry {
            focal: 24.0,
            model: TcaModel::Linear {
                kr: 1.0004,
                kb: 0.9995,
            },
        }],
        vignetting: vec![VigEntry {
            focal: 24.0,
            aperture: 8.0,
            distance: 1000.0,
            k: [-0.4, 0.1, -0.05],
        }],
    };
    let mut p = edited();
    // C1 DCP HueSatMap and a D2 look LUT ride along.
    let e = engine(BackendPreference::Cpu);
    let dcp = e.import(
        aigle_engine_api::ImportKind::Dcp,
        common::synthetic_dcp(15.0, 0.8),
    );
    p.profile.camera = aigle_core::params::CameraProfile::Dcp(dcp.wait().unwrap().data_ref);
    let mut cube = String::from("LUT_3D_SIZE 9\n");
    for b in 0..9 {
        for g in 0..9 {
            for r in 0..9 {
                let (r, g, b) = (r as f64 / 8.0, g as f64 / 8.0, b as f64 / 8.0);
                cube += &format!(
                    "{} {} {}\n",
                    (0.9 * r + 0.1 * g * g).min(1.0),
                    g.powf(1.2),
                    0.8 * b + 0.15 * r
                );
            }
        }
    }
    let look = e.import(aigle_engine_api::ImportKind::LookLut, cube.into_bytes());
    p.profile.look = Some(look.wait().unwrap().data_ref);
    p.lens.profile = aigle_core::params::LensProfile::Resolved(aigle_engine::data::pin_lens(&prof));
    p.geometry.scalars.perspective_vertical = 25.0;
    p.geometry.scalars.perspective_horizontal = -10.0;
    p
}

fn monochrome() -> EditParams {
    let mut p = edited();
    p.bw.monochrome = true;
    p.bw.mix.red = 40.0;
    p.bw.mix.blue = -60.0;
    p.bw.mix.green = 25.0;
    p
}

#[test]
fn gpu_matches_cpu_reference() {
    let Some(gpu) = gpu_engine() else { return };
    let cpu = engine(BackendPreference::Cpu);
    assert_eq!(cpu.backend().0, BackendKind::Cpu);

    let mut sources = vec![
        SourceRef::Synthetic {
            pattern: SyntheticPattern::Gradient,
            width: 1800,
            height: 1200,
        },
        SourceRef::Synthetic {
            pattern: SyntheticPattern::ColorChart,
            width: 1200,
            height: 800,
        },
    ];
    if let Ok(dir) = std::env::var("AIGLE_TEST_RAWS") {
        let mut raws: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        raws.sort();
        sources.extend(raws.into_iter().map(SourceRef::File));
    }

    for src in &sources {
        for (name, params) in [
            ("default", EditParams::default()),
            ("edited", edited()),
            ("lens", with_lens()),
            ("bw", monochrome()),
        ] {
            // Fit (pyramid level > 0) and a 1:1 region (level 0, several tiles).
            for (view, region, size) in [
                ("fit", None, (800, 600)),
                (
                    "1:1",
                    Some(ImageRect::new(300.0, 200.0, 700.0, 520.0)),
                    (700, 520),
                ),
            ] {
                let a = render(&cpu, src, &params, region, size);
                let b = render(&gpu, src, &params, region, size);
                let (max, over, de99, demax) = compare(&a, &b);
                eprintln!(
                    "{src:?} {name} {view}: max {max} >1: {over:.1e} ΔE p99 {de99:.3} max {demax:.3}"
                );
                assert!(
                    max <= 2 && over <= 1e-5,
                    "{src:?} {name} {view}: code values differ by up to {max} ({over:.1e} > 1)"
                );
                assert!(
                    de99 <= 0.5 && demax <= 2.0,
                    "{src:?} {name} {view}: ΔE2000 p99 {de99} max {demax}"
                );
            }
        }
    }
}

#[test]
fn gpu_matches_cpu_in_every_output_space() {
    let Some(gpu) = gpu_engine() else { return };
    let cpu = engine(BackendPreference::Cpu);
    let src = SourceRef::Synthetic {
        pattern: SyntheticPattern::ColorChart,
        width: 1200,
        height: 800,
    };
    let region = Some(ImageRect::new(250.0, 150.0, 600.0, 400.0));
    for display in [
        DisplayProfileId::DisplayP3,
        DisplayProfileId::AdobeRgb,
        DisplayProfileId::ProPhoto,
        DisplayProfileId::Rec2020,
    ] {
        let a = render_in(&cpu, &src, &edited(), region, (600, 400), display.clone());
        let b = render_in(&gpu, &src, &edited(), region, (600, 400), display.clone());
        let (max, over, ..) = compare(&a, &b);
        eprintln!("{display:?}: max {max} >1: {over:.1e}");
        assert!(
            max <= 2 && over <= 1e-5,
            "{display:?}: code values differ by up to {max} ({over:.1e} > 1)"
        );
        // A different space renders different code values (the transfer and matrix are used).
        let srgb = render(&cpu, &src, &edited(), region, (600, 400));
        assert!(compare(&a, &srgb).0 > 4, "{display:?} looks like sRGB");
    }
}

/// A registered display profile renders like the built-in space it describes, on the CPU
/// and on the GPU (matrix from the colorants, curve through the `TRANSFER_TABLE` tables).
#[test]
fn display_profile_matches_its_builtin_space() {
    let cpu = engine(BackendPreference::Cpu);
    let bytes = aigle_engine::output_space::icc_profile(&DisplayProfileId::AdobeRgb);
    let imported = cpu
        .import(aigle_engine_api::ImportKind::DisplayProfile, bytes)
        .wait()
        .unwrap();
    let icc = DisplayProfileId::Icc(imported.data_ref.as_str().to_string());
    let src = SourceRef::Synthetic {
        pattern: SyntheticPattern::ColorChart,
        width: 1200,
        height: 800,
    };
    let region = Some(ImageRect::new(250.0, 150.0, 600.0, 400.0));
    let want = render_in(
        &cpu,
        &src,
        &edited(),
        region,
        (600, 400),
        DisplayProfileId::AdobeRgb,
    );
    let got = render_in(&cpu, &src, &edited(), region, (600, 400), icc.clone());
    let (max, over, ..) = compare(&want, &got);
    assert!(
        max <= 1 && over <= 1e-5,
        "CPU: up to {max} ({over:.1e} > 1)"
    );
    if let Some(gpu) = gpu_engine() {
        let g = render_in(&gpu, &src, &edited(), region, (600, 400), icc);
        let (max, over, ..) = compare(&want, &g);
        assert!(
            max <= 2 && over <= 1e-5,
            "GPU: up to {max} ({over:.1e} > 1)"
        );
    }
}

#[test]
fn deterministic_forces_cpu() {
    let e = aigle_engine::create(EngineConfig {
        deterministic: true,
        ..Default::default()
    });
    assert_eq!(e.backend().0, BackendKind::Cpu);
}
