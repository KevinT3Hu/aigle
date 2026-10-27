//! Display ICC profiles (doc 01 §4): reading the matrix/TRC model of an RGB display profile,
//! the model E1 encodes to. LUT-based (`A2B0` only) and grey profiles are rejected; profiles
//! that carry LUTs *and* matrix/TRC tags (common for calibration software) use the latter.

use aigle_core::color::{self, D65_XY, Mat3, bradford, xy_to_xyz};
use aigle_kernels::math::TABLE_N as TRC_N;

#[derive(Clone, Debug, PartialEq)]
pub struct DisplayIcc {
    /// Profile description (`desc`), or empty.
    pub name: String,
    /// Linear display RGB → XYZ, white adapted to D65 (relative colorimetric).
    pub rgb_to_xyz: Mat3,
    /// Encoded → linear per channel.
    pub trc: [Curve; 3],
}

#[derive(Clone, Debug, PartialEq)]
pub enum Curve {
    Identity,
    Gamma(f64),
    /// Uniformly sampled table on [0, 1].
    Table(Vec<f64>),
    /// ICC `para` function type and its parameters `g, a, b, c, d, e, f`.
    Para(u16, [f64; 7]),
}

impl Curve {
    /// Encoded → linear.
    pub fn eval(&self, x: f64) -> f64 {
        let x = x.clamp(0.0, 1.0);
        match self {
            Curve::Identity => x,
            Curve::Gamma(g) => x.powf(*g),
            Curve::Table(t) => {
                let f = x * (t.len() - 1) as f64;
                let i = (f as usize).min(t.len() - 2);
                t[i] + (t[i + 1] - t[i]) * (f - i as f64)
            }
            Curve::Para(kind, p) => {
                let [g, a, b, c, d, e, f] = *p;
                let pw = |v: f64| if v > 0.0 { v.powf(g) } else { 0.0 };
                match kind {
                    0 => pw(x),
                    1 => {
                        if x >= -b / a {
                            pw(a * x + b)
                        } else {
                            0.0
                        }
                    }
                    2 => {
                        if x >= -b / a {
                            pw(a * x + b) + c
                        } else {
                            c
                        }
                    }
                    3 => {
                        if x >= d {
                            pw(a * x + b)
                        } else {
                            c * x
                        }
                    }
                    _ => {
                        if x >= d {
                            pw(a * x + b) + e
                        } else {
                            c * x + f
                        }
                    }
                }
            }
        }
    }

    /// Linear → encoded, by bisection on the (made monotone) curve.
    pub fn invert(&self, y: f64) -> f64 {
        if let Curve::Identity = self {
            return y.clamp(0.0, 1.0);
        }
        if let Curve::Gamma(g) = self {
            return y.clamp(0.0, 1.0).powf(1.0 / g);
        }
        let (mut lo, mut hi) = (0.0f64, 1.0f64);
        for _ in 0..40 {
            let mid = 0.5 * (lo + hi);
            if self.eval(mid) < y {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        0.5 * (lo + hi)
    }
}

impl DisplayIcc {
    /// Linear → encoded tables (`math::TRANSFER_TABLE`), `3 × TABLE_N`.
    pub fn inverse_trc(&self) -> Vec<f32> {
        let mut out = Vec::with_capacity(3 * TRC_N);
        for c in &self.trc {
            for i in 0..TRC_N {
                let u = i as f64 / (TRC_N - 1) as f64;
                out.push(c.invert(u * u) as f32);
            }
        }
        out
    }
}

fn be32(b: &[u8], at: usize) -> Result<u32, String> {
    b.get(at..at + 4)
        .map(|s| u32::from_be_bytes(s.try_into().expect("4 bytes")))
        .ok_or_else(|| "truncated ICC profile".to_string())
}

fn be16(b: &[u8], at: usize) -> Result<u16, String> {
    b.get(at..at + 2)
        .map(|s| u16::from_be_bytes(s.try_into().expect("2 bytes")))
        .ok_or_else(|| "truncated ICC profile".to_string())
}

fn s15f16(b: &[u8], at: usize) -> Result<f64, String> {
    Ok(be32(b, at)? as i32 as f64 / 65536.0)
}

/// A tag signature and its body.
type Tag<'a> = ([u8; 4], &'a [u8]);

fn tags(b: &[u8]) -> Result<Vec<Tag<'_>>, String> {
    let n = be32(b, 128)? as usize;
    if n > 1000 {
        return Err("invalid ICC tag table".into());
    }
    (0..n)
        .map(|i| {
            let at = 132 + 12 * i;
            let sig: [u8; 4] = b
                .get(at..at + 4)
                .ok_or("truncated ICC profile")?
                .try_into()
                .expect("4 bytes");
            let (off, len) = (be32(b, at + 4)? as usize, be32(b, at + 8)? as usize);
            let body = b
                .get(off..off.saturating_add(len))
                .ok_or("ICC tag outside the profile")?;
            Ok((sig, body))
        })
        .collect()
}

fn xyz(t: &[u8]) -> Result<[f64; 3], String> {
    if t.get(..4) != Some(b"XYZ ") {
        return Err("colorant tag is not XYZ".into());
    }
    Ok([s15f16(t, 8)?, s15f16(t, 12)?, s15f16(t, 16)?])
}

