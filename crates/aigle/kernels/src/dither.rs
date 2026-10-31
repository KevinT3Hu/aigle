//! Position-seeded dither for deterministic 8-bit quantisation (doc 02 §6.5), and integer
//! hashing shared with grain.
//!
//! Quantisation uses a 64×64 blue-noise threshold texture (void-and-cluster, generated
//! deterministically on the host by [`blue_noise`]; part of the process version, doc 02
//! §13.1). Kernels read it from the first [`BLUE_LEN`] floats of the output kernel's aux
//! buffer; each channel uses a seed-derived toroidal offset so that channels are decorrelated.

pub const BLUE_N: u32 = 64;
pub const BLUE_LEN: usize = (BLUE_N * BLUE_N) as usize;

/// 32-bit integer hash (lowbias32).
#[inline]
pub fn hash32(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^= x >> 16;
    x
}

/// Uniform white noise in [0, 1) seeded from position and a seed.
#[inline]
pub fn noise(x: u32, y: u32, seed: u32) -> f32 {
    let h = hash32(x ^ hash32(y ^ hash32(seed)));
    (h >> 8) as f32 * (1.0 / 16_777_216.0)
}

/// Blue-noise threshold in [0, 1) at grid position (x, y) for channel `c`.
#[inline]
pub fn blue(tex: &[f32], x: u32, y: u32, seed: u32, c: u32) -> f32 {
    let h = hash32(seed ^ (0x9e37_79b9u32.wrapping_mul(c + 1)));
    let ox = h & (BLUE_N - 1);
    let oy = (h >> 8) & (BLUE_N - 1);
    let i = ((y + oy) & (BLUE_N - 1)) * BLUE_N + ((x + ox) & (BLUE_N - 1));
    tex[i as usize]
}

/// Quantise an encoded value in [0, 1] to 8 bits (0..=255) with threshold `d` in [0, 1).
/// Returns `u32`: 8-bit integers need `OpCapability Int8`, which not every driver has.
#[inline]
pub fn quantize8(v: f32, d: f32) -> u32 {
    // Clamp before converting: float→int of out-of-range values is undefined in SPIR-V.
    // `max` first so NaN maps to 0.
    let q = (v * 255.0 + d).max(0.0).min(255.0);
    q as u32
}

