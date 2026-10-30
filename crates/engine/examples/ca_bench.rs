//! Native CA profile and correctness benchmark: ca_bench FILE [REPEATS].
use std::time::Instant;
fn main() {
    let file = std::env::args_os().nth(1).expect("FILE");
    let repeats = std::env::args()
        .nth(2)
        .and_then(|n| n.parse().ok())
        .unwrap_or(3);
    let mut raw = aigle_libraw_sys::Raw::open(std::path::Path::new(&file)).unwrap();
    raw.unpack(&|| false).unwrap();
    let meta = raw.meta().unwrap();
    let (w, h) = (meta.width, meta.height);
    let (mut cfa, _, _) = raw.copy_cfa(w, h, None).unwrap();
    let pattern = [
        [meta.cfa[0][0] as i32, meta.cfa[0][1] as i32],
        [meta.cfa[1][0] as i32, meta.cfa[1][1] as i32],
    ];
    for (i, v) in cfa.iter_mut().enumerate() {
        *v = v.max(0.0)
            * [20000.0, 10000.0, 15000.0]
                [meta.cfa[(i / w as usize) % 6][(i % w as usize) % 6] as usize];
    }
    for i in 0..repeats {
        let mut pixels = cfa.clone();
        let t = Instant::now();
        aigle_rtprocess_sys::ca_correct(w, h, &mut pixels, pattern, &|| false).unwrap();
        println!(
            "serial {i}: {:?} hash {}",
            t.elapsed(),
            blake3::hash(bytemuck::cast_slice(&pixels))
        );
        let serial = pixels;
        let mut pixels = cfa.clone();
        let t = Instant::now();
        aigle_rtprocess_sys::ca_correct_parallel(w, h, &mut pixels, pattern, &|| false).unwrap();
        println!(
            "parallel {i}: {:?} hash {}",
            t.elapsed(),
            blake3::hash(bytemuck::cast_slice(&pixels))
        );
        let differences = serial
            .iter()
            .zip(&pixels)
            .filter(|(a, b)| a.to_bits() != b.to_bits())
            .count();
        assert_eq!(differences, 0, "serial/parallel pixels differ");
        if let Some(out) = std::env::var_os("CA_BENCH_DUMP") {
            std::fs::write(out, bytemuck::cast_slice(&pixels)).unwrap();
        }
    }
}
