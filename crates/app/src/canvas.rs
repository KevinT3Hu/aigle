//! Develop canvas: one engine `View` shown in GPUI (doc 01 §6).
//!
//! Frames arrive on the view's latest-wins channel and are copied once into a GPUI
//! `RenderImage`. While a new render is in flight the previous frame is reprojected to the
//! current zoom/pan, so pan and zoom respond immediately. The split comparison adds a second
//! view on `ParamsSource::Original` (the unedited photo framed like the edit) with the same
//! viewport.
//!
//! The crop tool shows the whole straightened frame (`View::set_uncropped`) with the crop
//! rectangle on top; the mask tool draws the selected shape's handles. Their drags commit
//! through `doc` like sliders: `Continuous` while dragging, `Final` on release, one gesture.

use std::sync::Arc;

use aigle_core::geometry::{Point as ImgPoint, Rect, Size};
use aigle_core::masks::{MaskShape, Pt, Stroke};
use aigle_doc::{GestureId, ParamOp};
use aigle_engine_api::{
    DisplayProfileId, Frame, Histogram, Interaction, Overlays, ParamsSource, PixelBuffer, Quality,
    View, ViewSpec, Viewport, watch,
};
use gpui::{
    App, Bounds, Context, Corners, CursorStyle, Entity, IntoElement, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, ParentElement, PathBuilder, Pixels, Point, Render, RenderImage,
    ScrollWheelEvent, Styled, Task, Window, canvas, div, fill, point, prelude::*, px, rgb, size,
};
use gpui_component::v_flex;
use image::{Frame as ImageFrame, RgbaImage};

use crate::crop::{self, Handle};
use crate::meta;
use crate::photo::{Photo, PhotoEvent, Tool, crop_ops};
use crate::tools::{self, Mapping, MaskSpace, Part};
use crate::viewport::{ViewState, reproject};

type Shown = (Arc<Frame>, Arc<RenderImage>);

/// How the original is compared with the edit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Compare {
    Off,
    /// The whole canvas shows the original.
    Before,
    /// Original left of the divider, edit right of it.
    Split,
}

struct CropDrag {
    /// `None`: rotate (the drag started outside the crop).
    handle: Option<Handle>,
    start: Rect,
    from: ImgPoint,
    start_angle: f32,
    /// Crop centre and the drag start in window px, for rotating.
    pivot: Point<Pixels>,
    from_window: Point<Pixels>,
    gesture: GestureId,
    last: Option<(String, Vec<ParamOp>)>,
}

enum MaskMode {
    Part(Part),
    Create,
    Brush,
}

struct MaskDrag {
    index: usize,
    mode: MaskMode,
    start: MaskShape,
    from: Pt,
    /// Last brush point in Source px.
    last_src: (f64, f64),
    gesture: GestureId,
    label: String,
    last: Option<Vec<ParamOp>>,
}

pub struct Canvas {
    photo: Entity<Photo>,
    view: View,
    state: ViewState,
    frame: Option<Shown>,
    /// Canvas bounds (logical px) and the window scale factor at the last layout.
    layout: Option<(Bounds<Pixels>, f32)>,
    drag: Option<Point<Pixels>>,
    /// Show the camera-parameter overlay.
    show_info: bool,
    /// Pointer position over the canvas (picker ring, brush outline, cursors).
    hover: Option<Point<Pixels>>,
    compare: Compare,
    /// Second view on `ParamsSource::Original` while splitting.
    before: Option<(View, Task<anyhow::Result<()>>)>,
    before_frame: Option<Shown>,
    /// Divider position as a fraction of the canvas width.
    split_at: f32,
    split_drag: bool,
    near_divider: bool,
    sent: Option<Viewport>,
    /// The tool and overlays the view was last set up for.
    tool: Tool,
    overlays: Overlays,
    crop_drag: Option<CropDrag>,
    mask_drag: Option<MaskDrag>,
}

/// Copy the frames of `rx` into the slot `slot` selects, latest wins.
fn listen(
    mut rx: watch::Receiver<Option<Arc<Frame>>>,
    slot: fn(&mut Canvas) -> &mut Option<Shown>,
    cx: &mut Context<Canvas>,
) -> Task<anyhow::Result<()>> {
    cx.spawn(async move |this, cx| {
        while rx.changed().await.is_ok() {
            let Some(f) = rx.borrow_and_update().clone() else {
                continue;
            };
            let Some(img) = render_image(&f.pixels) else {
                continue;
            };
            this.update(cx, |c, cx| {
                if let Some((_, old)) = slot(c).replace((f, img)) {
                    cx.drop_image(old, None);
                }
                cx.notify();
            })?;
        }
        Ok(())
    })
}

/// Grab distance of the split divider, logical px.
const DIVIDER_GRAB: f32 = 6.0;