/// 64×64 void-and-cluster blue noise (Ulichney 1993), thresholds `(rank + 0.5) / 4096`.
/// Deterministic: fixed initial pattern from [`hash32`], ties broken by lowest index.
#[cfg(not(target_arch = "spirv"))]
pub fn blue_noise() -> [f32; BLUE_LEN] {
    const N: usize = BLUE_N as usize;
    const SIGMA: f32 = 1.5;
    // Toroidal Gaussian energy kernel, indexed by wrapped offset.
    let mut kernel = [0f32; BLUE_LEN];
    for dy in 0..N {
        for dx in 0..N {
            let wx = dx.min(N - dx) as f32;
            let wy = dy.min(N - dy) as f32;
            kernel[dy * N + dx] = libm::expf(-(wx * wx + wy * wy) / (2.0 * SIGMA * SIGMA));
        }
    }
    fn update(energy: &mut [f32; BLUE_LEN], kernel: &[f32; BLUE_LEN], p: usize, sign: f32) {
        let (px, py) = (p % N, p / N);
        for y in 0..N {
            for x in 0..N {
                let k = kernel[((y + N - py) % N) * N + (x + N - px) % N];
                energy[y * N + x] += sign * k;
            }
        }
    }
    let argbest = |energy: &[f32; BLUE_LEN], pat: &[bool; BLUE_LEN], ones: bool, max: bool| {
        let mut best = usize::MAX;
        let mut bv = if max { f32::MIN } else { f32::MAX };
        for i in 0..BLUE_LEN {
            if pat[i] == ones && ((max && energy[i] > bv) || (!max && energy[i] < bv)) {
                bv = energy[i];
                best = i;
            }
        }
        best
    };

    // Initial pattern: ~10 % ones.
    let mut pat = [false; BLUE_LEN];
    let mut energy = [0f32; BLUE_LEN];
    let mut ones = 0;
    for (i, p) in pat.iter_mut().enumerate() {
        if hash32(i as u32 ^ 0x51ed_270b).is_multiple_of(10) {
            *p = true;
            ones += 1;
        }
    }
    for (i, &on) in pat.iter().enumerate() {
        if on {
            update(&mut energy, &kernel, i, 1.0);
        }
    }
    // Relax to a prototype binary pattern.
    for _ in 0..BLUE_LEN {
        let c = argbest(&energy, &pat, true, true);
        pat[c] = false;
        update(&mut energy, &kernel, c, -1.0);
        let v = argbest(&energy, &pat, false, false);
        pat[v] = true;
        update(&mut energy, &kernel, v, 1.0);
        if v == c {
            break;
        }
    }
    let mut rank = [0u32; BLUE_LEN];
    // Phase 1: rank the prototype's ones by removing the tightest cluster.
    {
        let (mut p, mut e) = (pat, energy);
        for r in (0..ones).rev() {
            let c = argbest(&e, &p, true, true);
            p[c] = false;
            update(&mut e, &kernel, c, -1.0);
            rank[c] = r as u32;
        }
    }
    // Phase 2: fill the largest voids.
    for r in ones..BLUE_LEN {
        let v = argbest(&energy, &pat, false, false);
        pat[v] = true;
        update(&mut energy, &kernel, v, 1.0);
        rank[v] = r as u32;
    }
    let mut out = [0f32; BLUE_LEN];
    for i in 0..BLUE_LEN {
        out[i] = (rank[i] as f32 + 0.5) / BLUE_LEN as f32;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blue_noise_is_a_permutation_with_little_low_frequency_energy() {
        let t = blue_noise();
        let mut ranks: Vec<u32> = t
            .iter()
            .map(|v| (v * BLUE_LEN as f32 - 0.5).round() as u32)
            .collect();
        ranks.sort_unstable();
        assert!(ranks.iter().enumerate().all(|(i, &r)| r == i as u32));
        // Mean of 4×4 blocks varies far less than for white noise (σ_white/4 ≈ 0.072).
        let n = BLUE_N as usize;
        let mut means = Vec::new();
        for by in 0..n / 4 {
            for bx in 0..n / 4 {
                let m: f32 = (0..16)
                    .map(|k| t[(by * 4 + k / 4) * n + bx * 4 + k % 4])
                    .sum::<f32>()
                    / 16.0;
                means.push(m);
            }
        }
        let mu = means.iter().sum::<f32>() / means.len() as f32;
        let sd = (means.iter().map(|m| (m - mu).powi(2)).sum::<f32>() / means.len() as f32).sqrt();
        assert!(sd < 0.03, "block sd {sd}");
        assert_eq!(blue_noise(), t, "deterministic");
    }

    /// rustc 1.95–1.98 (LLVM) vectorised this loop with wrong loads; rust-toolchain.toml pins
    /// a fixed compiler. Must run with optimisations (`cargo test --release`) to be meaningful.
    #[test]
    fn wrapped_index_loop_is_compiled_correctly() {
        #[inline(never)]
        fn wrapped_sum(t: &[u32], y: u32) -> u32 {
            let mut a = 0u32;
            for x in 0..64u32 {
                a += t[(((y + 47) & 63) * 64 + ((x + 6) & 63)) as usize];
            }
            a
        }
        let t: Vec<u32> = (0..4096).collect();
        let y = std::hint::black_box(53u32);
        let want: u32 = (0..64u32)
            .map(|x| t[std::hint::black_box((((y + 47) & 63) * 64 + ((x + 6) & 63)) as usize)])
            .sum();
        assert_eq!(
            wrapped_sum(&t, y),
            want,
            "miscompiled vectorised loop: use rustc ≥ 1.99"
        );
    }

    #[test]
    fn quantize_preserves_extremes_and_mean() {
        let t = blue_noise();
        assert_eq!(quantize8(0.0, 0.99), 0);
        assert_eq!(quantize8(1.0, 0.0), 255);
        assert_eq!(quantize8(0.5, 0.5), 128);
        let v = 100.3 / 255.0;
        let mut sum = 0u32;
        for y in 0..64 {
            for x in 0..64 {
                sum += quantize8(v, blue(&t, x, y, 7, 0));
            }
        }
        let mean = sum as f32 / 4096.0;
        assert!((mean - 100.3).abs() < 0.01, "{mean}");
    }
}
