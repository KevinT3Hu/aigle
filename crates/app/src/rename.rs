//! The rename dialog: a new name for a photo (its extension is kept) or a virtual copy.

use gpui::{
    Context, Entity, EventEmitter, IntoElement, ParentElement, Render, Styled, Subscription,
    Window, div, prelude::*, px,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::{ActiveTheme, Sizable, h_flex, v_flex};

pub enum RenameEvent {
    /// The dialog closed; `Some` with the typed name when confirmed.
    Done(Option<String>),
}

pub struct RenameDialog {
    title: String,
    /// Shown after the input (`.CR3`), empty for a virtual copy.
    suffix: String,
    name: Entity<InputState>,
    _sub: Subscription,
}

impl EventEmitter<RenameEvent> for RenameDialog {}

impl RenameDialog {
    pub fn new(
        title: String,
        current: String,
        suffix: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> RenameDialog {
        let name = cx.new(|cx| {
            let mut s = InputState::new(window, cx);
            s.set_value(current, window, cx);
            s
        });
        name.update(cx, |n, cx| n.focus(window, cx));
        let sub = cx.subscribe(&name, |d, _, ev: &InputEvent, cx| {
            if let InputEvent::PressEnter { .. } = ev {
                d.confirm(cx);
            }
        });
        RenameDialog {
            title,
            suffix,
            name,
            _sub: sub,
        }
    }

    fn confirm(&mut self, cx: &mut Context<Self>) {
        let name = self.name.read(cx).value().trim().to_string();
        cx.emit(RenameEvent::Done(Some(name)));
    }
}

impl Render for RenameDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let card = v_flex()
            .id("rename-card")
            .w(px(420.0))
            .p_4()
            .gap_3()
            .rounded_lg()
            .border_1()
            .border_color(theme.border)
            .bg(theme.popover)
            .shadow_lg()
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .text_base()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(self.title.clone()),
            )
            .child(
                h_flex()
                    .gap_1()
                    .child(div().flex_1().child(Input::new(&self.name).small()))
                    .when(!self.suffix.is_empty(), |d| {
                        d.child(
                            div()
                                .text_sm()
                                .text_color(theme.muted_foreground)
                                .child(self.suffix.clone()),
                        )
                    }),
            )
            .child(
                h_flex()
                    .pt_2()
                    .gap_2()
                    .justify_end()
                    .child(
                        Button::new("rename-cancel")
                            .label(crate::i18n::t("common.cancel"))
                            .small()
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(RenameEvent::Done(None)))),
                    )
                    .child(
                        Button::new("rename-go")
                            .label(crate::i18n::t("rename.confirm"))
                            .small()
                            .primary()
                            .on_click(cx.listener(|d, _, _, cx| d.confirm(cx))),
                    ),
            );
        div()
            .id("rename-backdrop")
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(gpui::rgba(0x00000080))
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|_, _, _, cx| cx.emit(RenameEvent::Done(None))),
            )
            .child(card)
    }
}