fn curve(t: &[u8]) -> Result<Curve, String> {
    match t.get(..4) {
        Some(b"curv") => {
            let n = be32(t, 8)? as usize;
            match n {
                0 => Ok(Curve::Identity),
                1 => Ok(Curve::Gamma(be16(t, 12)? as f64 / 256.0)),
                _ if n > 1 << 16 => Err("ICC curve too long".into()),
                _ => {
                    let mut v = (0..n)
                        .map(|i| Ok(be16(t, 12 + 2 * i)? as f64 / 65535.0))
                        .collect::<Result<Vec<_>, String>>()?;
                    let mut m = 0.0f64;
                    for x in &mut v {
                        m = m.max(*x);
                        *x = m;
                    }
                    Ok(Curve::Table(v))
                }
            }
        }
        Some(b"para") => {
            let kind = be16(t, 8)?;
            let count = match kind {
                0 => 1,
                1 => 3,
                2 => 4,
                3 => 5,
                4 => 7,
                _ => return Err(format!("unknown ICC parametric curve type {kind}")),
            };
            let mut p = [0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0];
            for (i, v) in p.iter_mut().enumerate().take(count) {
                *v = s15f16(t, 12 + 4 * i)?;
            }
            if p[0] <= 0.0 || (kind > 0 && p[1] == 0.0) {
                return Err("degenerate ICC parametric curve".into());
            }
            Ok(Curve::Para(kind, p))
        }
        _ => Err("unsupported ICC tone curve".into()),
    }
}

fn description(t: &[u8]) -> Option<String> {
    match t.get(..4)? {
        b"desc" => {
            let n = be32(t, 8).ok()? as usize;
            let s = t.get(12..12 + n.saturating_sub(1))?;
            Some(
                String::from_utf8_lossy(s)
                    .trim_end_matches('\0')
                    .to_string(),
            )
        }
        b"mluc" => {
            let (len, off) = (be32(t, 20).ok()? as usize, be32(t, 24).ok()? as usize);
            let s = t.get(off..off + len)?;
            let u: Vec<u16> = s
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| u16::from_be_bytes(*c))
                .collect();
            Some(String::from_utf16_lossy(&u))
        }
        _ => None,
    }
}

pub fn parse(b: &[u8]) -> Result<DisplayIcc, String> {
    if b.len() < 132 || b.get(36..40) != Some(b"acsp") {
        return Err("not an ICC profile".into());
    }
    if b.get(16..20) != Some(b"RGB ") {
        return Err("only RGB display profiles are supported".into());
    }
    let pcs = b.get(20..24);
    if pcs != Some(b"XYZ ") && pcs != Some(b"Lab ") {
        return Err("unsupported ICC connection space".into());
    }
    let tags = tags(b)?;
    let find = |sig: &[u8; 4]| tags.iter().find(|(s, _)| s == sig).map(|(_, t)| *t);
    let need = |sig: &[u8; 4]| {
        find(sig).ok_or_else(|| {
            "only matrix/TRC display profiles are supported (no colorant or curve tags)".to_string()
        })
    };
    let cols = [
        xyz(need(b"rXYZ")?)?,
        xyz(need(b"gXYZ")?)?,
        xyz(need(b"bXYZ")?)?,
    ];
    let trc = [
        curve(need(b"rTRC")?)?,
        curve(need(b"gTRC")?)?,
        curve(need(b"bTRC")?)?,
    ];
    let pcs_m: Mat3 = std::array::from_fn(|i| std::array::from_fn(|j| cols[j][i]));
    let white = color::mul_vec(&pcs_m, [1.0; 3]);
    if white[1] <= 0.0 || color::invert(&pcs_m).is_none() {
        return Err("degenerate ICC colorants".into());
    }
    let rgb_to_xyz = color::mul(&bradford(white, xy_to_xyz(D65_XY.0, D65_XY.1)), &pcs_m);
    Ok(DisplayIcc {
        name: find(b"desc").and_then(description).unwrap_or_default(),
        rgb_to_xyz,
        trc,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use aigle_engine_api::DisplayProfileId;

    #[test]
    fn reads_back_the_generated_profiles() {
        for d in [
            DisplayProfileId::AdobeRgb,
            DisplayProfileId::ProPhoto,
            DisplayProfileId::Rec2020,
        ] {
            let p = parse(&crate::output_space::icc_profile(&d)).unwrap();
            let m = crate::output_space::rgb_to_xyz(&d);
            for (got, want) in p.rgb_to_xyz.iter().flatten().zip(m.iter().flatten()) {
                assert!((got - want).abs() < 2e-4, "{d:?} {p:?}");
            }
            let t = crate::output_space::transfer(&d);
            for k in 1..=20 {
                let v = k as f64 / 20.0;
                let want = aigle_kernels::math::output_oetf(v as f32, t) as f64;
                assert!((p.trc[1].invert(v) - want).abs() < 1e-4, "{d:?} {v}");
            }
            assert!(!p.name.is_empty());
        }
    }

    #[test]
    fn rejects_non_profiles() {
        assert!(parse(b"hello").is_err());
        let mut b = crate::output_space::icc_profile(&DisplayProfileId::AdobeRgb);
        b[16..20].copy_from_slice(b"GRAY");
        assert!(parse(&b).is_err());
    }

    #[test]
    fn tables_and_gamma_curves() {
        let t = Curve::Table((0..256).map(|i| (i as f64 / 255.0).powf(2.2)).collect());
        let g = Curve::Gamma(2.2);
        for k in 0..=10 {
            let y = k as f64 / 10.0;
            assert!((t.invert(y) - g.invert(y)).abs() < 2e-3, "{y}");
        }
    }
}
