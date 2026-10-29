//! Export watermarks: a line of text or an image, sized and placed relative to the output,
//! converted from sRGB to the output space and blended by libvips after resizing and
//! sharpening (`aigle_vips_sys::Overlay`).

use aigle_core::color;
use aigle_engine_api::{DisplayProfileId, Watermark, WatermarkContent};
use aigle_kernels::math::{TRANSFER_TABLE, output_oetf, srgb_eotf};
use aigle_vips_sys as vips;

/// Font family of text watermarks (fontconfig picks the system's sans-serif face).
const FONT: &str = "Sans";

/// A watermark ready to blend: RGB in the output's sample format, alpha with the opacity
/// applied.
#[derive(Debug)]
pub struct Prepared {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<u8>,
    pub alpha: Vec<u8>,
}

impl Prepared {
    pub fn overlay(&self) -> vips::Overlay<'_> {
        vips::Overlay {
            x: self.x,
            y: self.y,
            width: self.width,
            height: self.height,
            rgb: &self.rgb,
            alpha: &self.alpha,
        }
    }
}

/// RGBA8 sRGB of the watermark `height` px tall.
fn source(wm: &Watermark, height: u32) -> Result<(u32, u32, Vec<u8>), String> {
    match &wm.content {
        WatermarkContent::Text(text) => {
            let (w, h, mask) = vips::text_mask(text.trim(), FONT, height)
                .map_err(|e| format!("watermark text: {e}"))?;
            let mut rgba = Vec::with_capacity(mask.len() * 4);
            for a in mask {
                rgba.extend_from_slice(&[wm.color[0], wm.color[1], wm.color[2], a]);
            }
            Ok((w, h, rgba))
        }
        WatermarkContent::Image(path) => vips::load_rgba8(path, height)
            .map_err(|e| format!("watermark image {}: {e}", path.display())),
    }
}

/// The watermark for an output of `size` in `output` space, `None` when there is nothing to
/// draw (empty text, zero opacity or size).
pub fn prepare(
    wm: &Watermark,
    size: (u32, u32),
    output: &DisplayProfileId,
    sixteen_bit: bool,
) -> Result<Option<Prepared>, String> {
    let empty = matches!(&wm.content, WatermarkContent::Text(t) if t.trim().is_empty());
    if empty || wm.opacity.is_nan() || wm.opacity <= 0.0 || wm.size.is_nan() || wm.size <= 0.0 {
        return Ok(None);
    }
    let (w_out, h_out) = size;
    let short = w_out.min(h_out) as f32;
    let margin = (wm.margin.clamp(0.0, 0.45) * short).round() as u32;
    let room_w = w_out.saturating_sub(2 * margin).max(1);
    let room_h = h_out.saturating_sub(2 * margin).max(1);
    let target = ((wm.size.min(1.0) * short).round() as u32).clamp(1, room_h);
    let (mut w, mut h, mut rgba) = source(wm, target)?;
    let mut tries = 0;
    while w > room_w {
        tries += 1;
        let fit = ((h as u64 * room_w as u64) / w as u64).max(1) as u32;
        let fit = if tries > 1 {
            fit.saturating_sub(tries).max(1)
        } else {
            fit
        };
        (w, h, rgba) = source(wm, fit)?;
        if tries == 4 && w > room_w {
            return Err("watermark does not fit the output".into());
        }
    }
    let (gx, gy) = wm.anchor.grid();
    let place = |room: u32, len: u32, g: u32| match g {
        0 => margin,
        1 => (room - len) / 2,
        _ => room - len - margin,
    };
    let x = place(w_out, w, gx);
    let y = place(h_out, h, gy);
    let opacity = wm.opacity.min(1.0);
    let alpha = rgba
        .as_chunks::<4>()
        .0
        .iter()
        .map(|p| (p[3] as f32 * opacity).round() as u8)
        .collect();
    let rgb = to_output(&rgba, output, sixteen_bit);
    Ok(Some(Prepared {
        x,
        y,
        width: w,
        height: h,
        rgb,
        alpha,
    }))
}

