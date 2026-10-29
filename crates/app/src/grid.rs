//! Library grid: thumbnails of the open folder with rating and color label.

use gpui::{
    App, Context, Div, Entity, IntoElement, ObjectFit, ParentElement, Render, ScrollStrategy,
    SharedString, Stateful, Styled, StyledImage, UniformListScrollHandle, Window, div, img,
    prelude::*, px, rgb, uniform_list,
};
use gpui_component::{ActiveTheme, h_flex, v_flex};

use crate::library::{Consumer, Item, Library, LibraryEvent, label_color};

const CELL: f32 = 200.0;
const GAP: f32 = 8.0;
const THUMB_H: f32 = 136.0;
const PAD: f32 = 12.0;

pub struct Grid {
    library: Entity<Library>,
    scroll: UniformListScrollHandle,
    cols: usize,
    /// Window width taken by panels beside the grid, in px.
    pub side: f32,
}

impl Grid {
    pub fn new(library: Entity<Library>, cx: &mut Context<Self>) -> Grid {
        cx.observe(&library, |_, _, cx| cx.notify()).detach();
        Grid {
            library,
            scroll: UniformListScrollHandle::new(),
            cols: 1,
            side: 0.0,
        }
    }

    /// Move the selection by `dx` photos and `dy` rows.
    pub fn move_selection(&mut self, dx: isize, dy: isize, cx: &mut Context<Self>) {
        let cols = self.cols as isize;
        let target = self.library.update(cx, |l, cx| {
            let n = l.len() as isize;
            if n == 0 {
                return None;
            }
            let at = l.selected.unwrap_or(0) as isize;
            let to = (at + dx + dy * cols).clamp(0, n - 1) as usize;
            l.select(to, cx);
            Some(to)
        });
        if let Some(i) = target {
            self.reveal(i);
        }
    }

    pub fn reveal(&self, index: usize) {
        self.scroll
            .scroll_to_item(index / self.cols.max(1), ScrollStrategy::Nearest);
    }
}

/// `★★★` for a rating, `✕` for rejected.
pub fn stars(rating: i8) -> String {
    if rating < 0 {
        "✕".into()
    } else {
        "★".repeat(rating as usize)
    }
}

/// Thumbnail of `item`, a placeholder while it loads, or why it cannot be shown.
pub fn thumb_or_placeholder(item: &Item, cx: &App) -> gpui::AnyElement {
    match &item.thumb {
        Some(t) => div()
            .relative()
            .size_full()
            .overflow_hidden()
            .child(
                img(t.clone())
                    .absolute()
                    .inset_0()
                    .size_full()
                    .object_fit(ObjectFit::Contain),
            )
            .into_any_element(),
        None if item.unreadable.is_some() => {
            let why = item.unreadable.clone().unwrap_or_default();
            div()
                .id(SharedString::from(format!("unreadable-{}", item.name())))
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_xs()
                .text_color(cx.theme().danger)
                .child(crate::i18n::t("grid.unreadable"))
                .tooltip(move |window, cx| {
                    gpui_component::tooltip::Tooltip::new(why.clone()).build(window, cx)
                })
                .into_any_element()
        }
        None => div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .text_xs()
            .text_color(cx.theme().muted_foreground)
            .child("…")
            .into_any_element(),
    }
}

/// Select on click (Ctrl adds or removes, Shift selects a range), open in develop after
/// `clicks_to_open` plain clicks.
pub fn on_pick(
    d: Stateful<Div>,
    lib: &Entity<Library>,
    index: usize,
    clicks_to_open: usize,
) -> Stateful<Div> {
    let lib = lib.clone();
    d.on_click(move |e, _, cx| {
        let m = e.modifiers();
        lib.update(cx, |l, cx| {
            if m.shift {
                l.extend_select(index, cx);
            } else if m.secondary() {
                l.toggle_select(index, cx);
            } else {
                l.select(index, cx);
                if e.click_count() >= clicks_to_open {
                    cx.emit(LibraryEvent::Open(index));
                }
            }
        });
    })
}

