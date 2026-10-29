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
use aigle_core::retouch::{MAX_RADIUS, MIN_RADIUS, RedEye, Spot};
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
use crate::tools::{self, CirclePart, Mapping, MaskSpace, Part};
use crate::viewport::{ViewState, reproject};

type Shown = (Arc<Frame>, Arc<RenderImage>);

// UI callbacks may run while their window is temporarily removed from App.windows.
// Defer eviction until it is back, otherwise drop_image(None) misses that window.
fn release_image(image: Arc<RenderImage>, cx: &mut App) {
    cx.defer(move |cx| cx.drop_image(image, None));
}

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

#[derive(Clone, Copy, PartialEq, Eq)]
enum RetouchKind {
    Spot,
    Eye,
}

/// A drag of the spot or red-eye tool.
struct RetouchDrag {
    kind: RetouchKind,
    index: usize,
    /// `None`: creating (a spot with a manual source, or a red-eye circle).
    part: Option<CirclePart>,
    from: Pt,
    from_window: Point<Pixels>,
    spots: Vec<Spot>,
    eyes: Vec<RedEye>,
    gesture: GestureId,
    last: Option<Vec<ParamOp>>,
}

/// Pointer travel (logical px) before a press becomes a drag.
const DRAG_START: f32 = 3.0;
/// Radius of a red-eye correction placed by a click, fraction of the long edge.
const EYE_RADIUS: f32 = 0.012;

