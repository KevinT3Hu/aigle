//! Searchable keybinding preferences. All edits are staged until Save.
use std::collections::BTreeSet;

use anyhow::{Result, bail};
use gpui::{
    Context, Entity, EventEmitter, FocusHandle, IntoElement, ParentElement, Render, SharedString,
    Styled, Subscription, Window, div, prelude::*, px,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::{ActiveTheme, Disableable, Selectable, Sizable, h_flex, v_flex};

use crate::actions::Quit;
use crate::i18n::t;
use crate::keymap::{self, Bindings};

pub struct Closed;

#[derive(Clone)]
struct Draft {
    bindings: Bindings,
    original: Bindings,
    defaults: Bindings,
}

impl Draft {
    fn new(bindings: Bindings) -> Self {
        Self {
            original: bindings.clone(),
            bindings,
            defaults: keymap::defaults(),
        }
    }

    fn keys(map: &Bindings, action: &str) -> Vec<String> {
        map.iter()
            .filter(|(_, name)| name.as_str() == action)
            .map(|(key, _)| key.clone())
            .collect()
    }

    fn modified(&self, action: &str) -> bool {
        Self::keys(&self.bindings, action) != Self::keys(&self.defaults, action)
    }

    fn parse_keys(value: &str) -> Result<Vec<String>> {
        if value.trim().is_empty() {
            return Ok(Vec::new());
        }
        let keys = value
            .split(',')
            .map(keymap::normalize)
            .collect::<Result<Vec<_>>>()?;
        let unique: BTreeSet<_> = keys.iter().collect();
        if unique.len() != keys.len() {
            bail!("{}", t("快捷键重复"));
        }
        if keys.iter().enumerate().any(|(i, key)| {
            keys.iter()
                .enumerate()
                .any(|(j, other)| i != j && other.starts_with(&format!("{key} ")))
        }) {
            bail!("{}", t("同一命令的快捷键不能互为前缀"));
        }
        Ok(keys)
    }

    fn conflicts(&self, action: &str, keys: &[String]) -> Vec<(String, String)> {
        self.bindings
            .iter()
            .filter(|(existing, owner)| {
                owner.as_str() != action
                    && keys.iter().any(|key| {
                        // A chord prefix is also a conflict: dispatch would delay or hide a command.
                        *existing == key
                            || existing.starts_with(&format!("{key} "))
                            || key.starts_with(&format!("{existing} "))
                    })
            })
            .map(|(key, owner)| (key.clone(), owner.clone()))
            .collect()
    }

    fn assign(&mut self, action: &str, keys: Vec<String>, replace: bool) -> Result<()> {
        let conflicts = self.conflicts(action, &keys);
        if !replace && !conflicts.is_empty() {
            bail!("{}", t("请先解决快捷键冲突"));
        }
        self.bindings.retain(|key, owner| {
            owner != action && !conflicts.iter().any(|(conflict, _)| conflict == key)
        });
        for key in keys {
            self.bindings.insert(key, action.to_owned());
        }
        Ok(())
    }
}

fn description(action: &str) -> (String, String) {
    let short = action.strip_prefix("aigle::").unwrap_or(action);
    let (label, group) = match short {
        "Open" => ("打开照片…", "文件"),
        "OpenFolder" => ("打开文件夹…", "文件"),
        "Export" => ("导出…", "文件"),
        "Quit" => ("退出", "文件"),
        "ShowGrid" => ("图库", "导航"),
        "ShowDevelop" => ("修图", "导航"),
        "OpenSelected" => ("打开所选照片 / 完成裁剪", "导航"),
        "Left" => ("上一张 / 向左", "导航"),
        "Right" => ("下一张 / 向右", "导航"),
        "Up" => ("向上", "导航"),
        "Down" => ("向下", "导航"),
        "Undo" => ("撤销", "编辑"),
        "Redo" => ("重做", "编辑"),
        "CopySettings" => ("拷贝设置…", "编辑"),
        "PasteSettings" => ("粘贴设置", "编辑"),
        "SyncSettings" => ("同步设置…", "编辑"),
        "NewPreset" => ("新建预设…", "编辑"),
        "CreateVirtualCopy" => ("创建虚拟副本", "编辑"),
        "RemoveVirtualCopy" => ("删除虚拟副本…", "编辑"),
        "SelectAll" => ("全选", "导航"),
        "ToggleCrop" => ("裁剪与拉直", "编辑"),
        "AutoTone" => ("自动色调", "编辑"),
        "ToggleMonochrome" => ("黑白", "编辑"),
        "ToggleMask" => ("蒙版", "编辑"),
        "ToggleWbPicker" => ("白平衡吸管", "编辑"),
        "SwapAspect" => ("横竖互换", "编辑"),
        "Cancel" => ("取消", "编辑"),
        "ToggleZoom" => ("缩放 / 适合窗口", "视图"),
        "ToggleBefore" => ("原图", "视图"),
        "ToggleSplit" => ("分屏对比", "视图"),
        "ToggleSoftProof" => ("软打样", "视图"),
        "ToggleGamutWarning" => ("色域警告", "视图"),
        "ProofSrgb" => ("软打样：sRGB", "视图"),
        "ProofDisplayP3" => ("软打样：Display P3", "视图"),
        "ProofAdobeRgb" => ("软打样：Adobe RGB", "视图"),
        "ToggleInfo" => ("拍摄参数", "视图"),
        "ToggleHistory" => ("历史记录", "视图"),
        "ToggleOverlay" => ("蒙版叠加", "视图"),
        "LanguageEnglish" => ("英语", "语言"),
        "LanguageChinese" => ("简体中文", "语言"),
        "DisplayAuto" => ("显示器配置文件：自动", "设置"),
        "DisplaySrgb" => ("显示器配置文件：sRGB", "设置"),
        "EditKeybindings" => ("编辑快捷键…", "设置"),
        "ReloadKeybindings" => ("重新加载快捷键", "设置"),
        _ if short.starts_with("Rate") => {
            return (format!("{} {}", t("评分"), &short[4..]), t("评分与标签"));
        }
        _ if short.starts_with("Label") => {
            return (
                format!("{} {}", t("颜色标签"), &short[5..]),
                t("评分与标签"),
            );
        }
        _ => return (short.to_owned(), t("其他")),
    };
    (t(label), t(group))
}

fn matches(action: &str, draft: &Draft, query: &str) -> bool {
    let (label, group) = description(action);
    let searchable = format!(
        "{action} {label} {group} {} {}",
        Draft::keys(&draft.bindings, action).join(" "),
        Draft::keys(&draft.defaults, action).join(" ")
    )
    .to_lowercase();
    query
        .to_lowercase()
        .replace('+', "-")
        .split_whitespace()
        .all(|word| searchable.contains(word))
}

pub struct KeybindingEditor {
    draft: Draft,
    actions: Vec<String>,
    selected: String,
    search: Entity<InputState>,
    shortcut: Entity<InputState>,
    focus: FocusHandle,
    modified_only: bool,
    recording: bool,
    recorded: Vec<String>,
    before_record: String,
    error: Option<String>,
    _subs: Vec<Subscription>,
}

impl EventEmitter<Closed> for KeybindingEditor {}

impl KeybindingEditor {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder(t("搜索命令或快捷键…")));
        let shortcut = cx.new(|cx| InputState::new(window, cx).placeholder("ctrl-k ctrl-s, alt-s"));
        let mut actions: Vec<_> = cx
            .all_action_names()
            .iter()
            .filter(|name| name.starts_with("aigle::"))
            .map(|name| (*name).to_owned())
            .collect();
        actions.sort_by_key(|name| description(name));
        let selected = actions.first().cloned().unwrap_or_default();
        let draft = Draft::new(keymap::current(cx));
        shortcut.update(cx, |input, cx| {
            input.set_value(
                Draft::keys(&draft.bindings, &selected).join(", "),
                window,
                cx,
            )
        });
        let focus = cx.focus_handle();
        let mut subs = vec![
            cx.subscribe(&search, |_, _, _: &InputEvent, cx| cx.notify()),
            cx.subscribe(&shortcut, |editor, _, _: &InputEvent, cx| {
                editor.error = None;
                cx.notify();
            }),
        ];
        let weak = cx.entity().downgrade();
        // Intercept recording before GPUI dispatches a shortcut, including global Quit.
        // Ordinary typing still uses the component library's normal input bindings.
        subs.push(cx.intercept_keystrokes(move |event, window, cx| {
            let Some(editor) = weak.upgrade() else {
                return;
            };
            let active = editor.read(cx).focus.contains_focused(window, cx);
            if !active {
                return;
            }
            let escape = event.keystroke.key == "escape" && !event.keystroke.modifiers.modified();
            if escape || editor.read(cx).recording {
                cx.stop_propagation();
                editor.update(cx, |editor, cx| {
                    if escape {
                        if editor.recording {
                            editor.recording = false;
                            let value = editor.before_record.clone();
                            editor.set_input(value, window, cx);
                        } else {
                            cx.emit(Closed);
                        }
                    } else {
                        let key = event.keystroke.unparse();
                        editor.recorded.push(key);
                        editor.set_input(editor.recorded.join(" "), window, cx);
                    }
                    cx.notify();
                });
            }
        }));
        Self {
            draft,
            actions,
            selected,
            search,
            shortcut,
            focus,
            modified_only: false,
            recording: false,
            recorded: Vec::new(),
            before_record: String::new(),
            error: None,
            _subs: subs,
        }
    }

    pub fn focus_search(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.search.update(cx, |input, cx| input.focus(window, cx));
    }

    fn set_input(&mut self, value: String, window: &mut Window, cx: &mut Context<Self>) {
        self.shortcut
            .update(cx, |input, cx| input.set_value(value, window, cx));
        self.error = None;
    }

    fn assign(&mut self, replace: bool, cx: &mut Context<Self>) -> bool {
        let value = self.shortcut.read(cx).value().to_string();
        let result = Draft::parse_keys(&value)
            .and_then(|keys| self.draft.assign(&self.selected, keys, replace));
        self.error = result.as_ref().err().map(|error| error.to_string());
        cx.notify();
        result.is_ok()
    }

    fn select(&mut self, action: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.recording || !self.assign(false, cx) {
            return;
        }
        self.selected = action;
        self.set_input(
            Draft::keys(&self.draft.bindings, &self.selected).join(", "),
            window,
            cx,
        );
        cx.notify();
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        if self.recording || !self.assign(false, cx) {
            return;
        }
        match keymap::save(self.draft.bindings.clone(), cx) {
            Ok(()) => cx.emit(Closed),
            Err(error) => {
                self.error = Some(format!("{}: {error:#}", t("无法保存快捷键")));
                cx.notify();
            }
        }
    }
}

