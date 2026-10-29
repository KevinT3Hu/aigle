//! Main window: title bar with the toolbar, then either the library grid or the develop view
//! (history sidebar, canvas, adjustment panel, filmstrip).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use aigle_core::EditParams;
use aigle_core::params::Aspect;
use aigle_doc::Origin;
use aigle_engine_api::{DisplayProfileId, Engine, SourceRef};
use aigle_library::{Catalog, PhotoKey, PhotoMeta};
use gpui::{
    App, Context, Entity, FocusHandle, IntoElement, ParentElement, PathPromptOptions, Render,
    SharedString, Styled, Subscription, Task, Window, div, prelude::*, px,
};
use gpui_component::button::{Button, ButtonGroup};
use gpui_component::menu::{DropdownMenu, PopupMenuItem};
use gpui_component::{ActiveTheme, Selectable, Sizable, TitleBar, WindowExt, h_flex, v_flex};

use crate::actions::*;
use crate::canvas::{Canvas, Compare};
use crate::copy_settings::{Clipboard, CopyDialog, CopyEvent, Group, Purpose};
use crate::export::{self, ExportDialog, ExportEvent, Request, Settings};
use crate::filmstrip::Filmstrip;
use crate::grid::Grid;
use crate::history::HistoryPanel;
use crate::keybinding_editor::{Closed as KeybindingsClosed, KeybindingEditor};
use crate::library::{CopyInfo, LABELS, Library, LibraryEvent};
use crate::metadata_panel::MetadataPanel;
use crate::panel::Panel;
use crate::photo::{Photo, PhotoEvent, Tool};
use crate::presets::{Preset, PresetsEvent, PresetsPanel};
use crate::rename::{RenameDialog, RenameEvent};
use crate::resources::Resources;

/// Width of the library's metadata panel.
const METADATA_W: f32 = 300.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Library,
    Develop,
}

struct OpenPhoto {
    photo: Entity<Photo>,
    canvas: Entity<Canvas>,
    panel: Entity<Panel>,
    history: Entity<HistoryPanel>,
    /// Pending save of the history to the catalog.
    history_save: Option<Task<()>>,
    _edits: Subscription,
}

const HISTORY_DELAY: std::time::Duration = std::time::Duration::from_secs(1);

fn set_display(mode: crate::display::Mode, window: &mut Window, cx: &mut App) {
    if let Err(error) = crate::display::set_mode(mode, cx) {
        window.push_notification(
            format!("{}: {error}", crate::i18n::t("display.save_failed")),
            cx,
        );
    }
}

struct ExportJob {
    id: u64,
    total: usize,
    done: usize,
    _task: Task<()>,
}

pub struct Workspace {
    engine: Engine,
    resources: Entity<Resources>,
    library: Entity<Library>,
    grid: Entity<Grid>,
    filmstrip: Entity<Filmstrip>,
    metadata: Entity<MetadataPanel>,
    /// The develop panel's Metadata tab: the open photo only.
    develop_metadata: Entity<MetadataPanel>,
    show_metadata: bool,
    rename_dialog: Option<(Entity<RenameDialog>, Subscription)>,
    mode: Mode,
    open: Option<OpenPhoto>,
    show_history: bool,
    focus: FocusHandle,
    export_settings: Settings,
    export_dialog: Option<(Entity<ExportDialog>, Subscription)>,
    keybinding_dialog: Option<(Entity<KeybindingEditor>, Subscription)>,
    copy_dialog: Option<(Entity<CopyDialog>, Subscription)>,
    /// Groups chosen the last time settings were copied.
    copy_groups: BTreeSet<Group>,
    clipboard: Option<Clipboard>,
    presets: Entity<PresetsPanel>,
    /// Running export batches.
    exports: Vec<ExportJob>,
    next_export: u64,
    _subs: Vec<Subscription>,
}

