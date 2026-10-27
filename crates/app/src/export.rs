//! Export dialog: format, size, colour space and file names for one or many photos, with
//! named export presets; files are written by engine export jobs in the background
//! (doc 02 §5.6).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use aigle_core::EditParams;
use aigle_engine_api::{
    DisplayProfileId, EncodedFormat, Engine, ExportOptions, MetadataPolicy, OutputSharpening,
    SourceRef,
};
use gpui::{
    Context, Entity, EventEmitter, IntoElement, ParentElement, PathPromptOptions, Render,
    SharedString, Styled, Subscription, Task, Window, div, prelude::*, px,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::slider::{Slider, SliderEvent, SliderState};
use gpui_component::{ActiveTheme, Selectable, Sizable, h_flex, v_flex};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Format {
    Jpeg,
    Png,
    Tiff8,
    Tiff16,
    WebP,
}

impl Format {
    const ALL: [(Format, &'static str); 5] = [
        (Format::Jpeg, "JPEG"),
        (Format::Png, "PNG"),
        (Format::Tiff8, "TIFF 8 位"),
        (Format::Tiff16, "TIFF 16 位"),
        (Format::WebP, "WebP"),
    ];

    pub fn extension(self) -> &'static str {
        match self {
            Format::Jpeg => "jpg",
            Format::Png => "png",
            Format::Tiff8 | Format::Tiff16 => "tif",
            Format::WebP => "webp",
        }
    }

    fn lossy(self) -> bool {
        matches!(self, Format::Jpeg | Format::WebP)
    }

    pub fn encoded(self, quality: u8) -> EncodedFormat {
        match self {
            Format::Jpeg => EncodedFormat::Jpeg { quality },
            Format::Png => EncodedFormat::Png,
            Format::Tiff8 => EncodedFormat::Tiff { sixteen_bit: false },
            Format::Tiff16 => EncodedFormat::Tiff { sixteen_bit: true },
            Format::WebP => EncodedFormat::WebP { quality },
        }
    }
}

pub const DEFAULT_TEMPLATE: &str = "{name}-edit";

/// Export settings, kept between exports and saved in export presets.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub format: Format,
    pub quality: u8,
    /// `None`: full size.
    pub long_edge: Option<u32>,
    pub output: DisplayProfileId,
    /// File name without extension; see [`render_template`].
    pub template: String,
    pub sharpening: OutputSharpening,
    pub metadata: MetadataPolicy,
    /// Written instead of the camera's artist / copyright when not empty.
    pub artist: String,
    pub copyright: String,
    /// Folder of the last export; not part of presets.
    #[serde(skip)]
    pub dir: Option<PathBuf>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            format: Format::Jpeg,
            quality: 90,
            long_edge: None,
            output: DisplayProfileId::Srgb,
            template: DEFAULT_TEMPLATE.into(),
            sharpening: OutputSharpening::Off,
            metadata: MetadataPolicy::All,
            artist: String::new(),
            copyright: String::new(),
            dir: None,
        }
    }
}

impl Settings {
    /// Equal ignoring the destination folder.
    fn same_preset(&self, other: &Settings) -> bool {
        Settings {
            dir: None,
            ..self.clone()
        } == Settings {
            dir: None,
            ..other.clone()
        }
    }
}

const SIZES: [(Option<u32>, &str); 5] = [
    (None, "原尺寸"),
    (Some(6000), "6000"),
    (Some(4096), "4096"),
    (Some(2048), "2048"),
    (Some(1080), "1080"),
];

/// What to export.
#[derive(Clone)]
pub struct Request {
    pub source: SourceRef,
    pub params: Arc<EditParams>,
    pub name: String,
    /// Folder of the photo, the default destination.
    pub folder: Option<PathBuf>,
}

