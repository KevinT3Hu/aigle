//! Raw pipeline tests on real files. Set `AIGLE_TEST_RAWS` to a directory of sample raws
//! (e.g. from raw.pixls.us, CC0); skipped otherwise. With `AIGLE_TEST_OUT` set, rendered
//! previews are written there for visual inspection.

use std::sync::Arc;
use std::time::{Duration, Instant};

use aigle_core::params::Demosaic;
use aigle_core::{EditParams, Revision};
use aigle_engine_api::{
    AnalysisSet, EncodedFormat, Engine, EngineConfig, Interaction, LoadState, OffscreenOpts,
    Priority, Quality, Session, SourceKind, SourceRef, ViewSpec,
};

fn samples() -> Vec<std::path::PathBuf> {
    let Ok(dir) = std::env::var("AIGLE_TEST_RAWS") else {
        eprintln!("AIGLE_TEST_RAWS not set; skipping raw tests");
        return Vec::new();
    };
    let mut v: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.is_file())
        .collect();
    v.sort();
    v
}

fn engine() -> Engine {
    aigle_engine::create(EngineConfig {
        memory_budget: Some(4 << 30),
        ..Default::default()
    })
}

fn wait_ready(s: &Session) -> Arc<aigle_engine_api::ImageInfo> {
    let mut rx = s.info();
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        match &*rx.borrow_and_update() {
            LoadState::Ready(i) => return i.clone(),
            LoadState::Failed(e) => panic!("load failed: {e}"),
            LoadState::Loading => {}
        }
        assert!(Instant::now() < deadline, "load timed out");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn raws_open_render_and_analyze() {
    let out_dir = std::env::var("AIGLE_TEST_OUT")
        .ok()
        .map(std::path::PathBuf::from);
    for path in samples() {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let e = engine();
        let t0 = Instant::now();
        let s = e.open(SourceRef::File(path.clone()));
        let info = wait_ready(&s);
        let load = t0.elapsed();
        assert_eq!(info.kind, SourceKind::Raw);
        let cam = info.camera.as_ref().expect("camera info");
        assert!(!cam.make.is_empty());

        let a = s
            .analyze(Arc::new(EditParams::default()), AnalysisSet::default())
            .wait()
            .unwrap();
        let h = a.histogram.as_ref().unwrap();
        let (r, g, b) = (h.red.mean, h.green.mean, h.blue.mean);
        eprintln!(
            "{name}: {} {} {}x{} as-shot {:.0}K/{:.0} load {:.2?} means r{r:.3} g{g:.3} b{b:.3} sensor-clip {:?}%",
            cam.make,
            cam.model,
            info.source_size.w,
            info.source_size.h,
            info.as_shot.temperature,
            info.as_shot.tint,
            load,
            a.clipping.as_ref().unwrap().sensor_clipped_pct,
        );
        assert!(
            (0.03..0.9).contains(&h.luma.mean),
            "{name}: implausible brightness {}",
            h.luma.mean
        );
        // A wrong black level or WB shows up as one channel collapsing (seen with a doubled
        // Canon black): every channel mean must stay within a factor of 2.5 of the others.
        let (lo, hi) = (r.min(g).min(b), r.max(g).max(b));
        assert!(hi < 2.5 * lo, "{name}: colour balance off: r{r} g{g} b{b}");
        assert!(
            (2000.0..15000.0).contains(&info.as_shot.temperature),
            "{name}: as-shot CCT"
        );
        assert!(a.clipping.as_ref().unwrap().sensor_clipped_pct.is_some());
        let n = a.noise_estimate.as_ref().expect("noise estimate");
        eprintln!("{name}: ISO {:?} noise {n:?}", cam.iso);
        assert!(
            (1e-4..0.05).contains(&n.luma_sigma) && (1e-4..0.1).contains(&n.chroma_sigma),
            "{name}: implausible noise {n:?}"
        );

        if let Some(dir) = &out_dir {
            let img = s
                .render_offscreen(
                    Arc::new(EditParams::default()),
                    OffscreenOpts {
                        long_edge: 1024,
                        format: EncodedFormat::Jpeg { quality: 90 },
                        before_after: false,
                    },
                )
                .wait()
                .unwrap();
            std::fs::create_dir_all(dir).unwrap();
            std::fs::write(dir.join(format!("{name}.jpg")), &img.bytes).unwrap();
        }

        let t = e
            .thumbnail(
                SourceRef::File(path.clone()),
                None,
                256,
                Priority::default(),
            )
            .wait()
            .unwrap();
        assert!(t.embedded, "{name}: embedded preview expected");
        assert_eq!(t.pixels.width.max(t.pixels.height), 256);
    }
}

#[test]
fn a_tier_change_rederives_the_source() {
    let Some(path) = samples().into_iter().find(|p| {
        let n = p.to_string_lossy().to_ascii_lowercase();
        !n.ends_with(".raf")
    }) else {
        return;
    };
    let e = engine();
    let s = e.open(SourceRef::File(path));
    wait_ready(&s);
    let view = s.create_view(ViewSpec::fit(600, 400));
    let mut frames = view.frames();
    let first = loop {
        if let Some(f) = frames.borrow_and_update().clone()
            && f.quality != Quality::Draft
        {
            break f;
        }
        std::thread::sleep(Duration::from_millis(10));
    };

    let mut p = EditParams::default();
    p.raw.demosaic = Demosaic::Amaze;
    p.raw.ca_correction = false;
    s.set_edits(Revision(1), Arc::new(p.clone()), Interaction::Final);

    // The analysis of the new params must come from a source decoded with them.
    let a1 = s
        .analyze(Arc::new(EditParams::default()), AnalysisSet::default())
        .wait()
        .unwrap();
    let a2 = s
        .analyze(Arc::new(p), AnalysisSet::default())
        .wait()
        .unwrap();
    assert_ne!(a1.rev_key, a2.rev_key);

    // Wait until the view has re-rendered from the re-derived source.
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        let f = frames.borrow_and_update().clone().unwrap();
        if f.rev == Revision(1) && f.quality != Quality::Draft && f.pixels.data != first.pixels.data
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "view did not pick up the re-derived source"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// A4 (doc 02 §7): re-running highlight reconstruction for another WB / algorithm on a
/// decoded source is much cheaper than a decode, gives exactly what a fresh decode with
/// those params gives, and only touches clipped pixels.
#[test]
fn highlight_reconstruction_rederives_for_user_wb() {
    use aigle_core::params::{HighlightRecon, WbMode};
    use aigle_engine::ledger::MemLedger;
    use aigle_engine::source;

    let ledger = MemLedger::new(8 << 30);
    let mut tested = 0;
    for path in samples() {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let src = SourceRef::File(path.clone());
        let t0 = Instant::now();
        let base = Arc::new(source::load(&src, &EditParams::default(), &ledger, None).unwrap());
        let load = t0.elapsed();
        let clipped = base.sensor_clipped.unwrap();
        let Some(as_shot) = base.applied_hl() else {
            eprintln!("{name}: nothing clipped");
            continue;
        };
        tested += 1;
        let mut p = EditParams::default();
        p.wb.mode = WbMode::Custom;
        p.wb.values.temperature = 3200.0;
        p.wb.values.tint = 10.0;
        for recon in [
            HighlightRecon::Off,
            HighlightRecon::InpaintOpposed,
            HighlightRecon::Guided,
            HighlightRecon::ColorPropagation,
        ] {
            p.raw.highlight_recon = recon;
            assert!(base.decode_matches(&p) && !base.matches(&p));
            let t1 = Instant::now();
            let derived = base.with_highlights(&p, &ledger, None).unwrap();
            let rederive = t1.elapsed();
            assert_ne!(derived.applied_hl(), Some(as_shot));
            assert_ne!(derived.a_key, base.a_key);
            assert!(derived.matches(&p));
            eprintln!(
                "{name}: clip {:.3}% {recon:?} rederive {rederive:.2?} vs load {load:.2?}",
                clipped * 100.0
            );
            if recon != HighlightRecon::ColorPropagation {
                assert!(rederive * 4 < load, "{name}: {recon:?} rederive too slow");
            }
            // Only clipped pixels differ.
            let (l0, d0) = (&base.pyramid.levels[0], &derived.pyramid.levels[0]);
            let changed = l0
                .data
                .iter()
                .zip(d0.data.iter())
                .filter(|(a, b)| a != b)
                .count();
            assert!(
                (changed as f32) < 1.0 + 30.0 * clipped * l0.data.len() as f32,
                "{name}: {recon:?} changed {changed} px"
            );
            // Same as decoding with these params directly (and back again).
            if recon == HighlightRecon::Guided {
                let fresh = source::load(&src, &p, &ledger, None).unwrap();
                assert_eq!(fresh.a_key, derived.a_key);
                for (a, b) in fresh.pyramid.levels.iter().zip(&derived.pyramid.levels) {
                    assert!(
                        a.data == b.data,
                        "{name}: rederive differs from a fresh decode"
                    );
                }
                let back = derived
                    .with_highlights(&EditParams::default(), &ledger, None)
                    .unwrap();
                assert_eq!(back.a_key, base.a_key);
                assert!(back.pyramid.levels[0].data == base.pyramid.levels[0].data);
            }
        }
    }
    if !samples().is_empty() {
        assert!(tested > 0, "no sample with clipped highlights");
    }
}

/// A WB-only edit re-runs A4 on the session's source (no re-decode) once it is final.
#[test]
fn session_wb_edit_rederives_highlights() {
    use aigle_core::params::WbMode;
    use aigle_engine::ledger::MemLedger;

    let ledger = MemLedger::new(8 << 30);
    let Some(path) = samples().into_iter().find(|p| {
        aigle_engine::source::load(
            &SourceRef::File(p.clone()),
            &EditParams::default(),
            &ledger,
            None,
        )
        .is_ok_and(|s| s.applied_hl().is_some())
    }) else {
        return;
    };
    let e = engine();
    let s = e.open(SourceRef::File(path));
    wait_ready(&s);
    let mut p = EditParams::default();
    p.wb.mode = WbMode::Custom;
    p.wb.values.temperature = 3200.0;
    p.wb.values.tint = 10.0;
    let view = s.create_view(ViewSpec::fit(600, 400));
    let mut frames = view.frames();
    s.set_edits(Revision(1), Arc::new(p.clone()), Interaction::Continuous);
    p.wb.values.temperature = 3000.0;
    s.set_edits(Revision(2), Arc::new(p.clone()), Interaction::Final);
    // The view settles on a full-quality frame of revision 2 (re-rendered once the A4
    // re-run lands), and jobs for the head params agree with it.
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if let Some(f) = frames.borrow_and_update().clone()
            && f.rev == Revision(2)
            && f.quality != Quality::Draft
        {
            break;
        }
        assert!(Instant::now() < deadline, "no final frame for the WB edit");
        std::thread::sleep(Duration::from_millis(20));
    }
    let t = Instant::now();
    s.analyze(Arc::new(p), AnalysisSet::default())
        .wait()
        .unwrap();
    eprintln!("analyze after the WB edit: {:.2?}", t.elapsed());
}

/// LibRaw's make/model names find the camconst white levels (when RawTherapee's table is
/// installed) and those levels are below LibRaw's nominal maximum for these cameras.
#[test]
fn camconst_white_levels_match_sample_cameras() {
    use aigle_engine::ledger::MemLedger;
    let Some(table) = aigle_engine::camconst::table() else {
        eprintln!("no camconst.json; skipping");
        return;
    };
    let ledger = MemLedger::new(8 << 30);
    for path in samples() {
        if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("dng"))
        {
            continue;
        }
        let s = aigle_engine::source::load(
            &SourceRef::File(path.clone()),
            &EditParams::default(),
            &ledger,
            None,
        )
        .unwrap();
        let cam = s.info.camera.as_ref().unwrap();
        let w = table.white(
            &cam.make,
            &cam.model,
            cam.iso.unwrap_or(0.0),
            cam.f_number.unwrap_or(0.0),
        );
        eprintln!("{} {}: white {w:?}", cam.make, cam.model);
        if cam.model == "X-T20" {
            assert_eq!(w, Some([16100.0; 4]));
        }
        if cam.model == "EOS 6D" {
            assert!(w.is_some_and(|w| w[0] < 15490.0), "{w:?}");
        }
    }
}

