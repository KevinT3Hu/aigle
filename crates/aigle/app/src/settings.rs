//! Unified application preferences and provider/model management.
pub use crate::providers::{CredentialStatus, ModelSettings, init};
use crate::{
    display,
    i18n::{Language, t},
    keybinding_editor::{Closed, KeybindingEditor},
    providers::{self, Provider},
};
use gpui::{
    Context, Entity, EventEmitter, FocusHandle, IntoElement, ParentElement, Render, Styled,
    Subscription, Task, Window, div, prelude::*, px,
};
use gpui_component::{
    ActiveTheme, Disableable, Selectable, Sizable,
    button::ButtonVariants,
    h_flex,
    input::{Input, InputState},
    menu::{DropdownMenu, PopupMenuItem},
    tab::TabBar,
    v_flex,
};
use std::collections::BTreeSet;

#[derive(Clone, Copy)]
pub enum Page {
    General,
    Model,
    Shortcuts,
}
impl Page {
    fn index(self) -> usize {
        match self {
            Self::General => 0,
            Self::Model => 1,
            Self::Shortcuts => 2,
        }
    }
}
pub enum SettingsEvent {
    Saved {
        language: Language,
        display: display::Mode,
        recursive: bool,
    },
    Closed,
}
pub struct SettingsDialog {
    page: usize,
    focus: FocusHandle,
    language: Language,
    display: display::Mode,
    recursive: bool,
    draft: ModelSettings,
    provider_index: Option<usize>,
    provider_name: Entity<InputState>,
    endpoint: Entity<InputState>,
    model: Entity<InputState>,
    key: Entity<InputState>,
    shortcuts: Entity<KeybindingEditor>,
    error: Option<String>,
    saving: bool,
    cleared_keys: BTreeSet<String>,
    waiting_for_key: bool,
    fetching: bool,
    models_fetched: bool,
    fetched_models: Vec<String>,
    fetch_generation: u64,
    fetch_task: Option<Task<()>>,
    save_task: Option<Task<()>>,
    _subs: Vec<Subscription>,
}
impl EventEmitter<SettingsEvent> for SettingsDialog {}
impl SettingsDialog {
    pub fn new(page: Page, recursive: bool, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let settings = cx.global::<ModelSettings>().clone();
        let provider_index = settings
            .providers
            .iter()
            .position(|p| Some(&p.id) == settings.active_provider.as_ref())
            .or((!settings.providers.is_empty()).then_some(0));
        let provider = provider_index
            .map(|i| settings.providers[i].clone())
            .unwrap_or_default();
        let mut input = |value: String| {
            cx.new(|cx| {
                let mut s = InputState::new(window, cx);
                s.set_value(value, window, cx);
                s
            })
        };
        let provider_name = input(provider.name);
        let endpoint = input(provider.endpoint);
        let key = input(provider.api_key);
        let model =
            cx.new(|cx| InputState::new(window, cx).placeholder(t("settings.model_placeholder")));
        key.update(cx, |s, cx| s.set_masked(true, window, cx));
        let shortcuts = cx.new(|cx| KeybindingEditor::new(window, cx));
        shortcuts.update(cx, |e, _| e.embedded = true);
        let sub = cx.subscribe(&shortcuts, |_, _, _: &Closed, cx| {
            cx.emit(SettingsEvent::Closed)
        });
        let loaded = cx.observe_global_in::<CredentialStatus>(window, |s, window, cx| {
            if s.waiting_for_key && !cx.global::<CredentialStatus>().loading {
                s.waiting_for_key = false;
                let loaded = cx.global::<ModelSettings>().clone();
                for p in &mut s.draft.providers {
                    if let Some(found) = loaded.providers.iter().find(|found| found.id == p.id) {
                        p.restore_credentials(found);
                    }
                }
                let value = s
                    .provider_index
                    .and_then(|i| s.draft.providers.get(i))
                    .map(|p| p.api_key.clone())
                    .unwrap_or_default();
                s.key.update(cx, |key, cx| key.set_value(value, window, cx));
            }
            cx.notify();
        });
        Self {
            page: page.index(),
            focus: cx.focus_handle(),
            language: crate::i18n::language(),
            display: cx.global::<display::DisplayColor>().mode,
            recursive,
            draft: settings,
            provider_index,
            provider_name,
            endpoint,
            model,
            key,
            shortcuts,
            error: None,
            saving: false,
            cleared_keys: BTreeSet::new(),
            waiting_for_key: cx.global::<CredentialStatus>().loading,
            fetching: false,
            models_fetched: false,
            fetched_models: vec![],
            fetch_generation: 0,
            fetch_task: None,
            save_task: None,
            _subs: vec![sub, loaded],
        }
    }
    pub fn select(&mut self, page: Page, window: &mut Window, cx: &mut Context<Self>) {
        self.page = page.index();
        self.focus(window, cx);
        cx.notify();
    }
    pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        match self.page {
            1 => self.endpoint.update(cx, |e, cx| e.focus(window, cx)),
            2 => self
                .shortcuts
                .update(cx, |e, cx| e.focus_search(window, cx)),
            _ => window.focus(&self.focus, cx),
        }
    }
    fn flush_provider(&mut self, cx: &mut Context<Self>) {
        if let Some(p) = self
            .provider_index
            .and_then(|i| self.draft.providers.get_mut(i))
        {
            p.name = self.provider_name.read(cx).value().trim().into();
            p.endpoint = self.endpoint.read(cx).value().trim().into();
            p.api_key = self.key.read(cx).value().to_string();
            p.add_model(self.model.read(cx).value().trim());
        }
    }
    fn show_provider(&mut self, index: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        self.provider_index = index;
        let p = index
            .and_then(|i| self.draft.providers.get(i))
            .cloned()
            .unwrap_or_default();
        self.provider_name
            .update(cx, |s, cx| s.set_value(p.name, window, cx));
        self.endpoint
            .update(cx, |s, cx| s.set_value(p.endpoint, window, cx));
        self.key
            .update(cx, |s, cx| s.set_value(p.api_key, window, cx));
        self.model.update(cx, |s, cx| s.set_value("", window, cx));
        self.fetch_generation += 1;
        self.fetch_task = None;
        self.fetching = false;
        self.models_fetched = false;
        self.fetched_models.clear();
        self.error = None;
        cx.notify();
    }
    fn switch_provider(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.flush_provider(cx);
        self.show_provider(Some(index), window, cx);
    }
    fn add_provider(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.flush_provider(cx);
        let mut p = Provider::new();
        p.name = format!(
            "{} {}",
            t("settings.provider"),
            self.draft.providers.len() + 1
        );
        self.draft.providers.push(p);
        self.show_provider(Some(self.draft.providers.len() - 1), window, cx);
    }
    fn remove_provider(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(i) = self.provider_index {
            self.draft.providers.remove(i);
            self.draft.repair_selection();
            self.show_provider(
                (!self.draft.providers.is_empty())
                    .then_some(i.min(self.draft.providers.len().saturating_sub(1))),
                window,
                cx,
            );
        }
    }
    fn add_model(&mut self, model: String, window: &mut Window, cx: &mut Context<Self>) {
        self.flush_provider(cx);
        if let Some(p) = self
            .provider_index
            .and_then(|i| self.draft.providers.get_mut(i))
        {
            p.add_model(&model);
        }
        self.model.update(cx, |s, cx| s.set_value("", window, cx));
        cx.notify();
    }
    fn fetch_models(&mut self, cx: &mut Context<Self>) {
        if self.fetching || cx.global::<CredentialStatus>().loading {
            return;
        }
        self.flush_provider(cx);
        let endpoint = self.endpoint.read(cx).value().trim().to_string();
        if let Err(e) = crate::agent::endpoint_base(&endpoint) {
            self.error = Some(e.to_string());
            cx.notify();
            return;
        }
        let key = self.key.read(cx).value().to_string();
        let generation = self.fetch_generation;
        self.fetching = true;
        self.error = None;
        let work = cx.background_executor().spawn(async move {
            providers::fetch_models(endpoint.clone(), key).map(|models| (endpoint, models))
        });
        self.fetch_task = Some(cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |s, cx| {
                if s.fetch_generation != generation {
                    return;
                }
                s.fetching = false;
                match result {
                    Ok((endpoint, models)) if s.endpoint.read(cx).value().trim() == endpoint => {
                        s.models_fetched = true;
                        s.fetched_models = models;
                    }
                    Ok(_) => {}
                    Err(e) => s.error = Some(format!("{}: {e}", t("settings.fetch_failed"))),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }
    fn save(&mut self, cx: &mut Context<Self>) {
        if self.saving || self.fetching || cx.global::<CredentialStatus>().loading {
            return;
        }
        self.flush_provider(cx);
        let mut settings = self.draft.clone();
        for p in &mut settings.providers {
            match crate::agent::endpoint_base(&p.endpoint) {
                Ok(endpoint) => p.endpoint = endpoint,
                Err(e) => {
                    self.error = Some(format!("{}: {e}", p.name));
                    cx.notify();
                    return;
                }
            }
            if p.name.is_empty() {
                p.name = rig_reqwest::reqwest::Url::parse(&p.endpoint)
                    .ok()
                    .and_then(|u| u.host_str().map(str::to_string))
                    .unwrap_or_else(|| t("settings.provider"));
            }
        }
        settings.repair_selection();
        if !self
            .shortcuts
            .update(cx, |editor, cx| editor.save_changes(cx))
        {
            self.page = 2;
            self.error = Some(t("settings.shortcuts_error"));
            cx.notify();
            return;
        }
        let jobs = providers::credential_changes(
            &settings,
            cx.global::<ModelSettings>(),
            &self.cleared_keys,
        );
        self.saving = true;
        self.error = None;
        let language = self.language;
        let display = self.display;
        let recursive = self.recursive;
        self.save_task = Some(cx.spawn(async move |this, cx| {
            for change in jobs {
                let job = cx.update(|cx| change.start(cx));
                if job.await.is_err() {
                    let _ = this.update(cx, |s, cx| {
                        s.saving = false;
                        s.error = Some(t("settings.keyring_save_failed"));
                        cx.notify();
                    });
                    return;
                }
            }
            providers::clear_legacy_flags(&mut settings);
            let (settings, result) = cx
                .background_spawn(async move {
                    let result = settings.save();
                    (settings, result)
                })
                .await;
            let _ = this.update(cx, |s, cx| {
                s.saving = false;
                match result {
                    Ok(()) => {
                        cx.set_global(settings);
                        cx.update_global::<CredentialStatus, _>(|status, _| status.failed = false);
                        cx.emit(SettingsEvent::Saved {
                            language,
                            display,
                            recursive,
                        });
                    }
                    Err(e) => {
                        s.error = Some(e.to_string());
                        cx.notify();
                    }
                }
            });
        }));
        cx.notify();
    }
    fn render_providers(
        &self,
        loading: bool,
        muted: gpui::Hsla,
        danger: gpui::Hsla,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let choices: Vec<_> = self
            .draft
            .providers
            .iter()
            .enumerate()
            .map(|(i, p)| (i, p.name.clone()))
            .collect();
        let selected = self.provider_index;
        let weak = cx.entity().downgrade();
        let current = self
            .provider_index
            .and_then(|i| self.draft.providers.get(i));
        let caption = current
            .map(|p| p.name.clone())
            .unwrap_or_else(|| t("settings.no_providers"));
        v_flex()
            .gap_2()
            .p_2()
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        crate::ui::labeled_button(
                            "settings-provider",
                            crate::ui::Glyph::Camera,
                            caption,
                        )
                        .small()
                        .disabled(loading || choices.is_empty())
                        .dropdown_menu(move |menu, _, _| {
                            choices.iter().fold(menu, |menu, (i, name)| {
                                let weak = weak.clone();
                                let i = *i;
                                menu.item(
                                    PopupMenuItem::new(name.clone())
                                        .checked(Some(i) == selected)
                                        .on_click(move |_, window, cx| {
                                            let _ = weak.update(cx, |s, cx| {
                                                s.switch_provider(i, window, cx)
                                            });
                                        }),
                                )
                            })
                        }),
                    )
                    .child(
                        crate::ui::labeled_button(
                            "add-provider",
                            crate::ui::Glyph::Add,
                            t("settings.add_provider"),
                        )
                        .small()
                        .disabled(loading)
                        .on_click(cx.listener(|s, _, window, cx| s.add_provider(window, cx))),
                    )
                    .when(current.is_some(), |d| {
                        d.child(
                            crate::ui::icon_button(
                                "remove-provider",
                                crate::ui::Glyph::Delete,
                                t("settings.remove_provider"),
                            )
                            .small()
                            .disabled(loading)
                            .on_click(
                                cx.listener(|s, _, window, cx| s.remove_provider(window, cx)),
                            ),
                        )
                    }),
            )
            .when(current.is_some(), |d| {
                d.child(t("settings.provider_name"))
                    .child(Input::new(&self.provider_name).small().disabled(loading))
                    .child(t("settings.endpoint"))
                    .child(Input::new(&self.endpoint).small().disabled(loading))
                    .child(t("settings.api_key"))
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                div()
                                    .flex_1()
                                    .child(Input::new(&self.key).small().disabled(loading)),
                            )
                            .child(
                                crate::ui::icon_button(
                                    "remove-model-key",
                                    crate::ui::Glyph::Delete,
                                    t("settings.remove_key"),
                                )
                                .small()
                                .disabled(loading)
                                .on_click(cx.listener(
                                    |s, _, window, cx| {
                                        if let Some(p) =
                                            s.provider_index.and_then(|i| s.draft.providers.get(i))
                                        {
                                            s.cleared_keys.insert(p.id.clone());
                                        }
                                        s.key.update(cx, |key, cx| key.set_value("", window, cx));
                                        cx.notify();
                                    },
                                )),
                            ),
                    )
                    .child(
                        h_flex()
                            .justify_between()
                            .gap_2()
                            .child(t("settings.models"))
                            .child(
                                crate::ui::labeled_button(
                                    "fetch-models",
                                    crate::ui::Glyph::Reset,
                                    t(if self.fetching {
                                        "settings.fetching"
                                    } else {
                                        "settings.fetch_models"
                                    }),
                                )
                                .small()
                                .disabled(loading || self.fetching)
                                .on_click(cx.listener(|s, _, _, cx| s.fetch_models(cx))),
                            ),
                    )
                    .children(
                        current
                            .into_iter()
                            .flat_map(|p| p.models.iter())
                            .enumerate()
                            .map(|(i, model)| {
                                let model = model.clone();
                                let remove = model.clone();
                                h_flex()
                                    .gap_2()
                                    .justify_between()
                                    .child(div().flex_1().min_w_0().truncate().child(model))
                                    .child(
                                        crate::ui::icon_button(
                                            ("remove-model", i),
                                            crate::ui::Glyph::Close,
                                            t("settings.remove_model"),
                                        )
                                        .xsmall()
                                        .ghost()
                                        .on_click(
                                            cx.listener(move |s, _, _, cx| {
                                                if let Some(p) = s
                                                    .provider_index
                                                    .and_then(|i| s.draft.providers.get_mut(i))
                                                {
                                                    p.models.retain(|m| m != &remove);
                                                }
                                                cx.notify();
                                            }),
                                        ),
                                    )
                            }),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .child(div().flex_1().child(Input::new(&self.model).small()))
                            .child(
                                crate::ui::labeled_button(
                                    "add-model",
                                    crate::ui::Glyph::Add,
                                    t("settings.add_model"),
                                )
                                .small()
                                .on_click(cx.listener(
                                    |s, _, window, cx| {
                                        let model = s.model.read(cx).value().to_string();
                                        s.add_model(model, window, cx);
                                    },
                                )),
                            ),
                    )
                    .when(self.models_fetched, |d| {
                        d.child(
                            v_flex()
                                .id("discovered-models")
                                .gap_1()
                                .max_h(px(180.0))
                                .overflow_y_scroll()
                                .when(self.fetched_models.is_empty(), |d| {
                                    d.child(t("settings.no_models"))
                                })
                                .children(self.fetched_models.iter().enumerate().map(
                                    |(i, model)| {
                                        let exists =
                                            current.is_some_and(|p| p.models.contains(model));
                                        let add = model.clone();
                                        h_flex()
                                            .gap_2()
                                            .justify_between()
                                            .child(
                                                div()
                                                    .flex_1()
                                                    .min_w_0()
                                                    .truncate()
                                                    .child(model.clone()),
                                            )
                                            .child(
                                                crate::ui::labeled_button(
                                                    ("discovered-model", i),
                                                    if exists {
                                                        crate::ui::Glyph::Check
                                                    } else {
                                                        crate::ui::Glyph::Add
                                                    },
                                                    t(if exists {
                                                        "settings.model_added"
                                                    } else {
                                                        "settings.add_model"
                                                    }),
                                                )
                                                .xsmall()
                                                .disabled(exists)
                                                .on_click(cx.listener(move |s, _, window, cx| {
                                                    s.add_model(add.clone(), window, cx)
                                                })),
                                            )
                                    },
                                )),
                        )
                    })
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child(t("settings.vision_note")),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child(t("settings.key_note")),
                    )
            })
            .when(loading, |d| {
                d.child(
                    div()
                        .text_xs()
                        .text_color(muted)
                        .child(t("settings.keyring_loading")),
                )
            })
            .when(cx.global::<CredentialStatus>().failed, |d| {
                d.child(
                    div()
                        .text_xs()
                        .text_color(danger)
                        .child(t("settings.keyring_read_failed")),
                )
            })
            .into_any_element()
    }
}
impl Render for SettingsDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.saving {
            return v_flex()
                .p_3()
                .child(t("settings.keyring_saving"))
                .into_any_element();
        }
        let loading_key = cx.global::<CredentialStatus>().loading;
        let theme = cx.theme();
        let border = theme.border;
        let muted = theme.muted_foreground;
        let danger = theme.danger;
        let display_info = match &cx.global::<display::DisplayColor>().detected {
            Some(d) => d.name.clone(),
            None => t("display.none"),
        };
        let tabs = TabBar::new("settings-pages")
            .selected_index(self.page)
            .children([
                t("settings.general"),
                t("settings.model"),
                t("settings.shortcuts"),
            ])
            .on_click(cx.listener(|s, page: &usize, window, cx| {
                s.page = *page;
                s.focus(window, cx);
                s.error = None;
                cx.notify();
            }));
        let content = match self.page {
            0 => v_flex()
                .gap_4()
                .p_3()
                .child(
                    div()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child(t("menu.language")),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .child(
                            crate::ui::labeled_button(
                                "settings-en",
                                crate::ui::Glyph::Check,
                                t("language.name.en"),
                            )
                            .selected(self.language == Language::English)
                            .on_click(cx.listener(|s, _, _, cx| {
                                s.language = Language::English;
                                cx.notify();
                            })),
                        )
                        .child(
                            crate::ui::labeled_button(
                                "settings-zh",
                                crate::ui::Glyph::Check,
                                t("language.name.zh_cn"),
                            )
                            .selected(self.language == Language::Chinese)
                            .on_click(cx.listener(|s, _, _, cx| {
                                s.language = Language::Chinese;
                                cx.notify();
                            })),
                        ),
                )
                .child(
                    div()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child(t("menu.display_profile")),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .child(
                            crate::ui::labeled_button(
                                "settings-auto",
                                crate::ui::Glyph::Auto,
                                t("display.auto"),
                            )
                            .selected(self.display == display::Mode::Auto)
                            .on_click(cx.listener(|s, _, _, cx| {
                                s.display = display::Mode::Auto;
                                cx.notify();
                            })),
                        )
                        .child(
                            crate::ui::labeled_button(
                                "settings-srgb",
                                crate::ui::Glyph::View,
                                "sRGB",
                            )
                            .selected(self.display == display::Mode::Srgb)
                            .on_click(cx.listener(|s, _, _, cx| {
                                s.display = display::Mode::Srgb;
                                cx.notify();
                            })),
                        ),
                )
                .child(div().text_xs().text_color(muted).child(display_info))
                .child(
                    div()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child(t("settings.library")),
                )
                .child(
                    crate::ui::labeled_button(
                        "settings-subfolders",
                        crate::ui::Glyph::Folder,
                        t("action.ToggleSubfolders"),
                    )
                    .selected(self.recursive)
                    .on_click(cx.listener(|s, _, _, cx| {
                        s.recursive = !s.recursive;
                        cx.notify();
                    })),
                )
                .into_any_element(),
            1 => self.render_providers(loading_key, muted, danger, cx),
            _ => self.shortcuts.clone().into_any_element(),
        };
        v_flex()
            .gap_3()
            .min_h_0()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|_, event: &gpui::KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" && !event.keystroke.modifiers.modified() {
                    cx.stop_propagation();
                    cx.emit(SettingsEvent::Closed);
                }
            }))
            .on_action(|_: &crate::actions::Quit, _, cx| cx.stop_propagation())
            .child(tabs)
            .child(
                div()
                    .id("settings-content")
                    .max_h(px(
                        (f32::from(window.viewport_size().height) - 240.0).max(180.0)
                    ))
                    .overflow_y_scroll()
                    .child(content),
            )
            .when_some(self.error.clone(), |d, e| {
                d.child(div().text_color(danger).child(e))
            })
            .child(
                h_flex()
                    .gap_2()
                    .justify_end()
                    .pt_3()
                    .border_t_1()
                    .border_color(border)
                    .child(
                        crate::ui::labeled_button(
                            "settings-cancel",
                            crate::ui::Glyph::Close,
                            t("common.cancel"),
                        )
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(SettingsEvent::Closed))),
                    )
                    .child(
                        crate::ui::labeled_button(
                            "settings-save",
                            crate::ui::Glyph::Save,
                            t("common.save"),
                        )
                        .primary()
                        .disabled(loading_key || self.fetching)
                        .on_click(cx.listener(|s, _, _, cx| s.save(cx))),
                    ),
            )
            .into_any_element()
    }
}
