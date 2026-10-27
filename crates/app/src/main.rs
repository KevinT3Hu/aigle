//! aigle GUI (doc 01 §6). The only crate that depends on GPUI.
//!
//! `aigle-gui [FOLDER | PHOTO]` opens a folder in the grid or a photo in develop;
//! `aigle-gui --synthetic` opens a generated test image.

rust_i18n::i18n!("locales", fallback = "en");

mod actions;
mod canvas;
mod copy_settings;
mod crop;
mod curve;
mod export;
mod filmstrip;
mod grid;
mod history;
mod i18n;
mod keybinding_editor;
mod keymap;
mod library;
mod mask_panel;
mod meta;
mod panel;
mod photo;
mod resources;
mod tools;
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

fn source_from_args() -> Option<SourceRef> {
    let arg = std::env::args_os().nth(1)?;
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
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "warn,aigle=info".into()),
        )
        .init();
    let source = source_from_args();

    gpui_platform::application()
        .with_assets(gpui_kit_assets::Assets)
        .run(move |cx: &mut App| {
            gpui_component::init(cx);
            i18n::init();
            Theme::change(ThemeMode::Dark, None, cx);
            let keymap_error = keymap::init(cx);

            let engine = aigle_engine::create(EngineConfig::default());
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
            cx.activate(true);
        });
}