impl Canvas {
    pub fn new(photo: Entity<Photo>, cx: &mut Context<Self>) -> Canvas {
        let view = photo.read(cx).session.create_view(ViewSpec::fit(1, 1));
        listen(view.frames(), |c| &mut c.frame, cx).detach();
        cx.subscribe(&photo, |c, _, ev, cx| match ev {
            PhotoEvent::Loaded => {
                c.update_viewport(Interaction::Final, true, cx);
                cx.notify();
            }
            PhotoEvent::Edited => {
                c.update_viewport(Interaction::Final, false, cx);
                cx.notify();
            }
            PhotoEvent::ToolChanged => c.sync_tool(cx),
            PhotoEvent::Notice(_) => {}
        })
        .detach();
        cx.observe(&photo, |c, _, cx| {
            c.sync_overlays(cx);
            cx.notify()
        })
        .detach();
        Canvas {
            photo,
            view,
            state: ViewState::default(),
            frame: None,
            layout: None,
            drag: None,
            show_info: true,
            hover: None,
            compare: Compare::Off,
            before: None,
            before_frame: None,
            split_at: 0.5,
            split_drag: false,
            near_divider: false,
            sent: None,
            tool: Tool::None,
            overlays: Overlays::default(),
            crop_drag: None,
            mask_drag: None,
        }
    }

    pub fn histogram(&self) -> watch::Receiver<Option<Arc<Histogram>>> {
        self.view.histogram()
    }

    /// Enter or leave the crop view when the photo's tool changed.
    fn sync_tool(&mut self, cx: &mut Context<Self>) {
        let tool = self.photo.read(cx).tool;
        if tool == self.tool {
            return;
        }
        let crop_changed = (tool == Tool::Crop) != (self.tool == Tool::Crop);
        self.tool = tool;
        self.crop_drag = None;
        self.mask_drag = None;
        if crop_changed {
            if tool == Tool::Crop {
                self.set_compare(Compare::Off, cx);
            }
            self.view.set_uncropped(tool == Tool::Crop);
            self.state = ViewState::default();
            if let Some((_, old)) = self.frame.take() {
                cx.drop_image(old, None);
            }
            self.update_viewport(Interaction::Final, true, cx);
        }
        self.sync_overlays(cx);
        cx.notify();
    }

    fn sync_overlays(&mut self, cx: &App) {
        let p = self.photo.read(cx);
        let o = Overlays {
            mask_overlay: p
                .selected()
                .filter(|_| p.tool == Tool::Mask && p.mask_overlay)
                .map(|(_, m)| m.id),
            ..self.overlays.clone()
        };
        if o != self.overlays {
            self.view.set_overlays(o.clone());
            self.overlays = o;
        }
    }

    /// Extent of what the view shows: the crop, or the straightened frame in the crop tool.
    fn extent(&self, cx: &App) -> Option<(f64, f64)> {
        let p = self.photo.read(cx);
        if self.tool == Tool::Crop {
            let f = p.frame()?;
            Some((f.w, f.h))
        } else {
            p.crop_extent()
        }
    }

    /// Extent, canvas size in physical px and the window scale factor.
    fn geometry(&self, cx: &App) -> Option<((f64, f64), Size, f32)> {
        let crop = self.extent(cx)?;
        let (b, sf) = self.layout?;
        let out = Size::new(
            ((f32::from(b.size.width) * sf).round() as u32).max(1),
            ((f32::from(b.size.height) * sf).round() as u32).max(1),
        );
        Some((crop, out, sf))
    }

    fn mapping(&self, cx: &App) -> Option<Mapping> {
        let (crop, out, sf) = self.geometry(cx)?;
        let (b, _) = self.layout?;
        Some(Mapping {
            origin: b.origin,
            region: self.state.region(crop, out),
            scale: self.state.scale(crop, out),
            sf,
        })
    }

    /// Canvas pixel (physical) of a window position.
    fn local(&self, p: Point<Pixels>) -> ImgPoint {
        let Some((b, sf)) = self.layout else {
            return ImgPoint::new(0.0, 0.0);
        };
        ImgPoint::new(
            (f32::from(p.x - b.origin.x) * sf) as f64,
            (f32::from(p.y - b.origin.y) * sf) as f64,
        )
    }

    /// Send the viewport if it changed; `force` re-sends it (load, tool switch).
    fn update_viewport(&mut self, hint: Interaction, force: bool, cx: &App) {
        let Some((crop, out, _)) = self.geometry(cx) else {
            return;
        };
        let vp = Viewport {
            region: self.state.region(crop, out),
            output_size: (out.w, out.h),
            display: DisplayProfileId::Srgb,
        };
        if self.sent.as_ref() != Some(&vp) || force {
            self.view.set_viewport(vp.clone(), hint);
            if let Some((v, _)) = &self.before {
                v.set_viewport(vp.clone(), hint);
            }
            self.sent = Some(vp);
        }
    }

