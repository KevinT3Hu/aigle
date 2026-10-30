//! HDR rendering in relative linear Rec.2020 (diffuse white = 203 cd/m²).
//! SDR kernels and their output remain unchanged. HDR output is evaluated on the host,
//! retaining float values through tone, local adjustments, creative colour and resampling.

use aigle_kernels::{
    Rgb, math, pointwise,
    tiles::{OutParams, local},
};

pub const REFERENCE_WHITE: f32 = 203.0;
pub const PEAK_NITS: f32 = 1000.0;
pub const HEADROOM: f32 = PEAK_NITS / REFERENCE_WHITE;
/// Host-only output selector; never submitted to the SDR shader.
pub const TRANSFER_LINEAR_HDR: u32 = 5;

/// SMPTE ST 2084, absolute cd/m² → full-range PQ code value.
pub fn pq_encode(nits: f32) -> f32 {
    let p = (nits.clamp(0.0, 10000.0) / 10000.0).powf(2610.0 / 16384.0);
    ((3424.0 / 4096.0 + (2413.0 / 128.0) * p) / (1.0 + (2392.0 / 128.0) * p)).powf(2523.0 / 32.0)
}

/// Full-range PQ code value → absolute cd/m².
pub fn pq_decode(code: f32) -> f32 {
    let p = code.clamp(0.0, 1.0).powf(32.0 / 2523.0);
    10000.0
        * ((p - 3424.0 / 4096.0).max(0.0) / (2413.0 / 128.0 - (2392.0 / 128.0) * p))
            .powf(16384.0 / 2610.0)
}

/// BT.2100 HLG display EOTF at the reference 1000-nit display (system gamma 1.2).
/// The OOTF is luminance dependent, so it must be applied to RGB together.
pub fn hlg_decode(rgb: Rgb) -> Rgb {
    let scene = rgb.map(|v| {
        let v = v.clamp(0.0, 1.0);
        if v <= 0.5 {
            v * v / 3.0
        } else {
            (((v - 0.559_910_7) / 0.178_832_77).exp() + 0.284_668_92) / 12.0
        }
    });
    let gain = math::luminance(scene).max(0.0).powf(0.2) * (1000.0 / REFERENCE_WHITE);
    scene.map(|v| v * gain)
}

/// Render HDR without the SDR sigmoid's unit-white bound. The default is linear up to
/// diffuse white, with a smooth shoulder above white approaching the mastering peak.
/// Bounded creative LUTs/curves act on chromaticity-preserving, normalized values and
/// their result is expanded back into the highlight range afterward.
pub fn output(
    px: Rgb,
    fields: Option<[f32; local::D_FIELDS]>,
    gx: u32,
    gy: u32,
    p: &OutParams,
    aux: &[f32],
) -> (Rgb, u32) {
    let d = fields.unwrap_or([0.0; local::D_FIELDS]);
    let s = p.d1_sliders;
    let contrast = (s[0] + d[0]).clamp(-100.0, 100.0);
    let whites = (s[1] + d[1]).clamp(-100.0, 100.0);
    let blacks = (s[2] + d[2]).clamp(-100.0, 100.0) / 100.0;
    let px = pointwise::gamut_compress(px);
    let n = px.into_iter().fold(0.0f32, f32::max);
    let mut mapped = 0.18 * (n / 0.18).powf(2.0f32.powf(contrast / 100.0));
    mapped *= 2.0f32.powf(whites / 100.0);
    if mapped > 1.0 {
        mapped = 1.0 + (HEADROOM - 1.0) * (1.0 - (-(mapped - 1.0) / (HEADROOM - 1.0)).exp());
    }
    let v = px.map(|v| {
        let y = if n > 1e-10 { v * (mapped / n) } else { 0.0 };
        let toe = (1.0 - y.clamp(0.0, 1.0)).powi(4);
        (y + blacks * 0.05 * toe).max(0.0)
    });
    let scale = v.into_iter().fold(1.0f32, f32::max);
    let v = v.map(|v| v / scale);
    let v = if p.d2.is_identity() && d[3] == 0.0 && d[4] == 0.0 {
        v
    } else {
        pointwise::d2_local(v, &p.d2, aux, d[3] / 100.0, d[4])
    };
    let v = if p.d3.active == 0 {
        v
    } else {
        aigle_kernels::d3::d3(v, gx, gy, &p.d3, p.seed)
    };
    let mut v = v.map(|v| v * scale);
    let mut flags = 0;
    if p.e1.proof != 0 {
        let o = p.e1.proof as usize;
        let matrix = |o: usize, v: Rgb| -> Rgb {
            std::array::from_fn(|r| {
                aux[o + r * 3] * v[0] + aux[o + r * 3 + 1] * v[1] + aux[o + r * 3 + 2] * v[2]
            })
        };
        let proof = matrix(o, v);
        if proof.into_iter().any(|v| !(-0.001..=1.001).contains(&v)) {
            flags |= aigle_kernels::tiles::CLIP_GAMUT;
        }
        v = matrix(o + 9, pointwise::fit_output(proof, math::luminance(v)));
        // The second proof matrix already transforms into output primaries.
    } else {
        v = math::mat3_mul(&p.e1.matrix, v);
    }
    if v.into_iter().any(|v| v >= HEADROOM) {
        flags |= aigle_kernels::tiles::CLIP_HIGH;
    }
    if v.into_iter().any(|v| v <= 0.0) {
        flags |= aigle_kernels::tiles::CLIP_LOW;
    }
    (
        v.map(|v| {
            if v.is_finite() {
                v.clamp(0.0, HEADROOM)
            } else {
                0.0
            }
        }),
        flags,
    )
}

