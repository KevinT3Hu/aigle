//! Point-curve editor of the curves tab (doc 02 §6.4).
//!
//! Click on the graph to add a point on the curve, drag points to shape it, double-click a
//! point or drag it out of the graph to remove it. One drag is one gesture: `Continuous`
//! commands while dragging, `Final` on release. The curve is drawn with the renderer's own
//! spline, over the histogram of the rendered image.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use aigle_core::params::tone::{Spline, identity_points, points_are_identity};
use aigle_core::params::{CurvePoint, PointCurves};
use aigle_doc::{EditCommand, GestureId, ParamOp};
use aigle_engine_api::{Histogram, Interaction};
use gpui::{
    Bounds, Context, Entity, Hsla, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, ParentElement, PathBuilder, Pixels, Point, Render, Rgba, SharedString, Styled,
    Window, canvas, div, fill, point, prelude::*, px, rgb, rgba, size,
};
use gpui_component::button::Button;
use gpui_component::{ActiveTheme, Selectable, Sizable, h_flex, v_flex};

use crate::photo::Photo;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Channel {
    Rgb,
    Red,
    Green,
    Blue,
}

impl Channel {
    const ALL: [Channel; 4] = [Channel::Rgb, Channel::Red, Channel::Green, Channel::Blue];

    fn path(self) -> &'static str {
        match self {
            Channel::Rgb => "curve.point.rgb",
            Channel::Red => "curve.point.red",
            Channel::Green => "curve.point.green",
            Channel::Blue => "curve.point.blue",
        }
    }

    /// Catalog key of the channel name, or the name itself.
    fn label(self) -> &'static str {
        match self {
            Channel::Rgb => "RGB",
            Channel::Red => "curve_editor.channel.red",
            Channel::Green => "curve_editor.channel.green",
            Channel::Blue => "curve_editor.channel.blue",
        }
    }

    fn points(self, c: &PointCurves) -> &Vec<CurvePoint> {
        match self {
            Channel::Rgb => &c.rgb,
            Channel::Red => &c.red,
            Channel::Green => &c.green,
            Channel::Blue => &c.blue,
        }
    }

    fn color(self) -> Rgba {
        match self {
            Channel::Rgb => rgb(0xe0e0e0),
            Channel::Red => rgb(0xff5050),
            Channel::Green => rgb(0x50e050),
            Channel::Blue => rgb(0x5090ff),
        }
    }

    fn bins(self, h: &Histogram) -> &[u32] {
        match self {
            Channel::Rgb => &h.luma,
            Channel::Red => &h.red,
            Channel::Green => &h.green,
            Channel::Blue => &h.blue,
        }
    }
}

/// Smallest x distance between two points.
const MIN_GAP: f32 = 0.01;
/// Pointer distance that grabs a point, logical px.
const HIT: f32 = 8.0;
/// Dragging an inner point this far outside the graph removes it, logical px.
const REMOVE: f32 = 24.0;
const GRAPH: f32 = 292.0;

/// Points the editor works on: the stored curve, or the identity when it is empty.
fn editable(stored: &[CurvePoint]) -> Vec<CurvePoint> {
    if stored.len() < 2 {
        identity_points()
    } else {
        stored.to_vec()
    }
}

/// Graph coordinates (unclamped) of a window position.
fn to_unit(b: Bounds<Pixels>, p: Point<Pixels>) -> (f32, f32) {
    (
        f32::from(p.x - b.origin.x) / f32::from(b.size.width),
        1.0 - f32::from(p.y - b.origin.y) / f32::from(b.size.height),
    )
}

fn to_px(b: Bounds<Pixels>, x: f32, y: f32) -> Point<Pixels> {
    point(
        b.origin.x + b.size.width * x,
        b.origin.y + b.size.height * (1.0 - y),
    )
}

/// The point within `HIT` of `p`, nearest first.
fn hit(points: &[CurvePoint], b: Bounds<Pixels>, p: Point<Pixels>) -> Option<usize> {
    points
        .iter()
        .enumerate()
        .map(|(i, q)| {
            let c = to_px(b, q.x, q.y);
            let (dx, dy) = (f32::from(c.x - p.x), f32::from(c.y - p.y));
            (i, dx * dx + dy * dy)
        })
        .filter(|(_, d)| *d <= HIT * HIT)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i)
}

/// Insert a point at `x` on the curve; `None` if `x` is too close to an existing point.
fn insert(points: &mut Vec<CurvePoint>, x: f32) -> Option<usize> {
    let x = x.clamp(0.0, 1.0);
    if points.iter().any(|q| (q.x - x).abs() < MIN_GAP) {
        return None;
    }
    let y = Spline::new(points).map_or(x, |s| s.eval(x as f64) as f32);
    let i = points.partition_point(|q| q.x < x);
    points.insert(
        i,
        CurvePoint {
            x,
            y: y.clamp(0.0, 1.0),
        },
    );
    Some(i)
}

