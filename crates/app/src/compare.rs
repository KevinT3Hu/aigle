//! Independent engine views for culling; editing is left to the develop workspace.
use crate::{
    canvas::{Canvas, Navigation},
    library::Library,
    photo::Photo,
};
use aigle_library::PhotoKey;
use gpui::{
    Context, Entity, EventEmitter, IntoElement, ParentElement, Render, Styled, Subscription,
    Window, div, prelude::*, px,
};
use gpui_component::{ActiveTheme, Selectable, Sizable, h_flex, v_flex};

struct Candidate {
    key: PhotoKey,
    name: String,
    canvas: Entity<Canvas>,
}
pub struct Closed;
pub struct Survey {
    candidates: Vec<Candidate>,
    library: Entity<Library>,
    linked: bool,
    last_navigation: Option<Navigation>,
    _subs: Vec<Subscription>,
}
impl EventEmitter<Closed> for Survey {}
impl Survey {
    pub fn new(
        photos: Vec<(PhotoKey, String, Entity<Photo>)>,
        library: Entity<Library>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut candidates = Vec::new();
        let mut subs = vec![cx.observe(&library, |_, _, cx| cx.notify())];
        for (key, name, photo) in photos {
            let canvas = cx.new(|cx| Canvas::new(photo, window, cx));
            subs.push(cx.subscribe(&canvas, |s, from, nav: &Navigation, cx| {
                s.last_navigation = Some(*nav);
                if !s.linked {
                    return;
                }
                for c in &s.candidates {
                    if c.canvas != from {
                        c.canvas.update(cx, |c, cx| c.apply_navigation(*nav, cx));
                    }
                }
            }));
            candidates.push(Candidate { key, name, canvas });
        }
        Self {
            candidates,
            library,
            linked: true,
            last_navigation: None,
            _subs: subs,
        }
    }
}
impl Render for Survey {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let controls = h_flex()
            .gap_2()
            .p_2()
            .child(
                div()
                    .flex_1()
                    .child(crate::i18n::t("action.CompareSelected")),
            )
            .child(
                crate::ui::icon_button(
                    "link-views",
                    crate::ui::Glyph::Copy,
                    crate::i18n::t("compare.link"),
                )
                .small()
                .selected(self.linked)
                .on_click(cx.listener(|s, _, _, cx| {
                    s.linked = !s.linked;
                    if s.linked
                        && let Some(nav) = s.last_navigation
                    {
                        for c in &s.candidates {
                            c.canvas.update(cx, |c, cx| c.apply_navigation(nav, cx));
                        }
                    }
                    cx.notify();
                })),
            )
            .child(
                crate::ui::icon_button(
                    "close-compare",
                    crate::ui::Glyph::Close,
                    crate::i18n::t("common.done"),
                )
                .small()
                .on_click(cx.listener(|_, _, _, cx| cx.emit(Closed))),
            );
        let mut rows = v_flex().flex_1().min_h_0().gap_1();
        let columns = if self.candidates.len() > 2 {
            2
        } else {
            self.candidates.len().max(1)
        };
        for chunk in self.candidates.chunks(columns) {
            let mut row = h_flex().flex_1().min_h_0().gap_1();
            for c in chunk {
                let key = c.key.clone();
                let library = self.library.clone();
                let rating = library
                    .read(cx)
                    .item_by_key(&key)
                    .map_or(0, |it| it.rating());
                let header = h_flex()
                    .gap_1()
                    .px_2()
                    .h(px(30.0))
                    .child(div().flex_1().truncate().text_xs().child(c.name.clone()))
                    .child(
                        crate::ui::icon_button(
                            gpui::SharedString::from(format!("reject-{}", c.name)),
                            crate::ui::Glyph::Disable,
                            crate::i18n::t(if rating < 0 {
                                "compare.unreject"
                            } else {
                                "compare.reject"
                            }),
                        )
                        .xsmall()
                        .selected(rating < 0)
                        .on_click(move |_, _, cx| {
                            library.update(cx, |l, cx| {
                                l.set_rating_by_key(&key, if rating < 0 { 0 } else { -1 }, cx);
                            });
                        }),
                    )
                    .child(
                        crate::ui::icon_button(
                            gpui::SharedString::from(format!("remove-{}", c.name)),
                            crate::ui::Glyph::Close,
                            crate::i18n::t("compare.remove"),
                        )
                        .xsmall()
                        .on_click({
                            let key = c.key.clone();
                            cx.listener(move |s, _, _, cx| {
                                s.candidates.retain(|c| c.key != key);
                                if s.candidates.is_empty() {
                                    cx.emit(Closed);
                                }
                                cx.notify();
                            })
                        }),
                    );
                row = row.child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .border_1()
                        .border_color(cx.theme().border)
                        .child(header)
                        .child(div().flex_1().min_h_0().child(c.canvas.clone())),
                );
            }
            rows = rows.child(row);
        }
        v_flex().flex_1().min_h_0().child(controls).child(rows)
    }
}
