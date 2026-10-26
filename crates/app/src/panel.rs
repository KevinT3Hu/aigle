//! Develop panel: histogram and the global adjustments, grouped into tabs.
//!
//! Each slider edits one parameter path through `doc`: `Continuous` while dragging, `Final`
//! on release, all commands of one drag sharing a gesture so they form one undo step.

use std::sync::Arc;

use aigle_core::EditParams;
use aigle_core::params::{Aspect, LensProfile, WbMode};
use aigle_doc::{EditCommand, GestureId, ParamOp};
use aigle_engine_api::{Histogram, Interaction, watch};
use gpui::{
    App, Context, Entity, IntoElement, ParentElement, PathBuilder, Render, SharedString, Styled,
    Subscription, Task, Window, canvas, div, point, prelude::*, px, rgb, rgba,
};
use gpui_component::button::Button;
use gpui_component::slider::{Slider, SliderEvent, SliderScale, SliderState};
use gpui_component::tab::TabBar;
use gpui_component::{ActiveTheme, Selectable, Sizable, h_flex, v_flex};

use crate::curve::CurveEditor;
use crate::mask_panel::MaskPanel;
use crate::meta;
use crate::photo::{Photo, PhotoEvent, Tool};

const INFO_TAB: &str = "信息";

/// Buttons shown above a section's sliders.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Extra {
    None,
    WhiteBalance,
    LensProfile,
    PointCurve,
}

pub struct Section {
    pub title: &'static str,
    pub extra: Extra,
    /// (path, label).
    pub rows: &'static [(&'static str, &'static str)],
}

pub struct TabDef {
    pub title: &'static str,
    pub sections: &'static [Section],
}

const fn section(title: &'static str, rows: &'static [(&'static str, &'static str)]) -> Section {
    Section {
        title,
        extra: Extra::None,
        rows,
    }
}

