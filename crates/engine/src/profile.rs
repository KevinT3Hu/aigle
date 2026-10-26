//! Camera colour calibration (doc 02 §6.2 steps 2–5): DNG / DCP profiles and the DNG
//! dual-illuminant model.
//!
//! * [`CameraCalibration`]: ColorMatrix / ForwardMatrix / CameraCalibration per calibration
//!   illuminant, interpolated in inverse CCT for the scene white (DNG spec 1.6 §6), plus
//!   AnalogBalance. LibRaw cameras have a single D65 matrix.
//! * [`DngProfile`]: what a `.dcp` file (or a DNG file's IFD 0) carries: the calibration, the
//!   HueSatMap tables per illuminant, the LookTable and BaselineExposureOffset. The profile's
//!   ToneCurve is not used: D1 is aigle's rendering transform.
//!
//! Chromatic adaptation is CAT16 (decided, doc 02 §6.2), not the Bradford transform of the
//! DNG SDK, except where a ForwardMatrix already contains the camera's adaptation to D50.

use std::collections::HashMap;

use aigle_core::color::{self, Mat3};

/// D50, the DNG profile connection space white.
pub const D50_XY: (f64, f64) = (0.3457, 0.3585);

/// One calibration illuminant.
#[derive(Clone, Debug, PartialEq)]
pub struct Illuminant {
    /// Correlated colour temperature in K.
    pub cct: f64,
    /// XYZ → camera (DNG ColorMatrix).
    pub color_matrix: Mat3,
    /// White-balanced camera → XYZ D50 (DNG ForwardMatrix), if given.
    pub forward: Option<Mat3>,
    /// Per-unit camera calibration (DNG CameraCalibration), identity if absent.
    pub calibration: Mat3,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CameraCalibration {
    /// One or two illuminants, sorted by CCT (lowest first).
    pub ill: Vec<Illuminant>,
    pub analog_balance: [f64; 3],
}

/// CCT of an EXIF LightSource code (values of the DNG SDK).
pub fn illuminant_cct(code: u32) -> Option<f64> {
    Some(match code {
        17 | 3 => 2850.0,         // Standard light A, tungsten
        24 => 3200.0,             // ISO studio tungsten
        18 => 4874.0,             // Standard light B
        19 => 6774.0,             // Standard light C
        23 => 5003.0,             // D50
        20 | 1 | 4 | 9 => 5503.0, // D55, daylight, flash, fine weather
        21 | 10 => 6504.0,        // D65, cloudy
        22 | 11 => 7504.0,        // D75, shade
        12 => 6430.0,             // daylight fluorescent
        13 => 5000.0,             // day white fluorescent
        14 | 2 => 4150.0,         // cool white fluorescent, fluorescent
        15 => 3450.0,             // white fluorescent
        16 => 2940.0,             // warm white fluorescent
        _ => return None,
    })
}

impl CameraCalibration {
    /// A single D65 ColorMatrix (LibRaw cameras, rendered inputs).
    pub fn single(xyz_to_camera: Mat3) -> CameraCalibration {
        CameraCalibration {
            ill: vec![Illuminant {
                cct: 6504.0,
                color_matrix: xyz_to_camera,
                forward: None,
                calibration: color::IDENTITY,
            }],
            analog_balance: [1.0; 3],
        }
    }

    /// Weight of `ill[0]` for scene white `xy` (linear in inverse CCT, clamped).
    pub fn weight(&self, xy: (f64, f64)) -> f64 {
        if self.ill.len() < 2 {
            return 1.0;
        }
        let (t1, t2) = (self.ill[0].cct, self.ill[1].cct);
        let t = color::xy_to_cct_tint(xy.0, xy.1).0;
        if !(t.is_finite()) || t <= t1 || t2 <= t1 {
            1.0
        } else if t >= t2 {
            0.0
        } else {
            (1.0 / t - 1.0 / t2) / (1.0 / t1 - 1.0 / t2)
        }
    }

