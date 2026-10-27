//! Rendered-image sources (JPEG, PNG, TIFF, WebP, HEIF/AVIF, …), entered at A5 (doc 02 §15).
//!
//! Decoded by libvips: autorotated, alpha dropped, converted through the embedded ICC
//! profile (sRGB when there is none) by lcms2 to XYZ D65, then to linear Rec.2020. EXIF
//! camera and lens fields are read for lens matching and export metadata.

use std::path::Path;

use aigle_core::color;
use aigle_engine_api::{CameraInfo, CancelToken, ShotInfo};
use aigle_kernels::Rgb;
use aigle_vips_sys as vips;
use rayon::prelude::*;

use super::LoadError;

pub struct Decoded {
    pub width: u32,
    pub height: u32,
    pub exif_orientation: u16,
    /// Linear Rec.2020, oriented.
    pub rgb: Vec<Rgb>,
    pub camera: Option<CameraInfo>,
}

fn fail(path: &Path) -> impl Fn(vips::Error) -> LoadError + '_ {
    move |e| match e {
        vips::Error::Cancelled => LoadError::Cancelled,
        e => LoadError::Failed(format!("{}: {e}", path.display())),
    }
}

/// `admit` is called with the image size before decoding the pixels.
pub fn decode(
    path: &Path,
    admit: &mut dyn FnMut(u32, u32) -> Result<(), LoadError>,
    cancel: Option<&CancelToken>,
) -> Result<Decoded, LoadError> {
    let (im, h) = vips::Image::open(path).map_err(fail(path))?;
    let (w, ht) = (h.width.max(0) as u32, h.height.max(0) as u32);
    if w == 0 || ht == 0 {
        return Err(LoadError::Failed(format!(
            "{}: empty image",
            path.display()
        )));
    }
    admit(w, ht)?;
    let camera = camera_info(&im);
    let mut xyz = vec![0f32; w as usize * ht as usize * 3];
    im.decode_xyz(&h, &mut xyz, &|| cancel.is_some_and(|c| c.is_cancelled()))
        .map_err(fail(path))?;
    drop(im);
    // libvips XYZ has white at Y = 100.
    let m = color::to_f32(&color::scale(
        &color::invert(&color::REC2020_TO_XYZ).expect("invertible"),
        0.01,
    ));
    let rgb = xyz
        .par_chunks_exact(3)
        .map(|p| aigle_kernels::math::mat3_mul(&m, [p[0], p[1], p[2]]))
        .collect();
    Ok(Decoded {
        width: w,
        height: ht,
        exif_orientation: h.orientation.clamp(1, 8) as u16,
        rgb,
        camera,
    })
}

/// Value of a libvips EXIF string, `"VALUE (DESCRIPTION, TYPE, n components, m bytes)"`.
fn exif_value(s: &str) -> &str {
    let s = s.trim_end();
    if !s.ends_with(')') {
        return s.trim();
    }
    let mut depth = 0;
    for (i, c) in s.char_indices().rev() {
        match c {
            ')' => depth += 1,
            '(' => {
                depth -= 1;
                if depth == 0 {
                    return s[..i].trim();
                }
            }
            _ => {}
        }
    }
    s.trim()
}

/// A rational or decimal EXIF value.
fn exif_number(s: &str) -> Option<f32> {
    let v = exif_value(s).split_whitespace().next()?;
    match v.split_once('/') {
        Some((n, d)) => {
            let (n, d): (f64, f64) = (n.parse().ok()?, d.parse().ok()?);
            (d != 0.0).then(|| (n / d) as f32)
        }
        None => v.parse().ok(),
    }
    .filter(|v: &f32| v.is_finite() && *v > 0.0)
}