impl Workspace {
    /// `path`: a folder opens in the grid, a photo opens in develop with its folder in the
    /// filmstrip.
    pub fn new(
        engine: Engine,
        source: Option<SourceRef>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Workspace {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let (catalog, catalog_error) =
            match crate::data_dir().map(|d| Catalog::open(&d.join("catalog.sqlite3"))) {
                Some(Ok(c)) => (c, None),
                Some(Err(e)) => {
                    tracing::error!("{e}");
                    (Catalog::in_memory(), Some(e.to_string()))
                }
                None => (Catalog::in_memory(), Some("no data folder".to_string())),
            };
        let catalog = Arc::new(catalog);
        let library = cx.new(|cx| Library::new(engine.clone(), catalog, cx));
        let grid = cx.new(|cx| Grid::new(library.clone(), cx));
        let filmstrip = cx.new(|cx| Filmstrip::new(library.clone(), cx));
        let metadata = cx.new(|cx| MetadataPanel::new(library.clone(), false, window, cx));
        let develop_metadata = cx.new(|cx| MetadataPanel::new(library.clone(), true, window, cx));
        grid.update(cx, |g, _| g.side = METADATA_W);
        let (store, warnings) =
            crate::presets::Store::open(crate::config_dir().map(|d| d.join("presets")));
        for w in &warnings {
            tracing::warn!("preset {w}");
        }
        let presets = cx.new(|_| PresetsPanel::new(store));
        let subs = vec![
            cx.subscribe_in(&library, window, |w, _, e, window, cx| match e {
                LibraryEvent::Open(i) => w.open_index(*i, window, cx),
                LibraryEvent::Notice(m) => window.push_notification(m.clone(), cx),
            }),
            cx.subscribe_in(&presets, window, |w, _, e, window, cx| match e {
                PresetsEvent::Apply(i) => w.apply_preset(*i, window, cx),
                PresetsEvent::Create => w.open_new_preset(window, cx),
                PresetsEvent::Remove(i) => w.remove_preset(*i, window, cx),
            }),
            cx.observe(&library, |_, _, cx| cx.notify()),
            cx.on_app_quit(|w, cx| {
                w.save_history(true, cx);
                w.library.update(cx, |l, cx| l.flush(cx));
                async {}
            }),
        ];
        let resources = cx.new(Resources::new);
        let mut ws = Workspace {
            engine,
            resources,
            library,
            grid,
            filmstrip,
            metadata,
            develop_metadata,
            show_metadata: true,
            rename_dialog: None,
            mode: Mode::Library,
            open: None,
            show_history: true,
            focus,
            export_settings: Settings::default(),
            export_dialog: None,
            keybinding_dialog: None,
            copy_dialog: None,
            copy_groups: Group::defaults(),
            clipboard: None,
            presets,
            exports: Vec::new(),
            next_export: 0,
            _subs: subs,
        };
        match source {
            Some(SourceRef::File(p)) => ws.open_path(p, window, cx),
            Some(s) => ws.open_source(s, None, PhotoMeta::default(), window, cx),
            None => ws.update_title(window, cx),
        }
        if let Some(e) = catalog_error {
            window.push_notification(
                rust_i18n::t!("library.catalog_open_failed", e = e).into_owned(),
                cx,
            );
        }
        ws
    }

    /// Open a folder in the grid, or a photo in develop together with its folder.
    pub fn open_path(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if path.is_dir() {
            self.library
                .update(cx, |l, cx| l.open_folder(path, None, cx));
            self.close_photo(cx);
            self.mode = Mode::Library;
            self.update_title(window, cx);
            cx.notify();
            return;
        }
        let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
        let initial = self.library.update(cx, |l, cx| {
            l.open_folder(dir, Some(path.clone()), cx);
            l.read_meta(&path)
        });
        self.open_source(SourceRef::File(path), None, initial, window, cx);
    }

    fn open_index(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let (item, meta) = self.library.update(cx, |l, cx| {
            l.select(index, cx);
            let item = l.item(index).map(|it| (it.key(), it.copy.clone()));
            (item, l.meta_now(index))
        });
        let Some((key, copy)) = item else { return };
        if self.open_key(cx).as_ref() == Some(&key) {
            self.mode = Mode::Develop;
            self.update_title(window, cx);
            cx.notify();
            return;
        }
        self.open_source(SourceRef::File(key.path), copy, meta, window, cx);
    }

    fn open_key(&self, cx: &App) -> Option<PhotoKey> {
        self.open.as_ref().and_then(|o| o.photo.read(cx).key())
    }

    fn open_source(
        &mut self,
        source: SourceRef,
        copy: Option<CopyInfo>,
        meta: PhotoMeta,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_photo(cx);
        let engine = self.engine.clone();
        let catalog = self.library.read(cx).catalog.clone();
        let history = match &source {
            SourceRef::File(p) => {
                let key = PhotoKey {
                    path: p.clone(),
                    copy: copy.as_ref().map(|c| c.id),
                };
                catalog
                    .history(&key)
                    .inspect_err(|e| tracing::warn!("{e}"))
                    .ok()
                    .flatten()
            }
            SourceRef::Synthetic { .. } => None,
        };
        let photo = cx.new(|cx| {
            Photo::open(
                &engine,
                source,
                copy,
                meta.edits.unwrap_or_default(),
                meta.snapshots,
                history,
                cx,
            )
        });
        let canvas = cx.new(|cx| Canvas::new(photo.clone(), window, cx));
        let hist = canvas.read(cx).histogram();
        let develop_metadata = self.develop_metadata.clone();
        let panel = cx.new(|cx| Panel::new(photo.clone(), hist, develop_metadata, window, cx));
        let history = cx.new(|cx| HistoryPanel::new(photo.clone(), cx));
        let library = self.library.clone();
        let edits = cx.subscribe(&photo, move |w, photo, e, cx| match e {
            PhotoEvent::Edited => {
                let p = photo.read(cx);
                if let Some(key) = p.key() {
                    let head = p.head().clone();
                    library.update(cx, |l, cx| l.photo_edited(&key, &head, cx));
                    w.schedule_history_save(cx);
                }
            }
            PhotoEvent::SnapshotsChanged => {
                let p = photo.read(cx);
                if let Some(key) = p.key() {
                    let snapshots = p.snapshots.clone();
                    library.update(cx, |l, cx| l.set_snapshots(&key, snapshots, cx));
                }
            }
            _ => {}
        });
        self.open = Some(OpenPhoto {
            photo,
            canvas,
            panel,
            history,
            history_save: None,
            _edits: edits,
        });
        self.mode = Mode::Develop;
        self.update_title(window, cx);
        cx.notify();
    }

    /// Save the open photo's history to the catalog shortly after the last edit.
    fn schedule_history_save(&mut self, cx: &mut Context<Self>) {
        let Some(o) = &mut self.open else { return };
        o.history_save = Some(cx.spawn(async move |w, cx| {
            cx.background_executor().timer(HISTORY_DELAY).await;
            let _ = w.update(cx, |w, cx| w.save_history(false, cx));
        }));
    }

    /// Store the open photo's history: in the background, or `now` (closing, quitting).
    fn save_history(&mut self, now: bool, cx: &mut Context<Self>) {
        let Some(o) = &mut self.open else { return };
        if o.history_save.take().is_none() && !now {
            return;
        }
        let p = o.photo.read(cx);
        let Some(key) = p.key() else { return };
        let saved = p.doc.saved_history();
        let catalog = self.library.read(cx).catalog.clone();
        let write = move || {
            if let Err(e) = catalog.set_history(&key, &saved) {
                tracing::warn!("{e}");
            }
        };
        if now {
            write();
        } else {
            cx.background_spawn(async move { write() }).detach();
        }
    }

    fn close_photo(&mut self, cx: &mut Context<Self>) {
        if self.open.as_ref().is_some_and(|o| o.history_save.is_some()) {
            self.save_history(true, cx);
        }
        if self.open.take().is_some() {
            self.library.update(cx, |l, cx| l.flush(cx));
        }
    }

    fn update_title(&self, window: &mut Window, cx: &App) {
        let title = match (self.mode, &self.open) {
            (Mode::Develop, Some(o)) => format!("{} — aigle", o.photo.read(cx).name),
            _ => match self.library.read(cx).folder_name() {
                n if n.is_empty() => "aigle".to_string(),
                n => format!("{n} — aigle"),
            },
        };
        window.set_window_title(&title);
    }

    fn set_mode(&mut self, mode: Mode, window: &mut Window, cx: &mut Context<Self>) {
        if mode == Mode::Develop {
            let selected = self.library.read(cx).selected;
            let selected_key = self.library.read(cx).selected_item().map(|it| it.key());
            if let Some(i) = selected
                && selected_key != self.open_key(cx)
            {
                self.open_index(i, window, cx);
                return;
            }
            if self.open.is_none() {
                return;
            }
        }
        if mode == Mode::Library {
            let sel = self.library.read(cx).selected;
            if let Some(i) = sel {
                self.grid.read(cx).reveal(i);
            }
        }
        self.mode = mode;
        self.update_title(window, cx);
        cx.notify();
    }

    fn prompt_open(&mut self, folder: bool, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: !folder,
            directories: folder,
            multiple: false,
            prompt: Some(crate::i18n::t("library.open_prompt").into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await
                && let Some(p) = paths.into_iter().next()
            {
                this.update_in(cx, |w, window, cx| w.open_path(p, window, cx))?;
            }
            anyhow::Ok(())
        })
        .detach();
    }

    fn with_photo(&self, cx: &mut Context<Self>, f: impl FnOnce(&mut Photo, &mut Context<Photo>)) {
        if self.mode == Mode::Develop
            && let Some(o) = &self.open
        {
            o.photo.update(cx, f);
        }
    }