fn cell(
    item: &Item,
    index: usize,
    (selected, primary): (bool, bool),
    folder: Option<String>,
    lib: &Entity<Library>,
    cx: &App,
) -> Stateful<Div> {
    let theme = cx.theme();
    let name = div()
        .id(SharedString::from(format!("cell-name-{index}")))
        .w_full()
        .truncate()
        .text_xs()
        .text_center()
        .child(item.name())
        .when_some(folder, |d, folder| {
            let tip = format!("{folder}/{}", item.name());
            d.tooltip(move |window, cx| {
                gpui_component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
            })
        });
    let d = v_flex()
        .id(SharedString::from(format!("cell-{index}")))
        .w(px(CELL - GAP))
        .h(px(CELL - GAP))
        .p_2()
        .gap_1()
        .rounded_md()
        .border_1()
        .border_color(gpui::transparent_black())
        .cursor_pointer()
        .when(selected, |d| d.bg(theme.list_active))
        .when(primary, |d| d.border_color(theme.primary))
        .when(!selected, |d| d.hover(|d| d.bg(theme.list_hover)))
        .child(
            div()
                .w_full()
                .h(px(THUMB_H))
                .child(thumb_or_placeholder(item, cx)),
        )
        .child(name)
        .child(
            h_flex()
                .h(px(14.0))
                .gap_1()
                .justify_center()
                .text_xs()
                .when_some(item.label().and_then(label_color), |d, c| {
                    d.child(div().size(px(8.0)).rounded_full().bg(rgb(c)))
                })
                .child(div().text_color(rgb(0xe8c547)).child(stars(item.rating())))
                .when(item.edited(), |d| {
                    d.child(
                        div()
                            .text_color(theme.muted_foreground)
                            .child(crate::i18n::t("grid.edited")),
                    )
                }),
        );
    on_pick(d, lib, index, 2)
}

impl Render for Grid {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let width = f32::from(window.viewport_size().width) - self.side - 2.0 * PAD;
        self.cols = (((width + GAP) / CELL).floor() as usize).max(1);
        let cols = self.cols;
        let (n, loading, folder, total) = {
            let l = self.library.read(cx);
            (l.len(), l.loading, l.dir.is_some(), l.total())
        };
        if n == 0 {
            let msg = match (folder, loading) {
                (false, _) => crate::keymap::hint("grid.empty_hint", cx),
                (true, true) => crate::i18n::t("grid.reading"),
                (true, false) if total > 0 => crate::i18n::t("grid.none_match"),
                (true, false) => crate::i18n::t("grid.no_photos"),
            };
            return div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(cx.theme().muted_foreground)
                .child(msg)
                .into_any_element();
        }
        uniform_list(
            "grid",
            n.div_ceil(cols),
            cx.processor(
                move |this: &mut Grid, range: std::ops::Range<usize>, _, cx| {
                    let lib = this.library.clone();
                    lib.update(cx, |l, cx| {
                        l.want(Consumer::Grid, range.start * cols..range.end * cols, cx)
                    });
                    let l = lib.read(cx);
                    let multi = l.selection_len() > 1;
                    range
                        .map(|row| {
                            h_flex().px(px(PAD)).pb(px(GAP)).gap(px(GAP)).children(
                                (row * cols..((row + 1) * cols).min(n)).filter_map(|i| {
                                    let primary = multi && l.selected == Some(i);
                                    let it = l.item(i)?;
                                    let folder = l.subfolder(it);
                                    Some(cell(it, i, (l.is_selected(i), primary), folder, &lib, cx))
                                }),
                            )
                        })
                        .collect::<Vec<_>>()
                },
            ),
        )
        .track_scroll(&self.scroll)
        .size_full()
        .pt(px(PAD))
        .into_any_element()
    }
}
