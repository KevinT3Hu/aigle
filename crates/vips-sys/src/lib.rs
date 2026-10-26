//! Safe wrappers over the aigle libvips shim (`shim/aigle_vips.c`).
//!
//! libvips is initialised on first use. Loading decodes through the embedded ICC profile to
//! float XYZ; saving pulls the image from a callback in strips, so the encoder never holds
//! the whole image.

use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Header {
    /// After autorotation.
    pub width: i32,
    pub height: i32,
    /// EXIF orientation of the file (1 if none).
    pub orientation: i32,
    pub bands: i32,
    pub has_icc: i32,
}

#[repr(C)]
struct ExifC {
    make: *const c_char,
    model: *const c_char,
    lens_make: *const c_char,
    lens_model: *const c_char,
    software: *const c_char,
    datetime: *const c_char,
    artist: *const c_char,
    copyright: *const c_char,
    exposure_time: f64,
    f_number: f64,
    focal_length: f64,
    iso: f64,
    exposure_bias: f64,
}

#[repr(C)]
struct SaveOptsC {
    width: c_int,
    height: c_int,
    sixteen_bit: c_int,
    out_width: c_int,
    out_height: c_int,
    icc: *const c_char,
    exif: *const ExifC,
    suffix: *const c_char,
}

type FillFn =
    unsafe extern "C" fn(*mut c_void, c_int, c_int, c_int, c_int, *mut u8, usize) -> c_int;
type CancelFn = unsafe extern "C" fn(*mut c_void) -> c_int;

unsafe extern "C" {
    fn aigle_vips_init() -> c_int;
    fn aigle_vips_last_error() -> *const c_char;
    fn aigle_vips_open(path: *const c_char, h: *mut Header) -> *mut c_void;
    fn aigle_vips_unref(im: *mut c_void);
    fn aigle_vips_get_string(
        im: *mut c_void,
        name: *const c_char,
        buf: *mut c_char,
        len: c_int,
    ) -> c_int;
    fn aigle_vips_decode_xyz(
        im: *mut c_void,
        out: *mut f32,
        cancel: Option<CancelFn>,
        user: *mut c_void,
    ) -> c_int;
    fn aigle_vips_save_file(
        o: *const SaveOptsC,
        fill: FillFn,
        user: *mut c_void,
        path: *const c_char,
    ) -> c_int;
    fn aigle_vips_save_buffer(
        o: *const SaveOptsC,
        fill: FillFn,
        user: *mut c_void,
        buf: *mut *mut c_void,
        len: *mut usize,
    ) -> c_int;
    fn aigle_vips_free(p: *mut c_void);
    fn aigle_vips_decode_rgb8(
        data: *const c_void,
        len: usize,
        w: *mut c_int,
        h: *mut c_int,
        out: *mut *mut u8,
    ) -> c_int;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    Vips(String),
    Cancelled,
    /// The fill callback reported an error (or panicked).
    Fill,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Vips(e) => write!(f, "libvips: {}", e.trim()),
            Error::Cancelled => write!(f, "cancelled"),
            Error::Fill => write!(f, "image generation failed"),
        }
    }
}

impl std::error::Error for Error {}

fn last_error() -> Error {
    // SAFETY: the shim returns a pointer to its static, NUL-terminated buffer.
    Error::Vips(
        unsafe { CStr::from_ptr(aigle_vips_last_error()) }
            .to_string_lossy()
            .into_owned(),
    )
}

/// Initialise libvips (once per process).
pub fn init() -> Result<(), Error> {
    static INIT: std::sync::Mutex<Option<Result<(), Error>>> = std::sync::Mutex::new(None);
    let mut g = INIT.lock().unwrap_or_else(|e| e.into_inner());
    g.get_or_insert_with(|| {
        // SAFETY: plain initialisation call, guarded to run once.
        if unsafe { aigle_vips_init() } == 0 {
            Ok(())
        } else {
            Err(last_error())
        }
    })
    .clone()
}

fn cpath(p: &Path) -> Result<CString, Error> {
    CString::new(p.as_os_str().as_bytes()).map_err(|_| Error::Vips("path contains NUL".into()))
}

/// An opened (not yet decoded) image.
pub struct Image(*mut c_void);

// SAFETY: a VipsImage may be used from any thread; it is not shared between threads here.
unsafe impl Send for Image {}

impl Drop for Image {
    fn drop(&mut self) {
        // SAFETY: owned reference from `aigle_vips_open`.
        unsafe { aigle_vips_unref(self.0) }
    }
}

