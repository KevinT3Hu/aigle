//! End-to-end tests of the engine through the public `aigle-engine-api` surface.

use std::sync::Arc;
use std::time::{Duration, Instant};

use aigle_core::geometry::ImageRect;
use aigle_core::{EditParams, Revision};
use aigle_engine_api::{
    AnalysisPrecision, AnalysisSet, BackendPreference, DisplayProfileId, EncodedFormat, Engine,
    EngineConfig, ExportOptions, Frame, Histogram, Interaction, LoadState, OffscreenOpts,
    ParamsSource, Priority, Quality, SampleStage, Session, SourceRef, SyntheticPattern, ViewSpec,
    Viewport, watch,
};

/// Per-process disk cache, so that tests never touch the user's cache.
fn cache_dir() -> std::path::PathBuf {
    std::env::temp_dir().join(format!("aigle-test-cache-{}", std::process::id()))
}

fn engine() -> Engine {
    aigle_engine::create(EngineConfig {
        memory_budget: Some(1 << 30),
        cache_dir: Some(cache_dir()),
        ..Default::default()
    })
}

fn synthetic(w: u32, h: u32) -> SourceRef {
    SourceRef::Synthetic {
        pattern: SyntheticPattern::ColorChart,
        width: w,
        height: h,
    }
}

