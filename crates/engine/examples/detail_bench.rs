//! S-tier CPU benchmark: cargo run --release -p aigle-engine --example detail_bench
//! Optional arguments: case substring, iteration count. Fixed four-worker pool.
use aigle_core::EditParams;
use aigle_engine::{
    detail::{self, SParams},
    noise::NoiseLevel,
};
use aigle_kernels::{Rgb, detail::*, resample::Rgb2D};
use rayon::prelude::*;
use std::{hint::black_box, time::Instant};
struct Image(Vec<Rgb>);
impl Rgb2D for Image {
    fn width(&self) -> u32 {
        512
    }
    fn height(&self) -> u32 {
        512
    }
    fn load(&self, x: u32, y: u32) -> Rgb {
        self.0[(y * 512 + x) as usize]
    }
}
// Keep the pre-optimization executor here to make before/after measurements reproducible.
fn scalar_reference<R: Rgb2D + Sync>(plan: &detail::SPlan, src: &R) -> Vec<Rgb> {
    let n = plan.plane_len();
    let mut work = vec![0.0; plan.work_len()];
    for p in &plan.passes {
        let (gw, gh) = (p.up[U_GW] as usize, p.up[U_GH] as usize);
        let res: Vec<[f32; 4]> = (0..gw * gh)
            .into_par_iter()
            .map(|i| s_px(&work, src, &p.fp, &p.up, (i % gw) as u32, (i / gw) as u32))
            .collect();
        if p.up[U_OP] == OP_REBUILD {
            let o = p.up[U_OUT] as usize * n;
            work[o..o + 3 * gw * gh]
                .par_chunks_mut(3)
                .zip(&res)
                .for_each(|(d, v)| d.copy_from_slice(&v[..3]));
        } else {
            for k in 0..4 {
                let pl = p.up[U_OUT + k];
                if pl != NONE {
                    let o = pl as usize * n;
                    work[o..o + n]
                        .par_iter_mut()
                        .zip(&res)
                        .for_each(|(d, v)| *d = v[k]);
                }
            }
        }
    }
    let (o, len) = plan.out_range();
    work[o..o + len].as_chunks::<3>().0.to_vec()
}
fn main() {
    let filter = std::env::args().nth(1).unwrap_or_default();
    let count = std::env::args()
        .nth(2)
        .map(|s| s.parse::<usize>().unwrap())
        .unwrap_or(10);
    let src = Image(
        (0..512 * 512)
            .map(|i| {
                let x = (i % 512) as f32 / 512.0;
                let y = (i / 512) as f32 / 512.0;
                [0.05 + 0.8 * x, 0.03 + 0.7 * y, 0.04 + 0.6 * x * y]
            })
            .collect(),
    );
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(4)
        .build()
        .unwrap();
    for name in ["chroma", "combined"] {
        if !name.contains(&filter) {
            continue;
        }
        let mut edit = EditParams::default();
        edit.detail.noise.chroma = 80.0;
        edit.detail.sharpen.amount = if name == "combined" { 60.0 } else { 0.0 };
        edit.detail.noise.luma = if name == "combined" { 50.0 } else { 0.0 };
        edit.presence.texture = if name == "combined" { 20.0 } else { 0.0 };
        let plan = detail::plan(
            512,
            512,
            0,
            &SParams::of(&edit),
            NoiseLevel {
                sigma: 0.01,
                offset: 1e-3,
            },
            128,
            128,
            256,
            256,
            None,
        );
        let mut times = [Vec::new(), Vec::new()];
        let mut hash = 0;
        assert!(count > 0);
        for iteration in 0..=count {
            let mut outputs = [Vec::new(), Vec::new()];
            // Alternate order to limit systematic cache/temperature bias.
            for j in 0..2 {
                let which = (iteration + j) % 2;
                let start = Instant::now();
                outputs[which] = pool.install(|| {
                    if which == 0 {
                        scalar_reference(black_box(&plan), black_box(&src))
                    } else {
                        detail::run_cpu(black_box(&plan), black_box(&src))
                    }
                });
                times[which].push(start.elapsed().as_secs_f64() * 1000.0);
            }
            assert_eq!(outputs[0].len(), outputs[1].len());
            assert!(
                outputs[0]
                    .iter()
                    .flatten()
                    .zip(outputs[1].iter().flatten())
                    .all(|(a, b)| a.to_bits() == b.to_bits()),
                "{name}: output changed"
            );
            hash = outputs[1]
                .iter()
                .flatten()
                .fold(0xcbf29ce484222325u64, |h, v| {
                    (h ^ v.to_bits() as u64).wrapping_mul(0x100000001b3)
                });
            black_box(&outputs);
        }
        for t in &mut times {
            t.remove(0);
            t.sort_by(f64::total_cmp);
        }
        let before = times[0][count / 2];
        let after = times[1][count / 2];
        println!(
            "{name:12} {before:8.3} -> {after:8.3} ms/block ({:.2}x), passes={}, hash={hash:016x}",
            before / after,
            plan.passes.len()
        );
    }
}
