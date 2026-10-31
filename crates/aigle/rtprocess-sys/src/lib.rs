//! Safe wrappers over the aigle librtprocess shim.
//!
//! Planes are contiguous `width × height` f32 in the librtprocess scale (0..65535).
//! Demosaic and CA correction poll a `cancelled` closure about every megapixel and stop
//! with [`Error::is_cancelled`] (the managed librtprocess is patched for this).

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
    fn aigle_rp_rcd_parallel(
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
    fn aigle_rp_ca_correct_parallel(
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

/// RCD with up to eight native tile workers (serial fallback on non-GCC toolchains).
/// The callback can run on any tile worker and must be safe to share across threads.
pub fn demosaic_rcd_parallel(
    w: u32,
    h: u32,
    cfa: &mut [f32],
    pattern: [[i32; 2]; 2],
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<Planes, Error> {
    unsafe extern "C" fn poll(user: *mut c_void) -> c_int {
        // SAFETY: pointer and shared closure live through the joined native call.
        let f = unsafe { &*(user as *const &(dyn Fn() -> bool + Sync)) };
        f() as c_int
    }
    let mut out = Planes::new(dims(w, h, cfa.len()));
    let user: *const &(dyn Fn() -> bool + Sync) = &cancelled;
    // SAFETY: all buffers have w*h elements; native tile writes are disjoint and
    // all threads join before returning. The callback is Sync and outlives them.
    check(unsafe {
        aigle_rp_rcd_parallel(
            w as c_int,
            h as c_int,
            cfa.as_mut_ptr(),
            out.r.as_mut_ptr(),
            out.g.as_mut_ptr(),
            out.b.as_mut_ptr(),
            pattern.as_ptr(),
            Some(poll),
            user as *mut c_void,
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

/// Automatic CA using up to eight tile workers and a deterministic raster-order
/// reduction. The callback may run on native workers and must be Sync.
pub fn ca_correct_parallel(
    w: u32,
    h: u32,
    cfa: &mut [f32],
    pattern: [[i32; 2]; 2],
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<(), Error> {
    unsafe extern "C" fn poll(user: *mut c_void) -> c_int {
        // SAFETY: the shared closure lives through the joined native call.
        let f = unsafe { &*(user as *const &(dyn Fn() -> bool + Sync)) };
        f() as c_int
    }
    dims(w, h, cfa.len());
    let user: *const &(dyn Fn() -> bool + Sync) = &cancelled;
    // SAFETY: the CFA has w*h elements, writes are partitioned by tile/row,
    // and native workers join before the CFA or callback can be dropped.
    check(unsafe {
        aigle_rp_ca_correct_parallel(
            w as c_int,
            h as c_int,
            cfa.as_mut_ptr(),
            pattern.as_ptr(),
            Some(poll),
            user as *mut c_void,
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
            .map(|i| (i.wrapping_mul(2654435761) % 60000) as f32)
            .collect()
    }

    #[test]
    fn parallel_rcd_is_bit_identical_and_cancels() {
        // Odd edges, several native tiles per worker, and every Bayer arrangement.
        for pattern in [
            [[0, 1], [1, 2]],
            [[2, 1], [1, 0]],
            [[1, 0], [2, 1]],
            [[1, 2], [0, 1]],
        ] {
            for (w, h) in [(815, 617), (832, 624), (194, 194), (16, 16)] {
                let mut raw = cfa(w, h);
                let serial =
                    demosaic_bayer(BayerAlgo::Rcd, w, h, &mut raw, pattern, &|| false).unwrap();
                let parallel = demosaic_rcd_parallel(w, h, &mut raw, pattern, &|| false).unwrap();
                for (a, b) in [
                    (&serial.r, &parallel.r),
                    (&serial.g, &parallel.g),
                    (&serial.b, &parallel.b),
                ] {
                    let diffs: Vec<_> = a
                        .iter()
                        .zip(b)
                        .enumerate()
                        .filter(|(_, (a, b))| a.to_bits() != b.to_bits())
                        .take(5)
                        .collect();
                    assert!(diffs.is_empty(), "{pattern:?}: {diffs:?}");
                }
            }
        }
        let (w, h) = (1800, 1200);
        let polls = AtomicU32::new(0);
        let err = demosaic_rcd_parallel(w, h, &mut cfa(w, h), [[0, 1], [1, 2]], &|| {
            polls.fetch_add(1, Ordering::Relaxed) >= 1
        })
        .err()
        .expect("cancelled");
        assert!(err.is_cancelled());
    }

    fn ca_fixture(w: u32, h: u32, pattern: [[i32; 2]; 2]) -> Vec<f32> {
        (0..w * h)
            .map(|i| {
                let (x, y) = ((i % w) as f32, (i / w) as f32);
                let channel = pattern[(i / w % 2) as usize][(i % w % 2) as usize];
                let scale = [1.002f32, 1.0, 0.998][channel as usize];
                let sx = (x - w as f32 * 0.5) * scale;
                let sy = (y - h as f32 * 0.5) * scale;
                (0.5 + 0.15 * (sx * 0.1).sin()
                    + 0.15 * (sy * 0.13).cos()
                    + 0.1 * ((sx + sy) * 0.07).sin())
                    * 40000.0
            })
            .collect()
    }

    #[test]
    fn parallel_ca_matches_serial_and_is_repeatable() {
        for pattern in [
            [[0, 1], [1, 2]],
            [[2, 1], [1, 0]],
            [[1, 0], [2, 1]],
            [[1, 2], [0, 1]],
        ] {
            for (w, h) in [(832, 624), (815, 617)] {
                let input = ca_fixture(w, h, pattern);
                let mut serial = input.clone();
                ca_correct(w, h, &mut serial, pattern, &|| false).unwrap();
                assert_ne!(input, serial, "fixture must exercise the correction");
                for _ in 0..2 {
                    let mut parallel = input.clone();
                    ca_correct_parallel(w, h, &mut parallel, pattern, &|| false).unwrap();
                    assert!(
                        serial
                            .iter()
                            .zip(&parallel)
                            .all(|(a, b)| a.to_bits() == b.to_bits()),
                        "{w}x{h} {pattern:?}: serial/parallel differ"
                    );
                }
            }
        }
    }

    #[test]
    fn parallel_ca_cancels_during_tiles() {
        let (w, h) = (1800, 1200);
        let pattern = [[0, 1], [1, 2]];
        let polls = AtomicU32::new(0);
        let result = ca_correct_parallel(w, h, &mut ca_fixture(w, h, pattern), pattern, &|| {
            polls.fetch_add(1, Ordering::Relaxed) >= 2
        });
        assert!(result.unwrap_err().is_cancelled());
        let result = ca_correct_parallel(16, 16, &mut vec![10000.0; 256], pattern, &|| true);
        assert!(result.unwrap_err().is_cancelled());
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
