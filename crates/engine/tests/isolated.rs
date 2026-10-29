//! Decoding in worker processes gives the same pixels as decoding in-process, and bad
//! files fail their job instead of the process. `harness = false`: the binary is also the
//! worker, and runs once more as a child with isolation on.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use aigle_core::EditParams;
use aigle_engine_api::{
    BackendPreference, EngineConfig, JobError, Priority, ProcessKind, SourceRef,
};

const CHILD: &str = "AIGLE_TEST_ISOLATED_DIR";

fn thumbnails(dir: &Path) -> Vec<u8> {
    let engine = aigle_engine::create(EngineConfig {
        process: ProcessKind::Cli,
        backend: BackendPreference::Cpu,
        deterministic: true,
        memory_budget: Some(1 << 30),
        cache_dir: Some(dir.join("cache")),
        ..Default::default()
    });
    let thumb = |name: &str| {
        futures::executor::block_on(engine.thumbnail(
            SourceRef::File(dir.join(name)),
            Some(Arc::new(EditParams::default())),
            64,
            Priority(0),
        ))
    };
    for bad in ["garbage.jpg", "garbage.nef", "missing.png"] {
        match thumb(bad) {
            Err(JobError::Failed(e)) => eprintln!("{bad}: {e}"),
            r => panic!("{bad}: {:?}", r.map(|t| t.pixels.width)),
        }
    }
    let t = thumb("photo.png").expect("photo.png");
    assert_eq!((t.pixels.width, t.pixels.height), (64, 48));
    t.pixels.data.to_vec()
}

fn main() {
    if aigle_isolate::is_worker() {
        aigle_isolate::serve(aigle_engine::WORKER_OPS);
    }
    if let Some(dir) = std::env::var_os(CHILD) {
        assert!(aigle_isolate::enable("/proc/self/exe".into(), Vec::new()));
        let px = thumbnails(Path::new(&dir));
        std::fs::write(Path::new(&dir).join("isolated.bin"), px).unwrap();
        return;
    }

    let dir = std::env::temp_dir().join(format!("aigle-isolated-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let img = image::RgbImage::from_fn(128, 96, |x, y| {
        image::Rgb([(x * 2) as u8, (y * 2) as u8, ((x + y) % 256) as u8])
    });
    img.save(dir.join("photo.png")).unwrap();
    let garbage: Vec<u8> = (0..20_000u32)
        .map(|i| (i.wrapping_mul(2654435761) >> 24) as u8)
        .collect();
    std::fs::write(dir.join("garbage.jpg"), &garbage).unwrap();
    std::fs::write(dir.join("garbage.nef"), &garbage).unwrap();

    let here = thumbnails(&dir);
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .env(CHILD, &dir)
        .status()
        .unwrap();
    assert!(status.success(), "isolated run: {status}");
    let isolated = std::fs::read(dir.join("isolated.bin")).unwrap();
    assert!(here == isolated, "isolated decode differs");
    let _ = std::fs::remove_dir_all(PathBuf::from(&dir));
    eprintln!("isolated decode matches in-process decode");
}
