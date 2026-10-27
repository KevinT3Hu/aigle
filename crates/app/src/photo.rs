//! The open photo: engine session plus its `doc` (doc 01 §5).

use std::path::PathBuf;
use std::sync::Arc;

use aigle_core::geometry::{GeometryMap, ImagePoint, Orientation, Size};
use aigle_core::masks::LocalAdjustment;
use aigle_core::params::{Aspect, CropRect, GeometryParams};
use aigle_core::{EditParams, MaskId};
use aigle_doc::{CommandError, EditCommand, GestureId, ParamOp, PhotoDoc};
use aigle_engine_api::{Engine, ImageInfo, Interaction, LoadState, Session, SourceRef};
use aigle_library::Snapshot;
use gpui::{Context, EventEmitter, Task};

use crate::crop::{self, Frame};
use crate::meta::FileFacts;
use crate::tools::{NewMask, Reorient, reorient_ops};

pub enum PhotoEvent {
    /// Loading finished (successfully or not).
    Loaded,
    /// The head changed (edit, undo, redo).
    Edited,
    /// A message for the user (failed pick, clamped value).
    Notice(String),
    /// The canvas tool changed.
    ToolChanged,
    /// The saved snapshots changed.
    SnapshotsChanged,
}

pub struct Photo {
    pub name: String,
    /// The photo's file; `None` for generated images.
    pub path: Option<PathBuf>,
    pub source: SourceRef,
    pub file: FileFacts,
    pub engine: Engine,
    pub session: Session,
    pub doc: PhotoDoc,
    /// Saved states of the edits, oldest first; persisted in the sidecar.
    pub snapshots: Vec<Snapshot>,
    pub load: LoadState,
    /// The white balance picker is active on the canvas.
    pub wb_pick: bool,
    pick_job: Option<Task<()>>,
    next_gesture: u64,
    /// The canvas tool in use.
    pub tool: Tool,
    /// The crop at the start of the current straightening gesture.
    crop_anchor: Option<(Option<GestureId>, CropRect)>,
    /// The local adjustment edited with the mask tool.
    pub selected_mask: Option<MaskId>,
    /// Tint the selected mask on the canvas.
    pub mask_overlay: bool,
    /// The next drag on the canvas creates a mask of this kind.
    pub pending_mask: Option<NewMask>,
    pub brush: Brush,
}

/// Brush settings of the mask tool.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Brush {
    /// Radius as a fraction of the source long edge.
    pub size: f32,
    pub feather: f32,
    pub flow: f32,
    pub erase: bool,
}

impl Default for Brush {
    fn default() -> Self {
        Brush {
            size: 0.03,
            feather: 50.0,
            flow: 100.0,
            erase: false,
        }
    }
}

/// Canvas tools that replace pan/zoom dragging.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    None,
    Crop,
    Mask,
}

/// Ops setting the crop to `c`.
pub fn crop_ops(c: &CropRect) -> Vec<ParamOp> {
    vec![
        ParamOp::set("geometry.crop.x", c.x),
        ParamOp::set("geometry.crop.y", c.y),
        ParamOp::set("geometry.crop.w", c.w),
        ParamOp::set("geometry.crop.h", c.h),
    ]
}

impl EventEmitter<PhotoEvent> for Photo {}

impl Photo {
    /// Open `source` with the edits `initial` and `snapshots` (from its sidecar).
    pub fn open(
        engine: &Engine,
        source: SourceRef,
        initial: EditParams,
        snapshots: Vec<Snapshot>,
        cx: &mut Context<Self>,
    ) -> Photo {
        let name = match &source {
            SourceRef::File(p) => p
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            SourceRef::Synthetic { pattern, .. } => format!("{pattern:?}"),
        };
        let file = match &source {
            SourceRef::File(p) => FileFacts {
                name: name.clone(),
                path: p.parent().map(|d| d.display().to_string()),
                bytes: std::fs::metadata(p).ok().map(|m| m.len()),
            },
            SourceRef::Synthetic { .. } => FileFacts {
                name: name.clone(),
                ..Default::default()
            },
        };
        let path = match &source {
            SourceRef::File(p) => Some(p.clone()),
            SourceRef::Synthetic { .. } => None,
        };
        let session = engine.open(source.clone());
        let mut doc = PhotoDoc::new(initial).unwrap_or_else(|e| {
            tracing::warn!("stored edits are invalid, opening unedited: {e}");
            PhotoDoc::new(EditParams::default()).expect("default params are valid")
        });
        doc.attach(session.clone());

        let mut info = session.info();
        cx.spawn(async move |this, cx| {
            loop {
                let state = info.borrow_and_update().clone();
                if !matches!(state, LoadState::Loading) {
                    this.update(cx, |p, cx| {
                        p.load = state;
                        cx.emit(PhotoEvent::Loaded);
                        cx.notify();
                    })?;
                    break;
                }
                if info.changed().await.is_err() {
                    break;
                }
            }
            anyhow::Ok(())
        })
        .detach();

        Photo {
            name,
            path,
            source,
            file,
            engine: engine.clone(),
            session,
            doc,
            snapshots,
            load: LoadState::Loading,
            wb_pick: false,
            pick_job: None,
            next_gesture: 1,
            tool: Tool::None,
            crop_anchor: None,
            selected_mask: None,
            mask_overlay: false,
            pending_mask: None,
            brush: Brush::default(),
        }
    }

