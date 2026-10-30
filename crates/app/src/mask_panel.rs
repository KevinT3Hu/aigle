//! Mask tool panel: add, pick and delete local adjustments and edit the selected one
//! (doc 02 §9). Shapes are edited on the canvas.

use aigle_core::masks::{CombineOp, LocalAdjustment, LocalParams, MaskShape, RangeKind};
use aigle_doc::{GestureId, ParamOp};
use aigle_engine_api::Interaction;
use gpui::{
    Context, Entity, IntoElement, ParentElement, Render, SharedString, Styled, Subscription,
    Window, div, prelude::*,
};
use gpui_component::button::Button;
use gpui_component::menu::{DropdownMenu, PopupMenuItem};
use gpui_component::slider::{Slider, SliderEvent, SliderState};
use gpui_component::{ActiveTheme, Disableable, Selectable, Sizable, h_flex, v_flex};
use serde_json::Value;

use crate::photo::{Photo, PhotoEvent};
use crate::tools::{self, NewMask};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Key {
    /// Index into `LocalParams::SPECS`.
    Local(usize),
    Opacity,
    Feather,
    BrushSize,
    BrushFeather,
    BrushFlow,
    Low,
    High,
    Smoothness,
    Tolerance,
}

fn local_label(name: &str) -> &'static str {
    match name {
        "exposure" => "param.local.exposure",
        "temperature" => "param.local.temperature",
        "tint" => "param.local.tint",
        "contrast" => "param.local.contrast",
        "highlights" => "param.local.highlights",
        "shadows" => "param.local.shadows",
        "whites" => "param.local.whites",
        "blacks" => "param.local.blacks",
        "texture" => "param.local.texture",
        "clarity" => "param.local.clarity",
        "dehaze" => "param.local.dehaze",
        "saturation" => "param.local.saturation",
        "hue" => "param.local.hue",
        "sharpness" => "param.local.sharpness",
        "noise" => "param.local.noise",
        _ => "",
    }
}

struct Knob {
    key: Key,
    label: &'static str,
    state: Entity<SliderState>,
    decimals: usize,
    _sub: Subscription,
}

pub struct MaskPanel {
    photo: Entity<Photo>,
    knobs: Vec<Knob>,
    gesture: Option<GestureId>,
    error: Option<String>,
}

fn local_value(a: &LocalParams, name: &str) -> f32 {
    serde_json::to_value(a)
        .ok()
        .and_then(|j| j.get(name).and_then(Value::as_f64))
        .unwrap_or(0.0) as f32
}

impl MaskPanel {
    pub fn new(photo: Entity<Photo>, window: &mut Window, cx: &mut Context<Self>) -> MaskPanel {
        let mut defs: Vec<(Key, &'static str, f32, f32, f32, f32)> = vec![
            (Key::BrushSize, "masks.size", 0.2, 20.0, 0.1, 3.0),
            (Key::BrushFeather, "masks.feather", 0.0, 100.0, 1.0, 50.0),
            (Key::BrushFlow, "masks.flow", 1.0, 100.0, 1.0, 100.0),
            (Key::Opacity, "masks.opacity", 0.0, 100.0, 1.0, 100.0),
            (Key::Feather, "masks.feather", 0.0, 100.0, 1.0, 50.0),
        ];
        defs.extend([
            (Key::Low, "masks.range.low", 0.0, 1.0, 0.01, 0.0),
            (Key::High, "masks.range.high", 0.0, 1.0, 0.01, 1.0),
            (
                Key::Smoothness,
                "masks.range.smoothness",
                0.0,
                100.0,
                1.0,
                20.0,
            ),
            (
                Key::Tolerance,
                "masks.range.tolerance",
                0.0,
                100.0,
                1.0,
                20.0,
            ),
        ]);
        for (i, s) in LocalParams::SPECS.iter().enumerate() {
            defs.push((
                Key::Local(i),
                local_label(s.name),
                s.min,
                s.max,
                s.step,
                0.0,
            ));
        }
        let knobs = defs
            .into_iter()
            .enumerate()
            .map(|(i, (key, label, min, max, step, default))| {
                let state = cx.new(|_| {
                    SliderState::new()
                        .max(max)
                        .min(min)
                        .step(step)
                        .default_value(default)
                });
                let sub = cx.subscribe_in(&state, window, move |p, _, ev, window, cx| {
                    p.on_slider(i, ev, window, cx)
                });
                Knob {
                    key,
                    label,
                    state,
                    decimals: if step < 0.05 {
                        2
                    } else if step < 0.5 {
                        1
                    } else {
                        0
                    },
                    _sub: sub,
                }
            })
            .collect();
        cx.subscribe_in(&photo, window, |p, _, ev, window, cx| match ev {
            PhotoEvent::Edited | PhotoEvent::ToolChanged => {
                p.error = None;
                p.sync(window, cx)
            }
            PhotoEvent::Notice(m) => {
                p.error = Some(m.clone());
                cx.notify();
            }
            PhotoEvent::Loaded | PhotoEvent::SnapshotsChanged => {}
        })
        .detach();
        cx.observe_in(&photo, window, |p, _, window, cx| p.sync(window, cx))
            .detach();
        let mut panel = MaskPanel {
            photo,
            knobs,
            gesture: None,
            error: None,
        };
        panel.sync(window, cx);
        panel
    }

