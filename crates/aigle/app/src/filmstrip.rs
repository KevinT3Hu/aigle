//! Filmstrip under the develop canvas: the folder's photos in a horizontal strip; clicking
//! one switches to it. Only the visible stretch is laid out and asks for thumbnails.

use gpui::{
    Context, Entity, IntoElement, ParentElement, Render, ScrollHandle, SharedString, Styled,
    Window, div, point, prelude::*, px, rgb,
};
use gpui_component::{ActiveTheme, h_flex, v_flex};

use crate::grid::{on_pick, stars, thumb_or_placeholder};
use crate::library::{Consumer, Library, label_color};

const ITEM_W: f32 = 120.0;
pub const HEIGHT: f32 = 104.0;
/// Extra thumbnails requested on each side of the visible stretch.
const AHEAD: usize = 4;

pub struct Filmstrip {
    library: Entity<Library>,
    scroll: ScrollHandle,
    revealed: Option<usize>,
}

impl Filmstrip {
    pub fn new(library: Entity<Library>, cx: &mut Context<Self>) -> Filmstrip {
        cx.observe(&library, |_, _, cx| cx.notify()).detach();
        Filmstrip {
            library,
            scroll: ScrollHandle::new(),
            revealed: None,
        }
    }

    /// Scroll so that `index` is visible, centred if it was off-screen.
    fn reveal(&mut self, index: usize, width: f32) {
        let left = -f32::from(self.scroll.offset().x);
        let x = index as f32 * ITEM_W;
        if x < left || x + ITEM_W > left + width {
            let to = (x + ITEM_W / 2.0 - width / 2.0).max(0.0);
            self.scroll.set_offset(point(px(-to), px(0.0)));
        }
        self.revealed = Some(index);
    }
}

impl Render for Filmstrip {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (n, selected) = {
            let l = self.library.read(cx);
            (l.len(), l.selected)
        };
        let bounds = self.scroll.bounds();
        let width = if bounds.size.width > px(0.0) {
            f32::from(bounds.size.width)
        } else {
            f32::from(window.viewport_size().width)
        };
        if let Some(s) = selected
            && self.revealed != Some(s)
        {
            self.reveal(s, width);
        }
        let left = -f32::from(self.scroll.offset().x);
        let first = ((left / ITEM_W).floor().max(0.0) as usize).min(n);
        let last = (((left + width) / ITEM_W).ceil() as usize + 1).min(n);
        self.library.update(cx, |l, cx| {
            l.want(
                Consumer::Filmstrip,
                first.saturating_sub(AHEAD)..last + AHEAD,
                cx,
            )
        });

        let theme = cx.theme();
        let l = self.library.read(cx);
        let multi = l.selection_len() > 1;
        let items = (first..last).filter_map(|i| {
            let it = l.item(i)?;
            let picked = l.is_selected(i);
            let d = v_flex()
                .id(SharedString::from(format!("strip-{i}")))
                .flex_none()
                .w(px(ITEM_W))
                .h_full()
                .p_1()
                .gap_0p5()
                .rounded_sm()
                .border_1()
                .border_color(gpui::transparent_black())
                .cursor_pointer()
                .when(picked, |d| d.bg(theme.list_active))
                .when(multi && selected == Some(i), |d| {
                    d.border_color(theme.primary)
                })
                .when(!picked, |d| d.hover(|d| d.bg(theme.list_hover)))
                .child(
                    div()
                        .w_full()
                        .flex_1()
                        .min_h_0()
                        .child(thumb_or_placeholder(it, cx)),
                )
                .child(
                    h_flex()
                        .h(px(12.0))
                        .gap_1()
                        .justify_center()
                        .text_size(px(10.0))
                        .when_some(it.label().and_then(label_color), |d, c| {
                            d.child(div().size(px(6.0)).rounded_full().bg(rgb(c)))
                        })
                        .child(div().text_color(rgb(0xe8c547)).child(stars(it.rating()))),
                );
            Some(on_pick(d, &self.library, i, 1))
        });

        div()
            .id("filmstrip")
            .h(px(HEIGHT))
            .w_full()
            .flex_none()
            .border_t_1()
            .border_color(theme.border)
            .overflow_x_scroll()
            .track_scroll(&self.scroll)
            .child(
                h_flex()
                    .h_full()
                    .py_1()
                    .w(px(n as f32 * ITEM_W))
                    .child(div().flex_none().w(px(first as f32 * ITEM_W)))
                    .children(items),
            )
    }
}
