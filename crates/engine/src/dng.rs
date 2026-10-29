//! DNG export: a raw's sensor data written as a DNG 1.4 file (lossless JPEG tiles,
//! Compression 7) with its colour matrix, as-shot white balance, EXIF and a rendered
//! preview. The edits are not baked in; the app copies them into the DNG's sidecar.
//!
//! Layout: IFD0 holds a small RGB thumbnail and the DNG tags, SubIFD 1 the CFA data,
//! SubIFD 2 a JPEG preview, plus the EXIF and GPS IFDs. Previews are stored in sensor
//! orientation; readers apply IFD0's Orientation to all of them.

use aigle_core::color::{self, Mat3};
use aigle_engine_api::{CameraInfo, ExportOptions, MetadataPolicy};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

/// Sensor data of a Bayer or X-Trans raw (`source::raw::mosaic`).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Mosaic {
    /// Visible area, sensor orientation.
    pub width: u32,
    pub height: u32,
    /// CFA colour (0 R, 1 G, 2 B) at (row % 6, col % 6).
    pub cfa: [[u8; 6]; 6],
    pub xtrans: bool,
    pub black: u16,
    pub white: u16,
    /// EXIF orientation of the sensor data.
    pub orientation: u16,
    /// XYZ (D65) → camera; `None` for a camera LibRaw has no matrix for.
    pub xyz_to_cam: Option<Mat3>,
    /// As-shot multipliers, green = 1.
    pub as_shot: [f64; 3],
    pub baseline_exposure: Option<f32>,
    pub camera: CameraInfo,
    #[serde(skip)]
    pub data: Vec<u16>,
}

/// A rendered preview in sensor orientation, 8-bit sRGB.
pub struct Preview {
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<u8>,
}

// ---------------------------------------------------------------------------------------------
// Lossless JPEG (ITU T.81 process 14, predictor 1), two interleaved components per row as
// Adobe's converter writes it: same-colour neighbours of a Bayer row are two samples apart.

const TILE: u32 = 256;

struct Bits {
    out: Vec<u8>,
    acc: u32,
    n: u32,
}

impl Bits {
    fn put(&mut self, code: u32, len: u32) {
        if len == 0 {
            return;
        }
        self.acc = (self.acc << len) | (code & ((1 << len) - 1));
        self.n += len;
        while self.n >= 8 {
            self.n -= 8;
            let b = (self.acc >> self.n) as u8;
            self.out.push(b);
            if b == 0xff {
                self.out.push(0);
            }
        }
        self.acc &= (1 << self.n) - 1;
    }

    fn finish(mut self) -> Vec<u8> {
        if self.n > 0 {
            let pad = 8 - self.n;
            self.put((1 << pad) - 1, pad);
        }
        self.out
    }
}

/// Difference category (number of magnitude bits); -32768 is category 16.
fn category(d: i32) -> u32 {
    32 - d.unsigned_abs().leading_zeros()
}

/// Code lengths (BITS, HUFFVAL) of an optimal Huffman code limited to 16 bits for symbol
/// frequencies `freq` (T.81 Annex K.2 and K.3).
fn huffman(freq: &[u64; 17]) -> ([u8; 16], Vec<u8>) {
    let mut f = [0u64; 18];
    f[..17].copy_from_slice(freq);
    f[17] = 1; // reserved so that no code is all ones
    let mut size = [0u32; 18];
    let mut others = [-1i32; 18];
    loop {
        let mut v1: i32 = -1;
        let mut v2: i32 = -1;
        for i in 0..18 {
            if f[i] == 0 {
                continue;
            }
            if v1 < 0 || f[i] <= f[v1 as usize] {
                v2 = v1;
                v1 = i as i32;
            } else if v2 < 0 || f[i] <= f[v2 as usize] {
                v2 = i as i32;
            }
        }
        if v2 < 0 {
            break;
        }
        let (mut a, mut b) = (v1 as usize, v2 as usize);
        f[a] += f[b];
        f[b] = 0;
        size[a] += 1;
        while others[a] >= 0 {
            a = others[a] as usize;
            size[a] += 1;
        }
        others[a] = b as i32;
        size[b] += 1;
        while others[b] >= 0 {
            b = others[b] as usize;
            size[b] += 1;
        }
    }
    let mut bits = [0u32; 40];
    for &s in &size {
        if s > 0 {
            bits[s as usize] += 1;
        }
    }
    let mut i = 39;
    while i > 16 {
        while bits[i] > 0 {
            let mut j = i - 2;
            while bits[j] == 0 {
                j -= 1;
            }
            bits[i] -= 2;
            bits[i - 1] += 1;
            bits[j + 1] += 2;
            bits[j] -= 1;
        }
        i -= 1;
    }
    while bits[i] == 0 {
        i -= 1;
    }
    bits[i] -= 1; // drop the reserved symbol (it has the longest code)
    let mut vals = Vec::new();
    for len in 1..40 {
        for (s, &sz) in size.iter().enumerate().take(17) {
            if sz as usize == len {
                vals.push(s as u8);
            }
        }
    }
    let mut out = [0u8; 16];
    for (o, b) in out.iter_mut().zip(&bits[1..17]) {
        *o = *b as u8;
    }
    vals.truncate(out.iter().map(|&b| b as usize).sum());
    (out, vals)
}

