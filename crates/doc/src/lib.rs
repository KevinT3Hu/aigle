//! Per-photo document layer (doc 01 §5).
//!
//! A [`PhotoDoc`] owns one photo's committed edit state: the head `EditParams`, its
//! revision and the in-memory undo history. Every writer (GPUI panels, the agent, presets)
//! changes edits only through [`EditCommand`]s; the document validates them, merges them
//! field by field, records history and pushes the new head to the engine with
//! `Session::set_edits`.
//!
//! * **Merging.** Ops address parameter paths. An op is rejected if its path (or a parent or
//!   child of it) changed after the command's `base_rev`; the error carries the current
//!   values so the writer can retry.
//! * **Gestures.** Commands that share a [`GestureId`] (one slider drag) coalesce into a
//!   single history entry.
//! * **Undo/redo** restore whole snapshots and always produce a new revision, so the
//!   engine's latest-wins ordering by revision keeps working.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use aigle_core::masks::LocalAdjustment;
use aigle_core::params::{ClampReport, ParamsError};
use aigle_core::{EditParams, MaskId, ParamPath, Revision};
use aigle_engine_api::{Interaction, Session};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Who issued a command; shown in the history.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Origin {
    User,
    Agent { run_id: u64, step: u32 },
    Preset,
    Profile,
    ExternalReload,
}

/// One change to the params.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum ParamOp {
    /// Scalar paths (`light.exposure`) take a number; any other path addresses the JSON form
    /// of `EditParams` (`wb.mode` = `"custom"`).
    Set {
        path: ParamPath,
        value: Value,
    },
    /// Relative change of a scalar.
    Add {
        path: ParamPath,
        delta: f64,
    },
    /// Back to the default.
    Reset {
        path: ParamPath,
    },
    AddMask {
        mask: LocalAdjustment,
    },
    RemoveMask {
        id: MaskId,
    },
    /// Replace every edit (applying a profile in the GUI, external reload; doc 01 §5).
    ReplaceAll {
        params: Box<EditParams>,
    },
}

impl ParamOp {
    pub fn set(path: &str, value: impl Into<Value>) -> ParamOp {
        ParamOp::Set {
            path: path.into(),
            value: value.into(),
        }
    }

    /// The path this op touches; `""` stands for everything.
    fn path(&self) -> &str {
        match self {
            ParamOp::Set { path, .. } | ParamOp::Add { path, .. } | ParamOp::Reset { path } => {
                path.as_str()
            }
            ParamOp::AddMask { .. } | ParamOp::RemoveMask { .. } => "masks",
            ParamOp::ReplaceAll { .. } => "",
        }
    }
}

/// Commands of one continuous gesture share an id and become one history entry.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct GestureId(pub u64);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EditCommand {
    /// The revision the writer last saw.
    pub base_rev: Revision,
    pub ops: Vec<ParamOp>,
    pub origin: Origin,
    /// Shown in the history.
    pub label: String,
    pub gesture: Option<GestureId>,
}

impl EditCommand {
    pub fn user(base_rev: Revision, label: impl Into<String>, ops: Vec<ParamOp>) -> Self {
        EditCommand {
            base_rev,
            ops,
            origin: Origin::User,
            label: label.into(),
            gesture: None,
        }
    }

