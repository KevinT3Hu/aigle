//! Lossy DNG (JPEG-compressed LinearRaw, as written by Adobe DNG Converter and some phones)
//! decodes through libjpeg. The files are generated here: a tiled 8-bit sRGB-encoded image,
//! which LibRaw linearises with the sRGB curve when no MapPolynomial opcode is present.

use std::path::PathBuf;

use aigle_libraw_sys::{Kind, Raw};

const W: u32 = 192;
const H: u32 = 96;
const TILE_W: u32 = 64;
const TILE_H: u32 = 48;

fn photo() -> image::RgbImage {
    image::RgbImage::from_fn(W, H, |x, y| {
        image::Rgb([
            (40 + x) as u8,
            (60 + 2 * y) as u8,
            (200 - (x + y) / 2) as u8,
        ])
    })
}

fn jpeg(img: &image::RgbImage) -> Vec<u8> {
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 98)
        .encode_image(img)
        .unwrap();
    out
}

enum Value {
    Short(Vec<u16>),
    Long(Vec<u32>),
    Byte(Vec<u8>),
    Ascii(&'static str),
    Rational(Vec<(u32, u32)>),
    SRational(Vec<(i32, i32)>),
}

/// A little-endian DNG with one IFD holding the tiles.
fn dng(tiles: &[Vec<u8>]) -> Vec<u8> {
    let tiles_x = W.div_ceil(TILE_W);
    assert_eq!(tiles.len() as u32, tiles_x * H.div_ceil(TILE_H));
    let xyz_to_srgb = [
        3.2406, -1.5372, -0.4986, -0.9689, 1.8758, 0.0415, 0.0557, -0.2040, 1.0570,
    ];
    let mut entries: Vec<(u16, Value)> = vec![
        (254, Value::Long(vec![0])),
        (256, Value::Long(vec![W])),
        (257, Value::Long(vec![H])),
        (258, Value::Short(vec![8, 8, 8])),
        (259, Value::Short(vec![34892])),
        (262, Value::Short(vec![34892])),
        (271, Value::Ascii("Aigle")),
        (272, Value::Ascii("Lossy Test")),
        (274, Value::Short(vec![1])),
        (277, Value::Short(vec![3])),
        (284, Value::Short(vec![1])),
        (322, Value::Long(vec![TILE_W])),
        (323, Value::Long(vec![TILE_H])),
        (324, Value::Long(vec![0; tiles.len()])),
        (
            325,
            Value::Long(tiles.iter().map(|t| t.len() as u32).collect()),
        ),
        (50706, Value::Byte(vec![1, 4, 0, 0])),
        (50708, Value::Ascii("Aigle Lossy Test")),
        (50717, Value::Long(vec![255])),
        (
            50721,
            Value::SRational(
                xyz_to_srgb
                    .iter()
                    .map(|v: &f64| ((v * 10000.0).round() as i32, 10000))
                    .collect(),
            ),
        ),
        (50728, Value::Rational(vec![(1, 1); 3])),
        (50778, Value::Short(vec![21])),
    ];
    entries.sort_by_key(|e| e.0);

    let ifd_len = 2 + 12 * entries.len() + 4;
    let mut extra_at = 8 + ifd_len;
    let mut ifd = (entries.len() as u16).to_le_bytes().to_vec();
    let mut extra = Vec::new();
    let mut tile_offsets_at = None;
    for (tag, v) in &entries {
        let (ty, count, bytes): (u16, usize, Vec<u8>) = match v {
            Value::Byte(b) => (1, b.len(), b.clone()),
            Value::Ascii(s) => {
                let mut b = s.as_bytes().to_vec();
                b.push(0);
                (2, b.len(), b)
            }
            Value::Short(s) => (3, s.len(), s.iter().flat_map(|x| x.to_le_bytes()).collect()),
            Value::Long(l) => (4, l.len(), l.iter().flat_map(|x| x.to_le_bytes()).collect()),
            Value::Rational(r) => (
                5,
                r.len(),
                r.iter()
                    .flat_map(|(n, d)| [n.to_le_bytes(), d.to_le_bytes()].concat())
                    .collect(),
            ),
            Value::SRational(r) => (
                10,
                r.len(),
                r.iter()
                    .flat_map(|(n, d)| [n.to_le_bytes(), d.to_le_bytes()].concat())
                    .collect(),
            ),
        };
        ifd.extend(tag.to_le_bytes());
        ifd.extend(ty.to_le_bytes());
        ifd.extend((count as u32).to_le_bytes());
        if bytes.len() <= 4 {
            let mut b = bytes;
            b.resize(4, 0);
            ifd.extend(b);
        } else {
            if *tag == 324 {
                tile_offsets_at = Some(extra.len());
            }
            ifd.extend((extra_at as u32).to_le_bytes());
            extra_at += bytes.len() + bytes.len() % 2;
            extra.extend(&bytes);
            if bytes.len() % 2 == 1 {
                extra.push(0);
            }
        }
    }
    ifd.extend(0u32.to_le_bytes());

    let mut file = b"II*\0".to_vec();
    file.extend(8u32.to_le_bytes());
    file.extend(ifd);
    let at = tile_offsets_at.expect("several tiles");
    let mut offsets = Vec::new();
    for t in tiles {
        offsets.push(extra_at as u32);
        extra_at += t.len();
    }
    for (i, o) in offsets.iter().enumerate() {
        extra[at + 4 * i..at + 4 * i + 4].copy_from_slice(&o.to_le_bytes());
    }
    file.extend(extra);
    for t in tiles {
        file.extend(t);
    }
    file
}

fn tiles(img: &image::RgbImage) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    for ty in 0..H.div_ceil(TILE_H) {
        for tx in 0..W.div_ceil(TILE_W) {
            let tile =
                image::imageops::crop_imm(img, tx * TILE_W, ty * TILE_H, TILE_W, TILE_H).to_image();
            out.push(jpeg(&tile));
        }
    }
    out
}