fn camera_info(im: &vips::Image) -> Option<CameraInfo> {
    let s = |name: &str| {
        im.get_string(name)
            .map(|v| exif_value(&v).to_owned())
            .filter(|v| !v.is_empty())
    };
    let n = |name: &str| im.get_string(name).and_then(|v| exif_number(&v));
    let code = |name: &str| {
        im.get_string(name)
            .and_then(|v| exif_value(&v).parse::<u16>().ok())
    };
    let make = s("exif-ifd0-Make");
    let model = s("exif-ifd0-Model");
    if make.is_none() && model.is_none() {
        return None;
    }
    Some(CameraInfo {
        make: make.unwrap_or_default(),
        model: model.unwrap_or_default(),
        iso: n("exif-ifd2-ISOSpeedRatings").or_else(|| n("exif-ifd2-PhotographicSensitivity")),
        exposure_time: n("exif-ifd2-ExposureTime"),
        f_number: n("exif-ifd2-FNumber"),
        focal_length: n("exif-ifd2-FocalLength"),
        focal_length_35mm: n("exif-ifd2-FocalLengthIn35mmFilm"),
        lens_make: s("exif-ifd2-LensMake"),
        lens_model: s("exif-ifd2-LensModel"),
        lens_focal_range: None,
        shot: ShotInfo {
            date_time: s("exif-ifd2-DateTimeOriginal")
                .or_else(|| s("exif-ifd0-DateTime"))
                .filter(|d| crate::source::raw::valid_exif_date(d)),
            utc_offset: s("exif-ifd2-OffsetTimeOriginal")
                .filter(|o| crate::source::raw::valid_utc_offset(o)),
            exposure_bias: s("exif-ifd2-ExposureBiasValue").and_then(|v| exif_signed(&v)),
            exposure_program: code("exif-ifd2-ExposureProgram"),
            metering_mode: code("exif-ifd2-MeteringMode"),
            flash: code("exif-ifd2-Flash"),
            exposure_mode: code("exif-ifd2-ExposureMode"),
            white_balance: code("exif-ifd2-WhiteBalance"),
            max_aperture: n("exif-ifd2-MaxApertureValue").map(|apex| 2f32.powf(apex / 2.0)),
            body_serial: s("exif-ifd2-BodySerialNumber"),
            lens_serial: s("exif-ifd2-LensSerialNumber"),
            firmware: None,
            artist: s("exif-ifd0-Artist"),
            copyright: s("exif-ifd0-Copyright"),
            gps: None,
        },
    })
}

