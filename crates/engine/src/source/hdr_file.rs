//! CICP detection for HDR PNG/HEIF files, including libvips versions which discard NCLX.
//! Only box/chunk headers and bounded metadata are read; media payloads are skipped.
use aigle_core::color;
use aigle_kernels::Rgb;
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::Path,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cicp {
    pub primaries: u16,
    pub transfer: u16,
}
impl Cicp {
    pub fn hdr(self) -> bool {
        matches!(self.transfer, 16 | 18)
    }
    pub fn matrix(self) -> Result<[[f32; 3]; 3], String> {
        let native = match self.primaries {
            1 => color::SRGB_TO_XYZ,
            9 => color::REC2020_TO_XYZ,
            12 => color::DISPLAY_P3_TO_XYZ,
            _ => {
                return Err(format!(
                    "HDR CICP primaries {} are unsupported",
                    self.primaries
                ));
            }
        };
        Ok(color::to_f32(&color::mul(
            &color::invert(&color::REC2020_TO_XYZ).unwrap(),
            &native,
        )))
    }
    pub fn decode(self, rgb: Rgb, matrix: &[[f32; 3]; 3]) -> Rgb {
        if self.transfer == 16 {
            aigle_kernels::math::mat3_mul(
                matrix,
                rgb.map(|v| crate::hdr::pq_decode(v) / crate::hdr::REFERENCE_WHITE),
            )
        } else {
            let scene = rgb.map(|v| {
                let v = v.clamp(0.0, 1.0);
                if v <= 0.5 {
                    v * v / 3.0
                } else {
                    (((v - 0.559_910_7) / 0.178_832_77).exp() + 0.284_668_92) / 12.0
                }
            });
            let linear = aigle_kernels::math::mat3_mul(matrix, scene);
            let gain = aigle_kernels::math::luminance(linear).max(0.0).powf(0.2) * 1000.0
                / crate::hdr::REFERENCE_WHITE;
            linear.map(|v| v * gain)
        }
    }
}

pub fn detect(path: &Path, im: &aigle_vips_sys::Image) -> Result<Option<Cicp>, String> {
    let number = |key| im.get_string(key).and_then(|s| s.parse::<u16>().ok());
    if let (Some(primaries), Some(transfer)) = (
        number("cicp-colour-primaries"),
        number("cicp-transfer-characteristics"),
    ) {
        return Ok(Some(Cicp {
            primaries,
            transfer,
        })
        .filter(|c| c.hdr()));
    }
    if let Some(c) = im.icc_cicp() {
        let c = Cicp {
            primaries: c[0] as u16,
            transfer: c[1] as u16,
        };
        if c.hdr() {
            return Ok(Some(c));
        }
    }
    let mut file = File::open(path).map_err(|e| e.to_string())?;
    let length = file.metadata().map_err(|e| e.to_string())?.len();
    let mut signature = [0; 12];
    if file.read_exact(&mut signature).is_err() {
        return Ok(None);
    }
    let result = if &signature[..8] == b"\x89PNG\r\n\x1a\n" {
        png(&mut file, length)
    } else if &signature[4..8] == b"ftyp" {
        heif(&mut file, length)
    } else {
        Ok(None)
    }?;
    Ok(result.filter(|c| c.hdr()))
}
fn png(f: &mut File, end: u64) -> Result<Option<Cicp>, String> {
    let mut at = 8;
    while at + 12 <= end {
        f.seek(SeekFrom::Start(at)).map_err(|e| e.to_string())?;
        let mut head = [0; 8];
        f.read_exact(&mut head).map_err(|e| e.to_string())?;
        let len = u32::from_be_bytes(head[..4].try_into().unwrap()) as u64;
        let next = at
            .checked_add(12 + len)
            .filter(|n| *n <= end)
            .ok_or("invalid PNG chunk length")?;
        if &head[4..] == b"cICP" {
            if len != 4 {
                return Err("invalid PNG cICP chunk".into());
            }
            let mut c = [0; 4];
            f.read_exact(&mut c).map_err(|e| e.to_string())?;
            if c[2] != 0 || c[3] != 1 {
                return Err("PNG HDR requires full-range RGB cICP".into());
            }
            return Ok(Some(Cicp {
                primaries: c[0] as u16,
                transfer: c[1] as u16,
            }));
        }
        if matches!(&head[4..], b"IDAT" | b"IEND") {
            break;
        }
        at = next;
    }
    Ok(None)
}