/// The file name (without extension) for photo `name`, number `seq` of `total`.
///
/// `{name}` is the photo's file name without extension and `{seq}` its 1-based number,
/// zero-padded to the width of `total`. Path separators become `_`; an empty result falls
/// back to the photo name.
pub fn render_template(template: &str, name: &str, seq: usize, total: usize) -> String {
    let stem = Path::new(name)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "export".into());
    let width = total.max(1).to_string().len();
    let out = template
        .replace("{name}", &stem)
        .replace("{seq}", &format!("{seq:0width$}"))
        .replace(['/', '\\', '\0'], "_");
    match out.trim() {
        "" | "." | ".." => stem,
        s => s.to_string(),
    }
}

/// `path` with the extension of `format` unless it already has a matching one.
pub fn with_extension(path: PathBuf, format: Format) -> PathBuf {
    let ok = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .is_some_and(|e| match format {
            Format::Jpeg => e == "jpg" || e == "jpeg",
            Format::Tiff8 | Format::Tiff16 => e == "tif" || e == "tiff",
            f => e == f.extension(),
        });
    if ok {
        path
    } else {
        path.with_extension(format.extension())
    }
}

/// Destination files for a batch into `dir`: names from the template, numbered `-2`, `-3`, …
/// where they would overwrite an existing file or another photo of the batch.
pub fn batch_paths(
    requests: &[Request],
    settings: &Settings,
    dir: &Path,
    exists: impl Fn(&Path) -> bool,
) -> Vec<PathBuf> {
    let ext = settings.format.extension();
    let mut taken = HashSet::new();
    requests
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let stem = render_template(&settings.template, &r.name, i + 1, requests.len());
            let path = std::iter::once(dir.join(format!("{stem}.{ext}")))
                .chain((2..).map(|n| dir.join(format!("{stem}-{n}.{ext}"))))
                .find(|p| !taken.contains(p) && !exists(p))
                .expect("unbounded");
            taken.insert(path.clone());
            path
        })
        .collect()
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExportPreset {
    pub name: String,
    pub settings: Settings,
}

fn presets_path() -> Option<PathBuf> {
    crate::config_dir().map(|d| d.join("export_presets.json"))
}

fn load_presets(path: Option<&Path>) -> Vec<ExportPreset> {
    let Some(bytes) = path.and_then(|p| std::fs::read(p).ok()) else {
        return Vec::new();
    };
    serde_json::from_slice(&bytes).unwrap_or_else(|e| {
        tracing::warn!("export presets: {e}");
        Vec::new()
    })
}

fn save_presets(path: Option<&Path>, presets: &[ExportPreset]) -> std::io::Result<()> {
    let path = path.ok_or_else(|| std::io::Error::other("no configuration folder"))?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(presets)?)?;
    std::fs::rename(&tmp, path)
}

pub enum ExportEvent {
    /// The dialog closed; `Some` with each photo's destination when the user confirmed.
    Done(Option<(Vec<(Request, PathBuf)>, Settings)>),
}

pub struct ExportDialog {
    requests: Vec<Request>,
    settings: Settings,
    quality: Entity<SliderState>,
    template: Entity<InputState>,
    artist: Entity<InputState>,
    copyright: Entity<InputState>,
    preset_name: Entity<InputState>,
    presets: Vec<ExportPreset>,
    presets_path: Option<PathBuf>,
    error: Option<String>,
    prompt: Option<Task<()>>,
    _subs: Vec<Subscription>,
}

impl EventEmitter<ExportEvent> for ExportDialog {}