/// Copy of a little-endian DNG with a NoiseProfile added to IFD 0 (the IFD is rewritten at
/// the end of the file; all other offsets are absolute and stay valid).
fn with_noise_profile(b: &[u8], pairs: &[(f64, f64)]) -> Vec<u8> {
    assert_eq!(&b[..2], b"II");
    let u16_at = |o: usize| u16::from_le_bytes([b[o], b[o + 1]]);
    let u32_at = |o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
    let off = u32_at(4) as usize;
    let n = u16_at(off) as usize;
    let mut entries: Vec<[u8; 12]> = (0..n)
        .map(|i| b[off + 2 + 12 * i..off + 14 + 12 * i].try_into().unwrap())
        .collect();
    let next = u32_at(off + 2 + 12 * n);
    let mut out = b.to_vec();
    while !out.len().is_multiple_of(4) {
        out.push(0);
    }
    let data_at = out.len() as u32;
    for (s, o) in pairs {
        out.extend_from_slice(&s.to_le_bytes());
        out.extend_from_slice(&o.to_le_bytes());
    }
    let mut e = [0u8; 12];
    e[..2].copy_from_slice(&51041u16.to_le_bytes());
    e[2..4].copy_from_slice(&12u16.to_le_bytes());
    e[4..8].copy_from_slice(&(2 * pairs.len() as u32).to_le_bytes());
    e[8..].copy_from_slice(&data_at.to_le_bytes());
    entries.retain(|x| u16::from_le_bytes([x[0], x[1]]) != 51041);
    entries.push(e);
    entries.sort_by_key(|x| u16::from_le_bytes([x[0], x[1]]));
    let ifd_at = out.len() as u32;
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    for x in &entries {
        out.extend_from_slice(x);
    }
    out.extend_from_slice(&next.to_le_bytes());
    out[4..8].copy_from_slice(&ifd_at.to_le_bytes());
    out
}

