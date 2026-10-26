//! Safe wrapper over the aigle LibRaw shim (`shim/aigle_libraw.h`).
//!
//! All `unsafe` FFI of the A1 decode lives here; the engine only sees [`Raw`].

use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

#[repr(C)]
#[derive(Clone)]
struct MetaC {
    width: i32,
    height: i32,
    flip: i32,
    kind: i32,
    colors: i32,
    filters: u32,
    cfa: [[i32; 6]; 6],
    cam_mul: [f32; 4],
    pre_mul: [f32; 4],
    cam_xyz: [[f32; 3]; 4],
    rgb_cam: [[f32; 4]; 3],
    iso: f32,
    shutter: f32,
    aperture: f32,
    focal_len: f32,
    baseline_exposure: f32,
    black: [f32; 4],
    white: f32,
    black_pattern_h: i32,
    black_pattern_w: i32,
    lens_make: [c_char; 128],
    lens_model: [c_char; 128],
    focal_35mm: f32,
    lens_min_focal: f32,
    lens_max_focal: f32,
    make: [c_char; 64],
    model: [c_char; 64],
    unsupported_reason: [c_char; 128],
    datetime_original: [c_char; 32],
    offset_time: [c_char; 16],
    exposure_bias: f32,
    exposure_program: i32,
    metering_mode: i32,
    flash: i32,
    exposure_mode: i32,
    white_balance: i32,
    max_aperture: f32,
    body_serial: [c_char; 64],
    lens_serial: [c_char; 64],
    firmware: [c_char; 128],
    artist: [c_char; 64],
    copyright: [c_char; 128],
    gps_valid: i32,
    gps_lat: f64,
    gps_lon: f64,
    gps_alt: f32,
}

#[repr(C)]
struct ThumbC {
    kind: i32,
    width: i32,
    height: i32,
    data: *const u8,
    len: usize,
    opaque: *mut c_void,
}

type CancelFn = extern "C" fn(*mut c_void) -> c_int;

unsafe extern "C" {
    fn aigle_raw_new() -> *mut c_void;
    fn aigle_raw_free(h: *mut c_void);
    fn aigle_raw_open(h: *mut c_void, path: *const c_char) -> c_int;
    fn aigle_raw_get_size(
        h: *mut c_void,
        width: *mut c_int,
        height: *mut c_int,
        raw_width: *mut c_int,
        raw_height: *mut c_int,
    );
    fn aigle_raw_unpack(h: *mut c_void, cancel: Option<CancelFn>, user: *mut c_void) -> c_int;
    fn aigle_raw_strerror(code: c_int) -> *const c_char;
    fn aigle_raw_get_meta(h: *mut c_void, out: *mut MetaC) -> c_int;
    fn aigle_raw_copy_cfa(
        h: *mut c_void,
        white: *const f32,
        out: *mut f32,
        clip: *mut u8,
        clipped: *mut u64,
    ) -> c_int;
    fn aigle_raw_copy_rgb(h: *mut c_void, out: *mut f32) -> c_int;
    fn aigle_raw_get_thumb(h: *mut c_void, out: *mut ThumbC) -> c_int;
    fn aigle_raw_thumb_free(t: *mut ThumbC);
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    pub code: i32,
    pub message: String,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "LibRaw: {} ({})", self.message, self.code)
    }
}

impl std::error::Error for Error {}

fn check(code: c_int) -> Result<(), Error> {
    if code == 0 {
        return Ok(());
    }
    // SAFETY: libraw_strerror returns a pointer to a static string for every code.
    let message = unsafe { CStr::from_ptr(aigle_raw_strerror(code)) }
        .to_string_lossy()
        .into_owned();
    Err(Error { code, message })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Bayer,
    XTrans,
    /// Already demosaiced linear data (linear DNG, sRAW).
    Rgb,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Meta {
    pub width: u32,
    pub height: u32,
    /// dcraw flip: bit 2 transpose, bit 1 vertical flip, bit 0 horizontal flip.
    pub flip: i32,
    pub kind: Result<Kind, String>,
    /// CFA colour (0 R, 1 G, 2 B) at (row % 6, col % 6) of the visible area.
    pub cfa: [[u8; 6]; 6],
    pub cam_mul: [f32; 4],
    pub pre_mul: [f32; 4],
    /// XYZ (D65) → camera, one row per camera channel.
    pub cam_xyz: [[f32; 3]; 4],
    /// camera → linear sRGB (LibRaw).
    pub rgb_cam: [[f32; 4]; 3],
    pub iso: f32,
    pub shutter: f32,
    pub aperture: f32,
    pub focal_len: f32,
    pub baseline_exposure: Option<f32>,
    /// Black per CFA colour index 0..3 (raw units), excluding any extra black pattern.
    pub black: [f32; 4],
    pub white: f32,
    pub black_pattern: (u32, u32),
    pub lens_make: String,
    pub lens_model: String,
    /// 35 mm-equivalent focal length (0 if unknown).
    pub focal_35mm: f32,
    pub lens_focal_range: (f32, f32),
    pub make: String,
    pub model: String,
    pub shot: Shot,
}

/// EXIF capture facts; see `aigle_engine_api::ShotInfo` for the meaning of each field.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Shot {
    pub date_time: Option<String>,
    pub utc_offset: Option<String>,
    pub exposure_bias: Option<f32>,
    pub exposure_program: Option<u16>,
    pub metering_mode: Option<u16>,
    pub flash: Option<u16>,
    pub exposure_mode: Option<u16>,
    pub white_balance: Option<u16>,
    pub max_aperture: Option<f32>,
    pub body_serial: Option<String>,
    pub lens_serial: Option<String>,
    pub firmware: Option<String>,
    pub artist: Option<String>,
    pub copyright: Option<String>,
    /// (latitude, longitude, altitude) in degrees and metres.
    pub gps: Option<(f64, f64, Option<f32>)>,
}