pub const TABS: &[TabDef] = &[
    TabDef {
        title: "基本",
        sections: &[
            Section {
                title: "白平衡",
                extra: Extra::WhiteBalance,
                rows: &[("wb.temperature", "色温"), ("wb.tint", "色调")],
            },
            section(
                "影调",
                &[
                    ("light.exposure", "曝光"),
                    ("light.contrast", "对比度"),
                    ("light.highlights", "高光"),
                    ("light.shadows", "阴影"),
                    ("light.whites", "白色色阶"),
                    ("light.blacks", "黑色色阶"),
                ],
            ),
            section(
                "偏好",
                &[
                    ("presence.texture", "纹理"),
                    ("presence.clarity", "清晰度"),
                    ("presence.dehaze", "去朦胧"),
                    ("presence.vibrance", "自然饱和度"),
                    ("presence.saturation", "饱和度"),
                ],
            ),
        ],
    },
    TabDef {
        title: "曲线",
        sections: &[
            Section {
                title: "点曲线",
                extra: Extra::PointCurve,
                rows: &[],
            },
            section(
                "参数曲线",
                &[
                    ("curve.parametric.highlights", "高光"),
                    ("curve.parametric.lights", "亮调"),
                    ("curve.parametric.darks", "暗调"),
                    ("curve.parametric.shadows", "阴影"),
                ],
            ),
        ],
    },
    TabDef {
        title: "颜色",
        sections: &[
            section(
                "色相",
                &[
                    ("mixer.red.hue", "红色"),
                    ("mixer.orange.hue", "橙色"),
                    ("mixer.yellow.hue", "黄色"),
                    ("mixer.green.hue", "绿色"),
                    ("mixer.aqua.hue", "浅绿色"),
                    ("mixer.blue.hue", "蓝色"),
                    ("mixer.purple.hue", "紫色"),
                    ("mixer.magenta.hue", "洋红"),
                ],
            ),
            section(
                "饱和度",
                &[
                    ("mixer.red.saturation", "红色"),
                    ("mixer.orange.saturation", "橙色"),
                    ("mixer.yellow.saturation", "黄色"),
                    ("mixer.green.saturation", "绿色"),
                    ("mixer.aqua.saturation", "浅绿色"),
                    ("mixer.blue.saturation", "蓝色"),
                    ("mixer.purple.saturation", "紫色"),
                    ("mixer.magenta.saturation", "洋红"),
                ],
            ),
            section(
                "明亮度",
                &[
                    ("mixer.red.luminance", "红色"),
                    ("mixer.orange.luminance", "橙色"),
                    ("mixer.yellow.luminance", "黄色"),
                    ("mixer.green.luminance", "绿色"),
                    ("mixer.aqua.luminance", "浅绿色"),
                    ("mixer.blue.luminance", "蓝色"),
                    ("mixer.purple.luminance", "紫色"),
                    ("mixer.magenta.luminance", "洋红"),
                ],
            ),
        ],
    },
    TabDef {
        title: "分级",
        sections: &[
            section(
                "阴影",
                &[
                    ("grading.shadows.hue", "色相"),
                    ("grading.shadows.saturation", "饱和度"),
                    ("grading.shadows.luminance", "明亮度"),
                ],
            ),
            section(
                "中间调",
                &[
                    ("grading.midtones.hue", "色相"),
                    ("grading.midtones.saturation", "饱和度"),
                    ("grading.midtones.luminance", "明亮度"),
                ],
            ),
            section(
                "高光",
                &[
                    ("grading.highlights.hue", "色相"),
                    ("grading.highlights.saturation", "饱和度"),
                    ("grading.highlights.luminance", "明亮度"),
                ],
            ),
            section(
                "全局",
                &[
                    ("grading.global.hue", "色相"),
                    ("grading.global.saturation", "饱和度"),
                    ("grading.global.luminance", "明亮度"),
                ],
            ),
            section(
                "混合",
                &[("grading.blending", "混合"), ("grading.balance", "平衡")],
            ),
        ],
    },
    TabDef {
        title: "细节",
        sections: &[
            section(
                "锐化",
                &[
                    ("detail.sharpen.amount", "数量"),
                    ("detail.sharpen.radius", "半径"),
                    ("detail.sharpen.detail", "细节"),
                    ("detail.sharpen.masking", "蒙版"),
                ],
            ),
            section(
                "降噪",
                &[
                    ("detail.noise.luma", "明亮度"),
                    ("detail.noise.detail", "明亮度细节"),
                    ("detail.noise.contrast", "明亮度对比"),
                    ("detail.noise.chroma", "颜色"),
                    ("detail.noise.chroma_detail", "颜色细节"),
                ],
            ),
        ],
    },
    TabDef {
        title: "光学",
        sections: &[
            Section {
                title: "镜头校正",
                extra: Extra::LensProfile,
                rows: &[
                    ("lens.distortion", "畸变"),
                    ("lens.tca", "色差"),
                    ("lens.vignetting", "暗角"),
                ],
            },
            section(
                "变换",
                &[
                    ("geometry.angle", "角度"),
                    ("geometry.perspective_vertical", "垂直透视"),
                    ("geometry.perspective_horizontal", "水平透视"),
                ],
            ),
        ],
    },
    TabDef {
        title: "效果",
        sections: &[
            section(
                "暗角",
                &[
                    ("effects.vignette.amount", "数量"),
                    ("effects.vignette.midpoint", "中点"),
                    ("effects.vignette.roundness", "圆度"),
                    ("effects.vignette.feather", "羽化"),
                ],
            ),
            section(
                "颗粒",
                &[
                    ("effects.grain.amount", "数量"),
                    ("effects.grain.size", "大小"),
                    ("effects.grain.roughness", "粗糙度"),
                ],
            ),
        ],
    },
];

struct Row {
    path: &'static str,
    label: &'static str,
    /// History label, e.g. "饱和度 · 蓝色".
    history: String,
    state: Entity<SliderState>,
    decimals: usize,
    _sub: Subscription,
}

