//! Raw A tier (doc 02 §3): A1 decode (LibRaw) → A2 raw prep → A3 demosaic (librtprocess)
//! → oriented camera RGB with the reference WB premultiplied, plus the per-channel clip mask
//! A4 (`super::highlights`) works from.
//!
//! Data conventions:
//! * A2 output: black-subtracted CFA, 1.0 = white level per channel (camconst white levels
//!   when known, `crate::camconst`), camera native.
//! * A3 input/output: librtprocess scale, `cfa · wb_c / max(wb) · 65535`, so no channel
//!   exceeds 65535 before clipping (librtprocess clamps its input there).
//! * Stored source: `camera native · wb_c` with wb normalised to green = 1, i.e. the reference
//!   WB premultiplied (doc 02 §3, A3 ordering rationale).
//!
//! Noise: the raw-level noise model is the vendor's (DNG NoiseProfile, doc 02 §8), not
//! estimated here.
//! Cancellation (doc 05 §8): LibRaw polls at its stage transitions; CA correction and
//! demosaic poll about every megapixel (patched librtprocess).

use std::path::Path;

use aigle_core::color::{self, Mat3};
use aigle_core::geometry::{Orientation, Size};
use aigle_core::params::Demosaic;
use aigle_engine_api::{CameraInfo, CancelToken};
use aigle_kernels::Rgb;
use aigle_libraw_sys::{Kind, Meta, Raw, Thumb};
use aigle_rtprocess_sys::{self as rp, BayerAlgo, Planes};
use rayon::prelude::*;

use super::{ATier, LoadError, SourceColor};
use crate::profile::{self, CameraCalibration};

const SCALE: f32 = 65535.0;

pub struct RawDecoded {
    /// Oriented size.
    pub size: Size,
    pub exif_orientation: u16,
    /// Oriented camera RGB, reference WB premultiplied (before A4).
    pub rgb: Vec<Rgb>,
    /// Oriented per-pixel clip bits (bit c: a channel-c site clipped here), undilated;
    /// `None` if nothing clipped.
    pub clip_mask: Option<Vec<u8>>,
    /// White levels used (camconst), `None` = LibRaw's nominal maximum.
    pub white_levels: Option<[f32; 4]>,
    pub color: SourceColor,
    /// DNG NoiseProfile pairs (empty if absent).
    pub noise_profile: Vec<(f64, f64)>,
    pub camera: CameraInfo,
    /// Fraction (0..1) of CFA pixels at the white level.
    pub sensor_clipped: f32,
    /// Demosaic actually used (after resolving `Auto` / sensor mismatches).
    pub demosaic: Demosaic,
}

fn fail(e: impl std::fmt::Display) -> LoadError {
    LoadError::Failed(e.to_string())
}

fn check(cancel: Option<&CancelToken>) -> Result<(), LoadError> {
    if cancel.is_some_and(|c| c.is_cancelled()) {
        Err(LoadError::Cancelled)
    } else {
        Ok(())
    }
}

/// dcraw `flip` → EXIF orientation (LibRaw: 3 = 180°, 5 = 90° CCW, 6 = 90° CW).
pub fn flip_to_exif(flip: i32) -> u16 {
    match flip {
        1 => 2,
        2 => 4,
        3 => 3,
        4 => 5,
        5 => 8,
        6 => 6,
        7 => 7,
        _ => 1,
    }
}

