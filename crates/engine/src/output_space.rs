//! Output RGB spaces (E1, doc 02 §6.5): primaries, transfer curve and the ICC profile that
//! is embedded in exports.
//!
//! sRGB and Display P3 embed libvips' built-in profiles. Adobe RGB, ProPhoto and Rec.2020
//! embed matrix/TRC ICC v4 profiles generated here from the same primaries and curves the
//! kernels encode with, so a colour-managed reader decodes exactly what was rendered.

use std::sync::{Arc, OnceLock};

use aigle_core::color::{
    self, D50_XY, D65_XY, Mat3, adobe_rgb_to_xyz, bradford, prophoto_to_xyz, prophoto_to_xyz_d50,
    xy_to_xyz,
};
use aigle_engine_api::DisplayProfileId;
use aigle_kernels::math::{
    TRANSFER_ADOBE, TRANSFER_PROPHOTO, TRANSFER_REC709, TRANSFER_SRGB, TRANSFER_TABLE,
};
use aigle_kernels::pointwise::E1Params;

/// The registered display profile `d` refers to, if it is one and can be found.
pub fn display_icc(d: &DisplayProfileId) -> Option<Arc<crate::icc::DisplayIcc>> {
    let DisplayProfileId::Icc(hash) = d else {
        return None;
    };
    let r = aigle_core::DataRef(hash.clone());
    let p = crate::data::file(crate::data::FileKind::DisplayIcc, &r, crate::icc::parse);
    if p.is_none() {
        tracing::warn!(profile = %hash, "display profile not found; rendering as sRGB");
    }
    p
}

/// Linear output RGB → XYZ (D65) of `d`. Unresolved display profiles render as sRGB.
pub fn rgb_to_xyz(d: &DisplayProfileId) -> Mat3 {
    match d {
        DisplayProfileId::Srgb => color::SRGB_TO_XYZ,
        DisplayProfileId::Icc(_) => display_icc(d).map_or(color::SRGB_TO_XYZ, |p| p.rgb_to_xyz),
        DisplayProfileId::DisplayP3 => color::DISPLAY_P3_TO_XYZ,
        DisplayProfileId::AdobeRgb => adobe_rgb_to_xyz(),
        DisplayProfileId::ProPhoto => prophoto_to_xyz(),
        DisplayProfileId::Rec2020 => color::REC2020_TO_XYZ,
    }
}

/// `math::TRANSFER_*` of `d`.
pub fn transfer(d: &DisplayProfileId) -> u32 {
    match d {
        DisplayProfileId::Srgb | DisplayProfileId::DisplayP3 => TRANSFER_SRGB,
        DisplayProfileId::Icc(_) => {
            if display_icc(d).is_some() {
                TRANSFER_TABLE
            } else {
                TRANSFER_SRGB
            }
        }
        DisplayProfileId::AdobeRgb => TRANSFER_ADOBE,
        DisplayProfileId::ProPhoto => TRANSFER_PROPHOTO,
        DisplayProfileId::Rec2020 => TRANSFER_REC709,
    }
}

/// Linear Rec.2020 → linear output primaries.
pub fn e1_matrix(d: &DisplayProfileId) -> Mat3 {
    color::mul(
        &color::invert(&rgb_to_xyz(d)).expect("invertible"),
        &color::REC2020_TO_XYZ,
    )
}

/// E1 for `d`; a display profile's curve tables are appended to the output `aux` buffer.
pub fn e1_params(d: &DisplayProfileId, aux: &mut Vec<f32>) -> E1Params {
    let mut e1 = E1Params {
        matrix: color::to_f32(&e1_matrix(d)),
        transfer: transfer(d),
        table: 0,
        proof: 0,
    };
    if e1.transfer == TRANSFER_TABLE
        && let Some(p) = display_icc(d)
    {
        e1.table = aux.len() as u32;
        aux.extend(p.inverse_trc());
    }
    e1
}

/// Soft proof for `proof` on `display` (`E1Params::proof`): linear Rec.2020 → linear proof
/// RGB, then linear proof RGB → linear display primaries, row-major.
pub fn proof_matrices(display: &DisplayProfileId, proof: &DisplayProfileId) -> Vec<f32> {
    let back = color::mul(
        &color::invert(&rgb_to_xyz(display)).expect("invertible"),
        &rgb_to_xyz(proof),
    );
    [e1_matrix(proof), back]
        .iter()
        .flatten()
        .flatten()
        .map(|v| *v as f32)
        .collect()
}

/// Power that approximately linearises the output curve for resizing, `None` for the sRGB
/// curve (which libvips handles exactly).
pub fn resize_gamma(d: &DisplayProfileId) -> Option<f64> {
    match transfer(d) {
        TRANSFER_ADOBE => Some(563.0 / 256.0),
        TRANSFER_PROPHOTO => Some(1.8),
        TRANSFER_REC709 => Some(1.0 / 0.45),
        _ => None,
    }
}

