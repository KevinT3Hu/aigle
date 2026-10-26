//! Coordinate spaces and geometry maps (doc 02 §2).
//!
//! * **Sensor**: raw grid, unrotated.
//! * **Source**: oriented grid (EXIF orientation ∘ user `orientation_delta`); unit = level-0 px.
//! * **Crop**: after lens, perspective, rotation and crop; origin at the crop's top-left.
//! * **Grid**: a raster sampling of a region of Source or Crop at scale `s` (grid px per unit).
//!
//! The engine and the UI share this code so the UI can map cursors and overlays without a
//! round trip (doc 01 §4). All coordinates are continuous: pixel `i` covers `[i, i+1)`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::lens::LensModel;
use crate::params::{GeometryParams, OrientationDelta};

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

impl Point {
    pub const fn new(x: f64, y: f64) -> Self {
        Point { x, y }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct Size {
    pub w: u32,
    pub h: u32,
}

impl Size {
    pub const fn new(w: u32, h: u32) -> Self {
        Size { w, h }
    }
    pub fn long_edge(&self) -> u32 {
        self.w.max(self.h)
    }
    pub fn area(&self) -> u64 {
        self.w as u64 * self.h as u64
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    pub const fn new(x: f64, y: f64, w: f64, h: f64) -> Self {
        Rect { x, y, w, h }
    }
    pub fn right(&self) -> f64 {
        self.x + self.w
    }
    pub fn bottom(&self) -> f64 {
        self.y + self.h
    }
    pub fn intersect(&self, o: &Rect) -> Option<Rect> {
        let x0 = self.x.max(o.x);
        let y0 = self.y.max(o.y);
        let x1 = self.right().min(o.right());
        let y1 = self.bottom().min(o.bottom());
        (x1 > x0 && y1 > y0).then(|| Rect::new(x0, y0, x1 - x0, y1 - y0))
    }
}

/// A point in Crop space (post-crop image coordinates, doc 01 §3).
pub type ImagePoint = Point;
/// A rectangle in Crop space.
pub type ImageRect = Rect;

// ---------------------------------------------------------------------------------------------
// Orientation: the dihedral group D4 as a 2×2 integer matrix acting on centred coordinates.

/// Lossless orientation (EXIF 1–8 and user 90° rotations/flips).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct Orientation {
    m: [[i8; 2]; 2],
}

impl Default for Orientation {
    fn default() -> Self {
        Orientation::IDENTITY
    }
}

impl Orientation {
    pub const IDENTITY: Orientation = Orientation {
        m: [[1, 0], [0, 1]],
    };
    const ROT90_CW: Orientation = Orientation {
        m: [[0, -1], [1, 0]],
    };
    const FLIP_H: Orientation = Orientation {
        m: [[-1, 0], [0, 1]],
    };

    /// EXIF `Orientation` tag: the transform that must be applied to the stored image to
    /// display it upright. Unknown values are treated as 1.
    pub fn from_exif(tag: u16) -> Orientation {
        let m = match tag {
            2 => [[-1, 0], [0, 1]],
            3 => [[-1, 0], [0, -1]],
            4 => [[1, 0], [0, -1]],
            5 => [[0, 1], [1, 0]],
            6 => [[0, -1], [1, 0]],
            7 => [[0, -1], [-1, 0]],
            8 => [[0, 1], [-1, 0]],
            _ => [[1, 0], [0, 1]],
        };
        Orientation { m }
    }

    pub fn to_exif(self) -> u16 {
        (1..=8)
            .find(|&t| Orientation::from_exif(t) == self)
            .unwrap_or(1)
    }

    pub fn from_delta(d: OrientationDelta) -> Orientation {
        let mut o = if d.flip {
            Orientation::FLIP_H
        } else {
            Orientation::IDENTITY
        };
        for _ in 0..(d.quarter_turns % 4) {
            o = o.then(Orientation::ROT90_CW);
        }
        o
    }

    /// `self` followed by `next`.
    pub fn then(self, next: Orientation) -> Orientation {
        let a = next.m;
        let b = self.m;
        let mut m = [[0i8; 2]; 2];
        for (i, row) in m.iter_mut().enumerate() {
            for (j, v) in row.iter_mut().enumerate() {
                *v = a[i][0] * b[0][j] + a[i][1] * b[1][j];
            }
        }
        Orientation { m }
    }

    pub fn inverse(self) -> Orientation {
        // Orthogonal matrix: inverse = transpose.
        let m = self.m;
        Orientation {
            m: [[m[0][0], m[1][0]], [m[0][1], m[1][1]]],
        }
    }

    pub fn swaps_axes(self) -> bool {
        self.m[0][0] == 0
    }

    pub fn apply_size(self, s: Size) -> Size {
        if self.swaps_axes() {
            Size::new(s.h, s.w)
        } else {
            s
        }
    }

    /// Map a continuous point in an image of size `from` to the oriented image.
    pub fn apply_point(self, p: Point, from: Size) -> Point {
        let to = self.apply_size(from);
        let cx = p.x - from.w as f64 / 2.0;
        let cy = p.y - from.h as f64 / 2.0;
        let m = self.m;
        Point::new(
            m[0][0] as f64 * cx + m[0][1] as f64 * cy + to.w as f64 / 2.0,
            m[1][0] as f64 * cx + m[1][1] as f64 * cy + to.h as f64 / 2.0,
        )
    }

    /// Integer pixel index mapping, for lossless reorientation of rasters.
    pub fn apply_pixel(self, x: u32, y: u32, from: Size) -> (u32, u32) {
        let p = self.apply_point(Point::new(x as f64 + 0.5, y as f64 + 0.5), from);
        ((p.x - 0.5).round() as u32, (p.y - 0.5).round() as u32)
    }
}

// ---------------------------------------------------------------------------------------------
// Source ↔ Crop

/// Optical facts for B1 beyond `GeometryParams`: an evaluated lens profile and the
/// 35 mm-equivalent focal length (perspective correction).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Optics {
    pub lens: Option<LensModel>,
    pub focal_35mm: Option<f64>,
}

/// Corrected Source → Source: fill scale ∘ perspective ∘ lens (doc 02 §3 B1, read from the
/// output side).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Correction {
    center: Point,
    /// Corrected-frame zoom (constrain crop); corrected px are divided by it.
    fill: f64,
    /// Homography on centred coordinates, corrected → source, if any, followed by a shift
    /// that keeps the image centre fixed.
    persp: Option<(Mat33, (f64, f64))>,
    lens: Option<LensModel>,
}

