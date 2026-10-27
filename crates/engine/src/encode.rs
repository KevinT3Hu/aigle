//! Encoding and output-size resampling of rendered BGRA8 images.
//! Encoding through libvips (doc 04 §4): in-memory encodes of rendered BGRA8 images and
//! streamed saves, where libvips pulls the image strip by strip from a callback so that the
//! whole output is never resident (doc 04 §6). Output ICC profiles (sRGB, Display P3, and the
//! generated Adobe RGB / ProPhoto / Rec.2020 profiles of `crate::output_space`) and EXIF are
//! embedded.

use std::path::Path;

use aigle_engine_api::{
    CameraInfo, DisplayProfileId, EncodedFormat, ExportOptions, MetadataPolicy, OutputSharpening,
};
use aigle_kernels::math::{srgb_eotf, srgb_oetf};
use aigle_vips_sys as vips;
use rayon::prelude::*;

use crate::output_space;

/// Saver suffix with options for `fmt` (libvips syntax).
pub fn suffix(fmt: EncodedFormat) -> String {
    match fmt {
        EncodedFormat::Jpeg { quality } => {
            format!(".jpg[Q={},optimize_coding=true]", quality.clamp(1, 100))
        }
        EncodedFormat::Png => ".png[compression=6]".into(),
        EncodedFormat::Tiff { .. } => ".tif[compression=deflate,predictor=horizontal]".into(),
        EncodedFormat::WebP { quality } => format!(".webp[Q={}]", quality.clamp(1, 100)),
    }
}

pub fn sixteen_bit(fmt: EncodedFormat) -> bool {
    matches!(fmt, EncodedFormat::Tiff { sixteen_bit: true })
}

/// The `icc` / `icc_data` fields of [`vips::SaveOpts`] for an output profile.
pub fn icc_args(icc: &output_space::Icc) -> (Option<&'static str>, Option<&[u8]>) {
    match icc {
        output_space::Icc::Builtin(n) => (Some(n), None),
        output_space::Icc::Data(d) => (None, Some(d)),
    }
}

/// EXIF of an export from the source's camera data.
pub fn exif_for(camera: Option<&CameraInfo>) -> vips::Exif {
    let mut x = vips::Exif {
        software: Some(format!("aigle {}", env!("CARGO_PKG_VERSION"))),
        ..Default::default()
    };
    if let Some(c) = camera {
        let s = |v: &str| (!v.is_empty()).then(|| v.to_owned());
        x.make = s(&c.make);
        x.model = s(&c.model);
        x.lens_make = c.lens_make.clone();
        x.lens_model = c.lens_model.clone();
        x.exposure_time = c.exposure_time.map(f64::from);
        x.f_number = c.f_number.map(f64::from);
        x.focal_length = c.focal_length.map(f64::from);
        x.iso = c.iso.map(f64::from);
        x.datetime = c.shot.date_time.clone();
        x.artist = c.shot.artist.clone();
        x.copyright = c.shot.copyright.clone();
        x.exposure_bias = c.shot.exposure_bias.map(f64::from);
    }
    x
}

/// EXIF of an export from the source's camera data under `opts`' metadata policy; `None`
/// writes no EXIF.
pub fn export_exif(camera: Option<&CameraInfo>, opts: &ExportOptions) -> Option<vips::Exif> {
    let mut x = match opts.metadata {
        MetadataPolicy::Strip => return None,
        MetadataPolicy::All => exif_for(camera),
        MetadataPolicy::CopyrightOnly => {
            let full = exif_for(camera);
            vips::Exif {
                software: full.software,
                artist: full.artist,
                copyright: full.copyright,
                ..Default::default()
            }
        }
    };
    fn set(v: &Option<String>) -> Option<&str> {
        v.as_deref().map(str::trim).filter(|s| !s.is_empty())
    }
    if let Some(a) = set(&opts.artist) {
        x.artist = Some(a.to_owned());
    }
    if let Some(c) = set(&opts.copyright) {
        x.copyright = Some(c.to_owned());
    }
    Some(x)
}

