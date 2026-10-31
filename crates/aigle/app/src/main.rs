//! aigle GUI (doc 01 §6). The only crate that depends on GPUI.
//!
//! `aigle-gui [FOLDER | PHOTO]` opens a folder in the grid or a photo in develop;
//! `aigle-gui --synthetic` opens a generated test image.

rust_i18n::i18n!("locales", fallback = "en");

mod actions;
mod auto_tone;
mod canvas;
mod compare;
mod copy_settings;
mod crash;
mod crop;
mod curve;
mod display;
mod export;
mod export_queue;
mod filmstrip;
mod grid;
mod hdr_surface;
mod history;
mod i18n;
mod import;
mod keybinding_editor;
mod keymap;
mod library;
mod library_bar;
mod mask_panel;
mod meta;
mod metadata_panel;
mod native_canvas;
mod panel;
mod photo;
mod presets;
mod rename;
mod resources;
mod retouch_panel;
mod saved_searches;
mod scopes;
mod tools;
mod ui;
mod view;
mod viewport;
mod workspace;

use std::path::PathBuf;

use aigle_engine_api::{EngineConfig, SourceRef, SyntheticPattern};
use gpui::{
    App, AppContext, Bounds, TitlebarOptions, WindowBounds, WindowDecorations, WindowOptions, px,
    size,
};
use gpui_component::{Root, Theme, ThemeMode, TitleBar};

use crate::workspace::Workspace;

/// `$XDG_CONFIG_HOME/aigle`, else `~/.config/aigle`.
pub fn config_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".config")))
        .map(|p| p.join("aigle"))
}

/// `$XDG_DATA_HOME/aigle`, else `~/.local/share/aigle`: the catalog.
pub fn data_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_DATA_HOME")
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".local/share")))
        .map(|p| p.join("aigle"))
}

fn source_from_args() -> Option<SourceRef> {
    let arg = std::env::args_os().nth(1)?;
    if arg == "--remote" {
        let result = (|| -> anyhow::Result<SourceRef> {
            let mut args = std::env::args().skip(2);
            let server = args
                .next()
                .ok_or_else(|| anyhow::anyhow!("Usage: aigle-gui --remote SERVER PHOTO_ID"))?;
            let id = args
                .next()
                .ok_or_else(|| anyhow::anyhow!("Usage: aigle-gui --remote SERVER PHOTO_ID"))?;
            anyhow::ensure!(
                args.next().is_none(),
                "Usage: aigle-gui --remote SERVER PHOTO_ID"
            );
            let token = std::env::var("THESAUROS_TOKEN")
                .map_err(|_| anyhow::anyhow!("Set THESAUROS_TOKEN to connect to Thesauros"))?;
            let cache = data_dir()
                .ok_or_else(|| anyhow::anyhow!("No application data directory"))?
                .join("thesauros")
                .join(blake3::hash(server.as_bytes()).to_hex().as_str());
            let remote = aigle_remote::Remote::new(&server, token)?;
            let path = remote.checkout_or_open(&id, &cache)?;
            tracing::info!(path = %path.display(), "opened Thesauros checkout; use thesauros-client push to synchronize edits");
            Ok(SourceRef::File(path))
        })();
        return Some(result.unwrap_or_else(|e| {
            eprintln!("aigle: {e:#}");
            std::process::exit(2);
        }));
    }
    if arg == "--hdr-probe" {
        return Some(SourceRef::File(
            std::env::args_os()
                .nth(2)
                .map(PathBuf::from)
                .unwrap_or_else(|| std::env::temp_dir().join("aigle-hdr/studio-scene.exr")),
        ));
    }
    if arg == "--synthetic" {
        return Some(SourceRef::Synthetic {
            pattern: SyntheticPattern::Gradient,
            width: 6000,
            height: 4000,
        });
    }
    Some(SourceRef::File(PathBuf::from(arg)))
}

fn main() {
    if aigle_isolate::is_worker() {
        crash::init_worker_logging();
        let _span = tracing::info_span!("worker", pid = std::process::id()).entered();
        aigle_isolate::serve(&[aigle_engine::WORKER_OPS, aigle_library::WORKER_OPS].concat());
    }
    let log = crash::init_logging();
    let crashed_before = crash::previous_reports();
    let _session = crash::Session::start(log.clone());
    aigle_isolate::enable("/proc/self/exe".into(), crash::worker_env(log.as_deref()));
    let source = source_from_args();
    let probe = std::env::args_os()
        .nth(1)
        .is_some_and(|a| a == "--hdr-probe");

    gpui_platform::application()
        .with_assets(gpui_kit_assets::Assets)
        .run(move |cx: &mut App| {
            gpui_component::init(cx);
            i18n::init();
            Theme::change(ThemeMode::Dark, None, cx);
            let keymap_error = keymap::init(cx);

            let engine = aigle_engine::create(EngineConfig::default());
            display::init(engine.clone(), cx);
            // GNOME's Wayland compositor has no server-side decorations, so the window draws its
            // own title bar (`TitleBar` in the workspace) and frame (`Root`). Where the window
            // manager does decorate, gpui falls back to that and the title bar hides its controls.
            let options = WindowOptions {
                titlebar: Some(TitlebarOptions {
                    title: Some("aigle".into()),
                    ..TitleBar::title_bar_options()
                }),
                window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                    None,
                    size(px(1440.0), px(900.0)),
                    cx,
                ))),
                window_decorations: Some(WindowDecorations::Client),
                window_background: gpui::WindowBackgroundAppearance::Transparent,
                window_min_size: Some(size(px(900.0), px(560.0))),
                ..TitleBar::window_options()
            };
            let opened = cx.open_window(options, |window, cx| {
                let ws = cx.new(|cx| Workspace::new(engine, source, window, cx));
                let root = cx.new(|cx| Root::new(ws, window, cx));
                if let Some(error) = keymap_error {
                    window.defer(cx, move |window, cx| {
                        use gpui_component::WindowExt;
                        window.push_notification(error, cx);
                    });
                }
                if let Some(report) = crashed_before.last() {
                    let text =
                        rust_i18n::t!("common.crashed_before", path = report.display().to_string())
                            .into_owned();
                    window.defer(cx, move |window, cx| {
                        use gpui_component::WindowExt;
                        window.push_notification(text, cx);
                    });
                }
                root
            });
            if let Err(e) = opened {
                tracing::error!("cannot open the main window: {e:#}");
                cx.quit();
            }
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
            if probe {
                let seconds = std::env::var("AIGLE_HDR_PROBE_SECONDS")
                    .ok()
                    .and_then(|v| v.parse::<u64>().ok())
                    .unwrap_or(30)
                    .clamp(5, 300);
                cx.spawn(async move |cx| {
                    cx.background_executor()
                        .timer(std::time::Duration::from_secs(seconds))
                        .await;
                    cx.update(|cx| cx.quit());
                })
                .detach();
            }
            cx.activate(true);
        });
}