fn wait_ready(s: &Session) {
    let mut rx = s.info();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match &*rx.borrow_and_update() {
            LoadState::Ready(_) => return,
            LoadState::Failed(e) => panic!("load failed: {e}"),
            LoadState::Loading => {}
        }
        assert!(Instant::now() < deadline, "load timed out");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Wait for a frame satisfying `pred`.
fn wait_frame(
    rx: &mut watch::Receiver<Option<Arc<Frame>>>,
    pred: impl Fn(&Frame) -> bool,
) -> Arc<Frame> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(f) = rx.borrow_and_update().clone()
            && pred(&f)
        {
            return f;
        }
        assert!(Instant::now() < deadline, "no matching frame");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn mean_bgr(f: &Frame) -> [f64; 3] {
    let mut acc = [0f64; 3];
    let mut n = 0.0;
    for px in f.pixels.data.as_chunks::<4>().0.iter().filter(|p| p[3] > 0) {
        for c in 0..3 {
            acc[c] += px[c] as f64;
        }
        n += 1.0;
    }
    acc.map(|v| v / n)
}

#[test]
fn view_renders_and_follows_edits() {
    let e = engine();
    let s = e.open(synthetic(1200, 800));
    let view = s.create_view(ViewSpec::fit(300, 200));
    let mut frames = view.frames();
    wait_ready(&s);

    let f0 = wait_frame(&mut frames, |f| f.quality != Quality::Draft);
    assert_eq!((f0.pixels.width, f0.pixels.height), (300, 200));
    assert_eq!(f0.rev, Revision::ZERO);
    let m0 = mean_bgr(&f0);

    // Continuous edit → Draft first, then Refined after settling, both at the new revision.
    let mut p = EditParams::default();
    p.light.exposure = 1.0;
    s.set_edits(Revision(1), Arc::new(p.clone()), Interaction::Continuous);
    let refined = wait_frame(&mut frames, |f| {
        f.rev == Revision(1) && f.quality != Quality::Draft
    });
    let m1 = mean_bgr(&refined);
    assert!(
        m1.iter().zip(m0).all(|(a, b)| *a > b),
        "+1 EV must brighten: {m0:?} → {m1:?}"
    );

    // Stale revisions are ignored.
    s.set_edits(
        Revision(0),
        Arc::new(EditParams::default()),
        Interaction::Final,
    );
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(frames.borrow().as_ref().unwrap().rev, Revision(1));

    // Histogram arrives for the settled state.
    let mut hist = view.histogram();
    let deadline = Instant::now() + Duration::from_secs(10);
    while hist.borrow_and_update().is_none() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn original_view_ignores_edits() {
    let e = engine();
    let s = e.open(synthetic(600, 400));
    let before = s.create_view(ViewSpec::fit(150, 100));
    before.set_params_source(ParamsSource::Original);
    let after = s.create_view(ViewSpec::fit(150, 100));
    let (mut fb, mut fa) = (before.frames(), after.frames());
    wait_ready(&s);
    let b0 = wait_frame(&mut fb, |f| f.quality != Quality::Draft);
    wait_frame(&mut fa, |f| f.quality != Quality::Draft);

    let mut p = EditParams::default();
    p.light.exposure = 1.0;
    s.set_edits(Revision(1), Arc::new(p), Interaction::Final);
    wait_frame(&mut fa, |f| {
        f.rev == Revision(1) && f.quality != Quality::Draft
    });
    std::thread::sleep(Duration::from_millis(100));
    let b1 = fb.borrow().clone().unwrap();
    assert!(
        Arc::ptr_eq(&b0, &b1),
        "the original view must not re-render"
    );

    // Switching back to the head renders the edit.
    before.set_params_source(ParamsSource::Current);
    let f = wait_frame(&mut fb, |f| {
        f.rev == Revision(1) && f.pixels.data != b0.pixels.data
    });
    assert!(mean_bgr(&f)[1] > mean_bgr(&b0)[1]);
}

#[test]
fn original_follows_framing_and_uncropped_ignores_the_crop() {
    let e = engine();
    let s = e.open(synthetic(600, 400));
    let before = s.create_view(ViewSpec::fit(150, 100));
    before.set_params_source(ParamsSource::Original);
    let whole = s.create_view(ViewSpec::fit(150, 100));
    whole.set_uncropped(true);
    let (mut fb, mut fw, mut hw) = (before.frames(), whole.frames(), whole.histogram());
    wait_ready(&s);
    let transparent = |f: &Frame| {
        f.pixels
            .data
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|p| p[3] == 0)
            .count()
    };
    let b0 = wait_frame(&mut fb, |f| f.quality != Quality::Draft);
    let w0 = wait_frame(&mut fw, |f| f.quality != Quality::Draft);
    assert_eq!(transparent(&b0), 0);
    let h0 = wait_hist(&mut hw, |_| true);

    let mut p = EditParams::default();
    p.geometry.crop.w = 0.5;
    s.set_edits(Revision(1), Arc::new(p), Interaction::Final);
    // The original takes the crop: half of the fitted canvas is now outside it.
    let b1 = wait_frame(&mut fb, |f| {
        f.rev == Revision(1) && f.quality != Quality::Draft
    });
    assert!(transparent(&b1) > 150 * 100 / 3, "{}", transparent(&b1));
    // The uncropped view keeps its frame but its histogram is of the cropped image.
    let h1 = wait_hist(&mut hw, |h| **h != *h0);
    assert_ne!(*h1, *h0);
    std::thread::sleep(Duration::from_millis(100));
    assert!(Arc::ptr_eq(&w0, &fw.borrow().clone().unwrap()));
    assert_eq!(transparent(&w0), 0);
}

#[test]
fn histogram_is_of_the_whole_image_and_ignores_pan_and_zoom() {
    let e = engine();
    let s = e.open(synthetic(1200, 800));
    let view = s.create_view(ViewSpec::fit(300, 200));
    let (mut frames, mut hist) = (view.frames(), view.histogram());
    wait_ready(&s);
    wait_frame(&mut frames, |f| f.quality != Quality::Draft);
    let h0 = wait_hist(&mut hist, |_| true);

    // During a gesture the histogram follows without waiting for the settle timer.
    let mut p = EditParams::default();
    p.light.exposure = 1.0;
    let t = Instant::now();
    s.set_edits(Revision(1), Arc::new(p.clone()), Interaction::Continuous);
    let h1 = wait_hist(&mut hist, |h| **h != *h0);
    assert!(
        t.elapsed() < Duration::from_millis(150),
        "{:?}",
        t.elapsed()
    );
    assert!(mean_luma(&h1) > mean_luma(&h0));
    wait_frame(&mut frames, |f| {
        f.rev == Revision(1) && f.quality != Quality::Draft
    });
    std::thread::sleep(Duration::from_millis(100));
    // The settled histogram uses the full overview, larger than the gesture one.
    let count = |h: &Histogram| h.luma.iter().map(|&c| c as u64).sum::<u64>();
    let settled = wait_hist(&mut hist, |h| count(h) > count(&h1));

    // Zoom and pan re-render the view but leave the histogram alone.
    for x in [0.0, 300.0, 600.0] {
        view.set_viewport(
            Viewport {
                region: ImageRect::new(x, 0.0, 300.0, 200.0),
                output_size: (300, 200),
                display: DisplayProfileId::Srgb,
            },
            Interaction::Final,
        );
        wait_frame(&mut frames, |f| {
            f.viewport.region.x == x && f.quality != Quality::Draft
        });
    }
    std::thread::sleep(Duration::from_millis(100));
    assert!(Arc::ptr_eq(&settled, hist.borrow().as_ref().unwrap()));

    // Zoomed in, an edit still updates the whole-image histogram.
    p.light.exposure = -1.0;
    s.set_edits(Revision(2), Arc::new(p), Interaction::Final);
    let h2 = wait_hist(&mut hist, |h| !Arc::ptr_eq(h, &settled));
    assert!(mean_luma(&h2) < mean_luma(&settled));
}

fn mean_luma(h: &Histogram) -> f64 {
    let n: u64 = h.luma.iter().map(|&c| c as u64).sum();
    let s: u64 = h
        .luma
        .iter()
        .enumerate()
        .map(|(i, &c)| i as u64 * c as u64)
        .sum();
    s as f64 / n as f64
}

fn wait_hist(
    rx: &mut watch::Receiver<Option<Arc<Histogram>>>,
    pred: impl Fn(&Arc<Histogram>) -> bool,
) -> Arc<Histogram> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(h) = rx.borrow_and_update().clone()
            && pred(&h)
        {
            return h;
        }
        assert!(Instant::now() < deadline, "no matching histogram");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn viewport_zoom_reports_full_at_one_to_one() {
    let e = engine();
    let s = e.open(synthetic(1200, 800));
    wait_ready(&s);
    let view = s.create_view(ViewSpec::fit(100, 100));
    let mut frames = view.frames();
    view.set_viewport(
        Viewport {
            region: ImageRect::new(100.0, 100.0, 256.0, 128.0),
            output_size: (256, 128),
            display: Default::default(),
        },
        Interaction::Final,
    );
    let f = wait_frame(&mut frames, |f| f.pixels.width == 256);
    assert_eq!(f.quality, Quality::Full);
    assert!(
        f.pixels.data.as_chunks::<4>().0.iter().all(|p| p[3] == 255),
        "fully inside the crop"
    );
}

#[test]
fn tiled_render_matches_across_tile_boundaries() {
    // A 1:1 render of a region straddling a tile edge equals the same pixels from a larger
    // render: seams are a bug (doc 02 §14).
    let e = engine();
    let s = e.open(synthetic(1400, 900));
    wait_ready(&s);
    let render = |region: ImageRect, w: u32, h: u32| {
        let view = s.create_view(ViewSpec::fit(1, 1));
        let mut frames = view.frames();
        view.set_viewport(
            Viewport {
                region,
                output_size: (w, h),
                display: Default::default(),
            },
            Interaction::Final,
        );
        wait_frame(&mut frames, |f| f.pixels.width == w)
    };
    let big = render(ImageRect::new(0.0, 0.0, 1024.0, 600.0), 1024, 600);
    let small = render(ImageRect::new(500.0, 480.0, 24.0, 64.0), 24, 64);
    for y in 0..64usize {
        for x in 0..24usize {
            let a = &small.pixels.data[(y * 24 + x) * 4..][..4];
            let b = &big.pixels.data[((480 + y) * 1024 + 500 + x) * 4..][..4];
            assert_eq!(a, b, "pixel ({x},{y})");
        }
    }
}

#[test]
fn offscreen_analyze_sample() {
    let e = engine();
    let s = e.open(synthetic(1200, 800));

    let img = s
        .render_offscreen(
            Arc::new(EditParams::default()),
            OffscreenOpts {
                long_edge: 300,
                ..Default::default()
            },
        )
        .wait()
        .unwrap();
    assert_eq!((img.width, img.height), (300, 200));
    assert!(image::load_from_memory(&img.bytes).is_ok());

    let ba = s
        .render_offscreen(
            Arc::new(EditParams::default()),
            OffscreenOpts {
                long_edge: 300,
                before_after: true,
                format: EncodedFormat::Png,
            },
        )
        .wait()
        .unwrap();
    assert_eq!(ba.width, 600);

    let a = s
        .analyze(Arc::new(EditParams::default()), AnalysisSet::default())
        .wait()
        .unwrap();
    assert_eq!(a.precision, AnalysisPrecision::Overview);
    let mut bright = EditParams::default();
    bright.light.exposure = 3.0;
    let b = s
        .analyze(Arc::new(bright), AnalysisSet::default())
        .wait()
        .unwrap();
    let lum = |x: &aigle_engine_api::Analysis| x.histogram.as_ref().unwrap().luma.mean;
    assert!(lum(&b) > lum(&a));
    assert_ne!(a.rev_key, b.rev_key);

    // Neutral patch of the chart (bottom row, col 3 = 0.18) is mid-grey in scene-linear.
    let px = s
        .sample(
            aigle_core::geometry::Point::new(700.0, 700.0),
            3.0,
            SampleStage::SceneLinear,
        )
        .wait()
        .unwrap();
    for c in px.rgb {
        assert!((c - 0.18).abs() < 2e-3, "{:?}", px.rgb);
    }
}

#[test]
fn export_and_thumbnail() {
    let e = engine();
    let dir = std::env::temp_dir().join(format!("aigle-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("out.jpg");
    let r = e
        .export(
            synthetic(1000, 500),
            Arc::new(EditParams::default()),
            ExportOptions {
                path: path.clone(),
                format: EncodedFormat::default(),
                long_edge: Some(400),
                output: Default::default(),
                ..Default::default()
            },
        )
        .wait()
        .unwrap();
    assert_eq!((r.width, r.height), (400, 200));
    let img = image::open(&path).unwrap();
    assert_eq!((img.width(), img.height()), (400, 200));

    // Rendered input: round-trip the export through the file source path.
    let t = e
        .thumbnail(SourceRef::File(path.clone()), None, 64, Priority::default())
        .wait()
        .unwrap();
    assert_eq!(t.pixels.width.max(t.pixels.height), 64);
    // Edited thumbnails are rendered once, then read from the disk cache.
    let mut p = EditParams::default();
    p.light.exposure = 1.0;
    let p = Arc::new(p);
    let thumb = |p: &Arc<EditParams>| {
        e.thumbnail(
            SourceRef::File(path.clone()),
            Some(p.clone()),
            128,
            Priority::default(),
        )
        .wait()
        .unwrap()
    };
    let a = thumb(&p);
    let thumbs = cache_dir().join("thumbs");
    let entries = || std::fs::read_dir(&thumbs).map_or(0, |d| d.count());
    let n = entries();
    assert!(n >= 1, "no disk-cache entry");
    let b = thumb(&p);
    assert_eq!(entries(), n);
    assert_eq!(
        (a.pixels.width, a.pixels.height),
        (b.pixels.width, b.pixels.height)
    );
    let diff: u64 = a
        .pixels
        .data
        .iter()
        .zip(b.pixels.data.iter())
        .map(|(x, y)| x.abs_diff(*y) as u64)
        .sum();
    let mean = diff as f64 / a.pixels.data.len() as f64;
    assert!(
        mean < 1.5,
        "cached (lossy WebP) thumbnail differs by {mean} on average"
    );
    assert!(!b.embedded);
    // A corrupted entry is detected and re-rendered.
    for f in std::fs::read_dir(&thumbs).unwrap().flatten() {
        std::fs::write(f.path(), b"garbage").unwrap();
    }
    let c = thumb(&p);
    assert_eq!(c.pixels.width, a.pixels.width);
    std::fs::remove_dir_all(&dir).ok();
}

fn export_to(
    e: &Engine,
    src: SourceRef,
    p: &EditParams,
    path: &std::path::Path,
    format: EncodedFormat,
    output: DisplayProfileId,
) {
    e.export(
        src,
        Arc::new(p.clone()),
        ExportOptions {
            path: path.to_owned(),
            format,
            long_edge: None,
            output,
            ..Default::default()
        },
    )
    .wait()
    .unwrap();
}

#[test]
fn sixteen_bit_tiff_matches_8_bit_and_has_finer_steps() {
    // doc 03 §9: 16-bit exports render from an f32 source with f32 intermediates.
    let dir = std::env::temp_dir().join(format!("aigle-16-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut p = EditParams::default();
    p.light.exposure = 0.3;
    p.detail.noise.luma = 30.0;
    p.presence.clarity = 20.0;
    p.presence.vibrance = 20.0;
    let src = SourceRef::Synthetic {
        pattern: SyntheticPattern::Gradient,
        width: 1200,
        height: 700,
    };
    for backend in [BackendPreference::Cpu, BackendPreference::Auto] {
        let e = aigle_engine::create(EngineConfig {
            memory_budget: Some(2 << 30),
            backend,
            ..Default::default()
        });
        let (t16, png) = (dir.join("a.tif"), dir.join("a.png"));
        export_to(
            &e,
            src.clone(),
            &p,
            &t16,
            EncodedFormat::Tiff { sixteen_bit: true },
            Default::default(),
        );
        export_to(
            &e,
            src.clone(),
            &p,
            &png,
            EncodedFormat::Png,
            Default::default(),
        );
        let a = image::open(&t16).unwrap();
        assert!(
            matches!(a, image::DynamicImage::ImageRgb16(_)),
            "{:?}",
            a.color()
        );
        let a = a.to_rgb16();
        let b = image::open(&png).unwrap().to_rgb8();
        assert_eq!(a.dimensions(), b.dimensions());
        let mut worst = 0f32;
        for (x, y) in a.pixels().zip(b.pixels()) {
            for c in 0..3 {
                worst = worst.max((x.0[c] as f32 / 257.0 - y.0[c] as f32).abs());
            }
        }
        assert!(
            worst <= 2.0,
            "{backend:?}: 16-bit vs 8-bit differ by {worst} codes"
        );
        // The exposure ramp along one row: far more distinct levels than 8 bits can hold.
        let row: std::collections::BTreeSet<u16> =
            (0..a.width()).map(|x| a.get_pixel(x, 350).0[1]).collect();
        let row8: std::collections::BTreeSet<u8> =
            (0..b.width()).map(|x| b.get_pixel(x, 350).0[1]).collect();
        assert!(
            row.len() > 3 * row8.len(),
            "{} vs {}",
            row.len(),
            row8.len()
        );
    }
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn pending_and_snapshot_views_render_their_layers() {
    let e = engine();
    let s = e.open(synthetic(400, 300));
    wait_ready(&s);
    let mut head = EditParams::default();
    head.light.exposure = -1.0;
    s.set_edits(Revision(1), Arc::new(head), Interaction::Final);
    let view = |ps: ParamsSource| {
        let mut spec = ViewSpec::fit(200, 150);
        spec.params = ps;
        s.create_view(spec)
    };
    let (cur, pend, snap) = (
        view(ParamsSource::Current),
        view(ParamsSource::Pending),
        view(ParamsSource::Snapshot(Revision(7))),
    );
    let settled = |v: &aigle_engine_api::View, rev: u64| {
        let mut rx = v.frames();
        mean_bgr(&wait_frame(&mut rx, |f| {
            f.rev == Revision(rev) && f.quality != Quality::Draft
        }))
    };
    let base = settled(&cur, 1);
    // Without a layer, Pending and an unknown snapshot render the head.
    assert_eq!(settled(&pend, 1), base);
    assert_eq!(settled(&snap, 1), base);
    let mut bright = EditParams::default();
    bright.light.exposure = 1.0;
    s.set_pending(
        Some((Revision(2), Arc::new(bright.clone()))),
        Interaction::Final,
    );
    s.set_snapshot(Revision(7), Some(Arc::new(EditParams::default())));
    let (p, sn) = (settled(&pend, 2), settled(&snap, 7));
    assert!(p[1] > sn[1] && sn[1] > base[1], "{base:?} {sn:?} {p:?}");
    assert_eq!(settled(&cur, 1), base, "the head view is unaffected");
    s.set_pending(None, Interaction::Final);
    assert_eq!(settled(&pend, 1), base);
}

#[test]
fn failures_are_reported() {
    let e = engine();
    let s = e.open(SourceRef::File("/nonexistent/IMG_0001.CR3".into()));
    let mut rx = s.info();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let LoadState::Failed(_) = &*rx.borrow_and_update() {
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(
        s.render_offscreen(Arc::new(EditParams::default()), OffscreenOpts::default())
            .wait()
            .is_err()
    );
    assert!(s.auto_white_balance(None).wait().is_err());
}

#[test]
fn dropping_a_job_cancels_it() {
    let e = engine();
    let s = e.open(synthetic(4000, 3000));
    let job = s.analyze(
        Arc::new(EditParams::default()),
        AnalysisSet {
            exact: true,
            ..Default::default()
        },
    );
    let token = job.cancel_token().clone();
    drop(job);
    assert!(token.is_cancelled());
}

// ---------------------------------------------------------------------------------------------
// S, C2 and D2 through the public API.

fn chart_session(e: &Engine) -> Session {
    let s = e.open(synthetic(1200, 800));
    wait_ready(&s);
    s
}

/// Offscreen PNG render decoded to RGB8.
fn render_rgb(s: &Session, p: &EditParams, long_edge: u32) -> image::RgbImage {
    let img = s
        .render_offscreen(
            Arc::new(p.clone()),
            OffscreenOpts {
                long_edge,
                format: EncodedFormat::Png,
                before_after: false,
            },
        )
        .wait()
        .unwrap();
    image::load_from_memory(&img.bytes).unwrap().to_rgb8()
}

/// Mean RGB of a chart patch (6 columns × 4 rows, rows 0–2 decreasing saturation, row 3 neutral).
fn patch(img: &image::RgbImage, col: u32, row: u32) -> [f64; 3] {
    let (pw, ph) = (img.width() / 6, img.height() / 4);
    let (cx, cy) = (col * pw + pw / 2, row * ph + ph / 2);
    let mut acc = [0f64; 3];
    let mut n = 0.0;
    for y in cy - ph / 4..cy + ph / 4 {
        for x in cx - pw / 4..cx + pw / 4 {
            let p = img.get_pixel(x, y).0;
            for c in 0..3 {
                acc[c] += p[c] as f64;
            }
            n += 1.0;
        }
    }
    acc.map(|v| v / n)
}

fn luma(p: [f64; 3]) -> f64 {
    0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2]
}

fn chroma(p: [f64; 3]) -> f64 {
    p.iter().cloned().fold(f64::MIN, f64::max) - p.iter().cloned().fold(f64::MAX, f64::min)
}

#[test]
fn c2_tone_controls_move_the_right_tones() {
    let e = engine();
    let s = chart_session(&e);
    let base = render_rgb(&s, &EditParams::default(), 600);
    // Neutral row: col 0 is 3 EV under mid-grey, col 5 is 2 EV over.
    let (dark0, bright0) = (luma(patch(&base, 0, 3)), luma(patch(&base, 5, 3)));

    let mut p = EditParams::default();
    p.light.shadows = 80.0;
    let img = render_rgb(&s, &p, 600);
    assert!(
        luma(patch(&img, 0, 3)) > dark0 + 8.0,
        "shadows must lift dark tones"
    );
    assert!(
        (luma(patch(&img, 5, 3)) - bright0).abs() < 4.0,
        "shadows must leave highlights"
    );

    let mut p = EditParams::default();
    p.light.highlights = -80.0;
    let img = render_rgb(&s, &p, 600);
    assert!(
        luma(patch(&img, 5, 3)) < bright0 - 8.0,
        "highlights must darken bright tones"
    );
    assert!(
        (luma(patch(&img, 0, 3)) - dark0).abs() < 3.0,
        "highlights must leave shadows"
    );

    // Dehaze adds contrast between the neutral extremes.
    let mut p = EditParams::default();
    p.presence.dehaze = 60.0;
    let img = render_rgb(&s, &p, 600);
    let spread = |i: &image::RgbImage| luma(patch(i, 5, 3)) - luma(patch(i, 0, 3));
    assert!(spread(&img) > spread(&base), "dehaze must add contrast");
}

#[test]
fn clarity_and_texture_add_local_contrast() {
    let e = engine();
    let s = chart_session(&e);
    // Mean absolute horizontal gradient of the rendered luma on 4×4 block means (which
    // removes the dither): local contrast.
    let detail = |p: &EditParams| {
        let img = render_rgb(&s, p, 1200);
        let (w, h) = (img.width() / 4, img.height() / 4);
        let block = |bx: u32, by: u32| {
            let mut acc = 0.0;
            for y in 0..4 {
                for x in 0..4 {
                    acc += luma(img.get_pixel(bx * 4 + x, by * 4 + y).0.map(|v| v as f64));
                }
            }
            acc / 16.0
        };
        let mut acc = 0f64;
        for by in 0..h {
            for bx in 1..w {
                acc += (block(bx, by) - block(bx - 1, by)).abs();
            }
        }
        acc
    };
    let base = detail(&EditParams::default());
    let mut p = EditParams::default();
    p.presence.clarity = 80.0;
    let c = detail(&p);
    assert!(c > base * 1.02, "clarity {base} → {c}");
    let mut p = EditParams::default();
    p.presence.clarity = -80.0;
    assert!(detail(&p) < base, "negative clarity");
    let mut p = EditParams::default();
    p.presence.texture = 80.0;
    assert!(detail(&p) > base * 1.01, "texture");
}

#[test]
fn d2_curves_mixer_grading() {
    use aigle_core::params::CurvePoint;
    let e = engine();
    let s = chart_session(&e);
    let base = render_rgb(&s, &EditParams::default(), 600);

    let mut p = EditParams::default();
    p.curve.point.rgb = vec![
        CurvePoint { x: 0.0, y: 0.0 },
        CurvePoint { x: 0.5, y: 0.3 },
        CurvePoint { x: 1.0, y: 1.0 },
    ];
    let img = render_rgb(&s, &p, 600);
    assert!(
        luma(patch(&img, 3, 3)) < luma(patch(&base, 3, 3)) - 15.0,
        "curve darkens midtones"
    );

    // Blue band desaturation leaves red alone. Chart hue = col / 6: col 0 red, col 4 blue.
    let mut p = EditParams::default();
    p.mixer.blue.saturation = -100.0;
    let img = render_rgb(&s, &p, 600);
    assert!(
        chroma(patch(&img, 4, 0)) < 0.6 * chroma(patch(&base, 4, 0)),
        "blue desaturated"
    );
    assert!(
        (chroma(patch(&img, 0, 0)) - chroma(patch(&base, 0, 0))).abs() < 3.0,
        "red kept"
    );

    // Global saturation −100 is greyscale.
    let mut p = EditParams::default();
    p.presence.saturation = -100.0;
    let img = render_rgb(&s, &p, 600);
    assert!(chroma(patch(&img, 1, 0)) < 3.0, "greyscale");

    // Shadow wheel towards blue tints dark neutrals and leaves bright ones.
    let mut p = EditParams::default();
    p.grading.shadows.hue = 240.0;
    p.grading.shadows.saturation = 80.0;
    let img = render_rgb(&s, &p, 600);
    let d = patch(&img, 0, 3);
    let b = patch(&img, 5, 3);
    assert!(d[2] > d[0] + 3.0, "dark neutral tinted blue: {d:?}");
    assert!((b[2] - b[0]).abs() < 4.0, "bright neutral kept: {b:?}");
}

#[test]
fn s_and_d2_are_independent_of_the_tile_grid() {
    // Moving the crop by whole pixels shifts the tile grid relative to the image; with S and
    // D2 active (no crop-dependent C2), the image must be the same up to dither (±1).
    let e = engine();
    let s = e.open(synthetic(1000, 800));
    wait_ready(&s);
    let mut p = EditParams::default();
    p.detail.noise.luma = 50.0;
    p.detail.sharpen.amount = 100.0;
    p.presence.texture = 50.0;
    p.presence.vibrance = 40.0;
    p.mixer.green.hue = 30.0;
    let render = |p: &EditParams, rev: u64, region: ImageRect| {
        s.set_edits(Revision(rev), Arc::new(p.clone()), Interaction::Final);
        let view = s.create_view(ViewSpec::fit(1, 1));
        let mut frames = view.frames();
        view.set_viewport(
            Viewport {
                region,
                output_size: (300, 200),
                display: Default::default(),
            },
            Interaction::Final,
        );
        wait_frame(&mut frames, |f| {
            f.rev == Revision(rev) && f.pixels.width == 300 && f.quality != Quality::Draft
        })
    };
    let a = render(&p, 1, ImageRect::new(450.0, 420.0, 300.0, 200.0));
    let mut q = p.clone();
    q.geometry.crop.x = 0.125;
    q.geometry.crop.w = 0.875;
    let b = render(&q, 2, ImageRect::new(325.0, 420.0, 300.0, 200.0));
    let max = a
        .pixels
        .data
        .iter()
        .zip(b.pixels.data.iter())
        .map(|(x, y)| x.abs_diff(*y))
        .max()
        .unwrap();
    assert!(max <= 1, "tile-grid dependent result: {max}");
}

#[test]
fn d3_vignette_and_grain() {
    let e = engine();
    let s = chart_session(&e);
    let base = render_rgb(&s, &EditParams::default(), 600);
    let corner = |i: &image::RgbImage| luma(i.get_pixel(3, 3).0.map(|v| v as f64));
    let centre = |i: &image::RgbImage| luma(i.get_pixel(300, 200).0.map(|v| v as f64));

    let mut p = EditParams::default();
    p.effects.vignette.amount = -80.0;
    let img = render_rgb(&s, &p, 600);
    assert!(
        corner(&img) < corner(&base) - 10.0,
        "vignette darkens corners"
    );
    assert!(
        (centre(&img) - centre(&base)).abs() < 2.0,
        "vignette keeps centre"
    );

    // Grain adds pixel-level variance inside a flat patch and keeps its mean.
    let mut p = EditParams::default();
    p.effects.grain.amount = 70.0;
    let img = render_rgb(&s, &p, 1200);
    let flat = render_rgb(&s, &EditParams::default(), 1200);
    let stats = |i: &image::RgbImage| {
        let px: Vec<f64> = (430..470)
            .flat_map(|y| (570..630).map(move |x| (x, y)))
            .map(|(x, y)| i.get_pixel(x, y).0[1] as f64)
            .collect();
        let m = px.iter().sum::<f64>() / px.len() as f64;
        (
            m,
            px.iter().map(|v| (v - m).powi(2)).sum::<f64>() / px.len() as f64,
        )
    };
    let ((m0, v0), (m1, v1)) = (stats(&flat), stats(&img));
    assert!(v1 > v0 + 4.0, "grain variance {v0} → {v1}");
    assert!((m1 - m0).abs() < 2.0, "grain mean {m0} → {m1}");
}

#[test]
fn clipping_overlay_and_display_p3() {
    use aigle_engine_api::{DisplayProfileId, Overlays};
    let e = engine();
    let s = chart_session(&e);
    let mut p = EditParams::default();
    p.light.exposure = 3.0;
    p.presence.saturation = 60.0;
    s.set_edits(Revision(1), Arc::new(p), Interaction::Final);
    let render = |display: DisplayProfileId, clipping: bool| {
        let view = s.create_view(ViewSpec::fit(1, 1));
        let mut frames = view.frames();
        view.set_overlays(Overlays {
            clipping,
            ..Default::default()
        });
        view.set_viewport(
            Viewport {
                region: ImageRect::new(0.0, 0.0, 1200.0, 800.0),
                output_size: (300, 200),
                display,
            },
            Interaction::Final,
        );
        wait_frame(&mut frames, |f| {
            f.rev == Revision(1) && f.pixels.width == 300 && f.quality != Quality::Draft
        })
    };
    let plain = render(DisplayProfileId::Srgb, false);
    assert!(
        plain
            .pixels
            .data
            .as_chunks::<4>()
            .0
            .iter()
            .all(|p| p[3] == 255),
        "frames are opaque"
    );
    let over = render(DisplayProfileId::Srgb, true);
    let count = |f: &Frame, px: [u8; 4]| {
        f.pixels
            .data
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|p| **p == px)
            .count()
    };
    assert!(
        count(&over, [0, 0, 255, 255]) > 100,
        "highlight clipping shown in red"
    );
    // The wider P3 gamut flags fewer saturated colours as out of gamut (high takes
    // precedence in the overlay; these saturated, bright patches are mostly both).
    let p3 = render(DisplayProfileId::DisplayP3, true);
    let flagged = |f: &Frame| count(f, [255, 0, 0, 255]) + count(f, [0, 0, 255, 255]);
    assert!(flagged(&p3) < flagged(&over), "P3 is wider than sRGB");
}

// ---------------------------------------------------------------------------------------------
// Local adjustments (M).

fn mask(
    id: u32,
    shape: aigle_core::masks::MaskShape,
    adjust: aigle_core::masks::LocalParams,
) -> aigle_core::masks::LocalAdjustment {
    aigle_core::masks::LocalAdjustment {
        id: aigle_core::MaskId(id),
        name: format!("m{id}"),
        shape,
        invert: false,
        opacity: 1.0,
        adjust,
    }
}

#[test]
fn local_adjustments_apply_only_where_masked() {
    use aigle_core::masks::*;
    let e = engine();
    let s = chart_session(&e);
    let base = render_rgb(&s, &EditParams::default(), 600);
    // Patch centres in normalised coords: col c → (c + 0.5)/6, row r → (r + 0.5)/4.
    let at = |c: u32, r: u32| Pt {
        x: (c as f32 + 0.5) / 6.0,
        y: (r as f32 + 0.5) / 4.0,
    };

    // Radial +2 EV on neutral patch (3, 3) only.
    let mut p = EditParams::default();
    p.masks.push(mask(
        1,
        MaskShape::Radial {
            center: at(3, 3),
            radii: [0.05, 0.08],
            angle: 0.0,
            feather: 10.0,
        },
        LocalParams {
            exposure: Some(2.0),
            ..Default::default()
        },
    ));
    let img = render_rgb(&s, &p, 600);
    assert!(
        luma(patch(&img, 3, 3)) > luma(patch(&base, 3, 3)) + 30.0,
        "inside brightened"
    );
    assert!(
        (luma(patch(&img, 1, 3)) - luma(patch(&base, 1, 3))).abs() < 1.5,
        "outside unchanged"
    );

    // Brush stroke across row 0 desaturates it (D-tier field); row 2 untouched.
    let mut p = EditParams::default();
    p.masks.push(mask(
        2,
        MaskShape::Brush {
            strokes: vec![Stroke {
                points: vec![at(0, 0), at(5, 0)],
                size: 0.06,
                feather: 20.0,
                flow: 100.0,
                erase: false,
            }],
        },
        LocalParams {
            saturation: Some(-100.0),
            contrast: Some(30.0),
            ..Default::default()
        },
    ));
    let img = render_rgb(&s, &p, 600);
    assert!(
        chroma(patch(&img, 2, 0)) < 0.2 * chroma(patch(&base, 2, 0)),
        "brushed row desaturated"
    );
    assert!(
        (chroma(patch(&img, 2, 2)) - chroma(patch(&base, 2, 2))).abs() < 2.0,
        "other row kept"
    );

    // Luminance range on the whole frame: only dark neutrals get +1 EV.
    let mut p = EditParams::default();
    p.masks.push(mask(
        3,
        MaskShape::Range {
            range: RangeKind::Luminance {
                low: 0.0,
                high: 0.25,
            },
            smoothness: 10.0,
        },
        LocalParams {
            exposure: Some(1.0),
            ..Default::default()
        },
    ));
    let img = render_rgb(&s, &p, 600);
    assert!(
        luma(patch(&img, 0, 3)) > luma(patch(&base, 0, 3)) + 5.0,
        "dark neutral lifted"
    );
    assert!(
        (luma(patch(&img, 5, 3)) - luma(patch(&base, 5, 3))).abs() < 1.5,
        "bright neutral kept"
    );

    // Inverted linear gradient with local warmth: the far side gets warmer.
    let mut p = EditParams::default();
    let mut m = mask(
        4,
        MaskShape::Linear {
            from: Pt { x: 0.2, y: 0.5 },
            to: Pt { x: 0.6, y: 0.5 },
        },
        LocalParams {
            temperature: Some(80.0),
            ..Default::default()
        },
    );
    m.invert = true;
    p.masks.push(m);
    let img = render_rgb(&s, &p, 600);
    let (b5, i5) = (patch(&base, 5, 3), patch(&img, 5, 3));
    assert!(
        i5[0] - i5[2] > b5[0] - b5[2] + 10.0,
        "warmer on the far side: {b5:?} → {i5:?}"
    );
    let (b0, i0) = (patch(&base, 0, 3), patch(&img, 0, 3));
    assert!(
        ((i0[0] - i0[2]) - (b0[0] - b0[2])).abs() < 1.5,
        "near side unchanged"
    );
}

#[test]
fn local_texture_and_overlay() {
    use aigle_core::masks::*;
    use aigle_engine_api::Overlays;
    let e = engine();
    let s = chart_session(&e);
    // Local texture +100 on the left half (S tier).
    let mut p = EditParams::default();
    p.masks.push(mask(
        5,
        MaskShape::Linear {
            from: Pt { x: 0.45, y: 0.5 },
            to: Pt { x: 0.5, y: 0.5 },
        },
        LocalParams {
            texture: Some(100.0),
            sharpness: Some(60.0),
            ..Default::default()
        },
    ));
    let img = render_rgb(&s, &p, 1200);
    let base = render_rgb(&s, &EditParams::default(), 1200);
    let diff = |x0: u32| {
        let mut d = 0f64;
        for y in 0..800 {
            for x in x0..x0 + 300 {
                d += (img.get_pixel(x, y).0[1] as f64 - base.get_pixel(x, y).0[1] as f64).abs();
            }
        }
        d / (300.0 * 800.0)
    };
    assert!(
        diff(100) > diff(800) + 0.2,
        "left half changed more: {} vs {}",
        diff(100),
        diff(800)
    );

    // Mask overlay tints exactly the masked side red.
    s.set_edits(Revision(1), Arc::new(p), Interaction::Final);
    let view = s.create_view(ViewSpec::fit(1, 1));
    let mut frames = view.frames();
    view.set_overlays(Overlays {
        mask_overlay: Some(aigle_core::MaskId(5)),
        ..Default::default()
    });
    view.set_viewport(
        Viewport {
            region: ImageRect::new(0.0, 0.0, 1200.0, 800.0),
            output_size: (300, 200),
            display: Default::default(),
        },
        Interaction::Final,
    );
    let f = wait_frame(&mut frames, |f| {
        f.rev == Revision(1) && f.pixels.width == 300 && f.quality != Quality::Draft
    });
    let px = |x: usize, y: usize| &f.pixels.data[(y * 300 + x) * 4..][..4];
    let (l, r) = (px(30, 190), px(270, 190));
    assert!(l[2] as i32 - l[0] as i32 > 60, "left is tinted: {l:?}");
    assert!(
        (r[2] as i32 - r[0] as i32).abs() < 10,
        "right is not: {r:?}"
    );
}

// B1: lens correction and perspective.

fn test_lens() -> aigle_core::lens::LensProfileData {
    use aigle_core::lens::*;
    LensProfileData {
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
            model: DistModel::Poly3 { k1: -0.05 },
        }],
        tca: vec![],
        vignetting: vec![VigEntry {
            focal: 24.0,
            aperture: 8.0,
            distance: 1000.0,
            k: [-0.5, 0.0, 0.0],
        }],
    }
}

#[test]
fn lens_correction_and_perspective() {
    use aigle_core::params::LensProfile;
    let e = engine();
    let s = chart_session(&e);
    let base = render_rgb(&s, &EditParams::default(), 600);
    let corner = |i: &image::RgbImage| luma(i.get_pixel(4, 4).0.map(|v| v as f64));
    let diff = |a: &image::RgbImage, b: &image::RgbImage| {
        a.pixels()
            .zip(b.pixels())
            .map(|(x, y)| (0..3).map(|c| x.0[c].abs_diff(y.0[c]) as u64).sum::<u64>())
            .sum::<u64>() as f64
            / (a.width() * a.height()) as f64
    };

    // Vignetting only: the corner brightens (gain 1/(1 - 0.5 r²) at r ≈ 1), geometry unchanged.
    let mut p = EditParams::default();
    p.lens.profile = LensProfile::Resolved(aigle_engine::data::pin_lens(&test_lens()));
    p.lens.amounts.distortion = 0.0;
    let vig = render_rgb(&s, &p, 600);
    assert!(
        corner(&vig) > corner(&base) + 10.0,
        "vignetting correction lifts the corners"
    );
    let c = |i: &image::RgbImage| i.get_pixel(300, 200).0;
    assert_eq!(c(&vig), c(&base), "centre unchanged");

    // Distortion: pixels move; amount 0 for everything equals no correction.
    p.lens.amounts.distortion = 100.0;
    let dist = render_rgb(&s, &p, 600);
    assert!(
        diff(&dist, &vig) > 1.0,
        "distortion correction moves pixels"
    );
    p.lens.amounts.distortion = 0.0;
    p.lens.amounts.vignetting = 0.0;
    let none = render_rgb(&s, &p, 600);
    assert!(diff(&none, &base) < 0.05, "zero amounts are the identity");

    // An unknown pinned profile renders uncorrected (and does not fail).
    let mut q = EditParams::default();
    q.lens.profile = LensProfile::Resolved(aigle_core::DataRef("00".repeat(32)));
    assert!(diff(&render_rgb(&s, &q, 600), &base) < 0.05);

    // Perspective: changes the image; constrain off leaves black outside the source.
    let mut q = EditParams::default();
    q.geometry.scalars.perspective_vertical = 50.0;
    let persp = render_rgb(&s, &q, 600);
    assert!(diff(&persp, &base) > 1.0);
    q.geometry.constrain = false;
    let open = render_rgb(&s, &q, 600);
    let (w, h) = (open.width(), open.height());
    let top = luma(open.get_pixel(2, 2).0.map(|v| v as f64));
    let bottom = luma(open.get_pixel(2, h - 3).0.map(|v| v as f64));
    assert!(
        top.min(bottom) < 1.0,
        "unconstrained keystone shows black outside the source ({top}, {bottom}, {w}x{h})"
    );
}

#[test]
fn match_lens_from_exif() {
    let Ok(dir) = std::env::var("AIGLE_TEST_RAWS") else {
        return;
    };
    let e = engine();
    let mut found = 0;
    for f in std::fs::read_dir(dir).unwrap() {
        let path = f.unwrap().path();
        let s = e.open(SourceRef::File(path.clone()));
        wait_ready(&s);
        let m = match s.match_lens().wait() {
            Ok(m) => m,
            Err(err) => {
                eprintln!("no lensfun database, skipping: {err}");
                return;
            }
        };
        eprintln!(
            "{}: {:?}",
            path.display(),
            m.first().map(|l| (&l.lens_model, l.score))
        );
        let Some(best) = m.first() else { continue };
        found += 1;
        // The pinned profile renders, and differs from the uncorrected image.
        let mut p = EditParams::default();
        p.lens.profile = aigle_core::params::LensProfile::Resolved(best.data_ref.clone());
        let a = render_rgb(&s, &p, 400);
        let b = render_rgb(&s, &EditParams::default(), 400);
        assert_ne!(a.as_raw(), b.as_raw(), "{}", path.display());
    }
    assert!(found > 0, "no sample raw matched a lens");
}

// C1 DCP profiles and D2 look LUTs (doc 02 §6.2 step 5, §6.4).

mod common;
use common::synthetic_dcp;

#[test]
fn dcp_hue_sat_map_and_look_lut() {
    use aigle_core::params::CameraProfile;
    use aigle_engine_api::ImportKind;
    let e = engine();
    let s = chart_session(&e);
    let base = render_rgb(&s, &EditParams::default(), 600);

    // An identity DCP (no hue shift) renders like the built-in matrix.
    let id = e
        .import(ImportKind::Dcp, synthetic_dcp(0.0, 1.0))
        .wait()
        .unwrap();
    assert_eq!(id.name, "Synthetic test");
    let mut p = EditParams::default();
    p.profile.camera = CameraProfile::Dcp(id.data_ref.clone());
    let img = render_rgb(&s, &p, 600);
    for (col, row) in [(0, 0), (2, 1), (4, 0), (5, 3)] {
        let (a, b) = (patch(&base, col, row), patch(&img, col, row));
        assert!(
            (0..3).all(|c| (a[c] - b[c]).abs() < 3.0),
            "identity DCP patch ({col},{row}): {a:?} vs {b:?}"
        );
    }
    // Desaturating HueSatMap: chroma drops, neutrals stay.
    let r = e
        .import(ImportKind::Dcp, synthetic_dcp(0.0, 0.3))
        .wait()
        .unwrap();
    p.profile.camera = CameraProfile::Dcp(r.data_ref);
    let img = render_rgb(&s, &p, 600);
    assert!(chroma(patch(&img, 0, 0)) < 0.6 * chroma(patch(&base, 0, 0)));
    assert!(chroma(patch(&img, 2, 3)) < 3.0, "neutral stays neutral");
    // Importing is idempotent; a missing DCP falls back to the camera matrix.
    assert_eq!(
        e.import(ImportKind::Dcp, synthetic_dcp(0.0, 1.0))
            .wait()
            .unwrap()
            .data_ref,
        id.data_ref
    );
    assert!(
        e.import(ImportKind::Dcp, b"not a dcp".to_vec())
            .wait()
            .is_err()
    );
    p.profile.camera = CameraProfile::Dcp(aigle_core::DataRef("ab".repeat(32)));
    let img = render_rgb(&s, &p, 600);
    assert_eq!(patch(&img, 0, 0), patch(&base, 0, 0));

    // Look LUT: an inverting cube inverts (in the sRGB encoding); identity is the identity.
    let cube = |f: &dyn Fn(f64) -> f64| {
        let mut t = String::from("TITLE \"t\"\nLUT_3D_SIZE 5\n");
        for b in 0..5 {
            for g in 0..5 {
                for r in 0..5 {
                    let v = |i: i32| f(i as f64 / 4.0);
                    t += &format!("{} {} {}\n", v(r), v(g), v(b));
                }
            }
        }
        t.into_bytes()
    };
    let inv = e
        .import(ImportKind::LookLut, cube(&|x| 1.0 - x))
        .wait()
        .unwrap();
    let ident = e.import(ImportKind::LookLut, cube(&|x| x)).wait().unwrap();
    let mut p = EditParams::default();
    p.profile.look = Some(ident.data_ref);
    let img = render_rgb(&s, &p, 600);
    for (col, row) in [(0, 0), (3, 2), (5, 3)] {
        let (a, b) = (patch(&base, col, row), patch(&img, col, row));
        assert!(
            (0..3).all(|c| (a[c] - b[c]).abs() < 1.5),
            "identity look: {a:?} vs {b:?}"
        );
    }
    p.profile.look = Some(inv.data_ref);
    let img = render_rgb(&s, &p, 600);
    // Patches inside sRGB (the saturated row 0 is not, and E1 fits base and inverse
    // differently there).
    for (col, row) in [(0, 2), (3, 2), (1, 3)] {
        let (a, b) = (patch(&base, col, row), patch(&img, col, row));
        assert!(
            (0..3).all(|c| (255.0 - a[c] - b[c]).abs() < 4.0),
            "inverting look: {a:?} vs {b:?}"
        );
    }
    assert!(
        e.import(ImportKind::LookLut, b"LUT_3D_SIZE 3\n0 0 0\n".to_vec())
            .wait()
            .is_err()
    );
}

/// Scene-linear samples go through the S tier: sharpening changes a sample right at an edge
/// but not in a flat patch.
#[test]
fn white_balance_picker_makes_the_point_neutral() {
    use aigle_core::geometry::Point;
    use aigle_core::params::WbMode;
    let e = engine();
    let s = chart_session(&e);
    let at = |i: u32, j: u32| Point::new(100.0 + 200.0 * i as f64, 100.0 + 200.0 * j as f64);
    let mut tested = 0;
    for (i, j) in [(0, 3), (2, 3), (4, 3), (1, 0), (3, 1), (5, 2)] {
        s.set_edits(
            Revision(1),
            Arc::new(EditParams::default()),
            Interaction::Final,
        );
        let wb = s.neutral_white_balance(at(i, j), 8.0).wait().unwrap();
        if wb.low_confidence_reason.is_some() {
            continue;
        }
        let mut p = EditParams::default();
        p.wb.mode = WbMode::Custom;
        p.wb.values.temperature = wb.temperature;
        p.wb.values.tint = wb.tint;
        s.set_edits(Revision(2), Arc::new(p), Interaction::Final);
        let rgb = s
            .sample(at(i, j), 8.0, SampleStage::SceneLinear)
            .wait()
            .unwrap()
            .rgb;
        let m = (rgb[0] + rgb[1] + rgb[2]) / 3.0;
        for c in rgb {
            assert!(
                (c / m - 1.0).abs() < 0.02,
                "patch ({i}, {j}): {wb:?} → {rgb:?}"
            );
        }
        tested += 1;
    }
    assert!(tested >= 4, "only {tested} patches were in range");
    // Off the image there is nothing to pick.
    assert!(
        s.neutral_white_balance(Point::new(-500.0, -500.0), 4.0)
            .wait()
            .is_err()
    );
}

#[test]
fn scene_sample_includes_the_s_tier() {
    let e = engine();
    let s = e.open(synthetic(1200, 800));
    wait_ready(&s);
    let sample = |s: &Session, x: f64| {
        s.sample(
            aigle_core::geometry::Point::new(x, 700.0),
            0.0,
            SampleStage::SceneLinear,
        )
        .wait()
        .unwrap()
        .rgb
    };
    let mut off = EditParams::default();
    off.detail.sharpen.amount = 0.0;
    off.detail.noise.luma = 0.0;
    off.detail.noise.chroma = 0.0;
    off.presence.texture = 0.0;
    s.set_edits(Revision(1), Arc::new(off.clone()), Interaction::Final);
    let (edge0, flat0) = (sample(&s, 601.0), sample(&s, 700.0));
    let mut on = off;
    on.detail.sharpen.amount = 100.0;
    on.detail.sharpen.detail = 100.0;
    on.detail.sharpen.masking = 0.0;
    s.set_edits(Revision(2), Arc::new(on), Interaction::Final);
    let (edge1, flat1) = (sample(&s, 601.0), sample(&s, 700.0));
    assert!(edge1[1] > edge0[1] * 1.02, "{edge0:?} → {edge1:?}");
    for c in 0..3 {
        assert!((flat1[c] - flat0[c]).abs() < 2e-3, "{flat0:?} → {flat1:?}");
    }
}

#[test]
fn spilled_source_renders_identically() {
    // doc 03 §10 "spill is transparent": a budget too small for the pyramid moves it into
    // memory-mapped files; the render must be bit-identical (deterministic CPU path).
    let dir = std::env::temp_dir().join(format!("aigle-spill-e2e-{}", std::process::id()));
    let render = |budget: u64| {
        let e = aigle_engine::create(EngineConfig {
            memory_budget: Some(budget),
            deterministic: true,
            cache_dir: Some(dir.clone()),
            ..Default::default()
        });
        let s = e.open(synthetic(1800, 1200));
        wait_ready(&s);
        let mut p = EditParams::default();
        p.detail.noise.luma = 40.0;
        p.presence.clarity = 30.0;
        p.light.exposure = 0.4;
        s.set_edits(Revision(1), Arc::new(p), Interaction::Final);
        let view = s.create_view(ViewSpec::fit(1, 1));
        let mut frames = view.frames();
        view.set_viewport(
            Viewport {
                region: ImageRect::new(400.0, 300.0, 600.0, 400.0),
                output_size: (600, 400),
                display: Default::default(),
            },
            Interaction::Final,
        );
        wait_frame(&mut frames, |f| {
            f.rev == Revision(1) && f.pixels.width == 600 && f.quality == Quality::Full
        })
    };
    // Pyramid ≈ 17 MB: spilled above 50 % of a 24 MB budget.
    let small = render(24 << 20);
    assert!(dir.join("source").is_dir(), "the pyramid was not spilled");
    let large = render(1 << 30);
    assert!(small.pixels.data == large.pixels.data);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn interactive_renders_preempt_an_export() {
    // doc 05 §12 scenario "slider drag during export": the export yields at its chunk
    // boundaries, so edits stay responsive, and it still completes.
    let e = aigle_engine::create(EngineConfig {
        memory_budget: Some(4 << 30),
        ..Default::default()
    });
    let src = synthetic(6000, 4000);
    let s = e.open(src.clone());
    wait_ready(&s);
    let view = s.create_view(ViewSpec::fit(1, 1));
    let mut frames = view.frames();
    view.set_viewport(
        Viewport {
            region: ImageRect::new(2000.0, 1500.0, 1200.0, 800.0),
            output_size: (1200, 800),
            display: Default::default(),
        },
        Interaction::Final,
    );
    let mut rev = 0;
    let mut edit = |frames: &mut watch::Receiver<Option<Arc<Frame>>>| {
        rev += 1;
        let mut p = EditParams::default();
        p.light.exposure = 0.05 * rev as f32;
        let t0 = Instant::now();
        s.set_edits(Revision(rev), Arc::new(p), Interaction::Final);
        wait_frame(frames, |f| {
            f.rev == Revision(rev) && f.quality != Quality::Draft
        });
        t0.elapsed()
    };
    edit(&mut frames);
    let median = |mut v: Vec<Duration>| {
        v.sort();
        v[v.len() / 2]
    };
    let idle = median((0..5).map(|_| edit(&mut frames)).collect());
    let dir = std::env::temp_dir().join(format!("aigle-preempt-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut p = EditParams::default();
    p.detail.noise.luma = 40.0;
    p.presence.clarity = 30.0;
    let export = e.export(
        src,
        Arc::new(p),
        ExportOptions {
            path: dir.join("out.jpg"),
            format: EncodedFormat::Jpeg { quality: 90 },
            long_edge: None,
            output: Default::default(),
            ..Default::default()
        },
    );
    std::thread::sleep(Duration::from_millis(200));
    let busy = median((0..5).map(|_| edit(&mut frames)).collect());
    eprintln!("edit latency idle {idle:?}, during export {busy:?}");
    assert!(
        busy < idle * 3 + Duration::from_millis(100),
        "idle {idle:?}, during export {busy:?}"
    );
    export.wait().expect("export completes");
    let _ = std::fs::remove_dir_all(&dir);
}
