//! Crop tool geometry, independent of GPUI.
//!
//! Rectangles are in the straightened frame in Source px (the coordinates of an uncropped
//! view). A crop is valid when every point of it maps inside the source, so straightening and
//! perspective never show empty corners (Lightroom's "constrain to image").

use aigle_core::geometry::{GeometryMap, Point, Rect, Size};
use aigle_core::params::{Aspect, CropRect, GeometryParams, OrientationDelta};

/// Smallest crop side as a fraction of the frame's short side.
const MIN_SIDE: f64 = 0.02;
/// Tolerance of the inside test in Source px (crop values are rounded to their step).
const EPS: f64 = 0.5;

/// The straightened frame of one geometry: its size and the map used for validity tests.
#[derive(Clone, Copy, Debug)]
pub struct Frame {
    map: GeometryMap,
    pub w: f64,
    pub h: f64,
}

impl Frame {
    /// `source` is the oriented source size including the orientation delta.
    pub fn new(source: Size, g: &GeometryParams) -> Frame {
        let mut full = g.clone();
        full.crop = CropRect::default();
        Frame {
            map: GeometryMap::new(source, &full),
            w: source.w as f64,
            h: source.h as f64,
        }
    }

    fn inside(&self, q: Point) -> bool {
        let p = self.map.crop_to_source(q);
        p.x >= -EPS && p.y >= -EPS && p.x <= self.w + EPS && p.y <= self.h + EPS
    }

    pub fn valid(&self, r: Rect) -> bool {
        let n = if self.map.is_affine() { 1 } else { 8 };
        (0..=n).all(|i| {
            let t = i as f64 / n as f64;
            [
                Point::new(r.x + t * r.w, r.y),
                Point::new(r.x + t * r.w, r.bottom()),
                Point::new(r.x, r.y + t * r.h),
                Point::new(r.right(), r.y + t * r.h),
            ]
            .into_iter()
            .all(|q| self.inside(q))
        })
    }

    pub fn min_side(&self) -> f64 {
        self.w.min(self.h) * MIN_SIDE
    }

    pub fn rect_of(&self, c: &CropRect) -> Rect {
        Rect::new(
            c.x as f64 * self.w,
            c.y as f64 * self.h,
            c.w as f64 * self.w,
            c.h as f64 * self.h,
        )
    }

    pub fn crop_of(&self, r: Rect) -> CropRect {
        let x = (r.x / self.w).clamp(0.0, 1.0);
        let y = (r.y / self.h).clamp(0.0, 1.0);
        CropRect {
            x: x as f32,
            y: y as f32,
            w: (r.w / self.w).clamp(0.001, 1.0 - x) as f32,
            h: (r.h / self.h).clamp(0.001, 1.0 - y) as f32,
        }
    }

    #[cfg(test)]
    pub fn full(&self) -> Rect {
        Rect::new(0.0, 0.0, self.w, self.h)
    }

    /// `r` scaled about its centre until valid (keeping its aspect); the centre moves to the
    /// frame centre first if it is outside the image.
    pub fn shrink_to_fit(&self, r: Rect) -> Rect {
        if self.valid(r) {
            return r;
        }
        let mut c = Point::new(r.x + r.w / 2.0, r.y + r.h / 2.0);
        if !self.inside(c) {
            c = Point::new(self.w / 2.0, self.h / 2.0);
        }
        let at = |s: f64| Rect::new(c.x - r.w * s / 2.0, c.y - r.h * s / 2.0, r.w * s, r.h * s);
        let (mut lo, mut hi) = (0.0, 1.0);
        for _ in 0..40 {
            let m = 0.5 * (lo + hi);
            if self.valid(at(m)) { lo = m } else { hi = m }
        }
        at(lo)
    }

