//! Safe wrappers over the aigle librtprocess shim.
//!
//! Planes are contiguous `width × height` f32 in the librtprocess scale (0..65535).
//! Demosaic and CA correction poll a `cancelled` closure about every megapixel and stop
//! with [`Error::is_cancelled`] (the vendored librtprocess is patched for this).

use std::os::raw::{c_int, c_void};

type CancelFn = unsafe extern "C" fn(*mut c_void) -> c_int;

unsafe extern "C" {
    fn aigle_rp_rcd(
        w: c_int,
        h: c_int,
        cfa: *mut f32,
        r: *mut f32,
        g: *mut f32,
        b: *mut f32,
        p: *const [i32; 2],
        cancel: Option<CancelFn>,
        user: *mut c_void,
    ) -> c_int;
    fn aigle_rp_amaze(
        w: c_int,
        h: c_int,
        cfa: *mut f32,
        r: *mut f32,
        g: *mut f32,
        b: *mut f32,
        p: *const [i32; 2],
        cancel: Option<CancelFn>,
        user: *mut c_void,
    ) -> c_int;
    fn aigle_rp_markesteijn(
        w: c_int,
        h: c_int,
        cfa: *mut f32,
        r: *mut f32,
        g: *mut f32,
        b: *mut f32,
        p: *const [i32; 6],
        rgb_cam: *const [f32; 4],
        passes: c_int,
        cancel: Option<CancelFn>,
        user: *mut c_void,
    ) -> c_int;
    fn aigle_rp_ca_correct(
        w: c_int,
        h: c_int,
        cfa: *mut f32,
        p: *const [i32; 2],
        cancel: Option<CancelFn>,
        user: *mut c_void,
    ) -> c_int;
    fn aigle_rp_hl_inpaint(
        w: c_int,
        h: c_int,
        r: *mut f32,
        g: *mut f32,
        b: *mut f32,
        chmax: *const f32,
        clmax: *const f32,
    ) -> c_int;
}

/// `RP_CANCELLED` of the patched librtprocess.
const CANCELLED: i32 = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Error(pub i32);

impl Error {
    pub fn is_cancelled(&self) -> bool {
        self.0 == CANCELLED
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let what = match self.0 {
            1 => "out of memory",
            2 => "wrong CFA pattern",
            3 => "CA correction failed",
            CANCELLED => "cancelled",
            _ => "internal error",
        };
        write!(f, "librtprocess: {what} ({})", self.0)
    }
}

impl std::error::Error for Error {}

fn check(rc: c_int) -> Result<(), Error> {
    if rc == 0 { Ok(()) } else { Err(Error(rc)) }
}

/// Called by the shim at progress points; `user` is a `&&dyn Fn() -> bool`.
unsafe extern "C" fn cancel_trampoline(user: *mut c_void) -> c_int {
    // SAFETY: `user` points to a `&dyn Fn() -> bool` that outlives the shim call
    // ([`with_cancel`]).
    let f = unsafe { &*(user as *const &dyn Fn() -> bool) };
    f() as c_int
}

/// Run a shim call with `cancelled` as its cancel callback.
fn with_cancel(
    cancelled: &dyn Fn() -> bool,
    call: impl FnOnce(Option<CancelFn>, *mut c_void) -> c_int,
) -> Result<(), Error> {
    let user: *const &dyn Fn() -> bool = &cancelled;
    check(call(Some(cancel_trampoline), user as *mut c_void))
}

/// Output planes of a demosaic.
pub struct Planes {
    pub r: Vec<f32>,
    pub g: Vec<f32>,
    pub b: Vec<f32>,
}

impl Planes {
    fn new(n: usize) -> Planes {
        Planes {
            r: vec![0.0; n],
            g: vec![0.0; n],
            b: vec![0.0; n],
        }
    }
}

