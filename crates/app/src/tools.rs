//! Canvas tools: hit testing and painting of the crop rectangle and of local-adjustment
//! shapes, plus the edits their drags produce.

use aigle_core::geometry::{GeometryMap, ImagePoint, ImageRect, Point as ImgPoint, Size};
use aigle_core::masks::{LocalAdjustment, LocalParams, MaskShape, Pt, Stroke};
use aigle_core::{EditParams, MaskId};
use aigle_doc::ParamOp;
use gpui::{Bounds, PathBuilder, Pixels, Point, Window, fill, point, px, size};

use crate::crop::Handle;

/// Grab distance of handles, logical px.
pub const GRAB: f32 = 9.0;

/// Canvas placement of the region a frame shows.
#[derive(Clone, Copy, Debug)]
pub struct Mapping {
    pub origin: Point<Pixels>,
    pub region: ImageRect,
    /// Physical canvas px per image unit.
    pub scale: f64,
    pub sf: f32,
}

impl Mapping {
    pub fn to_window(self, p: ImgPoint) -> Point<Pixels> {
        let k = self.scale / self.sf as f64;
        self.origin
            + point(
                px(((p.x - self.region.x) * k) as f32),
                px(((p.y - self.region.y) * k) as f32),
            )
    }

    pub fn to_image(self, p: Point<Pixels>) -> ImgPoint {
        let k = self.sf as f64 / self.scale;
        ImgPoint::new(
            self.region.x + f32::from(p.x - self.origin.x) as f64 * k,
            self.region.y + f32::from(p.y - self.origin.y) as f64 * k,
        )
    }

    /// Logical px of `d` image units.
    pub fn len(&self, d: f64) -> f32 {
        (d * self.scale / self.sf as f64) as f32
    }
}

fn dist(a: Point<Pixels>, b: Point<Pixels>) -> f32 {
    f32::from(a.x - b.x).hypot(f32::from(a.y - b.y))
}

fn line(window: &mut Window, a: Point<Pixels>, b: Point<Pixels>, w: f32, color: gpui::Rgba) {
    let mut p = PathBuilder::stroke(px(w));
    p.move_to(a);
    p.line_to(b);
    if let Ok(path) = p.build() {
        window.paint_path(path, color);
    }
}

/// White line with a dark outline so it reads on any image.
fn outlined(window: &mut Window, a: Point<Pixels>, b: Point<Pixels>) {
    line(window, a, b, 3.0, gpui::rgba(0x00000090));
    line(window, a, b, 1.0, gpui::rgba(0xffffffe0));
}

fn dot(window: &mut Window, c: Point<Pixels>, r: f32, filled: bool) {
    let r = px(r);
    let q = fill(
        Bounds::new(c - point(r, r), size(r * 2.0, r * 2.0)),
        if filled {
            gpui::rgba(0xffffffff)
        } else {
            gpui::rgba(0x202020e0)
        },
    )
    .corner_radii(r)
    .border_widths(px(1.5))
    .border_color(if filled {
        gpui::rgba(0x000000c0)
    } else {
        gpui::rgba(0xffffffff)
    });
    window.paint_quad(q);
}