fn nonempty(b: &[c_char]) -> Option<String> {
    Some(cstr(b)).filter(|s| !s.is_empty())
}

fn code(v: i32) -> Option<u16> {
    u16::try_from(v).ok()
}

fn finite(v: f32) -> Option<f32> {
    v.is_finite().then_some(v)
}

pub enum Thumb {
    Jpeg(Vec<u8>),
    Rgb8 {
        width: u32,
        height: u32,
        data: Vec<u8>,
    },
}

/// One LibRaw instance. `Send` (one worker at a time), not `Sync`.
pub struct Raw {
    h: *mut c_void,
}

// SAFETY: the reentrant LibRaw build keeps all state per instance; `Raw` is used by one
// thread at a time (`&mut self` on every mutating call).
unsafe impl Send for Raw {}

fn cstr(b: &[c_char]) -> String {
    let bytes: Vec<u8> = b
        .iter()
        .take_while(|&&c| c != 0)
        .map(|&c| c as u8)
        .collect();
    String::from_utf8_lossy(&bytes).trim().to_string()
}

extern "C" fn cancel_trampoline(user: *mut c_void) -> c_int {
    // SAFETY: `user` is the `&dyn Fn() -> bool` passed by `unpack` and outlives the call.
    let f = unsafe { &*(user as *const &dyn Fn() -> bool) };
    f() as c_int
}

impl Raw {
    pub fn open(path: &Path) -> Result<Raw, Error> {
        // SAFETY: plain constructor; null on allocation failure.
        let h = unsafe { aigle_raw_new() };
        if h.is_null() {
            return Err(Error {
                code: -100007,
                message: "out of memory".into(),
            });
        }
        let raw = Raw { h };
        let c = CString::new(path.as_os_str().as_bytes()).map_err(|_| Error {
            code: -2,
            message: "path contains NUL".into(),
        })?;
        // SAFETY: valid handle and NUL-terminated path.
        check(unsafe { aigle_raw_open(raw.h, c.as_ptr()) })?;
        Ok(raw)
    }

    /// (image width, height, raw width, raw height), available after [`Raw::open`] without
    /// decoding.
    pub fn size(&self) -> (u32, u32, u32, u32) {
        let (mut w, mut h, mut rw, mut rh) = (0, 0, 0, 0);
        // SAFETY: valid handle; out-pointers to locals.
        unsafe { aigle_raw_get_size(self.h, &mut w, &mut h, &mut rw, &mut rh) };
        let u = |v: c_int| v.max(0) as u32;
        (u(w), u(h), u(rw), u(rh))
    }

    /// Decode the raw data (A1). `cancelled` is polled at LibRaw progress points.
    pub fn unpack(&mut self, cancelled: &dyn Fn() -> bool) -> Result<(), Error> {
        let user: *const &dyn Fn() -> bool = &cancelled;
        // SAFETY: the trampoline only dereferences `user` during this call.
        check(unsafe { aigle_raw_unpack(self.h, Some(cancel_trampoline), user as *mut c_void) })
    }

