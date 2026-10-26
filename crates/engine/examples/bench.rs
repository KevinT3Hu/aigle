//! Interactive-latency benchmark, CPU vs GPU backend:
//! `cargo run --release -p aigle-engine --example bench [RAW_FILE]`
//!
//! Measures the time from `set_edits` to the settled frame of a 1:1 viewport, for an edit that
//! invalidates the C1 tiles (exposure), one that only re-runs the output tiles (contrast) and
//! one that re-runs the S tier (luma noise reduction).

use std::sync::Arc;
use std::time::{Duration, Instant};

use aigle_core::geometry::ImageRect;
use aigle_core::{EditParams, Revision};
use aigle_engine_api::{
    BackendPreference, EngineConfig, Interaction, LoadState, Quality, SourceRef, SyntheticPattern,
    ViewSpec, Viewport,
};

fn main() {
    // `RUST_LOG=aigle_engine::pipeline=debug` prints per-render stage timings.
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();
    let src = match std::env::args().nth(1) {
        Some(p) => SourceRef::File(p.into()),
        None => SourceRef::Synthetic {
            pattern: SyntheticPattern::Gradient,
            width: 6000,
            height: 4000,
        },
    };
    for pref in [BackendPreference::Cpu, BackendPreference::Auto] {
        let e = aigle_engine::create(EngineConfig {
            backend: pref,
            ..Default::default()
        });
        println!("== {:?}", e.backend());
        let s = e.open(src.clone());
        let mut info = s.info();
        while matches!(*info.borrow_and_update(), LoadState::Loading) {
            std::thread::sleep(Duration::from_millis(5));
        }
        let size = (2560u32, 1440u32);
        let view = s.create_view(ViewSpec::fit(size.0, size.1));
        view.set_viewport(
            Viewport {
                region: ImageRect::new(1000.0, 800.0, size.0 as f64, size.1 as f64),
                output_size: size,
                display: Default::default(),
            },
            Interaction::Final,
        );
        let mut frames = view.frames();
        let mut rev = 0u64;
        let mut run = |label: &str, f: &dyn Fn(&mut EditParams, u64)| {
            let mut times = Vec::new();
            for i in 0..8 {
                rev += 1;
                let mut p = EditParams::default();
                f(&mut p, i);
                // Let the previous settled frame's histogram pass finish first.
                std::thread::sleep(Duration::from_millis(
                    std::env::var("BENCH_GAP_MS")
                        .ok()
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(400),
                ));
                let t0 = Instant::now();
                s.set_edits(Revision(rev), Arc::new(p), Interaction::Final);
                loop {
                    if let Some(fr) = frames.borrow_and_update().clone()
                        && fr.rev == Revision(rev)
                        && fr.quality != Quality::Draft
                    {
                        break;
                    }
                    std::thread::sleep(Duration::from_micros(200));
                }
                times.push(t0.elapsed());
            }
            times.sort();
            println!(
                "  {label:<28} median {:>7.1?}  min {:>7.1?}",
                times[times.len() / 2],
                times[0]
            );
        };
        run("exposure (C1 + output tiles)", &|p, i| {
            p.light.exposure = 0.1 * (i + 1) as f32
        });
        run("contrast (output tiles only)", &|p, i| {
            p.light.contrast = 5.0 * (i + 1) as f32
        });
        run("luma NR (S + C1 + output)", &|p, i| {
            p.detail.noise.luma = 10.0 * (i + 1) as f32
        });
    }
}