    pub fn info(&self) -> Option<&Arc<ImageInfo>> {
        match &self.load {
            LoadState::Ready(i) => Some(i),
            _ => None,
        }
    }

    pub fn head(&self) -> &Arc<EditParams> {
        self.doc.head()
    }

    /// Source size under the orientation delta of `g`.
    pub fn oriented_size(&self, g: &GeometryParams) -> Option<Size> {
        let info = self.info()?;
        Some(Orientation::from_delta(g.orientation_delta).apply_size(info.source_size))
    }

    /// The straightened frame of the head (crop tool coordinates).
    pub fn frame(&self) -> Option<Frame> {
        let g = &self.head().geometry;
        Some(Frame::new(self.oriented_size(g)?, g))
    }

    pub fn set_tool(&mut self, tool: Tool, cx: &mut Context<Self>) {
        let tool = if self.info().is_some() {
            tool
        } else {
            Tool::None
        };
        if self.tool != tool {
            self.tool = tool;
            self.wb_pick = false;
            if tool != Tool::Mask {
                self.mask_overlay = false;
            }
            cx.emit(PhotoEvent::ToolChanged);
            cx.notify();
        }
    }

    pub fn toggle_tool(&mut self, tool: Tool, cx: &mut Context<Self>) {
        let next = if self.tool == tool { Tool::None } else { tool };
        self.set_tool(next, cx);
    }

    /// Commit `ops` as a user command; errors are reported as a notice.
    pub fn command(
        &mut self,
        label: impl AsRef<str>,
        ops: Vec<ParamOp>,
        gesture: Option<GestureId>,
        hint: Interaction,
        cx: &mut Context<Self>,
    ) -> bool {
        let mut cmd = EditCommand::user(self.doc.rev(), label.as_ref(), ops);
        cmd.gesture = gesture;
        match self.edit(cmd, hint, cx) {
            Ok(()) => true,
            Err(e) => {
                cx.emit(PhotoEvent::Notice(e.to_string()));
                false
            }
        }
    }

    /// Turn the displayed image a quarter; crop and masks turn with it.
    pub fn rotate_quarter(&mut self, cw: bool, cx: &mut Context<Self>) {
        let head = self.head().clone();
        let Some(size) = self.oriented_size(&head.geometry) else {
            return;
        };
        let g = &head.geometry;
        let delta = crop::turn(g.orientation_delta, cw);
        let mut ops = crop_ops(&crop::rotate_quarter(&g.crop, cw));
        ops.push(ParamOp::set(
            "geometry.orientation_delta",
            serde_json::to_value(delta).expect("serialisable"),
        ));
        if let Aspect::Ratio { w, h } = g.aspect {
            ops.push(ParamOp::set(
                "geometry.aspect",
                serde_json::to_value(Aspect::Ratio { w: h, h: w }).expect("serialisable"),
            ));
        }
        let (w, h) = (size.w as f64, size.h as f64);
        let r = if cw {
            Reorient::Cw { w, h }
        } else {
            Reorient::Ccw { w, h }
        };
        ops.extend(reorient_ops(&head, r));
        let label = if cw {
            crate::i18n::t("向右旋转 90°")
        } else {
            crate::i18n::t("向左旋转 90°")
        };
        self.command(label, ops, None, Interaction::Final, cx);
    }

