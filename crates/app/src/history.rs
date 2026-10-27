//! History sidebar: saved snapshots of the open photo, then every edit; clicking a snapshot
//! applies it and clicking an edit jumps there (doc 01 §5).

use aigle_doc::Origin;
use gpui::{
    Context, Entity, IntoElement, ParentElement, Render, SharedString, Styled, Window, div,
    prelude::*, px,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::{ActiveTheme, Sizable, h_flex, v_flex};

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

fn origin_tag(o: &Origin) -> Option<String> {
    match o {
        Origin::User => None,
        Origin::Agent { .. } => Some("AI".into()),
        Origin::Preset => Some(crate::i18n::t("预设")),
        Origin::Profile => Some(crate::i18n::t("配置")),
        Origin::ExternalReload => Some(crate::i18n::t("外部")),
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

        let rows = std::iter::once((crate::i18n::t("打开照片").to_string(), None))
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
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(crate::i18n::history_label(&label)),
                    )
                    .when_some(tag, |d, t| d.child(div().text_color(muted).child(t)))
            })
            .collect::<Vec<_>>();

        let snapshots = self
            .photo
            .read(cx)
            .snapshots
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let apply = photo.clone();
                let remove = photo.clone();
                h_flex()
                    .px_3()
                    .py_1()
                    .gap_2()
                    .text_xs()
                    .hover(|d| d.bg(theme.list_hover))
                    .child(
                        div()
                            .id(SharedString::from(format!("snapshot-{i}")))
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .cursor_pointer()
                            .on_click(move |_, _, cx| {
                                apply.update(cx, |p, cx| p.apply_snapshot(i, cx))
                            })
                            .child(s.name.clone()),
                    )
                    .child(
                        Button::new(SharedString::from(format!("snapshot-del-{i}")))
                            .label("×")
                            .xsmall()
                            .ghost()
                            .tooltip(crate::i18n::t("删除快照"))
                            .on_click(move |_, _, cx| {
                                remove.update(cx, |p, cx| p.remove_snapshot(i, cx))
                            }),
                    )
            })
            .collect::<Vec<_>>();
        let no_snapshots = snapshots.is_empty();
        let add = photo.clone();
        let heading = |s: String| {
            div()
                .text_sm()
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .child(s)
        };

        v_flex()
            .size_full()
            .child(
                h_flex()
                    .px_3()
                    .py_2()
                    .justify_between()
                    .child(heading(crate::i18n::t("快照_标题")))
                    .child(
                        Button::new("snapshot-add")
                            .label("+")
                            .xsmall()
                            .ghost()
                            .tooltip(crate::i18n::t("将当前设置存为快照"))
                            .on_click(move |_, _, cx| add.update(cx, |p, cx| p.add_snapshot(cx))),
                    ),
            )
            .child(
                div()
                    .id("snapshot-scroll")
                    .flex_none()
                    .max_h(px(180.0))
                    .overflow_y_scroll()
                    .when(no_snapshots, |d| {
                        d.child(
                            div()
                                .px_3()
                                .pb_1()
                                .text_xs()
                                .text_color(muted)
                                .child(crate::i18n::t("暂无快照")),
                        )
                    })
                    .child(v_flex().children(snapshots)),
            )
            .child(
                div()
                    .px_3()
                    .py_2()
                    .child(heading(crate::i18n::t("历史记录"))),
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