/// The ICC profile embedded with an export to `d`.
pub enum Icc {
    /// A libvips built-in profile name.
    Builtin(&'static str),
    Data(Arc<[u8]>),
}

pub fn icc(d: &DisplayProfileId) -> Icc {
    static ADOBE: OnceLock<Arc<[u8]>> = OnceLock::new();
    static PROPHOTO: OnceLock<Arc<[u8]>> = OnceLock::new();
    static REC2020: OnceLock<Arc<[u8]>> = OnceLock::new();
    let data = |cell: &'static OnceLock<Arc<[u8]>>, d: &DisplayProfileId| {
        Icc::Data(cell.get_or_init(|| icc_profile(d).into()).clone())
    };
    match d {
        DisplayProfileId::DisplayP3 => Icc::Builtin("p3"),
        DisplayProfileId::Srgb | DisplayProfileId::Icc(_) => Icc::Builtin("srgb"),
        DisplayProfileId::AdobeRgb => data(&ADOBE, d),
        DisplayProfileId::ProPhoto => data(&PROPHOTO, d),
        DisplayProfileId::Rec2020 => data(&REC2020, d),
    }
}

/// Profile description.
fn description(d: &DisplayProfileId) -> &'static str {
    match d {
        DisplayProfileId::AdobeRgb => "Adobe RGB (1998) compatible (aigle)",
        DisplayProfileId::ProPhoto => "ProPhoto RGB (ROMM) (aigle)",
        DisplayProfileId::Rec2020 => "Rec. ITU-R BT.2020 (aigle)",
        DisplayProfileId::DisplayP3 => "Display P3 (aigle)",
        DisplayProfileId::Srgb | DisplayProfileId::Icc(_) => "sRGB IEC61966-2.1 (aigle)",
    }
}

/// ICC parametric curve (`para`) of `transfer`: function type and parameters giving the
/// encoded → linear direction.
fn para(transfer: u32) -> (u16, Vec<f64>) {
    match transfer {
        TRANSFER_ADOBE => (0, vec![563.0 / 256.0]),
        // Y = (aX + b)^g for X ≥ d, cX below.
        TRANSFER_PROPHOTO => (3, vec![1.8, 1.0, 0.0, 1.0 / 16.0, 16.0 / 512.0]),
        TRANSFER_REC709 => (
            3,
            vec![1.0 / 0.45, 1.0 / 1.099, 0.099 / 1.099, 1.0 / 4.5, 0.081],
        ),
        _ => (
            3,
            vec![2.4, 1.0 / 1.055, 0.055 / 1.055, 1.0 / 12.92, 0.040_45],
        ),
    }
}

fn s15f16(v: f64) -> [u8; 4] {
    ((v * 65536.0).round() as i32).to_be_bytes()
}

fn xyz_tag(v: [f64; 3]) -> Vec<u8> {
    let mut b = b"XYZ \0\0\0\0".to_vec();
    for c in v {
        b.extend(s15f16(c));
    }
    b
}

fn mluc(text: &str) -> Vec<u8> {
    let utf16: Vec<u8> = text.encode_utf16().flat_map(u16::to_be_bytes).collect();
    let mut b = b"mluc\0\0\0\0".to_vec();
    b.extend(1u32.to_be_bytes());
    b.extend(12u32.to_be_bytes());
    b.extend(b"enUS");
    b.extend((utf16.len() as u32).to_be_bytes());
    b.extend(28u32.to_be_bytes());
    b.extend(utf16);
    b
}

/// Matrix/TRC display-class ICC v4.3 profile for `d`, D50 PCS, Bradford-adapted colorants.
pub fn icc_profile(d: &DisplayProfileId) -> Vec<u8> {
    let d65 = xy_to_xyz(D65_XY.0, D65_XY.1);
    let d50 = xy_to_xyz(D50_XY.0, D50_XY.1);
    let (native, chad) = match d {
        DisplayProfileId::ProPhoto => (prophoto_to_xyz_d50(), color::IDENTITY),
        _ => (rgb_to_xyz(d), bradford(d65, d50)),
    };
    let pcs = color::mul(&chad, &native);
    let col = |j: usize| [pcs[0][j], pcs[1][j], pcs[2][j]];
    let (kind, params) = para(transfer(d));
    let mut trc = b"para\0\0\0\0".to_vec();
    trc.extend(kind.to_be_bytes());
    trc.extend([0, 0]);
    for p in params {
        trc.extend(s15f16(p));
    }
    let mut sf32 = b"sf32\0\0\0\0".to_vec();
    for row in chad {
        for v in row {
            sf32.extend(s15f16(v));
        }
    }
    // ICC PCS illuminant, exactly as the spec writes it.
    let pcs_white = [0.9642, 1.0, 0.8249];
    let tags: Vec<(&[u8; 4], Vec<u8>)> = vec![
        (b"desc", mluc(description(d))),
        (b"cprt", mluc("No copyright, use freely")),
        (b"wtpt", xyz_tag(pcs_white)),
        (b"chad", sf32),
        (b"rXYZ", xyz_tag(col(0))),
        (b"gXYZ", xyz_tag(col(1))),
        (b"bXYZ", xyz_tag(col(2))),
        (b"rTRC", trc.clone()),
        (b"gTRC", trc.clone()),
        (b"bTRC", trc),
    ];
    let pad = |n: usize| (n + 3) & !3;
    let mut offset = 128 + 4 + 12 * tags.len();
    let mut table = Vec::new();
    let mut data = Vec::new();
    for (sig, body) in &tags {
        table.extend(*sig);
        table.extend((offset as u32).to_be_bytes());
        table.extend((body.len() as u32).to_be_bytes());
        data.extend(body);
        data.resize(data.len() + pad(body.len()) - body.len(), 0);
        offset += pad(body.len());
    }
    let size = offset as u32;
    let mut h = Vec::with_capacity(size as usize);
    h.extend(size.to_be_bytes());
    h.extend([0; 4]); // preferred CMM
    h.extend(0x0430_0000u32.to_be_bytes());
    h.extend(b"mntrRGB XYZ ");
    // Fixed creation date keeps the profile (and exports) reproducible.
    for v in [2026u16, 1, 1, 0, 0, 0] {
        h.extend(v.to_be_bytes());
    }
    h.extend(b"acsp");
    h.extend([0; 4 + 4 + 4 + 4 + 8]); // platform, flags, manufacturer, model, attributes
    h.extend(0u32.to_be_bytes()); // perceptual intent
    for v in pcs_white {
        h.extend(s15f16(v));
    }
    h.extend(b"aigl");
    h.extend([0; 16 + 28]); // profile ID (not computed), reserved
    debug_assert_eq!(h.len(), 128);
    h.extend((tags.len() as u32).to_be_bytes());
    h.extend(table);
    h.extend(data);
    debug_assert_eq!(h.len(), size as usize);
    h
}