pub struct Panel {
    photo: Entity<Photo>,
    rows: Vec<Row>,
    tab: usize,
    histogram: Option<Arc<Histogram>>,
    curve: Entity<CurveEditor>,
    masks: Entity<MaskPanel>,
    gesture: Option<GestureId>,
    job: Option<Task<()>>,
    lens_name: Option<String>,
    error: Option<String>,
}

fn decimals(step: f32) -> usize {
    let mut d = 0;
    let mut s = step;
    while d < 4 && (s - s.round()).abs() > 1e-4 {
        s *= 10.0;
        d += 1;
    }
    d
}

/// The value a slider shows: As Shot white balance shows the as-shot values until the user
/// edits it.
fn displayed(photo: &Photo, path: &str) -> f32 {
    let head = photo.head();
    if head.wb.mode == WbMode::AsShot
        && let Some(info) = photo.info()
    {
        match path {
            "wb.temperature" => return info.as_shot.temperature,
            "wb.tint" => return info.as_shot.tint,
            _ => {}
        }
    }
    head.get(path).unwrap_or_default()
}

const WB_PATHS: [&str; 3] = ["wb.mode", "wb.temperature", "wb.tint"];

fn resets(paths: &[&str]) -> Vec<ParamOp> {
    paths
        .iter()
        .map(|p| ParamOp::Reset { path: (*p).into() })
        .collect()
}

impl Panel {
    pub fn new(
        photo: Entity<Photo>,
        mut histogram: watch::Receiver<Option<Arc<Histogram>>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Panel {
        let mut rows = Vec::new();
        for s in TABS.iter().flat_map(|t| t.sections) {
            for &(path, label) in s.rows {
                let spec = EditParams::default()
                    .spec(path)
                    .expect("panel paths are scalar params");
                let mut st = SliderState::new()
                    .max(spec.max)
                    .min(spec.min)
                    .step(spec.step)
                    .default_value(spec.default);
                if path == "wb.temperature" {
                    st = st.scale(SliderScale::Logarithmic);
                }
                let state = cx.new(|_| st);
                let i = rows.len();
                let sub = cx.subscribe_in(&state, window, move |p, _, ev, _, cx| {
                    p.on_slider(i, ev, cx)
                });
                rows.push(Row {
                    path,
                    label,
                    history: format!("{} · {label}", s.title),
                    state,
                    decimals: decimals(spec.step),
                    _sub: sub,
                });
            }
        }
        cx.subscribe_in(&photo, window, |p, _, ev, window, cx| match ev {
            PhotoEvent::Loaded | PhotoEvent::Edited => {
                p.error = None;
                p.sync(window, cx)
            }
            PhotoEvent::Notice(m) => {
                p.error = Some(m.clone());
                cx.notify();
            }
            PhotoEvent::ToolChanged => cx.notify(),
        })
        .detach();
        cx.observe(&photo, |_, _, cx| cx.notify()).detach();
        cx.spawn(async move |this, cx| {
            while histogram.changed().await.is_ok() {
                let h = histogram.borrow_and_update().clone();
                this.update(cx, |p, cx| {
                    p.curve.update(cx, |c, cx| {
                        c.histogram = h.clone();
                        cx.notify();
                    });
                    p.histogram = h;
                    cx.notify();
                })?;
            }
            anyhow::Ok(())
        })
        .detach();
        let curve = cx.new(|cx| CurveEditor::new(photo.clone(), cx));
        let masks = cx.new(|cx| MaskPanel::new(photo.clone(), window, cx));
        let mut panel = Panel {
            photo,
            rows,
            tab: 0,
            histogram: None,
            curve,
            masks,
            gesture: None,
            job: None,
            lens_name: None,
            error: None,
        };
        panel.sync(window, cx);
        panel
    }

    /// Move the sliders to the head (after load, undo, redo, edits from elsewhere).
    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.gesture.is_some() {
            return;
        }
        let photo = self.photo.read(cx);
        let values: Vec<f32> = self.rows.iter().map(|r| displayed(photo, r.path)).collect();
        for (r, v) in self.rows.iter().zip(values) {
            r.state.update(cx, |s, cx| {
                if s.value().start() != v {
                    s.set_value(v, window, cx);
                }
            });
        }
        cx.notify();
    }