    fn value(&self, key: Key, cx: &Context<Self>) -> Option<f32> {
        let p = self.photo.read(cx);
        let b = p.brush;
        let sel = p.selected().map(|(_, m)| m);
        Some(match key {
            Key::BrushSize => b.size * 100.0,
            Key::BrushFeather => b.feather,
            Key::BrushFlow => b.flow,
            Key::Opacity => sel?.opacity * 100.0,
            Key::Feather => match &sel?.shape {
                MaskShape::Radial { feather, .. } => *feather,
                _ => return None,
            },
            Key::Low | Key::High => match &sel?.shape {
                MaskShape::Range {
                    range: RangeKind::Luminance { low, high },
                    ..
                } => {
                    if key == Key::Low {
                        *low
                    } else {
                        *high
                    }
                }
                _ => return None,
            },
            Key::Smoothness => match &sel?.shape {
                MaskShape::Range { smoothness, .. } => *smoothness,
                _ => return None,
            },
            Key::Tolerance => match &sel?.shape {
                MaskShape::Range {
                    range: RangeKind::Color { tolerance, .. },
                    ..
                } => *tolerance,
                _ => return None,
            },
            Key::Local(i) => local_value(&sel?.adjust, LocalParams::SPECS[i].name),
        })
    }

    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.gesture.is_some() {
            return;
        }
        let values: Vec<Option<f32>> = self.knobs.iter().map(|k| self.value(k.key, cx)).collect();
        for (k, v) in self.knobs.iter().zip(values) {
            if let Some(v) = v {
                k.state.update(cx, |s, cx| {
                    if s.value().start() != v {
                        s.set_value(v, window, cx);
                    }
                });
            }
        }
        cx.notify();
    }

    fn on_slider(
        &mut self,
        i: usize,
        ev: &SliderEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (v, hint) = match ev {
            SliderEvent::Change(v) => (v.start(), Interaction::Continuous),
            SliderEvent::Release(v) => (v.start(), Interaction::Final),
        };
        let key = self.knobs[i].key;
        let brush = matches!(key, Key::BrushSize | Key::BrushFeather | Key::BrushFlow);
        if brush {
            self.photo.update(cx, |p, cx| {
                match key {
                    Key::BrushSize => p.brush.size = v / 100.0,
                    Key::BrushFeather => p.brush.feather = v,
                    _ => p.brush.flow = v,
                }
                cx.notify();
            });
            return;
        }
        let Some(index) = self.photo.read(cx).selected().map(|(i, _)| i) else {
            return;
        };
        let gesture = match self.gesture {
            Some(g) => g,
            None => {
                let g = self.photo.update(cx, |p, _| p.new_gesture());
                self.gesture = Some(g);
                g
            }
        };
        let op = match key {
            Key::Opacity => ParamOp::set(&format!("masks.{index}.opacity"), v / 100.0),
            Key::Feather => ParamOp::set(&format!("masks.{index}.shape.feather"), v),
            Key::Low | Key::High | Key::Smoothness | Key::Tolerance => {
                let p = self.photo.read(cx);
                let mut shape = p.head().masks[index].shape.clone();
                if let MaskShape::Range { range, smoothness } = &mut shape {
                    match (key, range) {
                        (Key::Low, RangeKind::Luminance { low, high }) => *low = v.min(*high),
                        (Key::High, RangeKind::Luminance { low, high }) => *high = v.max(*low),
                        (Key::Tolerance, RangeKind::Color { tolerance, .. }) => *tolerance = v,
                        (Key::Smoothness, _) => *smoothness = v,
                        _ => return,
                    }
                }
                ParamOp::set(
                    &format!("masks.{index}.shape"),
                    serde_json::to_value(shape).unwrap(),
                )
            }
            Key::Local(j) => {
                let path = format!("masks.{index}.adjust.{}", LocalParams::SPECS[j].name);
                let value = if v == 0.0 {
                    Value::Null
                } else {
                    Value::from(v)
                };
                ParamOp::Set {
                    path: path.as_str().into(),
                    value,
                }
            }
            _ => return,
        };
        let label = crate::i18n::join(&["masks.title", self.knobs[i].label]);
        self.photo.update(cx, |p, cx| {
            p.command(&label, vec![op], Some(gesture), hint, cx)
        });
        if hint == Interaction::Final {
            self.gesture = None;
            self.sync(window, cx);
        }
    }

    fn knob(&self, key: Key, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let k = self
            .knobs
            .iter()
            .find(|k| k.key == key)
            .expect("every key has a knob");
        let v = k.state.read(cx).value().start();
        let value = format!("{v:.*}", k.decimals);
        let photo = self.photo.clone();
        v_flex()
            .gap_0p5()
            .child(
                h_flex()
                    .id(SharedString::from(format!("knob-{key:?}")))
                    .justify_between()
                    .text_xs()
                    .on_click(move |e, _, cx| {
                        // Double-click resets a local parameter.
                        if e.click_count() >= 2
                            && let Key::Local(j) = key
                        {
                            photo.update(cx, |p, cx| {
                                if let Some((i, _)) = p.selected() {
                                    let path =
                                        format!("masks.{i}.adjust.{}", LocalParams::SPECS[j].name);
                                    p.command(
                                        crate::i18n::join(&["masks.title", "common.reset"]),
                                        vec![ParamOp::Set {
                                            path: path.as_str().into(),
                                            value: Value::Null,
                                        }],
                                        None,
                                        Interaction::Final,
                                        cx,
                                    );
                                }
                            });
                        }
                    })
                    .child(crate::i18n::t(k.label))
                    .child(div().text_color(cx.theme().muted_foreground).child(value)),
            )
            .child(Slider::new(&k.state))
    }

    fn add_button(&self, kind: NewMask, pending: Option<NewMask>) -> Button {
        let photo = self.photo.clone();
        crate::ui::icon_button(
            SharedString::from(format!("add-{kind:?}")),
            match kind {
                NewMask::Linear => crate::ui::Glyph::Gradient,
                NewMask::Radial => crate::ui::Glyph::Lens,
                NewMask::Brush => crate::ui::Glyph::Brush,
            },
            kind.label(),
        )
        .small()
        .selected(pending == Some(kind))
        .tooltip(format!(
            "{} — {}",
            kind.label(),
            crate::i18n::t("masks.place_tooltip")
        ))
        .on_click(move |_, _, cx| {
            photo.update(cx, |p, cx| {
                p.pending_mask = if p.pending_mask == Some(kind) {
                    None
                } else {
                    Some(kind)
                };
                cx.notify();
            })
        })
    }

    fn range_button(&self, color: bool) -> Button {
        let photo = self.photo.clone();
        let label = crate::i18n::t(if color {
            "masks.range.color"
        } else {
            "masks.range.luminance"
        });
        crate::ui::icon_button(
            if color { "range-color" } else { "range-luma" },
            if color {
                crate::ui::Glyph::Picker
            } else {
                crate::ui::Glyph::Filter
            },
            label.clone(),
        )
        .small()
        .on_click(move |_, _, cx| {
            photo.update(cx, |p, cx| {
                let mut mask = tools::new_mask(p.head(), NewMask::Brush, Default::default());
                mask.name = label.clone();
                mask.shape = MaskShape::Range {
                    range: if color {
                        RangeKind::Color {
                            samples: vec![[0.18; 3]],
                            tolerance: 20.0,
                        }
                    } else {
                        RangeKind::Luminance {
                            low: 0.0,
                            high: 1.0,
                        }
                    },
                    smoothness: 20.0,
                };
                let id = mask.id;
                if p.command(
                    &label,
                    vec![ParamOp::AddMask { mask }],
                    None,
                    Interaction::Final,
                    cx,
                ) {
                    p.pending_mask = None;
                    p.select_mask(Some(id), cx);
                    if color {
                        p.color_mask_pick = Some(id);
                    }
                }
            })
        })
    }

    fn list(&self, masks: &[LocalAdjustment], cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let selected = self.photo.read(cx).selected_mask;
        let (active, hover, muted) = (theme.list_active, theme.list_hover, theme.muted_foreground);
        v_flex().children(masks.iter().map(|m| {
            let id = m.id;
            let photo = self.photo.clone();
            h_flex()
                .id(SharedString::from(format!("mask-{}", id.0)))
                .px_2()
                .py_1()
                .gap_2()
                .rounded_sm()
                .text_xs()
                .cursor_pointer()
                .when(selected == Some(id), |d| d.bg(active))
                .hover(|d| d.bg(hover))
                .on_click(move |_, _, cx| photo.update(cx, |p, cx| p.select_mask(Some(id), cx)))
                .child(div().flex_1().truncate().child(m.name.clone()))
                .child(div().text_color(muted).child(tools::kind_label(&m.shape)))
                .when(m.invert, |d| {
                    d.child(
                        div()
                            .text_color(muted)
                            .child(crate::i18n::t("masks.invert")),
                    )
                })
        }))
    }
}