impl Correction {
    fn map(&self, c: Point) -> ([Point; 3], f64) {
        let (mut x, mut y) = (
            (c.x - self.center.x) / self.fill,
            (c.y - self.center.y) / self.fill,
        );
        if let Some((h, (ox, oy))) = &self.persp {
            let (nx, ny) = apply_h(h, x, y);
            x = nx - ox;
            y = ny - oy;
        }
        let p = Point::new(x + self.center.x, y + self.center.y);
        match &self.lens {
            Some(l) => l.to_source(p),
            None => ([p; 3], 1.0),
        }
    }
}

/// The composed Source→Crop map for one set of geometry params (B1, doc 02 §5.5).
///
/// From the output side: crop ∘ straighten (rotate by `angle` about the source centre) gives a
/// point of the *corrected* frame; the correction (fill scale, perspective homography, lens
/// distortion and per-channel TCA) maps it to the Source point that is sampled. Without lens
/// and perspective the map is affine and exact. The crop rect is normalised to the source size
/// and expressed in the straightened frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GeometryMap {
    source: Size,
    cos: f64,
    sin: f64,
    center: Point,
    crop: Rect,
    corr: Option<Correction>,
    constrain: bool,
}

/// Perspective: maximum tilt (±100) in degrees.
const MAX_TILT_DEG: f64 = 30.0;

fn rot_x(t: f64) -> [[f64; 3]; 3] {
    let (c, s) = (t.cos(), t.sin());
    [[1.0, 0.0, 0.0], [0.0, c, -s], [0.0, s, c]]
}