    fn set_layout(&mut self, bounds: Bounds<Pixels>, sf: f32, cx: &mut Context<Self>) {
        if self.layout == Some((bounds, sf)) {
            return;
        }
        self.layout = Some((bounds, sf));
        self.update_viewport(Interaction::Final, false, cx);
        cx.notify();
    }

    pub fn toggle_zoom(&mut self, cx: &mut Context<Self>) {
        if self.tool == Tool::Crop {
            return;
        }
        let Some((crop, out, _)) = self.geometry(cx) else {
            return;
        };
        let at = ImgPoint::new(out.w as f64 / 2.0, out.h as f64 / 2.0);
        self.state.toggle_one_to_one(at, crop, out);
        self.update_viewport(Interaction::Final, false, cx);
        cx.notify();
    }

    pub fn toggle_info(&mut self, cx: &mut Context<Self>) {
        self.show_info = !self.show_info;
        cx.notify();
    }

    pub fn show_info(&self) -> bool {
        self.show_info
    }

    pub fn compare(&self) -> Compare {
        self.compare
    }

    pub fn toggle_before(&mut self, cx: &mut Context<Self>) {
        let next = if self.compare == Compare::Before {
            Compare::Off
        } else {
            Compare::Before
        };
        self.set_compare(next, cx);
    }

    pub fn toggle_split(&mut self, cx: &mut Context<Self>) {
        let next = if self.compare == Compare::Split {
            Compare::Off
        } else {
            Compare::Split
        };
        self.set_compare(next, cx);
    }

    fn set_compare(&mut self, mode: Compare, cx: &mut Context<Self>) {
        let mode = if self.tool == Tool::Crop {
            Compare::Off
        } else {
            mode
        };
        if self.compare == mode {
            return;
        }
        self.compare = mode;
        self.view.set_params_source(if mode == Compare::Before {
            ParamsSource::Original
        } else {
            ParamsSource::Current
        });
        if mode == Compare::Split {
            let v = self.photo.read(cx).session.create_view(ViewSpec::fit(1, 1));
            v.set_params_source(ParamsSource::Original);
            if let Some(vp) = &self.sent {
                v.set_viewport(vp.clone(), Interaction::Final);
            }
            let task = listen(v.frames(), |c| &mut c.before_frame, cx);
            self.before = Some((v, task));
        } else {
            self.before = None;
            self.split_drag = false;
            if let Some((_, old)) = self.before_frame.take() {
                cx.drop_image(old, None);
            }
        }
        cx.notify();
    }

    /// Divider x in window coordinates while splitting.
    fn divider_x(&self) -> Option<Pixels> {
        let (b, _) = self.layout?;
        (self.compare == Compare::Split).then(|| b.origin.x + b.size.width * self.split_at)
    }

    fn near_divider(&self, p: Point<Pixels>) -> bool {
        self.divider_x()
            .is_some_and(|x| (f32::from(p.x - x)).abs() <= DIVIDER_GRAB)
    }

    /// Crop-space point under a window position and the picker radius in Crop px.
    fn image_point(&self, p: Point<Pixels>, cx: &App) -> Option<(ImgPoint, f32)> {
        let m = self.mapping(cx)?;
        Some((
            m.to_image(p),
            ((PICK_RADIUS * m.sf) as f64 / m.scale).max(0.5) as f32,
        ))
    }

    fn mask_space(&self, cx: &App) -> Option<MaskSpace> {
        let p = self.photo.read(cx);
        let head = p.head();
        Some(MaskSpace::new(p.oriented_size(&head.geometry)?, head))
    }

    // -----------------------------------------------------------------------------------------
    // Crop tool

    fn crop_rect(&self, cx: &App) -> Option<(crop::Frame, Rect)> {
        let p = self.photo.read(cx);
        let f = p.frame()?;
        Some((f, f.rect_of(&p.head().geometry.crop)))
    }

    fn start_crop(&mut self, e: &MouseDownEvent, cx: &mut Context<Self>) {
        let (Some(m), Some((_, r))) = (self.mapping(cx), self.crop_rect(cx)) else {
            return;
        };
        let handle = tools::crop_hit(&m, r, e.position);
        if e.click_count >= 2 && handle == Some(Handle::Move) {
            self.photo.update(cx, |p, cx| p.set_tool(Tool::None, cx));
            return;
        }
        let gesture = self.photo.update(cx, |p, _| p.new_gesture());
        self.crop_drag = Some(CropDrag {
            handle,
            start: r,
            from: m.to_image(e.position),
            start_angle: self.photo.read(cx).head().geometry.scalars.angle,
            pivot: m.to_window(ImgPoint::new(r.x + r.w / 2.0, r.y + r.h / 2.0)),
            from_window: e.position,
            gesture,
            last: None,
        });
    }