    fn ops_for(&self, i: usize, v: f32, cx: &App) -> Vec<ParamOp> {
        let path = self.rows[i].path;
        let mut ops = vec![ParamOp::set(path, v)];
        if let Some(other) = match path {
            "wb.temperature" => Some("wb.tint"),
            "wb.tint" => Some("wb.temperature"),
            _ => None,
        } {
            let photo = self.photo.read(cx);
            ops.push(ParamOp::set(other, displayed(photo, other)));
            ops.push(ParamOp::set("wb.mode", "custom"));
        }
        ops
    }

    fn on_slider(&mut self, i: usize, ev: &SliderEvent, cx: &mut Context<Self>) {
        let (v, hint) = match ev {
            SliderEvent::Change(v) => (v.start(), Interaction::Continuous),
            SliderEvent::Release(v) => (v.start(), Interaction::Final),
        };
        let gesture = match self.gesture {
            Some(g) => g,
            None => {
                let g = self.photo.update(cx, |p, _| p.new_gesture());
                self.gesture = Some(g);
                g
            }
        };
        let ops = self.ops_for(i, v, cx);
        let label = self.rows[i].history.clone();
        self.send(&label, ops, Some(gesture), hint, cx);
        if hint == Interaction::Final {
            self.gesture = None;
        }
    }

    fn send(
        &mut self,
        label: &str,
        ops: Vec<ParamOp>,
        gesture: Option<GestureId>,
        hint: Interaction,
        cx: &mut Context<Self>,
    ) {
        let r = self.photo.update(cx, |p, cx| {
            let mut cmd = EditCommand::user(p.doc.rev(), label, ops);
            cmd.gesture = gesture;
            p.edit(cmd, hint, cx)
        });
        self.error = r.err().map(|e| e.to_string());
    }

    fn reset(&mut self, i: usize, cx: &mut Context<Self>) {
        let path = self.rows[i].path;
        let ops = if path.starts_with("wb.") {
            resets(&WB_PATHS)
        } else {
            resets(&[path])
        };
        let label = format!("重置 {}", self.rows[i].history);
        self.send(&label, ops, None, Interaction::Final, cx);
    }

    fn as_shot(&mut self, cx: &mut Context<Self>) {
        self.send(
            "白平衡 · 原照设置",
            resets(&WB_PATHS),
            None,
            Interaction::Final,
            cx,
        );
    }

    fn auto_wb(&mut self, cx: &mut Context<Self>) {
        let job = self.photo.read(cx).session.auto_white_balance(None);
        self.job = Some(cx.spawn(async move |this, cx| {
            let r = job.await;
            let _ = this.update(cx, |p, cx| match r {
                Ok(wb) => {
                    let ops = vec![
                        ParamOp::set("wb.temperature", wb.temperature),
                        ParamOp::set("wb.tint", wb.tint),
                        ParamOp::set("wb.mode", "auto"),
                    ];
                    p.send("白平衡 · 自动", ops, None, Interaction::Final, cx);
                    if let Some(why) = wb.low_confidence_reason {
                        p.error = Some(format!("自动白平衡置信度低：{why}"));
                    }
                }
                Err(e) => p.error = Some(e.to_string()),
            });
        }));
    }

    fn match_lens(&mut self, cx: &mut Context<Self>) {
        let job = self.photo.read(cx).session.match_lens();
        self.job = Some(cx.spawn(async move |this, cx| {
            let r = job.await;
            let _ = this.update(cx, |p, cx| {
                match r {
                    Ok(found) => match found.into_iter().next() {
                        Some(m) => {
                            let profile = LensProfile::Resolved(m.data_ref);
                            let value = serde_json::to_value(profile).expect("serialisable");
                            p.send(
                                "镜头校正 · 启用配置文件",
                                vec![ParamOp::set("lens.profile", value)],
                                None,
                                Interaction::Final,
                                cx,
                            );
                            p.lens_name = Some(format!("{} {}", m.lens_maker, m.lens_model));
                        }
                        None => p.error = Some("镜头数据库中没有匹配的镜头".into()),
                    },
                    Err(e) => p.error = Some(e.to_string()),
                }
                cx.notify();
            });
        }));
    }