/// Unsharp-mask `(sigma, amount)` for an output sharpening strength.
pub fn sharpen(s: OutputSharpening) -> Option<(f64, f64)> {
    match s {
        OutputSharpening::Off => None,
        OutputSharpening::Low => Some((0.5, 1.0)),
        OutputSharpening::Standard => Some((0.5, 2.0)),
        OutputSharpening::High => Some((0.7, 3.0)),
    }
}

/// Encode a BGRA8 image in memory.
pub fn encode(
    width: u32,
    height: u32,
    bgra: &[u8],
    fmt: EncodedFormat,
    display: &DisplayProfileId,
    exif: Option<&vips::Exif>,
) -> Result<Vec<u8>, String> {
    let fill = |x: u32, y: u32, w: u32, h: u32, out: &mut [u8], stride: usize| {
        for j in 0..h as usize {
            let src = &bgra[((y as usize + j) * width as usize + x as usize) * 4..];
            let dst = &mut out[j * stride..];
            for i in 0..w as usize {
                dst[i * 3] = src[i * 4 + 2];
                dst[i * 3 + 1] = src[i * 4 + 1];
                dst[i * 3 + 2] = src[i * 4];
            }
        }
        true
    };
    let icc = output_space::icc(display);
    let (name, data) = icc_args(&icc);
    let opts = vips::SaveOpts {
        width,
        height,
        sixteen_bit: false,
        out_size: (width, height),
        icc: name,
        exif,
        suffix: &suffix(match fmt {
            EncodedFormat::Tiff { .. } => EncodedFormat::Tiff { sixteen_bit: false },
            f => f,
        }),
        sharpen: None,
        icc_data: data,
        resize_gamma: None,
    };
    vips::save_buffer(&opts, &fill).map_err(|e| format!("encode: {e}"))
}

/// Save an image generated by `fill` to `path` atomically (temp file in the same directory,
/// fsync, rename; doc 01 §9.3). `opts.suffix` selects the saver.
pub fn save_streamed(
    path: &Path,
    opts: &vips::SaveOpts<'_>,
    fill: &vips::Fill<'_>,
) -> Result<(), vips::Error> {
    let _slot = crate::arbiter::io().acquire(None);
    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy())
        .unwrap_or_default();
    let base = dir.join(format!(".{name}.aigle-tmp"));
    let ext = opts.suffix.split('[').next().unwrap_or("");
    let tmp = std::path::PathBuf::from(format!("{}{ext}", base.display()));
    let res = vips::save_file(opts, fill, &base).and_then(|()| {
        std::fs::File::open(&tmp)
            .and_then(|f| f.sync_all())
            .and_then(|()| std::fs::rename(&tmp, path))
            .map_err(|e| vips::Error::Vips(format!("{}: {e}", path.display())))
    });
    if res.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    res
}