    fn move_crop(&mut self, p: Point<Pixels>, cx: &mut Context<Self>) {
        let (Some(m), Some(d)) = (self.mapping(cx), self.crop_drag.as_ref()) else {
            return;
        };
        let photo = self.photo.read(cx);
        let Some(f) = photo.frame() else { return };
        let (label, ops) = match d.handle {
            Some(h) => {
                let q = m.to_image(p);
                let k = crop::ratio(photo.head().geometry.aspect, f.w, f.h);
                let r = crop::drag(&f, d.start, h, q.x - d.from.x, q.y - d.from.y, k);
                (crate::i18n::t("裁剪"), crop_ops(&f.crop_of(r)))
            }
            None => {
                let a0 = f32::from(d.from_window.y - d.pivot.y)
                    .atan2(f32::from(d.from_window.x - d.pivot.x));
                let a1 = f32::from(p.y - d.pivot.y).atan2(f32::from(p.x - d.pivot.x));
                let mut delta = (a1 - a0).to_degrees();
                if delta > 180.0 {
                    delta -= 360.0;
                } else if delta < -180.0 {
                    delta += 360.0;
                }
                let angle = (d.start_angle + delta).clamp(-45.0, 45.0);
                (
                    crate::i18n::t("拉直"),
                    vec![ParamOp::set("geometry.angle", angle)],
                )
            }
        };
        let gesture = d.gesture;
        self.photo.update(cx, |ph, cx| {
            ph.command(
                &label,
                ops.clone(),
                Some(gesture),
                Interaction::Continuous,
                cx,
            )
        });
        if let Some(d) = &mut self.crop_drag {
            d.last = Some((label, ops));
        }
    }

    fn end_crop(&mut self, cx: &mut Context<Self>) {
        if let Some(CropDrag {
            gesture,
            last: Some((label, ops)),
            ..
        }) = self.crop_drag.take()
        {
            self.photo.update(cx, |p, cx| {
                p.command(label, ops, Some(gesture), Interaction::Final, cx)
            });
        }
    }

    // -----------------------------------------------------------------------------------------
    // Mask tool

    /// Start a mask drag at `e`; `false` lets the click pan instead.
    fn start_mask(&mut self, e: &MouseDownEvent, cx: &mut Context<Self>) -> bool {
        let (Some(m), Some(sp)) = (self.mapping(cx), self.mask_space(cx)) else {
            return false;
        };
        let at = sp.norm_of(m.to_image(e.position));
        let (pending, brush) = {
            let p = self.photo.read(cx);
            (p.pending_mask, p.brush)
        };
        let stroke = || Stroke {
            points: Vec::new(),
            size: brush.size,
            feather: brush.feather,
            flow: brush.flow,
            erase: brush.erase,
        };
        let gesture = self.photo.update(cx, |p, _| p.new_gesture());
        let src = sp.src(at);

        if let Some(kind) = pending {
            let mask = tools::new_mask(self.photo.read(cx).head(), kind, at);
            let id = mask.id;
            let index = self.photo.read(cx).head().masks.len();
            let mut shape = mask.shape.clone();
            if kind == tools::NewMask::Brush {
                shape = tools::brush_add(&shape, at, Some(stroke()));
            }
            let mut first = mask.clone();
            first.shape = shape.clone();
            let ok = self.photo.update(cx, |p, cx| {
                p.pending_mask = None;
                let ok = p.command(
                    crate::i18n::t("添加蒙版"),
                    vec![ParamOp::AddMask { mask: first }],
                    Some(gesture),
                    Interaction::Continuous,
                    cx,
                );
                if ok {
                    p.select_mask(Some(id), cx);
                }
                ok
            });
            if !ok {
                return true;
            }
            self.mask_drag = Some(MaskDrag {
                index,
                mode: if kind == tools::NewMask::Brush {
                    MaskMode::Brush
                } else {
                    MaskMode::Create
                },
                start: shape,
                from: at,
                last_src: src,
                gesture,
                label: crate::i18n::t("添加蒙版"),
                last: None,
            });
            return true;
        }

        let photo = self.photo.read(cx);
        let selected = photo.selected().map(|(i, a)| (i, a.shape.clone()));
        if let Some((index, shape)) = &selected
            && let Some(part) = tools::mask_hit(&sp, &m, shape, e.position)
        {
            self.mask_drag = Some(MaskDrag {
                index: *index,
                mode: MaskMode::Part(part),
                start: shape.clone(),
                from: at,
                last_src: src,
                gesture,
                label: crate::i18n::t("调整蒙版"),
                last: None,
            });
            return true;
        }
        let picked = photo.head().masks.iter().find(|a| {
            Some(a.id) != photo.selected_mask
                && tools::anchor(&sp, &a.shape).is_some_and(|c| {
                    let w = tools::anchor_window(&sp, &m, c);
                    f32::from(w.x - e.position.x).hypot(f32::from(w.y - e.position.y))
                        <= tools::GRAB
                })
        });
        if let Some(a) = picked {
            let id = a.id;
            self.photo.update(cx, |p, cx| p.select_mask(Some(id), cx));
            return true;
        }
        if let Some((index, shape @ MaskShape::Brush { .. })) = selected {
            let shape = tools::brush_add(&shape, at, Some(stroke()));
            let ops = tools::shape_ops(index, &shape);
            self.photo.update(cx, |p, cx| {
                p.command(
                    crate::i18n::t("画笔"),
                    ops.clone(),
                    Some(gesture),
                    Interaction::Continuous,
                    cx,
                )
            });
            self.mask_drag = Some(MaskDrag {
                index,
                mode: MaskMode::Brush,
                start: shape,
                from: at,
                last_src: src,
                gesture,
                label: crate::i18n::t("画笔"),
                last: Some(ops),
            });
            return true;
        }
        false
    }