    fn lens_off(&mut self, cx: &mut Context<Self>) {
        self.send(
            "镜头校正 · 关闭",
            resets(&["lens.profile"]),
            None,
            Interaction::Final,
            cx,
        );
    }

    fn render_extra(&self, extra: Extra, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        match extra {
            Extra::None => None,
            Extra::PointCurve => Some(self.curve.clone().into_any_element()),
            Extra::WhiteBalance => {
                let picking = self.photo.read(cx).wb_pick;
                Some(
                    h_flex()
                        .gap_2()
                        .child(
                            Button::new("wb-pick")
                                .label("吸管")
                                .small()
                                .selected(picking)
                                .tooltip("点击画面中应为中性灰的位置（W）")
                                .on_click(cx.listener(move |p, _, _, cx| {
                                    p.photo.update(cx, |ph, cx| ph.set_wb_pick(!picking, cx))
                                })),
                        )
                        .child(
                            Button::new("wb-as-shot")
                                .label("原照设置")
                                .small()
                                .on_click(cx.listener(|p, _, _, cx| p.as_shot(cx))),
                        )
                        .child(
                            Button::new("wb-auto")
                                .label("自动")
                                .small()
                                .on_click(cx.listener(|p, _, _, cx| p.auto_wb(cx))),
                        )
                        .into_any_element(),
                )
            }
            Extra::LensProfile => {
                let on = matches!(
                    self.photo.read(cx).head().lens.profile,
                    LensProfile::Resolved(_)
                );
                let status = match (on, &self.lens_name) {
                    (false, _) => "未启用配置文件".to_string(),
                    (true, Some(n)) => n.clone(),
                    (true, None) => "已启用配置文件".to_string(),
                };
                Some(
                    v_flex()
                        .gap_1()
                        .child(
                            h_flex()
                                .gap_2()
                                .child(
                                    Button::new("lens-match")
                                        .label("自动匹配")
                                        .small()
                                        .on_click(cx.listener(|p, _, _, cx| p.match_lens(cx))),
                                )
                                .child(
                                    Button::new("lens-off")
                                        .label("关闭")
                                        .small()
                                        .on_click(cx.listener(|p, _, _, cx| p.lens_off(cx))),
                                ),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(status),
                        )
                        .into_any_element(),
                )
            }
        }
    }

    fn render_row(&self, i: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let r = &self.rows[i];
        let v = r.state.read(cx).value().start();
        let value = format!("{v:.*}", r.decimals);
        v_flex()
            .gap_0p5()
            .child(
                h_flex()
                    .id(SharedString::from(format!("row-{}", r.path)))
                    .justify_between()
                    .text_xs()
                    .on_click(cx.listener(move |p, e: &gpui::ClickEvent, _, cx| {
                        if e.click_count() >= 2 {
                            p.reset(i, cx);
                        }
                    }))
                    .child(r.label)
                    .child(div().text_color(cx.theme().muted_foreground).child(value)),
            )
            .child(Slider::new(&r.state).accessibility_label(r.history.clone()))
    }

