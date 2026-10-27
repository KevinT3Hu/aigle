//! Display colour management: which profile the canvas renders to.
//!
//! In `Auto` mode the monitor's ICC profile is detected at startup (`$AIGLE_DISPLAY_ICC`, else
//! the profile colord assigns to the primary display) and registered with the engine; the
//! canvas then renders to `DisplayProfileId::Icc`. Without a usable profile, and in `Srgb`
//! mode, it renders sRGB. The mode is saved in `<config dir>/display`.

use std::collections::HashMap;
use std::path::PathBuf;

use aigle_engine_api::{DisplayProfileId, Engine, ImportKind};
use gpui::{App, AppContext, BorrowAppContext, Global};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Auto,
    Srgb,
}

/// A registered display profile.
#[derive(Clone, Debug, PartialEq)]
pub struct Detected {
    pub id: DisplayProfileId,
    pub name: String,
    pub path: PathBuf,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DisplayColor {
    pub mode: Mode,
    pub detected: Option<Detected>,
    /// Why no profile is in use in `Auto` mode, once detection finished.
    pub problem: Option<String>,
}

impl Global for DisplayColor {}

impl DisplayColor {
    /// The profile the canvas renders to.
    pub fn profile(&self) -> DisplayProfileId {
        match (self.mode, &self.detected) {
            (Mode::Auto, Some(d)) => d.id.clone(),
            _ => DisplayProfileId::Srgb,
        }
    }
}

/// The display profile for the canvas (sRGB before [`init`]).
pub fn profile(cx: &App) -> DisplayProfileId {
    cx.try_global::<DisplayColor>()
        .map(DisplayColor::profile)
        .unwrap_or_default()
}

fn preference_path() -> Option<PathBuf> {
    crate::config_dir().map(|p| p.join("display"))
}

pub fn init(engine: Engine, cx: &mut App) {
    let saved = preference_path().and_then(|p| std::fs::read_to_string(p).ok());
    let mode = match saved.as_deref().map(str::trim) {
        Some("srgb") => Mode::Srgb,
        _ => Mode::Auto,
    };
    cx.set_global(DisplayColor {
        mode,
        detected: None,
        problem: None,
    });
    cx.spawn(async move |cx| {
        let found = cx.background_spawn(async { find_profile() }).await;
        let result = match found {
            Ok(path) => match std::fs::read(&path) {
                Ok(bytes) => engine
                    .import(ImportKind::DisplayProfile, bytes)
                    .await
                    .map(|i| Detected {
                        id: DisplayProfileId::Icc(i.data_ref.as_str().to_string()),
                        name: if i.name.is_empty() {
                            path.file_name()
                                .map(|n| n.to_string_lossy().into_owned())
                                .unwrap_or_default()
                        } else {
                            i.name
                        },
                        path: path.clone(),
                    })
                    .map_err(|e| format!("{}: {e}", path.display())),
                Err(e) => Err(format!("{}: {e}", path.display())),
            },
            Err(e) => Err(e),
        };
        match &result {
            Ok(d) => tracing::info!(profile = %d.name, path = %d.path.display(), "display profile"),
            Err(e) => tracing::info!("no display profile, rendering sRGB: {e}"),
        }
        cx.update(|cx| {
            cx.update_global::<DisplayColor, _>(|d, _| match result {
                Ok(p) => d.detected = Some(p),
                Err(e) => d.problem = Some(e),
            });
            cx.refresh_windows();
        });
    })
    .detach();
}

pub fn set_mode(mode: Mode, cx: &mut App) -> std::io::Result<()> {
    cx.update_global::<DisplayColor, _>(|d, _| d.mode = mode);
    cx.refresh_windows();
    if let Some(path) = preference_path() {
        std::fs::create_dir_all(path.parent().expect("preference has a parent"))?;
        std::fs::write(
            path,
            match mode {
                Mode::Auto => "auto",
                Mode::Srgb => "srgb",
            },
        )?;
    }
    Ok(())
}

fn find_profile() -> Result<PathBuf, String> {
    if let Some(p) = std::env::var_os("AIGLE_DISPLAY_ICC").filter(|p| !p.is_empty()) {
        return Ok(PathBuf::from(p));
    }
    futures::executor::block_on(colord()).map_err(|e| format!("colord: {e}"))
}

/// The default profile of the primary display device in colord (else of the first display
/// device that has one).
async fn colord() -> zbus::Result<PathBuf> {
    use zbus::zvariant::OwnedObjectPath;
    const NAME: &str = "org.freedesktop.ColorManager";
    let conn = zbus::Connection::system().await?;
    let cm = zbus::Proxy::new(&conn, NAME, "/org/freedesktop/ColorManager", NAME).await?;
    let devices: Vec<OwnedObjectPath> = cm.call("GetDevicesByKind", &("display",)).await?;
    let mut best: Option<(bool, OwnedObjectPath)> = None;
    for path in devices {
        let dev = zbus::Proxy::new(&conn, NAME, path, format!("{NAME}.Device")).await?;
        if !dev.get_property::<bool>("Enabled").await.unwrap_or(true) {
            continue;
        }
        let profiles: Vec<OwnedObjectPath> = dev.get_property("Profiles").await?;
        let Some(first) = profiles.into_iter().next() else {
            continue;
        };
        let meta: HashMap<String, String> = dev.get_property("Metadata").await.unwrap_or_default();
        let primary = meta.get("OutputPriority").map(String::as_str) == Some("primary");
        if best.as_ref().is_none_or(|(p, _)| primary && !p) {
            best = Some((primary, first));
        }
    }
    let (_, profile) = best.ok_or_else(|| zbus::Error::Failure("no display profile".into()))?;
    let p = zbus::Proxy::new(&conn, NAME, profile, format!("{NAME}.Profile")).await?;
    let file: String = p.get_property("Filename").await?;
    if file.is_empty() {
        return Err(zbus::Error::Failure("the profile has no file".into()));
    }
    Ok(PathBuf::from(file))
}
