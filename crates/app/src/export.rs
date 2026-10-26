//! Export dialog: format, size and colour space for one photo; the file is written by an
//! engine export job in the background (doc 02 §5.6).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use aigle_core::EditParams;
use aigle_engine_api::{DisplayProfileId, EncodedFormat, Engine, ExportOptions, SourceRef};
use gpui::{
    Context, Entity, EventEmitter, IntoElement, ParentElement, Render, SharedString, Styled,
    Subscription, Task, Window, div, prelude::*, px,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::slider::{Slider, SliderEvent, SliderState};
use gpui_component::{ActiveTheme, Selectable, Sizable, h_flex, v_flex};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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

/// Export settings, kept between exports.
#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub format: Format,
    pub quality: u8,
    /// `None`: full size.
    pub long_edge: Option<u32>,
    pub output: DisplayProfileId,
    /// Folder of the last export.
    pub dir: Option<PathBuf>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            format: Format::Jpeg,
            quality: 90,
            long_edge: None,
            output: DisplayProfileId::Srgb,
            dir: None,
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

/// `name` with its extension replaced by `ext` (an `-edit` suffix keeps it from colliding
/// with a JPEG original of the same stem).
pub fn file_name(name: &str, ext: &str) -> String {
    let stem = Path::new(name)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "export".into());
    format!("{stem}-edit.{ext}")
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

pub enum ExportEvent {
    /// The dialog closed; `Some` when the user chose a destination.
    Done(Option<(Request, Settings, PathBuf)>),
}

pub struct ExportDialog {
    request: Request,
    settings: Settings,
    quality: Entity<SliderState>,
    prompt: Option<Task<()>>,
    _sub: Subscription,
}

impl EventEmitter<ExportEvent> for ExportDialog {}

impl ExportDialog {
    pub fn new(
        request: Request,
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
        let sub = cx.subscribe_in(&quality, window, |d, _, ev: &SliderEvent, _, cx| {
            let v = match ev {
                SliderEvent::Change(v) | SliderEvent::Release(v) => v.start(),
            };
            d.settings.quality = v.round().clamp(1.0, 100.0) as u8;
            cx.notify();
        });
        ExportDialog {
            request,
            settings,
            quality,
            prompt: None,
            _sub: sub,
        }
    }

    fn choose(&mut self, cx: &mut Context<Self>) {
        let dir = self
            .settings
            .dir
            .clone()
            .or_else(|| self.request.folder.clone())
            .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
            .unwrap_or_else(|| PathBuf::from("."));
        let name = file_name(&self.request.name, self.settings.format.extension());
        let rx = cx.prompt_for_new_path(&dir, Some(&name));
        self.prompt = Some(cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(path))) = rx.await else {
                let _ = this.update(cx, |d, cx| {
                    d.prompt = None;
                    cx.notify()
                });
                return;
            };
            let _ = this.update(cx, |d, cx| {
                let path = with_extension(path, d.settings.format);
                d.settings.dir = path.parent().map(Path::to_path_buf);
                cx.emit(ExportEvent::Done(Some((
                    d.request.clone(),
                    d.settings.clone(),
                    path,
                ))));
            });
        }));
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
                    .label(label)
                    .small()
                    .selected(selected)
                    .on_click(cx.listener(move |d, _, _, cx| {
                        set(&mut d.settings, v.clone());
                        cx.notify();
                    }))
            }))
    }
}

/// Start the export job; dropping the returned job cancels it.
pub fn run(
    engine: &Engine,
    request: &Request,
    settings: &Settings,
    path: PathBuf,
) -> aigle_engine_api::Job<aigle_engine_api::ExportResult> {
    engine.export(
        request.source.clone(),
        request.params.clone(),
        ExportOptions {
            path,
            format: settings.format.encoded(settings.quality),
            long_edge: settings.long_edge,
            output: settings.output.clone(),
        },
    )
}

impl Render for ExportDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let s = self.settings.clone();
        let theme = cx.theme();
        let (muted, bg, border) = (theme.muted_foreground, theme.popover, theme.border);
        let label = |t: &'static str| div().text_xs().text_color(muted).child(t);
        let busy = self.prompt.is_some();
        let card = v_flex()
            .id("export-card")
            .w(px(440.0))
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
                    .child(format!("导出 {}", self.request.name)),
            )
            .child(label("格式"))
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
                        .child(label("质量"))
                        .child(div().text_xs().child(s.quality.to_string())),
                )
                .child(Slider::new(&self.quality))
            })
            .child(label("长边（像素）"))
            .child(self.choice(
                "size",
                SIZES.to_vec(),
                s.long_edge,
                |s, v| s.long_edge = v,
                cx,
            ))
            .child(label("色彩空间"))
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
            .child(
                h_flex()
                    .pt_2()
                    .gap_2()
                    .justify_end()
                    .child(
                        Button::new("export-cancel")
                            .label("取消")
                            .small()
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(ExportEvent::Done(None)))),
                    )
                    .child(
                        Button::new("export-go")
                            .label(if busy { "选择位置…" } else { "导出…" })
                            .small()
                            .primary()
                            .on_click(cx.listener(|d, _, _, cx| d.choose(cx))),
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

    #[test]
    fn names_and_extensions() {
        assert_eq!(file_name("IMG_1.CR3", "jpg"), "IMG_1-edit.jpg");
        let p = with_extension(PathBuf::from("/x/a.JPEG"), Format::Jpeg);
        assert_eq!(p, PathBuf::from("/x/a.JPEG"));
        let p = with_extension(PathBuf::from("/x/a"), Format::Tiff16);
        assert_eq!(p, PathBuf::from("/x/a.tif"));
        let p = with_extension(PathBuf::from("/x/a.png"), Format::WebP);
        assert_eq!(p, PathBuf::from("/x/a.webp"));
    }
}