    /// The valid rect closest to `to` on the way from the valid rect `from` (edges move
    /// linearly, so a fixed aspect is kept).
    pub fn clamp_toward(&self, from: Rect, to: Rect) -> Rect {
        if self.valid(to) {
            return to;
        }
        let lerp = |t: f64| {
            let l = from.x + (to.x - from.x) * t;
            let tp = from.y + (to.y - from.y) * t;
            let w = from.w + (to.w - from.w) * t;
            let h = from.h + (to.h - from.h) * t;
            Rect::new(l, tp, w, h)
        };
        let (mut lo, mut hi) = (0.0, 1.0);
        for _ in 0..30 {
            let m = 0.5 * (lo + hi);
            if self.valid(lerp(m)) { lo = m } else { hi = m }
        }
        lerp(lo)
    }

    /// The largest rect of aspect `k` (w/h) centred on `r`'s centre inside the frame,
    /// then made valid.
    pub fn with_aspect(&self, r: Rect, k: f64) -> Rect {
        let c = Point::new(r.x + r.w / 2.0, r.y + r.h / 2.0);
        let half_w = c.x.min(self.w - c.x);
        let half_h = c.y.min(self.h - c.y);
        let (hw, hh) = if half_w / half_h > k {
            (half_h * k, half_h)
        } else {
            (half_w, half_w / k)
        };
        self.shrink_to_fit(Rect::new(c.x - hw, c.y - hh, 2.0 * hw, 2.0 * hh))
    }
}

/// Width over height of `aspect` for a frame of `w × h`; `None` for free.
pub fn ratio(aspect: Aspect, w: f64, h: f64) -> Option<f64> {
    match aspect {
        Aspect::Free => None,
        Aspect::Original => Some(w / h),
        Aspect::Ratio { w, h } if w > 0 && h > 0 => Some(w as f64 / h as f64),
        Aspect::Ratio { .. } => None,
    }
}

/// What a crop drag moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Handle {
    Move,
    N,
    S,
    E,
    W,
    NE,
    NW,
    SE,
    SW,
}

impl Handle {
    pub const ALL: [Handle; 8] = [
        Handle::NW,
        Handle::N,
        Handle::NE,
        Handle::E,
        Handle::SE,
        Handle::S,
        Handle::SW,
        Handle::W,
    ];

    /// Position of the handle on `r` as fractions of its size.
    pub fn anchor(self) -> (f64, f64) {
        match self {
            Handle::Move => (0.5, 0.5),
            Handle::N => (0.5, 0.0),
            Handle::S => (0.5, 1.0),
            Handle::E => (1.0, 0.5),
            Handle::W => (0.0, 0.5),
            Handle::NE => (1.0, 0.0),
            Handle::NW => (0.0, 0.0),
            Handle::SE => (1.0, 1.0),
            Handle::SW => (0.0, 1.0),
        }
    }

    /// Which edges move: (left, top, right, bottom).
    fn edges(self) -> (bool, bool, bool, bool) {
        let (ax, ay) = self.anchor();
        (ax == 0.0, ay == 0.0, ax == 1.0, ay == 1.0)
    }
}