    fn with_canvas(
        &self,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut Canvas, &mut Context<Canvas>),
    ) {
        if self.mode == Mode::Develop
            && let Some(o) = &self.open
        {
            o.canvas.update(cx, f);
            cx.notify();
        }
    }

    /// Left/right: previous/next photo in develop, selection in the grid.
    fn step(&mut self, dx: isize, dy: isize, window: &mut Window, cx: &mut Context<Self>) {
        match self.mode {
            Mode::Library => self.grid.update(cx, |g, cx| g.move_selection(dx, dy, cx)),
            Mode::Develop if dy == 0 => {
                let target = {
                    let l = self.library.read(cx);
                    let n = l.len() as isize;
                    l.selected
                        .map(|s| s as isize + dx)
                        .filter(|t| (0..n).contains(t))
                };
                if let Some(t) = target {
                    self.open_index(t as usize, window, cx);
                }
            }
            Mode::Develop => {}
        }
    }

    /// The photos ratings and labels apply to: the grid selection, or the open photo (which
    /// develop keeps primary).
    fn marked(&self, cx: &App) -> Vec<usize> {
        let l = self.library.read(cx);
        match self.mode {
            Mode::Library => l.selection(),
            Mode::Develop => l.selected.into_iter().collect(),
        }
    }

    fn rate(&mut self, rating: i8, cx: &mut Context<Self>) {
        let targets = self.marked(cx);
        self.library.update(cx, |l, cx| {
            for i in targets {
                l.set_rating(i, rating, cx);
            }
        });
    }

    /// Set label `n` on the marked photos, or clear it when the primary photo has it.
    fn label(&mut self, n: usize, cx: &mut Context<Self>) {
        let targets = self.marked(cx);
        let label = LABELS[n].0;
        self.library.update(cx, |l, cx| {
            let clear = l.selected_item().and_then(|it| it.label()) == Some(label);
            for i in targets {
                l.set_label(i, (!clear).then_some(label), cx);
            }
        });
    }

    fn change_language(
        &mut self,
        language: crate::i18n::Language,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Err(error) = crate::i18n::save_language(language) {
            window.push_notification(
                format!("{}: {error}", crate::i18n::t("language.save_failed")),
                cx,
            );
        }
        self.update_title(window, cx);
        cx.refresh_windows();
    }

    fn toggle_history(&mut self, cx: &mut Context<Self>) {
        self.show_history = !self.show_history;
        cx.notify();
    }

    fn toggle_tool(&mut self, tool: Tool, cx: &mut Context<Self>) {
        self.with_photo(cx, |p, cx| p.toggle_tool(tool, cx));
        cx.notify();
    }

    /// Esc: drop a pending mask, else leave the tool, else stop picking.
    fn cancel(&mut self, cx: &mut Context<Self>) {
        if self.export_dialog.take().is_some()
            || self.copy_dialog.take().is_some()
            || self.rename_dialog.take().is_some()
        {
            cx.notify();
            return;
        }
        self.with_photo(cx, |p, cx| {
            if p.pending_mask.take().is_some() {
                cx.notify();
            } else if p.wb_pick {
                p.set_wb_pick(false, cx);
            } else {
                p.set_tool(Tool::None, cx);
            }
        });
    }

    fn swap_aspect(&mut self, cx: &mut Context<Self>) {
        self.with_photo(cx, |p, cx| {
            if p.tool == Tool::Crop
                && let Aspect::Ratio { w, h } = p.head().geometry.aspect
            {
                p.set_aspect(Aspect::Ratio { w: h, h: w }, cx);
            }
        });
    }

    /// What export applies to: the selected photos, the open one with its current edits.
    /// In develop with a single selection that is just the open photo (which may not be in
    /// the library, like a synthetic image).
    fn export_requests(&mut self, cx: &mut Context<Self>) -> Vec<Request> {
        let open = self.open.as_ref().map(|o| {
            let p = o.photo.read(cx);
            Request {
                source: p.source.clone(),
                params: p.head().clone(),
                name: p.name.clone(),
                folder: p
                    .path
                    .as_ref()
                    .and_then(|f| f.parent().map(Path::to_path_buf)),
            }
        });
        let selection = self.library.read(cx).selection();
        let open_key = self.open_key(cx);
        if self.mode == Mode::Develop
            && selection.len() <= 1
            && let Some(r) = open
        {
            return vec![r];
        }
        selection
            .into_iter()
            .filter_map(|index| {
                let meta = self.library.update(cx, |l, _| l.meta_now(index));
                let l = self.library.read(cx);
                let it = l.item(index)?;
                if open_key.as_ref() == Some(&it.key())
                    && let Some(r) = &open
                {
                    return Some(r.clone());
                }
                Some(Request {
                    source: SourceRef::File(it.entry.path.clone()),
                    params: Arc::new(meta.edits.unwrap_or_default()),
                    name: it.name(),
                    folder: l.dir.clone(),
                })
            })
            .collect()
    }

    /// The edits that copy reads: the open photo in develop, the selection in the grid.
    fn current_edits(&mut self, cx: &mut Context<Self>) -> Option<Arc<EditParams>> {
        if self.mode == Mode::Develop
            && let Some(o) = &self.open
        {
            return Some(o.photo.read(cx).head().clone());
        }
        let index = self.library.read(cx).selected?;
        let meta = self.library.update(cx, |l, _| l.meta_now(index));
        Some(Arc::new(meta.edits.unwrap_or_default()))
    }

    fn open_groups(
        &mut self,
        groups: BTreeSet<Group>,
        purpose: Purpose,
        window: &mut Window,
        cx: &mut Context<Self>,
        done: impl Fn(&mut Workspace, BTreeSet<Group>, String, &mut Window, &mut Context<Self>)
        + 'static,
    ) {
        self.export_dialog = None;
        let dialog = cx.new(|cx| CopyDialog::new(groups, purpose, window, cx));
        let sub = cx.subscribe_in(&dialog, window, move |w, _, ev, window, cx| {
            let CopyEvent::Done(chosen) = ev;
            w.copy_dialog = None;
            if let Some((groups, name)) = chosen {
                done(w, groups.clone(), name.clone(), window, cx);
            }
            window.focus(&w.focus, cx);
            cx.notify();
        });
        self.copy_dialog = Some((dialog, sub));
        cx.notify();
    }

    fn open_copy(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(params) = self.current_edits(cx) else {
            window.push_notification(crate::i18n::t("copy_settings.no_source"), cx);
            return;
        };
        let groups = self.copy_groups.clone();
        self.open_groups(
            groups,
            Purpose::Copy,
            window,
            cx,
            move |w, groups, _, window, cx| {
                w.copy_groups = groups.clone();
                w.clipboard = Some(Clipboard {
                    params: params.clone(),
                    groups,
                });
                window.push_notification(crate::i18n::t("copy_settings.copied"), cx);
            },
        );
    }

    /// Paste `clip` onto the library photos `targets`; the open photo among them gets it as
    /// an undoable command, the others in their sidecars or copies.
    fn apply_to(
        &mut self,
        targets: &[PhotoKey],
        label: &str,
        origin: Origin,
        clip: &Clipboard,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let open = self.open_key(cx);
        let mut failed = None;
        for key in targets {
            if open.as_ref() == Some(key)
                && let Some(o) = &self.open
            {
                o.photo.update(cx, |ph, cx| {
                    ph.apply_ops(origin.clone(), label, clip.ops(), cx)
                });
                continue;
            }
            let Some(i) = self.library.read(cx).index_of(key) else {
                continue;
            };
            let r = self.library.update(cx, |l, cx| {
                l.edit_stored(i, label, origin.clone(), |e| clip.paste_onto(e), cx)
            });
            if let Err(e) = r {
                failed.get_or_insert(e.to_string());
            }
        }
        if let Some(e) = failed {
            window.push_notification(e, cx);
        }
    }

    fn selected_keys(&self, cx: &App) -> Vec<PhotoKey> {
        let l = self.library.read(cx);
        l.selection()
            .into_iter()
            .filter_map(|i| l.item(i).map(|it| it.key()))
            .collect()
    }

    /// Paste `clip` onto the open photo in develop, or onto every selected photo in the grid.
    fn edit_targets(
        &mut self,
        label: &str,
        origin: Origin,
        clip: &Clipboard,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match self.mode {
            Mode::Develop => {
                if let Some(o) = &self.open {
                    o.photo
                        .update(cx, |p, cx| p.apply_ops(origin, label, clip.ops(), cx));
                }
            }
            Mode::Library => {
                let targets = self.selected_keys(cx);
                self.apply_to(&targets, label, origin, clip, window, cx);
            }
        }
    }

    fn paste(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(clip) = self.clipboard.clone() else {
            window.push_notification(crate::i18n::t("copy_settings.nothing_copied"), cx);
            return;
        };
        self.edit_targets("action.PasteSettings", Origin::User, &clip, window, cx);
    }

    /// Copy chosen groups of the primary photo onto the other selected photos.
    fn open_sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(params) = self.current_edits(cx) else {
            window.push_notification(crate::i18n::t("copy_settings.no_source"), cx);
            return;
        };
        let source = match (self.mode, &self.open) {
            (Mode::Develop, Some(_)) => self.open_key(cx),
            _ => self.library.read(cx).selected_item().map(|it| it.key()),
        };
        let targets: Vec<PhotoKey> = self
            .selected_keys(cx)
            .into_iter()
            .filter(|k| Some(k) != source.as_ref())
            .collect();
        if targets.is_empty() {
            window.push_notification(crate::i18n::t("copy_settings.no_targets"), cx);
            return;
        }
        let groups = self.copy_groups.clone();
        let purpose = Purpose::Sync(targets.len());
        self.open_groups(
            groups,
            purpose,
            window,
            cx,
            move |w, groups, _, window, cx| {
                w.copy_groups = groups.clone();
                let clip = Clipboard {
                    params: params.clone(),
                    groups,
                };
                w.apply_to(
                    &targets,
                    "history.sync_settings",
                    Origin::User,
                    &clip,
                    window,
                    cx,
                );
            },
        );
    }

    fn apply_preset(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(preset) = self
            .presets
            .read(cx)
            .store
            .items
            .get(index)
            .map(|(_, p)| p.clone())
        else {
            return;
        };
        let label = crate::i18n::join(&["history.preset", &preset.name]);
        let clip = preset.clipboard();
        self.edit_targets(&label, Origin::Preset, &clip, window, cx);
    }

    fn open_new_preset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(params) = self.current_edits(cx) else {
            window.push_notification(crate::i18n::t("copy_settings.no_source"), cx);
            return;
        };
        let presets = self.presets.clone();
        self.open_groups(
            Group::defaults(),
            Purpose::Preset,
            window,
            cx,
            move |_, groups, name, window, cx| {
                let r = presets.update(cx, |p, cx| {
                    let name = if name.is_empty() {
                        crate::presets::unique_name(
                            p.store.names(),
                            &crate::i18n::t("presets.default_name"),
                        )
                    } else {
                        name
                    };
                    cx.notify();
                    p.store.save(Preset {
                        name,
                        groups,
                        params: (*params).clone(),
                    })
                });
                if let Err(e) = r {
                    window.push_notification(
                        rust_i18n::t!("presets.save_failed", e = e).into_owned(),
                        cx,
                    );
                }
            },
        );
    }

    fn remove_preset(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let r = self.presets.update(cx, |p, cx| {
            cx.notify();
            p.store.remove(index)
        });
        if let Err(e) = r {
            window.push_notification(
                rust_i18n::t!("presets.delete_failed", e = e).into_owned(),
                cx,
            );
        }
    }

    /// Make virtual copies of the open photo (opening the copy) or of the grid selection.
    fn create_virtual_copy(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let indices: Vec<usize> = match self.mode {
            Mode::Develop => {
                let key = self.open_key(cx);
                key.and_then(|k| self.library.read(cx).index_of(&k))
                    .into_iter()
                    .collect()
            }
            Mode::Library => self.library.read(cx).selection(),
        };
        if indices.is_empty() {
            window.push_notification(crate::i18n::t("library.no_copy_source"), cx);
            return;
        }
        let created = self
            .library
            .update(cx, |l, cx| l.create_copies(&indices, cx));
        if self.mode == Mode::Develop
            && let Some(i) = created
                .first()
                .and_then(|k| self.library.read(cx).index_of(k))
        {
            self.open_index(i, window, cx);
        }
        cx.notify();
    }

    /// Delete the virtual copies among the open photo (develop) or the grid selection, after
    /// asking. Photos themselves are never deleted.
    fn remove_virtual_copies(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let keys: Vec<PhotoKey> = match self.mode {
            Mode::Develop => self.open_key(cx).into_iter().collect(),
            Mode::Library => self.selected_keys(cx),
        };
        let keys: Vec<PhotoKey> = keys.into_iter().filter(|k| k.copy.is_some()).collect();
        if keys.is_empty() {
            window.push_notification(crate::i18n::t("library.no_virtual_copies"), cx);
            return;
        }
        let message = rust_i18n::t!("library.delete_copies_prompt", arg0 = keys.len()).into_owned();
        let detail = crate::i18n::t("library.delete_copies_detail");
        let (delete, cancel) = (
            crate::i18n::t("common.delete"),
            crate::i18n::t("common.cancel"),
        );
        let answer = window.prompt(
            gpui::PromptLevel::Warning,
            &message,
            Some(&detail),
            &[delete.as_str(), cancel.as_str()],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if answer.await != Ok(0) {
                return anyhow::Ok(());
            }
            this.update_in(cx, |w, window, cx| {
                let indices: Vec<usize> = keys
                    .iter()
                    .filter_map(|k| w.library.read(cx).index_of(k))
                    .collect();
                let removed = w.library.update(cx, |l, cx| l.remove_copies(&indices, cx));
                let open = w.open_key(cx);
                if open.as_ref().is_some_and(|k| removed.contains(k)) {
                    if let Some(o) = &mut w.open {
                        o.history_save = None;
                    }
                    w.close_photo(cx);
                    let next = w.library.read(cx).selected;
                    match (w.mode, next) {
                        (Mode::Develop, Some(i)) => w.open_index(i, window, cx),
                        _ => {
                            w.mode = Mode::Library;
                            w.update_title(window, cx);
                        }
                    }
                }
                cx.notify();
            })
        })
        .detach();
    }

    fn toggle_metadata(&mut self, cx: &mut Context<Self>) {
        self.show_metadata = !self.show_metadata;
        let side = if self.show_metadata { METADATA_W } else { 0.0 };
        self.grid.update(cx, |g, cx| {
            g.side = side;
            cx.notify();
        });
        cx.notify();
    }

    /// If the open photo (or a copy of it) is one of the files `paths`, close it, saving
    /// its history, and show the grid. Returns the key that was open.
    fn release_open(
        &mut self,
        paths: &[PathBuf],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<PhotoKey> {
        let key = self.open_key(cx).filter(|k| paths.contains(&k.path))?;
        self.close_photo(cx);
        self.mode = Mode::Library;
        self.update_title(window, cx);
        cx.notify();
        Some(key)
    }

    fn open_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let target = {
            let l = self.library.read(cx);
            l.selected_item()
                .map(|it| (it.key(), it.entry.name.clone(), it.copy.clone()))
        };
        let Some((key, file, copy)) = target else {
            window.push_notification(crate::i18n::t("library.nothing_selected"), cx);
            return;
        };
        let (title, current, suffix) = match &copy {
            Some(c) => (
                crate::i18n::t("rename.copy_title"),
                c.name.clone(),
                String::new(),
            ),
            None => {
                let p = Path::new(&file);
                let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned());
                let ext = p.extension().map(|e| format!(".{}", e.to_string_lossy()));
                (
                    crate::i18n::t("rename.title"),
                    stem.unwrap_or(file.clone()),
                    ext.unwrap_or_default(),
                )
            }
        };
        self.export_dialog = None;
        self.copy_dialog = None;
        let dialog = cx.new(|cx| RenameDialog::new(title, current, suffix, window, cx));
        let sub = cx.subscribe_in(&dialog, window, move |w, _, ev, window, cx| {
            let RenameEvent::Done(name) = ev;
            w.rename_dialog = None;
            if let Some(name) = name {
                w.rename_to(&key, name, window, cx);
            }
            window.focus(&w.focus, cx);
            cx.notify();
        });
        self.rename_dialog = Some((dialog, sub));
        cx.notify();
    }

    /// Rename the photo or copy `key`, reopening it in develop if it was open there.
    fn rename_to(
        &mut self,
        key: &PhotoKey,
        name: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(index) = self.library.read(cx).index_of(key) else {
            return;
        };
        let develop = self.mode == Mode::Develop;
        let affected = self.open_key(cx).is_some_and(|open| match key.copy {
            None => open.path == key.path,
            Some(_) => open == *key,
        });
        let was_open = if affected {
            self.release_open(std::slice::from_ref(&key.path), window, cx)
        } else {
            None
        };
        let renamed = self.library.update(cx, |l, cx| l.rename(index, name, cx));
        let reopen = match renamed {
            Ok(new) => was_open.map(|k| PhotoKey {
                path: new.path,
                ..k
            }),
            Err(e) => {
                window.push_notification(
                    rust_i18n::t!("library.rename_failed", e = e).into_owned(),
                    cx,
                );
                was_open
            }
        };
        if let Some(k) = reopen
            && develop
            && let Some(i) = self.library.read(cx).index_of(&k)
        {
            self.open_index(i, window, cx);
        }
        cx.notify();
    }

    fn prompt_move(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let keys = self.marked_keys(cx);
        if keys.is_empty() {
            window.push_notification(crate::i18n::t("library.nothing_selected"), cx);
            return;
        }
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(crate::i18n::t("library.move_prompt").into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await
                && let Some(dest) = paths.into_iter().next()
            {
                this.update_in(cx, |w, window, cx| w.move_to(&keys, dest, window, cx))?;
            }
            anyhow::Ok(())
        })
        .detach();
    }

    fn move_to(
        &mut self,
        keys: &[PhotoKey],
        dest: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let indices: Vec<usize> = {
            let l = self.library.read(cx);
            keys.iter().filter_map(|k| l.index_of(k)).collect()
        };
        let paths = self.library.read(cx).photo_paths(&indices);
        self.release_open(&paths, window, cx);
        self.library
            .update(cx, |l, cx| l.move_photos(&indices, dest, cx));
    }

    /// The keys of [`Workspace::marked`].
    fn marked_keys(&self, cx: &App) -> Vec<PhotoKey> {
        let l = self.library.read(cx);
        self.marked(cx)
            .into_iter()
            .filter_map(|i| l.item(i).map(|it| it.key()))
            .collect()
    }

    /// Move the marked photos to the trash after asking; their virtual copies go with them.
    /// Selected copies of photos that stay are deleted. With only copies selected this is
    /// [`Workspace::remove_virtual_copies`].
    fn trash_marked(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let keys = self.marked_keys(cx);
        let paths: Vec<PathBuf> = keys
            .iter()
            .filter(|k| k.copy.is_none())
            .map(|k| k.path.clone())
            .collect();
        if paths.is_empty() {
            if keys.is_empty() {
                window.push_notification(crate::i18n::t("library.nothing_selected"), cx);
            } else {
                self.remove_virtual_copies(window, cx);
            }
            return;
        }
        let copies: Vec<PhotoKey> = keys
            .into_iter()
            .filter(|k| k.copy.is_some() && !paths.contains(&k.path))
            .collect();
        let message = rust_i18n::t!("library.trash_prompt", arg0 = paths.len()).into_owned();
        let detail = crate::i18n::t("library.trash_detail");
        let (trash, cancel) = (
            crate::i18n::t("library.trash"),
            crate::i18n::t("common.cancel"),
        );
        let answer = window.prompt(
            gpui::PromptLevel::Warning,
            &message,
            Some(&detail),
            &[trash.as_str(), cancel.as_str()],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if answer.await != Ok(0) {
                return anyhow::Ok(());
            }
            this.update_in(cx, |w, window, cx| {
                let mut released: Vec<PathBuf> = paths.clone();
                released.extend(copies.iter().map(|k| k.path.clone()));
                if let Some(open) = w.open_key(cx)
                    && (paths.contains(&open.path) || copies.contains(&open))
                {
                    w.release_open(&released, window, cx);
                }
                w.library.update(cx, |l, cx| {
                    let lone: Vec<usize> = copies.iter().filter_map(|k| l.index_of(k)).collect();
                    l.remove_copies(&lone, cx);
                    l.trash_photos(paths, cx);
                });
                cx.notify();
            })
        })
        .detach();
    }

    fn open_keybindings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((editor, _)) = &self.keybinding_dialog {
            editor.update(cx, |editor, cx| editor.focus_search(window, cx));
            return;
        }
        self.export_dialog = None;
        let editor = cx.new(|cx| KeybindingEditor::new(window, cx));
        let subscription = cx.subscribe_in(
            &editor,
            window,
            |w, _, _: &KeybindingsClosed, window, cx| {
                window.close_dialog(cx);
                w.keybinding_dialog = None;
                window.focus(&w.focus, cx);
                cx.notify();
            },
        );
        self.keybinding_dialog = Some((editor.clone(), subscription));
        let initial_focus = std::cell::Cell::new(true);
        window.open_dialog(cx, move |dialog, window, cx| {
            if initial_focus.replace(false) {
                editor.update(cx, |editor, cx| editor.focus_search(window, cx));
            }
            dialog
                .title(crate::i18n::t("keybindings.title"))
                .w(px(820.0))
                .max_w(window.viewport_size().width - px(48.0))
                .margin_top(px(24.0))
                .overlay_closable(false)
                .keyboard(false)
                .close_button(false)
                .child(editor.clone())
        });
        cx.notify();
    }

    fn open_export(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let requests = self.export_requests(cx);
        if requests.is_empty() {
            window.push_notification(crate::i18n::t("export.nothing"), cx);
            return;
        }
        self.copy_dialog = None;
        let settings = self.export_settings.clone();
        let dialog = cx.new(|cx| ExportDialog::new(requests, settings, window, cx));
        let sub = cx.subscribe_in(&dialog, window, |w, _, ev, window, cx| {
            let ExportEvent::Done(chosen) = ev;
            w.export_dialog = None;
            if let Some((jobs, settings)) = chosen {
                w.export_settings = settings.clone();
                w.start_export(jobs.clone(), settings.clone(), window, cx);
            }
            window.focus(&w.focus, cx);
            cx.notify();
        });
        self.export_dialog = Some((dialog, sub));
        cx.notify();
    }

    /// Export `jobs` one after another in the background.
    fn start_export(
        &mut self,
        jobs: Vec<(Request, PathBuf)>,
        settings: Settings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let id = self.next_export;
        self.next_export += 1;
        let total = jobs.len();
        let engine = self.engine.clone();
        let task = cx.spawn_in(window, async move |this, cx| {
            let mut last = None;
            let mut failures = Vec::new();
            for (request, path) in jobs {
                let shown = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                match export::run(&engine, &request, &settings, path).await {
                    Ok(done) => last = Some((shown, done)),
                    Err(e) => failures.push((shown, e.to_string())),
                }
                let ok = this.update(cx, |w, cx| {
                    if let Some(e) = w.exports.iter_mut().find(|e| e.id == id) {
                        e.done += 1;
                    }
                    cx.notify();
                });
                if ok.is_err() {
                    return;
                }
            }
            let _ = this.update_in(cx, |w, window, cx| {
                w.exports.retain(|e| e.id != id);
                let msg = match (total, last, failures.first()) {
                    (1, Some((shown, done)), None) => rust_i18n::t!(
                        "export.done_one",
                        arg0 = done.width,
                        arg1 = done.height,
                        shown = shown
                    )
                    .into_owned(),
                    (1, _, Some((shown, e))) => {
                        rust_i18n::t!("export.failed_one", shown = shown, e = e).into_owned()
                    }
                    (_, _, None) => rust_i18n::t!("export.done_many", arg0 = total).into_owned(),
                    (_, _, Some((shown, e))) => rust_i18n::t!(
                        "export.failed_many",
                        arg0 = failures.len(),
                        shown = shown,
                        e = e
                    )
                    .into_owned(),
                };
                window.push_notification(msg, cx);
                cx.notify();
            });
        });
        self.exports.push(ExportJob {
            id,
            total,
            done: 0,
            _task: task,
        });
        cx.notify();
    }

    fn toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let develop = self.mode == Mode::Develop;
        let (can_undo, can_redo, show_info, compare, tool, monochrome) = match (&self.open, develop)
        {
            (Some(o), true) => {
                let p = o.photo.read(cx);
                let c = o.canvas.read(cx);
                (
                    p.doc.can_undo(),
                    p.doc.can_redo(),
                    c.show_info(),
                    c.compare(),
                    p.tool,
                    p.head().bw.monochrome,
                )
            }
            _ => (false, false, false, Compare::Off, Tool::None, false),
        };
        let show_history = self.show_history;
        let show_metadata = self.show_metadata;
        let can_paste = self.clipboard.is_some();
        let ws = cx.entity().downgrade();
        let preset_names: Vec<SharedString> = self
            .presets
            .read(cx)
            .store
            .names()
            .map(|n| SharedString::from(n.to_string()))
            .collect();
        let mask_overlay = self
            .open
            .as_ref()
            .is_some_and(|o| o.photo.read(cx).mask_overlay);
        let (proof, proof_target, gamut_warning) = match &self.open {
            Some(o) => {
                let c = o.canvas.read(cx);
                (
                    c.proof().cloned(),
                    c.proof_target().clone(),
                    c.gamut_warning(),
                )
            }
            None => (None, DisplayProfileId::Srgb, false),
        };
        let title = {
            let l = self.library.read(cx);
            match (&self.open, develop) {
                (Some(o), true) => o.photo.read(cx).name.clone(),
                _ if l.dir.is_some() && l.len() < l.total() => rust_i18n::t!(
                    "toolbar.title_filtered",
                    arg0 = l.folder_name(),
                    arg1 = l.len(),
                    arg2 = l.total()
                )
                .into_owned(),
                _ if l.dir.is_some() => rust_i18n::t!(
                    "toolbar.title_count",
                    arg0 = l.folder_name(),
                    arg1 = l.total()
                )
                .into_owned(),
                _ => String::new(),
            }
        };
        let file_focus = self.focus.clone();
        let edit_focus = self.focus.clone();
        let view_focus = self.focus.clone();
        let language = crate::i18n::language();
        let display = cx.global::<crate::display::DisplayColor>().clone();
        let row = h_flex()
            .flex_1()
            .h_full()
            .gap_2()
            .pr_2()
            .child(
                Button::new("file-menu")
                    .label(crate::i18n::t("menu.file"))
                    .small()
                    .dropdown_menu(move |menu, _, _| {
                        menu.action_context(file_focus.clone())
                            .menu(crate::i18n::t("action.Open"), Box::new(Open))
                            .menu(crate::i18n::t("action.OpenFolder"), Box::new(OpenFolder))
                            .separator()
                            .menu(crate::i18n::t("action.RenamePhoto"), Box::new(RenamePhoto))
                            .menu(crate::i18n::t("action.MovePhotos"), Box::new(MovePhotos))
                            .menu(crate::i18n::t("action.TrashPhotos"), Box::new(TrashPhotos))
                            .separator()
                            .menu(crate::i18n::t("action.Export"), Box::new(Export))
                            .separator()
                            .menu(crate::i18n::t("action.Quit"), Box::new(Quit))
                    }),
            )
            .child(
                Button::new("edit-menu")
                    .label(crate::i18n::t("menu.edit"))
                    .small()
                    .dropdown_menu(move |menu, window, cx| {
                        menu.action_context(edit_focus.clone())
                            .menu_with_disabled(
                                crate::i18n::t("action.Undo"),
                                Box::new(Undo),
                                !can_undo,
                            )
                            .menu_with_disabled(
                                crate::i18n::t("action.Redo"),
                                Box::new(Redo),
                                !can_redo,
                            )
                            .separator()
                            .menu(
                                crate::i18n::t("action.CopySettings"),
                                Box::new(CopySettings),
                            )
                            .menu_with_disabled(
                                crate::i18n::t("action.PasteSettings"),
                                Box::new(PasteSettings),
                                !can_paste,
                            )
                            .menu(
                                crate::i18n::t("action.SyncSettings"),
                                Box::new(SyncSettings),
                            )
                            .separator()
                            .menu(
                                crate::i18n::t("action.CreateVirtualCopy"),
                                Box::new(CreateVirtualCopy),
                            )
                            .menu(
                                crate::i18n::t("action.RemoveVirtualCopy"),
                                Box::new(RemoveVirtualCopy),
                            )
                            .separator()
                            .menu(crate::i18n::t("action.NewPreset"), Box::new(NewPreset))
                            .submenu(crate::i18n::t("menu.apply_preset"), window, cx, {
                                let names = preset_names.clone();
                                let ws = ws.clone();
                                move |menu, _, _| {
                                    if names.is_empty() {
                                        return menu.label(crate::i18n::t("presets.empty"));
                                    }
                                    names.iter().enumerate().fold(menu, |menu, (i, n)| {
                                        let ws = ws.clone();
                                        menu.item(PopupMenuItem::new(n.clone()).on_click(
                                            move |_, window, cx| {
                                                let _ = ws.update(cx, |w, cx| {
                                                    w.apply_preset(i, window, cx)
                                                });
                                            },
                                        ))
                                    })
                                }
                            })
                            .separator()
                            .menu_with_disabled(
                                crate::i18n::t("action.AutoTone"),
                                Box::new(AutoTone),
                                !develop,
                            )
                            .menu_with_check_and_disabled(
                                crate::i18n::t("action.ToggleMonochrome"),
                                monochrome,
                                Box::new(ToggleMonochrome),
                                !develop,
                            )
                            .separator()
                            .submenu(
                                crate::i18n::t("menu.tools"),
                                window,
                                cx,
                                move |menu, _, _| {
                                    menu.menu_with_check_and_disabled(
                                        crate::i18n::t("action.ToggleCrop"),
                                        tool == Tool::Crop,
                                        Box::new(ToggleCrop),
                                        !develop,
                                    )
                                    .menu_with_check_and_disabled(
                                        crate::i18n::t("action.ToggleMask"),
                                        tool == Tool::Mask,
                                        Box::new(ToggleMask),
                                        !develop,
                                    )
                                    .menu_with_check_and_disabled(
                                        crate::i18n::t("action.ToggleSpot"),
                                        tool == Tool::Spot,
                                        Box::new(ToggleSpot),
                                        !develop,
                                    )
                                    .menu_with_check_and_disabled(
                                        crate::i18n::t("action.ToggleRedEye"),
                                        tool == Tool::RedEye,
                                        Box::new(ToggleRedEye),
                                        !develop,
                                    )
                                    .menu_with_disabled(
                                        crate::i18n::t("action.ToggleWbPicker"),
                                        Box::new(ToggleWbPicker),
                                        !develop,
                                    )
                                },
                            )
                    }),
            )
            .child(
                Button::new("view-menu")
                    .label(crate::i18n::t("menu.view"))
                    .small()
                    .dropdown_menu(move |menu, window, cx| {
                        menu.action_context(view_focus.clone())
                            .menu_with_check(
                                crate::i18n::t("action.ShowGrid"),
                                !develop,
                                Box::new(ShowGrid),
                            )
                            .menu_with_check(
                                crate::i18n::t("action.ShowDevelop"),
                                develop,
                                Box::new(ShowDevelop),
                            )
                            .separator()
                            .submenu(
                                crate::i18n::t("menu.compare"),
                                window,
                                cx,
                                move |menu, _, _| {
                                    menu.menu_with_check_and_disabled(
                                        crate::i18n::t("action.ToggleBefore"),
                                        compare == Compare::Before,
                                        Box::new(ToggleBefore),
                                        !develop,
                                    )
                                    .menu_with_check_and_disabled(
                                        crate::i18n::t("action.ToggleSplit"),
                                        compare == Compare::Split,
                                        Box::new(ToggleSplit),
                                        !develop,
                                    )
                                },
                            )
                            .submenu(crate::i18n::t("menu.soft_proof"), window, cx, {
                                let (proof, target) = (proof.clone(), proof_target.clone());
                                move |menu, _, _| {
                                    let menu = menu.menu_with_check_and_disabled(
                                        crate::i18n::t("action.ToggleSoftProof"),
                                        proof.is_some(),
                                        Box::new(ToggleSoftProof),
                                        !develop,
                                    );
                                    let menu = [
                                        (DisplayProfileId::Srgb, "sRGB"),
                                        (DisplayProfileId::DisplayP3, "Display P3"),
                                        (DisplayProfileId::AdobeRgb, "Adobe RGB"),
                                    ]
                                    .into_iter()
                                    .fold(
                                        menu.separator(),
                                        |menu, (d, label)| {
                                            let action: Box<dyn gpui::Action> = match d {
                                                DisplayProfileId::DisplayP3 => {
                                                    Box::new(ProofDisplayP3)
                                                }
                                                DisplayProfileId::AdobeRgb => {
                                                    Box::new(ProofAdobeRgb)
                                                }
                                                _ => Box::new(ProofSrgb),
                                            };
                                            menu.menu_with_check_and_disabled(
                                                label,
                                                target == d,
                                                action,
                                                !develop,
                                            )
                                        },
                                    );
                                    menu.separator().menu_with_check_and_disabled(
                                        crate::i18n::t("action.ToggleGamutWarning"),
                                        gamut_warning,
                                        Box::new(ToggleGamutWarning),
                                        !develop,
                                    )
                                }
                            })
                            .menu_with_disabled(
                                crate::i18n::t("action.ToggleZoom"),
                                Box::new(ToggleZoom),
                                !develop,
                            )
                            .separator()
                            .menu_with_check_and_disabled(
                                crate::i18n::t("action.ToggleMetadata"),
                                show_metadata,
                                Box::new(ToggleMetadata),
                                develop,
                            )
                            .menu_with_check_and_disabled(
                                crate::i18n::t("action.ToggleHistory"),
                                show_history,
                                Box::new(ToggleHistory),
                                !develop,
                            )
                            .menu_with_check_and_disabled(
                                crate::i18n::t("action.ToggleInfo"),
                                show_info,
                                Box::new(ToggleInfo),
                                !develop,
                            )
                            .menu_with_check_and_disabled(
                                crate::i18n::t("action.ToggleOverlay"),
                                mask_overlay,
                                Box::new(ToggleOverlay),
                                tool != Tool::Mask,
                            )
                            .separator()
                            .submenu(crate::i18n::t("menu.display_profile"), window, cx, {
                                let display = display.clone();
                                move |menu, _, _| {
                                    let display = display.clone();
                                    let menu = menu
                                        .menu_with_check(
                                            crate::i18n::t("display.auto"),
                                            display.mode == crate::display::Mode::Auto,
                                            Box::new(DisplayAuto),
                                        )
                                        .menu_with_check(
                                            "sRGB",
                                            display.mode == crate::display::Mode::Srgb,
                                            Box::new(DisplaySrgb),
                                        )
                                        .separator();
                                    match (&display.detected, &display.problem) {
                                        (Some(d), _) => menu.label(
                                            rust_i18n::t!("display.detected", name = d.name)
                                                .into_owned(),
                                        ),
                                        (None, Some(_)) => {
                                            menu.label(crate::i18n::t("display.none"))
                                        }
                                        (None, None) => {
                                            menu.label(crate::i18n::t("display.detecting"))
                                        }
                                    }
                                }
                            })
                            .separator()
                            .submenu(
                                crate::i18n::t("menu.keybindings"),
                                window,
                                cx,
                                |menu, _, _| {
                                    menu.menu(
                                        crate::i18n::t("action.EditKeybindings"),
                                        Box::new(EditKeybindings),
                                    )
                                    .menu(
                                        crate::i18n::t("action.ReloadKeybindings"),
                                        Box::new(ReloadKeybindings),
                                    )
                                },
                            )
                            .separator()
                            .submenu(
                                crate::i18n::t("menu.language"),
                                window,
                                cx,
                                move |menu, _, _| {
                                    menu.menu_with_check(
                                        crate::i18n::t("language.name.en"),
                                        language == crate::i18n::Language::English,
                                        Box::new(LanguageEnglish),
                                    )
                                    .menu_with_check(
                                        crate::i18n::t("language.name.zh_cn"),
                                        language == crate::i18n::Language::Chinese,
                                        Box::new(LanguageChinese),
                                    )
                                },
                            )
                    }),
            )
            .child(
                ButtonGroup::new("mode")
                    .small()
                    .child(
                        Button::new("grid")
                            .label(crate::i18n::t("action.ShowGrid"))
                            .selected(!develop)
                            .tooltip(crate::keymap::hint("toolbar.grid_tooltip", cx))
                            .on_click(cx.listener(|w, _, window, cx| {
                                w.set_mode(Mode::Library, window, cx)
                            })),
                    )
                    .child(
                        Button::new("develop")
                            .label(crate::i18n::t("action.ShowDevelop"))
                            .selected(develop)
                            .tooltip(crate::keymap::hint("toolbar.develop_tooltip", cx))
                            .on_click(cx.listener(|w, _, window, cx| {
                                w.set_mode(Mode::Develop, window, cx)
                            })),
                    ),
            )
            .when(!self.exports.is_empty(), |row| {
                let (done, total) = self
                    .exports
                    .iter()
                    .fold((0, 0), |(d, t), e| (d + e.done, t + e.total));
                row.child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(
                            rust_i18n::t!("toolbar.exporting", arg0 = done, arg1 = total)
                                .into_owned(),
                        ),
                )
            })
            .child(
                div()
                    .flex_1()
                    .text_sm()
                    .text_center()
                    .truncate()
                    .child(title),
            )
            .child(self.resources.clone());
        TitleBar::new().child(row)
    }

    fn develop(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let border = cx.theme().border;
        let Some(o) = &self.open else {
            return h_flex()
                .flex_1()
                .justify_center()
                .text_color(cx.theme().muted_foreground)
                .child(crate::keymap::hint("develop.empty_hint", cx))
                .into_any_element();
        };
        let has_strip = self.library.read(cx).len() > 0;
        v_flex()
            .flex_1()
            .min_h_0()
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .when(self.show_history, |d| {
                        d.child(
                            v_flex()
                                .w(px(200.0))
                                .h_full()
                                .flex_none()
                                .border_r_1()
                                .border_color(border)
                                .child(self.presets.clone())
                                .child(
                                    div()
                                        .flex_1()
                                        .min_h_0()
                                        .border_t_1()
                                        .border_color(border)
                                        .child(o.history.clone()),
                                ),
                        )
                    })
                    .child(div().flex_1().h_full().min_w_0().child(o.canvas.clone()))
                    .child(
                        div()
                            .w(px(340.0))
                            .h_full()
                            .flex_none()
                            .border_l_1()
                            .border_color(border)
                            .child(o.panel.clone()),
                    ),
            )
            .when(has_strip, |d| d.child(self.filmstrip.clone()))
            .into_any_element()
    }
}

