//! Runs against real raw files when `AIGLE_TEST_RAWS` points at a directory of samples
//! (e.g. from raw.pixls.us); skipped otherwise.

use aigle_libraw_sys::{Kind, Raw};

fn samples() -> Vec<std::path::PathBuf> {
    let Ok(dir) = std::env::var("AIGLE_TEST_RAWS") else {
        eprintln!("AIGLE_TEST_RAWS not set; skipping");
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

#[test]
fn decode_samples() {
    for path in samples() {
        let mut raw = Raw::open(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        raw.unpack(&|| false).unwrap();
        let m = raw.meta().unwrap();
        eprintln!(
            "{}: {} {} {}x{} flip {} kind {:?} cam_mul {:?} cfa0 {:?} BE {:?}",
            path.file_name().unwrap().to_string_lossy(),
            m.make,
            m.model,
            m.width,
            m.height,
            m.flip,
            m.kind,
            m.cam_mul,
            m.cfa[0],
            m.baseline_exposure
        );
        assert!(m.width > 100 && m.height > 100);
        let date = m.shot.date_time.as_deref().unwrap_or_default();
        assert!(date.len() >= 19 && date.as_bytes()[4] == b':', "{date:?}");
        assert!(
            m.shot.exposure_program.is_some_and(|p| p <= 9),
            "{:?}",
            m.shot
        );
        match m.kind {
            Ok(Kind::Bayer | Kind::XTrans) => {
                let (cfa, _, clipped) = raw.copy_cfa(m.width, m.height, None).unwrap();
                let mean = cfa.iter().map(|&v| v as f64).sum::<f64>() / cfa.len() as f64;
                let max = cfa.iter().cloned().fold(f32::MIN, f32::max);
                eprintln!("  mean {mean:.4} max {max:.4} clipped {clipped}");
                assert!(mean > 0.0 && mean < 1.0 && max <= 1.05);
            }
            Ok(Kind::Rgb) => {
                raw.copy_rgb(m.width, m.height).unwrap();
            }
            Err(e) => eprintln!("  unsupported: {e}"),
        }
        assert!(raw.thumbnail().is_ok(), "thumbnail");
    }
}

#[test]
fn cancel_is_reported() {
    let Some(path) = samples().into_iter().next() else {
        return;
    };
    let mut raw = Raw::open(&path).unwrap();
    // Cancellation is polled at progress points; it may or may not trigger for a given
    // decoder, but must never crash.
    let _ = raw.unpack(&|| true);
}

#[test]
fn missing_file_errors() {
    assert!(Raw::open(std::path::Path::new("/nonexistent.cr2")).is_err());
}