    fn mix(&self, g: f64, f: impl Fn(&Illuminant) -> Mat3) -> Mat3 {
        let a = f(&self.ill[0]);
        let Some(b) = self.ill.get(1).map(f) else {
            return a;
        };
        let mut m = [[0.0; 3]; 3];
        for r in 0..3 {
            for c in 0..3 {
                m[r][c] = g * a[r][c] + (1.0 - g) * b[r][c];
            }
        }
        m
    }

    fn ab_cc(&self, g: f64) -> Mat3 {
        color::mul(
            &color::diag(self.analog_balance),
            &self.mix(g, |i| i.calibration),
        )
    }

    /// XYZ → camera for scene white `xy`: AB · CC · CM.
    pub fn xyz_to_camera(&self, xy: (f64, f64)) -> Mat3 {
        let g = self.weight(xy);
        color::mul(&self.ab_cc(g), &self.mix(g, |i| i.color_matrix))
    }

    /// Camera neutral of white `xy`, green = 1.
    pub fn neutral(&self, xy: (f64, f64)) -> [f64; 3] {
        let n = color::mul_vec(&self.xyz_to_camera(xy), color::xy_to_xyz(xy.0, xy.1));
        let g = if n[1].abs() > 1e-12 { n[1] } else { 1.0 };
        n.map(|v| v / g)
    }

    /// White chromaticity of a camera neutral (DNG SDK `NeutralToXY`, iterated).
    pub fn neutral_to_xy(&self, neutral: [f64; 3]) -> (f64, f64) {
        let mut xy = D50_XY;
        for _ in 0..30 {
            let Some(inv) = color::invert(&self.xyz_to_camera(xy)) else {
                return color::D65_XY;
            };
            let next = color::xyz_to_xy(color::mul_vec(&inv, neutral));
            if !(next.0.is_finite() && next.1.is_finite()) || next.1 <= 0.05 {
                return color::D65_XY;
            }
            let done = (next.0 - xy.0).abs() + (next.1 - xy.1).abs() < 1e-7;
            xy = next;
            if done {
                break;
            }
        }
        xy
    }

    /// Camera RGB → XYZ adapted to the white `dst`, for scene white `xy`. The scene neutral
    /// (green = 1) maps to Y = 1.
    pub fn camera_to_xyz(&self, xy: (f64, f64), dst: (f64, f64)) -> Mat3 {
        let g = self.weight(xy);
        let neutral = self.neutral(xy);
        let to = |w: (f64, f64)| color::xy_to_xyz(w.0, w.1);
        if self.ill.iter().all(|i| i.forward.is_some()) {
            let fm = self.mix(g, |i| i.forward.expect("checked"));
            let abcc_inv = color::invert(&self.ab_cc(g)).unwrap_or(color::IDENTITY);
            let r = color::mul_vec(&abcc_inv, neutral);
            let m = color::mul(
                &fm,
                &color::mul(&color::diag(r.map(|v| 1.0 / v)), &abcc_inv),
            );
            // FM rows sum to the D50 white with Y = 1 (DNG spec); normalise defensively.
            let y = color::mul_vec(&m, neutral)[1];
            let m = color::scale(&m, if y > 0.0 { 1.0 / y } else { 1.0 });
            return color::mul(&color::cat16(to(D50_XY), to(dst)), &m);
        }
        let m = color::invert(&self.xyz_to_camera(xy)).unwrap_or(color::SRGB_TO_XYZ);
        let y = color::mul_vec(&m, neutral)[1];
        let m = color::scale(&m, if y > 0.0 { 1.0 / y } else { 1.0 });
        color::mul(&color::cat16(to(xy), to(dst)), &m)
    }
}

/// An HSV table: `dims = [hue, sat, val]` divisions, `data` = `val·hue·sat` entries of
/// `[hue_shift_deg, sat_scale, val_scale]` (DNG order).
#[derive(Clone, Debug, PartialEq)]
pub struct HsvTable {
    pub dims: [u32; 3],
    pub srgb: bool,
    pub data: Vec<f32>,
}

impl HsvTable {
    /// `g · a + (1 − g) · b` (same dims).
    pub fn mix(a: &HsvTable, b: &HsvTable, g: f64) -> HsvTable {
        let g = g as f32;
        HsvTable {
            dims: a.dims,
            srgb: a.srgb,
            data: a
                .data
                .iter()
                .zip(&b.data)
                .map(|(x, y)| g * x + (1.0 - g) * y)
                .collect(),
        }
    }