/// Atomic write: temp file in the same directory, fsync, rename (doc 01 §9.3). Writes take
/// an I/O slot by class (doc 05 §5).
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    let _slot = crate::arbiter::io().acquire(None);
    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let name = path
        .file_name()
        .ok_or_else(|| format!("{}: not a file path", path.display()))?;
    let tmp = dir.join(format!(".{}.aigle-tmp", name.to_string_lossy()));
    let res = (|| {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    if res.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    res.map_err(|e| format!("{}: {e}", path.display()))
}

/// Area-average downsample of BGRA8 in linear light.
pub fn resize_area(w: u32, h: u32, bgra: &[u8], nw: u32, nh: u32) -> Vec<u8> {
    if (w, h) == (nw, nh) {
        return bgra.to_vec();
    }
    let lut: Vec<f32> = (0..256).map(|i| srgb_eotf(i as f32 / 255.0)).collect();
    let fx = w as f64 / nw as f64;
    let fy = h as f64 / nh as f64;
    let mut out = vec![0u8; nw as usize * nh as usize * 4];
    out.par_chunks_mut(nw as usize * 4)
        .enumerate()
        .for_each(|(oy, row)| {
            let y0 = oy as f64 * fy;
            let y1 = y0 + fy;
            for ox in 0..nw as usize {
                let x0 = ox as f64 * fx;
                let x1 = x0 + fx;
                let mut acc = [0f64; 4];
                let mut wsum = 0f64;
                for sy in y0.floor() as u32..(y1.ceil() as u32).min(h) {
                    let wy = (y1.min(sy as f64 + 1.0) - y0.max(sy as f64)).max(0.0);
                    for sx in x0.floor() as u32..(x1.ceil() as u32).min(w) {
                        let wx = (x1.min(sx as f64 + 1.0) - x0.max(sx as f64)).max(0.0);
                        let wgt = wx * wy;
                        let i = ((sy * w + sx) * 4) as usize;
                        for c in 0..3 {
                            acc[c] += lut[bgra[i + c] as usize] as f64 * wgt;
                        }
                        acc[3] += bgra[i + 3] as f64 * wgt;
                        wsum += wgt;
                    }
                }
                let o = &mut row[ox * 4..ox * 4 + 4];
                for c in 0..3 {
                    o[c] = (srgb_oetf((acc[c] / wsum) as f32) * 255.0)
                        .round()
                        .clamp(0.0, 255.0) as u8;
                }
                o[3] = (acc[3] / wsum).round() as u8;
            }
        });
    out
}

/// Horizontal concatenation of two BGRA8 images of equal height.
pub fn side_by_side(a: (u32, u32, &[u8]), b: (u32, u32, &[u8])) -> (u32, u32, Vec<u8>) {
    let (aw, h, ad) = a;
    let (bw, bh, bd) = b;
    debug_assert_eq!(h, bh);
    let w = aw + bw;
    let mut out = Vec::with_capacity(w as usize * h as usize * 4);
    for y in 0..h as usize {
        out.extend_from_slice(&ad[y * aw as usize * 4..(y + 1) * aw as usize * 4]);
        out.extend_from_slice(&bd[y * bw as usize * 4..(y + 1) * bw as usize * 4]);
    }
    (w, h, out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_metadata_policy() {
        let cam = CameraInfo {
            make: "Nikon".into(),
            model: "D3200".into(),
            ..Default::default()
        };
        let mut opts = ExportOptions {
            copyright: Some("© Me".into()),
            artist: Some("  ".into()),
            ..Default::default()
        };
        let all = export_exif(Some(&cam), &opts).unwrap();
        assert_eq!(all.model.as_deref(), Some("D3200"));
        assert_eq!(all.copyright.as_deref(), Some("© Me"));
        assert_eq!(all.artist, None);
        opts.metadata = MetadataPolicy::CopyrightOnly;
        let some = export_exif(Some(&cam), &opts).unwrap();
        assert_eq!((some.make, some.copyright.as_deref()), (None, Some("© Me")));
        opts.metadata = MetadataPolicy::Strip;
        assert_eq!(export_exif(Some(&cam), &opts), None);
    }

    #[test]
    fn resize_preserves_flat_colour() {
        let src = [10u8, 128, 250, 255].repeat(9 * 6);
        let out = resize_area(9, 6, &src, 4, 3);
        for px in out.as_chunks::<4>().0 {
            assert_eq!(*px, [10, 128, 250, 255]);
        }
    }

    #[test]
    fn encodes_with_profile_and_exif() {
        let px = [0u8, 0, 255, 255].repeat(16);
        let d = DisplayProfileId::Srgb;
        let png = encode(4, 4, &px, EncodedFormat::Png, &d, None).unwrap();
        let (w, _, rgb) = vips::decode_rgb8(&png).unwrap();
        assert_eq!((w, &rgb[..3]), (4, &[255u8, 0, 0][..]));
        let cam = CameraInfo {
            make: "Nikon".into(),
            model: "D3200".into(),
            iso: Some(800.0),
            ..Default::default()
        };
        let exif = exif_for(Some(&cam));
        for fmt in [
            EncodedFormat::Jpeg { quality: 90 },
            EncodedFormat::WebP { quality: 80 },
            EncodedFormat::Tiff { sixteen_bit: false },
        ] {
            assert!(!encode(4, 4, &px, fmt, &d, Some(&exif)).unwrap().is_empty());
        }
    }
}