pub struct Canvas {
    photo: Entity<Photo>,
    view: View,
    state: ViewState,
    frame: Option<Shown>,
    _frames: Task<anyhow::Result<()>>,
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
    /// Soft-proof target used when proofing is turned on.
    proof_target: DisplayProfileId,
    crop_drag: Option<CropDrag>,
    mask_drag: Option<MaskDrag>,
    retouch_drag: Option<RetouchDrag>,
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
                    release_image(old, cx);
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
    pub fn new(photo: Entity<Photo>, window: &mut Window, cx: &mut Context<Self>) -> Canvas {
        let view = photo.read(cx).session.create_view(ViewSpec::fit(1, 1));
        let frames = listen(view.frames(), |c| &mut c.frame, cx);
        cx.on_release_in(window, |canvas, window, cx| {
            for slot in [&mut canvas.frame, &mut canvas.before_frame] {
                if let Some((_, image)) = slot.take() {
                    cx.drop_image(image, Some(window));
                }
            }
        })
        .detach();
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
            PhotoEvent::Notice(_) | PhotoEvent::SnapshotsChanged => {}
        })
        .detach();
        cx.observe(&photo, |c, _, cx| {
            c.sync_overlays(cx);
            cx.notify()
        })
        .detach();
        cx.observe_global::<crate::display::DisplayColor>(|c, cx| {
            c.update_viewport(Interaction::Final, false, cx);
            cx.notify()
        })
        .detach();
        Canvas {
            photo,
            view,
            state: ViewState::default(),
            frame: None,
            _frames: frames,
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
            proof_target: DisplayProfileId::Srgb,
            crop_drag: None,
            mask_drag: None,
            retouch_drag: None,
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
        self.retouch_drag = None;
        if crop_changed {
            if tool == Tool::Crop {
                self.set_compare(Compare::Off, cx);
            }
            self.view.set_uncropped(tool == Tool::Crop);
            self.state = ViewState::default();
            if let Some((_, old)) = self.frame.take() {
                release_image(old, cx);
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
            display: crate::display::profile(cx),
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

    /// Soft-proof target, if proofing.
    pub fn proof(&self) -> Option<&DisplayProfileId> {
        self.overlays.proof.as_ref()
    }

    pub fn gamut_warning(&self) -> bool {
        self.overlays.gamut_warning
    }

    pub fn proof_target(&self) -> &DisplayProfileId {
        &self.proof_target
    }

    /// Turn soft proofing on or off (to the last chosen target).
    pub fn toggle_proof(&mut self, cx: &mut Context<Self>) {
        let next = match self.overlays.proof {
            Some(_) => None,
            None => Some(self.proof_target.clone()),
        };
        self.set_proof_overlays(next, self.overlays.gamut_warning, cx);
    }

    /// Proof to `target` (turning proofing on).
    pub fn set_proof_target(&mut self, target: DisplayProfileId, cx: &mut Context<Self>) {
        self.proof_target = target.clone();
        self.set_proof_overlays(Some(target), self.overlays.gamut_warning, cx);
    }

    /// The gamut warning implies proofing.
    pub fn toggle_gamut_warning(&mut self, cx: &mut Context<Self>) {
        let on = !self.overlays.gamut_warning;
        let proof = match (&self.overlays.proof, on) {
            (None, true) => Some(self.proof_target.clone()),
            (p, _) => p.clone(),
        };
        self.set_proof_overlays(proof, on, cx);
    }

    fn set_proof_overlays(
        &mut self,
        proof: Option<DisplayProfileId>,
        gamut_warning: bool,
        cx: &mut Context<Self>,
    ) {
        self.overlays.gamut_warning = gamut_warning && proof.is_some();
        self.overlays.proof = proof;
        self.view.set_overlays(self.overlays.clone());
        if let Some((v, _)) = &self.before {
            v.set_overlays(Overlays {
                proof: self.overlays.proof.clone(),
                gamut_warning: self.overlays.gamut_warning,
                ..Default::default()
            });
        }
        cx.notify();
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
            v.set_overlays(Overlays {
                proof: self.overlays.proof.clone(),
                gamut_warning: self.overlays.gamut_warning,
                ..Default::default()
            });
            if let Some(vp) = &self.sent {
                v.set_viewport(vp.clone(), Interaction::Final);
            }
            let task = listen(v.frames(), |c| &mut c.before_frame, cx);
            self.before = Some((v, task));
        } else {
            self.before = None;
            self.split_drag = false;
            if let Some((_, old)) = self.before_frame.take() {
                release_image(old, cx);
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
                ("crop.crop", crop_ops(&f.crop_of(r)))
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
                    "crop.straighten",
                    vec![ParamOp::set("geometry.angle", angle)],
                )
            }
        };
        let gesture = d.gesture;
        self.photo.update(cx, |ph, cx| {
            ph.command(
                label,
                ops.clone(),
                Some(gesture),
                Interaction::Continuous,
                cx,
            )
        });
        if let Some(d) = &mut self.crop_drag {
            d.last = Some((label.to_owned(), ops));
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
                    "masks.add",
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
                label: "masks.add".into(),
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
                label: "masks.edit".into(),
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
                    "masks.kind.brush",
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
                label: "masks.kind.brush".into(),
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
    // Spot and red-eye tools

    /// Start a spot or red-eye drag at `e`; `false` lets the click pan instead.
    fn start_retouch(&mut self, e: &MouseDownEvent, cx: &mut Context<Self>) -> bool {
        let (Some(m), Some(sp)) = (self.mapping(cx), self.mask_space(cx)) else {
            return false;
        };
        if e.click_count >= 2 {
            return false;
        }
        let kind = if self.tool == Tool::Spot {
            RetouchKind::Spot
        } else {
            RetouchKind::Eye
        };
        let at = sp.norm_of(m.to_image(e.position));
        let photo = self.photo.read(cx);
        let head = photo.head().clone();
        let hit = match kind {
            RetouchKind::Spot => {
                tools::spot_hit(&sp, &m, &head.spots, photo.selected_spot, e.position)
            }
            RetouchKind::Eye => {
                tools::eye_hit(&sp, &m, &head.red_eye, photo.selected_eye, e.position)
            }
        };
        let (index, part) = match hit {
            Some((i, part)) => (i, Some(part)),
            None => (
                match kind {
                    RetouchKind::Spot => head.spots.len(),
                    RetouchKind::Eye => head.red_eye.len(),
                },
                None,
            ),
        };
        let gesture = self.photo.update(cx, |p, cx| {
            if part.is_some() {
                match kind {
                    RetouchKind::Spot => p.selected_spot = Some(index),
                    RetouchKind::Eye => p.selected_eye = Some(index),
                }
                cx.notify();
            }
            p.new_gesture()
        });
        self.retouch_drag = Some(RetouchDrag {
            kind,
            index,
            part,
            from: at,
            from_window: e.position,
            spots: head.spots.clone(),
            eyes: head.red_eye.clone(),
            gesture,
            last: None,
        });
        true
    }

    fn move_retouch(&mut self, p: Point<Pixels>, cx: &mut Context<Self>) {
        let (Some(m), Some(sp)) = (self.mapping(cx), self.mask_space(cx)) else {
            return;
        };
        let at = sp.norm_of(m.to_image(p));
        let b = self.photo.read(cx).spot_brush;
        let Some(d) = &mut self.retouch_drag else {
            return;
        };
        if d.last.is_none()
            && f32::from(p.x - d.from_window.x).hypot(f32::from(p.y - d.from_window.y)) < DRAG_START
        {
            return;
        }
        let delta = Pt {
            x: at.x - d.from.x,
            y: at.y - d.from.y,
        };
        let (label, op) = match (d.kind, d.part) {
            (RetouchKind::Spot, part) => {
                let mut spots = d.spots.clone();
                match part {
                    Some(part) => {
                        spots[d.index] = tools::drag_spot(&sp, &d.spots[d.index], part, at, delta)
                    }
                    None => {
                        let mut s = Spot::new(b.mode, d.from, at, b.size);
                        s.feather = b.feather;
                        s.opacity = b.opacity;
                        spots.push(s);
                    }
                }
                ("tool.spot", tools::spots_op(&spots))
            }
            (RetouchKind::Eye, part) => {
                let mut eyes = d.eyes.clone();
                match part {
                    Some(part) => {
                        eyes[d.index] = tools::drag_eye(&sp, &d.eyes[d.index], part, at, delta)
                    }
                    None => eyes.push(RedEye::new(
                        d.from,
                        tools::radius_to(&sp, d.from, at).clamp(MIN_RADIUS, MAX_RADIUS),
                    )),
                }
                ("tool.red_eye", tools::eyes_op(&eyes))
            }
        };
        let ops = vec![op];
        let first = d.last.is_none();
        d.last = Some(ops.clone());
        let (kind, index, gesture, creating) = (d.kind, d.index, d.gesture, d.part.is_none());
        self.photo.update(cx, |ph, cx| {
            if ph.command(label, ops, Some(gesture), Interaction::Continuous, cx)
                && first
                && creating
            {
                match kind {
                    RetouchKind::Spot => ph.selected_spot = Some(index),
                    RetouchKind::Eye => ph.selected_eye = Some(index),
                }
            }
        });
    }

    fn end_retouch(&mut self, cx: &mut Context<Self>) {
        let Some(d) = self.retouch_drag.take() else {
            return;
        };
        let label = match d.kind {
            RetouchKind::Spot => "tool.spot",
            RetouchKind::Eye => "tool.red_eye",
        };
        match (d.last, d.part, d.kind) {
            (Some(ops), _, _) => {
                self.photo.update(cx, |p, cx| {
                    p.command(label, ops, Some(d.gesture), Interaction::Final, cx)
                });
            }
            // A click on empty image: a spot with an automatic source, or a red-eye
            // correction of the default size.
            (None, None, RetouchKind::Spot) => {
                self.photo.update(cx, |p, cx| p.add_spot(d.from, cx));
            }
            (None, None, RetouchKind::Eye) => {
                let mut eyes = d.eyes;
                eyes.push(RedEye::new(d.from, EYE_RADIUS));
                let i = eyes.len() - 1;
                self.photo.update(cx, |p, cx| {
                    if p.command(
                        label,
                        vec![tools::eyes_op(&eyes)],
                        Some(d.gesture),
                        Interaction::Final,
                        cx,
                    ) {
                        p.selected_eye = Some(i);
                    }
                });
            }
            (None, Some(_), _) => {}
        }
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
            Tool::Spot | Tool::RedEye => {
                if self.start_retouch(e, cx) {
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
        if self.retouch_drag.is_some() {
            if left {
                self.move_retouch(e.position, cx);
            } else {
                self.end_retouch(cx);
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
        self.end_retouch(cx);
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
            Tool::Spot | Tool::RedEye => {
                let (m, sp) = (self.mapping(cx)?, self.mask_space(cx)?);
                let head = p.head();
                let hit = if p.tool == Tool::Spot {
                    tools::spot_hit(&sp, &m, &head.spots, p.selected_spot, hover)
                } else {
                    tools::eye_hit(&sp, &m, &head.red_eye, p.selected_eye, hover)
                };
                Some(match hit {
                    Some((_, CirclePart::Edge)) => CursorStyle::ResizeLeftRight,
                    Some(_) => CursorStyle::OpenHand,
                    None => CursorStyle::Crosshair,
                })
            }
            _ => None,
        }
    }

    fn status(&self, cx: &App) -> String {
        let photo = self.photo.read(cx);
        match &photo.load {
            aigle_engine_api::LoadState::Loading => return crate::i18n::t("canvas.loading"),
            aigle_engine_api::LoadState::Failed(e) => {
                return rust_i18n::t!("canvas.open_failed", e = e).into_owned();
            }
            aigle_engine_api::LoadState::Ready(_) => {}
        }
        let zoom = self
            .geometry(cx)
            .map(|(crop, out, _)| format!("{:.0}%", self.state.scale(crop, out) * 100.0))
            .unwrap_or_default();
        let q = match self.frame.as_ref().map(|(f, _)| f.quality) {
            Some(Quality::Draft) => crate::i18n::t("canvas.quality.draft"),
            Some(Quality::Refined) => crate::i18n::t("canvas.quality.refined"),
            Some(Quality::Full) => "1:1".into(),
            None => crate::i18n::t("canvas.rendering"),
        };
        let before = match self.compare {
            Compare::Off => None,
            Compare::Before => Some(crate::i18n::t("canvas.before")),
            Compare::Split => Some(crate::i18n::t("canvas.split")),
        };
        let hint = if photo.wb_pick {
            Some(crate::keymap::hint("canvas.hint.wb_pick", cx))
        } else {
            match photo.tool {
                Tool::Crop => Some(crate::keymap::hint("canvas.hint.crop", cx)),
                Tool::Mask if photo.pending_mask.is_some() => {
                    Some(crate::i18n::t("canvas.hint.place_mask"))
                }
                Tool::Mask => Some(crate::keymap::hint("canvas.hint.mask", cx)),
                Tool::Spot => Some(crate::keymap::hint("canvas.hint.spot", cx)),
                Tool::RedEye => Some(crate::keymap::hint("canvas.hint.red_eye", cx)),
                Tool::None => None,
            }
        };
        [Some(zoom), Some(q), before, hint]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" · ")
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
    Spots {
        space: Box<MaskSpace>,
        spots: Vec<Spot>,
        selected: Option<usize>,
        /// Outline of a new spot at the cursor, in image units.
        brush: Option<f64>,
    },
    Eyes {
        space: Box<MaskSpace>,
        eyes: Vec<RedEye>,
        selected: Option<usize>,
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
            Tool::Spot => match self.mask_space(cx) {
                Some(space) => {
                    let spots = photo.head().spots.clone();
                    let over = match (self.hover, self.mapping(cx)) {
                        (Some(h), Some(m)) => {
                            tools::spot_hit(&space, &m, &spots, photo.selected_spot, h).is_some()
                        }
                        _ => true,
                    };
                    let brush = (!over && self.retouch_drag.is_none())
                        .then(|| photo.spot_brush.size as f64 * space.long_edge());
                    ToolOverlay::Spots {
                        space: Box::new(space),
                        spots,
                        selected: photo.selected_spot,
                        brush,
                    }
                }
                None => ToolOverlay::None,
            },
            Tool::RedEye => match self.mask_space(cx) {
                Some(space) => ToolOverlay::Eyes {
                    space: Box::new(space),
                    eyes: photo.head().red_eye.clone(),
                    selected: photo.selected_eye,
                },
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
                            ToolOverlay::Spots {
                                space,
                                spots,
                                selected,
                                brush,
                            } => {
                                tools::paint_spots(window, space, &m, spots, *selected);
                                if let (Some(r), Some(h)) = (brush, hover) {
                                    let r = m.len(*r);
                                    tools::paint_brush_cursor(window, h, r, r);
                                }
                            }
                            ToolOverlay::Eyes {
                                space,
                                eyes,
                                selected,
                            } => tools::paint_eyes(window, space, &m, eyes, *selected),
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
                d.child(tag(crate::i18n::t("canvas.before")).right(px(w - x + 8.0)))
                    .child(tag(crate::i18n::t("canvas.after")).left(px(x + 8.0)))
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
