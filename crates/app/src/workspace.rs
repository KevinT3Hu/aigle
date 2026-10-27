//! Main window: title bar with the toolbar, then either the library grid or the develop view
//! (history sidebar, canvas, adjustment panel, filmstrip).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use aigle_core::EditParams;
use aigle_core::params::Aspect;
use aigle_engine_api::{Engine, SourceRef};
use gpui::{
    App, Context, Entity, FocusHandle, IntoElement, ParentElement, PathPromptOptions, Render,
    Styled, Subscription, Task, Window, div, prelude::*, px,
};
use gpui_component::button::{Button, ButtonGroup};
use gpui_component::menu::DropdownMenu;
use gpui_component::{ActiveTheme, Selectable, Sizable, TitleBar, WindowExt, h_flex, v_flex};

use crate::actions::*;
use crate::canvas::{Canvas, Compare};
use crate::copy_settings::{Clipboard, CopyDialog, CopyEvent, Group};
use crate::export::{self, ExportDialog, ExportEvent, Request, Settings};
use crate::filmstrip::Filmstrip;
use crate::grid::Grid;
use crate::history::HistoryPanel;
use crate::keybinding_editor::{Closed as KeybindingsClosed, KeybindingEditor};
use crate::library::{LABELS, Library, LibraryEvent};
use crate::panel::Panel;
use crate::photo::{Photo, PhotoEvent, Tool};
use crate::resources::Resources;

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
    _edits: Subscription,
}

