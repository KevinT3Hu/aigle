//! Spot removal and red-eye panel: the settings of the selected spot or eye (or of the next
//! spot when none is selected), delete and clear. Circles are placed on the canvas.

use aigle_core::retouch::{MAX_RADIUS, MIN_RADIUS, SpotMode};
use aigle_doc::{GestureId, ParamOp};
use aigle_engine_api::Interaction;
use gpui::{
    Context, Entity, IntoElement, ParentElement, Render, SharedString, Styled, Subscription,
    Window, div, prelude::*,
};
use gpui_component::button::Button;
use gpui_component::slider::{Slider, SliderEvent, SliderState};
use gpui_component::{ActiveTheme, Selectable, Sizable, h_flex, v_flex};

use crate::photo::{Photo, PhotoEvent, Tool};
use crate::tools;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Key {
    SpotSize,
    SpotFeather,
    SpotOpacity,
    EyeSize,
    EyePupil,
    EyeDarken,
}

struct Knob {
    key: Key,
    label: &'static str,
    state: Entity<SliderState>,
    _sub: Subscription,
}

pub struct RetouchPanel {
    photo: Entity<Photo>,
    knobs: Vec<Knob>,
    gesture: Option<GestureId>,
    error: Option<String>,
}

/// Radii are shown as a percentage of the long edge.
const PCT: f32 = 100.0;