fn polyline(window: &mut Window, pts: &[Point<Pixels>]) {
    for (w, color) in [(3.0, gpui::rgba(0x00000090)), (1.0, gpui::rgba(0xffffffe0))] {
        let mut b = PathBuilder::stroke(px(w));
        for (i, p) in pts.iter().enumerate() {
            if i == 0 {
                b.move_to(*p);
            } else {
                b.line_to(*p);
            }
        }
        if let Ok(path) = b.build() {
            window.paint_path(path, color);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Crop

/// What a crop drag starting at `p` does: a handle, `Move` inside, `None` (rotate) outside.
pub fn crop_hit(m: &Mapping, r: ImageRect, p: Point<Pixels>) -> Option<Handle> {
    for h in Handle::ALL {
        let (ax, ay) = h.anchor();
        let c = m.to_window(ImgPoint::new(r.x + ax * r.w, r.y + ay * r.h));
        if dist(c, p) <= GRAB {
            return Some(h);
        }
    }
    let q = m.to_image(p);
    (q.x >= r.x && q.x <= r.right() && q.y >= r.y && q.y <= r.bottom()).then_some(Handle::Move)
}

pub fn crop_cursor(h: Option<Handle>) -> gpui::CursorStyle {
    use gpui::CursorStyle as C;
    match h {
        None => C::Crosshair,
        Some(Handle::Move) => C::PointingHand,
        Some(Handle::N | Handle::S) => C::ResizeUpDown,
        Some(Handle::E | Handle::W) => C::ResizeLeftRight,
        Some(Handle::NW | Handle::SE) => C::ResizeUpLeftDownRight,
        Some(Handle::NE | Handle::SW) => C::ResizeUpRightDownLeft,
    }
}

/// Dim outside the crop, its border, rule-of-thirds lines and the handles.
pub fn paint_crop(window: &mut Window, m: &Mapping, r: ImageRect, canvas: Bounds<Pixels>) {
    let tl = m.to_window(ImgPoint::new(r.x, r.y));
    let br = m.to_window(ImgPoint::new(r.right(), r.bottom()));
    let dim = gpui::rgba(0x000000a8);
    let (l, t, rr, b) = (canvas.left(), canvas.top(), canvas.right(), canvas.bottom());
    for q in [
        Bounds::from_corners(point(l, t), point(rr, tl.y)),
        Bounds::from_corners(point(l, br.y), point(rr, b)),
        Bounds::from_corners(point(l, tl.y), point(tl.x, br.y)),
        Bounds::from_corners(point(br.x, tl.y), point(rr, br.y)),
    ] {
        if q.size.width > px(0.0) && q.size.height > px(0.0) {
            window.paint_quad(fill(q, dim));
        }
    }
    let third = |a: Pixels, b: Pixels, k: f32| a + (b - a) * k;
    for k in [1.0 / 3.0, 2.0 / 3.0] {
        let x = third(tl.x, br.x, k);
        let y = third(tl.y, br.y, k);
        line(
            window,
            point(x, tl.y),
            point(x, br.y),
            1.0,
            gpui::rgba(0xffffff50),
        );
        line(
            window,
            point(tl.x, y),
            point(br.x, y),
            1.0,
            gpui::rgba(0xffffff50),
        );
    }
    let tr = point(br.x, tl.y);
    let bl = point(tl.x, br.y);
    polyline(window, &[tl, tr, br, bl, tl]);
    for h in Handle::ALL {
        let (ax, ay) = h.anchor();
        let c = point(third(tl.x, br.x, ax as f32), third(tl.y, br.y, ay as f32));
        let s = px(4.0);
        window.paint_quad(
            fill(
                Bounds::new(c - point(s, s), size(s * 2.0, s * 2.0)),
                gpui::rgba(0xffffffff),
            )
            .border_widths(px(1.0))
            .border_color(gpui::rgba(0x000000c0)),
        );
    }
}

// ---------------------------------------------------------------------------------------------
// Masks

/// Maps normalised Source points (mask anchoring) to Crop space and back.
#[derive(Clone, Copy, Debug)]
pub struct MaskSpace {
    map: GeometryMap,
    w: f64,
    h: f64,
}

impl MaskSpace {
    /// `source` is the oriented source size including the orientation delta.
    pub fn new(source: Size, p: &EditParams) -> MaskSpace {
        MaskSpace {
            map: GeometryMap::new(source, &p.geometry),
            w: source.w as f64,
            h: source.h as f64,
        }
    }

    #[cfg(test)]
    pub fn to_crop(self, p: Pt) -> ImagePoint {
        self.map
            .source_to_crop(ImgPoint::new(p.x as f64 * self.w, p.y as f64 * self.h))
    }

    pub fn norm_of(&self, q: ImagePoint) -> Pt {
        let s = self.map.crop_to_source(q);
        Pt {
            x: (s.x / self.w) as f32,
            y: (s.y / self.h) as f32,
        }
    }

    /// Source px of a normalised point.
    pub fn src(&self, p: Pt) -> (f64, f64) {
        (p.x as f64 * self.w, p.y as f64 * self.h)
    }

    pub fn norm(&self, x: f64, y: f64) -> Pt {
        Pt {
            x: (x / self.w) as f32,
            y: (y / self.h) as f32,
        }
    }

    pub fn long_edge(&self) -> f64 {
        self.w.max(self.h)
    }
}

/// Kinds of mask the panel can add.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NewMask {
    Linear,
    Radial,
    Brush,
}

impl NewMask {
    pub fn label(self) -> String {
        match self {
            NewMask::Linear => crate::i18n::t("线性渐变"),
            NewMask::Radial => crate::i18n::t("径向渐变"),
            NewMask::Brush => crate::i18n::t("画笔"),
        }
    }
}

pub fn kind_label(s: &MaskShape) -> String {
    match s {
        MaskShape::Linear { .. } => crate::i18n::t("线性渐变"),
        MaskShape::Radial { .. } => crate::i18n::t("径向渐变"),
        MaskShape::Brush { .. } => crate::i18n::t("画笔"),
        MaskShape::Semantic { .. } => crate::i18n::t("智能"),
        MaskShape::Range { .. } => crate::i18n::t("范围"),
        MaskShape::Combine { .. } => crate::i18n::t("组合"),
    }
}

/// A fresh adjustment of kind `k` starting at `at` (normalised Source).
pub fn new_mask(p: &EditParams, k: NewMask, at: Pt) -> LocalAdjustment {
    let id = MaskId(p.masks.iter().map(|m| m.id.0 + 1).max().unwrap_or(1));
    let n = p.masks.len() + 1;
    let shape = match k {
        NewMask::Linear => MaskShape::Linear { from: at, to: at },
        NewMask::Radial => MaskShape::Radial {
            center: at,
            radii: [0.0, 0.0],
            angle: 0.0,
            feather: 50.0,
        },
        NewMask::Brush => MaskShape::Brush {
            strokes: Vec::new(),
        },
    };
    LocalAdjustment {
        id,
        name: rust_i18n::t!("蒙版 {n}", n = n).into_owned(),
        shape,
        invert: false,
        opacity: 1.0,
        adjust: LocalParams::default(),
    }
}

/// Part of a mask shape a drag moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    LinearFrom,
    LinearTo,
    LinearBoth,
    RadialCenter,
    /// The first axis: its length and the ellipse angle.
    RadialMajor,
    RadialMinor,
}

/// Handle positions (Source px) of a shape.
fn handles(sp: &MaskSpace, s: &MaskShape) -> Vec<(Part, (f64, f64))> {
    match s {
        MaskShape::Linear { from, to } => {
            let (a, b) = (sp.src(*from), sp.src(*to));
            vec![
                (Part::LinearFrom, a),
                (Part::LinearTo, b),
                (Part::LinearBoth, ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0)),
            ]
        }
        MaskShape::Radial {
            center,
            radii,
            angle,
            ..
        } => {
            let c = sp.src(*center);
            let (rx, ry) = (radii[0] as f64 * sp.w, radii[1] as f64 * sp.h);
            let a = (*angle as f64).to_radians();
            let (co, si) = (a.cos(), a.sin());
            vec![
                (Part::RadialCenter, c),
                (Part::RadialMajor, (c.0 + rx * co, c.1 + rx * si)),
                (Part::RadialMinor, (c.0 - ry * si, c.1 + ry * co)),
            ]
        }
        _ => Vec::new(),
    }
}