    pub fn meta(&mut self) -> Result<Meta, Error> {
        // SAFETY: MetaC is plain data; zeroed is a valid bit pattern and the shim fills it.
        let mut m: MetaC = unsafe { std::mem::zeroed() };
        // SAFETY: valid handle, out-pointer to an owned struct.
        check(unsafe { aigle_raw_get_meta(self.h, &mut m) })?;
        let kind = match m.kind {
            0 => Ok(Kind::Bayer),
            1 => Ok(Kind::XTrans),
            2 => Ok(Kind::Rgb),
            _ => Err(cstr(&m.unsupported_reason)),
        };
        Ok(Meta {
            width: m.width.max(0) as u32,
            height: m.height.max(0) as u32,
            flip: m.flip,
            kind,
            cfa: m.cfa.map(|r| r.map(|c| c.clamp(0, 2) as u8)),
            cam_mul: m.cam_mul,
            pre_mul: m.pre_mul,
            cam_xyz: m.cam_xyz,
            rgb_cam: m.rgb_cam,
            iso: m.iso,
            shutter: m.shutter,
            aperture: m.aperture,
            focal_len: m.focal_len,
            baseline_exposure: (m.baseline_exposure > -998.0).then_some(m.baseline_exposure),
            black: m.black,
            white: m.white,
            black_pattern: (
                m.black_pattern_h.max(0) as u32,
                m.black_pattern_w.max(0) as u32,
            ),
            lens_make: cstr(&m.lens_make),
            lens_model: cstr(&m.lens_model),
            focal_35mm: m.focal_35mm,
            lens_focal_range: (m.lens_min_focal, m.lens_max_focal),
            make: cstr(&m.make),
            model: cstr(&m.model),
            shot: Shot {
                date_time: nonempty(&m.datetime_original),
                utc_offset: nonempty(&m.offset_time),
                exposure_bias: finite(m.exposure_bias),
                exposure_program: code(m.exposure_program),
                metering_mode: code(m.metering_mode),
                flash: code(m.flash),
                exposure_mode: code(m.exposure_mode),
                white_balance: code(m.white_balance),
                max_aperture: finite(m.max_aperture).filter(|v| *v > 0.0),
                body_serial: nonempty(&m.body_serial),
                lens_serial: nonempty(&m.lens_serial),
                firmware: nonempty(&m.firmware),
                artist: nonempty(&m.artist),
                copyright: nonempty(&m.copyright),
                gps: (m.gps_valid != 0 && m.gps_lat.is_finite() && m.gps_lon.is_finite())
                    .then(|| (m.gps_lat, m.gps_lon, finite(m.gps_alt))),
            },
        })
    }

    /// Black-subtracted CFA normalised to the white level (A2 input), plus a clip mask and
    /// the number of clipped sites. `white` overrides the white level per CFA colour index
    /// (R, G, B, G2) in absolute raw units; it is capped at LibRaw's maximum.
    pub fn copy_cfa(
        &mut self,
        width: u32,
        height: u32,
        white: Option<[f32; 4]>,
    ) -> Result<(Vec<f32>, Vec<u8>, u64), Error> {
        let n = width as usize * height as usize;
        let mut out = vec![0f32; n];
        let mut clip = vec![0u8; n];
        let mut clipped = 0u64;
        // SAFETY: buffers sized to the visible area reported by `meta`.
        check(unsafe {
            aigle_raw_copy_cfa(
                self.h,
                white.as_ref().map_or(std::ptr::null(), |w| w.as_ptr()),
                out.as_mut_ptr(),
                clip.as_mut_ptr(),
                &mut clipped,
            )
        })?;
        Ok((out, clip, clipped))
    }

    /// Black-subtracted, normalised interleaved RGB for [`Kind::Rgb`].
    pub fn copy_rgb(&mut self, width: u32, height: u32) -> Result<Vec<f32>, Error> {
        let mut out = vec![0f32; width as usize * height as usize * 3];
        // SAFETY: buffer sized to the visible area × 3.
        check(unsafe { aigle_raw_copy_rgb(self.h, out.as_mut_ptr()) })?;
        Ok(out)
    }

    /// Embedded preview, if the file has a usable one.
    pub fn thumbnail(&mut self) -> Result<Thumb, Error> {
        // SAFETY: zeroed ThumbC is valid; the shim fills it and we free it below.
        let mut t: ThumbC = unsafe { std::mem::zeroed() };
        // SAFETY: valid handle and out-pointer.
        check(unsafe { aigle_raw_get_thumb(self.h, &mut t) })?;
        // SAFETY: on success `data` points at `len` bytes owned by `opaque`.
        let bytes = unsafe { std::slice::from_raw_parts(t.data, t.len) }.to_vec();
        let res = match t.kind {
            1 => Ok(Thumb::Jpeg(bytes)),
            _ => Ok(Thumb::Rgb8 {
                width: t.width as u32,
                height: t.height as u32,
                data: bytes,
            }),
        };
        // SAFETY: frees the LibRaw allocation exactly once.
        unsafe { aigle_raw_thumb_free(&mut t) };
        res
    }
}

impl Drop for Raw {
    fn drop(&mut self) {
        // SAFETY: handle created by aigle_raw_new, freed once.
        unsafe { aigle_raw_free(self.h) };
    }
}
