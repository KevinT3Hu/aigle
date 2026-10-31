//! Agent chat presentation and integration with the active photo document.
use crate::agent::{AgentEvent, profile_ops, run};
use crate::photo::Photo;
use crate::providers::ModelSettings as Settings;
use aigle_doc::Origin;
use futures::{StreamExt, channel::mpsc::unbounded};
use gpui::{
    Context, Entity, IntoElement, ParentElement, Render, ScrollHandle, Styled, StyledImage,
    Subscription, Task, Window, div, img, prelude::*, px,
};
use gpui_component::{
    ActiveTheme, Disableable, Sizable,
    button::ButtonVariants,
    h_flex,
    input::{InputEvent, Textarea, TextareaState},
    menu::{DropdownMenu, PopupMenuItem},
    text::TextView,
    v_flex,
};
use serde_json::Value;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Role {
    User,
    Assistant,
    Tool,
}
struct ChatMessage {
    role: Role,
    text: String,
    profile: Vec<(String, Value)>,
    preview: Option<Arc<gpui::Image>>,
}
impl ChatMessage {
    fn text(role: Role, text: String) -> Self {
        Self {
            role,
            text,
            profile: vec![],
            preview: None,
        }
    }
}
pub struct AgentPanel {
    photo: Entity<Photo>,
    prompt: Entity<TextareaState>,
    transcript: Vec<ChatMessage>,
    status: String,
    busy: bool,
    cancel: Arc<AtomicBool>,
    task: Option<Task<()>>,
    scroll: ScrollHandle,
    _input: Subscription,
}
impl Drop for AgentPanel {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}
impl AgentPanel {
    pub fn new(photo: Entity<Photo>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let prompt = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder(crate::i18n::t("agent.placeholder"))
                .auto_grow(1, 4)
                .submit_on_enter(true)
        });
        let input = cx.subscribe_in(&prompt, window, |p, _, event: &InputEvent, window, cx| {
            if let InputEvent::PressEnter { shift: false, .. } = event {
                p.send(window, cx);
            }
            cx.notify();
        });
        Self {
            photo,
            prompt,
            transcript: vec![],
            status: String::new(),
            busy: false,
            cancel: Arc::new(AtomicBool::new(false)),
            task: None,
            scroll: ScrollHandle::new(),
            _input: input,
        }
    }
    fn push(&mut self, message: ChatMessage, cx: &mut Context<Self>) {
        self.transcript.push(message);
        self.scroll.scroll_to_bottom();
        cx.notify();
    }
    fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy || self.photo.read(cx).info().is_none() {
            return;
        }
        let prompt = self.prompt.read(cx).value().trim().to_string();
        if prompt.is_empty() {
            return;
        }
        if cx.global::<crate::settings::CredentialStatus>().loading {
            self.status = crate::i18n::t("settings.keyring_loading");
            cx.notify();
            return;
        }
        let Some(settings) = cx.global::<Settings>().active() else {
            self.status = crate::i18n::t("agent.configure");
            cx.notify();
            return;
        };
        let key = settings.api_key.clone();
        self.push(ChatMessage::text(Role::User, prompt), cx);
        self.prompt.update(cx, |p, cx| p.set_value("", window, cx));
        let conversation = self
            .transcript
            .iter()
            .filter(|m| m.role != Role::Tool)
            .map(|m| {
                format!(
                    "{}: {}",
                    if m.role == Role::User {
                        "User"
                    } else {
                        "Assistant"
                    },
                    m.text
                )
            })
            .collect::<Vec<_>>()
            .join("\n\n");
        let photo = self.photo.read(cx);
        let session = photo.session.clone();
        let base = (**photo.head()).clone();
        let original = base.clone();
        let revision = photo.doc.rev();
        self.cancel = Arc::new(AtomicBool::new(false));
        let cancel = self.cancel.clone();
        self.busy = true;
        self.status = crate::i18n::t("agent.working");
        let (events, mut incoming) = unbounded();
        let work = cx
            .background_executor()
            .spawn(async move { run(settings, key, session, base, conversation, cancel, events) });
        self.task = Some(cx.spawn(async move |this, cx| {
            while let Some(event) = incoming.next().await {
                if this
                    .update(cx, |p, cx| match event {
                        AgentEvent::Profile(profile) => p.push(
                            ChatMessage {
                                role: Role::Tool,
                                text: profile.explanation,
                                profile: profile.adjustments.into_iter().collect(),
                                preview: None,
                            },
                            cx,
                        ),
                        AgentEvent::Preview(image) => p.push(
                            ChatMessage {
                                role: Role::Tool,
                                text: crate::i18n::t("agent.preview"),
                                profile: vec![],
                                preview: Some(Arc::new(gpui::Image::from_bytes(
                                    gpui::ImageFormat::Jpeg,
                                    image,
                                ))),
                            },
                            cx,
                        ),
                    })
                    .is_err()
                {
                    return;
                }
            }
            let result = work.await;
            let _ = this.update(cx, |p, cx| {
                p.busy = false;
                if p.cancel.load(Ordering::Relaxed) {
                    p.status = crate::i18n::t("agent.cancelled");
                    cx.notify();
                    return;
                }
                match result {
                    Ok((params, answer)) => {
                        if p.photo.read(cx).doc.rev() != revision {
                            p.status = crate::i18n::t("agent.changed");
                        } else {
                            let ops = profile_ops(&original, &params);
                            let applied = p.photo.update(cx, |ph, cx| {
                                ph.apply_ops(
                                    Origin::Agent {
                                        run_id: std::time::SystemTime::now()
                                            .duration_since(std::time::UNIX_EPOCH)
                                            .unwrap_or_default()
                                            .as_millis()
                                            as u64,
                                        step: 1,
                                    },
                                    "Agent edit",
                                    ops,
                                    cx,
                                )
                            });
                            if applied {
                                p.push(ChatMessage::text(Role::Assistant, answer), cx);
                                p.status = crate::i18n::t("agent.applied");
                            } else {
                                p.status = crate::i18n::t("agent.apply_failed");
                            }
                        }
                    }
                    Err(e) => p.status = format!("Agent failed: {e}"),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }
}
impl AgentPanel {
    fn choose_model(&mut self, provider: &str, model: Option<&str>, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let mut settings = cx.global::<Settings>().clone();
        let result = settings
            .choose(provider, model)
            .and_then(|_| settings.save());
        match result {
            Ok(()) => {
                cx.set_global(settings);
                cx.refresh_windows();
            }
            Err(e) => {
                self.status = e.to_string();
                cx.notify();
            }
        }
    }
    fn model_bar(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let settings = cx.global::<Settings>();
        let current = settings.selected_provider();
        let provider_label = current
            .map(|p| p.name.clone())
            .unwrap_or_else(|| crate::i18n::t("settings.provider"));
        let model_label = settings
            .active_model
            .clone()
            .unwrap_or_else(|| crate::i18n::t("settings.models"));
        let providers: Vec<_> = settings
            .providers
            .iter()
            .map(|p| (p.id.clone(), p.name.clone()))
            .collect();
        let models = current.map(|p| p.models.clone()).unwrap_or_default();
        let provider_id = settings.active_provider.clone();
        let selected_model = settings.active_model.clone();
        let ready = !self.prompt.read(cx).value().trim().is_empty()
            && !self.busy
            && self.photo.read(cx).info().is_some()
            && settings.active().is_some()
            && !cx.global::<crate::settings::CredentialStatus>().loading;
        let weak = cx.entity().downgrade();
        let weak_model = weak.clone();
        let active_provider = provider_id.clone();
        h_flex()
            .gap_1()
            .min_w_0()
            .child(
                div().flex_1().min_w_0().child(
                    crate::ui::labeled_button(
                        "chat-provider",
                        crate::ui::Glyph::Camera,
                        provider_label,
                    )
                    .xsmall()
                    .ghost()
                    .w_full()
                    .disabled(self.busy || providers.is_empty())
                    .dropdown_menu(move |menu, _, _| {
                        providers.iter().fold(menu, |menu, (id, name)| {
                            let weak = weak.clone();
                            let id = id.clone();
                            menu.item(
                                PopupMenuItem::new(name.clone())
                                    .checked(Some(&id) == provider_id.as_ref())
                                    .on_click(move |_, _, cx| {
                                        let _ =
                                            weak.update(cx, |p, cx| p.choose_model(&id, None, cx));
                                    }),
                            )
                        })
                    }),
                ),
            )
            .child(
                div().flex_1().min_w_0().child(
                    crate::ui::labeled_button("chat-model", crate::ui::Glyph::Auto, model_label)
                        .xsmall()
                        .ghost()
                        .w_full()
                        .disabled(self.busy || models.is_empty())
                        .dropdown_menu(move |menu, _, _| {
                            models.iter().fold(menu, |menu, model| {
                                let weak = weak_model.clone();
                                let id = active_provider.clone();
                                let model = model.clone();
                                menu.item(
                                    PopupMenuItem::new(model.clone())
                                        .checked(Some(&model) == selected_model.as_ref())
                                        .on_click(move |_, _, cx| {
                                            if let Some(id) = &id {
                                                let _ = weak.update(cx, |p, cx| {
                                                    p.choose_model(id, Some(&model), cx)
                                                });
                                            }
                                        }),
                                )
                            })
                        }),
                ),
            )
            .child(
                crate::ui::icon_button(
                    "agent-settings",
                    crate::ui::Glyph::Edit,
                    crate::i18n::t("settings.title"),
                )
                .xsmall()
                .ghost()
                .on_click(|_, window, cx| {
                    window.dispatch_action(Box::new(crate::actions::ConfigureModel), cx)
                }),
            )
            .when(self.busy, |d| {
                d.child(
                    crate::ui::icon_button(
                        "agent-cancel",
                        crate::ui::Glyph::Close,
                        crate::i18n::t("common.cancel"),
                    )
                    .xsmall()
                    .on_click(cx.listener(|p, _, _, cx| {
                        p.cancel.store(true, Ordering::Relaxed);
                        p.status = crate::i18n::t("agent.cancelling");
                        cx.notify();
                    })),
                )
            })
            .when(!self.busy, |d| {
                d.child(
                    crate::ui::icon_button(
                        "agent-send",
                        crate::ui::Glyph::Auto,
                        format!(
                            "{} · {}",
                            crate::i18n::t("agent.send"),
                            crate::i18n::t("agent.enter_hint")
                        ),
                    )
                    .xsmall()
                    .primary()
                    .disabled(!ready)
                    .on_click(cx.listener(|p, _, window, cx| p.send(window, cx))),
                )
            })
            .into_any_element()
    }
}

impl Render for AgentPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let border = theme.border;
        let muted = theme.muted_foreground;
        let foreground = theme.foreground;
        let user_bg = theme.accent;
        let other_bg = theme.muted;
        let messages = self
            .transcript
            .iter()
            .enumerate()
            .map(|(i, message)| {
                let user = message.role == Role::User;
                let tool = message.role == Role::Tool;
                let name = crate::i18n::t(if user {
                    "agent.you"
                } else if tool {
                    "agent.tool"
                } else {
                    "panel.tab.agent"
                });
                let bubble = v_flex()
                    .min_w_0()
                    .gap_1()
                    .p_2()
                    .rounded_lg()
                    .border_1()
                    .border_color(border)
                    .bg(if user { user_bg } else { other_bg })
                    .text_color(foreground)
                    .child(
                        div()
                            .text_xs()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(muted)
                            .child(name),
                    )
                    .child(
                        TextView::markdown(("chat-text", i), message.text.clone())
                            .selectable(true)
                            .scrollable(false),
                    )
                    .when(!message.profile.is_empty(), |d| {
                        d.child(
                            v_flex()
                                .gap_1()
                                .pt_1()
                                .border_t_1()
                                .border_color(border)
                                .child(
                                    div()
                                        .text_xs()
                                        .font_weight(gpui::FontWeight::SEMIBOLD)
                                        .child(crate::i18n::t("agent.profile")),
                                )
                                .children(message.profile.iter().map(|(path, value)| {
                                    h_flex()
                                        .justify_between()
                                        .gap_1()
                                        .text_xs()
                                        .child(crate::i18n::t(&format!("param.{path}")))
                                        .child(match value {
                                            Value::Number(n) => {
                                                format!("{:.2}", n.as_f64().unwrap_or_default())
                                            }
                                            Value::Array(items) => format!("{} items", items.len()),
                                            Value::Object(_) => "Updated".to_string(),
                                            _ => value.to_string(),
                                        })
                                })),
                        )
                    })
                    .when_some(message.preview.clone(), |d, image| {
                        d.child(
                            img(image)
                                .w_full()
                                .max_h(px(180.0))
                                .object_fit(gpui::ObjectFit::Contain)
                                .rounded_md(),
                        )
                    });
                h_flex()
                    .w_full()
                    .items_start()
                    .when(user, |d| d.child(div().w(px(24.0)).flex_none()))
                    .child(div().flex_1().min_w_0().child(bubble))
                    .when(!user, |d| d.child(div().w(px(12.0)).flex_none()))
            })
            .collect::<Vec<_>>();
        v_flex()
            .size_full()
            .min_h_0()
            .text_sm()
            .child(
                div()
                    .id("agent-messages")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .child(
                        v_flex()
                            .gap_1()
                            .p_2()
                            .when(self.transcript.is_empty(), |d| {
                                d.child(
                                    v_flex()
                                        .gap_1()
                                        .py_2()
                                        .child(
                                            div()
                                                .text_base()
                                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                                .child(crate::i18n::t("agent.welcome")),
                                        )
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(muted)
                                                .child(crate::i18n::t("agent.sharing")),
                                        ),
                                )
                            })
                            .children(messages)
                            .when(self.busy, |d| {
                                d.child(
                                    div()
                                        .p_2()
                                        .rounded_lg()
                                        .bg(other_bg)
                                        .text_color(muted)
                                        .child(crate::i18n::t("agent.working")),
                                )
                            }),
                    ),
            )
            .child(
                v_flex()
                    .flex_none()
                    .gap_1()
                    .p_2()
                    .border_t_1()
                    .border_color(border)
                    .when(!self.status.is_empty() && !self.busy, |d| {
                        d.child(div().text_xs().text_color(muted).child(self.status.clone()))
                    })
                    .child(
                        div()
                            .key_context(crate::keymap::COMPOSER_CONTEXT)
                            .child(Textarea::new(&self.prompt).disabled(self.busy)),
                    )
                    .child(self.model_bar(cx)),
            )
    }
}