unsafe extern "C" fn cancel_trampoline(user: *mut c_void) -> c_int {
    // SAFETY: `user` is a `&&dyn Fn() -> bool` living for the duration of the call.
    let f = unsafe { &*(user as *const &dyn Fn() -> bool) };
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).unwrap_or(true) as c_int
}

impl Image {
    /// Open `path` and read its header (cheap: pixels are decoded by [`Image::decode_xyz`]).
    pub fn open(path: &Path) -> Result<(Image, Header), Error> {
        init()?;
        let c = cpath(path)?;
        let mut h = Header::default();
        // SAFETY: valid C string and out-pointer.
        let im = unsafe { aigle_vips_open(c.as_ptr(), &mut h) };
        if im.is_null() {
            return Err(last_error());
        }
        Ok((Image(im), h))
    }

    /// Metadata field as a string, e.g. `exif-ifd0-Make`.
    pub fn get_string(&self, name: &str) -> Option<String> {
        let n = CString::new(name).ok()?;
        let mut buf = vec![0u8; 1024];
        // SAFETY: buffer of the given length; the shim NUL-terminates.
        let rc = unsafe {
            aigle_vips_get_string(
                self.0,
                n.as_ptr(),
                buf.as_mut_ptr().cast(),
                buf.len() as c_int,
            )
        };
        (rc == 0).then(|| {
            CStr::from_bytes_until_nul(&buf)
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default()
        })
    }

    /// Decode into `out` (`width × height × 3` floats of XYZ, D65, white Y = 100),
    /// autorotated.
    pub fn decode_xyz(
        &self,
        h: &Header,
        out: &mut [f32],
        cancelled: &dyn Fn() -> bool,
    ) -> Result<(), Error> {
        assert_eq!(out.len(), h.width as usize * h.height as usize * 3);
        let user: *const &dyn Fn() -> bool = &cancelled;
        // SAFETY: `out` has the decoded size; `user` outlives the call.
        let rc = unsafe {
            aigle_vips_decode_xyz(
                self.0,
                out.as_mut_ptr(),
                Some(cancel_trampoline),
                user as *mut c_void,
            )
        };
        match rc {
            0 => Ok(()),
            1 => Err(Error::Cancelled),
            _ => Err(last_error()),
        }
    }
}

/// EXIF written with an export.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Exif {
    pub make: Option<String>,
    pub model: Option<String>,
    pub lens_make: Option<String>,
    pub lens_model: Option<String>,
    pub software: Option<String>,
    /// `YYYY:MM:DD HH:MM:SS`.
    pub datetime: Option<String>,
    pub artist: Option<String>,
    pub copyright: Option<String>,
    pub exposure_time: Option<f64>,
    pub f_number: Option<f64>,
    pub focal_length: Option<f64>,
    pub iso: Option<f64>,
    /// EV.
    pub exposure_bias: Option<f64>,
}

/// Output of a save.
#[derive(Clone, Debug)]
pub struct SaveOpts<'a> {
    pub width: u32,
    pub height: u32,
    /// 16-bit samples (RGB16), else 8-bit sRGB.
    pub sixteen_bit: bool,
    /// Resize target (linear-light Lanczos-3); `(width, height)` for none.
    pub out_size: (u32, u32),
    /// libvips built-in profile to embed: `"srgb"`, `"p3"`.
    pub icc: Option<&'a str>,
    pub exif: Option<&'a Exif>,
    /// Saver suffix with options, e.g. `.jpg[Q=90]`, `.tif[compression=deflate]`.
    pub suffix: &'a str,
}

/// Fills the RGB samples of rectangle (x, y, w, h); row `j` starts at byte `j · stride` of
/// the slice. Called from libvips worker threads. Returns false on failure (aborts the save).
pub type Fill<'a> = dyn Fn(u32, u32, u32, u32, &mut [u8], usize) -> bool + Sync + 'a;

struct FillCtx<'a> {
    fill: &'a Fill<'a>,
    bpp: usize,
}

unsafe extern "C" fn fill_trampoline(
    user: *mut c_void,
    x: c_int,
    y: c_int,
    w: c_int,
    h: c_int,
    out: *mut u8,
    stride: usize,
) -> c_int {
    // SAFETY: `user` is the `FillCtx` of the running save.
    let c = unsafe { &*(user as *const FillCtx<'_>) };
    let len = stride * (h as usize - 1) + w as usize * c.bpp;
    // SAFETY: libvips hands out a region buffer of `h` rows of `stride` bytes.
    let buf = unsafe { std::slice::from_raw_parts_mut(out, len) };
    let ok = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        (c.fill)(x as u32, y as u32, w as u32, h as u32, buf, stride)
    }))
    .unwrap_or(false);
    if ok { 0 } else { -1 }
}

