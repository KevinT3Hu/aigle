//! Synthetic sources in linear Rec.2020, for headless development and tests.

use aigle_engine_api::SyntheticPattern;
use aigle_kernels::Rgb;
use rayon::prelude::*;

fn hue_to_rgb(h: f32) -> Rgb {
    let h6 = (h.rem_euclid(1.0)) * 6.0;
    let x = 1.0 - (h6 % 2.0 - 1.0).abs();
    match h6 as u32 {
        0 => [1.0, x, 0.0],
        1 => [x, 1.0, 0.0],
        2 => [0.0, 1.0, x],
        3 => [0.0, x, 1.0],
        4 => [x, 0.0, 1.0],
        _ => [1.0, 0.0, x],
    }
}

/// Mix a hue with grey at saturation `s`.
fn tint(h: f32, s: f32, v: f32) -> Rgb {
    let c = hue_to_rgb(h);
    c.map(|ch| v * (1.0 - s + s * ch))
}

pub fn render(pattern: SyntheticPattern, w: u32, h: u32) -> Vec<Rgb> {
    let mut out = vec![[0.0; 3]; w as usize * h as usize];
    out.par_chunks_mut(w as usize)
        .enumerate()
        .for_each(|(y, row)| {
            let fy = (y as f32 + 0.5) / h as f32;
            for (x, px) in row.iter_mut().enumerate() {
                let fx = (x as f32 + 0.5) / w as f32;
                *px = match pattern {
                    // Exposure ramp −6 … +2 EV around mid-grey; top 20 % neutral.
                    SyntheticPattern::Gradient => {
                        let v = 0.18 * 2f32.powf(fx * 8.0 - 6.0);
                        if fy < 0.2 {
                            [v; 3]
                        } else {
                            tint((fy - 0.2) / 0.8, 0.7, v)
                        }
                    }
                    // 6×4 patches: three rows of hues at decreasing saturation, one neutral row.
                    SyntheticPattern::ColorChart => {
                        let col = ((fx * 6.0) as u32).min(5);
                        let rowi = ((fy * 4.0) as u32).min(3);
                        if rowi == 3 {
                            [0.18 * 2f32.powf(col as f32 - 3.0); 3]
                        } else {
                            let sat = [0.8, 0.5, 0.25][rowi as usize];
                            tint(col as f32 / 6.0, sat, 0.25)
                        }
                    }
                };
            }
        });
    out
}
