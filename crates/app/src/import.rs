//! Import dialog: copy photos from a memory card or folder into a destination folder, into
//! subfolders and under file names made from templates, optionally into a backup folder
//! too. The copying itself is [`aigle_library::import`]; the workspace runs it.

use std::path::{Path, PathBuf};

use aigle_library::import::{self, DEFAULT_FOLDERS, DEFAULT_NAMES, ImportOptions, Source, Volume};
use gpui::{
    Context, Entity, EventEmitter, IntoElement, ParentElement, PathPromptOptions, Render,
    SharedString, Styled, Subscription, Task, Window, div, prelude::*, px,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::checkbox::Checkbox;
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::{ActiveTheme, Selectable, Sizable, h_flex, v_flex};
use serde::{Deserialize, Serialize};

use crate::i18n::t;

/// The choices of the last import, offered again.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub source: Option<PathBuf>,
    pub dest: Option<PathBuf>,
    pub folders: String,
    pub names: String,
    pub backup_on: bool,
    pub backup: Option<PathBuf>,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            source: None,
            dest: None,
            folders: DEFAULT_FOLDERS.into(),
            names: DEFAULT_NAMES.into(),
            backup_on: false,
            backup: None,
        }
    }
}

fn settings_path() -> Option<PathBuf> {
    crate::config_dir().map(|d| d.join("import_settings.json"))
}

fn load(path: Option<&Path>) -> Settings {
    let Some(bytes) = path.and_then(|p| std::fs::read(p).ok()) else {
        return Settings::default();
    };
    serde_json::from_slice(&bytes).unwrap_or_else(|e| {
        tracing::warn!("import settings: {e}");
        Settings::default()
    })
}

fn save(path: Option<&Path>, settings: &Settings) -> std::io::Result<()> {
    let path = path.ok_or_else(|| std::io::Error::other("no configuration folder"))?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(settings)?)?;
    std::fs::rename(&tmp, path)
}

