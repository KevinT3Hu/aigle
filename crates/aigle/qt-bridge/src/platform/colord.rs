use anyhow::{Context, Result, ensure};
use std::{collections::HashMap, path::PathBuf};

pub(crate) async fn monitor_profile(screen: &str) -> Result<PathBuf> {
    use zbus::zvariant::OwnedObjectPath;
    const NAME: &str = "org.freedesktop.ColorManager";
    let conn = zbus::Connection::system().await?;
    let cm = zbus::Proxy::new(&conn, NAME, "/org/freedesktop/ColorManager", NAME).await?;
    let devices: Vec<OwnedObjectPath> = cm.call("GetDevicesByKind", &("display",)).await?;
    let mut best: Option<(u8, OwnedObjectPath)> = None;
    for path in devices {
        let device = zbus::Proxy::new(&conn, NAME, path, format!("{NAME}.Device")).await?;
        if !device.get_property::<bool>("Enabled").await.unwrap_or(true) {
            continue;
        }
        let metadata: HashMap<String, String> =
            device.get_property("Metadata").await.unwrap_or_default();
        let output = metadata
            .get("XRANDR_name")
            .or_else(|| metadata.get("OutputName"));
        let priority = if !screen.is_empty() && output.is_some_and(|name| name == screen) {
            3
        } else if metadata
            .get("OutputPriority")
            .is_some_and(|p| p == "primary")
        {
            2
        } else {
            1
        };
        // An identified different monitor must not supply this screen's calibration.
        if !screen.is_empty() && output.is_some_and(|name| name != screen) {
            continue;
        }
        let profiles: Vec<OwnedObjectPath> = device.get_property("Profiles").await?;
        if let Some(first) = profiles.into_iter().next() {
            if best.as_ref().is_none_or(|(score, _)| priority > *score) {
                best = Some((priority, first));
            }
        }
    }
    let (_, profile) = best.context("No monitor ICC profile found")?;
    let profile = zbus::Proxy::new(&conn, NAME, profile, format!("{NAME}.Profile")).await?;
    let file: String = profile.get_property("Filename").await?;
    ensure!(!file.is_empty(), "Monitor profile has no file");
    Ok(PathBuf::from(file))
}