    /// Mirror the displayed image left to right; crop, angle and masks follow.
    pub fn mirror(&mut self, cx: &mut Context<Self>) {
        let head = self.head().clone();
        let g = &head.geometry;
        let mut c = g.crop.clone();
        c.x = (1.0 - c.x - c.w).max(0.0);
        let mut ops = crop_ops(&c);
        ops.push(ParamOp::set(
            "geometry.orientation_delta",
            serde_json::to_value(crop::mirror(g.orientation_delta)).expect("serialisable"),
        ));
        ops.push(ParamOp::set("geometry.angle", -g.scalars.angle));
        ops.extend(reorient_ops(&head, Reorient::Mirror));
        self.command(
            crate::i18n::t("水平翻转"),
            ops,
            None,
            Interaction::Final,
            cx,
        );
    }

    /// Lock the crop to `aspect`, refitting the crop around its centre.
    pub fn set_aspect(&mut self, aspect: Aspect, cx: &mut Context<Self>) {
        let Some(f) = self.frame() else { return };
        let g = &self.head().geometry;
        let mut ops = vec![ParamOp::set(
            "geometry.aspect",
            serde_json::to_value(aspect).expect("serialisable"),
        )];
        if let Some(k) = crop::ratio(aspect, f.w, f.h) {
            let r = f.with_aspect(f.rect_of(&g.crop), k);
            ops.extend(crop_ops(&f.crop_of(r)));
        }
        self.command(
            crate::i18n::t("裁剪 · 长宽比"),
            ops,
            None,
            Interaction::Final,
            cx,
        );
    }

    pub fn reset_crop(&mut self, cx: &mut Context<Self>) {
        let ops = ["geometry.crop", "geometry.angle", "geometry.aspect"]
            .iter()
            .map(|p| ParamOp::Reset { path: (*p).into() })
            .collect();
        self.command(
            crate::i18n::t("重置裁剪"),
            ops,
            None,
            Interaction::Final,
            cx,
        );
    }

    /// The selected mask, if it still exists, with its index.
    pub fn selected(&self) -> Option<(usize, &LocalAdjustment)> {
        let id = self.selected_mask?;
        self.head()
            .masks
            .iter()
            .enumerate()
            .find(|(_, m)| m.id == id)
    }

    pub fn select_mask(&mut self, id: Option<MaskId>, cx: &mut Context<Self>) {
        if self.selected_mask != id {
            self.selected_mask = id;
            cx.notify();
        }
    }

    pub fn delete_mask(&mut self, id: MaskId, cx: &mut Context<Self>) {
        if self.command(
            crate::i18n::t("删除蒙版"),
            vec![ParamOp::RemoveMask { id }],
            None,
            Interaction::Final,
            cx,
        ) && self.selected_mask == Some(id)
        {
            self.selected_mask = None;
        }
    }

    /// Straightening or perspective changes without a crop of their own refit the crop
    /// taken at the start of the gesture into the image, so it can grow back.
    fn constrain_crop(&mut self, mut cmd: EditCommand) -> EditCommand {
        let touches_crop = cmd.ops.iter().any(|op| match op {
            ParamOp::Set { path, .. } | ParamOp::Reset { path } | ParamOp::Add { path, .. } => {
                path.as_str().starts_with("geometry.crop") || path.as_str() == "geometry"
            }
            ParamOp::ReplaceAll { .. } => true,
            _ => false,
        });
        if touches_crop {
            self.crop_anchor = None;
            return cmd;
        }
        let Ok(next) = self.doc.preview(&cmd.ops) else {
            return cmd;
        };
        let head = self.head().geometry.clone();
        let g = &next.geometry;
        if g.scalars == head.scalars && g.orientation_delta == head.orientation_delta {
            return cmd;
        }
        let anchor = match &self.crop_anchor {
            Some((gesture, c)) if gesture.is_some() && *gesture == cmd.gesture => c.clone(),
            _ => head.crop.clone(),
        };
        self.crop_anchor = Some((cmd.gesture, anchor.clone()));
        let Some(size) = self.oriented_size(g) else {
            return cmd;
        };
        let f = Frame::new(size, g);
        let fitted = f.crop_of(f.shrink_to_fit(f.rect_of(&anchor)));
        if fitted != g.crop {
            cmd.ops.extend(crop_ops(&fitted));
        }
        cmd
    }

    /// Crop-space extent of the current head, in source px.
    pub fn crop_extent(&self) -> Option<(f64, f64)> {
        let info = self.info()?;
        let g = &self.head().geometry;
        let size = Orientation::from_delta(g.orientation_delta).apply_size(info.source_size);
        Some(GeometryMap::new(size, g).crop_extent())
    }

    pub fn new_gesture(&mut self) -> GestureId {
        self.next_gesture += 1;
        GestureId(self.next_gesture)
    }

