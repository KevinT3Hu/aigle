//! Develop presets: named sets of adjustment groups, one JSON file each in
//! `<config>/aigle/presets`. Applying a preset pastes its groups as one undoable command.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use aigle_core::EditParams;
use gpui::{
    Context, EventEmitter, IntoElement, ParentElement, Render, SharedString, Styled, Window, div,
    prelude::*, px,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::{ActiveTheme, Sizable, h_flex, v_flex};
use serde::{Deserialize, Serialize};

use crate::copy_settings::{Clipboard, Group};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Preset {
    pub name: String,
    pub groups: BTreeSet<Group>,
    pub params: EditParams,
}

impl Preset {
    pub fn clipboard(&self) -> Clipboard {
        Clipboard {
            params: Arc::new(self.params.clone()),
            groups: self.groups.clone(),
        }
    }
}

/// A file name for preset `name`: characters unsafe in file names become `_`.
fn file_stem(name: &str) -> String {
    let stem: String = name
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                '_'
            } else {
                c
            }
        })
        .collect();
    match stem.trim().trim_start_matches('.') {
        "" => "preset".into(),
        s => s.to_string(),
    }
}

/// `"{base} N"` with the smallest N ≥ 1 not in `existing`.
pub fn unique_name<'a>(existing: impl Iterator<Item = &'a str> + Clone, base: &str) -> String {
    (1..)
        .map(|n| format!("{base} {n}"))
        .find(|c| !existing.clone().any(|e| e == c))
        .expect("unbounded")
}

/// Presets on disk, sorted by name.
pub struct Store {
    dir: Option<PathBuf>,
    pub items: Vec<(PathBuf, Preset)>,
}

impl Store {
    pub fn open(dir: Option<PathBuf>) -> (Store, Vec<String>) {
        let mut items = Vec::new();
        let mut warnings = Vec::new();
        if let Some(d) = &dir
            && let Ok(entries) = std::fs::read_dir(d)
        {
            for path in entries.flatten().map(|e| e.path()) {
                if path.extension().is_none_or(|e| e != "json") {
                    continue;
                }
                match std::fs::read(&path)
                    .map_err(|e| e.to_string())
                    .and_then(|b| serde_json::from_slice::<Preset>(&b).map_err(|e| e.to_string()))
                {
                    Ok(p) => items.push((path, p)),
                    Err(e) => warnings.push(format!("{}: {e}", path.display())),
                }
            }
        }
        let mut store = Store { dir, items };
        store.sort();
        (store, warnings)
    }

    fn sort(&mut self) {
        self.items.sort_by_key(|(_, p)| p.name.to_lowercase());
    }

    pub fn names(&self) -> impl Iterator<Item = &str> + Clone {
        self.items.iter().map(|(_, p)| p.name.as_str())
    }

    /// Save `preset`, replacing a preset of the same name.
    pub fn save(&mut self, preset: Preset) -> std::io::Result<()> {
        let dir = self
            .dir
            .clone()
            .ok_or_else(|| std::io::Error::other("no configuration folder"))?;
        std::fs::create_dir_all(&dir)?;
        let existing = self.items.iter().position(|(_, p)| p.name == preset.name);
        let path = match existing {
            Some(i) => self.items[i].0.clone(),
            None => free_path(&dir, &file_stem(&preset.name)),
        };
        let json = serde_json::to_vec_pretty(&preset).map_err(std::io::Error::other)?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, &path)?;
        match existing {
            Some(i) => self.items[i].1 = preset,
            None => self.items.push((path, preset)),
        }
        self.sort();
        Ok(())
    }

    pub fn remove(&mut self, index: usize) -> std::io::Result<()> {
        let Some((path, _)) = self.items.get(index) else {
            return Ok(());
        };
        match std::fs::remove_file(path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        self.items.remove(index);
        Ok(())
    }
}

fn free_path(dir: &Path, stem: &str) -> PathBuf {
    std::iter::once(dir.join(format!("{stem}.json")))
        .chain((2..).map(|n| dir.join(format!("{stem} ({n}).json"))))
        .find(|p| !p.exists())
        .expect("unbounded")
}

