//! Headless render of one file: `cargo run -p aigle-engine --example render -- IN OUT.jpg [PARAMS_JSON] [LONG_EDGE]`
//!
//! A development tool until the `aigle` CLI exists.

use std::sync::Arc;

use aigle_core::EditParams;
use aigle_engine_api::{EngineConfig, OffscreenOpts, SourceRef};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        eprintln!("usage: render IN OUT.jpg [PARAMS_JSON] [LONG_EDGE]");
        std::process::exit(2);
    }
    let params: EditParams = match args.get(2) {
        Some(j) => serde_json::from_str(j).expect("params JSON"),
        None => EditParams::default(),
    };
    let long_edge = args
        .get(3)
        .map(|s| s.parse().expect("long edge"))
        .unwrap_or(1024);
    let engine = aigle_engine::create(EngineConfig {
        process: aigle_engine_api::ProcessKind::Cli,
        ..Default::default()
    });
    let t0 = std::time::Instant::now();
    let session = engine.open(SourceRef::File(args[0].clone().into()));
    let img = session
        .render_offscreen(
            Arc::new(params),
            OffscreenOpts {
                long_edge,
                ..Default::default()
            },
        )
        .wait()
        .unwrap_or_else(|e| panic!("render failed: {e}"));
    std::fs::write(&args[1], &img.bytes).expect("write output");
    eprintln!("{}x{} in {:.2?}", img.width, img.height, t0.elapsed());
}