impl Render for KeybindingEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (border, muted, active, hover, danger) = (
            theme.border,
            theme.muted_foreground,
            theme.list_active,
            theme.list_hover,
            theme.danger,
        );
        let query = self.search.read(cx).value().to_string();
        let filtered: Vec<_> = self
            .actions
            .iter()
            .filter(|action| {
                (!self.modified_only || self.draft.modified(action))
                    && matches(action, &self.draft, &query)
            })
            .cloned()
            .collect();
        let value = self.shortcut.read(cx).value().to_string();
        let keys = Draft::parse_keys(&value);
        let conflicts = keys
            .as_ref()
            .map(|keys| self.draft.conflicts(&self.selected, keys))
            .unwrap_or_default();
        let has_conflicts = !conflicts.is_empty();
        let invalid = keys.is_err();
        let modified = self
            .actions
            .iter()
            .filter(|action| self.draft.modified(action))
            .count();
        let dirty = self.draft.original != self.draft.bindings
            || Draft::keys(&self.draft.bindings, &self.selected)
                != keys.as_ref().cloned().unwrap_or_default();
        let selected = self.selected.clone();
        let (label, group) = description(&selected);
        let defaults = Draft::keys(&self.draft.defaults, &selected);
        let mut list = v_flex().gap_1();
        for action in &filtered {
            let action = action.clone();
            let chosen = action == selected;
            let changed = self.draft.modified(&action);
            let keys = Draft::keys(&self.draft.bindings, &action);
            let (label, group) = description(&action);
            list = list.child(
                v_flex()
                    .id(SharedString::from(action.clone()))
                    .p_2()
                    .gap_1()
                    .rounded_md()
                    .cursor_pointer()
                    .when(chosen, |row| row.bg(active))
                    .hover(|row| row.bg(hover))
                    .on_click(cx.listener(move |editor, _, window, cx| {
                        editor.select(action.clone(), window, cx)
                    }))
                    .child(
                        h_flex()
                            .gap_2()
                            .child(div().flex_1().min_w_0().text_sm().child(label))
                            .when(changed, |row| {
                                row.child(div().text_xs().text_color(muted).child(t("已修改")))
                            }),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .text_xs()
                            .text_color(muted)
                            .child(div().flex_1().child(group))
                            .child(if keys.is_empty() {
                                t("未绑定")
                            } else {
                                keys.join(" / ")
                            }),
                    ),
            );
        }
        if filtered.is_empty() {
            list = list.child(
                div()
                    .p_4()
                    .text_sm()
                    .text_color(muted)
                    .child(t("没有匹配的命令")),
            );
        }
        let mut detail = v_flex()
            .w(px(300.0))
            .flex_none()
            .gap_3()
            .pl_4()
            .border_l_1()
            .border_color(border)
            .child(
                div()
                    .text_base()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(label),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child(format!("{group} · {selected}")),
            )
            .child(div().text_sm().child(t("按键")))
            .child(Input::new(&self.shortcut).small().disabled(self.recording))
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child(t("用逗号分隔多个快捷键，用空格分隔组合键序列。")),
            )
            .child(
                Button::new("record-shortcut")
                    .label(if self.recording {
                        t("完成录制")
                    } else {
                        t("录制快捷键")
                    })
                    .small()
                    .selected(self.recording)
                    .on_click(cx.listener(|editor, _, window, cx| {
                        editor.recording = !editor.recording;
                        if editor.recording {
                            editor.before_record = editor.shortcut.read(cx).value().to_string();
                            editor.recorded.clear();
                            window.focus(&editor.focus, cx);
                        } else {
                            editor
                                .shortcut
                                .update(cx, |input, cx| input.focus(window, cx));
                        }
                        cx.notify();
                    })),
            )
            .when(self.recording, |d| {
                d.child(
                    div()
                        .text_xs()
                        .text_color(muted)
                        .child(t("按下快捷键或序列，然后点击完成录制；Esc 取消录制。")),
                )
            })
            .child(div().text_xs().text_color(muted).child(format!(
                "{}: {}",
                t("默认快捷键"),
                if defaults.is_empty() {
                    t("未绑定")
                } else {
                    defaults.join(" / ")
                }
            )))
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("unbind-action")
                            .label(t("解除绑定"))
                            .small()
                            .disabled(self.recording)
                            .on_click(cx.listener(|editor, _, window, cx| {
                                editor
                                    .draft
                                    .bindings
                                    .retain(|_, owner| owner != &editor.selected);
                                editor.set_input(String::new(), window, cx);
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("reset-action")
                            .label(t("恢复默认"))
                            .small()
                            .disabled(self.recording)
                            .on_click(cx.listener(|editor, _, window, cx| {
                                editor.set_input(
                                    Draft::keys(&editor.draft.defaults, &editor.selected)
                                        .join(", "),
                                    window,
                                    cx,
                                );
                                editor.assign(false, cx);
                            })),
                    ),
            );
        if has_conflicts {
            detail = detail.child(div().text_sm().text_color(danger).child(t("快捷键冲突")));
            for (key, owner) in conflicts {
                detail = detail.child(
                    div()
                        .text_xs()
                        .text_color(danger)
                        .child(format!("{key} → {}", description(&owner).0)),
                );
            }
            detail = detail.child(
                Button::new("replace-conflicts")
                    .label(t("替换冲突绑定"))
                    .small()
                    .disabled(self.recording)
                    .on_click(cx.listener(|editor, _, _, cx| {
                        editor.assign(true, cx);
                    })),
            );
        } else {
            detail = detail.child(
                Button::new("assign-shortcut")
                    .label(t("应用此绑定"))
                    .small()
                    .disabled(invalid || self.recording)
                    .on_click(cx.listener(|editor, _, _, cx| {
                        editor.assign(false, cx);
                    })),
            );
        }
        if invalid {
            detail = detail.child(
                div()
                    .text_xs()
                    .text_color(danger)
                    .child(t("快捷键格式无效")),
            );
        }
        if let Some(error) = &self.error {
            detail = detail.child(div().text_xs().text_color(danger).child(error.clone()));
        }
        v_flex()
            .track_focus(&self.focus)
            .key_context("KeybindingEditor")
            .gap_3()
            .on_action(|_: &Quit, _, cx| cx.stop_propagation())
            .child(
                h_flex()
                    .gap_3()
                    .child(
                        div()
                            .flex_1()
                            .child(Input::new(&self.search).small().disabled(self.recording)),
                    )
                    .child(
                        Button::new("modified-only")
                            .label(t("仅显示已修改"))
                            .small()
                            .selected(self.modified_only)
                            .on_click(cx.listener(|editor, _, _, cx| {
                                editor.modified_only = !editor.modified_only;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div().text_xs().text_color(muted).child(
                    rust_i18n::t!(
                        "显示 %{count} 个命令，%{modified} 个已修改",
                        count = filtered.len(),
                        modified = modified
                    )
                    .into_owned(),
                ),
            )
            .child(
                h_flex()
                    .h(px(
                        (f32::from(window.viewport_size().height) - 280.0).clamp(220.0, 450.0)
                    ))
                    .items_start()
                    .gap_4()
                    .child(
                        div()
                            .id("keybinding-action-list")
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .overflow_y_scroll()
                            .child(list),
                    )
                    .child(
                        div()
                            .id("keybinding-detail-scroll")
                            .h_full()
                            .overflow_y_scroll()
                            .child(detail),
                    ),
            )
            .child(
                h_flex()
                    .pt_3()
                    .border_t_1()
                    .border_color(border)
                    .gap_2()
                    .child(
                        Button::new("reset-all-bindings")
                            .label(t("全部恢复默认"))
                            .small()
                            .disabled(self.recording)
                            .on_click(cx.listener(|editor, _, window, cx| {
                                editor.draft.bindings = editor.draft.defaults.clone();
                                editor.set_input(
                                    Draft::keys(&editor.draft.bindings, &editor.selected)
                                        .join(", "),
                                    window,
                                    cx,
                                );
                                cx.notify();
                            })),
                    )
                    .child(div().flex_1().text_xs().text_color(muted).child(if dirty {
                        t("有未保存的更改")
                    } else {
                        t("更改将在保存后生效")
                    }))
                    .child(
                        Button::new("cancel-keybindings")
                            .label(t("取消"))
                            .small()
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(Closed))),
                    )
                    .child(
                        Button::new("save-keybindings")
                            .label(t("保存"))
                            .small()
                            .primary()
                            .disabled(!dirty || invalid || has_conflicts || self.recording)
                            .on_click(cx.listener(|editor, _, _, cx| editor.save(cx))),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conflicts_require_explicit_replacement_and_cancellation_keeps_original() {
        let mut draft = Draft::new(keymap::defaults());
        let before = draft.bindings.clone();
        assert!(
            draft
                .assign("aigle::Export", vec!["ctrl-o".into()], false)
                .is_err()
        );
        assert_eq!(draft.bindings, before);
        draft
            .assign("aigle::Export", vec!["ctrl-o".into()], true)
            .unwrap();
        assert_eq!(draft.bindings["ctrl-o"], "aigle::Export");
        assert!(!draft.bindings.contains_key("ctrl-e"));
        assert_eq!(draft.original, before);
    }

    #[test]
    fn aliases_unbinding_and_chord_prefix_conflicts() {
        let mut draft = Draft::new(keymap::defaults());
        let keys = Draft::parse_keys("ctrl-p, ctrl-k ctrl-o").unwrap();
        draft.assign("aigle::Open", keys, false).unwrap();
        assert_eq!(Draft::keys(&draft.bindings, "aigle::Open").len(), 2);
        assert!(!draft.bindings.contains_key("ctrl-o"));
        assert!(
            !draft
                .conflicts("aigle::Export", &["ctrl-k".into()])
                .is_empty()
        );
        draft.assign("aigle::Open", Vec::new(), false).unwrap();
        assert!(Draft::keys(&draft.bindings, "aigle::Open").is_empty());
        assert!(Draft::parse_keys("ctrl-o, ctrl-o").is_err());
        assert!(Draft::parse_keys("ctrl-o,").is_err());
        assert!(Draft::parse_keys("ctrl-k, ctrl-k ctrl-o").is_err());
    }

    #[test]
    fn search_matches_names_categories_and_assigned_keys() {
        let mut draft = Draft::new(keymap::defaults());
        assert!(matches("aigle::Open", &draft, "open ctrl-o"));
        assert!(matches("aigle::Open", &draft, "Ctrl+O"));
        assert!(!matches("aigle::Open", &draft, "export"));
        draft
            .assign("aigle::Open", vec!["alt-p".into()], false)
            .unwrap();
        assert!(matches("aigle::Open", &draft, "alt-p"));
        assert!(draft.modified("aigle::Open"));
    }
}
