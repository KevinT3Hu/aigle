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
//!   single history entry. Consecutive user gestures on the same adjustment also share
//!   an undo step, until another edit or history navigation interrupts them.
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
    adjustment: Option<String>,
}

/// One history entry as shown in the history panel.
#[derive(Clone, Debug, PartialEq)]
pub struct HistoryItem {
    pub label: String,
    pub origin: Origin,
    /// Part of the head; `false` for undone entries that can still be redone.
    pub applied: bool,
}

/// Entries kept when history is saved; older applied entries are dropped first.
pub const MAX_SAVED_ENTRIES: usize = 200;

/// Label of the entry that reconciles saved history with edits changed elsewhere.
pub const EXTERNAL_LABEL: &str = "外部修改";

/// A document's history in storable form.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedHistory {
    /// `states[0]` is the state before the first entry, `states[i]` the one after entry `i`.
    pub states: Vec<EditParams>,
    pub entries: Vec<SavedEntry>,
    /// Number of applied entries.
    pub position: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedEntry {
    pub label: String,
    pub origin: Origin,
    pub paths: BTreeSet<String>,
}

impl SavedHistory {
    fn valid(&self) -> bool {
        self.states.len() == self.entries.len() + 1 && self.position <= self.entries.len()
    }

    /// The head the history ends at.
    pub fn head(&self) -> Option<&EditParams> {
        self.valid().then(|| &self.states[self.position])
    }

    /// Record a change to `after` made without opening the photo: undone entries are
    /// dropped, like a new edit would.
    pub fn push(&mut self, label: impl Into<String>, origin: Origin, after: EditParams) {
        if !self.valid() {
            return;
        }
        self.entries.truncate(self.position);
        self.states.truncate(self.position + 1);
        let before = &self.states[self.position];
        if *before == after {
            return;
        }
        self.entries.push(SavedEntry {
            label: label.into(),
            origin,
            paths: changed_fields(before, &after),
        });
        self.states.push(after);
        self.position += 1;
        self.trim();
    }

    /// History starting from `initial` with no entries.
    pub fn new(initial: EditParams) -> SavedHistory {
        SavedHistory {
            states: vec![initial],
            entries: Vec::new(),
            position: 0,
        }
    }

    fn trim(&mut self) {
        let drop = self
            .entries
            .len()
            .saturating_sub(MAX_SAVED_ENTRIES)
            .min(self.position);
        if drop > 0 {
            self.entries.drain(..drop);
            self.states.drain(..drop);
            self.position -= drop;
        }
    }
}

