//! History sidebar: every edit of the open photo; clicking one jumps there (doc 01 §5).

use aigle_doc::Origin;
use gpui::{
    Context, Entity, IntoElement, ParentElement, Render, SharedString, Styled, Window, div,
    prelude::*,
};
use gpui_component::{ActiveTheme, h_flex, v_flex};

use crate::photo::Photo;

pub struct HistoryPanel {
    photo: Entity<Photo>,
}

impl HistoryPanel {
    pub fn new(photo: Entity<Photo>, cx: &mut Context<Self>) -> HistoryPanel {
        cx.observe(&photo, |_, _, cx| cx.notify()).detach();
        HistoryPanel { photo }
    }
}

fn origin_tag(o: &Origin) -> Option<&'static str> {
    match o {
        Origin::User => None,
        Origin::Agent { .. } => Some("AI"),
        Origin::Preset => Some("预设"),
        Origin::Profile => Some("配置"),
        Origin::ExternalReload => Some("外部"),
    }
}

impl Render for HistoryPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let doc = &self.photo.read(cx).doc;
        let items = doc.history();
        let at = doc.position();
        let theme = cx.theme();
        let (fg, muted, active) = (theme.foreground, theme.muted_foreground, theme.list_active);
        let photo = self.photo.clone();

        let rows = std::iter::once(("打开照片".to_string(), None))
            .chain(
                items
                    .iter()
                    .map(|e| (e.label.clone(), origin_tag(&e.origin))),
            )
            .enumerate()
            .map(|(i, (label, tag))| {
                let photo = photo.clone();
                h_flex()
                    .id(SharedString::from(format!("history-{i}")))
                    .px_3()
                    .py_1()
                    .gap_2()
                    .text_xs()
                    .cursor_pointer()
                    .text_color(if i <= at { fg } else { muted })
                    .when(i == at, |d| d.bg(active))
                    .hover(|d| d.bg(theme.list_hover))
                    .on_click(move |_, _, cx| photo.update(cx, |p, cx| p.goto(i, cx)))
                    .child(div().flex_1().min_w_0().truncate().child(label))
                    .when_some(tag, |d, t| d.child(div().text_color(muted).child(t)))
            })
            .collect::<Vec<_>>();

        v_flex()
            .size_full()
            .child(
                div()
                    .px_3()
                    .py_2()
                    .text_sm()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child("历史记录"),
            )
            .child(
                div()
                    .id("history-scroll")
                    .flex_1()
                    .overflow_y_scroll()
                    .child(v_flex().children(rows)),
            )
    }
}