/// Encode a `w × h` tile (w even) of 16-bit samples.
pub fn lj92(tile: &[u16], w: u32, h: u32) -> Vec<u8> {
    debug_assert!(w.is_multiple_of(2) && tile.len() == (w * h) as usize);
    let (w, h) = (w as usize, h as usize);
    let diff = |i: usize| -> i32 {
        let (x, y) = (i % w, i / w);
        let pred = if x >= 2 {
            tile[i - 2]
        } else if y > 0 {
            tile[i - w]
        } else {
            1 << 15
        };
        (tile[i].wrapping_sub(pred) as i16) as i32
    };
    let mut freq = [0u64; 17];
    for i in 0..w * h {
        freq[category(diff(i)) as usize] += 1;
    }
    let (bits, vals) = huffman(&freq);
    let mut code = [(0u32, 0u32); 17];
    let (mut c, mut k) = (0u32, 0usize);
    for (len, &n) in bits.iter().enumerate() {
        for _ in 0..n {
            code[vals[k] as usize] = (c, len as u32 + 1);
            c += 1;
            k += 1;
        }
        c <<= 1;
    }

    let mut out = vec![0xff, 0xd8];
    let (fw, fh) = ((w / 2) as u16, h as u16);
    out.extend_from_slice(&[0xff, 0xc4]);
    out.extend_from_slice(&((2 + 1 + 16 + vals.len()) as u16).to_be_bytes());
    out.push(0x00);
    out.extend_from_slice(&bits);
    out.extend_from_slice(&vals);
    out.extend_from_slice(&[0xff, 0xc3, 0, 14, 16]);
    out.extend_from_slice(&fh.to_be_bytes());
    out.extend_from_slice(&fw.to_be_bytes());
    out.extend_from_slice(&[2, 1, 0x11, 0, 2, 0x11, 0]);
    out.extend_from_slice(&[0xff, 0xda, 0, 10, 2, 1, 0x00, 2, 0x00, 1, 0, 0]);
    let mut b = Bits { out, acc: 0, n: 0 };
    for i in 0..w * h {
        let d = diff(i);
        let s = category(d);
        let (cd, len) = code[s as usize];
        b.put(cd, len);
        if s > 0 && s < 16 {
            let extra = if d < 0 { d - 1 } else { d } as u32;
            b.put(extra, s);
        }
    }
    let mut out = b.finish();
    out.extend_from_slice(&[0xff, 0xd9]);
    out
}

/// The tiles of the mosaic, row-major, each [`TILE`]² with edge pixels repeated.
fn tiles(m: &Mosaic) -> Vec<Vec<u8>> {
    let (w, h) = (m.width as usize, m.height as usize);
    let (tx, ty) = (w.div_ceil(TILE as usize), h.div_ceil(TILE as usize));
    (0..tx * ty)
        .into_par_iter()
        .map(|t| {
            let (x0, y0) = ((t % tx) * TILE as usize, (t / tx) * TILE as usize);
            let mut buf = vec![0u16; (TILE * TILE) as usize];
            // Beyond the edge, repeat the last pixels of the same CFA phase.
            let clamp = |v: usize, len: usize| {
                if v < len {
                    v
                } else {
                    v - 2 * (v - (len - 1)).div_ceil(2)
                }
            };
            for j in 0..TILE as usize {
                let y = clamp(y0 + j, h);
                for i in 0..TILE as usize {
                    buf[j * TILE as usize + i] = m.data[y * w + clamp(x0 + i, w)];
                }
            }
            lj92(&buf, TILE, TILE)
        })
        .collect()
}