impl Render for MaskPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (masks, pending, selected, overlay, erase) = {
            let p = self.photo.read(cx);
            (
                p.head().masks.clone(),
                p.pending_mask,
                p.selected().map(|(_, m)| m.clone()),
                p.mask_overlay,
                p.brush.erase,
            )
        };
        let muted = cx.theme().muted_foreground;
        let title = |s: String| {
            div()
                .text_sm()
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .child(s)
        };
        let mut body = v_flex().gap_4().p_3().child(
            v_flex()
                .gap_2()
                .child(title(crate::i18n::t("masks.title")))
                .child(
                    h_flex()
                        .gap_1()
                        .flex_wrap()
                        .child(self.add_button(NewMask::Linear, pending))
                        .child(self.add_button(NewMask::Radial, pending))
                        .child(self.add_button(NewMask::Brush, pending))
                        .child(self.range_button(false))
                        .child(self.range_button(true)),
                )
                .when(masks.is_empty(), |d| {
                    d.child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child(crate::i18n::t("masks.empty")),
                    )
                })
                .child(self.list(&masks, cx)),
        );
        if let Some(e) = &self.error {
            body = body.child(
                div()
                    .text_xs()
                    .text_color(cx.theme().danger)
                    .child(e.clone()),
            );
        }
        let Some(sel) = selected else {
            return body;
        };
        let photo = self.photo.clone();
        let id = sel.id;
        let invert = sel.invert;
        let mut tools_row = h_flex()
            .gap_1()
            .flex_wrap()
            .child(
                crate::ui::icon_button(
                    "mask-invert",
                    crate::ui::Glyph::Invert,
                    crate::i18n::t("masks.invert"),
                )
                .small()
                .selected(invert)
                .on_click({
                    let photo = photo.clone();
                    move |_, _, cx| {
                        photo.update(cx, |p, cx| {
                            if let Some((i, _)) = p.selected() {
                                p.command(
                                    crate::i18n::join(&["masks.title", "masks.invert"]),
                                    vec![ParamOp::set(&format!("masks.{i}.invert"), !invert)],
                                    None,
                                    Interaction::Final,
                                    cx,
                                );
                            }
                        })
                    }
                }),
            )
            .child(
                crate::ui::icon_button(
                    "mask-overlay",
                    crate::ui::Glyph::View,
                    crate::i18n::t("masks.show_overlay"),
                )
                .small()
                .selected(overlay)
                .tooltip(crate::keymap::hint("masks.overlay_tooltip", cx))
                .on_click({
                    let photo = photo.clone();
                    move |_, _, cx| {
                        photo.update(cx, |p, cx| {
                            p.mask_overlay = !p.mask_overlay;
                            cx.notify();
                        })
                    }
                }),
            )
            .child(
                crate::ui::icon_button(
                    "mask-delete",
                    crate::ui::Glyph::Delete,
                    crate::i18n::t("common.delete"),
                )
                .small()
                .on_click({
                    let photo = photo.clone();
                    move |_, _, cx| photo.update(cx, |p, cx| p.delete_mask(id, cx))
                }),
            );
        let is_brush = matches!(sel.shape, MaskShape::Brush { .. });
        if is_brush {
            tools_row = tools_row.child(
                crate::ui::icon_button(
                    "brush-erase",
                    crate::ui::Glyph::Erase,
                    crate::i18n::t("masks.erase"),
                )
                .small()
                .selected(erase)
                .on_click(move |_, _, cx| {
                    photo.update(cx, |p, cx| {
                        p.brush.erase = !p.brush.erase;
                        cx.notify();
                    })
                }),
            );
        }
        let mut shape = v_flex()
            .gap_2()
            .child(title(crate::i18n::t("masks.shape")))
            .child(tools_row)
            .child(self.knob(Key::Opacity, cx));
        if matches!(sel.shape, MaskShape::Radial { .. }) {
            shape = shape.child(self.knob(Key::Feather, cx));
        }
        if is_brush {
            shape = shape
                .child(self.knob(Key::BrushSize, cx))
                .child(self.knob(Key::BrushFeather, cx))
                .child(self.knob(Key::BrushFlow, cx));
        }
        if let MaskShape::Range { range, .. } = &sel.shape {
            shape = shape.child(self.knob(Key::Smoothness, cx));
            match range {
                RangeKind::Luminance { .. } => {
                    shape = shape
                        .child(self.knob(Key::Low, cx))
                        .child(self.knob(Key::High, cx));
                }
                RangeKind::Color { .. } => {
                    let photo = self.photo.clone();
                    shape = shape.child(self.knob(Key::Tolerance, cx)).child(
                        crate::ui::icon_button(
                            "range-pick",
                            crate::ui::Glyph::Picker,
                            crate::i18n::t("masks.range.pick"),
                        )
                        .small()
                        .selected(self.photo.read(cx).color_mask_pick == Some(id))
                        .on_click(move |_, _, cx| {
                            photo.update(cx, |p, cx| {
                                p.color_mask_pick = if p.color_mask_pick == Some(id) {
                                    None
                                } else {
                                    Some(id)
                                };
                                p.wb_pick = false;
                                cx.notify();
                            })
                        }),
                    );
                }
            }
        }
        // A snapshot of another mask's coverage; its adjustment stays independent.
        for (op, key, glyph) in [
            (CombineOp::Add, "masks.combine.add", crate::ui::Glyph::Add),
            (
                CombineOp::Subtract,
                "masks.combine.subtract",
                crate::ui::Glyph::Erase,
            ),
            (
                CombineOp::Intersect,
                "masks.combine.intersect",
                crate::ui::Glyph::Filter,
            ),
        ] {
            let photo = self.photo.clone();
            let choices: Vec<_> = masks.iter().filter(|m| m.id != id).cloned().collect();
            let label = crate::i18n::t(key);
            shape = shape.child(
                crate::ui::labeled_button(SharedString::from(key), glyph, label.clone())
                    .small()
                    .disabled(choices.is_empty())
                    .tooltip(crate::i18n::t("masks.combine.hint"))
                    .dropdown_menu(move |menu, _, _| {
                        choices.iter().fold(menu, |menu, target| {
                            let target = target.clone();
                            let photo = photo.clone();
                            let label = label.clone();
                            menu.item(PopupMenuItem::new(target.name.clone()).on_click(
                                move |_, _, cx| {
                                    photo.update(cx, |p, cx| {
                                        if let Some((i, selected)) = p.selected() {
                                            let shape = combine_coverage(selected, &target, op);
                                            if stack_usage(&shape) > 8 {
                                                cx.emit(PhotoEvent::Notice(crate::i18n::t(
                                                    "masks.combine.limit",
                                                )));
                                                return;
                                            }
                                            p.command(
                                                &label,
                                                vec![
                                                    ParamOp::set(
                                                        &format!("masks.{i}.shape"),
                                                        serde_json::to_value(shape).unwrap(),
                                                    ),
                                                    ParamOp::set(
                                                        &format!("masks.{i}.invert"),
                                                        false,
                                                    ),
                                                ],
                                                None,
                                                Interaction::Final,
                                                cx,
                                            );
                                        }
                                    });
                                },
                            ))
                        })
                    }),
            );
        }
        body = body.child(shape);
        let mut adjust = v_flex()
            .gap_2()
            .child(title(crate::i18n::t("masks.adjustments")));
        for i in 0..LocalParams::SPECS.len() {
            adjust = adjust.child(self.knob(Key::Local(i), cx));
        }
        body.child(adjust)
    }
}