fn to_window(sp: &MaskSpace, m: &Mapping, p: (f64, f64)) -> Point<Pixels> {
    m.to_window(sp.map.source_to_crop(ImgPoint::new(p.0, p.1)))
}

pub fn mask_hit(sp: &MaskSpace, m: &Mapping, s: &MaskShape, p: Point<Pixels>) -> Option<Part> {
    handles(sp, s)
        .into_iter()
        .map(|(part, h)| (part, dist(to_window(sp, m, h), p)))
        .filter(|(_, d)| *d <= GRAB)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(part, _)| part)
}

/// `start` with `part` dragged to the normalised point `at` (moved by `delta` for parts
/// that translate).
pub fn drag_shape(sp: &MaskSpace, start: &MaskShape, part: Part, at: Pt, delta: Pt) -> MaskShape {
    let add = |p: Pt| Pt {
        x: p.x + delta.x,
        y: p.y + delta.y,
    };
    match (start, part) {
        (MaskShape::Linear { to, .. }, Part::LinearFrom) => MaskShape::Linear { from: at, to: *to },
        (MaskShape::Linear { from, .. }, Part::LinearTo) => MaskShape::Linear {
            from: *from,
            to: at,
        },
        (MaskShape::Linear { from, to }, Part::LinearBoth) => MaskShape::Linear {
            from: add(*from),
            to: add(*to),
        },
        (
            MaskShape::Radial {
                center,
                radii,
                angle,
                feather,
            },
            part,
        ) => {
            let c = sp.src(*center);
            let q = sp.src(at);
            let (dx, dy) = (q.0 - c.0, q.1 - c.1);
            let (mut radii, mut angle, mut center) = (*radii, *angle, *center);
            match part {
                Part::RadialCenter => center = add(center),
                Part::RadialMajor => {
                    angle = dy.atan2(dx).to_degrees() as f32;
                    radii[0] = (dx.hypot(dy) / sp.w) as f32;
                }
                Part::RadialMinor => {
                    let a = (angle as f64).to_radians();
                    let along = -dx * a.sin() + dy * a.cos();
                    radii[1] = (along.abs() / sp.h) as f32;
                }
                _ => {}
            }
            MaskShape::Radial {
                center,
                radii,
                angle,
                feather: *feather,
            }
        }
        (s, _) => s.clone(),
    }
}