fn dims(w: u32, h: u32, len: usize) -> usize {
    let n = w as usize * h as usize;
    assert_eq!(len, n, "plane size mismatch");
    assert!(w >= 16 && h >= 16, "image too small to demosaic");
    n
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BayerAlgo {
    Rcd,
    Amaze,
}

/// Bayer demosaic. `pattern[y][x]` is the colour (0 R, 1 G, 2 B) at (row % 2, col % 2).
/// `cfa` is not modified semantically but librtprocess takes a mutable row table.
pub fn demosaic_bayer(
    algo: BayerAlgo,
    w: u32,
    h: u32,
    cfa: &mut [f32],
    pattern: [[i32; 2]; 2],
    cancelled: &dyn Fn() -> bool,
) -> Result<Planes, Error> {
    let mut out = Planes::new(dims(w, h, cfa.len()));
    let f = match algo {
        BayerAlgo::Rcd => aigle_rp_rcd,
        BayerAlgo::Amaze => aigle_rp_amaze,
    };
    // SAFETY: all planes have w × h elements; pattern is a 2×2 array.
    with_cancel(cancelled, |cb, user| unsafe {
        f(
            w as c_int,
            h as c_int,
            cfa.as_mut_ptr(),
            out.r.as_mut_ptr(),
            out.g.as_mut_ptr(),
            out.b.as_mut_ptr(),
            pattern.as_ptr(),
            cb,
            user,
        )
    })?;
    Ok(out)
}

/// X-Trans demosaic (Markesteijn, `passes` = 1 or 3). `rgb_cam` is camera → sRGB.
pub fn demosaic_xtrans(
    w: u32,
    h: u32,
    cfa: &mut [f32],
    pattern: [[i32; 6]; 6],
    rgb_cam: [[f32; 4]; 3],
    passes: u32,
    cancelled: &dyn Fn() -> bool,
) -> Result<Planes, Error> {
    let mut out = Planes::new(dims(w, h, cfa.len()));
    // SAFETY: all planes have w × h elements; pattern 6×6; rgb_cam 3×4.
    with_cancel(cancelled, |cb, user| unsafe {
        aigle_rp_markesteijn(
            w as c_int,
            h as c_int,
            cfa.as_mut_ptr(),
            out.r.as_mut_ptr(),
            out.g.as_mut_ptr(),
            out.b.as_mut_ptr(),
            pattern.as_ptr(),
            rgb_cam.as_ptr(),
            passes as c_int,
            cb,
            user,
        )
    })?;
    Ok(out)
}

/// Automatic raw chromatic-aberration correction, in place (Bayer only). When cancelled the
/// data is partly corrected.
pub fn ca_correct(
    w: u32,
    h: u32,
    cfa: &mut [f32],
    pattern: [[i32; 2]; 2],
    cancelled: &dyn Fn() -> bool,
) -> Result<(), Error> {
    dims(w, h, cfa.len());
    // SAFETY: cfa has w × h elements.
    with_cancel(cancelled, |cb, user| unsafe {
        aigle_rp_ca_correct(
            w as c_int,
            h as c_int,
            cfa.as_mut_ptr(),
            pattern.as_ptr(),
            cb,
            user,
        )
    })
}

/// Highlight reconstruction by colour propagation/inpainting, in place.
/// `clmax` = per-channel clip level, `chmax` = per-channel maximum of the data.
pub fn highlight_inpaint(
    w: u32,
    h: u32,
    p: &mut Planes,
    chmax: [f32; 3],
    clmax: [f32; 3],
) -> Result<(), Error> {
    let n = dims(w, h, p.r.len());
    assert!(p.g.len() == n && p.b.len() == n);
    // SAFETY: planes have w × h elements; chmax/clmax are 3-element arrays.
    check(unsafe {
        aigle_rp_hl_inpaint(
            w as c_int,
            h as c_int,
            p.r.as_mut_ptr(),
            p.g.as_mut_ptr(),
            p.b.as_mut_ptr(),
            chmax.as_ptr(),
            clmax.as_ptr(),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn cfa(w: u32, h: u32) -> Vec<f32> {
        (0..w * h)
            .map(|i| ((i * 2654435761) % 60000) as f32)
            .collect()
    }

    #[test]
    fn demosaic_stops_when_cancelled() {
        let (w, h) = (1200u32, 900u32);
        let bayer = [[0, 1], [1, 2]];
        for algo in [BayerAlgo::Rcd, BayerAlgo::Amaze] {
            let polls = AtomicU32::new(0);
            let mut c = cfa(w, h);
            let ok = demosaic_bayer(algo, w, h, &mut c, bayer, &|| {
                polls.fetch_add(1, Ordering::Relaxed);
                false
            });
            assert!(ok.is_ok());
            let n = polls.load(Ordering::Relaxed);
            assert!(n >= 3, "{algo:?}: {n} polls");
            let polls = AtomicU32::new(0);
            let err = demosaic_bayer(algo, w, h, &mut c, bayer, &|| {
                polls.fetch_add(1, Ordering::Relaxed) >= 1
            })
            .err()
            .expect("cancelled");
            assert!(err.is_cancelled(), "{err}");
        }
        let err = ca_correct(w, h, &mut cfa(w, h), bayer, &|| true).unwrap_err();
        assert!(err.is_cancelled());
        let xt = [
            [1, 1, 0, 1, 1, 2],
            [1, 1, 2, 1, 1, 0],
            [2, 0, 1, 0, 2, 1],
            [1, 1, 2, 1, 1, 0],
            [1, 1, 0, 1, 1, 2],
            [0, 2, 1, 2, 0, 1],
        ];
        let id = [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
        ];
        let err = demosaic_xtrans(w, h, &mut cfa(w, h), xt, id, 1, &|| true).err();
        assert!(err.is_some_and(|e| e.is_cancelled()));
    }
}