fn heif(file: &mut File, length: u64) -> Result<Option<Cicp>, String> {
    let mut at = 0u64;
    while at + 8 <= length {
        file.seek(SeekFrom::Start(at)).map_err(|e| e.to_string())?;
        let mut head = [0; 8];
        file.read_exact(&mut head).map_err(|e| e.to_string())?;
        let mut len = u32::from_be_bytes(head[..4].try_into().unwrap()) as u64;
        let mut header = 8;
        if len == 1 {
            let mut wide = [0; 8];
            file.read_exact(&mut wide).map_err(|e| e.to_string())?;
            len = u64::from_be_bytes(wide);
            header = 16;
        } else if len == 0 {
            len = length - at;
        }
        let next = at
            .checked_add(len)
            .filter(|n| *n <= length && len >= header)
            .ok_or("invalid HEIF box length")?;
        if &head[4..] == b"meta" {
            // Do not bring mdat or unbounded metadata into memory just to inspect color.
            if len - header > 16 * 1024 * 1024 {
                return Err("HEIF color metadata exceeds 16 MiB".into());
            }
            let mut meta = vec![0; (len - header) as usize];
            file.read_exact(&mut meta).map_err(|e| e.to_string())?;
            return heif_meta(&meta);
        }
        at = next;
    }
    Ok(None)
}
type BoxChildren<'a> = Vec<([u8; 4], &'a [u8])>;
fn children(mut bytes: &[u8]) -> Result<BoxChildren<'_>, String> {
    let mut out = Vec::new();
    while !bytes.is_empty() {
        if bytes.len() < 8 {
            return Err("truncated HEIF metadata box".into());
        }
        let mut len = u32::from_be_bytes(bytes[..4].try_into().unwrap()) as u64;
        let mut header = 8;
        if len == 1 {
            if bytes.len() < 16 {
                return Err("truncated HEIF extended box".into());
            }
            len = u64::from_be_bytes(bytes[8..16].try_into().unwrap());
            header = 16;
        } else if len == 0 {
            len = bytes.len() as u64;
        }
        if len < header as u64 || len > bytes.len() as u64 {
            return Err("invalid HEIF metadata length".into());
        }
        out.push((
            bytes[4..8].try_into().unwrap(),
            &bytes[header..len as usize],
        ));
        bytes = &bytes[len as usize..];
    }
    Ok(out)
}
fn nclx(b: &[u8]) -> Option<Cicp> {
    (b.len() >= 11 && &b[..4] == b"nclx").then(|| Cicp {
        primaries: u16::from_be_bytes([b[4], b[5]]),
        transfer: u16::from_be_bytes([b[6], b[7]]),
    })
}
fn heif_meta(bytes: &[u8]) -> Result<Option<Cicp>, String> {
    use std::collections::HashMap;
    let full = bytes.get(4..).ok_or("truncated HEIF meta")?;
    let mut primary = None;
    let mut properties = Vec::new();
    let mut associations = HashMap::<u32, Vec<usize>>::new();
    for (kind, data) in children(full)? {
        match &kind {
            b"pitm" => {
                if data.len() < 6 {
                    return Err("truncated HEIF primary item".into());
                }
                primary = Some(if data[0] == 0 {
                    u16::from_be_bytes([data[4], data[5]]) as u32
                } else {
                    u32::from_be_bytes(
                        data.get(4..8)
                            .ok_or("truncated HEIF primary item")?
                            .try_into()
                            .unwrap(),
                    )
                });
            }
            b"iprp" => {
                for (kind, data) in children(data)? {
                    match &kind {
                        b"ipco" => {
                            for (kind, data) in children(data)? {
                                properties.push(if &kind == b"colr" { nclx(data) } else { None });
                            }
                        }
                        b"ipma" => {
                            if data.len() < 8 {
                                return Err("truncated HEIF property associations".into());
                            }
                            let wide_item = data[0] != 0;
                            let wide_property = data[3] & 1 != 0;
                            let count = u32::from_be_bytes(data[4..8].try_into().unwrap());
                            let mut data = &data[8..];
                            for _ in 0..count {
                                let n = if wide_item { 4 } else { 2 };
                                let id = data.get(..n).ok_or("truncated HEIF item id")?;
                                let id = if wide_item {
                                    u32::from_be_bytes(id.try_into().unwrap())
                                } else {
                                    u16::from_be_bytes(id.try_into().unwrap()) as u32
                                };
                                let count = *data.get(n).ok_or("truncated HEIF property count")?;
                                data = &data[n + 1..];
                                for _ in 0..count {
                                    let n = if wide_property { 2 } else { 1 };
                                    let p = data.get(..n).ok_or("truncated HEIF property index")?;
                                    let p = if wide_property {
                                        u16::from_be_bytes(p.try_into().unwrap()) & 0x7fff
                                    } else {
                                        (p[0] & 0x7f) as u16
                                    };
                                    if p != 0 {
                                        associations.entry(id).or_default().push(p as usize - 1);
                                    }
                                    data = &data[n..];
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    if let Some(indices) = primary.and_then(|id| associations.get(&id)) {
        let colors: Vec<_> = indices
            .iter()
            .filter_map(|&i| properties.get(i).copied().flatten())
            .collect();
        if let Some(first) = colors.first() {
            if colors.iter().any(|c| c != first) {
                return Err("conflicting primary-item HDR color properties".into());
            }
            return Ok(Some(*first));
        }
    }
    // Derived grid images may place the color description on their tile items. A
    // unanimous profile is safe; never apply an auxiliary item's different profile.
    let mut colors = properties.into_iter().flatten();
    let first = colors.next();
    if colors.any(|c| Some(c) != first) {
        return Err("ambiguous HEIF derived-image color properties".into());
    }
    Ok(first)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn atom(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut b = ((data.len() + 8) as u32).to_be_bytes().to_vec();
        b.extend(kind);
        b.extend(data);
        b
    }
    #[test]
    fn heif_uses_primary_item_instead_of_auxiliary_color_profile() {
        let mut props = atom(b"colr", b"nclx\0\x09\0\x10\0\x09\x80");
        props.extend(atom(b"colr", b"nclx\0\x01\0\x0d\0\0\x80"));
        let mut iprp = atom(b"ipco", &props);
        iprp.extend(atom(
            b"ipma",
            &[0, 0, 0, 0, 0, 0, 0, 2, 0, 7, 1, 0x81, 0, 8, 1, 0x82],
        ));
        let mut meta = vec![0; 4];
        meta.extend(atom(b"pitm", &[0, 0, 0, 0, 0, 7]));
        meta.extend(atom(b"iprp", &iprp));
        assert_eq!(
            heif_meta(&meta).unwrap(),
            Some(Cicp {
                primaries: 9,
                transfer: 16
            })
        );
    }
    #[test]
    fn malformed_metadata_is_rejected_without_unbounded_allocation() {
        assert!(children(&[0, 0, 0, 99, b'c', b'o', b'l', b'r']).is_err());
        assert!(heif_meta(&[0; 3]).is_err());
    }
}
