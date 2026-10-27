//! Copy settings from one photo and paste them onto others, by adjustment group.
//!
//! Copying opens a dialog choosing the groups; pasting applies the copied values of those
//! groups as one command, so it is a single undo step on the open photo.

use std::collections::BTreeSet;
use std::sync::Arc;

use aigle_core::EditParams;
use aigle_doc::{ParamOp, value_at};
use gpui::{
    Context, EventEmitter, IntoElement, ParentElement, Render, SharedString, Styled, Window, div,
    prelude::*, px,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::{ActiveTheme, Selectable, Sizable, h_flex, v_flex};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Group {
    Profile,
    WhiteBalance,
    Light,
    Presence,
    Curve,
    Mixer,
    Grading,
    Detail,
    Lens,
    Effects,
    Raw,
    Geometry,
    Masks,
}

impl Group {
    pub const ALL: [Group; 13] = [
        Group::Profile,
        Group::WhiteBalance,
        Group::Light,
        Group::Presence,
        Group::Curve,
        Group::Mixer,
        Group::Grading,
        Group::Detail,
        Group::Lens,
        Group::Effects,
        Group::Raw,
        Group::Geometry,
        Group::Masks,
    ];

    /// The `EditParams` field the group copies.
    pub fn path(self) -> &'static str {
        match self {
            Group::Profile => "profile",
            Group::WhiteBalance => "wb",
            Group::Light => "light",
            Group::Presence => "presence",
            Group::Curve => "curve",
            Group::Mixer => "mixer",
            Group::Grading => "grading",
            Group::Detail => "detail",
            Group::Lens => "lens",
            Group::Effects => "effects",
            Group::Raw => "raw",
            Group::Geometry => "geometry",
            Group::Masks => "masks",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Group::Profile => "配置文件",
            Group::WhiteBalance => "白平衡",
            Group::Light => "影调",
            Group::Presence => "偏好",
            Group::Curve => "曲线",
            Group::Mixer => "颜色",
            Group::Grading => "分级",
            Group::Detail => "细节",
            Group::Lens => "镜头校正",
            Group::Effects => "效果",
            Group::Raw => "RAW 处理",
            Group::Geometry => "裁剪与变换",
            Group::Masks => "蒙版",
        }
    }

    /// Crop and masks are placed for one picture, so they are not copied unless asked.
    pub fn defaults() -> BTreeSet<Group> {
        Group::ALL
            .into_iter()
            .filter(|g| !matches!(g, Group::Geometry | Group::Masks))
            .collect()
    }
}

/// Copied settings: the source edit and the groups to paste from it.
#[derive(Clone, Debug, PartialEq)]
pub struct Clipboard {
    pub params: Arc<EditParams>,
    pub groups: BTreeSet<Group>,
}

impl Clipboard {
    /// Ops replacing the copied groups of the target with the source's values.
    pub fn ops(&self) -> Vec<ParamOp> {
        if self.groups.is_empty() {
            return Vec::new();
        }
        let mut ops = vec![ParamOp::set(
            "process_version",
            value_at(&self.params, "process_version"),
        )];
        ops.extend(
            self.groups
                .iter()
                .map(|g| ParamOp::set(g.path(), value_at(&self.params, g.path()))),
        );
        ops
    }

    /// `target` with the clipboard pasted onto it.
    pub fn paste_onto(&self, target: &EditParams) -> Result<EditParams, aigle_doc::CommandError> {
        aigle_doc::PhotoDoc::new(target.clone())?.preview(&self.ops())
    }
}

pub enum CopyEvent {
    /// The dialog closed; `Some` with the chosen groups when the user copied.
    Done(Option<BTreeSet<Group>>),
}

pub struct CopyDialog {
    groups: BTreeSet<Group>,
}

impl EventEmitter<CopyEvent> for CopyDialog {}

impl CopyDialog {
    pub fn new(groups: BTreeSet<Group>) -> CopyDialog {
        CopyDialog { groups }
    }

    fn toggle(&mut self, g: Group, cx: &mut Context<Self>) {
        if !self.groups.remove(&g) {
            self.groups.insert(g);
        }
        cx.notify();
    }