/// `~/Pictures`, where photos are usually kept.
fn default_dest() -> PathBuf {
    std::env::var_os("HOME")
        .map(|h| PathBuf::from(h).join("Pictures"))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// What to import, and how.
#[derive(Clone, Debug)]
pub struct Request {
    pub source: PathBuf,
    pub options: ImportOptions,
}

/// The import options of `settings`; why they cannot be used otherwise.
pub fn request(settings: &Settings) -> Result<Request, String> {
    let source = settings
        .source
        .clone()
        .ok_or_else(|| t("import.no_source"))?;
    let dest = settings.dest.clone().unwrap_or_else(default_dest);
    let backup = match (settings.backup_on, &settings.backup) {
        (false, _) => None,
        (true, None) => return Err(t("import.no_backup")),
        (true, Some(b)) if *b == dest || b.starts_with(&dest) || dest.starts_with(b) => {
            return Err(t("import.backup_overlaps"));
        }
        (true, Some(b)) => Some(b.clone()),
    };
    let options = ImportOptions {
        dest,
        folders: settings.folders.trim().to_string(),
        names: match settings.names.trim() {
            "" => DEFAULT_NAMES.to_string(),
            n => n.to_string(),
        },
        backup,
    };
    import::check(&options)
        .map_err(|e| rust_i18n::t!("import.bad_template", e = e).into_owned())?;
    Ok(Request { source, options })
}

/// `1.5 GB`.
pub fn size_text(bytes: u64) -> String {
    let b = bytes as f64;
    if b >= 1e9 {
        format!("{:.1} GB", b / 1e9)
    } else if b >= 1e6 {
        format!("{:.0} MB", b / 1e6)
    } else {
        format!("{:.0} KB", (b / 1e3).ceil())
    }
}

pub enum ImportEvent {
    /// The dialog closed; `Some` when the user confirmed.
    Done(Option<Request>),
}

/// Asks for a folder for one of the dialog's settings.
type Choose = fn(&mut ImportDialog, &mut Context<ImportDialog>);

/// What the chosen source holds.
enum Found {
    Reading,
    Photos {
        count: usize,
        bytes: u64,
        first: Option<Box<Source>>,
    },
    Failed(String),
}

pub struct ImportDialog {
    settings: Settings,
    settings_path: Option<PathBuf>,
    volumes: Vec<Volume>,
    found: Option<Found>,
    folders: Entity<InputState>,
    names: Entity<InputState>,
    error: Option<String>,
    scan: Option<Task<()>>,
    prompt: Option<Task<()>>,
    _subs: Vec<Subscription>,
}

impl EventEmitter<ImportEvent> for ImportDialog {}

impl ImportDialog {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> ImportDialog {
        let settings_path = settings_path();
        let mut settings = load(settings_path.as_deref());
        let volumes = import::removable_volumes();
        // A camera card that is plugged in is the likely source.
        if let Some(v) = volumes.iter().find(|v| v.camera) {
            settings.source = Some(v.path.clone());
        }
        let input =
            |value: &str, placeholder: &str, window: &mut Window, cx: &mut Context<Self>| {
                let value = value.to_string();
                let placeholder = placeholder.to_string();
                cx.new(|cx| {
                    let mut s = InputState::new(window, cx).placeholder(placeholder);
                    s.set_value(value, window, cx);
                    s
                })
            };
        let folders = input(&settings.folders, DEFAULT_FOLDERS, window, cx);
        let names = input(&settings.names, DEFAULT_NAMES, window, cx);
        let bind =
            |input: &Entity<InputState>, set: fn(&mut Settings, String), cx: &mut Context<Self>| {
                cx.subscribe(input, move |d, input, ev: &InputEvent, cx| {
                    if let InputEvent::Change = ev {
                        set(&mut d.settings, input.read(cx).value().to_string());
                        d.error = None;
                        cx.notify();
                    }
                })
            };
        let subs = vec![
            bind(&folders, |s, v| s.folders = v, cx),
            bind(&names, |s, v| s.names = v, cx),
        ];
        let mut d = ImportDialog {
            settings,
            settings_path,
            volumes,
            found: None,
            folders,
            names,
            error: None,
            scan: None,
            prompt: None,
            _subs: subs,
        };
        d.read_source(cx);
        d
    }

    /// Count the photos of the source in the background.
    fn read_source(&mut self, cx: &mut Context<Self>) {
        let Some(source) = self.settings.source.clone() else {
            self.found = None;
            self.scan = None;
            return;
        };
        self.found = Some(Found::Reading);
        self.scan = Some(cx.spawn(async move |this, cx| {
            let found = cx
                .background_spawn(async move {
                    match aigle_library::scan_recursive(&source) {
                        Ok(entries) => Found::Photos {
                            count: entries.len(),
                            bytes: entries.iter().map(|e| e.bytes).sum(),
                            first: entries
                                .into_iter()
                                .min_by_key(|e| e.modified)
                                .map(|entry| Box::new(Source { entry, facts: None })),
                        },
                        Err(e) => Found::Failed(e.to_string()),
                    }
                })
                .await;
            let _ = this.update(cx, |d, cx| {
                d.found = Some(found);
                cx.notify();
            });
        }));
    }

    fn set_source(&mut self, source: PathBuf, cx: &mut Context<Self>) {
        self.settings.source = Some(source);
        self.error = None;
        self.read_source(cx);
        cx.notify();
    }

    /// Ask for a folder and pass it to `set`.
    fn choose(
        &mut self,
        prompt: &str,
        set: fn(&mut Self, PathBuf, &mut Context<Self>),
        cx: &mut Context<Self>,
    ) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(t(prompt).into()),
        });
        self.prompt = Some(cx.spawn(async move |this, cx| {
            let chosen = rx.await;
            let _ = this.update(cx, |d, cx| {
                d.prompt = None;
                if let Ok(Ok(Some(paths))) = chosen
                    && let Some(p) = paths.into_iter().next()
                {
                    set(d, p, cx);
                }
                cx.notify();
            });
        }));
    }

    fn confirm(&mut self, cx: &mut Context<Self>) {
        if matches!(self.found, Some(Found::Photos { count: 0, .. })) {
            self.error = Some(t("import.nothing_found"));
            cx.notify();
            return;
        }
        match request(&self.settings) {
            Ok(r) => {
                let mut kept = self.settings.clone();
                kept.dest = Some(r.options.dest.clone());
                if let Err(e) = save(self.settings_path.as_deref(), &kept) {
                    tracing::warn!("import settings: {e}");
                }
                cx.emit(ImportEvent::Done(Some(r)));
            }
            Err(e) => {
                self.error = Some(e);
                cx.notify();
            }
        }
    }

    /// Where the first photo of the source would go, shown as an example.
    fn example(&self) -> Option<String> {
        let Some(Found::Photos {
            first: Some(first), ..
        }) = &self.found
        else {
            return None;
        };
        let r = request(&self.settings).ok()?;
        let planned = import::plan(std::slice::from_ref(first.as_ref()), &r.options).ok()?;
        Some(
            r.options
                .dest
                .join(&planned.first()?.relative)
                .display()
                .to_string(),
        )
    }
}