pub enum PresetsEvent {
    Apply(usize),
    Create,
    Remove(usize),
}

/// Sidebar list of presets; the workspace handles applying, creating and removing.
pub struct PresetsPanel {
    pub store: Store,
}

impl EventEmitter<PresetsEvent> for PresetsPanel {}

impl PresetsPanel {
    pub fn new(store: Store) -> PresetsPanel {
        PresetsPanel { store }
    }
}

impl Render for PresetsPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (muted, hover) = (theme.muted_foreground, theme.list_hover);
        let rows = self
            .store
            .items
            .iter()
            .enumerate()
            .map(|(i, (_, p))| {
                h_flex()
                    .px_3()
                    .py_1()
                    .gap_2()
                    .text_xs()
                    .hover(|d| d.bg(hover))
                    .child(
                        div()
                            .id(SharedString::from(format!("preset-{i}")))
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .cursor_pointer()
                            .on_click(
                                cx.listener(move |_, _, _, cx| cx.emit(PresetsEvent::Apply(i))),
                            )
                            .child(p.name.clone()),
                    )
                    .child(
                        Button::new(SharedString::from(format!("preset-del-{i}")))
                            .label("×")
                            .xsmall()
                            .ghost()
                            .tooltip(crate::i18n::t("删除预设"))
                            .on_click(
                                cx.listener(move |_, _, _, cx| cx.emit(PresetsEvent::Remove(i))),
                            ),
                    )
            })
            .collect::<Vec<_>>();
        let empty = rows.is_empty();
        v_flex()
            .child(
                h_flex()
                    .px_3()
                    .py_2()
                    .justify_between()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(crate::i18n::t("预设")),
                    )
                    .child(
                        Button::new("preset-add")
                            .label("+")
                            .xsmall()
                            .ghost()
                            .tooltip(crate::i18n::t("用当前设置新建预设"))
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(PresetsEvent::Create))),
                    ),
            )
            .child(
                div()
                    .id("preset-scroll")
                    .max_h(px(200.0))
                    .overflow_y_scroll()
                    .when(empty, |d| {
                        d.child(
                            div()
                                .px_3()
                                .pb_1()
                                .text_xs()
                                .text_color(muted)
                                .child(crate::i18n::t("暂无预设")),
                        )
                    })
                    .child(v_flex().children(rows)),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("aigle-presets-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    fn preset(name: &str, exposure: f32) -> Preset {
        let mut params = EditParams::default();
        params.light.exposure = exposure;
        Preset {
            name: name.into(),
            groups: [Group::Light].into_iter().collect(),
            params,
        }
    }

    #[test]
    fn save_reload_replace_and_remove() {
        let dir = temp_dir("store");
        let (mut s, w) = Store::open(Some(dir.clone()));
        assert!(s.items.is_empty() && w.is_empty());
        s.save(preset("b/warm", 1.0)).unwrap();
        s.save(preset("A", 0.5)).unwrap();
        s.save(preset("b/warm", 2.0)).unwrap();
        std::fs::write(dir.join("broken.json"), "{").unwrap();

        let (mut s, w) = Store::open(Some(dir.clone()));
        assert_eq!(w.len(), 1);
        assert_eq!(s.names().collect::<Vec<_>>(), ["A", "b/warm"]);
        assert_eq!(s.items[1].1.params.light.exposure, 2.0);
        assert_eq!(s.items[1].0, dir.join("b_warm.json"));

        s.remove(0).unwrap();
        let (s, _) = Store::open(Some(dir.clone()));
        assert_eq!(s.names().collect::<Vec<_>>(), ["b/warm"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn applying_pastes_only_the_preset_groups() {
        let p = preset("x", 1.25);
        let mut target = EditParams::default();
        target.presence.clarity = 40.0;
        let out = p.clipboard().paste_onto(&target).unwrap();
        assert_eq!(out.light.exposure, 1.25);
        assert_eq!(out.presence.clarity, 40.0);
    }

    #[test]
    fn names_and_stems() {
        assert_eq!(unique_name(["预设 1"].into_iter(), "预设"), "预设 2");
        assert_eq!(file_stem("a:b"), "a_b");
        assert_eq!(file_stem("..."), "preset");
    }
}
