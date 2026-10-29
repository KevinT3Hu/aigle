//! Detailed pan/zoom latency: `cargo run --release -p aigle-engine --example viewport_bench -- [cpu|auto] [FILE]`
use aigle_core::{EditParams, Revision, geometry::ImageRect};
use aigle_engine_api::{
    BackendPreference, EngineConfig, Frame, Interaction, LoadState, Quality, SourceRef,
    SyntheticPattern, View, ViewSpec, Viewport,
};
use std::sync::Arc;
use std::time::{Duration, Instant};

fn wait(view: &View, vp: &Viewport) -> Arc<Frame> {
    let mut rx = view.frames();
    let start = Instant::now();
    loop {
        if let Some(f) = rx.borrow_and_update().clone()
            && f.viewport == *vp
            && f.quality != Quality::Draft
        {
            return f;
        }
        assert!(start.elapsed() < Duration::from_secs(120), "frame timeout");
        std::thread::sleep(Duration::from_micros(200));
    }
}
fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();
    let args: Vec<_> = std::env::args().collect();
    // Continuous includes the UI's 150 ms settle timer before detailed output.
    let interaction = if std::env::var_os("VIEWPORT_BENCH_CONTINUOUS").is_some() {
        Interaction::Continuous
    } else {
        Interaction::Final
    };
    println!("interaction: {interaction:?}");
    let backend = if args.get(1).is_some_and(|a| a == "auto") {
        BackendPreference::Auto
    } else {
        BackendPreference::Cpu
    };
    let src = args
        .get(2)
        .map(|p| SourceRef::File(p.into()))
        .unwrap_or(SourceRef::Synthetic {
            pattern: SyntheticPattern::Gradient,
            width: 6000,
            height: 4000,
        });
    let e = aigle_engine::create(EngineConfig {
        backend,
        memory_budget: Some(4 << 30),
        ..Default::default()
    });
    println!("backend: {:?}", e.backend());
    let session = e.open(src);
    let mut info = session.info();
    let start = Instant::now();
    let (iw, ih) = loop {
        match &*info.borrow_and_update() {
            LoadState::Ready(info) => break (info.source_size.w as f64, info.source_size.h as f64),
            LoadState::Failed(err) => panic!("{err}"),
            _ => {}
        }
        assert!(start.elapsed() < Duration::from_secs(120));
        std::thread::sleep(Duration::from_millis(5));
    };
    for detail in [false, true] {
        let mut params = EditParams::default();
        if !detail {
            params.detail.sharpen.amount = 0.0;
            params.detail.noise.chroma = 0.0;
        }
        session.set_edits(
            Revision(if detail { 2 } else { 1 }),
            Arc::new(params),
            Interaction::Final,
        );
        // Match the canvas's independent whole-image backing.
        let backing = session.create_view(ViewSpec::fit(1024, 683));
        let full = Viewport {
            region: ImageRect::new(0.0, 0.0, iw, ih),
            output_size: (1024, 683),
            display: Default::default(),
        };
        backing.set_viewport(full.clone(), Interaction::Final);
        wait(&backing, &full);
        let view = session.create_view(ViewSpec::fit(1, 1));
        for warm in [false, true] {
            for (label, zooms) in [
                ("pan", vec![1.0; 10]),
                (
                    "zoom",
                    vec![0.5, 0.7, 1.0, 1.4, 2.0, 1.4, 1.0, 0.7, 0.5, 1.0],
                ),
            ] {
                let mut times = Vec::new();
                let mut hash = blake3::Hasher::new();
                for (i, zoom) in zooms.into_iter().enumerate() {
                    let vp = Viewport {
                        region: ImageRect::new(
                            800.0 + i as f64 * 137.0,
                            700.0,
                            1920.0 / zoom,
                            1080.0 / zoom,
                        ),
                        output_size: (1920, 1080),
                        display: Default::default(),
                    };
                    let t = Instant::now();
                    view.set_viewport(vp.clone(), interaction);
                    let frame = wait(&view, &vp);
                    times.push(t.elapsed().as_secs_f64() * 1000.0);
                    hash.update(&frame.pixels.data);
                }
                times.sort_by(f64::total_cmp);
                println!(
                    "detail={detail} warm={warm} {label}: median={:.2}ms p90={:.2}ms max={:.2}ms hash={}",
                    times[5],
                    times[8],
                    times[9],
                    hash.finalize()
                );
            }
        }
    }
}