/// Move point `i` to (`x`, `y`), keeping the points ordered and `MIN_GAP` apart.
fn move_point(points: &mut [CurvePoint], i: usize, x: f32, y: f32) {
    let lo = if i == 0 {
        0.0
    } else {
        points[i - 1].x + MIN_GAP
    };
    let hi = if i + 1 == points.len() {
        1.0
    } else {
        points[i + 1].x - MIN_GAP
    };
    points[i] = CurvePoint {
        x: x.clamp(lo, hi.max(lo)),
        y: y.clamp(0.0, 1.0),
    };
}

fn is_inner(points: &[CurvePoint], i: usize) -> bool {
    i > 0 && i + 1 < points.len()
}

/// How far outside `b` the position is, logical px.
fn outside(b: Bounds<Pixels>, p: Point<Pixels>) -> f32 {
    let dx = (f32::from(b.origin.x - p.x)).max(f32::from(p.x - b.right()));
    let dy = (f32::from(b.origin.y - p.y)).max(f32::from(p.y - b.bottom()));
    dx.max(dy).max(0.0)
}

struct Drag {
    index: usize,
    points: Vec<CurvePoint>,
    /// Point minus pointer at grab time, in graph units.
    grab: (f32, f32),
    removed: bool,
    gesture: GestureId,
}

impl Drag {
    fn committed(&self) -> Vec<CurvePoint> {
        let mut p = self.points.clone();
        if self.removed {
            p.remove(self.index);
        }
        p
    }
}

pub struct CurveEditor {
    photo: Entity<Photo>,
    channel: Channel,
    pub histogram: Option<Arc<Histogram>>,
    bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    drag: Option<Drag>,
    hover: Option<usize>,
    error: Option<String>,
}

impl CurveEditor {
    pub fn new(photo: Entity<Photo>, cx: &mut Context<Self>) -> CurveEditor {
        cx.observe(&photo, |_, _, cx| cx.notify()).detach();
        CurveEditor {
            photo,
            channel: Channel::Rgb,
            histogram: None,
            bounds: Rc::new(Cell::new(None)),
            drag: None,
            hover: None,
            error: None,
        }
    }

    fn stored(&self, cx: &Context<Self>) -> Vec<CurvePoint> {
        self.channel
            .points(&self.photo.read(cx).head().curve.point)
            .clone()
    }

    /// The points shown: the drag's while dragging, else the head's.
    fn shown(&self, cx: &Context<Self>) -> Vec<CurvePoint> {
        match &self.drag {
            Some(d) => d.committed(),
            None => editable(&self.stored(cx)),
        }
    }

    fn label(&self) -> String {
        crate::i18n::join(&["section.point_curve", self.channel.label()])
    }

    fn send(
        &mut self,
        points: &[CurvePoint],
        gesture: Option<GestureId>,
        hint: Interaction,
        cx: &mut Context<Self>,
    ) {
        let path = self.channel.path();
        let op = if points_are_identity(points) {
            ParamOp::Reset { path: path.into() }
        } else {
            ParamOp::set(path, serde_json::to_value(points).expect("serialisable"))
        };
        let label = self.label();
        let r = self.photo.update(cx, |p, cx| {
            let mut cmd = EditCommand::user(p.doc.rev(), label, vec![op]);
            cmd.gesture = gesture;
            p.edit(cmd, hint, cx)
        });
        self.error = r.err().map(|e| e.to_string());
    }

    fn reset(&mut self, cx: &mut Context<Self>) {
        self.drag = None;
        self.send(&[], None, Interaction::Final, cx);
        cx.notify();
    }

    fn on_down(&mut self, e: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(b) = self.bounds.get() else { return };
        let mut points = editable(&self.stored(cx));
        let index = match hit(&points, b, e.position) {
            Some(i) if e.click_count >= 2 => {
                if is_inner(&points, i) {
                    points.remove(i);
                    self.hover = None;
                    self.send(&points, None, Interaction::Final, cx);
                }
                return;
            }
            Some(i) => i,
            None if outside(b, e.position) == 0.0 && e.click_count < 2 => {
                match insert(&mut points, to_unit(b, e.position).0) {
                    Some(i) => i,
                    None => return,
                }
            }
            None => return,
        };
        let (ux, uy) = to_unit(b, e.position);
        let q = points[index];
        self.drag = Some(Drag {
            index,
            points,
            grab: (q.x - ux, q.y - uy),
            removed: false,
            gesture: self.photo.update(cx, |p, _| p.new_gesture()),
        });
        cx.notify();
    }