/// The shape of a mask being created by dragging from `from` to `to` (normalised Source).
pub fn create_shape(sp: &MaskSpace, start: &MaskShape, from: Pt, to: Pt) -> MaskShape {
    match start {
        MaskShape::Linear { .. } => MaskShape::Linear { from, to },
        MaskShape::Radial { feather, .. } => {
            let (a, b) = (sp.src(from), sp.src(to));
            let r = (b.0 - a.0).hypot(b.1 - a.1);
            MaskShape::Radial {
                center: from,
                radii: [(r / sp.w) as f32, (r / sp.h) as f32],
                angle: 0.0,
                feather: *feather,
            }
        }
        s => s.clone(),
    }
}

/// The brush shape with `p` appended to its last stroke, or a new stroke started.
pub fn brush_add(start: &MaskShape, p: Pt, new_stroke: Option<Stroke>) -> MaskShape {
    let MaskShape::Brush { strokes } = start else {
        return start.clone();
    };
    let mut strokes = strokes.clone();
    match new_stroke {
        Some(mut s) => {
            s.points = vec![p];
            strokes.push(s);
        }
        None => {
            if let Some(s) = strokes.last_mut() {
                s.points.push(p);
            }
        }
    }
    MaskShape::Brush { strokes }
}

/// Ops replacing the shape of mask `index`.
pub fn shape_ops(index: usize, s: &MaskShape) -> Vec<ParamOp> {
    vec![ParamOp::set(
        &format!("masks.{index}.shape"),
        serde_json::to_value(s).expect("serialisable"),
    )]
}

/// Outline and handles of the selected shape.
pub fn paint_mask(
    window: &mut Window,
    sp: &MaskSpace,
    m: &Mapping,
    s: &MaskShape,
    canvas: Bounds<Pixels>,
) {
    let w = |p: (f64, f64)| to_window(sp, m, p);
    match s {
        MaskShape::Linear { from, to } => {
            let (a, b) = (sp.src(*from), sp.src(*to));
            let (dx, dy) = (b.0 - a.0, b.1 - a.1);
            let len = dx.hypot(dy).max(1e-9);
            // Lines across the whole canvas, perpendicular to the gradient direction.
            let span = (f32::from(canvas.size.width) + f32::from(canvas.size.height)) as f64
                * m.sf as f64
                / m.scale
                * 2.0;
            let (nx, ny) = (-dy / len * span, dx / len * span);
            let mid = ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0);
            for c in [a, mid, b] {
                outlined(window, w((c.0 - nx, c.1 - ny)), w((c.0 + nx, c.1 + ny)));
            }
            outlined(window, w(a), w(b));
        }
        MaskShape::Radial {
            center,
            radii,
            angle,
            ..
        } => {
            let c = sp.src(*center);
            let (rx, ry) = (radii[0] as f64 * sp.w, radii[1] as f64 * sp.h);
            let ang = (*angle as f64).to_radians();
            let pts: Vec<Point<Pixels>> = (0..=64)
                .map(|i| {
                    let t = i as f64 / 64.0 * std::f64::consts::TAU;
                    let (u, v) = (rx * t.cos(), ry * t.sin());
                    w((
                        c.0 + u * ang.cos() - v * ang.sin(),
                        c.1 + u * ang.sin() + v * ang.cos(),
                    ))
                })
                .collect();
            polyline(window, &pts);
        }
        _ => {}
    }
    for (part, h) in handles(sp, s) {
        let big = matches!(part, Part::LinearBoth | Part::RadialCenter);
        dot(window, w(h), if big { 6.0 } else { 4.5 }, !big);
    }
}

