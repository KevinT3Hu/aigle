//! User key-to-action overrides. GPUI resolves named actions and dispatches them normally.
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::rc::Rc;

use anyhow::{Context as _, Result, bail};
use gpui::{App, DummyKeyboardMapper, Global, KeyBinding, KeyBindingContextPredicate, Keystroke};

use crate::actions::{CONTEXT, Quit};

const DEFAULTS: &str = include_str!("../keymap.json");
pub type Bindings = BTreeMap<String, String>;

#[derive(Default)]
struct ActiveKeymap(Bindings);
impl Global for ActiveKeymap {}

pub fn path() -> Result<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".config")))
        .context("Cannot find the user configuration directory")?;
    Ok(base.join("aigle/keymap.json"))
}

pub fn normalize(key: &str) -> Result<String> {
    if key.trim().is_empty() {
        bail!("A shortcut cannot be empty");
    }
    key.split_whitespace()
        .map(|key| {
            Keystroke::parse(key)
                .map(|key| key.unparse())
                .map_err(Into::into)
        })
        .collect::<Result<Vec<_>>>()
        .map(|keys| keys.join(" "))
}

fn parse(source: &str) -> Result<BTreeMap<String, Option<String>>> {
    let json: serde_json::Value = serde_json::from_str(source).context("Invalid keymap JSON")?;
    let object = json
        .as_object()
        .context("Keymap must be an object mapping shortcuts to action names or null")?;
    let mut result = BTreeMap::new();
    for (key, value) in object {
        let key = normalize(key).with_context(|| format!("Invalid shortcut {key:?}"))?;
        let action = match value {
            serde_json::Value::Null => None,
            serde_json::Value::String(name) if name.starts_with("aigle::") => Some(name.clone()),
            _ => bail!("Shortcut {key:?} must name an aigle:: action, or null to disable it"),
        };
        if result.insert(key.clone(), action).is_some() {
            bail!("Duplicate shortcut {key:?}");
        }
    }
    Ok(result)
}

fn merged(overrides: Option<&str>) -> Result<Bindings> {
    let mut result: Bindings = parse(DEFAULTS)?
        .into_iter()
        .filter_map(|(key, action)| action.map(|a| (key, a)))
        .collect();
    if let Some(source) = overrides {
        for (key, action) in parse(source)? {
            if let Some(action) = action {
                result.insert(key, action);
            } else {
                result.remove(&key);
            }
        }
    }
    Ok(result)
}

fn prepare(bindings: &Bindings, cx: &App) -> Result<Vec<KeyBinding>> {
    // Resolve and validate the entire map before replacing any live binding.
    let mut loaded = Vec::new();
    for (key, name) in bindings {
        let action = cx
            .build_action(name, None)
            .with_context(|| format!("Unknown action {name:?} for {key:?}"))?;
        let context = if name == "aigle::Quit" {
            None
        } else {
            Some(Rc::new(KeyBindingContextPredicate::parse(CONTEXT)?))
        };
        loaded.push(KeyBinding::load(
            key,
            action,
            context,
            false,
            None,
            &DummyKeyboardMapper,
        )?);
    }
    Ok(loaded)
}

fn install(bindings: Bindings, cx: &mut App) -> Result<()> {
    let loaded = prepare(&bindings, cx)?;
    // Keep component/text-input shortcuts, including bindings registered after startup.
    let others: Vec<_> = cx
        .key_bindings()
        .borrow()
        .bindings()
        .filter(|binding| !binding.action().name().starts_with("aigle::"))
        .cloned()
        .collect();
    cx.clear_key_bindings();
    cx.bind_keys(others);
    cx.bind_keys(loaded);
    cx.set_global(ActiveKeymap(bindings));
    Ok(())
}

pub fn reload(cx: &mut App) -> Result<()> {
    let path = path()?;
    let source = match std::fs::read_to_string(&path) {
        Ok(source) => Some(source),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error).with_context(|| format!("Cannot read {}", path.display())),
    };
    install(merged(source.as_deref())?, cx)
}

pub fn init(cx: &mut App) -> Option<String> {
    cx.on_action(|_: &Quit, cx| cx.quit());
    if let Err(error) = reload(cx) {
        install(merged(None).expect("valid default keymap"), cx).expect("valid default actions");
        Some(format!(
            "{}: {error:#}",
            crate::i18n::t("无法加载快捷键，已使用默认设置")
        ))
    } else {
        None
    }
}

pub fn current(cx: &App) -> Bindings {
    cx.try_global::<ActiveKeymap>()
        .map(|active| active.0.clone())
        .unwrap_or_else(defaults)
}

pub fn defaults() -> Bindings {
    merged(None).expect("valid default keymap")
}