/// `admit` is called with the declared peak ([`super::raw_peak`]) after the header is read
/// and before anything large is allocated.
pub fn decode(
    path: &Path,
    a: &ATier,
    cancel: Option<&CancelToken>,
    admit: &mut dyn FnMut(u64) -> Result<(), LoadError>,
) -> Result<RawDecoded, LoadError> {
    // A1
    let mut raw = Raw::open(path).map_err(fail)?;
    let (w, h, rw, rh) = raw.size();
    admit(super::raw_peak(w, h, rw, rh))?;
    raw.unpack(&|| cancel.is_some_and(|c| c.is_cancelled()))
        .map_err(|e| {
            if cancel.is_some_and(|c| c.is_cancelled()) {
                LoadError::Cancelled
            } else {
                fail(e)
            }
        })?;
    check(cancel)?;
    let meta = raw.meta().map_err(fail)?;
    let kind = meta
        .kind
        .clone()
        .map_err(|why| fail(format!("{}: unsupported raw ({why})", path.display())))?;
    let (w, h) = (meta.width, meta.height);
    if w < 16 || h < 16 {
        return Err(fail("raw image too small"));
    }
    let wb = reference_wb(&meta);
    let wb_max = wb.iter().cloned().fold(0.0f64, f64::max);
    let gain: [f32; 3] = wb.map(|m| (m / wb_max) as f32 * SCALE);
    let white_levels = white_levels(&meta, path);

    let (planes, sensor_clipped, demosaic, mask) = match kind {
        Kind::Bayer | Kind::XTrans => {
            let (mut cfa, clip, clipped) = raw.copy_cfa(w, h, white_levels).map_err(fail)?;
            drop(raw);
            let pattern = meta.cfa;
            // A2: hot/dead pixels on the normalised CFA.
            if a.hot_pixels {
                fix_hot_pixels(&mut cfa, w, h, &pattern);
            }
            // Per-channel site bits for A4.
            let mask = (clipped > 0).then(|| {
                let mut m = clip;
                m.par_chunks_mut(w as usize)
                    .enumerate()
                    .for_each(|(y, row)| {
                        for (x, v) in row.iter_mut().enumerate() {
                            *v = if *v != 0 {
                                1 << pattern[y % 6][x % 6]
                            } else {
                                0
                            };
                        }
                    });
                m
            });
            // Reference WB premultiply into the librtprocess scale.
            cfa.par_chunks_mut(w as usize)
                .enumerate()
                .for_each(|(y, row)| {
                    for (x, v) in row.iter_mut().enumerate() {
                        *v = v.max(0.0) * gain[pattern[y % 6][x % 6] as usize];
                    }
                });
            check(cancel)?;
            let bayer2 = [
                [pattern[0][0] as i32, pattern[0][1] as i32],
                [pattern[1][0] as i32, pattern[1][1] as i32],
            ];
            let cancelled = || cancel.is_some_and(|c| c.is_cancelled());
            let rp_err = |e: rp::Error| {
                if e.is_cancelled() {
                    LoadError::Cancelled
                } else {
                    fail(e)
                }
            };
            if a.ca_correction && kind == Kind::Bayer {
                if let Err(e) = rp::ca_correct(w, h, &mut cfa, bayer2, &cancelled) {
                    if e.is_cancelled() {
                        return Err(LoadError::Cancelled);
                    }
                    tracing::warn!("CA correction skipped: {e}");
                }
                check(cancel)?;
            }
            // A3
            let (planes, used) = match (kind, a.demosaic) {
                (Kind::Bayer, Demosaic::Amaze) => (
                    rp::demosaic_bayer(BayerAlgo::Amaze, w, h, &mut cfa, bayer2, &cancelled)
                        .map_err(rp_err)?,
                    Demosaic::Amaze,
                ),
                (Kind::Bayer, _) => (
                    rp::demosaic_bayer(BayerAlgo::Rcd, w, h, &mut cfa, bayer2, &cancelled)
                        .map_err(rp_err)?,
                    Demosaic::Rcd,
                ),
                (_, _) => {
                    let pattern6 = pattern.map(|r| r.map(|c| c as i32));
                    (
                        rp::demosaic_xtrans(w, h, &mut cfa, pattern6, meta.rgb_cam, 3, &cancelled)
                            .map_err(rp_err)?,
                        Demosaic::Markesteijn3,
                    )
                }
            };
            (planes, clipped as f32 / (w as f32 * h as f32), used, mask)
        }
        Kind::Rgb => {
            let rgb = raw.copy_rgb(w, h).map_err(fail)?;
            drop(raw);
            let n = w as usize * h as usize;
            let mut p = Planes {
                r: vec![0.0; n],
                g: vec![0.0; n],
                b: vec![0.0; n],
            };
            for (i, px) in rgb.as_chunks::<3>().0.iter().enumerate() {
                p.r[i] = px[0].max(0.0) * gain[0];
                p.g[i] = px[1].max(0.0) * gain[1];
                p.b[i] = px[2].max(0.0) * gain[2];
            }
            let bits: Vec<u8> = rgb
                .as_chunks::<3>()
                .0
                .iter()
                .map(|px| (0..3).filter(|&c| px[c] >= 0.995).map(|c| 1u8 << c).sum())
                .collect();
            let clipped = bits.iter().filter(|&&b| b != 0).count();
            (
                p,
                clipped as f32 / n as f32,
                Demosaic::Passthrough,
                (clipped > 0).then_some(bits),
            )
        }
    };
    check(cancel)?;

    // Back to the stored convention (green = 1 reference WB) and into Source orientation.
    let to_stored = (wb_max as f32) / SCALE;
    let exif = flip_to_exif(meta.flip);
    let o = Orientation::from_exif(exif);
    let sensor = Size::new(w, h);
    let size = o.apply_size(sensor);
    let inv = o.inverse();
    let mut rgb = vec![[0f32; 3]; size.area() as usize];
    rgb.par_chunks_mut(size.w as usize)
        .enumerate()
        .for_each(|(y, row)| {
            for (x, px) in row.iter_mut().enumerate() {
                let (sx, sy) = inv.apply_pixel(x as u32, y as u32, size);
                let i = sy as usize * w as usize + sx as usize;
                *px = [
                    planes.r[i] * to_stored,
                    planes.g[i] * to_stored,
                    planes.b[i] * to_stored,
                ];
            }
        });
    drop(planes);
    let clip_mask = mask.map(|m| {
        let mut out = vec![0u8; size.area() as usize];
        out.par_chunks_mut(size.w as usize)
            .enumerate()
            .for_each(|(y, row)| {
                for (x, v) in row.iter_mut().enumerate() {
                    let (sx, sy) = inv.apply_pixel(x as u32, y as u32, size);
                    *v = m[sy as usize * w as usize + sx as usize];
                }
            });
        out
    });

    let (color, noise_profile) = source_color(&meta, wb, path);
    Ok(RawDecoded {
        size,
        exif_orientation: exif,
        rgb,
        clip_mask,
        white_levels,
        color,
        noise_profile,
        camera: CameraInfo {
            make: meta.make.clone(),
            model: meta.model.clone(),
            iso: (meta.iso > 0.0).then_some(meta.iso),
            exposure_time: (meta.shutter > 0.0).then_some(meta.shutter),
            f_number: (meta.aperture > 0.0).then_some(meta.aperture),
            focal_length: (meta.focal_len > 0.0).then_some(meta.focal_len),
            focal_length_35mm: (meta.focal_35mm > 0.0).then_some(meta.focal_35mm),
            lens_make: (!meta.lens_make.is_empty()).then(|| meta.lens_make.clone()),
            lens_model: (!meta.lens_model.is_empty()).then(|| meta.lens_model.clone()),
            lens_focal_range: (meta.lens_focal_range.0 > 0.0).then_some(meta.lens_focal_range),
            shot: shot_info(&meta.shot),
        },
        sensor_clipped,
        demosaic,
    })
}

