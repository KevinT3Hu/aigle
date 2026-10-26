//! Canvas zoom and pan in Crop space, independent of GPUI (doc 01 §4 "instant pan and zoom").

use aigle_core::geometry::{GridMap, ImageRect, Point, Rect, Size};

pub const MAX_SCALE: f64 = 16.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Zoom {
    /// The whole crop fits the canvas.
    Fit,
    /// Output px per image px.
    Scale(f64),
}

/// Zoom level and centre of one canvas.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewState {
    pub zoom: Zoom,
    /// Centre of the visible region in Crop space; ignored while fitted.
    pub center: (f64, f64),
}

impl Default for ViewState {
    fn default() -> Self {
        ViewState {
            zoom: Zoom::Fit,
            center: (0.0, 0.0),
        }
    }
}

pub fn fit_scale(crop: (f64, f64), out: Size) -> f64 {
    (out.w as f64 / crop.0).min(out.h as f64 / crop.1)
}

impl ViewState {
    pub fn scale(&self, crop: (f64, f64), out: Size) -> f64 {
        match self.zoom {
            Zoom::Fit => fit_scale(crop, out),
            Zoom::Scale(s) => s,
        }
    }

    /// Visible region for an `out`-pixel canvas. Along an axis where the image is smaller than
    /// the canvas it is centred; otherwise the region stays inside the crop.
    pub fn region(&self, crop: (f64, f64), out: Size) -> ImageRect {
        let s = self.scale(crop, out);
        let (rw, rh) = (out.w as f64 / s, out.h as f64 / s);
        let (cx, cy) = match self.zoom {
            Zoom::Fit => (crop.0 / 2.0, crop.1 / 2.0),
            Zoom::Scale(_) => self.center,
        };
        let place = |c: f64, r: f64, extent: f64| {
            if r >= extent {
                (extent - r) / 2.0
            } else {
                (c - r / 2.0).clamp(0.0, extent - r)
            }
        };
        Rect::new(place(cx, rw, crop.0), place(cy, rh, crop.1), rw, rh)
    }

    /// Keep the centre where [`Self::region`] actually put it.
    fn settle(&mut self, crop: (f64, f64), out: Size) {
        let r = self.region(crop, out);
        self.center = (r.x + r.w / 2.0, r.y + r.h / 2.0);
    }

    /// Zoom to `scale`, keeping the image point under canvas pixel `at` fixed. Scales at or
    /// below the fit scale snap to [`Zoom::Fit`].
    pub fn zoom_to(&mut self, scale: f64, at: Point, crop: (f64, f64), out: Size) {
        let fit = fit_scale(crop, out);
        let old = self.region(crop, out);
        let s0 = self.scale(crop, out);
        let p = (old.x + at.x / s0, old.y + at.y / s0);
        let s = scale.min(MAX_SCALE);
        if s <= fit * 1.0001 {
            self.zoom = Zoom::Fit;
            self.settle(crop, out);
            return;
        }
        self.zoom = Zoom::Scale(s);
        let (rw, rh) = (out.w as f64 / s, out.h as f64 / s);
        let (x, y) = (p.0 - at.x / s, p.1 - at.y / s);
        self.center = (x + rw / 2.0, y + rh / 2.0);
        self.settle(crop, out);
    }

    /// Multiply the current scale by `factor` around `at`.
    pub fn zoom_by(&mut self, factor: f64, at: Point, crop: (f64, f64), out: Size) {
        let s = self.scale(crop, out) * factor;
        self.zoom_to(s, at, crop, out);
    }

    /// Fit ↔ 1:1 at `at`.
    pub fn toggle_one_to_one(&mut self, at: Point, crop: (f64, f64), out: Size) {
        match self.zoom {
            Zoom::Scale(_) => {
                self.zoom = Zoom::Fit;
                self.settle(crop, out);
            }
            Zoom::Fit => self.zoom_to(1.0, at, crop, out),
        }
    }

    /// Drag by `dx, dy` canvas pixels.
    pub fn pan(&mut self, dx: f64, dy: f64, crop: (f64, f64), out: Size) {
        if self.zoom == Zoom::Fit {
            return;
        }
        let s = self.scale(crop, out);
        self.center = (self.center.0 - dx / s, self.center.1 - dy / s);
        self.settle(crop, out);
    }
}

/// Where a frame rendered for `frame_region` lands on a canvas currently showing `region` at
/// `out` pixels, in canvas pixels. Used to reproject the previous frame while a new one renders.
pub fn reproject(frame_region: ImageRect, region: ImageRect, out: Size) -> Rect {
    let now = GridMap { region, size: out };
    let old = GridMap {
        region: frame_region,
        size: out,
    };
    now.reproject(&old)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CROP: (f64, f64) = (6000.0, 4000.0);
    const OUT: Size = Size { w: 1200, h: 1000 };

    #[test]
    fn fit_centres_the_crop() {
        let v = ViewState::default();
        let r = v.region(CROP, OUT);
        assert_eq!(v.scale(CROP, OUT), 0.2);
        assert!((r.w - 6000.0).abs() < 1e-9);
        assert!((r.y + 500.0).abs() < 1e-9 && (r.h - 5000.0).abs() < 1e-9);
    }

    #[test]
    fn zoom_keeps_the_point_under_the_cursor() {
        let mut v = ViewState::default();
        let at = Point::new(300.0, 400.0);
        let before = v.region(CROP, OUT);
        let p = (before.x + at.x / 0.2, before.y + at.y / 0.2);
        v.zoom_to(1.0, at, CROP, OUT);
        let r = v.region(CROP, OUT);
        assert_eq!(v.zoom, Zoom::Scale(1.0));
        assert!((r.x + at.x - p.0).abs() < 1e-6 && (r.y + at.y - p.1).abs() < 1e-6);

        v.zoom_by(0.01, at, CROP, OUT);
        assert_eq!(v.zoom, Zoom::Fit);
        v.zoom_by(1e6, at, CROP, OUT);
        assert_eq!(v.zoom, Zoom::Scale(MAX_SCALE));
    }

    #[test]
    fn pan_stays_inside_the_crop() {
        let mut v = ViewState::default();
        v.toggle_one_to_one(Point::new(0.0, 0.0), CROP, OUT);
        let r = v.region(CROP, OUT);
        assert_eq!((r.x, r.y), (0.0, 0.0));
        v.pan(-100.0, -50.0, CROP, OUT);
        let r = v.region(CROP, OUT);
        assert_eq!((r.x, r.y), (100.0, 50.0));
        v.pan(-1e9, -1e9, CROP, OUT);
        let r = v.region(CROP, OUT);
        assert_eq!((r.x, r.y), (4800.0, 3000.0));
        v.toggle_one_to_one(Point::new(0.0, 0.0), CROP, OUT);
        assert_eq!(v.zoom, Zoom::Fit);
    }

    #[test]
    fn reprojection_of_a_panned_frame() {
        let old = Rect::new(100.0, 100.0, 1200.0, 1000.0);
        let now = Rect::new(150.0, 100.0, 1200.0, 1000.0);
        let r = reproject(old, now, OUT);
        assert_eq!((r.x, r.y, r.w, r.h), (-50.0, 0.0, 1200.0, 1000.0));
        let zoomed = Rect::new(100.0, 100.0, 600.0, 500.0);
        let r = reproject(old, zoomed, OUT);
        assert_eq!((r.w, r.h), (2400.0, 2000.0));
    }
}