struct Strings(Vec<CString>);

impl Strings {
    fn opt(&mut self, s: &Option<String>) -> *const c_char {
        match s {
            Some(s) => {
                let c = CString::new(s.replace('\0', "")).expect("no NUL");
                let p = c.as_ptr();
                self.0.push(c);
                p
            }
            None => std::ptr::null(),
        }
    }
}

fn with_opts<R>(o: &SaveOpts<'_>, f: impl FnOnce(&SaveOptsC) -> R) -> Result<R, Error> {
    init()?;
    let mut keep = Strings(Vec::new());
    let exif = o.exif.map(|x| ExifC {
        make: keep.opt(&x.make),
        model: keep.opt(&x.model),
        lens_make: keep.opt(&x.lens_make),
        lens_model: keep.opt(&x.lens_model),
        software: keep.opt(&x.software),
        datetime: keep.opt(&x.datetime),
        artist: keep.opt(&x.artist),
        copyright: keep.opt(&x.copyright),
        exposure_time: x.exposure_time.unwrap_or(0.0),
        f_number: x.f_number.unwrap_or(0.0),
        focal_length: x.focal_length.unwrap_or(0.0),
        iso: x.iso.unwrap_or(0.0),
        exposure_bias: x.exposure_bias.unwrap_or(f64::NAN),
    });
    let icc = keep.opt(&o.icc.map(str::to_owned));
    let suffix = keep.opt(&Some(o.suffix.to_owned()));
    let c = SaveOptsC {
        width: o.width as c_int,
        height: o.height as c_int,
        sixteen_bit: o.sixteen_bit as c_int,
        out_width: o.out_size.0 as c_int,
        out_height: o.out_size.1 as c_int,
        icc,
        exif: exif
            .as_ref()
            .map_or(std::ptr::null(), |e| e as *const ExifC),
        suffix,
    };
    Ok(f(&c))
}

fn bpp(o: &SaveOpts<'_>) -> usize {
    if o.sixteen_bit { 6 } else { 3 }
}

/// Generate the image from `fill` and save it. libvips picks the saver and its options from
/// `path` followed by `opts.suffix` (e.g. `out.tmp` + `.jpg[Q=90]`); the file written is
/// `path` plus the suffix's extension, without the options.
pub fn save_file(opts: &SaveOpts<'_>, fill: &Fill<'_>, path: &Path) -> Result<(), Error> {
    let ctx = FillCtx {
        fill,
        bpp: bpp(opts),
    };
    let p = cpath(path)?;
    let rc = with_opts(opts, |c| {
        // SAFETY: options, context and path outlive the call.
        unsafe {
            aigle_vips_save_file(
                c,
                fill_trampoline,
                &ctx as *const FillCtx<'_> as *mut c_void,
                p.as_ptr(),
            )
        }
    })?;
    if rc == 0 { Ok(()) } else { Err(last_error()) }
}

/// [`save_file`] into memory.
pub fn save_buffer(opts: &SaveOpts<'_>, fill: &Fill<'_>) -> Result<Vec<u8>, Error> {
    let ctx = FillCtx {
        fill,
        bpp: bpp(opts),
    };
    let mut buf: *mut c_void = std::ptr::null_mut();
    let mut len = 0usize;
    let rc = with_opts(opts, |c| {
        // SAFETY: options and context outlive the call; out-pointers to locals.
        unsafe {
            aigle_vips_save_buffer(
                c,
                fill_trampoline,
                &ctx as *const FillCtx<'_> as *mut c_void,
                &mut buf,
                &mut len,
            )
        }
    })?;
    if rc != 0 {
        return Err(last_error());
    }
    // SAFETY: libvips allocated `len` bytes at `buf`; copied, then freed with g_free.
    let v = unsafe { std::slice::from_raw_parts(buf as *const u8, len) }.to_vec();
    // SAFETY: allocated by libvips with g_malloc.
    unsafe { aigle_vips_free(buf) };
    Ok(v)
}