// ---------------------------------------------------------------------------------------------
// TIFF

#[derive(Clone, Copy)]
enum Ty {
    Byte = 1,
    Ascii = 2,
    Short = 3,
    Long = 4,
    Rational = 5,
    SRational = 10,
}

enum Value {
    Data {
        ty: Ty,
        count: u32,
        bytes: Vec<u8>,
    },
    /// LONG offsets of blobs, by index.
    Blobs(Vec<usize>),
    /// LONG offsets of IFDs, by index.
    Ifds(Vec<usize>),
}

struct Ifd(Vec<(u16, Value)>);

impl Ifd {
    fn data(&mut self, tag: u16, ty: Ty, count: u32, bytes: Vec<u8>) {
        self.0.push((tag, Value::Data { ty, count, bytes }));
    }
    fn short(&mut self, tag: u16, v: &[u16]) {
        let b = v.iter().flat_map(|x| x.to_le_bytes()).collect();
        self.data(tag, Ty::Short, v.len() as u32, b);
    }
    fn long(&mut self, tag: u16, v: &[u32]) {
        let b = v.iter().flat_map(|x| x.to_le_bytes()).collect();
        self.data(tag, Ty::Long, v.len() as u32, b);
    }
    fn byte(&mut self, tag: u16, v: &[u8]) {
        self.data(tag, Ty::Byte, v.len() as u32, v.to_vec());
    }
    fn ascii(&mut self, tag: u16, s: &str) {
        let mut b: Vec<u8> = s.bytes().filter(|&c| c != 0).collect();
        b.push(0);
        self.data(tag, Ty::Ascii, b.len() as u32, b);
    }
    fn ascii_opt(&mut self, tag: u16, s: Option<&str>) {
        if let Some(s) = s.map(str::trim).filter(|s| !s.is_empty()) {
            self.ascii(tag, s);
        }
    }
    fn rational(&mut self, tag: u16, v: &[f64]) {
        let b = v.iter().flat_map(|&x| {
            let (n, d) = urational(x);
            n.to_le_bytes().into_iter().chain(d.to_le_bytes())
        });
        self.data(tag, Ty::Rational, v.len() as u32, b.collect());
    }
    fn srational(&mut self, tag: u16, v: &[f64]) {
        let b = v.iter().flat_map(|&x| {
            let n = (x * 10000.0)
                .round()
                .clamp(i32::MIN as f64, i32::MAX as f64) as i32;
            n.to_le_bytes().into_iter().chain(10000i32.to_le_bytes())
        });
        self.data(tag, Ty::SRational, v.len() as u32, b.collect());
    }
    fn len_of(v: &Value) -> usize {
        match v {
            Value::Data { bytes, .. } => bytes.len(),
            Value::Blobs(b) => b.len() * 4,
            Value::Ifds(i) => i.len() * 4,
        }
    }
    fn size(&self) -> usize {
        2 + 12 * self.0.len()
            + 4
            + self
                .0
                .iter()
                .map(|(_, v)| Ifd::len_of(v))
                .filter(|&n| n > 4)
                .map(|n| n + n % 2)
                .sum::<usize>()
    }
}

fn urational(x: f64) -> (u32, u32) {
    if x.is_nan() || x <= 0.0 {
        return (0, 1);
    }
    if x < 1.0 {
        let d = (1.0 / x).round();
        if ((1.0 / d) - x).abs() < x * 1e-3 {
            return (1, d as u32);
        }
    }
    let n = (x * 10000.0).round().min(u32::MAX as f64) as u32;
    (n, 10000)
}

