//! User look LUTs (`profile.look`, doc 02 §6.4): Adobe / Resolve `.cube` files.
//!
//! A 3D LUT is applied with tetrahedral interpolation (`aigle_kernels::luts::cube`) at the
//! start of D2, through an sRGB shaper on sRGB primaries: the convention of creative `.cube`
//! looks, which carry no colour-space metadata. A 1D LUT is expanded to a 33³ cube.

use aigle_core::color;

/// A parsed `.cube`: `n³` RGB entries, red fastest.
#[derive(Clone, Debug, PartialEq)]
pub struct CubeLut {
    pub title: String,
    pub n: u32,
    pub domain_min: [f32; 3],
    pub domain_max: [f32; 3],
    pub data: Vec<f32>,
}

const EXPANDED_1D: u32 = 33;

pub fn parse(text: &str) -> Result<CubeLut, String> {
    let mut title = String::new();
    let (mut n3, mut n1) = (0u32, 0u32);
    let (mut dmin, mut dmax) = ([0f32; 3], [1f32; 3]);
    let mut vals: Vec<f32> = Vec::new();
    let three = |rest: &[&str], line: usize| -> Result<[f32; 3], String> {
        let v: Vec<f32> = rest
            .iter()
            .map(|s| s.parse::<f32>())
            .collect::<Result<_, _>>()
            .map_err(|e| format!("line {line}: {e}"))?;
        match v.as_slice() {
            [a, b, c] => Ok([*a, *b, *c]),
            [a, b] => Ok([*a, *b, 0.0]),
            _ => Err(format!("line {line}: expected 3 numbers")),
        }
    };
    for (i, raw) in text.lines().enumerate() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let words: Vec<&str> = line.split_whitespace().collect();
        let first = words[0];
        if first.starts_with(|c: char| c.is_ascii_digit() || c == '-' || c == '+' || c == '.') {
            let v = three(&words, i + 1)?;
            vals.extend_from_slice(&v);
            continue;
        }
        match first {
            "TITLE" => title = line["TITLE".len()..].trim().trim_matches('"').to_string(),
            "LUT_3D_SIZE" => {
                n3 = words
                    .get(1)
                    .and_then(|s| s.parse().ok())
                    .ok_or(format!("line {}: bad LUT_3D_SIZE", i + 1))?
            }
            "LUT_1D_SIZE" => {
                n1 = words
                    .get(1)
                    .and_then(|s| s.parse().ok())
                    .ok_or(format!("line {}: bad LUT_1D_SIZE", i + 1))?
            }
            "DOMAIN_MIN" => dmin = three(&words[1..], i + 1)?,
            "DOMAIN_MAX" => dmax = three(&words[1..], i + 1)?,
            "LUT_3D_INPUT_RANGE" | "LUT_1D_INPUT_RANGE" => {
                let v = three(&words[1..], i + 1)?;
                dmin = [v[0]; 3];
                dmax = [v[1]; 3];
            }
            // Unknown keywords (e.g. LUT_IN_VIDEO_RANGE) are ignored.
            _ => {}
        }
    }
    if (0..3).any(|c| dmax[c] <= dmin[c]) {
        return Err("DOMAIN_MAX must exceed DOMAIN_MIN".into());
    }
    if vals.iter().any(|v| !v.is_finite()) {
        return Err("non-finite LUT entry".into());
    }
    if n3 > 0 {
        if !(2..=256).contains(&n3) {
            return Err(format!("LUT_3D_SIZE {n3} out of range"));
        }
        let want = (n3 as usize).pow(3) * 3;
        if vals.len() != want {
            return Err(format!(
                "expected {} entries, found {}",
                want / 3,
                vals.len() / 3
            ));
        }
        return Ok(CubeLut {
            title,
            n: n3,
            domain_min: dmin,
            domain_max: dmax,
            data: vals,
        });
    }
    if n1 > 0 {
        if !(2..=65536).contains(&n1) || vals.len() != n1 as usize * 3 {
            return Err(format!(
                "1D LUT: expected {n1} entries, found {}",
                vals.len() / 3
            ));
        }
        // Expand: per-channel curve, linear interpolation, on a 33³ grid over the domain.
        let curve = |c: usize, x: f32| {
            let t = ((x - dmin[c]) / (dmax[c] - dmin[c])).clamp(0.0, 1.0) * (n1 - 1) as f32;
            let i = (t as usize).min(n1 as usize - 2);
            let f = t - i as f32;
            vals[i * 3 + c] * (1.0 - f) + vals[(i + 1) * 3 + c] * f
        };
        let n = EXPANDED_1D;
        let mut data = Vec::with_capacity((n as usize).pow(3) * 3);
        let at = |c: usize, i: u32| dmin[c] + (dmax[c] - dmin[c]) * i as f32 / (n - 1) as f32;
        for b in 0..n {
            for g in 0..n {
                for r in 0..n {
                    data.extend_from_slice(&[
                        curve(0, at(0, r)),
                        curve(1, at(1, g)),
                        curve(2, at(2, b)),
                    ]);
                }
            }
        }
        return Ok(CubeLut {
            title,
            n,
            domain_min: dmin,
            domain_max: dmax,
            data,
        });
    }
    Err("no LUT_3D_SIZE or LUT_1D_SIZE".into())
}

impl CubeLut {
    /// Kernel layout (`aigle_kernels::luts::CUBE_HEADER`).
    pub fn pack(&self, out: &mut Vec<f32>) {
        out.push(self.n as f32);
        out.extend_from_slice(&self.domain_min);
        out.extend_from_slice(&self.domain_max);
        let to_srgb = color::mul(
            &color::invert(&color::SRGB_TO_XYZ).expect("invertible"),
            &color::REC2020_TO_XYZ,
        );
        let from_srgb = color::invert(&to_srgb).expect("invertible");
        for m in [to_srgb, from_srgb] {
            out.extend(color::to_f32(&m).iter().flatten());
        }
        out.extend_from_slice(&self.data);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_3d_and_1d() {
        let mut s = String::from(
            "# comment\nTITLE \"Warm\"\nLUT_3D_SIZE 2\nDOMAIN_MIN 0 0 0\nDOMAIN_MAX 1 1 1\n",
        );
        for b in 0..2 {
            for g in 0..2 {
                for r in 0..2 {
                    s += &format!("{r} {g} {b}\n");
                }
            }
        }
        let l = parse(&s).unwrap();
        assert_eq!((l.title.as_str(), l.n, l.data.len()), ("Warm", 2, 24));
        assert_eq!(&l.data[3..6], &[1.0, 0.0, 0.0]);
        assert!(parse("LUT_3D_SIZE 2\n0 0 0\n").is_err());

        let l = parse("LUT_1D_SIZE 2\n0 0 0\n1 0.5 1\n").unwrap();
        assert_eq!(l.n, 33);
        // Corner (r=1, g=1, b=0) → (1, 0.5, 0).
        let i = (32 * 33 + 32) * 3;
        assert_eq!(&l.data[i..i + 3], &[1.0, 0.5, 0.0]);
    }
}