    fn on_move(&mut self, e: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(b) = self.bounds.get() else { return };
        if self.drag.is_some() && e.pressed_button != Some(MouseButton::Left) {
            self.finish(cx);
            return;
        }
        let Some(d) = &mut self.drag else {
            let h = hit(&editable(&self.stored(cx)), b, e.position);
            if h != self.hover {
                self.hover = h;
                cx.notify();
            }
            return;
        };
        d.removed = is_inner(&d.points, d.index) && outside(b, e.position) > REMOVE;
        let (ux, uy) = to_unit(b, e.position);
        let (x, y) = (ux + d.grab.0, uy + d.grab.1);
        move_point(&mut d.points, d.index, x, y);
        let (points, g) = (d.committed(), d.gesture);
        self.send(&points, Some(g), Interaction::Continuous, cx);
        cx.notify();
    }

    fn finish(&mut self, cx: &mut Context<Self>) {
        let Some(d) = self.drag.take() else { return };
        self.send(&d.committed(), Some(d.gesture), Interaction::Final, cx);
        cx.notify();
    }

    fn readout(&self, points: &[CurvePoint]) -> Option<String> {
        let i = self.drag.as_ref().map(|d| d.index).or(self.hover)?;
        if self.drag.as_ref().is_some_and(|d| d.removed) {
            return Some(crate::i18n::t("curve_editor.release_to_delete"));
        }
        let q = points.get(i)?;
        let v = |t: f32| (t * 255.0).round() as i32;
        Some(rust_i18n::t!("curve_editor.readout", arg0 = v(q.x), arg1 = v(q.y)).into_owned())
    }
}

fn stroke(window: &mut Window, pts: impl IntoIterator<Item = Point<Pixels>>, w: f32, c: Hsla) {
    let mut b = PathBuilder::stroke(px(w));
    for (i, p) in pts.into_iter().enumerate() {
        if i == 0 {
            b.move_to(p);
        } else {
            b.line_to(p);
        }
    }
    if let Ok(path) = b.build() {
        window.paint_path(path, c);
    }
}

struct Scene {
    channel: Channel,
    points: Vec<CurvePoint>,
    /// Parametric ∘ RGB curve when the parametric curve is active (RGB channel only).
    composite: Option<Vec<f64>>,
    histogram: Option<Arc<Histogram>>,
    active: Option<usize>,
}

fn paint(b: Bounds<Pixels>, s: &Scene, window: &mut Window) {
    window.paint_quad(fill(b, rgb(0x141414)));
    if let Some(h) = &s.histogram {
        let bins = s.channel.bins(h);
        let peak = bins.iter().skip(1).take(bins.len().saturating_sub(2)).max();
        let peak = peak.copied().unwrap_or(1).max(1) as f32;
        let n = bins.len().max(2);
        let mut p = PathBuilder::fill();
        p.move_to(to_px(b, 0.0, 0.0));
        for (i, &c) in bins.iter().enumerate() {
            let y = (c as f32 / peak).min(1.0).sqrt() * 0.85;
            p.line_to(to_px(b, i as f32 / (n - 1) as f32, y));
        }
        p.line_to(to_px(b, 1.0, 0.0));
        p.close();
        if let Ok(path) = p.build() {
            window.paint_path(path, rgba(0xffffff1c));
        }
    }
    let grid: Hsla = rgba(0xffffff18).into();
    for k in 1..4 {
        let t = k as f32 / 4.0;
        stroke(window, [to_px(b, t, 0.0), to_px(b, t, 1.0)], 1.0, grid);
        stroke(window, [to_px(b, 0.0, t), to_px(b, 1.0, t)], 1.0, grid);
    }
    stroke(
        window,
        [to_px(b, 0.0, 0.0), to_px(b, 1.0, 1.0)],
        1.0,
        rgba(0xffffff30).into(),
    );
    const N: usize = 160;
    let xs = (0..=N).map(|i| i as f32 / N as f32);
    if let Some(c) = &s.composite {
        let n = c.len().max(2);
        let line = c
            .iter()
            .enumerate()
            .map(|(i, &y)| to_px(b, i as f32 / (n - 1) as f32, y as f32));
        stroke(window, line, 1.0, rgba(0xffffff60).into());
    }
    let spline = Spline::new(&s.points);
    let line = xs.map(|x| {
        let y = spline.as_ref().map_or(x, |sp| sp.eval(x as f64) as f32);
        to_px(b, x, y.clamp(0.0, 1.0))
    });
    stroke(window, line, 1.5, s.channel.color().into());
    for (i, q) in s.points.iter().enumerate() {
        let c = to_px(b, q.x, q.y);
        let r = if s.active == Some(i) {
            px(5.0)
        } else {
            px(4.0)
        };
        window.paint_quad(
            fill(
                Bounds::new(c - point(r, r), size(r * 2.0, r * 2.0)),
                s.channel.color(),
            )
            .corner_radii(r)
            .border_widths(px(1.0))
            .border_color(rgb(0x101010)),
        );
    }
}

