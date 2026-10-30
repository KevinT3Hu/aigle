//! RAW open benchmark: raw_load_bench FILE [REPEATS]. Includes the isolated decoder.
use aigle_engine_api::{BackendPreference, EngineConfig, LoadState, Quality, SourceRef, ViewSpec};
use std::time::{Duration, Instant};
fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();
    if aigle_isolate::is_worker() {
        aigle_isolate::serve(aigle_engine::WORKER_OPS);
    }
    aigle_isolate::enable(std::env::current_exe().unwrap(), vec![]);
    let path = std::env::args_os().nth(1).expect("FILE");
    let repeats = std::env::args()
        .nth(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(3);
    let engine = aigle_engine::create(EngineConfig {
        backend: BackendPreference::Cpu,
        memory_budget: Some(4 << 30),
        ..Default::default()
    });
    for i in 0..repeats {
        let t = Instant::now();
        let session = engine.open(SourceRef::File(path.clone().into()));
        let view = session.create_view(ViewSpec::fit(1800, 1200));
        let mut info = session.info();
        let mut frames = view.frames();
        let deadline = t + Duration::from_secs(120);
        loop {
            match &*info.borrow_and_update() {
                LoadState::Ready(info) => {
                    println!(
                        "run {i}: ready {:?} {}x{}",
                        t.elapsed(),
                        info.source_size.w,
                        info.source_size.h
                    );
                    break;
                }
                LoadState::Failed(e) => panic!("{e}"),
                _ => (),
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
        loop {
            if let Some(f) = frames.borrow_and_update().clone()
                && f.quality != Quality::Draft
            {
                println!("run {i}: preview {:?}", t.elapsed());
                println!(
                    "run {i}: {:?} pixels {}",
                    f.quality,
                    blake3::hash(&f.pixels.data)
                );
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}
