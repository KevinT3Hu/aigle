//! Mask tool panel: add, pick and delete local adjustments and edit the selected one
//! (doc 02 §9). Shapes are edited on the canvas.

use aigle_core::masks::{LocalAdjustment, LocalParams, MaskShape};
use aigle_doc::{GestureId, ParamOp};
use aigle_engine_api::Interaction;
use gpui::{
    Context, Entity, IntoElement, ParentElement, Render, SharedString, Styled, Subscription,
    Window, div, prelude::*,
};
use gpui_component::button::Button;
use gpui_component::slider::{Slider, SliderEvent, SliderState};
use gpui_component::{ActiveTheme, Selectable, Sizable, h_flex, v_flex};
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
}

fn local_label(name: &str) -> &'static str {
    match name {
        "exposure" => "曝光",
        "temperature" => "色温",
        "tint" => "色调",
        "contrast" => "对比度",
        "highlights" => "高光",
        "shadows" => "阴影",
        "whites" => "白色色阶",
        "blacks" => "黑色色阶",
        "texture" => "纹理",
        "clarity" => "清晰度",
        "dehaze" => "去朦胧",
        "saturation" => "饱和度",
        "hue" => "色相",
        "sharpness" => "锐化程度",
        "noise" => "降噪",
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
            (Key::BrushSize, "大小", 0.2, 20.0, 0.1, 3.0),
            (Key::BrushFeather, "羽化", 0.0, 100.0, 1.0, 50.0),
            (Key::BrushFlow, "流量", 1.0, 100.0, 1.0, 100.0),
            (Key::Opacity, "不透明度", 0.0, 100.0, 1.0, 100.0),
            (Key::Feather, "羽化", 0.0, 100.0, 1.0, 50.0),
        ];
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
                let sub = cx.subscribe_in(&state, window, move |p, _, ev, _, cx| {
                    p.on_slider(i, ev, cx)
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
            PhotoEvent::Loaded => {}
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

    fn on_slider(&mut self, i: usize, ev: &SliderEvent, cx: &mut Context<Self>) {
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
        let label = format!("蒙版 · {}", self.knobs[i].label);
        self.photo.update(cx, |p, cx| {
            p.command(&label, vec![op], Some(gesture), hint, cx)
        });
        if hint == Interaction::Final {
            self.gesture = None;
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
                                        "蒙版 · 重置",
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
                    .child(k.label)
                    .child(div().text_color(cx.theme().muted_foreground).child(value)),
            )
            .child(Slider::new(&k.state))
    }

    fn add_button(&self, kind: NewMask, pending: Option<NewMask>) -> Button {
        let photo = self.photo.clone();
        Button::new(SharedString::from(format!("add-{kind:?}")))
            .label(format!("+ {}", kind.label()))
            .small()
            .selected(pending == Some(kind))
            .tooltip("然后在画面上拖动放置")
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
                .when(m.invert, |d| d.child(div().text_color(muted).child("反相")))
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
        let title = |s: &'static str| {
            div()
                .text_sm()
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .child(s)
        };
        let mut body = v_flex().gap_4().p_3().child(
            v_flex()
                .gap_2()
                .child(title("蒙版"))
                .child(
                    h_flex()
                        .gap_1()
                        .flex_wrap()
                        .child(self.add_button(NewMask::Linear, pending))
                        .child(self.add_button(NewMask::Radial, pending))
                        .child(self.add_button(NewMask::Brush, pending)),
                )
                .when(masks.is_empty(), |d| {
                    d.child(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child("还没有蒙版。选择一种类型后在画面上拖动。"),
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
                Button::new("mask-invert")
                    .label("反相")
                    .small()
                    .selected(invert)
                    .on_click({
                        let photo = photo.clone();
                        move |_, _, cx| {
                            photo.update(cx, |p, cx| {
                                if let Some((i, _)) = p.selected() {
                                    p.command(
                                        "蒙版 · 反相",
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
                Button::new("mask-overlay")
                    .label("显示叠加")
                    .small()
                    .selected(overlay)
                    .tooltip("用红色显示蒙版范围（O）")
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
            .child(Button::new("mask-delete").label("删除").small().on_click({
                let photo = photo.clone();
                move |_, _, cx| photo.update(cx, |p, cx| p.delete_mask(id, cx))
            }));
        let is_brush = matches!(sel.shape, MaskShape::Brush { .. });
        if is_brush {
            tools_row = tools_row.child(
                Button::new("brush-erase")
                    .label("擦除")
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
            .child(title("形状"))
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
        body = body.child(shape);
        let mut adjust = v_flex().gap_2().child(title("调整"));
        for i in 0..LocalParams::SPECS.len() {
            adjust = adjust.child(self.knob(Key::Local(i), cx));
        }
        body.child(adjust)
    }
}