impl ExportDialog {
    pub fn new(
        requests: Vec<Request>,
        settings: Settings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> ExportDialog {
        let quality = cx.new(|_| {
            SliderState::new()
                .max(100.0)
                .min(1.0)
                .step(1.0)
                .default_value(settings.quality as f32)
        });
        let template = cx.new(|cx| {
            let mut s = InputState::new(window, cx).placeholder(DEFAULT_TEMPLATE);
            s.set_value(settings.template.clone(), window, cx);
            s
        });
        let artist = cx.new(|cx| {
            let mut s = InputState::new(window, cx).placeholder(crate::i18n::t("使用相机中的信息"));
            s.set_value(settings.artist.clone(), window, cx);
            s
        });
        let copyright = cx.new(|cx| {
            let mut s = InputState::new(window, cx).placeholder(crate::i18n::t("使用相机中的信息"));
            s.set_value(settings.copyright.clone(), window, cx);
            s
        });
        let preset_name =
            cx.new(|cx| InputState::new(window, cx).placeholder(crate::i18n::t("预设名称")));
        let bind =
            |input: &Entity<InputState>, set: fn(&mut Settings, String), cx: &mut Context<Self>| {
                cx.subscribe(input, move |d, input, ev: &InputEvent, cx| {
                    if let InputEvent::Change = ev {
                        set(&mut d.settings, input.read(cx).value().to_string());
                        cx.notify();
                    }
                })
            };
        let subs = vec![
            cx.subscribe_in(&quality, window, |d, _, ev: &SliderEvent, _, cx| {
                let v = match ev {
                    SliderEvent::Change(v) | SliderEvent::Release(v) => v.start(),
                };
                d.settings.quality = v.round().clamp(1.0, 100.0) as u8;
                cx.notify();
            }),
            bind(&template, |s, v| s.template = v, cx),
            bind(&artist, |s, v| s.artist = v, cx),
            bind(&copyright, |s, v| s.copyright = v, cx),
            cx.subscribe(&preset_name, |d, _, ev: &InputEvent, cx| {
                if let InputEvent::PressEnter { .. } = ev {
                    d.save_preset(cx);
                }
            }),
        ];
        let presets_path = presets_path();
        ExportDialog {
            presets: load_presets(presets_path.as_deref()),
            presets_path,
            requests,
            settings,
            quality,
            template,
            artist,
            copyright,
            preset_name,
            error: None,
            prompt: None,
            _subs: subs,
        }
    }

    fn batch(&self) -> bool {
        self.requests.len() > 1
    }

    fn dir(&self) -> PathBuf {
        self.settings
            .dir
            .clone()
            .or_else(|| self.requests.first().and_then(|r| r.folder.clone()))
            .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
            .unwrap_or_else(|| PathBuf::from("."))
    }

    /// One photo: ask for the file. Several: export into the chosen folder.
    fn confirm(&mut self, cx: &mut Context<Self>) {
        if self.batch() {
            let dir = self.dir();
            let paths = batch_paths(&self.requests, &self.settings, &dir, Path::exists);
            let mut settings = self.settings.clone();
            settings.dir = Some(dir);
            let jobs = self.requests.iter().cloned().zip(paths).collect();
            cx.emit(ExportEvent::Done(Some((jobs, settings))));
            return;
        }
        let Some(request) = self.requests.first().cloned() else {
            return;
        };
        let name = format!(
            "{}.{}",
            render_template(&self.settings.template, &request.name, 1, 1),
            self.settings.format.extension()
        );
        let rx = cx.prompt_for_new_path(&self.dir(), Some(&name));
        self.prompt = Some(cx.spawn(async move |this, cx| {
            let chosen = rx.await;
            let _ = this.update(cx, |d, cx| {
                d.prompt = None;
                if let Ok(Ok(Some(path))) = chosen {
                    let path = with_extension(path, d.settings.format);
                    d.settings.dir = path.parent().map(Path::to_path_buf);
                    cx.emit(ExportEvent::Done(Some((
                        vec![(request, path)],
                        d.settings.clone(),
                    ))));
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn choose_dir(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(crate::i18n::t("选择").into()),
        });
        self.prompt = Some(cx.spawn(async move |this, cx| {
            let chosen = rx.await;
            let _ = this.update(cx, |d, cx| {
                d.prompt = None;
                if let Ok(Ok(Some(paths))) = chosen
                    && let Some(p) = paths.into_iter().next()
                {
                    d.settings.dir = Some(p);
                }
                cx.notify();
            });
        }));
    }

    fn apply_preset(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(p) = self.presets.get(index) else {
            return;
        };
        let dir = self.settings.dir.take();
        self.settings = Settings {
            dir,
            ..p.settings.clone()
        };
        let s = self.settings.clone();
        self.quality
            .update(cx, |q, cx| q.set_value(s.quality as f32, window, cx));
        for (input, value) in [
            (&self.template, s.template),
            (&self.artist, s.artist),
            (&self.copyright, s.copyright),
        ] {
            input.update(cx, |i, cx| i.set_value(value, window, cx));
        }
        cx.notify();
    }

    /// Save the current settings under the typed name, replacing a preset of that name.
    fn save_preset(&mut self, cx: &mut Context<Self>) {
        let name = self.preset_name.read(cx).value().trim().to_string();
        if name.is_empty() {
            self.error = Some(crate::i18n::t("请输入预设名称"));
            cx.notify();
            return;
        }
        let settings = Settings {
            dir: None,
            ..self.settings.clone()
        };
        match self.presets.iter_mut().find(|p| p.name == name) {
            Some(p) => p.settings = settings,
            None => self.presets.push(ExportPreset { name, settings }),
        }
        self.store_presets(cx);
    }

    fn remove_preset(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.presets.len() {
            self.presets.remove(index);
            self.store_presets(cx);
        }
    }

    fn store_presets(&mut self, cx: &mut Context<Self>) {
        self.error = save_presets(self.presets_path.as_deref(), &self.presets)
            .err()
            .map(|e| rust_i18n::t!("无法保存预设：{e}", e = e).into_owned());
        cx.notify();
    }

    fn choice<T: PartialEq + Clone + 'static>(
        &self,
        id: &str,
        options: Vec<(T, &'static str)>,
        current: T,
        set: fn(&mut Settings, T),
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<T> {
        h_flex()
            .gap_1()
            .flex_wrap()
            .children(options.into_iter().map(|(v, label)| {
                let selected = v == current;
                Button::new(SharedString::from(format!("{id}-{label}")))
                    .label(crate::i18n::t(label))
                    .small()
                    .selected(selected)
                    .on_click(cx.listener(move |d, _, _, cx| {
                        set(&mut d.settings, v.clone());
                        cx.notify();
                    }))
            }))
    }

    fn render_presets(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let chips = self.presets.iter().enumerate().map(|(i, p)| {
            h_flex()
                .child(
                    Button::new(SharedString::from(format!("export-preset-{i}")))
                        .label(p.name.clone())
                        .small()
                        .selected(p.settings.same_preset(&self.settings))
                        .on_click(
                            cx.listener(move |d, _, window, cx| d.apply_preset(i, window, cx)),
                        ),
                )
                .child(
                    Button::new(SharedString::from(format!("export-preset-del-{i}")))
                        .label("×")
                        .xsmall()
                        .ghost()
                        .tooltip(crate::i18n::t("删除预设"))
                        .on_click(cx.listener(move |d, _, _, cx| d.remove_preset(i, cx))),
                )
        });
        v_flex()
            .gap_1()
            .child(h_flex().gap_1().flex_wrap().children(chips))
            .child(
                h_flex()
                    .gap_1()
                    .child(div().flex_1().child(Input::new(&self.preset_name).small()))
                    .child(
                        Button::new("export-preset-save")
                            .label(crate::i18n::t("存为预设"))
                            .small()
                            .on_click(cx.listener(|d, _, _, cx| d.save_preset(cx))),
                    ),
            )
    }
}

/// Start the export job; dropping the returned job cancels it.
pub fn run(
    engine: &Engine,
    request: &Request,
    settings: &Settings,
    path: PathBuf,
) -> aigle_engine_api::Job<aigle_engine_api::ExportResult> {
    let text = |s: &str| (!s.trim().is_empty()).then(|| s.trim().to_string());
    engine.export(
        request.source.clone(),
        request.params.clone(),
        ExportOptions {
            path,
            format: settings.format.encoded(settings.quality),
            long_edge: settings.long_edge,
            output: settings.output.clone(),
            sharpening: settings.sharpening,
            metadata: settings.metadata,
            artist: text(&settings.artist),
            copyright: text(&settings.copyright),
        },
    )
}

impl Render for ExportDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let s = self.settings.clone();
        let max_h = window.viewport_size().height - px(48.0);
        let theme = cx.theme();
        let (muted, bg, border, danger) = (
            theme.muted_foreground,
            theme.popover,
            theme.border,
            theme.danger,
        );
        let label = |t: String| div().text_xs().text_color(muted).child(t);
        let busy = self.prompt.is_some();
        let batch = self.batch();
        let title = match &self.requests[..] {
            [one] => rust_i18n::t!("导出 {}", arg0 = one.name).into_owned(),
            many => rust_i18n::t!("导出 {} 张照片", arg0 = many.len()).into_owned(),
        };
        let example = self.requests.first().map(|r| {
            format!(
                "{}.{}",
                render_template(&s.template, &r.name, 1, self.requests.len()),
                s.format.extension()
            )
        });
        let dir = self.dir();
        let card = v_flex()
            .id("export-card")
            .w(px(480.0))
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
                    .child(title),
            )
            .child(label(crate::i18n::t("导出预设")))
            .child(self.render_presets(cx))
            .child(label(crate::i18n::t("格式")))
            .child(self.choice(
                "fmt",
                Format::ALL.to_vec(),
                s.format,
                |s, f| s.format = f,
                cx,
            ))
            .when(s.format.lossy(), |d| {
                d.child(
                    h_flex()
                        .justify_between()
                        .child(label(crate::i18n::t("质量")))
                        .child(div().text_xs().child(s.quality.to_string())),
                )
                .child(Slider::new(&self.quality))
            })
            .child(label(crate::i18n::t("长边（像素）")))
            .child(self.choice(
                "size",
                SIZES.to_vec(),
                s.long_edge,
                |s, v| s.long_edge = v,
                cx,
            ))
            .child(label(crate::i18n::t("色彩空间")))
            .child(self.choice(
                "cs",
                vec![
                    (DisplayProfileId::Srgb, "sRGB"),
                    (DisplayProfileId::DisplayP3, "Display P3"),
                ],
                s.output.clone(),
                |s, v| s.output = v,
                cx,
            ))
            .child(label(crate::i18n::t("输出锐化")))
            .child(self.choice(
                "sharpen",
                vec![
                    (OutputSharpening::Off, "关"),
                    (OutputSharpening::Low, "低"),
                    (OutputSharpening::Standard, "标准"),
                    (OutputSharpening::High, "高"),
                ],
                s.sharpening,
                |s, v| s.sharpening = v,
                cx,
            ))
            .child(label(crate::i18n::t("元数据")))
            .child(self.choice(
                "meta",
                vec![
                    (MetadataPolicy::All, "全部"),
                    (MetadataPolicy::CopyrightOnly, "仅版权"),
                    (MetadataPolicy::Strip, "不含"),
                ],
                s.metadata,
                |s, v| s.metadata = v,
                cx,
            ))
            .when(s.metadata != MetadataPolicy::Strip, |d| {
                d.child(
                    h_flex()
                        .gap_2()
                        .child(
                            v_flex()
                                .flex_1()
                                .gap_1()
                                .child(label(crate::i18n::t("作者")))
                                .child(Input::new(&self.artist).small()),
                        )
                        .child(
                            v_flex()
                                .flex_1()
                                .gap_1()
                                .child(label(crate::i18n::t("版权")))
                                .child(Input::new(&self.copyright).small()),
                        ),
                )
            })
            .child(label(crate::i18n::t(
                "文件名（{name} 为原文件名，{seq} 为序号）",
            )))
            .child(Input::new(&self.template).small())
            .when_some(example, |d, e| {
                d.child(div().text_xs().text_color(muted).truncate().child(e))
            })
            .when(batch, |d| {
                d.child(label(crate::i18n::t("导出到"))).child(
                    h_flex()
                        .gap_2()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_xs()
                                .truncate()
                                .child(dir.display().to_string()),
                        )
                        .child(
                            Button::new("export-dir")
                                .label(crate::i18n::t("更改…"))
                                .small()
                                .on_click(cx.listener(|d, _, _, cx| d.choose_dir(cx))),
                        ),
                )
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
                        Button::new("export-cancel")
                            .label(crate::i18n::t("取消"))
                            .small()
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(ExportEvent::Done(None)))),
                    )
                    .child(
                        Button::new("export-go")
                            .label(match (busy, batch) {
                                (true, _) => crate::i18n::t("选择位置…"),
                                (false, true) => crate::i18n::t("导出"),
                                (false, false) => crate::i18n::t("导出…"),
                            })
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
            .id("export-backdrop")
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(gpui::rgba(0x00000080))
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|_, _, _, cx| cx.emit(ExportEvent::Done(None))),
            )
            .child(card)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(name: &str) -> Request {
        Request {
            source: SourceRef::File(PathBuf::from(name)),
            params: Arc::new(EditParams::default()),
            name: name.into(),
            folder: None,
        }
    }

    #[test]
    fn names_and_extensions() {
        assert_eq!(
            render_template(DEFAULT_TEMPLATE, "IMG_1.CR3", 1, 1),
            "IMG_1-edit"
        );
        assert_eq!(render_template("trip-{seq}", "a.nef", 7, 120), "trip-007");
        assert_eq!(render_template("{name}/{seq}", "a.nef", 2, 9), "a_2");
        assert_eq!(render_template("  ", "a.nef", 1, 1), "a");
        let p = with_extension(PathBuf::from("/x/a.JPEG"), Format::Jpeg);
        assert_eq!(p, PathBuf::from("/x/a.JPEG"));
        let p = with_extension(PathBuf::from("/x/a"), Format::Tiff16);
        assert_eq!(p, PathBuf::from("/x/a.tif"));
        let p = with_extension(PathBuf::from("/x/a.png"), Format::WebP);
        assert_eq!(p, PathBuf::from("/x/a.webp"));
    }

    #[test]
    fn batch_names_never_collide() {
        let reqs = [request("a.cr3"), request("a.nef"), request("b.cr3")];
        let settings = Settings {
            template: "{name}".into(),
            ..Settings::default()
        };
        let dir = Path::new("/out");
        let paths = batch_paths(&reqs, &settings, dir, |p| p == Path::new("/out/b.jpg"));
        assert_eq!(
            paths,
            [
                PathBuf::from("/out/a.jpg"),
                PathBuf::from("/out/a-2.jpg"),
                PathBuf::from("/out/b-2.jpg"),
            ]
        );
    }

    #[test]
    fn presets_round_trip_without_the_folder() {
        let dir = std::env::temp_dir().join(format!("aigle-export-{}", std::process::id()));
        let path = dir.join("export_presets.json");
        let settings = Settings {
            format: Format::WebP,
            quality: 70,
            long_edge: Some(2048),
            template: "web-{seq}".into(),
            dir: Some(PathBuf::from("/somewhere")),
            ..Settings::default()
        };
        let presets = vec![ExportPreset {
            name: "Web".into(),
            settings: settings.clone(),
        }];
        save_presets(Some(&path), &presets).unwrap();
        let loaded = load_presets(Some(&path));
        assert_eq!(loaded.len(), 1);
        assert!(loaded[0].settings.same_preset(&settings));
        assert_eq!(loaded[0].settings.dir, None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