fn shot_info(s: &aigle_libraw_sys::Shot) -> aigle_engine_api::ShotInfo {
    aigle_engine_api::ShotInfo {
        date_time: s.date_time.clone().filter(|d| valid_exif_date(d)),
        utc_offset: s.utc_offset.clone().filter(|o| valid_utc_offset(o)),
        exposure_bias: s.exposure_bias,
        exposure_program: s.exposure_program,
        metering_mode: s.metering_mode,
        flash: s.flash,
        exposure_mode: s.exposure_mode,
        white_balance: s.white_balance,
        max_aperture: s.max_aperture,
        body_serial: known(&s.body_serial),
        lens_serial: known(&s.lens_serial),
        firmware: s.firmware.clone(),
        artist: s.artist.clone(),
        copyright: s.copyright.clone(),
        // A GPS IFD without a fix holds zeros.
        gps: s
            .gps
            .filter(|g| g.0 != 0.0 || g.1 != 0.0)
            .map(
                |(latitude, longitude, altitude)| aigle_engine_api::GpsPosition {
                    latitude,
                    longitude,
                    altitude,
                },
            ),
    }
}

fn known(v: &Option<String>) -> Option<String> {
    v.clone()
        .filter(|s| !matches!(s.to_ascii_uppercase().as_str(), "N/A" | "NONE" | "UNKNOWN"))
}

/// `±HH:MM`.
pub(crate) fn valid_utc_offset(o: &str) -> bool {
    let b = o.as_bytes();
    b.len() == 6
        && matches!(b[0], b'+' | b'-')
        && b[1..3].iter().all(u8::is_ascii_digit)
        && b[3] == b':'
        && b[4..6].iter().all(u8::is_ascii_digit)
}

/// `YYYY:MM:DD HH:MM:SS` with a plausible year (cameras without a set clock write blanks or
/// zeros).
pub(crate) fn valid_exif_date(d: &str) -> bool {
    let b = d.as_bytes();
    b.len() >= 19
        && b[..4].iter().all(u8::is_ascii_digit)
        && d[..4] != *"0000"
        && b[4] == b':'
        && b[7] == b':'
}