impl Render for ImportDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let max_h = window.viewport_size().height - px(48.0);
        let theme = cx.theme();
        let (muted, bg, border, danger) = (
            theme.muted_foreground,
            theme.popover,
            theme.border,
            theme.danger,
        );
        let label = |s: String| div().text_xs().text_color(muted).child(s);
        let path_row =
            |id: &'static str, path: Option<&Path>, on: Choose, cx: &mut Context<Self>| {
                h_flex()
                    .gap_2()
                    .child(div().flex_1().min_w_0().text_xs().truncate().child(
                        path.map_or_else(|| t("import.none_chosen"), |p| p.display().to_string()),
                    ))
                    .child(
                        Button::new(id)
                            .label(t("export.change_dir"))
                            .small()
                            .on_click(cx.listener(move |d, _, _, cx| on(d, cx))),
                    )
            };
        let s = self.settings.clone();
        let dest = s.dest.clone().unwrap_or_else(default_dest);
        let found = match &self.found {
            None => None,
            Some(Found::Reading) => Some(t("import.reading")),
            Some(Found::Photos { count: 0, .. }) => Some(t("import.nothing_found")),
            Some(Found::Photos { count, bytes, .. }) => Some(
                rust_i18n::t!("import.found", arg0 = count, size = size_text(*bytes)).into_owned(),
            ),
            Some(Found::Failed(e)) => {
                Some(rust_i18n::t!("library.read_folder_failed", e = e).into_owned())
            }
        };
        let volumes = self.volumes.iter().enumerate().map(|(i, v)| {
            let path = v.path.clone();
            Button::new(SharedString::from(format!("import-volume-{i}")))
                .label(v.name.clone())
                .small()
                .selected(s.source.as_ref() == Some(&v.path))
                .on_click(cx.listener(move |d, _, _, cx| d.set_source(path.clone(), cx)))
        });
        let example = self.example();
        let card = v_flex()
            .id("import-card")
            .w(px(520.0))
            .max_h(max_h)
            .overflow_y_scroll()
            .p_4()
            .gap_3()
            .rounded_lg()
            .border_1()
            .border_color(border)
            .bg(bg)
            .shadow_lg()
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .text_base()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(t("import.title")),
            )
            .child(label(t("import.from")))
            .child(
                h_flex().gap_1().flex_wrap().children(volumes).child(
                    Button::new("import-source")
                        .label(t("import.choose_folder"))
                        .small()
                        .on_click(cx.listener(|d, _, _, cx| {
                            d.choose("import.from", |d, p, cx| d.set_source(p, cx), cx)
                        })),
                ),
            )
            .child(
                div().text_xs().truncate().child(
                    s.source
                        .as_deref()
                        .map_or_else(|| t("import.none_chosen"), |p| p.display().to_string()),
                ),
            )
            .when_some(found, |d, f| {
                d.child(div().text_xs().text_color(muted).child(f))
            })
            .child(label(t("import.to")))
            .child(path_row(
                "import-dest",
                Some(&dest),
                |d, cx| {
                    d.choose(
                        "import.to",
                        |d, p, cx| {
                            d.settings.dest = Some(p);
                            d.error = None;
                            cx.notify();
                        },
                        cx,
                    )
                },
                cx,
            ))
            .child(label(t("import.folders")))
            .child(Input::new(&self.folders).small())
            .child(label(t("import.names")))
            .child(Input::new(&self.names).small())
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child(t("import.fields_help")),
            )
            .when_some(example, |d, e| {
                d.child(
                    div()
                        .text_xs()
                        .text_color(muted)
                        .truncate()
                        .child(rust_i18n::t!("import.example", path = e).into_owned()),
                )
            })
            .child(
                Checkbox::new("import-backup")
                    .label(t("import.backup"))
                    .checked(s.backup_on)
                    .on_click(cx.listener(|d, on: &bool, _, cx| {
                        d.settings.backup_on = *on;
                        d.error = None;
                        cx.notify();
                    })),
            )
            .when(s.backup_on, |d| {
                d.child(path_row(
                    "import-backup-dir",
                    s.backup.as_deref(),
                    |d, cx| {
                        d.choose(
                            "import.backup",
                            |d, p, cx| {
                                d.settings.backup = Some(p);
                                d.error = None;
                                cx.notify();
                            },
                            cx,
                        )
                    },
                    cx,
                ))
            })
            .when_some(self.error.clone(), |d, e| {
                d.child(div().text_xs().text_color(danger).child(e))
            })
            .child(
                h_flex()
                    .pt_2()
                    .gap_2()
                    .justify_end()
                    .child(
                        Button::new("import-cancel")
                            .label(t("common.cancel"))
                            .small()
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(ImportEvent::Done(None)))),
                    )
                    .child(
                        Button::new("import-go")
                            .label(t("import.import"))
                            .small()
                            .primary()
                            .on_click(cx.listener(|d, _, _, cx| {
                                if d.prompt.is_none() {
                                    d.confirm(cx)
                                }
                            })),
                    ),
            );
        div()
            .id("import-backdrop")
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(gpui::rgba(0x00000080))
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|_, _, _, cx| cx.emit(ImportEvent::Done(None))),
            )
            .child(card)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_need_a_source_and_a_separate_backup() {
        let mut s = Settings {
            dest: Some("/photos".into()),
            ..Settings::default()
        };
        assert!(request(&s).is_err());
        s.source = Some("/media/me/CARD".into());
        let r = request(&s).unwrap();
        assert_eq!(r.options.folders, DEFAULT_FOLDERS);
        assert_eq!(r.options.backup, None);
        s.backup_on = true;
        assert!(request(&s).is_err());
        s.backup = Some("/photos/backup".into());
        assert!(request(&s).is_err());
        s.backup = Some("/mnt/backup".into());
        assert_eq!(
            request(&s).unwrap().options.backup,
            Some(PathBuf::from("/mnt/backup"))
        );
        s.names = "{nope}".into();
        assert!(request(&s).is_err());
        s.names = "  ".into();
        assert_eq!(request(&s).unwrap().options.names, DEFAULT_NAMES);
    }

    #[test]
    fn settings_round_trip() {
        let dir = std::env::temp_dir().join(format!("aigle-import-{}", std::process::id()));
        let path = dir.join("import.json");
        let s = Settings {
            source: Some("/media/me/CARD".into()),
            folders: "{year}".into(),
            backup_on: true,
            backup: Some("/mnt/b".into()),
            ..Settings::default()
        };
        save(Some(&path), &s).unwrap();
        assert_eq!(load(Some(&path)), s);
        assert_eq!(load(None), Settings::default());
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(size_text(1_500_000_000), "1.5 GB");
        assert_eq!(size_text(2_400_000), "2 MB");
        assert_eq!(size_text(10), "1 KB");
    }
}