#[cfg(test)]
mod tests {
    use super::*;
    use aigle_kernels::math::output_eotf;
    use aigle_vips_sys as vips;

    #[test]
    fn rec2020_is_the_working_space_and_white_is_preserved() {
        let id = e1_matrix(&DisplayProfileId::Rec2020);
        for (i, row) in id.iter().enumerate() {
            for (j, v) in row.iter().enumerate() {
                assert!((v - if i == j { 1.0 } else { 0.0 }).abs() < 1e-12);
            }
        }
        for d in [
            DisplayProfileId::AdobeRgb,
            DisplayProfileId::ProPhoto,
            DisplayProfileId::DisplayP3,
        ] {
            let w = color::mul_vec(&e1_matrix(&d), [1.0; 3]);
            assert!(w.iter().all(|v| (v - 1.0).abs() < 1e-6), "{d:?} {w:?}");
        }
    }

    /// An image encoded for each space and tagged with its generated profile decodes, through
    /// lcms2, to the XYZ the space's matrix and curve predict.
    #[test]
    fn generated_profiles_decode_to_the_rendered_colours() {
        let dir = std::env::temp_dir().join(format!("aigle-icc-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // 16-bit encoded test patches: white, mid grey, a saturated green, a dark red.
        let patches: [[u16; 3]; 4] = [
            [65535, 65535, 65535],
            [30000, 30000, 30000],
            [8000, 50000, 12000],
            [20000, 3000, 2000],
        ];
        for d in [
            DisplayProfileId::AdobeRgb,
            DisplayProfileId::ProPhoto,
            DisplayProfileId::Rec2020,
        ] {
            let profile = icc_profile(&d);
            let fill = |x: u32, _y: u32, w: u32, h: u32, out: &mut [u8], stride: usize| {
                for j in 0..h as usize {
                    for i in 0..w as usize {
                        let p = patches[(x as usize + i) % 4];
                        for (c, v) in p.iter().enumerate() {
                            let at = j * stride + i * 6 + c * 2;
                            out[at..at + 2].copy_from_slice(&v.to_ne_bytes());
                        }
                    }
                }
                true
            };
            let path = dir.join(format!("{d:?}.tif"));
            let opts = vips::SaveOpts {
                width: 4,
                height: 1,
                sixteen_bit: true,
                out_size: (4, 1),
                icc: None,
                exif: None,
                suffix: "[compression=none]",
                sharpen: None,
                icc_data: Some(&profile),
                resize_gamma: None,
            };
            vips::save_file(&opts, &fill, &path).unwrap();
            let (im, hd) = vips::Image::open(&path).unwrap();
            let mut xyz = vec![0f32; 4 * 3];
            im.decode_xyz(&hd, &mut xyz, &|| false).unwrap();
            let m = rgb_to_xyz(&d);
            for (k, p) in patches.iter().enumerate() {
                let lin = p.map(|v| output_eotf(v as f32 / 65535.0, transfer(&d)) as f64);
                let want = color::mul_vec(&m, lin).map(|v| v * 100.0);
                for c in 0..3 {
                    let got = xyz[k * 3 + c] as f64;
                    assert!(
                        (got - want[c]).abs() < 0.6,
                        "{d:?} patch {k} channel {c}: got {got}, want {}",
                        want[c]
                    );
                }
            }
        }
        std::fs::remove_dir_all(&dir).ok();
    }
}