/// A DNG NoiseProfile becomes the source's noise model (doc 02 §8): analysis reports it, and
/// S-tier NR strength follows it.
#[test]
fn dng_noise_profile_drives_the_noise_model() {
    use aigle_engine::ledger::MemLedger;
    use aigle_engine::noise::NoiseSource;
    use aigle_engine::source;

    let Some(path) = samples()
        .into_iter()
        .find(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("dng")))
    else {
        return;
    };
    let bytes = std::fs::read(&path).unwrap();
    if &bytes[..2] != b"II" {
        return;
    }
    let ledger = MemLedger::new(8 << 30);
    let plain = source::load(
        &SourceRef::File(path.clone()),
        &EditParams::default(),
        &ledger,
        None,
    )
    .unwrap();
    let (measured, how) = plain.noise_estimate();
    assert_ne!(how, NoiseSource::Vendor);

    let dir = std::env::temp_dir().join(format!("aigle-noise-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    // Two profiles: a quiet and a 16× noisier one (variance).
    let quiet = [(2e-5, 1e-8), (1e-5, 5e-9), (2.5e-5, 1e-8)];
    let loud: Vec<(f64, f64)> = quiet.iter().map(|(s, o)| (16.0 * s, 16.0 * o)).collect();
    let mut renders = Vec::new();
    let mut ests = Vec::new();
    for (i, prof) in [quiet.to_vec(), loud].into_iter().enumerate() {
        let p = dir.join(format!("noise{i}.dng"));
        std::fs::write(&p, with_noise_profile(&bytes, &prof)).unwrap();
        let src = source::load(
            &SourceRef::File(p.clone()),
            &EditParams::default(),
            &ledger,
            None,
        )
        .unwrap();
        let (est, how) = src.noise_estimate();
        assert_eq!(how, NoiseSource::Vendor);
        assert!(src.noise_model.is_some());
        ests.push(est);

        let e = engine();
        let s = e.open(SourceRef::File(p));
        wait_ready(&s);
        let mut ep = EditParams::default();
        ep.detail.noise.luma = 60.0;
        ep.detail.sharpen.amount = 0.0;
        let a = s
            .analyze(Arc::new(ep.clone()), AnalysisSet::default())
            .wait()
            .unwrap();
        assert_eq!(a.noise_estimate.as_ref(), Some(&ests[i]));
        let img = s
            .render_offscreen(
                Arc::new(ep),
                OffscreenOpts {
                    long_edge: 800,
                    format: EncodedFormat::Png,
                    before_after: false,
                },
            )
            .wait()
            .unwrap();
        renders.push(img.bytes);
    }
    let _ = std::fs::remove_dir_all(&dir);
    eprintln!("measured {measured:?}, vendor {:?}", ests);
    // 16× variance → 4× σ.
    let r = ests[1].luma_sigma / ests[0].luma_sigma;
    assert!((r - 4.0).abs() < 0.01, "{r}");
    // The NR strength follows the model, so the renders differ.
    assert_ne!(renders[0], renders[1]);
}

/// Auto WB on real raws (doc 06 §3.1): the estimate lands near the camera's as-shot white,
/// except on the 6D sample, whose as-shot white renders the (neutral) SpyderCube and
/// tablecloth visibly blue; there auto WB must warm it. Analysis reports the same estimate.
#[test]
fn auto_white_balance_on_raws() {
    for path in samples() {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let e = engine();
        let s = e.open(SourceRef::File(path));
        let info = wait_ready(&s);
        let t0 = Instant::now();
        let est = s.auto_white_balance(None).wait().unwrap();
        let dt = t0.elapsed();
        let a = &info.as_shot;
        let dm = (1e6 / est.temperature as f64 - 1e6 / a.temperature as f64).abs();
        eprintln!(
            "{name}: as-shot {:.0}K/{:.0}, auto {est:?}, Δ {dm:.1} mired, {dt:.2?}",
            a.temperature, a.tint
        );
        assert!(est.confidence > 0.0 && est.confidence <= 1.0);
        if name.ends_with(".CR2") {
            assert!(
                est.temperature > a.temperature + 1000.0,
                "{name}: blue cast kept"
            );
        } else {
            assert!(dm < 30.0, "{name}: auto WB far from as-shot");
        }
        assert!(
            (est.tint - a.tint).abs() < 25.0,
            "{name}: tint far from as-shot"
        );
        let an = s
            .analyze(Arc::new(EditParams::default()), AnalysisSet::default())
            .wait()
            .unwrap();
        assert_eq!(an.wb_estimate.as_ref(), Some(&est));
    }
}

#[test]
fn decode_cancels_within_the_demosaic() {
    // doc 05 §8: cancelling an open stops within ~1 MP of demosaic work, not after the
    // whole native call.
    for path in samples() {
        let src = SourceRef::File(path.clone());
        let mut p = EditParams::default();
        p.raw.demosaic = Demosaic::Amaze;
        let ledger = aigle_engine::ledger::MemLedger::new(4 << 30);
        let t0 = Instant::now();
        aigle_engine::source::load(&src, &p, &ledger, None).unwrap();
        let full = t0.elapsed();
        let cancel = aigle_engine_api::CancelToken::new();
        let c = cancel.clone();
        let t = std::thread::spawn(move || {
            std::thread::sleep(full / 2);
            c.cancel();
            Instant::now()
        });
        let res = aigle_engine::source::load(&src, &p, &ledger, Some(&cancel));
        let latency = Instant::now() - t.join().unwrap();
        eprintln!(
            "{}: decode {full:?}, cancelled after {latency:?}",
            path.display()
        );
        assert!(
            matches!(res, Err(aigle_engine::source::LoadError::Cancelled)),
            "{}",
            path.display()
        );
        assert!(
            latency < full / 4 + Duration::from_millis(100),
            "{latency:?}"
        );
    }
}

/// DNG export keeps the raw data: the DNG renders like the original under the same edits,
/// carries the camera and lens facts and an embedded preview. With `AIGLE_TEST_OUT` set the
/// DNGs are kept there.
#[test]
fn dng_export_round_trips() {
    let keep = std::env::var_os("AIGLE_TEST_OUT").map(std::path::PathBuf::from);
    let dir = keep
        .clone()
        .unwrap_or_else(|| std::env::temp_dir().join(format!("aigle-dng-{}", std::process::id())));
    std::fs::create_dir_all(&dir).unwrap();
    for path in samples() {
        if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("dng"))
        {
            continue;
        }
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let e = engine();
        let mut p = EditParams::default();
        p.light.exposure = 0.3;
        let p = Arc::new(p);
        let out = dir.join(format!("{name}.dng"));
        let t0 = Instant::now();
        let r = e
            .export(
                SourceRef::File(path.clone()),
                p.clone(),
                aigle_engine_api::ExportOptions {
                    path: out.clone(),
                    format: EncodedFormat::Dng,
                    ..Default::default()
                },
            )
            .wait();
        let r = match r {
            Ok(r) => r,
            Err(err) => {
                eprintln!("{name}: no DNG ({err})");
                continue;
            }
        };
        let size = std::fs::metadata(&out).unwrap().len();
        eprintln!(
            "{name}: DNG {}×{} {:.1} MB (raw {:.1} MB) in {:.2?}",
            r.width,
            r.height,
            size as f64 / 1e6,
            std::fs::metadata(&path).unwrap().len() as f64 / 1e6,
            t0.elapsed()
        );
        let render = |src: &std::path::Path| {
            let s = e.open(SourceRef::File(src.to_path_buf()));
            let info = wait_ready(&s);
            let img = s
                .render_offscreen(
                    p.clone(),
                    OffscreenOpts {
                        long_edge: 600,
                        format: EncodedFormat::Png,
                        before_after: false,
                    },
                )
                .wait()
                .unwrap();
            (info, aigle_vips_sys::decode_rgb8(&img.bytes).unwrap())
        };
        let (ia, a) = render(&path);
        let (ib, b) = render(&out);
        assert_eq!(ia.source_size, ib.source_size, "{name}");
        assert_eq!((r.width, r.height), (ia.source_size.w, ia.source_size.h));
        let (ca, cb) = (ia.camera.clone().unwrap(), ib.camera.clone().unwrap());
        assert_eq!((ca.make, ca.model), (cb.make, cb.model), "{name}");
        assert_eq!(ca.lens_model, cb.lens_model, "{name}");
        assert_eq!(ca.iso, cb.iso, "{name}");
        assert!(
            (ia.as_shot.temperature - ib.as_shot.temperature).abs() < 30.0,
            "{name}: as-shot {:?} vs {:?}",
            ia.as_shot,
            ib.as_shot
        );
        assert_eq!((a.0, a.1), (b.0, b.1), "{name}");
        let mean =
            a.2.iter()
                .zip(&b.2)
                .map(|(x, y)| x.abs_diff(*y) as f64)
                .sum::<f64>()
                / a.2.len() as f64;
        eprintln!("{name}: mean |Δ| {mean:.3}");
        assert!(mean < 1.0, "{name}: DNG renders differently ({mean:.3})");
        let t = e
            .thumbnail(SourceRef::File(out.clone()), None, 256, Priority::default())
            .wait()
            .unwrap();
        assert!(t.embedded, "{name}: no embedded preview");
    }
    if keep.is_none() {
        let _ = std::fs::remove_dir_all(&dir);
    }
}