fn rot_y(t: f64) -> [[f64; 3]; 3] {
    let (c, s) = (t.cos(), t.sin());
    [[c, 0.0, s], [0.0, 1.0, 0.0], [-s, 0.0, c]]
}

type Mat33 = [[f64; 3]; 3];

fn apply_h(h: &Mat33, x: f64, y: f64) -> (f64, f64) {
    let z = h[2][0] * x + h[2][1] * y + h[2][2];
    (
        (h[0][0] * x + h[0][1] * y + h[0][2]) / z,
        (h[1][0] * x + h[1][1] * y + h[1][2]) / z,
    )
}

fn mul3(a: &[[f64; 3]; 3], b: &[[f64; 3]; 3]) -> [[f64; 3]; 3] {
    let mut m = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            m[i][j] = a[i][0] * b[0][j] + a[i][1] * b[1][j] + a[i][2] * b[2][j];
        }
    }
    m
}

impl GeometryMap {
    /// `source` is the oriented source size (Source space). No lens profile.
    pub fn new(source: Size, g: &GeometryParams) -> GeometryMap {
        GeometryMap::with_optics(source, g, &Optics::default())
    }

    pub fn with_optics(source: Size, g: &GeometryParams, optics: &Optics) -> GeometryMap {
        let a = (g.scalars.angle as f64).to_radians();
        let (w, h) = (source.w as f64, source.h as f64);
        let c = &g.crop;
        let center = Point::new(w / 2.0, h / 2.0);
        let (pv, ph) = (
            g.scalars.perspective_vertical as f64,
            g.scalars.perspective_horizontal as f64,
        );
        let persp = (pv != 0.0 || ph != 0.0).then(|| {
            // Virtual camera: f in px from the 35 mm-equivalent focal length (default 50 mm).
            let f =
                w.hypot(h) * optics.focal_35mm.filter(|f| *f > 0.0).unwrap_or(50.0) / 43.266_615;
            let r = mul3(
                &rot_x((-pv / 100.0 * MAX_TILT_DEG).to_radians()),
                &rot_y((ph / 100.0 * MAX_TILT_DEG).to_radians()),
            );
            // source ray = R · (x, y, f); p = f · (rx/rz, ry/rz)  ⇒  H = K R K⁻¹, K = diag(f, f, 1).
            let k = [[f, 0.0, 0.0], [0.0, f, 0.0], [0.0, 0.0, 1.0]];
            let kinv = [[1.0 / f, 0.0, 0.0], [0.0, 1.0 / f, 0.0], [0.0, 0.0, 1.0]];
            let hm = mul3(&k, &mul3(&r, &kinv));
            (hm, apply_h(&hm, 0.0, 0.0))
        });
        let lens = optics.lens.filter(|l| !l.is_identity());
        let corr = (persp.is_some() || lens.is_some()).then(|| {
            let mut corr = Correction {
                center,
                fill: 1.0,
                persp,
                lens,
            };
            if g.constrain {
                corr.fill = fill_scale(&corr, w, h);
            }
            corr
        });
        GeometryMap {
            source,
            cos: a.cos(),
            sin: a.sin(),
            center,
            crop: Rect::new(
                c.x as f64 * w,
                c.y as f64 * h,
                c.w as f64 * w,
                c.h as f64 * h,
            ),
            corr,
            constrain: g.constrain,
        }
    }

    /// True when the map is affine (no lens, no perspective).
    pub fn is_affine(&self) -> bool {
        self.corr.is_none()
    }

    /// Corrected-frame zoom of the constrain crop (1 without correction). Grid px per Source
    /// px is the render scale times this.
    pub fn fill(&self) -> f64 {
        self.corr.map_or(1.0, |c| c.fill)
    }

    /// The correction can map Crop points outside the source (constrain off).
    pub fn may_leave_source(&self) -> bool {
        self.corr.is_some() && !self.constrain
    }

    /// Lens TCA active (per-channel sampling needed).
    pub fn has_tca(&self) -> bool {
        self.corr.and_then(|c| c.lens).is_some_and(|l| l.has_tca())
    }