/// Serialise little-endian TIFF: IFDs first (IFD 0 is the first IFD), then the blobs.
fn tiff(mut ifds: Vec<Ifd>, blobs: &[&[u8]]) -> Vec<u8> {
    for ifd in &mut ifds {
        ifd.0.sort_by_key(|(t, _)| *t);
    }
    let mut ifd_at = Vec::with_capacity(ifds.len());
    let mut at = 8usize;
    for ifd in &ifds {
        ifd_at.push(at);
        at += ifd.size();
    }
    let mut blob_at = Vec::with_capacity(blobs.len());
    for b in blobs {
        at += at % 2;
        blob_at.push(at);
        at += b.len();
    }
    let mut out = Vec::with_capacity(at);
    out.extend_from_slice(b"II*\0");
    out.extend_from_slice(&8u32.to_le_bytes());
    for (k, ifd) in ifds.iter().enumerate() {
        debug_assert_eq!(out.len(), ifd_at[k]);
        let mut ext = ifd_at[k] + 2 + 12 * ifd.0.len() + 4;
        let mut tail = Vec::new();
        out.extend_from_slice(&(ifd.0.len() as u16).to_le_bytes());
        for (tag, v) in &ifd.0 {
            let (ty, count, bytes) = match v {
                Value::Data { ty, count, bytes } => (*ty as u16, *count, bytes.clone()),
                Value::Blobs(ix) => (
                    Ty::Long as u16,
                    ix.len() as u32,
                    ix.iter()
                        .flat_map(|&i| (blob_at[i] as u32).to_le_bytes())
                        .collect(),
                ),
                Value::Ifds(ix) => (
                    Ty::Long as u16,
                    ix.len() as u32,
                    ix.iter()
                        .flat_map(|&i| (ifd_at[i] as u32).to_le_bytes())
                        .collect(),
                ),
            };
            out.extend_from_slice(&tag.to_le_bytes());
            out.extend_from_slice(&ty.to_le_bytes());
            out.extend_from_slice(&count.to_le_bytes());
            if bytes.len() <= 4 {
                let mut v = [0u8; 4];
                v[..bytes.len()].copy_from_slice(&bytes);
                out.extend_from_slice(&v);
            } else {
                out.extend_from_slice(&(ext as u32).to_le_bytes());
                ext += bytes.len() + bytes.len() % 2;
                tail.extend_from_slice(&bytes);
                if bytes.len() % 2 == 1 {
                    tail.push(0);
                }
            }
        }
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&tail);
    }
    for (b, &at) in blobs.iter().zip(&blob_at) {
        out.resize(at, 0);
        out.extend_from_slice(b);
    }
    out
}

// ---------------------------------------------------------------------------------------------

/// Tags of the EXIF IFD for `cam`.
fn exif_ifd(cam: &CameraInfo) -> Ifd {
    let mut e = Ifd(Vec::new());
    if let Some(v) = cam.exposure_time {
        e.rational(33434, &[v as f64]);
    }
    if let Some(v) = cam.f_number {
        e.rational(33437, &[v as f64]);
    }
    if let Some(v) = cam.iso {
        e.short(34855, &[v.round().clamp(1.0, 65535.0) as u16]);
    }
    e.byte(36864, b"0231");
    e.ascii_opt(36867, cam.shot.date_time.as_deref());
    e.ascii_opt(36881, cam.shot.utc_offset.as_deref());
    if let Some(v) = cam.shot.exposure_bias {
        e.srational(37380, &[v as f64]);
    }
    if let Some(v) = cam.focal_length {
        e.rational(37386, &[v as f64]);
    }
    if let Some(v) = cam.focal_length_35mm {
        e.short(41989, &[v.round().clamp(1.0, 65535.0) as u16]);
    }
    e.ascii_opt(42033, cam.shot.body_serial.as_deref());
    if let Some((lo, hi)) = cam.lens_focal_range {
        e.rational(42034, &[lo as f64, hi as f64, 0.0, 0.0]);
    }
    e.ascii_opt(42035, cam.lens_make.as_deref());
    e.ascii_opt(42036, cam.lens_model.as_deref());
    e.ascii_opt(42037, cam.shot.lens_serial.as_deref());
    e
}

fn gps_ifd(lat: f64, lon: f64, alt: Option<f32>) -> Ifd {
    let dms = |v: f64| {
        let v = v.abs();
        let d = v.trunc();
        let m = ((v - d) * 60.0).trunc();
        [d, m, ((v - d) * 60.0 - m) * 60.0]
    };
    let mut g = Ifd(Vec::new());
    g.byte(0, &[2, 3, 0, 0]);
    g.ascii(1, if lat < 0.0 { "S" } else { "N" });
    g.rational(2, &dms(lat));
    g.ascii(3, if lon < 0.0 { "W" } else { "E" });
    g.rational(4, &dms(lon));
    if let Some(a) = alt {
        g.byte(5, &[(a < 0.0) as u8]);
        g.rational(6, &[a.abs() as f64]);
    }
    g
}