    fn move_mask(&mut self, p: Point<Pixels>, cx: &mut Context<Self>) {
        let (Some(m), Some(sp)) = (self.mapping(cx), self.mask_space(cx)) else {
            return;
        };
        let at = sp.norm_of(m.to_image(p));
        let brush = self.photo.read(cx).brush;
        let Some(d) = &mut self.mask_drag else { return };
        let shape = match d.mode {
            MaskMode::Part(part) => {
                let delta = Pt {
                    x: at.x - d.from.x,
                    y: at.y - d.from.y,
                };
                tools::drag_shape(&sp, &d.start, part, at, delta)
            }
            MaskMode::Create => tools::create_shape(&sp, &d.start, d.from, at),
            MaskMode::Brush => {
                let s = sp.src(at);
                let spacing = (brush.size as f64 * sp.long_edge() * 0.2).max(1.0);
                if (s.0 - d.last_src.0).hypot(s.1 - d.last_src.1) < spacing {
                    return;
                }
                d.last_src = s;
                d.start = tools::brush_add(&d.start, at, None);
                d.start.clone()
            }
        };
        let ops = tools::shape_ops(d.index, &shape);
        d.last = Some(ops.clone());
        let (label, gesture) = (d.label.clone(), d.gesture);
        self.photo.update(cx, |ph, cx| {
            ph.command(label, ops, Some(gesture), Interaction::Continuous, cx)
        });
    }

    fn end_mask(&mut self, cx: &mut Context<Self>) {
        let Some(d) = self.mask_drag.take() else {
            return;
        };
        let ops = match (d.last, &d.mode) {
            (Some(ops), _) => ops,
            // A click without dragging: give the new gradient a visible default size.
            (None, MaskMode::Create) => {
                let Some(sp) = self.mask_space(cx) else {
                    return;
                };
                let (x, y) = sp.src(d.from);
                let r = sp.long_edge() * 0.15;
                let to = match d.start {
                    MaskShape::Linear { .. } => sp.norm(x, y + r),
                    _ => sp.norm(x + r, y),
                };
                tools::shape_ops(d.index, &tools::create_shape(&sp, &d.start, d.from, to))
            }
            (None, _) => tools::shape_ops(d.index, &d.start),
        };
        self.photo.update(cx, |p, cx| {
            p.command(d.label, ops, Some(d.gesture), Interaction::Final, cx)
        });
    }

    // -----------------------------------------------------------------------------------------
    // Events

    fn on_down(&mut self, e: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.photo.read(cx).wb_pick {
            if let Some((at, radius)) = self.image_point(e.position, cx) {
                self.photo.update(cx, |p, cx| p.pick_wb(at, radius, cx));
            }
            return;
        }
        let tool = self.tool;
        match tool {
            Tool::Crop => {
                self.start_crop(e, cx);
                cx.notify();
                return;
            }
            Tool::Mask => {
                if self.start_mask(e, cx) {
                    cx.notify();
                    return;
                }
            }
            Tool::None => {}
        }
        if e.click_count >= 2 {
            if let Some((crop, out, _)) = self.geometry(cx) {
                self.state
                    .toggle_one_to_one(self.local(e.position), crop, out);
                self.update_viewport(Interaction::Final, false, cx);
                cx.notify();
            }
            return;
        }
        if self.near_divider(e.position) {
            self.split_drag = true;
            return;
        }
        self.drag = Some(e.position);
    }