    pub fn in_gesture(mut self, g: GestureId) -> Self {
        self.gesture = Some(g);
        self
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Applied {
    pub rev: Revision,
    /// Values that were out of range and clamped.
    pub clamped: Vec<ClampReport>,
}

#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum CommandError {
    /// Paths changed after `base_rev`, with their current values.
    #[error("edited concurrently: {}", .current.iter().map(|(p, _)| p.as_str()).collect::<Vec<_>>().join(", "))]
    Conflict {
        rev: Revision,
        current: Vec<(String, Value)>,
    },
    #[error(transparent)]
    Params(#[from] ParamsError),
    #[error("cannot set `{path}`: {reason}")]
    Invalid { path: String, reason: String },
    #[error("no mask with id {0:?}")]
    UnknownMask(MaskId),
}

struct Entry {
    label: String,
    origin: Origin,
    before: Arc<EditParams>,
    after: Arc<EditParams>,
    paths: BTreeSet<String>,
    gesture: Option<GestureId>,
}

/// One history entry as shown in the history panel.
#[derive(Clone, Debug, PartialEq)]
pub struct HistoryItem {
    pub label: String,
    pub origin: Origin,
    /// Part of the head; `false` for undone entries that can still be redone.
    pub applied: bool,
}

/// Edit state of one open photo.
pub struct PhotoDoc {
    head: Arc<EditParams>,
    rev: Revision,
    changed_at: HashMap<String, Revision>,
    undo: Vec<Entry>,
    redo: Vec<Entry>,
    session: Option<Session>,
}

impl PhotoDoc {
    pub fn new(mut initial: EditParams) -> Result<PhotoDoc, ParamsError> {
        initial.sanitize()?;
        Ok(PhotoDoc {
            head: Arc::new(initial),
            rev: Revision(1),
            changed_at: HashMap::new(),
            undo: Vec::new(),
            redo: Vec::new(),
            session: None,
        })
    }

    pub fn head(&self) -> &Arc<EditParams> {
        &self.head
    }

    pub fn rev(&self) -> Revision {
        self.rev
    }

    /// Mirror the head into `session` from now on, starting with the current head.
    pub fn attach(&mut self, session: Session) {
        session.set_edits(self.rev, self.head.clone(), Interaction::Final);
        self.session = Some(session);
    }

    pub fn session(&self) -> Option<&Session> {
        self.session.as_ref()
    }

    pub fn detach(&mut self) -> Option<Session> {
        self.session.take()
    }

    /// Validate, merge and commit `cmd`. `hint` is passed to the engine: `Continuous` during
    /// a gesture, `Final` when it ends or for discrete changes.
    pub fn apply(&mut self, cmd: EditCommand, hint: Interaction) -> Result<Applied, CommandError> {
        self.check_conflicts(&cmd)?;
        let mut next = (*self.head).clone();
        let mut paths = BTreeSet::new();
        let mut clamped = Vec::new();
        for op in &cmd.ops {
            if let Some(r) = apply_op(&mut next, op)? {
                clamped.push(r);
            }
            paths.insert(op.path().to_string());
        }
        clamped.extend(next.sanitize()?);

        if next == *self.head {
            if hint == Interaction::Final {
                self.push(hint);
            }
            return Ok(Applied {
                rev: self.rev,
                clamped,
            });
        }

        let after = Arc::new(next);
        let merge = cmd.gesture.is_some()
            && self.redo.is_empty()
            && self.undo.last().is_some_and(|e| e.gesture == cmd.gesture);
        if merge {
            let e = self.undo.last_mut().expect("checked above");
            e.after = after.clone();
            e.paths.extend(paths.iter().cloned());
        } else {
            self.undo.push(Entry {
                label: cmd.label,
                origin: cmd.origin,
                before: self.head.clone(),
                after: after.clone(),
                paths: paths.clone(),
                gesture: cmd.gesture,
            });
            self.redo.clear();
        }
        self.commit(after, paths.iter().map(String::as_str), hint);
        Ok(Applied {
            rev: self.rev,
            clamped,
        })
    }

    /// The params `ops` would produce on the head, without committing (clamped and rounded
    /// like a commit).
    pub fn preview(&self, ops: &[ParamOp]) -> Result<EditParams, CommandError> {
        let mut next = (*self.head).clone();
        for op in ops {
            apply_op(&mut next, op)?;
        }
        next.sanitize()?;
        Ok(next)
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Undo the last entry; returns its label.
    pub fn undo(&mut self) -> Option<String> {
        let e = self.undo.pop()?;
        let label = e.label.clone();
        let paths: Vec<String> = e.paths.iter().cloned().collect();
        self.commit(
            e.before.clone(),
            paths.iter().map(String::as_str),
            Interaction::Final,
        );
        self.redo.push(e);
        Some(label)
    }

    /// Redo the last undone entry; returns its label.
    pub fn redo(&mut self) -> Option<String> {
        let mut e = self.redo.pop()?;
        let label = e.label.clone();
        let paths: Vec<String> = e.paths.iter().cloned().collect();
        self.commit(
            e.after.clone(),
            paths.iter().map(String::as_str),
            Interaction::Final,
        );
        e.gesture = None;
        self.undo.push(e);
        Some(label)
    }

    /// Every entry, oldest first: the applied ones, then the undone ones in redo order.
    pub fn history(&self) -> Vec<HistoryItem> {
        let item = |e: &Entry, applied| HistoryItem {
            label: e.label.clone(),
            origin: e.origin.clone(),
            applied,
        };
        self.undo
            .iter()
            .map(|e| item(e, true))
            .chain(self.redo.iter().rev().map(|e| item(e, false)))
            .collect()
    }

    /// Number of applied entries (0 = the photo as opened).
    pub fn position(&self) -> usize {
        self.undo.len()
    }

    /// Undo or redo until `position` entries are applied, as one revision. Returns whether
    /// the head moved.
    pub fn goto(&mut self, position: usize) -> bool {
        let target = position.min(self.undo.len() + self.redo.len());
        if target == self.undo.len() {
            return false;
        }
        let mut paths = BTreeSet::new();
        while self.undo.len() > target {
            let e = self.undo.pop().expect("len checked");
            paths.extend(e.paths.iter().cloned());
            self.redo.push(e);
        }
        while self.undo.len() < target {
            let mut e = self.redo.pop().expect("target is within history");
            paths.extend(e.paths.iter().cloned());
            e.gesture = None;
            self.undo.push(e);
        }
        let head = match self.undo.last() {
            Some(e) => e.after.clone(),
            None => self
                .redo
                .last()
                .expect("history is not empty")
                .before
                .clone(),
        };
        self.commit(head, paths.iter().map(String::as_str), Interaction::Final);
        true
    }

    fn commit<'a>(
        &mut self,
        head: Arc<EditParams>,
        paths: impl Iterator<Item = &'a str>,
        hint: Interaction,
    ) {
        self.rev = self.rev.next();
        for p in paths {
            self.changed_at.insert(p.to_string(), self.rev);
        }
        self.head = head;
        self.push(hint);
    }

    fn push(&self, hint: Interaction) {
        if let Some(s) = &self.session {
            s.set_edits(self.rev, self.head.clone(), hint);
        }
    }

    fn check_conflicts(&self, cmd: &EditCommand) -> Result<(), CommandError> {
        if cmd.base_rev >= self.rev {
            return Ok(());
        }
        let mut current = Vec::new();
        for op in &cmd.ops {
            let p = op.path();
            let hit = self
                .changed_at
                .iter()
                .any(|(k, r)| *r > cmd.base_rev && overlaps(k, p));
            if hit && !current.iter().any(|(q, _)| q == p) {
                current.push((p.to_string(), value_at(&self.head, p)));
            }
        }
        if current.is_empty() {
            Ok(())
        } else {
            Err(CommandError::Conflict {
                rev: self.rev,
                current,
            })
        }
    }
}

/// `a` and `b` name the same field, or one contains the other.
fn overlaps(a: &str, b: &str) -> bool {
    let within = |inner: &str, outer: &str| {
        outer.is_empty()
            || inner == outer
            || (inner.starts_with(outer) && inner.as_bytes().get(outer.len()) == Some(&b'.'))
    };
    within(a, b) || within(b, a)
}

fn json_pointer(path: &str) -> String {
    if path.is_empty() {
        String::new()
    } else {
        format!("/{}", path.replace('.', "/"))
    }
}

/// Current value at `path` (scalar path or JSON path).
pub fn value_at(params: &EditParams, path: &str) -> Value {
    if let Some(v) = params.get(path) {
        return Value::from(v);
    }
    serde_json::to_value(params)
        .ok()
        .and_then(|j| j.pointer(&json_pointer(path)).cloned())
        .unwrap_or(Value::Null)
}

/// Apply one op; returns a report if a scalar was clamped.
fn apply_op(p: &mut EditParams, op: &ParamOp) -> Result<Option<ClampReport>, CommandError> {
    match op {
        ParamOp::Set { path, value } => {
            if p.spec(path.as_str()).is_some() {
                let v = value.as_f64().ok_or_else(|| CommandError::Invalid {
                    path: path.to_string(),
                    reason: "expected a number".into(),
                })?;
                Ok(p.set(path.as_str(), v as f32)?)
            } else {
                set_json(p, path.as_str(), value.clone()).map(|_| None)
            }
        }
        ParamOp::Add { path, delta } => {
            let v = p
                .get(path.as_str())
                .ok_or_else(|| ParamsError::UnknownPath(path.to_string()))?;
            Ok(p.set(path.as_str(), (v as f64 + delta) as f32)?)
        }
        ParamOp::Reset { path } => {
            if p.spec(path.as_str()).is_some() {
                p.reset(path.as_str())?;
                Ok(None)
            } else {
                let d = value_at(&EditParams::default(), path.as_str());
                set_json(p, path.as_str(), d).map(|_| None)
            }
        }
        ParamOp::AddMask { mask } => {
            if p.masks.iter().any(|m| m.id == mask.id) {
                return Err(CommandError::Invalid {
                    path: "masks".into(),
                    reason: format!("mask id {:?} already exists", mask.id),
                });
            }
            p.masks.push(mask.clone());
            Ok(None)
        }
        ParamOp::RemoveMask { id } => {
            let n = p.masks.len();
            p.masks.retain(|m| m.id != *id);
            if p.masks.len() == n {
                Err(CommandError::UnknownMask(*id))
            } else {
                Ok(None)
            }
        }
        ParamOp::ReplaceAll { params } => {
            *p = (**params).clone();
            Ok(None)
        }
    }
}

fn set_json(p: &mut EditParams, path: &str, value: Value) -> Result<(), CommandError> {
    let invalid = |reason: String| CommandError::Invalid {
        path: path.to_string(),
        reason,
    };
    let mut j = serde_json::to_value(&*p).map_err(|e| invalid(e.to_string()))?;
    let pointer = json_pointer(path);
    let added = j.pointer(&pointer).is_none();
    let slot = match j.pointer_mut(&pointer) {
        Some(s) => Some(s),
        // An optional field that is unset is not serialised: add it to its parent object.
        None => pointer.rsplit_once('/').and_then(|(parent, key)| {
            j.pointer_mut(parent)
                .and_then(Value::as_object_mut)
                .map(|o| o.entry(key.to_string()).or_insert(Value::Null))
        }),
    }
    .ok_or_else(|| CommandError::Params(ParamsError::UnknownPath(path.to_string())))?;
    *slot = value;
    let set = slot.clone();
    let next: EditParams = serde_json::from_value(j).map_err(|e| invalid(e.to_string()))?;
    // A key added for an unset optional field must exist in the type, or it was dropped.
    if added && !set.is_null() {
        let back = serde_json::to_value(&next).map_err(|e| invalid(e.to_string()))?;
        if back.pointer(&pointer).is_none() {
            return Err(CommandError::Params(ParamsError::UnknownPath(
                path.to_string(),
            )));
        }
    }
    *p = next;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use aigle_core::params::WbMode;

    fn doc() -> PhotoDoc {
        PhotoDoc::new(EditParams::default()).unwrap()
    }

    #[test]
    fn json_paths_reach_unset_optional_fields() {
        use aigle_core::masks::{LocalAdjustment, LocalParams, MaskShape, Pt};
        let mut d = doc();
        let mask = LocalAdjustment {
            id: MaskId(1),
            name: "m".into(),
            shape: MaskShape::Linear {
                from: Pt::default(),
                to: Pt { x: 0.0, y: 1.0 },
            },
            invert: false,
            opacity: 1.0,
            adjust: LocalParams::default(),
        };
        let set = |d: &mut PhotoDoc, path: &str, v: Value| {
            let cmd = EditCommand::user(
                d.rev(),
                "x",
                vec![ParamOp::Set {
                    path: path.into(),
                    value: v,
                }],
            );
            d.apply(cmd, Interaction::Final)
        };
        let cmd = EditCommand::user(d.rev(), "add", vec![ParamOp::AddMask { mask }]);
        d.apply(cmd, Interaction::Final).unwrap();
        set(&mut d, "masks.0.adjust.exposure", Value::from(1.5)).unwrap();
        assert_eq!(d.head().masks[0].adjust.exposure, Some(1.5));
        set(&mut d, "masks.0.adjust.exposure", Value::Null).unwrap();
        assert_eq!(d.head().masks[0].adjust.exposure, None);
        assert!(matches!(
            set(&mut d, "masks.0.adjust.exposre", Value::from(1.0)),
            Err(CommandError::Params(ParamsError::UnknownPath(_)))
        ));
    }

    #[test]
    fn set_add_reset_and_json_paths() {
        let mut d = doc();
        let r0 = d.rev();
        let a = d
            .apply(
                EditCommand::user(
                    r0,
                    "Exposure",
                    vec![
                        ParamOp::set("light.exposure", 0.5),
                        ParamOp::Add {
                            path: "light.contrast".into(),
                            delta: 250.0,
                        },
                        ParamOp::set("wb.mode", "custom"),
                    ],
                ),
                Interaction::Final,
            )
            .unwrap();
        assert!(a.rev > r0);
        assert_eq!(a.clamped.len(), 1);
        assert_eq!(d.head().light.exposure, 0.5);
        assert_eq!(d.head().light.contrast, 100.0);
        assert_eq!(d.head().wb.mode, WbMode::Custom);
        assert_eq!(value_at(d.head(), "grading.blending"), Value::from(50.0));

        d.apply(
            EditCommand::user(
                d.rev(),
                "Reset",
                vec![
                    ParamOp::Reset {
                        path: "light.exposure".into(),
                    },
                    ParamOp::Reset {
                        path: "wb.mode".into(),
                    },
                ],
            ),
            Interaction::Final,
        )
        .unwrap();
        assert_eq!(d.head().light.exposure, 0.0);
        assert_eq!(d.head().wb.mode, WbMode::AsShot);

        let bad = d.apply(
            EditCommand::user(d.rev(), "x", vec![ParamOp::set("wb.mode", "purple")]),
            Interaction::Final,
        );
        assert!(matches!(bad, Err(CommandError::Invalid { .. })));
        let bad = d.apply(
            EditCommand::user(d.rev(), "x", vec![ParamOp::set("light.nope", 1.0)]),
            Interaction::Final,
        );
        assert!(matches!(bad, Err(CommandError::Params(_))));
    }

    #[test]
    fn conflicts_are_field_level() {
        let mut d = doc();
        let base = d.rev();
        d.apply(
            EditCommand::user(base, "a", vec![ParamOp::set("light.exposure", 1.0)]),
            Interaction::Final,
        )
        .unwrap();
        // A stale writer touching another field merges.
        d.apply(
            EditCommand::user(base, "b", vec![ParamOp::set("light.contrast", 10.0)]),
            Interaction::Final,
        )
        .unwrap();
        // The same field is rejected with the current value.
        let e = d
            .apply(
                EditCommand::user(base, "c", vec![ParamOp::set("light.exposure", -1.0)]),
                Interaction::Final,
            )
            .unwrap_err();
        let CommandError::Conflict { current, .. } = e else {
            panic!("{e:?}")
        };
        assert_eq!(current, vec![("light.exposure".into(), Value::from(1.0))]);
        // So is a parent path.
        assert!(
            d.apply(
                EditCommand::user(
                    base,
                    "d",
                    vec![ParamOp::Reset {
                        path: "light".into()
                    }]
                ),
                Interaction::Final
            )
            .is_err()
        );
        assert!(overlaps("light.exposure", "light"));
        assert!(!overlaps("light.exposure", "light.exp"));
        assert!(overlaps("anything", ""));
    }

    #[test]
    fn gestures_coalesce_and_undo_redo() {
        let mut d = doc();
        let g = GestureId(7);
        for v in [0.1, 0.2, 0.3] {
            d.apply(
                EditCommand::user(d.rev(), "Exposure", vec![ParamOp::set("light.exposure", v)])
                    .in_gesture(g),
                Interaction::Continuous,
            )
            .unwrap();
        }
        d.apply(
            EditCommand::user(
                d.rev(),
                "Contrast",
                vec![ParamOp::set("light.contrast", 5.0)],
            ),
            Interaction::Final,
        )
        .unwrap();
        assert_eq!(d.history().len(), 2);

        assert_eq!(d.undo().as_deref(), Some("Contrast"));
        assert_eq!(d.head().light.contrast, 0.0);
        assert_eq!(d.undo().as_deref(), Some("Exposure"));
        assert_eq!(d.head().light.exposure, 0.0);
        assert!(!d.can_undo());
        let before = d.rev();
        assert_eq!(d.redo().as_deref(), Some("Exposure"));
        assert!(d.rev() > before);
        assert_eq!(d.head().light.exposure, 0.3);

        // A new edit drops the redo stack; a redone entry does not absorb a later gesture.
        d.apply(
            EditCommand::user(
                d.rev(),
                "Exposure",
                vec![ParamOp::set("light.exposure", 1.0)],
            )
            .in_gesture(g),
            Interaction::Final,
        )
        .unwrap();
        assert!(!d.can_redo());
        assert_eq!(d.history().len(), 2);

        // No-op commands leave history and revision alone.
        let r = d.rev();
        d.apply(
            EditCommand::user(r, "same", vec![ParamOp::set("light.exposure", 1.0)]),
            Interaction::Final,
        )
        .unwrap();
        assert_eq!((d.rev(), d.history().len()), (r, 2));
    }

    #[test]
    fn goto_jumps_through_history_in_one_revision() {
        let mut d = doc();
        for (i, v) in [0.5, 1.0, 1.5].into_iter().enumerate() {
            d.apply(
                EditCommand::user(
                    d.rev(),
                    format!("e{i}"),
                    vec![ParamOp::set("light.exposure", v)],
                ),
                Interaction::Final,
            )
            .unwrap();
        }
        let r = d.rev();
        assert!(d.goto(0));
        assert_eq!(d.rev(), r.next());
        assert_eq!(d.head().light.exposure, 0.0);
        let h = d.history();
        assert_eq!(h.len(), 3);
        assert!(h.iter().all(|e| !e.applied));
        assert_eq!(h[0].label, "e0");

        assert!(d.goto(2));
        assert_eq!(d.head().light.exposure, 1.0);
        assert_eq!(d.position(), 2);
        assert_eq!(
            d.history().iter().map(|e| e.applied).collect::<Vec<_>>(),
            [true, true, false]
        );
        assert!(!d.goto(2));
        assert!(d.goto(99));
        assert_eq!(d.head().light.exposure, 1.5);
        // A stale writer that saw the old head of `light.exposure` is rejected.
        assert!(
            d.apply(
                EditCommand::user(r, "x", vec![ParamOp::set("light.exposure", 3.0)]),
                Interaction::Final
            )
            .is_err()
        );
    }

    #[test]
    fn masks_and_replace_all() {
        use aigle_core::masks::{MaskShape, Pt};
        let mut d = doc();
        let mask = LocalAdjustment {
            id: MaskId(1),
            name: "sky".into(),
            shape: MaskShape::Linear {
                from: Pt { x: 0.5, y: 0.0 },
                to: Pt { x: 0.5, y: 0.5 },
            },
            invert: false,
            opacity: 1.0,
            adjust: Default::default(),
        };
        d.apply(
            EditCommand::user(
                d.rev(),
                "Mask",
                vec![ParamOp::AddMask { mask: mask.clone() }],
            ),
            Interaction::Final,
        )
        .unwrap();
        assert!(
            d.apply(
                EditCommand::user(d.rev(), "Mask", vec![ParamOp::AddMask { mask }]),
                Interaction::Final
            )
            .is_err()
        );
        assert!(matches!(
            d.apply(
                EditCommand::user(d.rev(), "x", vec![ParamOp::RemoveMask { id: MaskId(9) }]),
                Interaction::Final
            ),
            Err(CommandError::UnknownMask(_))
        ));
        let mut p = EditParams::default();
        p.light.exposure = 2.0;
        d.apply(
            EditCommand::user(
                d.rev(),
                "Profile",
                vec![ParamOp::ReplaceAll {
                    params: Box::new(p.clone()),
                }],
            ),
            Interaction::Final,
        )
        .unwrap();
        assert_eq!(**d.head(), p);
        d.undo();
        assert_eq!(d.head().masks.len(), 1);
    }
}