    fn render_crop(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let aspect = self.photo.read(cx).head().geometry.aspect;
        let photo = self.photo.clone();
        let presets: [(&str, Aspect); 7] = [
            ("自由", Aspect::Free),
            ("原始", Aspect::Original),
            ("1:1", Aspect::Ratio { w: 1, h: 1 }),
            ("4:5", Aspect::Ratio { w: 4, h: 5 }),
            ("3:2", Aspect::Ratio { w: 3, h: 2 }),
            ("4:3", Aspect::Ratio { w: 4, h: 3 }),
            ("16:9", Aspect::Ratio { w: 16, h: 9 }),
        ];
        let same = |a: Aspect| match (a, aspect) {
            (Aspect::Ratio { w, h }, Aspect::Ratio { w: w2, h: h2 }) => {
                (w, h) == (w2, h2) || (w, h) == (h2, w2)
            }
            (a, b) => a == b,
        };
        let button =
            |id: &'static str, label: &'static str, f: fn(&mut Photo, &mut Context<Photo>)| {
                let photo = photo.clone();
                Button::new(id)
                    .label(label)
                    .small()
                    .on_click(move |_, _, cx| photo.update(cx, f))
            };
        let angle = self
            .rows
            .iter()
            .position(|r| r.path == "geometry.angle")
            .expect("the angle is a panel row");
        let title = |s: &'static str| {
            div()
                .text_sm()
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .child(s)
        };
        v_flex()
            .gap_4()
            .p_3()
            .child(
                v_flex().gap_2().child(title("长宽比")).child(
                    h_flex()
                        .gap_1()
                        .flex_wrap()
                        .children(presets.into_iter().map(|(label, a)| {
                            let photo = photo.clone();
                            Button::new(SharedString::from(format!("aspect-{label}")))
                                .label(label)
                                .small()
                                .selected(same(a))
                                .on_click(move |_, _, cx| {
                                    photo.update(cx, |p, cx| p.set_aspect(a, cx))
                                })
                        }))
                        .when(matches!(aspect, Aspect::Ratio { w, h } if w != h), |d| {
                            let Aspect::Ratio { w, h } = aspect else {
                                return d;
                            };
                            let photo = photo.clone();
                            d.child(
                                Button::new("aspect-swap")
                                    .label("横竖互换")
                                    .small()
                                    .tooltip("交换长宽比的方向（X）")
                                    .on_click(move |_, _, cx| {
                                        photo.update(cx, |p, cx| {
                                            p.set_aspect(Aspect::Ratio { w: h, h: w }, cx)
                                        })
                                    }),
                            )
                        }),
                ),
            )
            .child(
                v_flex()
                    .gap_2()
                    .child(title("拉直"))
                    .child(self.render_row(angle, cx))
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child("在裁剪框外拖动也可以旋转；裁剪框会自动缩小以避免空白角。"),
                    ),
            )
            .child(
                v_flex().gap_2().child(title("方向")).child(
                    h_flex()
                        .gap_1()
                        .flex_wrap()
                        .child(button("rot-ccw", "向左旋转", |p, cx| {
                            p.rotate_quarter(false, cx)
                        }))
                        .child(button("rot-cw", "向右旋转", |p, cx| {
                            p.rotate_quarter(true, cx)
                        }))
                        .child(button("mirror", "水平翻转", Photo::mirror)),
                ),
            )
            .child(
                h_flex()
                    .gap_1()
                    .child(button("crop-reset", "重置", Photo::reset_crop))
                    .child(button("crop-done", "完成", |p, cx| {
                        p.set_tool(Tool::None, cx)
                    })),
            )
            .when_some(self.error.clone(), |d, e| {
                d.child(div().text_xs().text_color(cx.theme().danger).child(e))
            })
    }

    fn render_info(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let photo = self.photo.read(cx);
        let mut body = v_flex().gap_4().p_3();
        let Some(info) = photo.info() else {
            return body.child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("照片尚未加载"),
            );
        };
        let muted = cx.theme().muted_foreground;
        for s in meta::sections(info, &photo.file) {
            let mut sec = v_flex().gap_1().child(
                div()
                    .text_sm()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .mb_1()
                    .child(s.title),
            );
            for (k, v) in s.rows {
                sec = sec.child(
                    h_flex()
                        .gap_3()
                        .items_start()
                        .text_xs()
                        .child(div().w(px(76.0)).flex_none().text_color(muted).child(k))
                        .child(div().flex_1().min_w_0().child(v)),
                );
            }
            body = body.child(sec);
        }
        body
    }

    fn render_tab(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut body = v_flex().gap_4().p_3();
        let mut i: usize = TABS[..self.tab]
            .iter()
            .flat_map(|t| t.sections)
            .map(|s| s.rows.len())
            .sum();
        for s in TABS[self.tab].sections {
            let mut sec = v_flex().gap_2().child(
                div()
                    .text_sm()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(s.title),
            );
            if let Some(e) = self.render_extra(s.extra, cx) {
                sec = sec.child(e);
            }
            for _ in s.rows {
                sec = sec.child(self.render_row(i, cx));
                i += 1;
            }
            body = body.child(sec);
        }
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

fn histogram_element(h: Option<Arc<Histogram>>) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            window.paint_quad(gpui::fill(bounds, rgb(0x141414)));
            let Some(h) = h else { return };
            // Ignore the end bins for scaling so that clipped pixels do not flatten the rest.
            let peak = [&h.red, &h.green, &h.blue]
                .iter()
                .flat_map(|c| c.iter().skip(1).take(254))
                .copied()
                .max()
                .unwrap_or(1)
                .max(1) as f32;
            let (x0, y0) = (bounds.origin.x, bounds.origin.y);
            let (w, hgt) = (bounds.size.width, bounds.size.height);
            for (bins, color) in [
                (&h.red, rgba(0xff4040a0)),
                (&h.green, rgba(0x40ff40a0)),
                (&h.blue, rgba(0x4080ffa0)),
            ] {
                let n = bins.len().max(2);
                let mut b = PathBuilder::fill();
                b.move_to(point(x0, y0 + hgt));
                for (i, &c) in bins.iter().enumerate() {
                    let x = x0 + w * (i as f32 / (n - 1) as f32);
                    let y = y0 + hgt * (1.0 - (c as f32 / peak).min(1.0).sqrt());
                    b.line_to(point(x, y));
                }
                b.line_to(point(x0 + w, y0 + hgt));
                b.close();
                if let Ok(path) = b.build() {
                    window.paint_path(path, color);
                }
            }
        },
    )
    .w_full()
    .h(px(110.0))
}