    fn on_move(&mut self, e: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        let wants_hover = {
            let p = self.photo.read(cx);
            p.wb_pick || p.tool != Tool::None
        };
        if wants_hover {
            self.hover = Some(e.position);
            cx.notify();
        }
        let left = e.pressed_button == Some(MouseButton::Left);
        if self.crop_drag.is_some() {
            if left {
                self.move_crop(e.position, cx);
            } else {
                self.end_crop(cx);
            }
            cx.notify();
            return;
        }
        if self.mask_drag.is_some() {
            if left {
                self.move_mask(e.position, cx);
            } else {
                self.end_mask(cx);
            }
            cx.notify();
            return;
        }
        let near = self.near_divider(e.position);
        if near != self.near_divider {
            self.near_divider = near;
            cx.notify();
        }
        if self.split_drag {
            if !left {
                self.split_drag = false;
            } else if let Some((b, _)) = self.layout {
                let f = f32::from(e.position.x - b.origin.x) / f32::from(b.size.width).max(1.0);
                self.split_at = f.clamp(0.02, 0.98);
            }
            cx.notify();
            return;
        }
        let Some(last) = self.drag else { return };
        if !left {
            self.end_drag(cx);
            return;
        }
        let Some((crop, out, sf)) = self.geometry(cx) else {
            return;
        };
        let d = e.position - last;
        self.drag = Some(e.position);
        self.state.pan(
            (f32::from(d.x) * sf) as f64,
            (f32::from(d.y) * sf) as f64,
            crop,
            out,
        );
        self.update_viewport(Interaction::Continuous, false, cx);
        cx.notify();
    }

    fn end_drag(&mut self, cx: &mut Context<Self>) {
        self.end_crop(cx);
        self.end_mask(cx);
        self.split_drag = false;
        if self.drag.take().is_some() {
            self.update_viewport(Interaction::Final, true, cx);
        }
    }

    fn on_scroll(&mut self, e: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.tool == Tool::Crop {
            return;
        }
        let Some((crop, out, _)) = self.geometry(cx) else {
            return;
        };
        let dy = f32::from(e.delta.pixel_delta(px(20.0)).y) as f64;
        if dy == 0.0 {
            return;
        }
        self.state
            .zoom_by((dy * 0.005).exp(), self.local(e.position), crop, out);
        self.update_viewport(Interaction::Continuous, false, cx);
        cx.notify();
    }

    fn cursor(&self, cx: &App) -> Option<CursorStyle> {
        let p = self.photo.read(cx);
        if p.wb_pick {
            return Some(CursorStyle::Crosshair);
        }
        if self.split_drag || self.near_divider {
            return Some(CursorStyle::ResizeLeftRight);
        }
        let hover = self.hover?;
        match p.tool {
            Tool::Crop => {
                let handle = match &self.crop_drag {
                    Some(d) => d.handle,
                    None => {
                        let (_, r) = self.crop_rect(cx)?;
                        tools::crop_hit(&self.mapping(cx)?, r, hover)
                    }
                };
                Some(tools::crop_cursor(handle))
            }
            Tool::Mask if p.pending_mask.is_some() => Some(CursorStyle::Crosshair),
            _ => None,
        }
    }

    fn status(&self, cx: &App) -> String {
        let photo = self.photo.read(cx);
        match &photo.load {
            aigle_engine_api::LoadState::Loading => return crate::i18n::t("正在加载…").into(),
            aigle_engine_api::LoadState::Failed(e) => {
                return rust_i18n::t!("无法打开：{e}", e = e).into_owned();
            }
            aigle_engine_api::LoadState::Ready(_) => {}
        }
        let zoom = self
            .geometry(cx)
            .map(|(crop, out, _)| format!("{:.0}%", self.state.scale(crop, out) * 100.0))
            .unwrap_or_default();
        let q = match self.frame.as_ref().map(|(f, _)| f.quality) {
            Some(Quality::Draft) => crate::i18n::t("草稿"),
            Some(Quality::Refined) => crate::i18n::t("精细"),
            Some(Quality::Full) => "1:1".into(),
            None => crate::i18n::t("渲染中"),
        };
        let before = match self.compare {
            Compare::Off => String::new(),
            Compare::Before => crate::i18n::t(" · 原图"),
            Compare::Split => crate::i18n::t(" · 分屏对比"),
        };
        let hint = if photo.wb_pick {
            crate::i18n::t(" · 吸管：点击应为中性灰的位置，Esc 取消")
        } else {
            match photo.tool {
                Tool::Crop => {
                    crate::i18n::t(" · 裁剪：拖动边角调整，框外拖动旋转，双击框内或 Enter 完成")
                }
                Tool::Mask if photo.pending_mask.is_some() => {
                    crate::i18n::t(" · 在画面上拖动以放置蒙版")
                }
                Tool::Mask => crate::i18n::t(" · 蒙版：拖动控制点调整，O 显示叠加"),
                Tool::None => String::new(),
            }
        };
        format!("{zoom} · {q}{before}{hint}")
    }
}