    /// Crop → corrected frame (straightening only).
    fn crop_to_corrected(&self, q: ImagePoint) -> Point {
        let rx = q.x + self.crop.x - self.center.x;
        let ry = q.y + self.crop.y - self.center.y;
        Point::new(
            self.cos * rx + self.sin * ry + self.center.x,
            -self.sin * rx + self.cos * ry + self.center.y,
        )
    }

    /// Source points sampled for R, G, B at Crop point `q`, and the vignetting gain.
    pub fn crop_to_source_rgb(&self, q: ImagePoint) -> ([Point; 3], f64) {
        let c = self.crop_to_corrected(q);
        match &self.corr {
            None => ([c; 3], 1.0),
            Some(k) => k.map(c),
        }
    }

    pub fn source_size(&self) -> Size {
        self.source
    }

    /// Size of Crop space in source-pixel units.
    pub fn crop_extent(&self) -> (f64, f64) {
        (self.crop.w, self.crop.h)
    }

    /// Crop rect in the straightened frame, in source px.
    pub fn crop_rect(&self) -> Rect {
        self.crop
    }

    pub fn crop_bounds(&self) -> ImageRect {
        Rect::new(0.0, 0.0, self.crop.w, self.crop.h)
    }

    pub fn is_identity_rotation(&self) -> bool {
        self.sin == 0.0
    }

    pub fn source_to_crop(&self, p: Point) -> ImagePoint {
        // Invert the correction by Newton iteration (numeric Jacobian), then straighten.
        let c = match &self.corr {
            None => p,
            Some(k) => {
                let mut c = p;
                for _ in 0..30 {
                    let f = k.map(c).0[1];
                    let (ex, ey) = (f.x - p.x, f.y - p.y);
                    if ex.abs() < 1e-9 && ey.abs() < 1e-9 {
                        break;
                    }
                    let e = 1e-3;
                    let fx = k.map(Point::new(c.x + e, c.y)).0[1];
                    let fy = k.map(Point::new(c.x, c.y + e)).0[1];
                    let (a, b, cc, d) = (
                        (fx.x - f.x) / e,
                        (fy.x - f.x) / e,
                        (fx.y - f.y) / e,
                        (fy.y - f.y) / e,
                    );
                    let det = a * d - b * cc;
                    if det.abs() < 1e-12 {
                        break;
                    }
                    c = Point::new(
                        c.x - (d * ex - b * ey) / det,
                        c.y - (-cc * ex + a * ey) / det,
                    );
                }
                c
            }
        };
        let dx = c.x - self.center.x;
        let dy = c.y - self.center.y;
        let rx = self.cos * dx - self.sin * dy + self.center.x;
        let ry = self.sin * dx + self.cos * dy + self.center.y;
        Point::new(rx - self.crop.x, ry - self.crop.y)
    }

    /// Source point sampled (green channel) for Crop point `q`.
    pub fn crop_to_source(&self, q: ImagePoint) -> Point {
        self.crop_to_source_rgb(q).0[1]
    }

    /// Axis-aligned bounds in Source space of a Crop-space rectangle (all channels; the
    /// boundary is sampled densely when the map is not affine).
    pub fn crop_rect_to_source_bounds(&self, r: &ImageRect) -> Rect {
        let n = if self.is_affine() { 1 } else { 16 };
        let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for i in 0..=n {
            let t = i as f64 / n as f64;
            for q in [
                Point::new(r.x + t * r.w, r.y),
                Point::new(r.x + t * r.w, r.bottom()),
                Point::new(r.x, r.y + t * r.h),
                Point::new(r.right(), r.y + t * r.h),
            ] {
                for c in self.crop_to_source_rgb(q).0 {
                    x0 = x0.min(c.x);
                    y0 = y0.min(c.y);
                    x1 = x1.max(c.x);
                    y1 = y1.max(c.y);
                }
            }
        }
        Rect::new(x0, y0, x1 - x0, y1 - y0)
    }