/// Brush outline at the cursor: inner (hard) and outer radius in logical px.
pub fn paint_brush_cursor(window: &mut Window, c: Point<Pixels>, outer: f32, inner: f32) {
    for r in [outer, inner] {
        let pts: Vec<Point<Pixels>> = (0..=48)
            .map(|i| {
                let t = i as f32 / 48.0 * std::f32::consts::TAU;
                c + point(px(r * t.cos()), px(r * t.sin()))
            })
            .collect();
        polyline(window, &pts);
    }
}

/// How the Source frame changes under a quarter turn or mirror of the display; masks follow
/// so they stay on their subject.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Reorient {
    /// Clockwise quarter turn of a frame that was `w × h` px.
    Cw {
        w: f64,
        h: f64,
    },
    Ccw {
        w: f64,
        h: f64,
    },
    Mirror,
}

fn reorient_shape(s: &mut MaskShape, r: Reorient) {
    let pt = |p: &mut Pt| {
        *p = match r {
            Reorient::Cw { .. } => Pt {
                x: 1.0 - p.y,
                y: p.x,
            },
            Reorient::Ccw { .. } => Pt {
                x: p.y,
                y: 1.0 - p.x,
            },
            Reorient::Mirror => Pt {
                x: 1.0 - p.x,
                y: p.y,
            },
        }
    };
    match s {
        MaskShape::Linear { from, to } => {
            pt(from);
            pt(to);
        }
        MaskShape::Radial {
            center,
            radii,
            angle,
            ..
        } => {
            pt(center);
            match r {
                Reorient::Cw { w, h } | Reorient::Ccw { w, h } => {
                    // The same px radii as fractions of the turned frame (h × w).
                    *radii = [
                        (radii[0] as f64 * w / h) as f32,
                        (radii[1] as f64 * h / w) as f32,
                    ];
                    *angle += if matches!(r, Reorient::Cw { .. }) {
                        90.0
                    } else {
                        -90.0
                    };
                }
                Reorient::Mirror => *angle = 180.0 - *angle,
            }
        }
        MaskShape::Brush { strokes } => {
            for st in strokes {
                st.points.iter_mut().for_each(pt);
            }
        }
        MaskShape::Combine { parts, .. } => {
            for p in parts {
                reorient_shape(p, r);
            }
        }
        MaskShape::Semantic { .. } | MaskShape::Range { .. } => {}
    }
}

/// Ops moving every mask of `p` with the reorientation `r`; `None` without masks.
pub fn reorient_ops(p: &EditParams, r: Reorient) -> Option<ParamOp> {
    if p.masks.is_empty() {
        return None;
    }
    let mut masks = p.masks.clone();
    for m in &mut masks {
        reorient_shape(&mut m.shape, r);
    }
    Some(ParamOp::set(
        "masks",
        serde_json::to_value(masks).expect("serialisable"),
    ))
}