/// The crop after dragging `handle` of `start` by `(dx, dy)` Source px, with aspect `k`
/// locked when given, kept valid.
pub fn drag(f: &Frame, start: Rect, handle: Handle, dx: f64, dy: f64, k: Option<f64>) -> Rect {
    if handle == Handle::Move {
        let x = (start.x + dx).clamp(0.0, (f.w - start.w).max(0.0));
        let y = (start.y + dy).clamp(0.0, (f.h - start.h).max(0.0));
        return f.clamp_toward(start, Rect::new(x, y, start.w, start.h));
    }
    let min = f.min_side();
    let (ml, mt, mr, mb) = handle.edges();
    let (mut l, mut t, mut r, mut b) = (start.x, start.y, start.right(), start.bottom());
    if ml {
        l = (l + dx).clamp(0.0, r - min);
    }
    if mr {
        r = (r + dx).clamp(l + min, f.w);
    }
    if mt {
        t = (t + dy).clamp(0.0, b - min);
    }
    if mb {
        b = (b + dy).clamp(t + min, f.h);
    }
    let mut out = Rect::new(l, t, r - l, b - t);
    if let Some(k) = k {
        let corner = (ml || mr) && (mt || mb);
        let (cx, cy) = (start.x + start.w / 2.0, start.y + start.h / 2.0);
        let (w, h) = if corner {
            if out.w / out.h > k {
                (out.w, out.w / k)
            } else {
                (out.h * k, out.h)
            }
        } else if ml || mr {
            (out.w, out.w / k)
        } else {
            (out.h * k, out.h)
        };
        // Grow from the fixed corner or edge; the other axis stays centred for edges.
        let x = if ml {
            r - w
        } else if mr {
            l
        } else {
            cx - w / 2.0
        };
        let y = if mt {
            b - h
        } else if mb {
            t
        } else {
            cy - h / 2.0
        };
        out = Rect::new(x, y, w, h);
        // Too large for the frame: scale back toward the fixed point.
        let fx = if ml {
            r
        } else if mr {
            l
        } else {
            cx
        };
        let fy = if mt {
            b
        } else if mb {
            t
        } else {
            cy
        };
        let s = [
            if out.x < 0.0 {
                (fx - 0.0) / (fx - out.x)
            } else {
                1.0
            },
            if out.y < 0.0 {
                (fy - 0.0) / (fy - out.y)
            } else {
                1.0
            },
            if out.right() > f.w {
                (f.w - fx) / (out.right() - fx)
            } else {
                1.0
            },
            if out.bottom() > f.h {
                (f.h - fy) / (out.bottom() - fy)
            } else {
                1.0
            },
        ]
        .into_iter()
        .filter(|s| s.is_finite())
        .fold(1.0f64, f64::min)
        .max(0.0);
        if s < 1.0 {
            out = Rect::new(
                fx + (out.x - fx) * s,
                fy + (out.y - fy) * s,
                out.w * s,
                out.h * s,
            );
        }
    }
    f.clamp_toward(start, out)
}

/// The crop after turning the frame a quarter clockwise (`cw`) or counter-clockwise.
pub fn rotate_quarter(c: &CropRect, cw: bool) -> CropRect {
    if cw {
        CropRect {
            x: 1.0 - c.y - c.h,
            y: c.x,
            w: c.h,
            h: c.w,
        }
    } else {
        CropRect {
            x: c.y,
            y: 1.0 - c.x - c.w,
            w: c.h,
            h: c.w,
        }
    }
}

/// Orientation delta after a quarter turn of the displayed image.
pub fn turn(d: OrientationDelta, cw: bool) -> OrientationDelta {
    let q = if cw { 1 } else { 3 };
    OrientationDelta {
        quarter_turns: (d.quarter_turns + q) % 4,
        flip: d.flip,
    }
}