    /// Normalised Source coordinates (mask anchoring, doc 02 §2) to Source px.
    pub fn denormalize(&self, p: crate::masks::Pt) -> Point {
        Point::new(
            p.x as f64 * self.source.w as f64,
            p.y as f64 * self.source.h as f64,
        )
    }
}

/// Smallest zoom of the corrected frame for which the whole frame maps inside the source
/// (no empty areas): bisection on the boundary (doc 02 §3 B1 "constrain").
fn fill_scale(corr: &Correction, w: f64, h: f64) -> f64 {
    let inside = |k: f64| {
        let c = Correction { fill: k, ..*corr };
        let n = 32;
        (0..=n).all(|i| {
            let t = i as f64 / n as f64;
            [
                Point::new(t * w, 0.0),
                Point::new(t * w, h),
                Point::new(0.0, t * h),
                Point::new(w, t * h),
            ]
            .iter()
            .all(|&q| {
                c.map(q)
                    .0
                    .iter()
                    .all(|p| p.x >= -1e-6 && p.y >= -1e-6 && p.x <= w + 1e-6 && p.y <= h + 1e-6)
            })
        })
    };
    let (mut lo, mut hi) = (0.25, 8.0);
    if !inside(hi) {
        return 1.0;
    }
    for _ in 0..50 {
        let m = 0.5 * (lo + hi);
        if inside(m) { hi = m } else { lo = m }
    }
    hi
}

// ---------------------------------------------------------------------------------------------
// Grid ↔ image

/// Maps between an output raster of `size` pixels and the `region` of Crop space it shows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GridMap {
    pub region: Rect,
    pub size: Size,
}

impl GridMap {
    pub fn scale_x(&self) -> f64 {
        self.size.w as f64 / self.region.w
    }
    pub fn scale_y(&self) -> f64 {
        self.size.h as f64 / self.region.h
    }
    pub fn to_image(&self, px: Point) -> ImagePoint {
        Point::new(
            self.region.x + px.x / self.scale_x(),
            self.region.y + px.y / self.scale_y(),
        )
    }
    pub fn to_grid(&self, p: ImagePoint) -> Point {
        Point::new(
            (p.x - self.region.x) * self.scale_x(),
            (p.y - self.region.y) * self.scale_y(),
        )
    }

    /// Where a frame rendered for `old` must be drawn inside this grid to stay registered:
    /// used to reproject the previous frame instantly while a new one renders (doc 01 §4).
    pub fn reproject(&self, old: &GridMap) -> Rect {
        let a = self.to_grid(Point::new(old.region.x, old.region.y));
        let b = self.to_grid(Point::new(old.region.right(), old.region.bottom()));
        Rect::new(a.x, a.y, b.x - a.x, b.y - a.y)
    }
}