    /// Kernel layout (`aigle_kernels::luts::HSV_HEADER`).
    pub fn pack(&self, out: &mut Vec<f32>) {
        out.extend_from_slice(&[
            self.dims[0] as f32,
            self.dims[1] as f32,
            self.dims[2] as f32,
            self.srgb as u32 as f32,
        ]);
        out.extend_from_slice(&self.data);
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct DngProfile {
    pub name: String,
    pub camera_model: String,
    pub calibration: CameraCalibration,
    /// HueSatMap per illuminant, in the order of `calibration.ill`.
    pub hue_sat: Vec<HsvTable>,
    pub look: Option<HsvTable>,
    pub baseline_exposure_offset: f64,
    /// AsShotNeutral of a DNG file.
    pub as_shot_neutral: Option<[f64; 3]>,
    /// DNG NoiseProfile `(S, O)` per CFA plane (one entry = all planes): noise variance
    /// `S·x + O` of the normalised raw signal `x` (1.0 = white level). Empty if absent.
    pub noise_profile: Vec<(f64, f64)>,
}

impl DngProfile {
    /// HueSatMap for scene white `xy`, interpolated like the matrices.
    pub fn hue_sat_at(&self, xy: (f64, f64)) -> Option<HsvTable> {
        match self.hue_sat.as_slice() {
            [] => None,
            [a] => Some(a.clone()),
            [a, b, ..] => Some(HsvTable::mix(a, b, self.calibration.weight(xy))),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// TIFF IFD 0 reader (DCP: magic 0x4352 "RC"; DNG: TIFF magic 42).

struct Tiff<'a> {
    b: &'a [u8],
    le: bool,
}

#[derive(Clone, Copy)]
struct Entry {
    typ: u16,
    count: u32,
    /// Offset of the value bytes.
    at: usize,
}

const TYPE_SIZE: [usize; 13] = [0, 1, 1, 2, 4, 8, 1, 1, 2, 4, 8, 4, 8];

impl Tiff<'_> {
    fn u16(&self, o: usize) -> Option<u16> {
        let s: [u8; 2] = self.b.get(o..o + 2)?.try_into().ok()?;
        Some(if self.le {
            u16::from_le_bytes(s)
        } else {
            u16::from_be_bytes(s)
        })
    }
    fn u32(&self, o: usize) -> Option<u32> {
        let s: [u8; 4] = self.b.get(o..o + 4)?.try_into().ok()?;
        Some(if self.le {
            u32::from_le_bytes(s)
        } else {
            u32::from_be_bytes(s)
        })
    }
    fn u64(&self, o: usize) -> Option<u64> {
        let s: [u8; 8] = self.b.get(o..o + 8)?.try_into().ok()?;
        Some(if self.le {
            u64::from_le_bytes(s)
        } else {
            u64::from_be_bytes(s)
        })
    }

    fn ifd0(&self) -> Result<HashMap<u16, Entry>, String> {
        self.ifd_at(self.u32(4).ok_or("truncated header")? as usize)
    }

    fn ifd_at(&self, off: usize) -> Result<HashMap<u16, Entry>, String> {
        let n = self.u16(off).ok_or("bad IFD offset")? as usize;
        let mut m = HashMap::with_capacity(n);
        for i in 0..n {
            let e = off + 2 + 12 * i;
            let (tag, typ) = (
                self.u16(e).ok_or("truncated IFD")?,
                self.u16(e + 2).ok_or("truncated IFD")?,
            );
            let count = self.u32(e + 4).ok_or("truncated IFD")?;
            let size = TYPE_SIZE.get(typ as usize).copied().unwrap_or(0) * count as usize;
            if size == 0 {
                continue;
            }
            let at = if size <= 4 {
                e + 8
            } else {
                self.u32(e + 8).ok_or("truncated IFD")? as usize
            };
            if at.checked_add(size).is_none_or(|end| end > self.b.len()) {
                return Err(format!("tag {tag}: value out of bounds"));
            }
            m.insert(tag, Entry { typ, count, at });
        }
        Ok(m)
    }

    fn nums(&self, e: &Entry) -> Vec<f64> {
        let sz = TYPE_SIZE[e.typ as usize];
        (0..e.count as usize)
            .filter_map(|i| {
                let o = e.at + i * sz;
                Some(match e.typ {
                    1 | 7 => self.b[o] as f64,
                    6 => self.b[o] as i8 as f64,
                    3 => self.u16(o)? as f64,
                    8 => self.u16(o)? as i16 as f64,
                    4 => self.u32(o)? as f64,
                    9 => self.u32(o)? as i32 as f64,
                    5 => {
                        let d = self.u32(o + 4)?;
                        if d == 0 {
                            0.0
                        } else {
                            self.u32(o)? as f64 / d as f64
                        }
                    }
                    10 => {
                        let d = self.u32(o + 4)? as i32;
                        if d == 0 {
                            0.0
                        } else {
                            self.u32(o)? as i32 as f64 / d as f64
                        }
                    }
                    11 => f32::from_bits(self.u32(o)?) as f64,
                    12 => f64::from_bits(self.u64(o)?),
                    _ => return None,
                })
            })
            .collect()
    }

    fn text(&self, e: &Entry) -> String {
        let s = &self.b[e.at..e.at + e.count as usize];
        let end = s.iter().position(|&c| c == 0).unwrap_or(s.len());
        String::from_utf8_lossy(&s[..end]).trim().to_string()
    }
}

mod tag {
    pub const UNIQUE_CAMERA_MODEL: u16 = 50708;
    pub const COLOR_MATRIX: [u16; 2] = [50721, 50722];
    pub const CAMERA_CALIBRATION: [u16; 2] = [50723, 50724];
    pub const ANALOG_BALANCE: u16 = 50727;
    pub const AS_SHOT_NEUTRAL: u16 = 50728;
    pub const CALIBRATION_ILLUMINANT: [u16; 2] = [50778, 50779];
    pub const PROFILE_NAME: u16 = 50936;
    pub const HSM_DIMS: u16 = 50937;
    pub const HSM_DATA: [u16; 2] = [50938, 50939];
    pub const FORWARD_MATRIX: [u16; 2] = [50964, 50965];
    pub const LOOK_DIMS: u16 = 50981;
    pub const LOOK_DATA: u16 = 50982;
    pub const HSM_ENCODING: u16 = 51107;
    pub const LOOK_ENCODING: u16 = 51108;
    pub const BASELINE_EXPOSURE_OFFSET: u16 = 51109;
    pub const NOISE_PROFILE: u16 = 51041;
    pub const SUB_IFDS: u16 = 330;
    pub const NEW_SUBFILE_TYPE: u16 = 254;
}

/// NoiseProfile pairs from IFD 0 or, failing that, the main raw sub-IFD (NewSubfileType 0).
fn noise_profile(t: &Tiff, ifd0: &HashMap<u16, Entry>) -> Vec<(f64, f64)> {
    let pairs = |v: Vec<f64>| -> Vec<(f64, f64)> {
        let p: Vec<(f64, f64)> = v.as_chunks::<2>().0.iter().map(|c| (c[0], c[1])).collect();
        let ok = matches!(p.len(), 1 | 3) && p.iter().all(|&(s, o)| s > 0.0 && o >= 0.0);
        if ok { p } else { Vec::new() }
    };
    if let Some(e) = ifd0.get(&tag::NOISE_PROFILE) {
        return pairs(t.nums(e));
    }
    let Some(subs) = ifd0.get(&tag::SUB_IFDS) else {
        return Vec::new();
    };
    for off in t.nums(subs) {
        let Ok(sub) = t.ifd_at(off as usize) else {
            continue;
        };
        let main = sub
            .get(&tag::NEW_SUBFILE_TYPE)
            .and_then(|e| t.nums(e).first().copied())
            .unwrap_or(0.0)
            == 0.0;
        if main && let Some(e) = sub.get(&tag::NOISE_PROFILE) {
            return pairs(t.nums(e));
        }
    }
    Vec::new()
}

fn mat3(v: &[f64]) -> Option<Mat3> {
    (v.len() == 9).then(|| [[v[0], v[1], v[2]], [v[3], v[4], v[5]], [v[6], v[7], v[8]]])
}

/// Parse a `.dcp` file or the colour tags of a DNG file's IFD 0. Only 3-colour cameras.
pub fn parse(bytes: &[u8]) -> Result<DngProfile, String> {
    let le = match bytes.get(..2) {
        Some(b"II") => true,
        Some(b"MM") => false,
        _ => return Err("not a TIFF / DCP file".into()),
    };
    let t = Tiff { b: bytes, le };
    match t.u16(2) {
        Some(42) | Some(0x4352) => {}
        _ => return Err("not a TIFF / DCP file".into()),
    }
    let ifd = t.ifd0()?;
    let nums = |tag: u16| ifd.get(&tag).map(|e| t.nums(e));
    let text = |tag: u16| ifd.get(&tag).map(|e| t.text(e)).unwrap_or_default();

    let mut ill = Vec::new();
    for k in 0..2 {
        let Some(cm) = nums(tag::COLOR_MATRIX[k]) else {
            continue;
        };
        let cm = mat3(&cm).ok_or("ColorMatrix is not 3×3 (only 3-colour cameras are supported)")?;
        let code = nums(tag::CALIBRATION_ILLUMINANT[k])
            .and_then(|v| v.first().copied())
            .unwrap_or(21.0);
        let cct = illuminant_cct(code as u32).unwrap_or(5003.0);
        ill.push((
            k,
            Illuminant {
                cct,
                color_matrix: cm,
                forward: nums(tag::FORWARD_MATRIX[k]).and_then(|v| mat3(&v)),
                calibration: nums(tag::CAMERA_CALIBRATION[k])
                    .and_then(|v| mat3(&v))
                    .unwrap_or(color::IDENTITY),
            },
        ));
    }
    if ill.is_empty() {
        return Err("no ColorMatrix".into());
    }
    let ab = nums(tag::ANALOG_BALANCE)
        .filter(|v| v.len() == 3)
        .map_or([1.0; 3], |v| [v[0], v[1], v[2]]);

    let table = |dims: u16, data: u16, enc: u16| -> Result<Option<HsvTable>, String> {
        let (Some(d), Some(v)) = (nums(dims), ifd.get(&data)) else {
            return Ok(None);
        };
        if d.len() != 3 {
            return Err("table dims must have 3 entries".into());
        }
        let dims = [d[0] as u32, d[1] as u32, d[2].max(1.0) as u32];
        let n = dims[0] as usize * dims[1] as usize * dims[2] as usize * 3;
        let vals = t.nums(v);
        if dims[0] < 1 || dims[1] < 2 || vals.len() != n {
            return Err(format!(
                "table data size {} does not match dims {dims:?}",
                vals.len()
            ));
        }
        Ok(Some(HsvTable {
            dims,
            srgb: nums(enc).and_then(|v| v.first().copied()) == Some(1.0),
            data: vals.into_iter().map(|x| x as f32).collect(),
        }))
    };
    let mut hsm = Vec::new();
    for (k, _) in &ill {
        if let Some(h) = table(tag::HSM_DIMS, tag::HSM_DATA[*k], tag::HSM_ENCODING)? {
            hsm.push(h);
        }
    }
    if hsm.len() != ill.len() {
        hsm.truncate(1.min(hsm.len()));
    }
    // Sort the illuminants (and their tables) by CCT.
    if ill.len() == 2 && ill[0].1.cct > ill[1].1.cct {
        ill.swap(0, 1);
        if hsm.len() == 2 {
            hsm.swap(0, 1);
        }
    }
    Ok(DngProfile {
        name: text(tag::PROFILE_NAME),
        camera_model: text(tag::UNIQUE_CAMERA_MODEL),
        calibration: CameraCalibration {
            ill: ill.into_iter().map(|(_, i)| i).collect(),
            analog_balance: ab,
        },
        hue_sat: hsm,
        look: table(tag::LOOK_DIMS, tag::LOOK_DATA, tag::LOOK_ENCODING)?,
        baseline_exposure_offset: nums(tag::BASELINE_EXPOSURE_OFFSET)
            .and_then(|v| v.first().copied())
            .unwrap_or(0.0),
        as_shot_neutral: nums(tag::AS_SHOT_NEUTRAL)
            .filter(|v| v.len() == 3)
            .map(|v| [v[0], v[1], v[2]]),
        noise_profile: noise_profile(&t, &ifd),
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A little-endian DCP with the given (tag, type, values) entries.
    pub(crate) fn build_dcp(entries: &[(u16, u16, Vec<f64>)], text: &[(u16, &str)]) -> Vec<u8> {
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
                    12 => b.extend_from_slice(&x.to_bits().to_le_bytes()),
                    _ => unreachable!(),
                }
            }
            all.push((*tag, *typ, v.len() as u32, b));
        }
        for (tag, s) in text {
            let mut b = s.as_bytes().to_vec();
            b.push(0);
            all.push((*tag, 2, b.len() as u32, b));
        }
        all.sort_by_key(|e| e.0);
        let mut out = b"IIRC".to_vec();
        out.extend_from_slice(&8u32.to_le_bytes());
        let n = all.len();
        out.extend_from_slice(&(n as u16).to_le_bytes());
        let mut data_at = 8 + 2 + 12 * n + 4;
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
                out.extend_from_slice(&(data_at as u32).to_le_bytes());
                data.extend_from_slice(b);
                data_at += b.len();
            }
        }
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&data);
        out
    }