/// As-shot multipliers normalised to green = 1; daylight `pre_mul` if as-shot is missing.
fn reference_wb(m: &Meta) -> [f64; 3] {
    let valid = |v: &[f32; 4]| v[0] > 0.0 && v[1] > 0.0 && v[2] > 0.0;
    let src = if valid(&m.cam_mul) {
        m.cam_mul
    } else if valid(&m.pre_mul) {
        m.pre_mul
    } else {
        [1.0; 4]
    };
    let g = src[1] as f64;
    [src[0] as f64 / g, 1.0, src[2] as f64 / g]
}

/// Camera colour description (doc 02 §6.2). DNG files: the file's own calibration
/// (ColorMatrix1/2, ForwardMatrix1/2, CameraCalibration1/2, AnalogBalance; dual-illuminant
/// interpolation) and its NoiseProfile. Other raws: LibRaw's `cam_xyz` (Adobe/dcraw D65
/// matrix) and no noise profile.
fn source_color(m: &Meta, wb: [f64; 3], path: &Path) -> (SourceColor, Vec<(f64, f64)>) {
    let dng = path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("dng"))
        .then(|| std::fs::read(path).map_err(|e| e.to_string()).and_then(|b| profile::parse(&b)))
        .and_then(|r| {
            r.map_err(|e| tracing::warn!(path = %path.display(), "DNG colour tags unusable, using LibRaw's matrix: {e}"))
                .ok()
        });
    let mut noise = Vec::new();
    let calibration = match dng {
        Some(p) => {
            noise = p.noise_profile;
            p.calibration
        }
        None => {
            let xyz_to_cam: Mat3 = [0, 1, 2].map(|r| m.cam_xyz[r].map(|v| v as f64));
            let usable = xyz_to_cam.iter().flatten().any(|v| *v != 0.0)
                && color::invert(&xyz_to_cam).is_some();
            // Unknown camera: treat camera RGB as linear sRGB.
            CameraCalibration::single(if usable {
                xyz_to_cam
            } else {
                color::invert(&color::SRGB_TO_XYZ).expect("invertible")
            })
        }
    };
    (
        SourceColor::new(wb, calibration, m.baseline_exposure.unwrap_or(0.0) as f64),
        noise,
    )
}

/// Per-camera white levels from the camconst table (not for DNG, which carry their own
/// WhiteLevel).
fn white_levels(m: &Meta, path: &Path) -> Option<[f32; 4]> {
    if path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("dng"))
    {
        return None;
    }
    let w = crate::camconst::table()?.white(&m.make, &m.model, m.iso, m.aperture)?;
    // Only levels below LibRaw's maximum change anything (the shim caps them there).
    w.iter().any(|&v| v > 0.0 && v < m.white).then_some(w)
}

/// Same-colour neighbour offsets for each of the 6×6 CFA positions: radius 2, widened to
/// radius 3 where that gives fewer than six (X-Trans red/blue).
fn same_colour_offsets(pattern: &[[u8; 6]; 6]) -> Vec<Vec<(i32, i32)>> {
    let mut out = Vec::with_capacity(36);
    for py in 0..6i32 {
        for px in 0..6i32 {
            let me = pattern[py as usize][px as usize];
            let mut offs = Vec::new();
            for r in 2..=3i32 {
                offs.clear();
                for dy in -r..=r {
                    for dx in -r..=r {
                        let c = pattern[(py + dy).rem_euclid(6) as usize]
                            [(px + dx).rem_euclid(6) as usize];
                        if (dx, dy) != (0, 0) && c == me {
                            offs.push((dx, dy));
                        }
                    }
                }
                if offs.len() >= 6 {
                    break;
                }
            }
            out.push(offs);
        }
    }
    out
}

/// Conservative hot/dead pixel repair on a Bayer or X-Trans CFA: a pixel more than 2×
/// brighter (or below a quarter) of all its same-colour neighbours within radius 2 (3 for
/// sparse X-Trans colours), and beyond a floor, is replaced by their mean.
fn fix_hot_pixels(cfa: &mut [f32], w: u32, h: u32, pattern: &[[u8; 6]; 6]) {
    const B: usize = 3;
    let (w, h) = (w as usize, h as usize);
    if w <= 2 * B || h <= 2 * B {
        return;
    }
    let offsets = same_colour_offsets(pattern);
    let src = cfa.to_vec();
    cfa.par_chunks_mut(w)
        .enumerate()
        .skip(B)
        .take(h - 2 * B)
        .for_each(|(y, row)| {
            for (x, v) in row.iter_mut().enumerate().skip(B).take(w - 2 * B) {
                let offs = &offsets[(y % 6) * 6 + x % 6];
                let mut mx = f32::MIN;
                let mut mn = f32::MAX;
                let mut sum = 0.0;
                for &(dx, dy) in offs {
                    let n = src[(y as i32 + dy) as usize * w + (x as i32 + dx) as usize];
                    mx = mx.max(n);
                    mn = mn.min(n);
                    sum += n;
                }
                let c = src[y * w + x];
                if (c > 2.0 * mx && c - mx > 0.02) || (c < 0.25 * mn && mn - c > 0.02) {
                    *v = sum / offs.len() as f32;
                }
            }
        });
}