impl Render for Panel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tool = self.photo.read(cx).tool;
        let top = v_flex()
            .size_full()
            .child(div().p_3().child(histogram_element(self.histogram.clone())));
        match tool {
            Tool::Crop => {
                return top
                    .child(
                        div()
                            .id("crop-scroll")
                            .flex_1()
                            .overflow_y_scroll()
                            .child(self.render_crop(cx)),
                    )
                    .into_any_element();
            }
            Tool::Mask => {
                return top
                    .child(
                        div()
                            .id("mask-scroll")
                            .flex_1()
                            .overflow_y_scroll()
                            .child(self.masks.clone()),
                    )
                    .into_any_element();
            }
            Tool::None => {}
        }
        let tabs = TabBar::new("develop-tabs")
            .underline()
            .small()
            .selected_index(self.tab)
            .children(TABS.iter().map(|t| t.title).chain([INFO_TAB]))
            .on_click(cx.listener(|p, ix: &usize, _, cx| {
                p.tab = *ix;
                p.error = None;
                cx.notify();
            }));
        let body = if self.tab < TABS.len() {
            self.render_tab(cx).into_any_element()
        } else {
            self.render_info(cx).into_any_element()
        };
        top.child(div().px_2().child(tabs))
            .child(
                div()
                    .id(("panel-scroll", self.tab))
                    .flex_1()
                    .overflow_y_scroll()
                    .child(body),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panel_paths_are_unique_scalar_params() {
        let mut seen = std::collections::HashSet::new();
        for s in TABS.iter().flat_map(|t| t.sections) {
            for (path, _) in s.rows {
                assert!(EditParams::default().spec(path).is_some(), "{path}");
                assert!(seen.insert(*path), "{path} appears twice");
            }
        }
        assert_eq!(decimals(0.01), 2);
        assert_eq!(decimals(1.0), 0);
        assert_eq!(decimals(0.1), 1);
    }
}