impl RetouchPanel {
    pub fn new(photo: Entity<Photo>, window: &mut Window, cx: &mut Context<Self>) -> RetouchPanel {
        let (rmin, rmax) = (MIN_RADIUS * PCT, MAX_RADIUS * PCT);
        let defs: [(Key, &'static str, f32, f32, f32, f32); 6] = [
            (Key::SpotSize, "大小", rmin, 10.0, 0.05, 1.5),
            (Key::SpotFeather, "羽化", 0.0, 100.0, 1.0, 50.0),
            (Key::SpotOpacity, "不透明度", 0.0, 100.0, 1.0, 100.0),
            (Key::EyeSize, "大小", rmin, rmax.min(10.0), 0.05, 1.2),
            (Key::EyePupil, "瞳孔范围", 0.0, 100.0, 1.0, 50.0),
            (Key::EyeDarken, "变暗", 0.0, 100.0, 1.0, 50.0),
        ];
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
        let mut panel = RetouchPanel {
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
        let b = p.spot_brush;
        let spot = p.spot().map(|(_, s)| s);
        let eye = p.eye().map(|(_, e)| e);
        Some(match key {
            Key::SpotSize => spot.map_or(b.size, |s| s.radius) * PCT,
            Key::SpotFeather => spot.map_or(b.feather, |s| s.feather),
            Key::SpotOpacity => spot.map_or(b.opacity, |s| s.opacity) * 100.0,
            Key::EyeSize => eye?.radius * PCT,
            Key::EyePupil => eye?.pupil,
            Key::EyeDarken => eye?.darken,
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
        // Spot settings are also the defaults of the next spot.
        self.photo.update(cx, |p, cx| {
            match key {
                Key::SpotSize => p.spot_brush.size = v / PCT,
                Key::SpotFeather => p.spot_brush.feather = v,
                Key::SpotOpacity => p.spot_brush.opacity = v / 100.0,
                _ => return,
            }
            cx.notify();
        });
        let (op, label) = {
            let p = self.photo.read(cx);
            match key {
                Key::SpotSize | Key::SpotFeather | Key::SpotOpacity => {
                    let Some((idx, _)) = p.spot() else { return };
                    let (field, value) = match key {
                        Key::SpotSize => ("radius", v / PCT),
                        Key::SpotFeather => ("feather", v),
                        _ => ("opacity", v / 100.0),
                    };
                    (
                        ParamOp::set(&format!("spots.{idx}.{field}"), value),
                        crate::i18n::t("污点去除"),
                    )
                }
                _ => {
                    let Some((idx, _)) = p.eye() else { return };
                    let (field, value) = match key {
                        Key::EyeSize => ("radius", v / PCT),
                        Key::EyePupil => ("pupil", v),
                        _ => ("darken", v),
                    };
                    (
                        ParamOp::set(&format!("red_eye.{idx}.{field}"), value),
                        crate::i18n::t("红眼校正"),
                    )
                }
            }
        };
        let gesture = match self.gesture {
            Some(g) => g,
            None => {
                let g = self.photo.update(cx, |p, _| p.new_gesture());
                self.gesture = Some(g);
                g
            }
        };
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
        let value = if matches!(key, Key::SpotSize | Key::EyeSize) {
            format!("{v:.2}%")
        } else {
            format!("{v:.0}")
        };
        v_flex()
            .gap_0p5()
            .child(
                h_flex()
                    .justify_between()
                    .text_xs()
                    .child(crate::i18n::t(k.label))
                    .child(div().text_color(cx.theme().muted_foreground).child(value)),
            )
            .child(Slider::new(&k.state))
    }

    fn mode_button(&self, mode: SpotMode, current: SpotMode) -> Button {
        let photo = self.photo.clone();
        let label = match mode {
            SpotMode::Heal => crate::i18n::t("修复"),
            SpotMode::Clone => crate::i18n::t("仿制"),
        };
        Button::new(SharedString::from(format!("spot-{mode:?}")))
            .label(label)
            .small()
            .selected(mode == current)
            .on_click(move |_, _, cx| {
                photo.update(cx, |p, cx| {
                    p.spot_brush.mode = mode;
                    if let Some((i, s)) = p.spot()
                        && s.mode != mode
                    {
                        p.command(
                            crate::i18n::t("污点去除 · 方式"),
                            vec![ParamOp::set(
                                &format!("spots.{i}.mode"),
                                serde_json::to_value(mode).expect("serialisable"),
                            )],
                            None,
                            Interaction::Final,
                            cx,
                        );
                    }
                    cx.notify();
                })
            })
    }

    fn render_spots(&self, cx: &mut Context<Self>) -> gpui::Div {
        let (count, mode, selected) = {
            let p = self.photo.read(cx);
            let sel = p.spot().map(|(i, s)| (i, s.mode));
            (
                p.head().spots.len(),
                sel.map_or(p.spot_brush.mode, |(_, m)| m),
                sel.map(|(i, _)| i),
            )
        };
        let muted = cx.theme().muted_foreground;
        let photo = self.photo.clone();
        v_flex()
            .gap_2()
            .child(title(crate::i18n::t("污点去除")))
            .child(
                h_flex()
                    .gap_1()
                    .child(self.mode_button(SpotMode::Heal, mode))
                    .child(self.mode_button(SpotMode::Clone, mode)),
            )
            .child(div().text_xs().text_color(muted).child(if count == 0 {
                crate::i18n::t("点击污点去除，拖动可手动选择源区域。")
            } else {
                rust_i18n::t!("共 {n} 处", n = count).into_owned()
            }))
            .child(self.knob(Key::SpotSize, cx))
            .child(self.knob(Key::SpotFeather, cx))
            .child(self.knob(Key::SpotOpacity, cx))
            .child(
                h_flex()
                    .gap_1()
                    .when_some(selected, |d, i| {
                        let photo = photo.clone();
                        d.child(
                            Button::new("spot-delete")
                                .label(crate::i18n::t("删除"))
                                .small()
                                .on_click(move |_, _, cx| {
                                    photo.update(cx, |p, cx| p.delete_spot(i, cx))
                                }),
                        )
                    })
                    .when(count > 0, |d| {
                        d.child(
                            Button::new("spot-clear")
                                .label(crate::i18n::t("全部清除"))
                                .small()
                                .on_click(move |_, _, cx| {
                                    photo.update(cx, |p, cx| {
                                        p.command(
                                            crate::i18n::t("清除污点"),
                                            vec![tools::spots_op(&[])],
                                            None,
                                            Interaction::Final,
                                            cx,
                                        );
                                        p.selected_spot = None;
                                    })
                                }),
                        )
                    }),
            )
    }

    fn render_eyes(&self, cx: &mut Context<Self>) -> gpui::Div {
        let (count, selected) = {
            let p = self.photo.read(cx);
            (p.head().red_eye.len(), p.eye().map(|(i, _)| i))
        };
        let muted = cx.theme().muted_foreground;
        let photo = self.photo.clone();
        let mut body = v_flex()
            .gap_2()
            .child(title(crate::i18n::t("红眼校正")))
            .child(div().text_xs().text_color(muted).child(if count == 0 {
                crate::i18n::t("点击瞳孔，或从瞳孔中心拖出范围。")
            } else {
                rust_i18n::t!("共 {n} 处", n = count).into_owned()
            }));
        if selected.is_some() {
            body = body
                .child(self.knob(Key::EyeSize, cx))
                .child(self.knob(Key::EyePupil, cx))
                .child(self.knob(Key::EyeDarken, cx));
        }
        body.child(
            h_flex()
                .gap_1()
                .when_some(selected, |d, i| {
                    let photo = photo.clone();
                    d.child(
                        Button::new("eye-delete")
                            .label(crate::i18n::t("删除"))
                            .small()
                            .on_click(move |_, _, cx| {
                                photo.update(cx, |p, cx| p.delete_eye(i, cx))
                            }),
                    )
                })
                .when(count > 0, |d| {
                    d.child(
                        Button::new("eye-clear")
                            .label(crate::i18n::t("全部清除"))
                            .small()
                            .on_click(move |_, _, cx| {
                                photo.update(cx, |p, cx| {
                                    p.command(
                                        crate::i18n::t("清除红眼校正"),
                                        vec![tools::eyes_op(&[])],
                                        None,
                                        Interaction::Final,
                                        cx,
                                    );
                                    p.selected_eye = None;
                                })
                            }),
                    )
                }),
        )
    }
}

fn title(s: String) -> gpui::Div {
    div()
        .text_sm()
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .child(s)
}

impl Render for RetouchPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tool = self.photo.read(cx).tool;
        let mut body = v_flex().gap_4().p_3();
        body = match tool {
            Tool::RedEye => body.child(self.render_eyes(cx)),
            _ => body.child(self.render_spots(cx)),
        };
        if let Some(e) = &self.error {
            body = body.child(
                div()
                    .text_xs()
                    .text_color(cx.theme().danger)
                    .child(e.clone()),
            );
        }
        body
    }
}