impl Render for Workspace {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = match self.mode {
            Mode::Library => {
                let border = cx.theme().border;
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .child(crate::library_bar::filter_bar(&self.library, cx))
                    .child(
                        h_flex()
                            .flex_1()
                            .min_h_0()
                            .child(div().flex_1().h_full().min_w_0().child(self.grid.clone()))
                            .when(self.show_metadata, |d| {
                                d.child(
                                    div()
                                        .w(px(METADATA_W))
                                        .h_full()
                                        .flex_none()
                                        .border_l_1()
                                        .border_color(border)
                                        .child(self.metadata.clone()),
                                )
                            }),
                    )
                    .into_any_element()
            }
            Mode::Develop => self.develop(cx),
        };
        let dialog = self.export_dialog.as_ref().map(|(d, _)| d.clone());
        let copy_dialog = self.copy_dialog.as_ref().map(|(d, _)| d.clone());
        let rename_dialog = self.rename_dialog.as_ref().map(|(d, _)| d.clone());
        v_flex()
            .size_full()
            .track_focus(&self.focus)
            .key_context(CONTEXT)
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .on_action(cx.listener(|w, _: &Open, window, cx| w.prompt_open(false, window, cx)))
            .on_action(cx.listener(|w, _: &OpenFolder, window, cx| w.prompt_open(true, window, cx)))
            .on_action(
                cx.listener(|w, _: &ShowGrid, window, cx| w.set_mode(Mode::Library, window, cx)),
            )
            .on_action(
                cx.listener(|w, _: &ShowDevelop, window, cx| w.set_mode(Mode::Develop, window, cx)),
            )
            .on_action(cx.listener(|w, _: &OpenSelected, window, cx| {
                if w.mode == Mode::Library {
                    w.set_mode(Mode::Develop, window, cx)
                } else {
                    w.with_photo(cx, |p, cx| {
                        if p.tool == Tool::Crop {
                            p.set_tool(Tool::None, cx)
                        }
                    })
                }
            }))
            .on_action(cx.listener(|w, _: &Left, window, cx| w.step(-1, 0, window, cx)))
            .on_action(cx.listener(|w, _: &Right, window, cx| w.step(1, 0, window, cx)))
            .on_action(cx.listener(|w, _: &Up, window, cx| w.step(0, -1, window, cx)))
            .on_action(cx.listener(|w, _: &Down, window, cx| w.step(0, 1, window, cx)))
            .on_action(cx.listener(|w, _: &Rate0, _, cx| w.rate(0, cx)))
            .on_action(cx.listener(|w, _: &Rate1, _, cx| w.rate(1, cx)))
            .on_action(cx.listener(|w, _: &Rate2, _, cx| w.rate(2, cx)))
            .on_action(cx.listener(|w, _: &Rate3, _, cx| w.rate(3, cx)))
            .on_action(cx.listener(|w, _: &Rate4, _, cx| w.rate(4, cx)))
            .on_action(cx.listener(|w, _: &Rate5, _, cx| w.rate(5, cx)))
            .on_action(cx.listener(|w, _: &Label1, _, cx| w.label(0, cx)))
            .on_action(cx.listener(|w, _: &Label2, _, cx| w.label(1, cx)))
            .on_action(cx.listener(|w, _: &Label3, _, cx| w.label(2, cx)))
            .on_action(cx.listener(|w, _: &Label4, _, cx| w.label(3, cx)))
            .on_action(cx.listener(|w, _: &Undo, _, cx| w.with_photo(cx, Photo::undo)))
            .on_action(cx.listener(|w, _: &Redo, _, cx| w.with_photo(cx, Photo::redo)))
            .on_action(cx.listener(|w, _: &CopySettings, window, cx| w.open_copy(window, cx)))
            .on_action(cx.listener(|w, _: &PasteSettings, window, cx| w.paste(window, cx)))
            .on_action(cx.listener(|w, _: &SyncSettings, window, cx| w.open_sync(window, cx)))
            .on_action(cx.listener(|w, _: &NewPreset, window, cx| w.open_new_preset(window, cx)))
            .on_action(
                cx.listener(|w, _: &CreateVirtualCopy, window, cx| {
                    w.create_virtual_copy(window, cx)
                }),
            )
            .on_action(cx.listener(|w, _: &RemoveVirtualCopy, window, cx| {
                w.remove_virtual_copies(window, cx)
            }))
            .on_action(cx.listener(|w, _: &RenamePhoto, window, cx| w.open_rename(window, cx)))
            .on_action(cx.listener(|w, _: &MovePhotos, window, cx| w.prompt_move(window, cx)))
            .on_action(cx.listener(|w, _: &TrashPhotos, window, cx| w.trash_marked(window, cx)))
            .on_action(cx.listener(|w, _: &ToggleMetadata, _, cx| w.toggle_metadata(cx)))
            .on_action(
                cx.listener(|w, _: &SelectAll, _, cx| {
                    w.library.update(cx, |l, cx| l.select_all(cx))
                }),
            )
            .on_action(
                cx.listener(|w, _: &ToggleZoom, _, cx| w.with_canvas(cx, Canvas::toggle_zoom)),
            )
            .on_action(
                cx.listener(|w, _: &ToggleBefore, _, cx| w.with_canvas(cx, Canvas::toggle_before)),
            )
            .on_action(
                cx.listener(|w, _: &ToggleSplit, _, cx| w.with_canvas(cx, Canvas::toggle_split)),
            )
            .on_action(
                cx.listener(|w, _: &ToggleSoftProof, _, cx| {
                    w.with_canvas(cx, Canvas::toggle_proof)
                }),
            )
            .on_action(cx.listener(|w, _: &ToggleGamutWarning, _, cx| {
                w.with_canvas(cx, Canvas::toggle_gamut_warning)
            }))
            .on_action(cx.listener(|w, _: &ProofSrgb, _, cx| {
                w.with_canvas(cx, |c, cx| c.set_proof_target(DisplayProfileId::Srgb, cx))
            }))
            .on_action(cx.listener(|w, _: &ProofDisplayP3, _, cx| {
                w.with_canvas(cx, |c, cx| {
                    c.set_proof_target(DisplayProfileId::DisplayP3, cx)
                })
            }))
            .on_action(cx.listener(|w, _: &ProofAdobeRgb, _, cx| {
                w.with_canvas(cx, |c, cx| {
                    c.set_proof_target(DisplayProfileId::AdobeRgb, cx)
                })
            }))
            .on_action(cx.listener(|w, _: &ToggleWbPicker, _, cx| {
                w.with_photo(cx, |p, cx| p.set_wb_pick(!p.wb_pick, cx))
            }))
            .on_action(
                cx.listener(|w, _: &ToggleInfo, _, cx| w.with_canvas(cx, Canvas::toggle_info)),
            )
            .on_action(cx.listener(|w, _: &ToggleHistory, _, cx| w.toggle_history(cx)))
            .on_action(cx.listener(|w, _: &ToggleCrop, _, cx| w.toggle_tool(Tool::Crop, cx)))
            .on_action(cx.listener(|w, _: &AutoTone, _, cx| {
                w.with_photo(cx, |p, cx| p.auto_tone(cx));
                cx.notify();
            }))
            .on_action(cx.listener(|w, _: &ToggleMonochrome, _, cx| {
                w.with_photo(cx, |p, cx| p.toggle_monochrome(cx));
                cx.notify();
            }))
            .on_action(cx.listener(|w, _: &ToggleMask, _, cx| w.toggle_tool(Tool::Mask, cx)))
            .on_action(cx.listener(|w, _: &ToggleSpot, _, cx| w.toggle_tool(Tool::Spot, cx)))
            .on_action(cx.listener(|w, _: &ToggleRedEye, _, cx| w.toggle_tool(Tool::RedEye, cx)))
            .on_action(cx.listener(|w, _: &DeleteRetouch, _, cx| {
                w.with_photo(cx, |p, cx| {
                    p.delete_selected_retouch(cx);
                })
            }))
            .on_action(cx.listener(|w, _: &ToggleOverlay, _, cx| {
                w.with_photo(cx, |p, cx| {
                    if p.tool == Tool::Mask {
                        p.mask_overlay = !p.mask_overlay;
                        cx.notify();
                    }
                })
            }))
            .on_action(cx.listener(|w, _: &SwapAspect, _, cx| w.swap_aspect(cx)))
            .on_action(cx.listener(|w, _: &Export, window, cx| w.open_export(window, cx)))
            .on_action(cx.listener(|w, _: &Cancel, _, cx| w.cancel(cx)))
            .on_action(cx.listener(|w, _: &LanguageEnglish, window, cx| {
                w.change_language(crate::i18n::Language::English, window, cx)
            }))
            .on_action(cx.listener(|w, _: &LanguageChinese, window, cx| {
                w.change_language(crate::i18n::Language::Chinese, window, cx)
            }))
            .on_action(cx.listener(|_, _: &DisplayAuto, window, cx| {
                set_display(crate::display::Mode::Auto, window, cx)
            }))
            .on_action(cx.listener(|_, _: &DisplaySrgb, window, cx| {
                set_display(crate::display::Mode::Srgb, window, cx)
            }))
            .on_action(
                cx.listener(|w, _: &EditKeybindings, window, cx| w.open_keybindings(window, cx)),
            )
            .on_action(cx.listener(|_, _: &ReloadKeybindings, window, cx| {
                match crate::keymap::reload(cx) {
                    Ok(()) => window.push_notification(crate::i18n::t("keybindings.reloaded"), cx),
                    Err(error) => window.push_notification(
                        format!("{}: {error:#}", crate::i18n::t("keybindings.failed")),
                        cx,
                    ),
                };
            }))
            .child(self.toolbar(cx))
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .child(body)
                    .when_some(dialog, |d, e| d.child(e))
                    .when_some(copy_dialog, |d, e| d.child(e))
                    .when_some(rename_dialog, |d, e| d.child(e)),
            )
    }
}
