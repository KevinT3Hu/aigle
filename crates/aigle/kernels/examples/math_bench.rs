//! CPU kernel benchmark: cargo run --release -p aigle-kernels --example math_bench
//! Optional arguments: case substring, iteration multiplier. Prints median of five runs.
use aigle_kernels::{
    Rgb,
    detail::*,
    masks, math,
    resample::{self, Rgb2D},
};
use std::{hint::black_box, time::Instant};

struct Image(Vec<Rgb>);
impl Rgb2D for Image {
    fn width(&self) -> u32 {
        256
    }
    fn height(&self) -> u32 {
        256
    }
    fn load(&self, x: u32, y: u32) -> Rgb {
        self.0[(y * 256 + x) as usize]
    }
}
fn bench(name: &str, n: usize, mut f: impl FnMut(usize) -> f32) {
    if !name.contains(&std::env::args().nth(1).unwrap_or_default()) {
        return;
    }
    let multiplier = std::env::args()
        .nth(2)
        .map(|s| s.parse::<usize>().unwrap())
        .unwrap_or(1);
    assert!(multiplier > 0);
    let n = n * multiplier;
    let mut times = Vec::new();
    let mut checksum = 0.0;
    for _ in 0..6 {
        let start = Instant::now();
        let mut sum = 0.0;
        for i in 0..n {
            sum += black_box(f(black_box(i)));
        }
        times.push(start.elapsed().as_secs_f64() * 1e9 / n as f64);
        checksum = black_box(sum);
    }
    times.remove(0);
    times.sort_by(f64::total_cmp);
    println!(
        "{name:24} {:10.2} ns/pixel checksum={checksum:.9}",
        times[2]
    );
}
fn program(ops: &[f32]) -> Vec<f32> {
    let mut p = vec![
        1.0,
        1.0,
        256.0,
        256.0,
        1.0 / 256.0,
        0.0,
        0.0,
        0.0,
        1.0 / 256.0,
        0.0,
    ];
    for _ in 0..4 {
        p.extend_from_slice(&[1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]);
    }
    p.extend_from_slice(&[1.0, 0.0, ops.len() as f32, 1.0]);
    p.extend_from_slice(ops);
    p
}
fn main() {
    let image = Image(
        (0..65536)
            .map(|i| {
                let x = (i % 251) as f32 / 250.0;
                [x, x * 0.7, x * 0.3]
            })
            .collect(),
    );
    let m = black_box([[1.1, -0.1, 0.0], [-0.05, 1.1, -0.05], [0.0, -0.1, 1.1]]);
    bench("matrix", 2_000_000, |i| {
        black_box(math::mat3_mul(&m, black_box(image.0[i & 65535])))[0]
    });
    for (name, ops) in [
        (
            "mask_radial",
            vec![
                masks::OP_RADIAL as f32,
                128.0,
                128.0,
                100.0,
                80.0,
                1.0,
                0.0,
                0.5,
            ],
        ),
        ("mask_luminance", vec![masks::OP_LUM as f32, 0.2, 0.8, 0.1]),
        (
            "mask_color",
            vec![masks::OP_COLOR as f32, 1.0, 0.1, 0.4, 0.7, 0.5, 0.3],
        ),
    ] {
        let p = black_box(program(&ops));
        bench(name, 300_000, |i| {
            masks::eval(
                &p,
                0,
                (i % 256) as f32,
                ((i / 256) % 256) as f32,
                image.0[i & 65535],
            )[0]
        });
    }
    for k in [1.0, 2.0] {
        bench(&format!("lanczos_{k}"), 30_000, |i| {
            black_box(resample::lanczos3(
                black_box(&image),
                (i % 256) as f32 + 0.37,
                ((i / 256) % 256) as f32 + 0.23,
                black_box(k),
            ))[0]
        });
    }
    for r in [3, 16, 48] {
        let work: Vec<f32> = image.0.iter().map(|p| p[0]).collect();
        let mut up = [0; UP_LEN];
        up[U_OP] = OP_BOX_H;
        up[U_RW] = 256;
        up[U_RH] = 256;
        up[U_A] = r;
        up[U_B] = 1;
        bench(&format!("box_radius_{r}"), 300_000, |i| {
            box_px(
                black_box(&work),
                black_box(&up),
                (i % 256) as u32,
                ((i / 256) % 256) as u32,
            )[0]
        });
    }
}
