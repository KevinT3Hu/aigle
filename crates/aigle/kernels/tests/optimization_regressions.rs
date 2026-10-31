//! Bitwise fingerprints captured from the scalar implementation before optimization.
use aigle_kernels::{
    Rgb, masks,
    resample::{self, Rgb2D},
};
fn hash(h: u64, v: f32) -> u64 {
    (h ^ v.to_bits() as u64).wrapping_mul(0x100000001b3)
}

struct Image {
    w: u32,
    h: u32,
}
impl Rgb2D for Image {
    fn width(&self) -> u32 {
        self.w
    }
    fn height(&self) -> u32 {
        self.h
    }
    fn load(&self, x: u32, y: u32) -> Rgb {
        let v = x.wrapping_mul(1664525) ^ y.wrapping_mul(1013904223);
        let f = (v >> 8) as f32 / 16777216.0;
        [f, f * f, 1.0 - f]
    }
}
#[test]
fn lanczos_boundary_and_footprint_bits() {
    let mut actual = 0xcbf29ce484222325;
    for (w, h) in [(1, 1), (2, 3), (17, 11), (64, 37)] {
        let src = Image { w, h };
        for k in [1.0, 1.125, 1.5, 2.0] {
            for i in 0..128 {
                let u = (i * 17 % (w + 16)) as f32 - 8.0 + 0.37;
                let v = (i * 13 % (h + 16)) as f32 - 8.0 + 0.23;
                let a = resample::lanczos3(&src, u, v, k);
                for c in a {
                    actual = hash(actual, c);
                }
            }
        }
    }
    assert_eq!(actual, 0x23a2c5cab1e3a421);
}
#[test]
fn mixed_mask_and_overlay_bits() {
    let programs = [
        vec![
            masks::OP_RADIAL as f32,
            50.0,
            50.0,
            30.0,
            20.0,
            0.8,
            0.6,
            0.3,
        ],
        vec![masks::OP_LUM as f32, 0.2, 0.8, 0.15],
        vec![
            masks::OP_COLOR as f32,
            2.0,
            0.15,
            1.0,
            0.6,
            0.3,
            0.2,
            0.9,
            0.5,
            0.1,
        ],
        vec![
            masks::OP_LUM as f32,
            0.0,
            0.6,
            0.1,
            masks::OP_LINEAR as f32,
            20.0,
            30.0,
            0.01,
            0.02,
            masks::OP_MUL as f32,
        ],
    ];
    let mut actual = 0xcbf29ce484222325;
    // A nonzero header offset also exercises lazy matrix reads in packed buffers.
    for base in [0, 7] {
        let mut p = vec![0.0; base];
        p.extend_from_slice(&[4.0, 2.0, 100.0, 100.0, 0.01, 0.0, 0.0, 0.0, 0.01, 0.0]);
        for _ in 0..4 {
            p.extend_from_slice(&[1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]);
        }
        for (i, ops) in programs.iter().enumerate() {
            p.extend_from_slice(&[0.7, (i % 2) as f32, ops.len() as f32, 1.0, -2.0]);
            p.extend_from_slice(ops);
        }
        for i in 0..2048 {
            let rgb = [
                (i % 251) as f32 / 200.0 - 0.1,
                (i % 107) as f32 / 100.0,
                (i % 53) as f32 / 50.0,
            ];
            let u = (i % 100) as f32 + 0.25;
            let v = (i % 71) as f32 + 0.75;
            let a = masks::eval(&p, base, u, v, rgb);
            for c in a {
                actual = hash(actual, c);
            }
            for index in 0..=4 {
                let a = masks::alpha_of(&p, base, index, u, v, rgb);
                actual = hash(actual, a);
            }
        }
    }
    assert_eq!(actual, 0xad8be89ce05aaa55);
}