    fn set_all(&mut self, all: bool, cx: &mut Context<Self>) {
        self.groups = if all {
            Group::ALL.into_iter().collect()
        } else {
            BTreeSet::new()
        };
        cx.notify();
    }
}

impl Render for CopyDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (bg, border) = (theme.popover, theme.border);
        let groups = h_flex()
            .gap_1()
            .flex_wrap()
            .children(Group::ALL.into_iter().map(|g| {
                Button::new(SharedString::from(format!("copy-{g:?}")))
                    .label(crate::i18n::t(g.label()))
                    .small()
                    .selected(self.groups.contains(&g))
                    .on_click(cx.listener(move |d, _, _, cx| d.toggle(g, cx)))
            }));
        let card = v_flex()
            .id("copy-card")
            .w(px(440.0))
            .p_4()
            .gap_3()
            .rounded_lg()
            .border_1()
            .border_color(border)
            .bg(bg)
            .shadow_lg()
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .text_base()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(crate::i18n::t("拷贝设置")),
            )
            .child(groups)
            .child(
                h_flex()
                    .gap_1()
                    .child(
                        Button::new("copy-all")
                            .label(crate::i18n::t("全选"))
                            .xsmall()
                            .ghost()
                            .on_click(cx.listener(|d, _, _, cx| d.set_all(true, cx))),
                    )
                    .child(
                        Button::new("copy-none")
                            .label(crate::i18n::t("全不选"))
                            .xsmall()
                            .ghost()
                            .on_click(cx.listener(|d, _, _, cx| d.set_all(false, cx))),
                    ),
            )
            .child(
                h_flex()
                    .pt_2()
                    .gap_2()
                    .justify_end()
                    .child(
                        Button::new("copy-cancel")
                            .label(crate::i18n::t("取消"))
                            .small()
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(CopyEvent::Done(None)))),
                    )
                    .child(
                        Button::new("copy-go")
                            .label(crate::i18n::t("拷贝"))
                            .small()
                            .primary()
                            .on_click(cx.listener(|d, _, _, cx| {
                                cx.emit(CopyEvent::Done(Some(d.groups.clone())))
                            })),
                    ),
            );
        div()
            .id("copy-backdrop")
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(gpui::rgba(0x00000080))
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|_, _, _, cx| cx.emit(CopyEvent::Done(None))),
            )
            .child(card)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edited() -> EditParams {
        let mut p = EditParams::default();
        p.light.exposure = 1.5;
        p.wb.values.tint = 20.0;
        p.geometry.scalars.angle = 3.0;
        p.raw.ca_correction = false;
        p
    }

    #[test]
    fn every_group_path_exists() {
        let d = EditParams::default();
        for g in Group::ALL {
            assert!(!value_at(&d, g.path()).is_null(), "{g:?}");
        }
    }

    #[test]
    fn pastes_only_the_chosen_groups() {
        let clip = Clipboard {
            params: Arc::new(edited()),
            groups: Group::defaults(),
        };
        let mut target = EditParams::default();
        target.presence.clarity = 30.0;
        target.geometry.scalars.angle = -2.0;
        let out = clip.paste_onto(&target).unwrap();
        assert_eq!(out.light.exposure, 1.5);
        assert_eq!(out.wb.values.tint, 20.0);
        assert!(!out.raw.ca_correction);
        // Presence is copied from the source, whose clarity is the default.
        assert_eq!(out.presence.clarity, 0.0);
        // Geometry is not copied by default.
        assert_eq!(out.geometry.scalars.angle, -2.0);

        let clip = Clipboard {
            groups: [Group::Geometry].into_iter().collect(),
            ..clip
        };
        let out = clip.paste_onto(&target).unwrap();
        assert_eq!(out.geometry.scalars.angle, 3.0);
        assert_eq!(out.presence.clarity, 30.0);
        assert_eq!(out.light.exposure, 0.0);
    }

    #[test]
    fn empty_selection_changes_nothing() {
        let clip = Clipboard {
            params: Arc::new(edited()),
            groups: BTreeSet::new(),
        };
        assert!(clip.ops().is_empty());
        let target = EditParams::default();
        assert_eq!(clip.paste_onto(&target).unwrap(), target);
    }
}