/// Custom HDR codecs share libvips' linear float resize, sharpening and watermark
/// pipeline. Source strips and codec blocks are bounded; a full output is never allocated.
pub fn save_encoded(
    path: &std::path::Path,
    vopts: &aigle_vips_sys::SaveOpts<'_>,
    fill: &aigle_vips_sys::Fill<'_>,
    opts: &aigle_engine_api::ExportOptions,
    cancel: &aigle_engine_api::CancelToken,
) -> Result<(), String> {
    use exr::prelude::*;
    use std::{
        io::Write,
        sync::{
            Mutex,
            atomic::{AtomicBool, Ordering},
        },
    };
    let _slot = crate::arbiter::io().acquire(Some(cancel));
    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(std::path::Path::new("."));
    let tmp = dir.join(format!(
        ".{}.aigle-tmp.{}",
        path.file_name()
            .ok_or("missing filename")?
            .to_string_lossy(),
        opts.format.extension()
    ));
    let result = aigle_vips_sys::with_float_image(vopts, fill, |source| {
        let (w, h) = (source.width, source.height);
        if opts.format == aigle_engine_api::EncodedFormat::HdrPng {
            let file =
                std::io::BufWriter::new(std::fs::File::create(&tmp).map_err(|e| e.to_string())?);
            let mut encoder = png::Encoder::new(file, w, h);
            encoder.set_color(png::ColorType::Rgb);
            encoder.set_depth(png::BitDepth::Sixteen);
            if let Some(exif) = vopts.exif {
                for (key, value) in [
                    ("Author", exif.artist.as_ref()),
                    ("Copyright", exif.copyright.as_ref()),
                    ("Software", exif.software.as_ref()),
                ] {
                    if let Some(v) = value {
                        encoder
                            .add_itxt_chunk(key.into(), v.clone())
                            .map_err(|e| e.to_string())?;
                    }
                }
            }
            let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
            writer
                .write_chunk(png::chunk::ChunkType(*b"cICP"), &[9, 16, 0, 1])
                .map_err(|e| e.to_string())?;
            // Conservative HDR light-level metadata, in 0.0001 cd/m² units (PNG 3).
            let level = (PEAK_NITS * 10000.0) as u32;
            let clli: Vec<u8> = [level, level]
                .into_iter()
                .flat_map(u32::to_be_bytes)
                .collect();
            writer
                .write_chunk(png::chunk::ChunkType(*b"cLLi"), &clli)
                .map_err(|e| e.to_string())?;
            let to_2020 = aigle_core::color::to_f32(&aigle_core::color::mul(
                &aigle_core::color::invert(&aigle_core::color::REC2020_TO_XYZ).unwrap(),
                &aigle_core::color::SRGB_TO_XYZ,
            ));
            let mut stream = writer.stream_writer().map_err(|e| e.to_string())?;
            let mut row = Vec::with_capacity(w as usize * 6);
            for y in 0..h {
                if cancel.is_cancelled() {
                    return Err("HDR export cancelled".into());
                }
                row.clear();
                for rgb in source.row(y).map_err(|e| e.to_string())?.as_chunks::<3>().0 {
                    let rgb = math::mat3_mul(&to_2020, [rgb[0], rgb[1], rgb[2]]);
                    for v in rgb {
                        row.extend(
                            ((pq_encode((v * REFERENCE_WHITE).clamp(0.0, PEAK_NITS)) * 65535.0)
                                .round() as u16)
                                .to_be_bytes(),
                        );
                    }
                }
                stream.write_all(&row).map_err(|e| e.to_string())?;
            }
            stream.finish().map_err(|e| e.to_string())?;
            writer.finish().map_err(|e| e.to_string())?;
        } else {
            let failed = AtomicBool::new(false);
            // EXR's block writer requests each source row in small channel groups. A
            // block-height cache prevents repeated pipeline evaluation per pixel/channel.
            let rows = Mutex::new(std::collections::HashMap::<usize, Vec<f32>>::new());
            let pixels = |pos: Vec2<usize>| {
                if cancel.is_cancelled() {
                    failed.store(true, Ordering::Relaxed);
                    return (0.0f32, 0.0f32, 0.0f32);
                }
                let mut rows = rows.lock().unwrap();
                if rows.len() >= 64 && !rows.contains_key(&pos.y()) {
                    rows.clear();
                }
                let row = rows
                    .entry(pos.y())
                    .or_insert_with(|| match source.row(pos.y() as u32) {
                        Ok(row) => row,
                        Err(_) => {
                            failed.store(true, Ordering::Relaxed);
                            vec![0.0; w as usize * 3]
                        }
                    });
                let p = &row[pos.x() * 3..];
                (p[0], p[1], p[2])
            };
            let mut attributes = LayerAttributes::named("HDR");
            attributes.white_luminance = Some(REFERENCE_WHITE);
            if let Some(exif) = vopts.exif {
                attributes.software_name = exif.software.as_deref().map(Text::from);
                attributes.owner = exif.artist.as_deref().map(Text::from);
                attributes.comments = exif.copyright.as_deref().map(Text::from);
            }
            let layer = Layer::new(
                (w as usize, h as usize),
                attributes,
                Encoding::SMALL_FAST_LOSSLESS,
                SpecificChannels::rgb(pixels),
            );
            let mut image = Image::from_layer(layer);
            image.attributes.chromaticities = Some(exr::meta::attribute::Chromaticities {
                red: Vec2(0.64, 0.33),
                green: Vec2(0.30, 0.60),
                blue: Vec2(0.15, 0.06),
                white: Vec2(0.3127, 0.3290),
            });
            image
                .write()
                .non_parallel()
                .to_file(&tmp)
                .map_err(|e| e.to_string())?;
            if failed.load(Ordering::Relaxed) {
                return Err("HDR export cancelled or source render failed".into());
            }
        }
        Ok(())
    })
    .and_then(|()| {
        if cancel.is_cancelled() {
            return Err("HDR export cancelled".into());
        }
        std::fs::File::open(&tmp)
            .and_then(|f| f.sync_all())
            .and_then(|()| std::fs::rename(&tmp, path))
            .map_err(|e| e.to_string())
    });
    if result.is_err() {
        let _ = std::fs::remove_file(tmp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pq_absolute_luminance_anchors_and_round_trip() {
        assert!((pq_encode(100.0) - 0.5080784).abs() < 1e-5);
        assert!((pq_encode(1000.0) - 0.7518271).abs() < 1e-5);
        for n in [0.0, 0.001, 0.1, 1.0, 80.0, 203.0, 1000.0, 10000.0] {
            assert!(
                (pq_decode(pq_encode(n)) - n).abs() < (n * 0.0001).max(0.00001),
                "{n}"
            );
        }
    }
    #[test]
    fn hlg_white_is_1000_nits_and_black_is_zero() {
        assert_eq!(hlg_decode([0.0; 3]), [0.0; 3]);
        for c in hlg_decode([1.0; 3]) {
            assert!((c * REFERENCE_WHITE - 1000.0).abs() < 0.01);
        }
    }
}