/// sRGB8 samples of RGBA pixels → RGB in the output space, 8-bit or native-endian 16-bit.
fn to_output(rgba: &[u8], output: &DisplayProfileId, sixteen_bit: bool) -> Vec<u8> {
    let transfer = crate::output_space::transfer(output);
    let srgb = matches!(output, DisplayProfileId::Srgb) || transfer == TRANSFER_TABLE;
    let m = color::to_f32(&color::mul(
        &color::invert(&crate::output_space::rgb_to_xyz(output)).expect("invertible"),
        &color::SRGB_TO_XYZ,
    ));
    let lut: Vec<f32> = (0..256).map(|i| srgb_eotf(i as f32 / 255.0)).collect();
    let mut out = Vec::with_capacity(rgba.len() / 4 * 3 * if sixteen_bit { 2 } else { 1 });
    for p in rgba.as_chunks::<4>().0.iter() {
        let enc: [f32; 3] = if srgb {
            [0, 1, 2].map(|c| p[c] as f32 / 255.0)
        } else {
            let lin = [0, 1, 2].map(|c| lut[p[c] as usize]);
            [0, 1, 2].map(|r| {
                let v = m[r][0] * lin[0] + m[r][1] * lin[1] + m[r][2] * lin[2];
                output_oetf(v.clamp(0.0, 1.0), transfer)
            })
        };
        for v in enc {
            if sixteen_bit {
                out.extend_from_slice(&((v * 65535.0).round() as u16).to_ne_bytes());
            } else {
                out.push((v * 255.0).round() as u8);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use aigle_engine_api::Anchor;

    fn text(anchor: Anchor) -> Watermark {
        Watermark {
            content: WatermarkContent::Text("© aigle".into()),
            anchor,
            ..Watermark::default()
        }
    }

    #[test]
    fn placed_inside_the_margins() {
        let size = (1000, 600);
        let p = prepare(
            &text(Anchor::BottomRight),
            size,
            &DisplayProfileId::Srgb,
            false,
        )
        .unwrap()
        .unwrap();
        let margin = 18; // 0.03 × 600
        assert_eq!(p.height, 24); // 0.04 × 600
        assert_eq!(p.x + p.width, 1000 - margin);
        assert_eq!(p.y + p.height, 600 - margin);
        assert_eq!(p.rgb.len(), p.alpha.len() * 3);
        // 60 % opacity: no pixel more opaque than 153.
        assert!(p.alpha.iter().all(|&a| a <= 153) && p.alpha.contains(&153));
        let c = prepare(&text(Anchor::Center), size, &DisplayProfileId::Srgb, false)
            .unwrap()
            .unwrap();
        assert_eq!(c.x, (1000 - c.width) / 2);
        let tl = prepare(&text(Anchor::TopLeft), size, &DisplayProfileId::Srgb, true)
            .unwrap()
            .unwrap();
        assert_eq!((tl.x, tl.y), (margin, margin));
        assert_eq!(tl.rgb.len(), tl.alpha.len() * 6);
    }

    #[test]
    fn long_text_shrinks_to_fit() {
        let wm = Watermark {
            content: WatermarkContent::Text("a very long watermark line ".repeat(8)),
            size: 0.2,
            ..Watermark::default()
        };
        let p = prepare(&wm, (400, 300), &DisplayProfileId::Srgb, false)
            .unwrap()
            .unwrap();
        assert!(
            p.width <= 400 - 2 * 9 && p.height < 60,
            "{}×{}",
            p.width,
            p.height
        );
    }

    #[test]
    fn nothing_to_draw() {
        let mut wm = text(Anchor::Center);
        wm.opacity = 0.0;
        assert!(
            prepare(&wm, (100, 100), &DisplayProfileId::Srgb, false)
                .unwrap()
                .is_none()
        );
        wm = Watermark::default();
        assert!(
            prepare(&wm, (100, 100), &DisplayProfileId::Srgb, false)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn colour_converted_to_the_output_space() {
        let red = [255u8, 0, 0, 255];
        let srgb = to_output(&red, &DisplayProfileId::Srgb, false);
        assert_eq!(srgb, [255, 0, 0]);
        let p3 = to_output(&red, &DisplayProfileId::DisplayP3, false);
        assert!(p3[0] < 245 && p3[1] > 40 && p3[2] > 20, "{p3:?}");
        let grey = to_output(&[128, 128, 128, 255], &DisplayProfileId::AdobeRgb, false);
        assert!(grey[0].abs_diff(grey[1]) <= 1 && grey[1].abs_diff(grey[2]) <= 1);
    }
}