/// Decode an encoded image (any format libvips reads) to 8-bit sRGB RGB.
pub fn decode_rgb8(bytes: &[u8]) -> Result<(u32, u32, Vec<u8>), Error> {
    init()?;
    let (mut w, mut h) = (0, 0);
    let mut out: *mut u8 = std::ptr::null_mut();
    // SAFETY: input slice and out-pointers valid for the call.
    let rc = unsafe {
        aigle_vips_decode_rgb8(bytes.as_ptr().cast(), bytes.len(), &mut w, &mut h, &mut out)
    };
    if rc != 0 {
        return Err(last_error());
    }
    let n = w as usize * h as usize * 3;
    // SAFETY: libvips wrote `w · h · 3` bytes; copied, then freed with g_free.
    let v = unsafe { std::slice::from_raw_parts(out, n) }.to_vec();
    // SAFETY: allocated by libvips with g_malloc.
    unsafe { aigle_vips_free(out.cast()) };
    Ok((w as u32, h as u32, v))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gradient(w: u32, h: u32) -> impl Fn(u32, u32, u32, u32, &mut [u8], usize) -> bool + Sync {
        move |x0, y0, rw, rh, out, stride| {
            for j in 0..rh as usize {
                for i in 0..rw as usize {
                    let (x, y) = (x0 as usize + i, y0 as usize + j);
                    let o = j * stride + i * 3;
                    out[o] = (x * 255 / (w as usize - 1)) as u8;
                    out[o + 1] = (y * 255 / (h as usize - 1)) as u8;
                    out[o + 2] = 128;
                }
            }
            true
        }
    }

    #[test]
    fn round_trip_png_and_resize() {
        let (w, h) = (300, 200);
        let f = gradient(w, h);
        let opts = SaveOpts {
            width: w,
            height: h,
            sixteen_bit: false,
            out_size: (w, h),
            icc: Some("srgb"),
            exif: None,
            suffix: ".png",
        };
        let png = save_buffer(&opts, &f).unwrap();
        let (dw, dh, px) = decode_rgb8(&png).unwrap();
        assert_eq!((dw, dh), (w, h));
        assert_eq!(&px[..3], &[0, 0, 128]);
        assert_eq!(px[(w as usize - 1) * 3], 255);
        let small = save_buffer(
            &SaveOpts {
                out_size: (150, 100),
                suffix: ".jpg[Q=95]",
                ..opts
            },
            &f,
        )
        .unwrap();
        assert_eq!(decode_rgb8(&small).unwrap().0, 150);
    }

    #[test]
    fn decode_xyz_white_is_d65_100() {
        let f = |_, _, rw: u32, rh: u32, out: &mut [u8], stride: usize| {
            for j in 0..rh as usize {
                out[j * stride..j * stride + rw as usize * 3].fill(255);
            }
            true
        };
        let dir = std::env::temp_dir().join(format!("aigle-vips-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("white.tif");
        let opts = SaveOpts {
            width: 32,
            height: 16,
            sixteen_bit: true,
            out_size: (32, 16),
            icc: None,
            exif: Some(&Exif {
                make: Some("Test".into()),
                ..Default::default()
            }),
            suffix: "[compression=deflate]",
        };
        save_file(&opts, &f, &path).unwrap();
        let jpg = dir.join("white");
        save_file(
            &SaveOpts {
                sixteen_bit: false,
                suffix: ".jpg[Q=100]",
                ..opts.clone()
            },
            &f,
            &jpg,
        )
        .unwrap();
        let (im, _) = Image::open(&dir.join("white.jpg")).unwrap();
        assert_eq!(
            im.get_string("exif-ifd0-Make")
                .map(|s| s.starts_with("Test")),
            Some(true)
        );
        let (im, hd) = Image::open(&path).unwrap();
        assert_eq!((hd.width, hd.height), (32, 16));
        let mut xyz = vec![0f32; 32 * 16 * 3];
        im.decode_xyz(&hd, &mut xyz, &|| false).unwrap();
        assert!((xyz[1] - 100.0).abs() < 0.5, "{:?}", &xyz[..3]);
        assert!(
            (xyz[0] - 95.05).abs() < 0.5 && (xyz[2] - 108.9).abs() < 0.5,
            "{:?}",
            &xyz[..3]
        );
        assert_eq!(
            im.decode_xyz(&hd, &mut xyz, &|| true),
            Err(Error::Cancelled)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn fill_failure_aborts_the_save() {
        let opts = SaveOpts {
            width: 64,
            height: 64,
            sixteen_bit: false,
            out_size: (64, 64),
            icc: None,
            exif: None,
            suffix: ".png",
        };
        assert!(save_buffer(&opts, &|_, _, _, _, _, _| false).is_err());
    }
}