fn write(name: &str, bytes: &[u8]) -> PathBuf {
    let p = std::env::temp_dir().join(format!("aigle-{}-{name}", std::process::id()));
    std::fs::write(&p, bytes).unwrap();
    p
}

/// sRGB encoding on the 0–255 scale.
fn linear_to_srgb(v: f32) -> f32 {
    let v = v.clamp(0.0, 1.0);
    255.0
        * if v <= 0.0031308 {
            v * 12.92
        } else {
            1.055 * v.powf(1.0 / 2.4) - 0.055
        }
}

#[test]
fn lossy_dng_decodes() {
    let img = photo();
    let path = write("lossy.dng", &dng(&tiles(&img)));
    let mut raw = Raw::open(&path).unwrap();
    raw.unpack(&|| false).unwrap();
    let m = raw.meta().unwrap();
    assert_eq!((m.width, m.height), (W, H));
    assert_eq!(m.make, "Aigle");
    assert!(matches!(m.kind, Ok(Kind::Rgb)), "{:?}", m.kind);
    let rgb = raw.copy_rgb(W, H).unwrap();
    let _ = std::fs::remove_file(&path);
    let (mut worst, mut sum) = (0f32, 0f32);
    for (i, px) in img.pixels().enumerate() {
        for c in 0..3 {
            let d = (linear_to_srgb(rgb[3 * i + c]) - px[c] as f32).abs();
            worst = worst.max(d);
            sum += d;
        }
    }
    let mean = sum / (3 * W * H) as f32;
    assert!(
        worst < 5.0 && mean < 1.0,
        "decoded pixels differ by up to {worst:.1}/255, {mean:.2} on average"
    );
}

#[test]
fn corrupt_lossy_dng_fails() {
    let img = photo();
    let mut t = tiles(&img);
    let garbage: Vec<u8> = (0..t[4].len() as u32)
        .map(|i| (i.wrapping_mul(2654435761) >> 24) as u8)
        .collect();
    t[4] = garbage;
    let path = write("corrupt.dng", &dng(&t));
    let mut raw = Raw::open(&path).unwrap();
    assert!(raw.unpack(&|| false).is_err());
    let _ = std::fs::remove_file(&path);

    let good = tiles(&img);
    let mut truncated = dng(&good);
    truncated.truncate(truncated.len() - good[5].len() / 2);
    let path = write("truncated.dng", &truncated);
    if let Ok(mut raw) = Raw::open(&path) {
        let _ = raw.unpack(&|| false);
    }
    let _ = std::fs::remove_file(&path);
}