/// The DNG file for `m` with `preview` under `opts`' metadata policy and artist/copyright.
pub fn build(m: &Mosaic, preview: Option<&Preview>, opts: &ExportOptions) -> Vec<u8> {
    let tiles = tiles(m);
    let cam = &m.camera;
    let thumb = preview.map(|p| thumbnail(p, 256));
    let jpeg = preview.and_then(|p| {
        let bgra: Vec<u8> = p
            .rgb
            .as_chunks::<3>()
            .0
            .iter()
            .flat_map(|c| [c[2], c[1], c[0], 255])
            .collect();
        crate::encode::encode(
            p.width,
            p.height,
            &bgra,
            aigle_engine_api::EncodedFormat::Jpeg { quality: 85 },
            &aigle_engine_api::DisplayProfileId::Srgb,
            None,
        )
        .map_err(|e| tracing::warn!("DNG preview: {e}"))
        .ok()
        .map(|b| (p.width, p.height, b))
    });

    let mut blobs: Vec<&[u8]> = tiles.iter().map(Vec::as_slice).collect();
    let n_tiles = blobs.len();
    let mut ifds = vec![Ifd(Vec::new())];
    let mut raw = Ifd(Vec::new());
    raw.long(254, &[0]);
    raw.long(256, &[m.width]);
    raw.long(257, &[m.height]);
    raw.short(258, &[16]);
    raw.short(259, &[7]);
    raw.short(262, &[32803]);
    raw.short(277, &[1]);
    raw.short(284, &[1]);
    raw.long(322, &[TILE]);
    raw.long(323, &[TILE]);
    raw.0.push((324, Value::Blobs((0..n_tiles).collect())));
    raw.long(
        325,
        &tiles.iter().map(|t| t.len() as u32).collect::<Vec<_>>(),
    );
    let dim = if m.xtrans { 6 } else { 2 };
    raw.short(33421, &[dim as u16, dim as u16]);
    let pattern: Vec<u8> = (0..dim)
        .flat_map(|y| (0..dim).map(move |x| (y, x)))
        .map(|(y, x)| m.cfa[y][x])
        .collect();
    raw.byte(33422, &pattern);
    raw.byte(50710, &[0, 1, 2]);
    raw.short(50711, &[1]);
    raw.short(50713, &[1, 1]);
    raw.long(50714, &[m.black as u32]);
    raw.long(50717, &[m.white as u32]);
    raw.rational(50718, &[1.0, 1.0]);
    raw.long(50719, &[0, 0]);
    raw.long(50720, &[m.width, m.height]);
    ifds.push(raw);
    let mut subs = vec![1];

    if let Some((w, h, bytes)) = &jpeg {
        let mut p = Ifd(Vec::new());
        p.long(254, &[1]);
        p.long(256, &[*w]);
        p.long(257, &[*h]);
        p.short(258, &[8, 8, 8]);
        p.short(259, &[7]);
        p.short(262, &[6]);
        p.0.push((273, Value::Blobs(vec![blobs.len()])));
        p.short(277, &[3]);
        p.long(278, &[*h]);
        p.long(279, &[bytes.len() as u32]);
        p.short(284, &[1]);
        p.short(530, &[2, 2]);
        p.long(50970, &[2]);
        blobs.push(bytes);
        subs.push(ifds.len());
        ifds.push(p);
    }

    let full = opts.metadata == MetadataPolicy::All;
    let keep_rights = opts.metadata != MetadataPolicy::Strip;
    let pick = |over: &Option<String>, own: &Option<String>| {
        over.as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .or_else(|| own.clone())
    };
    let artist = pick(&opts.artist, &cam.shot.artist).filter(|_| keep_rights);
    let copyright = pick(&opts.copyright, &cam.shot.copyright).filter(|_| keep_rights);

    let mut ifd0 = Ifd(Vec::new());
    ifd0.long(254, &[1]);
    let (tw, th, tb) = thumb.unwrap_or((1, 1, vec![0, 0, 0]));
    ifd0.long(256, &[tw]);
    ifd0.long(257, &[th]);
    ifd0.short(258, &[8, 8, 8]);
    ifd0.short(259, &[1]);
    ifd0.short(262, &[2]);
    ifd0.ascii_opt(271, Some(&cam.make));
    ifd0.ascii_opt(272, Some(&cam.model));
    ifd0.0.push((273, Value::Blobs(vec![blobs.len()])));
    ifd0.short(274, &[m.orientation]);
    ifd0.short(277, &[3]);
    ifd0.long(278, &[th]);
    ifd0.long(279, &[tb.len() as u32]);
    ifd0.short(284, &[1]);
    ifd0.ascii(305, &format!("aigle {}", env!("CARGO_PKG_VERSION")));
    ifd0.ascii_opt(315, artist.as_deref());
    ifd0.0.push((330, Value::Ifds(subs)));
    ifd0.ascii_opt(33432, copyright.as_deref());
    ifd0.byte(50706, &[1, 4, 0, 0]);
    ifd0.byte(50707, &[1, 1, 0, 0]);
    let model = format!("{} {}", cam.make, cam.model);
    ifd0.ascii(50708, model.trim());
    let matrix = m
        .xyz_to_cam
        .unwrap_or_else(|| color::invert(&color::SRGB_TO_XYZ).expect("invertible"));
    ifd0.srational(50721, &matrix.concat());
    ifd0.rational(
        50728,
        &m.as_shot.map(|v| if v > 0.0 { 1.0 / v } else { 1.0 }),
    );
    if let Some(b) = m.baseline_exposure {
        ifd0.srational(50730, &[b as f64]);
    }
    ifd0.short(50778, &[21]);
    if full {
        ifd0.0.push((34665, Value::Ifds(vec![ifds.len()])));
        ifds.push(exif_ifd(cam));
        if let Some(g) = &cam.shot.gps {
            ifd0.0.push((34853, Value::Ifds(vec![ifds.len()])));
            ifds.push(gps_ifd(g.latitude, g.longitude, g.altitude));
        }
    }
    blobs.push(&tb);
    ifds[0] = ifd0;
    tiff(ifds, &blobs)
}