impl Render for CurveEditor {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let points = self.shown(cx);
        let curve = &self.photo.read(cx).head().curve;
        let composite = (self.channel == Channel::Rgb
            && !aigle_core::params::tone::parametric_is_identity(&curve.parametric))
        .then(|| curve.master_table(256));
        let readout = self.readout(&points);
        let scene = Scene {
            channel: self.channel,
            points,
            composite,
            histogram: self.histogram.clone(),
            active: match &self.drag {
                Some(d) => (!d.removed).then_some(d.index),
                None => self.hover,
            },
        };
        let slot = self.bounds.clone();
        let muted = cx.theme().muted_foreground;

        let channels = h_flex()
            .gap_1()
            .children(Channel::ALL.map(|ch| {
                Button::new(SharedString::from(format!("curve-channel-{ch:?}")))
                    .label(crate::i18n::t(ch.label()))
                    .xsmall()
                    .selected(ch == self.channel)
                    .on_click(cx.listener(move |c, _, _, cx| {
                        c.channel = ch;
                        c.hover = None;
                        cx.notify();
                    }))
            }))
            .child(div().flex_1())
            .child(
                Button::new("curve-reset")
                    .label(crate::i18n::t("common.reset"))
                    .xsmall()
                    .on_click(cx.listener(|c, _, _, cx| c.reset(cx))),
            );

        let graph = div()
            .id("curve-graph")
            .w(px(GRAPH))
            .h(px(GRAPH))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_down))
            .on_mouse_move(cx.listener(Self::on_move))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|c, _: &MouseUpEvent, _, cx| c.finish(cx)),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|c, _: &MouseUpEvent, _, cx| c.finish(cx)),
            )
            .on_hover(cx.listener(|c, hovered: &bool, _, cx| {
                if !*hovered && c.hover.take().is_some() {
                    cx.notify();
                }
            }))
            .child(
                canvas(
                    move |b, _, _| slot.set(Some(b)),
                    move |b, _, window, _| paint(b, &scene, window),
                )
                .size_full(),
            );

        let hint = readout.unwrap_or_else(|| crate::i18n::t("curve_editor.help"));
        let mut body = v_flex()
            .gap_2()
            .child(channels)
            .child(graph)
            .child(div().text_xs().text_color(muted).child(hint));
        if let Some(e) = &self.error {
            body = body.child(
                div()
                    .text_xs()
                    .text_color(cx.theme().danger)
                    .child(e.clone()),
            );
        }
        body
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::size as gsize;

    fn b() -> Bounds<Pixels> {
        Bounds::new(point(px(10.0), px(20.0)), gsize(px(100.0), px(100.0)))
    }

    #[test]
    fn insert_move_and_hit() {
        let mut p = editable(&[]);
        let i = insert(&mut p, 0.5).unwrap();
        assert_eq!((i, p.len()), (1, 3));
        assert!(
            (p[1].y - 0.5).abs() < 1e-6,
            "inserted on the identity curve"
        );
        assert!(insert(&mut p, 0.505).is_none(), "too close to a point");

        // Points cannot cross their neighbours.
        move_point(&mut p, 1, 1.5, 0.8);
        assert_eq!(p[1].x, 1.0 - MIN_GAP);
        assert_eq!(p[1].y, 0.8);
        move_point(&mut p, 0, 0.2, -1.0);
        assert_eq!((p[0].x, p[0].y), (0.2, 0.0));

        let at = to_px(b(), p[1].x, p[1].y);
        assert_eq!(hit(&p, b(), at + point(px(3.0), px(3.0))), Some(1));
        assert_eq!(hit(&p, b(), point(px(60.0), px(120.0))), None);
        let (x, y) = to_unit(b(), at);
        assert!((x - p[1].x).abs() < 1e-5 && (y - p[1].y).abs() < 1e-5);

        assert!(is_inner(&p, 1) && !is_inner(&p, 0) && !is_inner(&p, 2));
        assert_eq!(outside(b(), point(px(50.0), px(50.0))), 0.0);
        assert_eq!(outside(b(), point(px(140.0), px(50.0))), 30.0);
    }
}