/// Embedded preview of a raw file, oriented, as 8-bit RGB.
pub fn embedded_preview(path: &Path) -> Result<(u32, u32, Vec<u8>), String> {
    let mut raw = Raw::open(path).map_err(|e| e.to_string())?;
    let flip = raw.meta().map(|m| m.flip).unwrap_or(0);
    let (w, h, rgb) = match raw.thumbnail().map_err(|e| e.to_string())? {
        Thumb::Jpeg(bytes) => aigle_vips_sys::decode_rgb8(&bytes).map_err(|e| e.to_string())?,
        Thumb::Rgb8 {
            width,
            height,
            data,
        } => (width, height, data),
    };
    let o = Orientation::from_exif(flip_to_exif(flip));
    let from = Size::new(w, h);
    let size = o.apply_size(from);
    let inv = o.inverse();
    let mut out = vec![0u8; size.area() as usize * 3];
    for y in 0..size.h {
        for x in 0..size.w {
            let (sx, sy) = inv.apply_pixel(x, y, size);
            let s = (sy * w + sx) as usize * 3;
            let d = (y * size.w + x) as usize * 3;
            out[d..d + 3].copy_from_slice(&rgb[s..s + 3]);
        }
    }
    Ok((size.w, size.h, out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flips_match_libraw_docs() {
        // 6 = 90° CW: sensor top-left ends up top-right.
        let o = Orientation::from_exif(flip_to_exif(6));
        assert_eq!(o.apply_pixel(0, 0, Size::new(4, 2)), (1, 0));
        let o = Orientation::from_exif(flip_to_exif(5));
        assert_eq!(o.apply_pixel(0, 0, Size::new(4, 2)), (0, 3));
        assert_eq!(flip_to_exif(3), 3);
    }

    const BAYER: [[u8; 6]; 6] = {
        let mut p = [[0u8; 6]; 6];
        let mut y = 0;
        while y < 6 {
            let mut x = 0;
            while x < 6 {
                p[y][x] = match (y % 2, x % 2) {
                    (0, 0) => 0,
                    (1, 1) => 2,
                    _ => 1,
                };
                x += 1;
            }
            y += 1;
        }
        p
    };

    /// Fujifilm X-Trans (X-T20 layout as reported by LibRaw).
    const XTRANS: [[u8; 6]; 6] = [
        [1, 1, 0, 1, 1, 2],
        [1, 1, 2, 1, 1, 0],
        [2, 0, 1, 0, 2, 1],
        [1, 1, 2, 1, 1, 0],
        [1, 1, 0, 1, 1, 2],
        [0, 2, 1, 2, 0, 1],
    ];

    #[test]
    fn hot_pixel_is_repaired() {
        for pattern in [BAYER, XTRANS] {
            let (w, h) = (14u32, 14u32);
            let mut cfa = vec![0.2f32; 196];
            for (y, x) in [(6, 6), (7, 8), (8, 7)] {
                let i = y * 14 + x;
                cfa.fill(0.2);
                cfa[i] = 0.9;
                cfa[i + 1 + 14] = 0.25; // normal variation stays
                fix_hot_pixels(&mut cfa, w, h, &pattern);
                assert!(
                    (cfa[i] - 0.2).abs() < 0.01,
                    "{pattern:?} {y},{x}: {}",
                    cfa[i]
                );
                assert_eq!(cfa[i + 15], 0.25);
            }
        }
    }

    #[test]
    fn same_colour_neighbours_exist_for_every_position() {
        for pattern in [BAYER, XTRANS] {
            for offs in same_colour_offsets(&pattern) {
                assert!(offs.len() >= 6, "{offs:?}");
            }
        }
        // Bayer red keeps the eight distance-2 neighbours.
        assert_eq!(same_colour_offsets(&BAYER)[0].len(), 8);
    }
}