/// Inversion belongs to coverage; opacity and local adjustments remain independent.
fn coverage(mask: &LocalAdjustment) -> MaskShape {
    if mask.invert {
        MaskShape::Combine {
            op: CombineOp::Subtract,
            parts: vec![
                MaskShape::Range {
                    range: RangeKind::Luminance {
                        low: 0.0,
                        high: 1.0,
                    },
                    smoothness: 0.0,
                },
                mask.shape.clone(),
            ],
        }
    } else {
        mask.shape.clone()
    }
}

fn combine_coverage(a: &LocalAdjustment, b: &LocalAdjustment, op: CombineOp) -> MaskShape {
    MaskShape::Combine {
        op,
        parts: vec![coverage(a), coverage(b)],
    }
}

fn stack_usage(shape: &MaskShape) -> usize {
    match shape {
        MaskShape::Combine { parts, .. } => parts
            .iter()
            .enumerate()
            .map(|(i, p)| stack_usage(p) + usize::from(i > 0))
            .max()
            .unwrap_or(1),
        _ => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn combinations_preserve_inversion_and_roundtrip() {
        let mut a = tools::new_mask(&Default::default(), NewMask::Brush, Default::default());
        a.invert = true;
        let b = a.clone();
        let shape = combine_coverage(&a, &b, CombineOp::Intersect);
        let MaskShape::Combine {
            op: CombineOp::Intersect,
            parts,
        } = &shape
        else {
            panic!()
        };
        assert!(matches!(
            parts[0],
            MaskShape::Combine {
                op: CombineOp::Subtract,
                ..
            }
        ));
        let roundtrip: MaskShape =
            serde_json::from_value(serde_json::to_value(&shape).unwrap()).unwrap();
        assert_eq!(shape, roundtrip);
        assert!(a.invert);
    }
}