/// A signed EXIF number (`-2/3`, `0.5`).
fn exif_signed(v: &str) -> Option<f32> {
    let v = v.split_whitespace().next()?;
    match v.split_once('/') {
        Some((n, d)) => {
            let (n, d): (f64, f64) = (n.parse().ok()?, d.parse().ok()?);
            (d != 0.0).then(|| (n / d) as f32)
        }
        None => v.parse().ok(),
    }
    .filter(|v: &f32| v.is_finite())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exif_strings_parse() {
        assert_eq!(
            exif_value("Canon EOS 6D (Canon EOS 6D, ASCII, 13 components, 13 bytes)"),
            "Canon EOS 6D"
        );
        assert_eq!(
            exif_number("1/200 (1/200 sec., Rational, 1 components, 8 bytes)"),
            Some(0.005)
        );
        assert_eq!(
            exif_number("28/10 (f/2.8, Rational, 1 components, 8 bytes)"),
            Some(2.8)
        );
        assert_eq!(
            exif_number("400 (400, Short, 1 components, 2 bytes)"),
            Some(400.0)
        );
        assert_eq!(exif_value("plain"), "plain");
    }

    /// HEIF/AVIF go through libvips' libheif module.
    #[test]
    fn avif_and_webp_decode() {
        let fill = |_, _, w: u32, h: u32, out: &mut [u8], stride: usize| {
            for j in 0..h as usize {
                for i in 0..w as usize {
                    out[j * stride + i * 3..][..3].copy_from_slice(&[200, 120, 40]);
                }
            }
            true
        };
        for suffix in [".avif[Q=90]", ".webp[Q=90]"] {
            let opts = vips::SaveOpts {
                width: 64,
                height: 32,
                sixteen_bit: false,
                out_size: (64, 32),
                icc: Some("srgb"),
                exif: None,
                suffix,
                sharpen: None,
            };
            let bytes = vips::save_buffer(&opts, &fill).unwrap();
            let ext = &suffix[1..5];
            let path = std::env::temp_dir().join(format!("aigle-{}.{ext}", std::process::id()));
            std::fs::write(&path, bytes).unwrap();
            let d = decode(&path, &mut |_, _| Ok(()), None).unwrap();
            std::fs::remove_file(&path).ok();
            assert_eq!((d.width, d.height), (64, 32), "{suffix}");
            let p = d.rgb[16 * 64 + 32];
            assert!(p[0] > p[1] && p[1] > p[2], "{suffix}: {p:?}");
        }
    }

    /// A Display P3 file of P3 green with EXIF decodes through its profile to the exact P3
    /// green in Rec.2020 (an sRGB reading would be far off), and its camera fields are read.
    #[test]
    fn profile_and_exif_are_read() {
        let bgra = [0u8, 255, 0, 255].repeat(16 * 16);
        let cam = CameraInfo {
            make: "Fujifilm".into(),
            model: "X-T20".into(),
            iso: Some(200.0),
            f_number: Some(5.6),
            exposure_time: Some(1.0 / 250.0),
            shot: ShotInfo {
                date_time: Some("2024:05:06 07:08:09".into()),
                artist: Some("Jane Doe".into()),
                copyright: Some("(c) Jane Doe".into()),
                exposure_bias: Some(-2.0 / 3.0),
                ..Default::default()
            },
            ..Default::default()
        };
        let exif = crate::encode::exif_for(Some(&cam));
        let bytes = crate::encode::encode(
            16,
            16,
            &bgra,
            aigle_engine_api::EncodedFormat::Jpeg { quality: 100 },
            &aigle_engine_api::DisplayProfileId::DisplayP3,
            Some(&exif),
        )
        .unwrap();
        let path = std::env::temp_dir().join(format!("aigle-p3-{}.jpg", std::process::id()));
        std::fs::write(&path, bytes).unwrap();
        let d = decode(&path, &mut |_, _| Ok(()), None).unwrap();
        std::fs::remove_file(&path).ok();
        let to2020 = color::mul(
            &color::invert(&color::REC2020_TO_XYZ).unwrap(),
            &color::DISPLAY_P3_TO_XYZ,
        );
        let want = color::mul_vec(&to2020, [0.0, 1.0, 0.0]);
        let got = d.rgb[8 * 16 + 8];
        for c in 0..3 {
            assert!(
                (got[c] as f64 - want[c]).abs() < 0.02,
                "{got:?} vs {want:?}"
            );
        }
        let c = d.camera.expect("EXIF camera");
        assert_eq!((c.make.as_str(), c.model.as_str()), ("Fujifilm", "X-T20"));
        assert_eq!(c.iso, Some(200.0));
        assert!((c.f_number.unwrap() - 5.6).abs() < 0.01);
        assert!((c.exposure_time.unwrap() - 0.004).abs() < 1e-5);
        assert_eq!(c.shot.date_time.as_deref(), Some("2024:05:06 07:08:09"));
        assert_eq!(c.shot.artist.as_deref(), Some("Jane Doe"));
        assert_eq!(c.shot.copyright.as_deref(), Some("(c) Jane Doe"));
        assert!(
            (c.shot.exposure_bias.unwrap() + 0.67).abs() < 0.01,
            "{:?}",
            c.shot
        );
    }

    #[test]
    fn signed_numbers_dates_and_offsets_parse() {
        assert_eq!(
            exif_signed("-2/3 (-0.67 EV, SRational, 1 components)"),
            Some(-2.0 / 3.0)
        );
        assert_eq!(exif_signed("0.5"), Some(0.5));
        assert_eq!(exif_signed("1/0"), None);
        use crate::source::raw::valid_exif_date;
        assert!(valid_exif_date("2024:05:06 07:08:09"));
        assert!(!valid_exif_date("0000:00:00 00:00:00"));
        assert!(!valid_exif_date("    :  :     :  :  "));
        use crate::source::raw::valid_utc_offset;
        assert!(valid_utc_offset("+08:00") && valid_utc_offset("-05:30"));
        assert!(!valid_utc_offset(":") && !valid_utc_offset("0800"));
    }
}