/// Fit-to-size helper: scale `extent` to fit in `long_edge` px, keeping the aspect ratio.
pub fn fit_long_edge(extent: (f64, f64), long_edge: u32) -> Size {
    let (w, h) = extent;
    let s = long_edge as f64 / w.max(h);
    Size::new(
        ((w * s).round() as u32).max(1),
        ((h * s).round() as u32).max(1),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Point, b: Point) -> bool {
        (a.x - b.x).abs() < 1e-9 && (a.y - b.y).abs() < 1e-9
    }

    #[test]
    fn exif_orientations_round_trip_and_compose() {
        let s = Size::new(6, 4);
        for t in 1..=8 {
            let o = Orientation::from_exif(t);
            assert_eq!(o.to_exif(), t);
            assert_eq!(o.then(o.inverse()), Orientation::IDENTITY);
            let p = Point::new(1.25, 3.5);
            let q = o.apply_point(p, s);
            assert!(close(o.inverse().apply_point(q, o.apply_size(s)), p));
        }
        // 6 = rotate 90° CW: top-left goes to top-right.
        let o = Orientation::from_exif(6);
        assert!(close(
            o.apply_point(Point::new(0.0, 0.0), s),
            Point::new(4.0, 0.0)
        ));
        assert_eq!(o.apply_pixel(0, 0, s), (3, 0));
        // Two quarter turns = 180°.
        let d = OrientationDelta {
            quarter_turns: 2,
            flip: false,
        };
        assert_eq!(Orientation::from_delta(d), Orientation::from_exif(3));
    }

    #[test]
    fn geometry_map_inverts() {
        let mut g = GeometryParams::default();
        g.scalars.angle = 7.5;
        g.crop.x = 0.1;
        g.crop.y = 0.2;
        g.crop.w = 0.6;
        g.crop.h = 0.5;
        let m = GeometryMap::new(Size::new(6000, 4000), &g);
        for p in [
            Point::new(0.0, 0.0),
            Point::new(123.4, 987.6),
            Point::new(3000.0, 2000.0),
        ] {
            assert!(close(m.source_to_crop(m.crop_to_source(p)), p));
        }
        let (w, h) = m.crop_extent();
        assert!((w - 3600.0).abs() < 1e-3 && (h - 2000.0).abs() < 1e-3);
    }

    #[test]
    fn perspective_and_lens_compose_and_invert() {
        use crate::lens::*;
        let size = Size::new(6000, 4000);
        let mut g = GeometryParams::default();
        g.scalars.perspective_vertical = 40.0;
        let m = GeometryMap::new(size, &g);
        // +vertical corrects converging verticals: the top row samples a narrower span.
        let (cw, ch) = m.crop_extent();
        let span =
            |y: f64| m.crop_to_source(Point::new(cw, y)).x - m.crop_to_source(Point::new(0.0, y)).x;
        assert!(span(0.0) < span(ch) * 0.95, "{} vs {}", span(0.0), span(ch));
        // Constrain: every frame point maps inside the source.
        for q in [
            Point::new(0.0, 0.0),
            Point::new(cw, 0.0),
            Point::new(0.0, ch),
            Point::new(cw, ch),
        ] {
            let p = m.crop_to_source(q);
            assert!(
                p.x >= -1e-3 && p.y >= -1e-3 && p.x <= 6000.001 && p.y <= 4000.001,
                "{p:?}"
            );
        }
        let prof = LensProfileData {
            lens_maker: "T".into(),
            lens_model: "T".into(),
            calib_crop: 1.0,
            calib_aspect: 1.5,
            camera_crop: 1.0,
            min_focal: 24.0,
            max_focal: 24.0,
            distortion: vec![DistEntry {
                focal: 24.0,
                real_focal: None,
                model: DistModel::Ptlens {
                    a: 0.01,
                    b: -0.05,
                    c: 0.02,
                },
            }],
            tca: vec![TcaEntry {
                focal: 24.0,
                model: TcaModel::Linear {
                    kr: 1.0004,
                    kb: 0.9996,
                },
            }],
            vignetting: vec![],
        };
        let lens = prof.model(
            &LensShot {
                focal: 24.0,
                aperture: 4.0,
                distance: 1000.0,
            },
            size,
            [1.0; 3],
        );
        g.scalars.angle = 3.0;
        let m = GeometryMap::with_optics(
            size,
            &g,
            &Optics {
                lens: Some(lens),
                focal_35mm: Some(24.0),
            },
        );
        assert!(!m.is_affine() && m.has_tca());
        for q in [
            Point::new(100.0, 200.0),
            Point::new(2500.0, 1800.0),
            Point::new(5000.0, 3000.0),
        ] {
            let back = m.source_to_crop(m.crop_to_source(q));
            assert!(
                (back.x - q.x).abs() < 1e-4 && (back.y - q.y).abs() < 1e-4,
                "{q:?} → {back:?}"
            );
        }
    }

    #[test]
    fn grid_reprojection() {
        let new = GridMap {
            region: Rect::new(0.0, 0.0, 200.0, 100.0),
            size: Size::new(400, 200),
        };
        let old = GridMap {
            region: Rect::new(50.0, 25.0, 100.0, 50.0),
            size: Size::new(400, 200),
        };
        assert_eq!(new.reproject(&old), Rect::new(100.0, 50.0, 200.0, 100.0));
    }
}