/// Top-level `EditParams` fields that differ between `a` and `b`.
fn changed_fields(a: &EditParams, b: &EditParams) -> BTreeSet<String> {
    let (Ok(Value::Object(a)), Ok(Value::Object(b))) =
        (serde_json::to_value(a), serde_json::to_value(b))
    else {
        return [String::new()].into_iter().collect();
    };
    a.keys()
        .chain(b.keys())
        .filter(|k| a.get(*k) != b.get(*k))
        .cloned()
        .collect()
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

    /// Open with `initial` (the stored edits) and the history saved for it. When the saved
    /// history ends elsewhere, the edits changed outside this document: undone entries are
    /// dropped and an [`Origin::ExternalReload`] entry leads to `initial`. Invalid history
    /// is ignored.
    pub fn restore(initial: EditParams, saved: SavedHistory) -> Result<PhotoDoc, ParamsError> {
        let mut doc = PhotoDoc::new(initial)?;
        if !saved.valid() {
            return Ok(doc);
        }
        let SavedHistory {
            states,
            mut entries,
            position,
        } = saved;
        let mut states: Vec<Arc<EditParams>> = states
            .into_iter()
            .map(|mut s| s.sanitize().map(|_| Arc::new(s)))
            .collect::<Result<_, _>>()
            .unwrap_or_default();
        if states.is_empty() {
            return Ok(doc);
        }
        let mut position = position;
        if *states[position] != *doc.head {
            entries.truncate(position);
            states.truncate(position + 1);
            entries.push(SavedEntry {
                label: EXTERNAL_LABEL.into(),
                origin: Origin::ExternalReload,
                paths: changed_fields(&states[position], &doc.head),
            });
            states.push(doc.head.clone());
            position += 1;
        }
        let mut all: Vec<Entry> = entries
            .into_iter()
            .enumerate()
            .map(|(i, e)| Entry {
                label: e.label,
                origin: e.origin,
                before: states[i].clone(),
                after: states[i + 1].clone(),
                paths: e.paths,
                gesture: None,
                adjustment: None,
            })
            .collect();
        doc.redo = all.split_off(position);
        doc.redo.reverse();
        doc.undo = all;
        doc.head = states[position].clone();
        Ok(doc)
    }

    /// The history in storable form, trimmed to [`MAX_SAVED_ENTRIES`].
    pub fn saved_history(&self) -> SavedHistory {
        let ordered: Vec<&Entry> = self.undo.iter().chain(self.redo.iter().rev()).collect();
        let first = ordered.first().map_or(&self.head, |e| &e.before);
        let mut saved = SavedHistory {
            states: std::iter::once((**first).clone())
                .chain(ordered.iter().map(|e| (*e.after).clone()))
                .collect(),
            entries: ordered
                .iter()
                .map(|e| SavedEntry {
                    label: e.label.clone(),
                    origin: e.origin.clone(),
                    paths: e.paths.clone(),
                })
                .collect(),
            position: self.undo.len(),
        };
        saved.trim();
        saved
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

        // A control's first op identifies the item (white-balance sliders also write
        // companion fields). Only interactive scalar/shape edits opt into cross-gesture
        // grouping; mask insertion/removal and discrete commands remain separate.
        let adjustment = if cmd.origin == Origin::User && cmd.gesture.is_some() {
            match cmd.ops.first() {
                Some(ParamOp::Set { path, .. } | ParamOp::Add { path, .. }) => {
                    Some(path.as_str().to_owned())
                }
                _ => None,
            }
        } else {
            None
        };
        let after = Arc::new(next);
        let merge = self.redo.is_empty()
            && self.undo.last().is_some_and(|e| {
                e.origin == cmd.origin
                    && ((cmd.gesture.is_some() && e.gesture == cmd.gesture)
                        || (adjustment.is_some() && e.adjustment == adjustment && e.paths == paths))
            });
        if merge {
            let e = self.undo.last_mut().expect("checked above");
            e.after = after.clone();
            e.label = cmd.label;
            e.gesture = cmd.gesture;
            e.paths.extend(paths.iter().cloned());
        } else {
            self.undo.push(Entry {
                label: cmd.label,
                origin: cmd.origin,
                before: self.head.clone(),
                after: after.clone(),
                paths: paths.clone(),
                gesture: cmd.gesture,
                adjustment,
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
        e.adjustment = None;
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
            e.adjustment = None;
            self.undo.push(e);
        }
        if let Some(e) = self.undo.last_mut() {
            e.gesture = None;
            e.adjustment = None;
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

    fn drag(d: &mut PhotoDoc, path: &str, value: f64, gesture: u64) {
        d.apply(
            EditCommand::user(d.rev(), path, vec![ParamOp::set(path, value)])
                .in_gesture(GestureId(gesture)),
            Interaction::Final,
        )
        .unwrap();
    }

    #[test]
    fn consecutive_adjustment_gestures_share_one_undo_step() {
        let mut d = doc();
        for (g, value) in [0.1, 0.5, 1.0].into_iter().enumerate() {
            drag(&mut d, "light.exposure", value, g as u64);
        }
        assert_eq!(d.history().len(), 1);
        assert_eq!(d.head().light.exposure, 1.0);
        let revision = d.rev();
        d.undo().unwrap();
        assert_eq!(d.head().light.exposure, 0.0);
        assert!(d.rev() > revision);
        d.redo().unwrap();
        assert_eq!(d.head().light.exposure, 1.0);
        // Redo closes the group, even when the same control is edited again.
        drag(&mut d, "light.exposure", 1.5, 99);
        assert_eq!(d.history().len(), 2);
        drag(&mut d, "light.contrast", 5.0, 100);
        drag(&mut d, "light.exposure", 2.0, 101);
        assert_eq!(d.history().len(), 4);
        d.undo();
        assert_eq!(d.head().light.exposure, 1.5);
        assert_eq!(d.head().light.contrast, 5.0);
        // An edit after undo branches instead of absorbing the earlier exposure entry.
        drag(&mut d, "light.contrast", 10.0, 102);
        assert_eq!(d.history().len(), 4);
        assert!(!d.can_redo());
        d.undo();
        assert_eq!(d.head().light.contrast, 5.0);
    }

    #[test]
    fn grouping_uses_the_primary_item_and_respects_origins_and_navigation() {
        let mut d = doc();
        for (g, temp) in [(1, 6000.0), (2, 6500.0)] {
            d.apply(
                EditCommand::user(
                    d.rev(),
                    "Temperature",
                    vec![
                        ParamOp::set("wb.temperature", temp),
                        ParamOp::set("wb.tint", 0.0),
                        ParamOp::set("wb.mode", "custom"),
                    ],
                )
                .in_gesture(GestureId(g)),
                Interaction::Final,
            )
            .unwrap();
        }
        assert_eq!(d.history().len(), 1);
        // Tint writes the same set of companion paths, but it is another control.
        d.apply(
            EditCommand::user(
                d.rev(),
                "Tint",
                vec![
                    ParamOp::set("wb.tint", 5.0),
                    ParamOp::set("wb.temperature", 6500.0),
                    ParamOp::set("wb.mode", "custom"),
                ],
            )
            .in_gesture(GestureId(3)),
            Interaction::Final,
        )
        .unwrap();
        assert_eq!(d.history().len(), 2);
        drag(&mut d, "light.exposure", 1.0, 4);
        assert!(d.goto(2));
        drag(&mut d, "wb.tint", 10.0, 5);
        assert_eq!(d.history().len(), 3);
        d.undo();
        assert_eq!(d.head().wb.values.tint, 5.0);
        d.redo();
        let mut agent =
            EditCommand::user(d.rev(), "Agent tint", vec![ParamOp::set("wb.tint", 15.0)])
                .in_gesture(GestureId(5));
        agent.origin = Origin::Agent { run_id: 1, step: 1 };
        d.apply(agent, Interaction::Final).unwrap();
        assert_eq!(d.history().len(), 4);
        // Discrete operations such as rotations or resets remain independent.
        for value in [20.0, 25.0] {
            d.apply(
                EditCommand::user(
                    d.rev(),
                    "Discrete tint",
                    vec![ParamOp::set("wb.tint", value)],
                ),
                Interaction::Final,
            )
            .unwrap();
        }
        assert_eq!(d.history().len(), 6);
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

    fn set(d: &mut PhotoDoc, label: &str, path: &str, v: f64) {
        d.apply(
            EditCommand::user(d.rev(), label, vec![ParamOp::set(path, v)]),
            Interaction::Final,
        )
        .unwrap();
    }

    #[test]
    fn saved_history_restores_entries_and_position() {
        let mut d = doc();
        set(&mut d, "Exposure", "light.exposure", 1.0);
        set(&mut d, "Contrast", "light.contrast", 10.0);
        set(&mut d, "Clarity", "presence.clarity", 20.0);
        d.undo();
        let saved: SavedHistory =
            serde_json::from_str(&serde_json::to_string(&d.saved_history()).unwrap()).unwrap();
        let mut r = PhotoDoc::restore((**d.head()).clone(), saved.clone()).unwrap();
        assert_eq!(r.history(), d.history());
        assert_eq!(r.position(), 2);
        r.redo();
        assert_eq!(r.head().presence.clarity, 20.0);
        r.goto(0);
        assert_eq!(**r.head(), EditParams::default());

        // Edits changed elsewhere: redo is dropped and an external entry leads to them.
        let mut changed = (**d.head()).clone();
        changed.light.exposure = -1.0;
        let r = PhotoDoc::restore(changed.clone(), saved).unwrap();
        let h = r.history();
        assert_eq!(h.len(), 3);
        assert_eq!(h[2].origin, Origin::ExternalReload);
        assert!(h.iter().all(|e| e.applied));
        assert_eq!(**r.head(), changed);
    }

    #[test]
    fn pushing_onto_saved_history() {
        let mut d = doc();
        set(&mut d, "Exposure", "light.exposure", 1.0);
        set(&mut d, "Contrast", "light.contrast", 10.0);
        d.undo();
        let mut saved = d.saved_history();
        let mut next = (**d.head()).clone();
        next.presence.clarity = 5.0;
        saved.push("Paste", Origin::User, next.clone());
        assert_eq!((saved.entries.len(), saved.position), (2, 2));
        assert_eq!(
            saved.entries[1].paths,
            ["presence".to_string()].into_iter().collect()
        );
        let r = PhotoDoc::restore(next.clone(), saved.clone()).unwrap();
        assert_eq!(r.history().len(), 2);
        assert_eq!(**r.head(), next);

        let mut long = SavedHistory::new(EditParams::default());
        for i in 0..MAX_SAVED_ENTRIES + 5 {
            let mut p = EditParams::default();
            p.light.exposure = (i + 1) as f32 * 0.01;
            long.push("x", Origin::User, p);
        }
        assert_eq!(long.entries.len(), MAX_SAVED_ENTRIES);
        assert!(long.head().is_some());
    }

    #[test]
    fn invalid_saved_history_is_ignored() {
        let bad = SavedHistory {
            states: vec![],
            entries: vec![],
            position: 3,
        };
        let r = PhotoDoc::restore(EditParams::default(), bad).unwrap();
        assert!(r.history().is_empty());
    }
}