/// Picker ring radius, logical px.
const PICK_RADIUS: f32 = 5.0;

fn paint_ring(window: &mut Window, c: Point<Pixels>) {
    for (r, w, color) in [
        (PICK_RADIUS + 1.0, 3.0, gpui::rgba(0x000000c0)),
        (PICK_RADIUS + 1.0, 1.0, gpui::rgba(0xffffffff)),
    ] {
        let mut b = PathBuilder::stroke(px(w));
        for i in 0..=32 {
            let a = i as f32 / 32.0 * std::f32::consts::TAU;
            let p = c + point(px(r * a.cos()), px(r * a.sin()));
            if i == 0 {
                b.move_to(p);
            } else {
                b.line_to(p);
            }
        }
        if let Ok(path) = b.build() {
            window.paint_path(path, color);
        }
    }
}

fn paint_divider(window: &mut Window, x: Pixels, b: Bounds<Pixels>) {
    window.paint_quad(fill(
        Bounds::new(point(x - px(1.5), b.origin.y), size(px(3.0), b.size.height)),
        gpui::rgba(0x00000080),
    ));
    window.paint_quad(fill(
        Bounds::new(point(x - px(0.5), b.origin.y), size(px(1.0), b.size.height)),
        gpui::rgba(0xffffffd0),
    ));
    let r = px(9.0);
    let c = point(x, b.origin.y + b.size.height / 2.0);
    window.paint_quad(
        fill(
            Bounds::new(c - point(r, r), size(r * 2.0, r * 2.0)),
            gpui::rgba(0x202020e0),
        )
        .corner_radii(r)
        .border_widths(px(1.0))
        .border_color(gpui::rgba(0xffffffd0)),
    );
}

pub fn render_image(p: &PixelBuffer) -> Option<Arc<RenderImage>> {
    let row = p.width as usize * 4;
    let mut data = Vec::with_capacity(row * p.height as usize);
    for y in 0..p.height as usize {
        let at = y * p.stride as usize;
        data.extend_from_slice(p.data.get(at..at + row)?);
    }
    // GPUI's RenderImage holds BGRA bytes in an `RgbaImage` container.
    let buf = RgbaImage::from_raw(p.width, p.height, data)?;
    Some(Arc::new(RenderImage::new([ImageFrame::new(buf)])))
}

/// What the tools draw over the image, computed at render time.
enum ToolOverlay {
    None,
    Crop(Rect),
    Mask {
        space: Box<MaskSpace>,
        selected: Option<MaskShape>,
        anchors: Vec<(f64, f64)>,
        /// Brush outline radius (outer, inner) in image units.
        brush: Option<(f64, f64)>,
    },
}