    fn flat(m: &Mat3) -> Vec<f64> {
        m.iter().flatten().copied().collect()
    }

    /// XYZ → camera matrices of a plausible camera at A and D65.
    pub(crate) const CM_A: Mat3 = [
        [0.95, -0.35, -0.05],
        [-0.45, 1.25, 0.2],
        [-0.05, 0.15, 0.85],
    ];
    pub(crate) const CM_D65: Mat3 = [[0.8, -0.25, -0.05], [-0.4, 1.2, 0.2], [-0.08, 0.2, 0.7]];

    #[test]
    fn parses_a_dual_illuminant_dcp_with_tables() {
        let hsm: Vec<f64> = (0..6 * 2 * 3)
            .map(|i| if i % 3 == 0 { 10.0 } else { 1.0 })
            .collect();
        let bytes = build_dcp(
            &[
                (tag::COLOR_MATRIX[0], 10, flat(&CM_D65)),
                (tag::COLOR_MATRIX[1], 10, flat(&CM_A)),
                (tag::CALIBRATION_ILLUMINANT[0], 3, vec![21.0]),
                (tag::CALIBRATION_ILLUMINANT[1], 3, vec![17.0]),
                (tag::HSM_DIMS, 4, vec![6.0, 2.0, 1.0]),
                (tag::HSM_DATA[0], 11, hsm.clone()),
                (tag::HSM_DATA[1], 11, hsm.iter().map(|v| v * 2.0).collect()),
                (tag::LOOK_DIMS, 4, vec![6.0, 2.0, 2.0]),
                (tag::LOOK_DATA, 11, vec![0.0; 6 * 2 * 2 * 3]),
                (tag::LOOK_ENCODING, 4, vec![1.0]),
                (tag::BASELINE_EXPOSURE_OFFSET, 10, vec![-0.25]),
                (
                    tag::NOISE_PROFILE,
                    12,
                    vec![2e-5, 1e-7, 1e-5, 5e-8, 3e-5, 2e-7],
                ),
            ],
            &[
                (tag::PROFILE_NAME, "Test Standard"),
                (tag::UNIQUE_CAMERA_MODEL, "Test Cam"),
            ],
        );
        let p = parse(&bytes).unwrap();
        assert_eq!(p.name, "Test Standard");
        assert_eq!(p.camera_model, "Test Cam");
        // Sorted by CCT: A first, with its table.
        assert_eq!(p.calibration.ill[0].cct, 2850.0);
        assert!((p.calibration.ill[0].color_matrix[0][0] - 0.95).abs() < 1e-4);
        assert_eq!(p.hue_sat.len(), 2);
        assert_eq!(p.hue_sat[0].data[0], 20.0);
        let look = p.look.as_ref().unwrap();
        assert!(look.srgb && look.dims == [6, 2, 2]);
        assert!((p.baseline_exposure_offset + 0.25).abs() < 1e-4);
        assert_eq!(
            p.noise_profile,
            vec![(2e-5, 1e-7), (1e-5, 5e-8), (3e-5, 2e-7)]
        );
    }