fn overrides(bindings: &Bindings) -> BTreeMap<String, Option<String>> {
    let defaults = defaults();
    let mut result = BTreeMap::new();
    for key in defaults.keys().chain(bindings.keys()) {
        if defaults.get(key) != bindings.get(key) {
            result.insert(key.clone(), bindings.get(key).cloned());
        }
    }
    result
}

/// Validate before touching disk. Persist disabled defaults explicitly, then replace the
/// active map only after the atomic write succeeds. Cancelled drafts never reach here.
pub fn save(bindings: Bindings, cx: &mut App) -> Result<()> {
    prepare(&bindings, cx)?;
    let path = path()?;
    std::fs::create_dir_all(path.parent().expect("config file has a parent"))?;
    let contents = serde_json::to_string_pretty(&overrides(&bindings))? + "\n";
    let suffix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos();
    let temporary = path.with_extension(format!("json.{}.{suffix}.tmp", std::process::id()));
    let write = || -> Result<()> {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(contents.as_bytes())?;
        file.sync_all()?;
        std::fs::rename(&temporary, &path)?;
        Ok(())
    };
    if let Err(error) = write() {
        let _ = std::fs::remove_file(&temporary);
        return Err(error);
    }
    install(bindings, cx)
}

/// Interpolate shortcut hints from the same effective map that dispatches actions.
pub fn hint(key: &str, cx: &App) -> String {
    let text = crate::i18n::t(key);
    let Some(active) = cx.try_global::<ActiveKeymap>() else {
        return text;
    };
    format_hint(&text, &active.0)
}

fn format_hint(text: &str, bindings: &Bindings) -> String {
    let mut names = Vec::new();
    let mut values = Vec::new();
    for name in [
        "Open",
        "OpenFolder",
        "ShowGrid",
        "ShowDevelop",
        "OpenSelected",
        "Cancel",
        "ToggleOverlay",
        "ToggleWbPicker",
        "SwapAspect",
    ] {
        names.push(format!("shortcut_{name}"));
        let action = format!("aigle::{name}");
        let keys: Vec<_> = bindings
            .iter()
            .filter(|(_, value)| **value == action)
            .map(|(key, _)| key.as_str())
            .collect();
        values.push(if keys.is_empty() {
            crate::i18n::t("未绑定")
        } else {
            keys.join(" / ")
        });
    }
    rust_i18n::replace_patterns(
        text,
        &names.iter().map(String::as_str).collect::<Vec<_>>(),
        &values,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid_and_overrides_replace_or_disable_keys() {
        let defaults = merged(None).unwrap();
        assert_eq!(defaults["ctrl-o"], "aigle::Open");
        let remapped = merged(Some(
            r#"{"ctrl-o":null,"ctrl-p":"aigle::Open","g":"aigle::ToggleCrop"}"#,
        ))
        .unwrap();
        assert!(!remapped.contains_key("ctrl-o"));
        assert_eq!(remapped["ctrl-p"], "aigle::Open");
        assert_eq!(remapped["g"], "aigle::ToggleCrop");
        assert_eq!(remapped["ctrl-z"], defaults["ctrl-z"]);
        assert_eq!(merged(Some("{}")).unwrap(), defaults);
    }

    #[test]
    fn saved_overrides_round_trip_unbound_defaults_and_aliases() {
        let mut bindings = defaults();
        bindings.remove("ctrl-o");
        bindings.insert("ctrl-p".into(), "aigle::Open".into());
        bindings.insert("ctrl-alt-p".into(), "aigle::Open".into());
        let json = serde_json::to_string(&overrides(&bindings)).unwrap();
        assert_eq!(merged(Some(&json)).unwrap(), bindings);
        assert_eq!(overrides(&defaults()).len(), 0);
    }

    #[test]
    fn hints_show_custom_shortcuts_and_unbound_actions() {
        let bindings = merged(Some(r#"{"ctrl-o":null,"ctrl-p":"aigle::Open","g":null}"#)).unwrap();
        assert_eq!(
            format_hint("Open (%{shortcut_Open})", &bindings),
            "Open (ctrl-p)"
        );
        assert_eq!(
            format_hint("Grid (%{shortcut_ShowGrid})", &bindings),
            "Grid (Unbound)"
        );
    }

    #[test]
    fn rejects_invalid_keys_and_values_and_supports_chords() {
        for bad in [
            "[]",
            "{",
            r#"{"":null}"#,
            r#"{"g":1}"#,
            r#"{"g":"typo"}"#,
            r#"{"invalidmodifier-a":"aigle::Open"}"#,
        ] {
            assert!(merged(Some(bad)).is_err(), "{bad}");
        }
        assert_eq!(
            merged(Some(r#"{"ctrl-k ctrl-o":"aigle::Open"}"#)).unwrap()["ctrl-k ctrl-o"],
            "aigle::Open"
        );
    }
}