impl Render for Canvas {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let me = cx.entity();
        let frame = self.frame.clone();
        let target = self.geometry(cx).map(|(crop, out, _)| {
            (
                self.state.region(crop, out),
                out,
                self.state.scale(crop, out),
            )
        });
        let status = self.status(cx);
        let photo = self.photo.read(cx);
        let overlay = if self.show_info && self.tool != Tool::Crop {
            photo
                .info()
                .map(|i| meta::overlay_lines(i))
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        let ring = self.hover.filter(|_| photo.wb_pick);
        let tool_overlay = match self.tool {
            Tool::Crop => self
                .crop_rect(cx)
                .map_or(ToolOverlay::None, |(_, r)| ToolOverlay::Crop(r)),
            Tool::Mask => match self.mask_space(cx) {
                Some(space) => {
                    let selected = photo.selected().map(|(_, m)| m.shape.clone());
                    let anchors = photo
                        .head()
                        .masks
                        .iter()
                        .filter(|m| Some(m.id) != photo.selected_mask)
                        .filter_map(|m| tools::anchor(&space, &m.shape))
                        .collect();
                    let brushing = matches!(selected, Some(MaskShape::Brush { .. }))
                        || photo.pending_mask == Some(tools::NewMask::Brush);
                    let b = photo.brush;
                    let brush = brushing.then(|| {
                        let outer = b.size as f64 * space.long_edge();
                        (outer, outer * (1.0 - b.feather as f64 / 100.0))
                    });
                    ToolOverlay::Mask {
                        space: Box::new(space),
                        selected,
                        anchors,
                        brush,
                    }
                }
                None => ToolOverlay::None,
            },
            Tool::None => ToolOverlay::None,
        };
        let hover = self.hover;
        let before_frame = self.before_frame.clone();
        let split = (self.compare == Compare::Split).then_some(self.split_at);
        let cursor = self.cursor(cx);
        let labels = split.zip(self.layout).map(|(f, (b, _))| {
            let w = f32::from(b.size.width);
            (w * f, w)
        });

        div()
            .id("canvas")
            .relative()
            .size_full()
            .overflow_hidden()
            .bg(rgb(0x1a1a1a))
            .when_some(cursor, |d, c| d.cursor(c))
            .on_hover(cx.listener(|c, hovered: &bool, _, cx| {
                if !*hovered && c.hover.take().is_some() {
                    cx.notify();
                }
            }))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_down))
            .on_mouse_move(cx.listener(Self::on_move))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|c, _: &MouseUpEvent, _, cx| c.end_drag(cx)),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|c, _: &MouseUpEvent, _, cx| c.end_drag(cx)),
            )
            .on_scroll_wheel(cx.listener(Self::on_scroll))
            .child(
                canvas(
                    move |bounds, window, cx| {
                        let sf = window.scale_factor();
                        cx.defer(move |cx| me.update(cx, |c, cx| c.set_layout(bounds, sf, cx)));
                        sf
                    },
                    move |bounds, sf, window, _| {
                        window.paint_quad(fill(bounds, rgb(0x1a1a1a)));
                        let Some((region, out, scale)) = target else {
                            return;
                        };
                        let paint = |window: &mut Window, shown: &Shown, clip: Bounds<Pixels>| {
                            let (f, img) = shown;
                            let r = reproject(f.viewport.region, region, out);
                            let to_px = |v: f64| px(v as f32 / sf);
                            let at = Bounds::new(
                                bounds.origin + point(to_px(r.x), to_px(r.y)),
                                size(to_px(r.w), to_px(r.h)),
                            );
                            let _ = window.paint_image(
                                clip,
                                at,
                                Corners::default(),
                                img.clone(),
                                0,
                                false,
                            );
                        };
                        match split {
                            Some(f) => {
                                let x = bounds.origin.x + bounds.size.width * f;
                                let left =
                                    Bounds::from_corners(bounds.origin, point(x, bounds.bottom()));
                                let right = Bounds::from_corners(
                                    point(x, bounds.origin.y),
                                    bounds.bottom_right(),
                                );
                                if let Some(s) = &before_frame {
                                    paint(window, s, left);
                                }
                                if let Some(s) = &frame {
                                    paint(window, s, right);
                                }
                                paint_divider(window, x, bounds);
                            }
                            None => {
                                if let Some(s) = &frame {
                                    paint(window, s, bounds);
                                }
                            }
                        }
                        let m = Mapping {
                            origin: bounds.origin,
                            region,
                            scale,
                            sf,
                        };
                        match &tool_overlay {
                            ToolOverlay::Crop(r) => tools::paint_crop(window, &m, *r, bounds),
                            ToolOverlay::Mask {
                                space,
                                selected,
                                anchors,
                                brush,
                            } => {
                                for a in anchors {
                                    tools::paint_anchor(
                                        window,
                                        tools::anchor_window(space, &m, *a),
                                    );
                                }
                                if let Some(s) = selected {
                                    tools::paint_mask(window, space, &m, s, bounds);
                                }
                                if let (Some((outer, inner)), Some(h)) = (brush, hover) {
                                    tools::paint_brush_cursor(
                                        window,
                                        h,
                                        m.len(*outer),
                                        m.len(*inner),
                                    );
                                }
                            }
                            ToolOverlay::None => {}
                        }
                        if let Some(c) = ring {
                            paint_ring(window, c);
                        }
                    },
                )
                .size_full(),
            )
            .when_some(labels, |d, (x, w)| {
                let tag = |s: String| {
                    div()
                        .absolute()
                        .top_2()
                        .px_2()
                        .py_0p5()
                        .rounded_sm()
                        .bg(gpui::rgba(0x000000a0))
                        .text_xs()
                        .text_color(rgb(0xe8e8e8))
                        .child(s)
                };
                d.child(tag(crate::i18n::t("原图")).right(px(w - x + 8.0)))
                    .child(tag(crate::i18n::t("调整后")).left(px(x + 8.0)))
            })
            .child(
                div()
                    .absolute()
                    .bottom_2()
                    .left_2()
                    .px_2()
                    .py_0p5()
                    .rounded_sm()
                    .bg(gpui::rgba(0x000000a0))
                    .text_xs()
                    .text_color(rgb(0xd0d0d0))
                    .child(status),
            )
            .when(!overlay.is_empty(), |d| {
                let mut lines = overlay.into_iter();
                let head = lines.next().unwrap_or_default();
                d.child(
                    v_flex()
                        .absolute()
                        .top_2()
                        .left_2()
                        .px_2()
                        .py_1()
                        .gap_0p5()
                        .rounded_md()
                        .bg(gpui::rgba(0x000000a0))
                        .text_color(rgb(0xe8e8e8))
                        .child(
                            div()
                                .text_xs()
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .child(head),
                        )
                        .children(lines.map(|l| div().text_xs().child(l))),
                )
            })
    }
}