    pub fn edit(
        &mut self,
        cmd: EditCommand,
        hint: Interaction,
        cx: &mut Context<Self>,
    ) -> Result<(), CommandError> {
        let cmd = self.constrain_crop(cmd);
        let before = self.doc.rev();
        let r = self.doc.apply(cmd, hint);
        if self.doc.rev() != before {
            cx.emit(PhotoEvent::Edited);
            cx.notify();
        }
        r.map(|_| ())
    }

    pub fn undo(&mut self, cx: &mut Context<Self>) {
        if self.doc.undo().is_some() {
            cx.emit(PhotoEvent::Edited);
            cx.notify();
        }
    }

    pub fn redo(&mut self, cx: &mut Context<Self>) {
        if self.doc.redo().is_some() {
            cx.emit(PhotoEvent::Edited);
            cx.notify();
        }
    }

    /// Jump to the state after `position` history entries.
    pub fn goto(&mut self, position: usize, cx: &mut Context<Self>) {
        if self.doc.goto(position) {
            cx.emit(PhotoEvent::Edited);
            cx.notify();
        }
    }

    /// Save the head as a new snapshot.
    pub fn add_snapshot(&mut self, cx: &mut Context<Self>) {
        let name = snapshot_name(&self.snapshots, &crate::i18n::t("快照"));
        self.snapshots.push(Snapshot {
            name,
            params: (**self.head()).clone(),
        });
        cx.emit(PhotoEvent::SnapshotsChanged);
        cx.notify();
    }

    pub fn remove_snapshot(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.snapshots.len() {
            self.snapshots.remove(index);
            cx.emit(PhotoEvent::SnapshotsChanged);
            cx.notify();
        }
    }

    /// Make snapshot `index` the head, as one undoable command.
    pub fn apply_snapshot(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(s) = self.snapshots.get(index) else {
            return;
        };
        let label = format!("{} · {}", crate::i18n::t("快照"), s.name);
        let ops = vec![ParamOp::ReplaceAll {
            params: Box::new(s.params.clone()),
        }];
        self.command(label, ops, None, Interaction::Final, cx);
    }

    pub fn set_wb_pick(&mut self, on: bool, cx: &mut Context<Self>) {
        let on = on && self.info().is_some();
        if self.wb_pick != on {
            self.wb_pick = on;
            cx.notify();
        }
    }

    /// Set the white balance so that the disc of `radius` around `at` (Crop space) becomes
    /// neutral; ends picking.
    pub fn pick_wb(&mut self, at: ImagePoint, radius: f32, cx: &mut Context<Self>) {
        self.set_wb_pick(false, cx);
        let job = self.session.neutral_white_balance(at, radius);
        self.pick_job = Some(cx.spawn(async move |this, cx| {
            let r = job.await;
            let _ = this.update(cx, |p, cx| {
                let wb = match r {
                    Ok(wb) => wb,
                    Err(_) => {
                        cx.emit(PhotoEvent::Notice(
                            crate::i18n::t("吸管：所选位置过曝、过暗或不在图像内").into(),
                        ));
                        return;
                    }
                };
                let cmd = EditCommand::user(
                    p.doc.rev(),
                    crate::i18n::t("白平衡 · 吸管"),
                    vec![
                        ParamOp::set("wb.temperature", wb.temperature),
                        ParamOp::set("wb.tint", wb.tint),
                        ParamOp::set("wb.mode", "custom"),
                    ],
                );
                if let Err(e) = p.edit(cmd, Interaction::Final, cx) {
                    cx.emit(PhotoEvent::Notice(e.to_string()));
                } else if wb.low_confidence_reason.is_some() {
                    cx.emit(PhotoEvent::Notice(
                        crate::i18n::t("吸管：所选颜色超出白平衡范围，已取最接近的值").into(),
                    ));
                }
            });
        }));
    }
}

/// `"{base} N"` with the smallest N ≥ 1 not used by an existing snapshot.
fn snapshot_name(existing: &[Snapshot], base: &str) -> String {
    (1..)
        .map(|n| format!("{base} {n}"))
        .find(|name| existing.iter().all(|s| &s.name != name))
        .expect("unbounded range")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_names_fill_gaps() {
        let snap = |name: &str| Snapshot {
            name: name.into(),
            params: EditParams::default(),
        };
        assert_eq!(snapshot_name(&[], "S"), "S 1");
        assert_eq!(snapshot_name(&[snap("S 1"), snap("S 3")], "S"), "S 2");
        assert_eq!(snapshot_name(&[snap("S 2")], "S"), "S 1");
    }
}