pub struct Workspace {
    engine: Engine,
    resources: Entity<Resources>,
    library: Entity<Library>,
    grid: Entity<Grid>,
    filmstrip: Entity<Filmstrip>,
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
    /// Running exports by file name.
    exports: Vec<(u64, String, Task<()>)>,
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
        let library = cx.new(|cx| Library::new(engine.clone(), cx));
        let grid = cx.new(|cx| Grid::new(library.clone(), cx));
        let filmstrip = cx.new(|cx| Filmstrip::new(library.clone(), cx));
        let subs = vec![
            cx.subscribe_in(&library, window, |w, _, e, window, cx| match e {
                LibraryEvent::Open(i) => w.open_index(*i, window, cx),
                LibraryEvent::Notice(m) => window.push_notification(m.clone(), cx),
            }),
            cx.observe(&library, |_, _, cx| cx.notify()),
            cx.on_app_quit(|w, cx| {
                w.library.read(cx).flush();
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
            exports: Vec::new(),
            next_export: 0,
            _subs: subs,
        };
        match source {
            Some(SourceRef::File(p)) => ws.open_path(p, window, cx),
            Some(s) => ws.open_source(s, None, Vec::new(), window, cx),
            None => ws.update_title(window, cx),
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
        self.open_source(
            SourceRef::File(path),
            initial.edits,
            initial.snapshots,
            window,
            cx,
        );
    }

    fn open_index(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let (path, meta) = self.library.update(cx, |l, cx| {
            l.select(index, cx);
            let path = l.items.get(index).map(|it| it.entry.path.clone());
            (path, l.meta_now(index))
        });
        let Some(path) = path else { return };
        let already = self
            .open
            .as_ref()
            .is_some_and(|o| o.photo.read(cx).path.as_deref() == Some(path.as_path()));
        if already {
            self.mode = Mode::Develop;
            self.update_title(window, cx);
            cx.notify();
            return;
        }
        self.open_source(
            SourceRef::File(path),
            meta.edits,
            meta.snapshots,
            window,
            cx,
        );
    }

    fn open_source(
        &mut self,
        source: SourceRef,
        edits: Option<aigle_core::EditParams>,
        snapshots: Vec<aigle_library::Snapshot>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_photo(cx);
        let engine = self.engine.clone();
        let photo =
            cx.new(|cx| Photo::open(&engine, source, edits.unwrap_or_default(), snapshots, cx));
        let canvas = cx.new(|cx| Canvas::new(photo.clone(), window, cx));
        let hist = canvas.read(cx).histogram();
        let panel = cx.new(|cx| Panel::new(photo.clone(), hist, window, cx));
        let history = cx.new(|cx| HistoryPanel::new(photo.clone(), cx));
        let library = self.library.clone();
        let edits = cx.subscribe(&photo, move |_, photo, e, cx| match e {
            PhotoEvent::Edited => {
                let p = photo.read(cx);
                if let Some(path) = p.path.clone() {
                    let head = p.head().clone();
                    library.update(cx, |l, cx| l.photo_edited(&path, &head, cx));
                }
            }
            PhotoEvent::SnapshotsChanged => {
                let p = photo.read(cx);
                if let Some(path) = p.path.clone() {
                    let snapshots = p.snapshots.clone();
                    library.update(cx, |l, _| l.set_snapshots(&path, snapshots));
                }
            }
            _ => {}
        });
        self.open = Some(OpenPhoto {
            photo,
            canvas,
            panel,
            history,
            _edits: edits,
        });
        self.mode = Mode::Develop;
        self.update_title(window, cx);
        cx.notify();
    }

    fn close_photo(&mut self, cx: &mut Context<Self>) {
        if self.open.take().is_some() {
            self.library.read(cx).flush();
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
            let open_path = self
                .open
                .as_ref()
                .and_then(|o| o.photo.read(cx).path.clone());
            let selected_path = self
                .library
                .read(cx)
                .selected_item()
                .map(|it| it.entry.path.clone());
            if let Some(i) = selected
                && selected_path != open_path
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
            prompt: Some(crate::i18n::t("打开").into()),
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
                    let n = l.items.len() as isize;
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

    /// The photo ratings and labels apply to: the selection, which develop keeps on the
    /// open photo.
    fn rate(&mut self, rating: i8, cx: &mut Context<Self>) {
        self.library.update(cx, |l, cx| {
            if let Some(i) = l.selected {
                l.set_rating(i, rating, cx);
            }
        });
    }

    fn label(&mut self, n: usize, cx: &mut Context<Self>) {
        self.library.update(cx, |l, cx| {
            if let Some(i) = l.selected {
                l.toggle_label(i, LABELS[n].0, cx);
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
                format!("{}: {error}", crate::i18n::t("无法保存语言设置")),
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
        if self.export_dialog.take().is_some() || self.copy_dialog.take().is_some() {
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

    /// What export applies to: the open photo in develop, the selection in the grid.
    fn export_request(&mut self, cx: &mut Context<Self>) -> Option<Request> {
        if self.mode == Mode::Develop
            && let Some(o) = &self.open
        {
            let p = o.photo.read(cx);
            let source = p.source.clone();
            return Some(Request {
                source,
                params: p.head().clone(),
                name: p.name.clone(),
                folder: p
                    .path
                    .as_ref()
                    .and_then(|f| f.parent().map(Path::to_path_buf)),
            });
        }
        let index = self.library.read(cx).selected?;
        let meta = self.library.update(cx, |l, _| l.meta_now(index));
        let l = self.library.read(cx);
        let it = l.items.get(index)?;
        Some(Request {
            source: SourceRef::File(it.entry.path.clone()),
            params: Arc::new(meta.edits.unwrap_or_default()),
            name: it.entry.name.clone(),
            folder: l.dir.clone(),
        })
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

    fn open_copy(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(params) = self.current_edits(cx) else {
            window.push_notification(crate::i18n::t("没有可拷贝设置的照片"), cx);
            return;
        };
        self.export_dialog = None;
        let dialog = cx.new(|_| CopyDialog::new(self.copy_groups.clone()));
        let sub = cx.subscribe_in(&dialog, window, move |w, _, ev, window, cx| {
            let CopyEvent::Done(chosen) = ev;
            w.copy_dialog = None;
            if let Some(groups) = chosen {
                w.copy_groups = groups.clone();
                w.clipboard = Some(Clipboard {
                    params: params.clone(),
                    groups: groups.clone(),
                });
                window.push_notification(crate::i18n::t("已拷贝设置"), cx);
            }
            window.focus(&w.focus, cx);
            cx.notify();
        });
        self.copy_dialog = Some((dialog, sub));
        cx.notify();
    }

    /// Paste onto the open photo in develop (one undo step), or onto the grid selection.
    fn paste(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(clip) = self.clipboard.clone() else {
            window.push_notification(crate::i18n::t("尚未拷贝设置"), cx);
            return;
        };
        let target = match self.mode {
            Mode::Develop => self.open.as_ref().map(|o| o.photo.clone()),
            Mode::Library => {
                let selected = self
                    .library
                    .read(cx)
                    .selected_item()
                    .map(|it| it.entry.path.clone());
                self.open
                    .as_ref()
                    .filter(|o| {
                        selected.is_some() && o.photo.read(cx).path.as_ref() == selected.as_ref()
                    })
                    .map(|o| o.photo.clone())
            }
        };
        if let Some(photo) = target {
            photo.update(cx, |p, cx| {
                p.command(
                    crate::i18n::t("粘贴设置"),
                    clip.ops(),
                    None,
                    aigle_engine_api::Interaction::Final,
                    cx,
                );
            });
            return;
        }
        if self.mode != Mode::Library {
            return;
        }
        let Some(index) = self.library.read(cx).selected else {
            return;
        };
        let (path, meta) = self.library.update(cx, |l, _| {
            let path = l.items.get(index).map(|it| it.entry.path.clone());
            (path, l.meta_now(index))
        });
        let Some(path) = path else { return };
        match clip.paste_onto(&meta.edits.unwrap_or_default()) {
            Ok(next) => self
                .library
                .update(cx, |l, cx| l.photo_edited(&path, &Arc::new(next), cx)),
            Err(e) => window.push_notification(e.to_string(), cx),
        }
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
                .title(crate::i18n::t("快捷键"))
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
        let Some(request) = self.export_request(cx) else {
            window.push_notification(crate::i18n::t("没有可导出的照片"), cx);
            return;
        };
        let settings = self.export_settings.clone();
        let dialog = cx.new(|cx| ExportDialog::new(request, settings, window, cx));
        let sub = cx.subscribe_in(&dialog, window, |w, _, ev, window, cx| {
            let ExportEvent::Done(chosen) = ev;
            w.export_dialog = None;
            if let Some((request, settings, path)) = chosen {
                w.export_settings = settings.clone();
                w.start_export(request, settings, path.clone(), window, cx);
            }
            window.focus(&w.focus, cx);
            cx.notify();
        });
        self.export_dialog = Some((dialog, sub));
        cx.notify();
    }

    fn start_export(
        &mut self,
        request: &Request,
        settings: &Settings,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let job = export::run(&self.engine, request, settings, path);
        let id = self.next_export;
        self.next_export += 1;
        let shown = name.clone();
        let task = cx.spawn_in(window, async move |this, cx| {
            let r = job.await;
            let _ = this.update_in(cx, |w, window, cx| {
                w.exports.retain(|(i, _, _)| *i != id);
                let msg = match r {
                    Ok(done) => rust_i18n::t!(
                        "已导出 {shown}（{}×{}）",
                        arg0 = done.width,
                        arg1 = done.height,
                        shown = shown
                    )
                    .into_owned(),
                    Err(e) => {
                        rust_i18n::t!("导出 {shown} 失败：{e}", shown = shown, e = e).into_owned()
                    }
                };
                window.push_notification(msg, cx);
                cx.notify();
            });
        });
        self.exports.push((id, name, task));
        cx.notify();
    }

    fn toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let develop = self.mode == Mode::Develop;
        let (can_undo, can_redo, show_info, compare, tool) = match (&self.open, develop) {
            (Some(o), true) => {
                let p = o.photo.read(cx);
                let c = o.canvas.read(cx);
                (
                    p.doc.can_undo(),
                    p.doc.can_redo(),
                    c.show_info(),
                    c.compare(),
                    p.tool,
                )
            }
            _ => (false, false, false, Compare::Off, Tool::None),
        };
        let show_history = self.show_history;
        let can_paste = self.clipboard.is_some();
        let mask_overlay = self
            .open
            .as_ref()
            .is_some_and(|o| o.photo.read(cx).mask_overlay);
        let title = {
            let l = self.library.read(cx);
            match (&self.open, develop) {
                (Some(o), true) => o.photo.read(cx).name.clone(),
                _ if l.dir.is_some() => {
                    rust_i18n::t!("{} · {} 张", arg0 = l.folder_name(), arg1 = l.items.len())
                        .into_owned()
                }
                _ => String::new(),
            }
        };
        let file_focus = self.focus.clone();
        let edit_focus = self.focus.clone();
        let view_focus = self.focus.clone();
        let language = crate::i18n::language();
        let row = h_flex()
            .flex_1()
            .h_full()
            .gap_2()
            .pr_2()
            .child(
                Button::new("file-menu")
                    .label(crate::i18n::t("文件"))
                    .small()
                    .dropdown_menu(move |menu, _, _| {
                        menu.action_context(file_focus.clone())
                            .menu(crate::i18n::t("打开照片…"), Box::new(Open))
                            .menu(crate::i18n::t("打开文件夹…"), Box::new(OpenFolder))
                            .separator()
                            .menu(crate::i18n::t("导出…"), Box::new(Export))
                            .separator()
                            .menu(crate::i18n::t("退出"), Box::new(Quit))
                    }),
            )
            .child(
                Button::new("edit-menu")
                    .label(crate::i18n::t("编辑"))
                    .small()
                    .dropdown_menu(move |menu, window, cx| {
                        menu.action_context(edit_focus.clone())
                            .menu_with_disabled(crate::i18n::t("撤销"), Box::new(Undo), !can_undo)
                            .menu_with_disabled(crate::i18n::t("重做"), Box::new(Redo), !can_redo)
                            .separator()
                            .menu(crate::i18n::t("拷贝设置…"), Box::new(CopySettings))
                            .menu_with_disabled(
                                crate::i18n::t("粘贴设置"),
                                Box::new(PasteSettings),
                                !can_paste,
                            )
                            .separator()
                            .submenu(crate::i18n::t("工具"), window, cx, move |menu, _, _| {
                                menu.menu_with_check_and_disabled(
                                    crate::i18n::t("裁剪与拉直"),
                                    tool == Tool::Crop,
                                    Box::new(ToggleCrop),
                                    !develop,
                                )
                                .menu_with_check_and_disabled(
                                    crate::i18n::t("蒙版"),
                                    tool == Tool::Mask,
                                    Box::new(ToggleMask),
                                    !develop,
                                )
                                .menu_with_disabled(
                                    crate::i18n::t("白平衡吸管"),
                                    Box::new(ToggleWbPicker),
                                    !develop,
                                )
                            })
                    }),
            )
            .child(
                Button::new("view-menu")
                    .label(crate::i18n::t("视图"))
                    .small()
                    .dropdown_menu(move |menu, window, cx| {
                        menu.action_context(view_focus.clone())
                            .menu_with_check(crate::i18n::t("图库"), !develop, Box::new(ShowGrid))
                            .menu_with_check(crate::i18n::t("修图"), develop, Box::new(ShowDevelop))
                            .separator()
                            .submenu(crate::i18n::t("对比"), window, cx, move |menu, _, _| {
                                menu.menu_with_check_and_disabled(
                                    crate::i18n::t("原图"),
                                    compare == Compare::Before,
                                    Box::new(ToggleBefore),
                                    !develop,
                                )
                                .menu_with_check_and_disabled(
                                    crate::i18n::t("分屏对比"),
                                    compare == Compare::Split,
                                    Box::new(ToggleSplit),
                                    !develop,
                                )
                            })
                            .menu_with_disabled(
                                crate::i18n::t("缩放 / 适合窗口"),
                                Box::new(ToggleZoom),
                                !develop,
                            )
                            .separator()
                            .menu_with_check_and_disabled(
                                crate::i18n::t("历史记录"),
                                show_history,
                                Box::new(ToggleHistory),
                                !develop,
                            )
                            .menu_with_check_and_disabled(
                                crate::i18n::t("拍摄参数"),
                                show_info,
                                Box::new(ToggleInfo),
                                !develop,
                            )
                            .menu_with_check_and_disabled(
                                crate::i18n::t("蒙版叠加"),
                                mask_overlay,
                                Box::new(ToggleOverlay),
                                tool != Tool::Mask,
                            )
                            .separator()
                            .submenu(crate::i18n::t("快捷键"), window, cx, |menu, _, _| {
                                menu.menu(crate::i18n::t("编辑快捷键…"), Box::new(EditKeybindings))
                                    .menu(
                                        crate::i18n::t("重新加载快捷键"),
                                        Box::new(ReloadKeybindings),
                                    )
                            })
                            .separator()
                            .submenu(crate::i18n::t("语言"), window, cx, move |menu, _, _| {
                                menu.menu_with_check(
                                    "English",
                                    language == crate::i18n::Language::English,
                                    Box::new(LanguageEnglish),
                                )
                                .menu_with_check(
                                    "简体中文",
                                    language == crate::i18n::Language::Chinese,
                                    Box::new(LanguageChinese),
                                )
                            })
                    }),
            )
            .child(
                ButtonGroup::new("mode")
                    .small()
                    .child(
                        Button::new("grid")
                            .label(crate::i18n::t("图库"))
                            .selected(!develop)
                            .tooltip(crate::keymap::hint("缩略图网格（G）", cx))
                            .on_click(cx.listener(|w, _, window, cx| {
                                w.set_mode(Mode::Library, window, cx)
                            })),
                    )
                    .child(
                        Button::new("develop")
                            .label(crate::i18n::t("修图"))
                            .selected(develop)
                            .tooltip(crate::keymap::hint(
                                "修图（D，或在网格中按 Enter / 双击）",
                                cx,
                            ))
                            .on_click(cx.listener(|w, _, window, cx| {
                                w.set_mode(Mode::Develop, window, cx)
                            })),
                    ),
            )
            .when(!self.exports.is_empty(), |row| {
                row.child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(
                            rust_i18n::t!("正在导出 {} 张", arg0 = self.exports.len()).into_owned(),
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
                .child(crate::keymap::hint(
                    "在图库中选择一张照片，或打开一张照片（Ctrl+O）",
                    cx,
                ))
                .into_any_element();
        };
        let has_strip = !self.library.read(cx).items.is_empty();
        v_flex()
            .flex_1()
            .min_h_0()
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .when(self.show_history, |d| {
                        d.child(
                            div()
                                .w(px(200.0))
                                .h_full()
                                .flex_none()
                                .border_r_1()
                                .border_color(border)
                                .child(o.history.clone()),
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
            Mode::Library => div()
                .flex_1()
                .min_h_0()
                .child(self.grid.clone())
                .into_any_element(),
            Mode::Develop => self.develop(cx),
        };
        let dialog = self.export_dialog.as_ref().map(|(d, _)| d.clone());
        let copy_dialog = self.copy_dialog.as_ref().map(|(d, _)| d.clone());
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
            .on_action(
                cx.listener(|w, _: &ToggleZoom, _, cx| w.with_canvas(cx, Canvas::toggle_zoom)),
            )
            .on_action(
                cx.listener(|w, _: &ToggleBefore, _, cx| w.with_canvas(cx, Canvas::toggle_before)),
            )
            .on_action(
                cx.listener(|w, _: &ToggleSplit, _, cx| w.with_canvas(cx, Canvas::toggle_split)),
            )
            .on_action(cx.listener(|w, _: &ToggleWbPicker, _, cx| {
                w.with_photo(cx, |p, cx| p.set_wb_pick(!p.wb_pick, cx))
            }))
            .on_action(
                cx.listener(|w, _: &ToggleInfo, _, cx| w.with_canvas(cx, Canvas::toggle_info)),
            )
            .on_action(cx.listener(|w, _: &ToggleHistory, _, cx| w.toggle_history(cx)))
            .on_action(cx.listener(|w, _: &ToggleCrop, _, cx| w.toggle_tool(Tool::Crop, cx)))
            .on_action(cx.listener(|w, _: &ToggleMask, _, cx| w.toggle_tool(Tool::Mask, cx)))
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
            .on_action(
                cx.listener(|w, _: &EditKeybindings, window, cx| w.open_keybindings(window, cx)),
            )
            .on_action(cx.listener(|_, _: &ReloadKeybindings, window, cx| {
                match crate::keymap::reload(cx) {
                    Ok(()) => window.push_notification(crate::i18n::t("快捷键已重新加载"), cx),
                    Err(error) => window.push_notification(
                        format!("{}: {error:#}", crate::i18n::t("快捷键操作失败")),
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
                    .when_some(copy_dialog, |d, e| d.child(e)),
            )
    }
}
