//! Test helpers shared by the integration tests.
#![allow(dead_code)]

/// A little-endian DCP: (tag, values as SRATIONAL / FLOAT / LONG / SHORT by `typ`).
pub fn build_dcp(entries: &[(u16, u16, Vec<f64>)], name: &str) -> Vec<u8> {
    let mut all: Vec<(u16, u16, u32, Vec<u8>)> = Vec::new();
    for (tag, typ, v) in entries {
        let mut b = Vec::new();
        for &x in v {
            match typ {
                3 => b.extend_from_slice(&(x as u16).to_le_bytes()),
                4 => b.extend_from_slice(&(x as u32).to_le_bytes()),
                10 => {
                    b.extend_from_slice(&((x * 10000.0).round() as i32).to_le_bytes());
                    b.extend_from_slice(&10000i32.to_le_bytes());
                }
                11 => b.extend_from_slice(&(x as f32).to_bits().to_le_bytes()),
                _ => unreachable!(),
            }
        }
        all.push((*tag, *typ, v.len() as u32, b));
    }
    let mut nb = name.as_bytes().to_vec();
    nb.push(0);
    all.push((50936, 2, nb.len() as u32, nb));
    all.sort_by_key(|e| e.0);
    let mut out = b"IIRC".to_vec();
    out.extend_from_slice(&8u32.to_le_bytes());
    out.extend_from_slice(&(all.len() as u16).to_le_bytes());
    let mut at = 8 + 2 + 12 * all.len() + 4;
    let mut data = Vec::new();
    for (tag, typ, count, b) in &all {
        out.extend_from_slice(&tag.to_le_bytes());
        out.extend_from_slice(&typ.to_le_bytes());
        out.extend_from_slice(&count.to_le_bytes());
        if b.len() <= 4 {
            let mut v = b.clone();
            v.resize(4, 0);
            out.extend_from_slice(&v);
        } else {
            out.extend_from_slice(&(at as u32).to_le_bytes());
            data.extend_from_slice(b);
            at += b.len();
        }
    }
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&data);
    out
}

/// A DCP for the synthetic (Rec.2020 "camera") source: identity colour, and a HueSatMap that
/// rotates every hue by `shift` degrees and scales saturation by `sat`.
pub fn synthetic_dcp(shift: f64, sat: f64) -> Vec<u8> {
    // XYZ → camera = XYZ → Rec.2020.
    let m = [
        [1.716_651, -0.355_671, -0.253_366],
        [-0.666_684, 1.616_481, 0.015_769],
        [0.017_640, -0.042_771, 0.942_103],
    ];
    let (hd, sd) = (6u32, 2u32);
    let hsm: Vec<f64> = (0..hd * sd).flat_map(|_| [shift, sat, 1.0]).collect();
    build_dcp(
        &[
            (50721, 10, m.iter().flatten().copied().collect()),
            (50778, 3, vec![21.0]),
            (50937, 4, vec![hd as f64, sd as f64, 1.0]),
            (50938, 11, hsm),
        ],
        "Synthetic test",
    )
}