/// Orientation delta after mirroring the displayed image horizontally (the mirror is applied
/// before the rotation, so the rotation reverses).
pub fn mirror(d: OrientationDelta) -> OrientationDelta {
    OrientationDelta {
        quarter_turns: (4 - d.quarter_turns) % 4,
        flip: !d.flip,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aigle_core::geometry::Orientation;

    fn frame(angle: f32) -> Frame {
        let mut g = GeometryParams::default();
        g.scalars.angle = angle;
        Frame::new(Size::new(600, 400), &g)
    }

    #[test]
    fn straightening_shrinks_the_crop_and_regrows_it() {
        let f0 = frame(0.0);
        let full = f0.full();
        assert!(f0.valid(full));
        let f = frame(10.0);
        assert!(!f.valid(full));
        let r = f.shrink_to_fit(full);
        assert!(f.valid(r));
        assert!((r.w / r.h - 1.5).abs() < 1e-9, "aspect kept");
        assert!(r.w > 400.0 && r.w < 600.0, "{r:?}");
        // Slightly larger is invalid: the fit is tight.
        let bigger = Rect::new(r.x - 2.0, r.y - 2.0, r.w + 4.0, r.h + 4.0 / 1.5);
        assert!(!f.valid(bigger));
        assert_eq!(f0.shrink_to_fit(full), full);
    }

    #[test]
    fn handles_resize_with_and_without_aspect() {
        let f = frame(0.0);
        let start = Rect::new(100.0, 100.0, 200.0, 100.0);
        let r = drag(&f, start, Handle::SE, 50.0, 10.0, None);
        assert_eq!((r.x, r.y, r.w, r.h), (100.0, 100.0, 250.0, 110.0));
        let r = drag(&f, start, Handle::SE, 50.0, 10.0, Some(2.0));
        assert!((r.w - 250.0).abs() < 1e-9 && (r.h - 125.0).abs() < 1e-9);
        assert_eq!((r.x, r.y), (100.0, 100.0));
        // An edge with a locked aspect grows the other axis about the centre.
        let r = drag(&f, start, Handle::E, 100.0, 0.0, Some(2.0));
        assert!((r.w - 300.0).abs() < 1e-9 && (r.h - 150.0).abs() < 1e-9);
        assert!((r.y - 75.0).abs() < 1e-9);
        // Moving stops at the frame edge; a too-large locked drag stays in the frame.
        let r = drag(&f, start, Handle::Move, -500.0, 0.0, None);
        assert_eq!((r.x, r.y, r.w), (0.0, 100.0, 200.0));
        let r = drag(&f, start, Handle::SE, 1000.0, 1000.0, Some(2.0));
        assert!(
            r.right() <= 600.0 + 1e-6 && r.bottom() <= 400.0 + 1e-6,
            "{r:?}"
        );
        assert!((r.w / r.h - 2.0).abs() < 1e-6);
        // Not below the minimum size.
        let r = drag(&f, start, Handle::W, 1000.0, 0.0, None);
        assert!((r.w - f.min_side()).abs() < 1e-9);
    }

    #[test]
    fn dragging_into_a_rotated_corner_is_stopped() {
        let f = frame(8.0);
        let start = f.shrink_to_fit(f.full());
        let r = drag(&f, start, Handle::NW, -100.0, -100.0, None);
        assert!(f.valid(r));
        assert!((r.x - start.x).abs() < 1.0 && (r.y - start.y).abs() < 1.0);
    }

    #[test]
    fn aspect_change_keeps_the_centre() {
        let f = frame(0.0);
        let r = f.with_aspect(f.full(), 1.0);
        assert_eq!((r.x, r.y, r.w, r.h), (100.0, 0.0, 400.0, 400.0));
        assert_eq!(ratio(Aspect::Original, 600.0, 400.0), Some(1.5));
        assert_eq!(ratio(Aspect::Free, 600.0, 400.0), None);
    }

    #[test]
    fn quarter_turns_and_mirrors_compose_like_the_display() {
        let c = CropRect {
            x: 0.1,
            y: 0.2,
            w: 0.3,
            h: 0.4,
        };
        let back = rotate_quarter(&rotate_quarter(&c, true), false);
        assert!((back.x - c.x).abs() < 1e-6 && (back.y - c.y).abs() < 1e-6);
        // A crop at the top-left ends up at the top-right after a clockwise turn.
        let r = rotate_quarter(&c, true);
        assert!((r.x - 0.4).abs() < 1e-6 && (r.y - 0.1).abs() < 1e-6);

        let size = Size::new(60, 40);
        let px = |d: OrientationDelta, x: u32, y: u32| {
            Orientation::from_delta(d).apply_pixel(x, y, size)
        };
        let d0 = OrientationDelta::default();
        let d1 = turn(d0, true);
        // Top-left pixel goes to the top-right after one clockwise turn (40 wide now).
        assert_eq!(px(d1, 0, 0), (39, 0));
        // Mirroring the turned image swaps left and right of what is displayed.
        let m = mirror(d1);
        assert_eq!(px(m, 0, 0), (0, 0));
        assert_eq!(mirror(mirror(d1)), d1);
    }
}