/// Area-averaged RGB thumbnail with long edge `max`.
fn thumbnail(p: &Preview, max: u32) -> (u32, u32, Vec<u8>) {
    let s = aigle_core::geometry::fit_long_edge(
        (p.width as f64, p.height as f64),
        max.min(p.width.max(p.height)),
    );
    let bgra: Vec<u8> = p
        .rgb
        .as_chunks::<3>()
        .0
        .iter()
        .flat_map(|c| [c[2], c[1], c[0], 255])
        .collect();
    let small = crate::encode::resize_area(p.width, p.height, &bgra, s.w, s.h);
    let rgb = small
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|c| [c[2], c[1], c[0]])
        .collect();
    (s.w, s.h, rgb)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn huffman_lengths_are_limited_and_complete() {
        let mut freq = [0u64; 17];
        for (i, f) in freq.iter_mut().enumerate() {
            *f = 1u64 << (40 - 2 * i.min(19));
        }
        let (bits, vals) = huffman(&freq);
        assert_eq!(vals.len(), 17);
        assert_eq!(bits.iter().map(|&b| b as usize).sum::<usize>(), 17);
        // Kraft sum < 1 (the all-ones code stays unused).
        let kraft: f64 = bits
            .iter()
            .enumerate()
            .map(|(l, &n)| n as f64 / 2f64.powi(l as i32 + 1))
            .sum();
        assert!(kraft < 1.0, "{kraft}");
        let one = huffman(&{
            let mut f = [0u64; 17];
            f[0] = 99;
            f
        });
        assert_eq!((one.0[0], one.1.as_slice()), (1, &[0u8][..]));
    }

    #[test]
    fn categories() {
        assert_eq!(category(0), 0);
        assert_eq!(category(1), 1);
        assert_eq!(category(-1), 1);
        assert_eq!(category(255), 8);
        assert_eq!(category(-256), 9);
        assert_eq!(category(-32768), 16);
    }

    #[test]
    fn tiff_layout_points_at_its_data() {
        let mut a = Ifd(Vec::new());
        a.short(256, &[7]);
        a.0.push((273, Value::Blobs(vec![0])));
        a.ascii(305, "hello world");
        let out = tiff(vec![a], &[b"PIXELS"]);
        assert_eq!(&out[..4], b"II*\0");
        let n = u16::from_le_bytes([out[8], out[9]]) as usize;
        assert_eq!(n, 3);
        let entry = |k: usize| &out[10 + 12 * k..22 + 12 * k];
        let off = u32::from_le_bytes(entry(1)[8..12].try_into().unwrap()) as usize;
        assert_eq!(&out[off..off + 6], b"PIXELS");
        let s = u32::from_le_bytes(entry(2)[8..12].try_into().unwrap()) as usize;
        assert_eq!(&out[s..s + 12], b"hello world\0");
    }
}
