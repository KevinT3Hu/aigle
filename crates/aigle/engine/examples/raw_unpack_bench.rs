//! Unpack latency, metadata and pixel hashes: raw_unpack_bench FILE [REPEATS].
use std::{path::Path, time::Instant};
fn main() {
    let path = std::env::args_os().nth(1).expect("FILE");
    let count = std::env::args()
        .nth(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(3);
    for i in 0..count {
        let t = Instant::now();
        let mut raw = aigle_libraw_sys::Raw::open(Path::new(&path)).unwrap();
        raw.unpack(&|| false).unwrap();
        let elapsed = t.elapsed();
        let m = raw.meta().unwrap();
        let meta = blake3::hash(format!("{m:?}").as_bytes());
        let (pixels, mask, clipped) = raw.copy_cfa(m.width, m.height, None).unwrap();
        println!(
            "run {i}: unpack {elapsed:?}, metadata {meta}, pixels {}, mask {}, clipped {clipped}",
            blake3::hash(bytemuck::cast_slice(&pixels)),
            blake3::hash(&mask)
        );
    }
}