    #[test]
    fn dual_illuminant_interpolation_and_neutral_round_trip() {
        let mk = |cct, cm| Illuminant {
            cct,
            color_matrix: cm,
            forward: None,
            calibration: color::IDENTITY,
        };
        let c = CameraCalibration {
            ill: vec![mk(2850.0, CM_A), mk(6504.0, CM_D65)],
            analog_balance: [1.0; 3],
        };
        let a = color::cct_tint_to_xy(2850.0, 0.0);
        let d65 = color::cct_tint_to_xy(6504.0, 0.0);
        assert!((c.weight(a) - 1.0).abs() < 1e-3);
        assert!(c.weight(d65).abs() < 1e-3);
        assert!(c.weight(color::cct_tint_to_xy(2000.0, 0.0)) == 1.0);
        let mid = c.weight(color::cct_tint_to_xy(4000.0, 0.0));
        assert!(mid > 0.2 && mid < 0.8, "{mid}");
        // The camera neutral of a white maps back to that white, and through camera_to_xyz to
        // the destination white with Y = 1.
        for t in [2500.0, 3200.0, 4500.0, 5500.0, 8000.0] {
            let xy = color::cct_tint_to_xy(t, 10.0);
            let n = c.neutral(xy);
            let back = c.neutral_to_xy(n);
            assert!(
                (back.0 - xy.0).abs() < 1e-6 && (back.1 - xy.1).abs() < 1e-6,
                "{t}: {back:?} vs {xy:?}"
            );
            let w = color::mul_vec(&c.camera_to_xyz(xy, color::D65_XY), n);
            let d = color::xy_to_xyz(color::D65_XY.0, color::D65_XY.1);
            for k in 0..3 {
                assert!((w[k] - d[k]).abs() < 1e-6, "{t}: {w:?}");
            }
        }
        // ForwardMatrix path: the neutral maps to D50 before adaptation.
        let fm = color::mul(
            &color::cat16(
                color::xy_to_xyz(color::D65_XY.0, color::D65_XY.1),
                color::xy_to_xyz(D50_XY.0, D50_XY.1),
            ),
            &color::mul(
                &color::invert(&CM_D65).unwrap(),
                &color::diag(color::mul_vec(
                    &CM_D65,
                    color::xy_to_xyz(color::D65_XY.0, color::D65_XY.1),
                )),
            ),
        );
        let mut f = c.clone();
        for i in &mut f.ill {
            i.forward = Some(fm);
        }
        let xy = color::cct_tint_to_xy(4500.0, 0.0);
        let w = color::mul_vec(&f.camera_to_xyz(xy, D50_XY), f.neutral(xy));
        assert!(
            (w[1] - 1.0).abs() < 1e-9 && (color::xyz_to_xy(w).0 - D50_XY.0).abs() < 1e-3,
            "{w:?}"
        );
    }
}