/// Small dot at each mask's anchor so unselected masks can be picked.
pub fn anchor(sp: &MaskSpace, s: &MaskShape) -> Option<(f64, f64)> {
    match s {
        MaskShape::Linear { from, to } => {
            let (a, b) = (sp.src(*from), sp.src(*to));
            Some(((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0))
        }
        MaskShape::Radial { center, .. } => Some(sp.src(*center)),
        MaskShape::Brush { strokes } => strokes
            .first()
            .and_then(|s| s.points.first())
            .map(|p| sp.src(*p)),
        _ => None,
    }
}

pub fn anchor_window(sp: &MaskSpace, m: &Mapping, p: (f64, f64)) -> Point<Pixels> {
    to_window(sp, m, p)
}

pub fn paint_anchor(window: &mut Window, c: Point<Pixels>) {
    dot(window, c, 4.0, false);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_follow_a_quarter_turn() {
        let mut p = EditParams::default();
        let mut m = new_mask(&p, NewMask::Radial, Pt { x: 0.1, y: 0.2 });
        m.shape = MaskShape::Radial {
            center: Pt { x: 0.1, y: 0.2 },
            radii: [0.1, 0.15],
            angle: 0.0,
            feather: 50.0,
        };
        p.masks.push(m);
        let op = reorient_ops(&p, Reorient::Cw { w: 600.0, h: 400.0 }).unwrap();
        let ParamOp::Set { value, .. } = op else {
            panic!()
        };
        let masks: Vec<LocalAdjustment> = serde_json::from_value(value).unwrap();
        let MaskShape::Radial {
            center,
            radii,
            angle,
            ..
        } = masks[0].shape
        else {
            panic!()
        };
        // Pixel (60, 80) of 600×400 lands at (320, 60) of the turned 400×600 frame.
        assert!((center.x - 0.8).abs() < 1e-6 && (center.y - 0.1).abs() < 1e-6);
        assert!((radii[0] * 400.0 - 60.0).abs() < 1e-3 && (radii[1] * 600.0 - 60.0).abs() < 1e-3);
        assert_eq!(angle, 90.0);
        assert!(reorient_ops(&EditParams::default(), Reorient::Mirror).is_none());
    }

    #[test]
    fn radial_handles_follow_the_drag() {
        let p = EditParams::default();
        let sp = MaskSpace::new(Size::new(600, 400), &p);
        let c = Pt { x: 0.5, y: 0.5 };
        let s = create_shape(
            &sp,
            &new_mask(&p, NewMask::Radial, c).shape,
            c,
            Pt { x: 0.6, y: 0.5 },
        );
        let MaskShape::Radial { radii, .. } = s else {
            panic!()
        };
        // A circle of 60 px: equal in px, different as fractions of width and height.
        assert!((radii[0] * 600.0 - 60.0).abs() < 1e-3 && (radii[1] * 400.0 - 60.0).abs() < 1e-3);
        // The major handle sets the angle; straight down is 90°.
        let s = drag_shape(
            &sp,
            &s,
            Part::RadialMajor,
            Pt { x: 0.5, y: 0.75 },
            Pt::default(),
        );
        let MaskShape::Radial { radii, angle, .. } = s else {
            panic!()
        };
        assert!((angle - 90.0).abs() < 1e-3);
        assert!((radii[0] * 600.0 - 100.0).abs() < 1e-3);
        let hs = handles(&sp, &s);
        assert!((hs[1].1.0 - 300.0).abs() < 1e-3 && (hs[1].1.1 - 300.0).abs() < 1e-3);
    }

    #[test]
    fn new_masks_get_fresh_ids_and_points_round_trip() {
        let mut p = EditParams::default();
        let a = new_mask(&p, NewMask::Linear, Pt::default());
        p.masks.push(a.clone());
        let b = new_mask(&p, NewMask::Brush, Pt::default());
        assert_ne!(a.id, b.id);
        p.geometry.scalars.angle = 5.0;
        p.geometry.crop.x = 0.1;
        let sp = MaskSpace::new(Size::new(600, 400), &p);
        let n = Pt { x: 0.4, y: 0.6 };
        let back = sp.norm_of(sp.to_crop(n));
        assert!((back.x - n.x).abs() < 1e-6 && (back.y - n.y).abs() < 1e-6);
        let s = brush_add(
            &b.shape,
            n,
            Some(Stroke {
                points: Vec::new(),
                size: 0.02,
                feather: 50.0,
                flow: 100.0,
                erase: false,
            }),
        );
        let s = brush_add(&s, Pt { x: 0.5, y: 0.6 }, None);
        let MaskShape::Brush { strokes } = s else {
            panic!()
        };
        assert_eq!(strokes.len(), 1);
        assert_eq!(strokes[0].points.len(), 2);
    }
}
