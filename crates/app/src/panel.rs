//! Develop panel: histogram and the global adjustments, grouped into tabs.
//!
//! Each slider edits one parameter path through `doc`: `Continuous` while dragging, `Final`
//! on release, all commands of one drag sharing a gesture so they form one undo step.

use std::sync::Arc;

use aigle_core::EditParams;
use aigle_core::params::{Aspect, CameraProfile, Demosaic, HighlightRecon, LensProfile, WbMode};
use aigle_doc::{EditCommand, GestureId, ParamOp, value_at};
use aigle_engine_api::{Histogram, ImportKind, Interaction, SourceKind, watch};
use gpui::{
    App, Context, Entity, IntoElement, ParentElement, PathBuilder, PathPromptOptions, Render,
    SharedString, Styled, Subscription, Task, Window, canvas, div, point, prelude::*, px, rgb,
    rgba,
};
use gpui_component::button::{Button, ButtonVariants};
use gpui_component::slider::{Slider, SliderEvent, SliderScale, SliderState};
use gpui_component::tab::TabBar;
use gpui_component::{ActiveTheme, Selectable, Sizable, h_flex, v_flex};
use serde_json::Value;

use crate::curve::CurveEditor;
use crate::mask_panel::MaskPanel;
use crate::meta;
use crate::photo::{Photo, PhotoEvent, Tool};
use crate::retouch_panel::RetouchPanel;

const PANEL_TABS: [&str; 4] = ["调整", "修饰", "蒙版", "信息"];
const MASK_TAB: usize = 2;
const GROUPS: [&[usize]; 2] = [&[0, 1, 2, 3], &[4, 5, 6, 7]];

/// Buttons shown above a section's sliders.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Extra {
    None,
    WhiteBalance,
    LensProfile,
    PointCurve,
    CameraProfile,
    RawSettings,
    Treatment,
}

impl Extra {
    /// Non-slider paths the section's reset button also restores.
    fn reset_paths(self) -> &'static [&'static str] {
        match self {
            Extra::None | Extra::LensProfile => &[],
            Extra::WhiteBalance => &["wb.mode"],
            Extra::PointCurve => &["curve.point"],
            Extra::CameraProfile => &["profile.camera", "profile.look"],
            Extra::Treatment => &["bw.monochrome"],
            Extra::RawSettings => &[
                "raw.demosaic",
                "raw.highlight_recon",
                "raw.ca_correction",
                "raw.hot_pixels",
            ],
        }
    }
}

impl Section {
    /// Every path the section's reset button restores.
    pub fn paths(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.rows
            .iter()
            .map(|(p, _)| *p)
            .chain(self.extra.reset_paths().iter().copied())
    }

    /// The colour mixer is replaced by the black & white mix in monochrome.
    fn hidden(&self, p: &EditParams) -> bool {
        let first = self.rows.first().map(|(path, _)| *path).unwrap_or("");
        if p.bw.monochrome {
            first.starts_with("mixer.")
        } else {
            first.starts_with("bw.")
        }
    }
}

pub struct Section {
    pub title: &'static str,
    pub extra: Extra,
    /// (path, label).
    pub rows: &'static [(&'static str, &'static str)],
}

pub struct GroupDef {
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

pub const TOOL_GROUPS: &[GroupDef] = &[
    GroupDef {
        title: "基本",
        sections: &[
            Section {
                title: "配置文件",
                extra: Extra::CameraProfile,
                rows: &[],
            },
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
    GroupDef {
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
    GroupDef {
        title: "颜色",
        sections: &[
            Section {
                title: "处理方式",
                extra: Extra::Treatment,
                rows: &[],
            },
            section(
                "黑白混合",
                &[
                    ("bw.red", "红色"),
                    ("bw.orange", "橙色"),
                    ("bw.yellow", "黄色"),
                    ("bw.green", "绿色"),
                    ("bw.aqua", "浅绿色"),
                    ("bw.blue", "蓝色"),
                    ("bw.purple", "紫色"),
                    ("bw.magenta", "洋红"),
                ],
            ),
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
    GroupDef {
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
    GroupDef {
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
            section(
                "去边",
                &[
                    ("detail.defringe.purple_amount", "紫色数量"),
                    ("detail.defringe.purple_hue", "紫色色相"),
                    ("detail.defringe.green_amount", "绿色数量"),
                    ("detail.defringe.green_hue", "绿色色相"),
                ],
            ),
        ],
    },
    GroupDef {
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
    GroupDef {
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
    GroupDef {
        title: "RAW 处理",
        sections: &[Section {
            title: "解码",
            extra: Extra::RawSettings,
            rows: &[],
        }],
    },
];

const DEMOSAIC: [(Demosaic, &str); 3] = [
    (Demosaic::Auto, "自动"),
    (Demosaic::Rcd, "RCD"),
    (Demosaic::Amaze, "AMaZE"),
];

const HIGHLIGHTS: [(HighlightRecon, &str); 4] = [
    (HighlightRecon::ColorPropagation, "颜色传播"),
    (HighlightRecon::InpaintOpposed, "反向修补"),
    (HighlightRecon::Guided, "引导式"),
    (HighlightRecon::Off, "关闭"),
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
    expanded: [bool; TOOL_GROUPS.len()],
    histogram: Option<Arc<Histogram>>,
    curve: Entity<CurveEditor>,
    masks: Entity<MaskPanel>,
    retouch: Entity<RetouchPanel>,
    gesture: Option<GestureId>,
    job: Option<Task<()>>,
    import_job: Option<Task<()>>,
    lens_name: Option<String>,
    /// Names of the imported DCP and look, as reported on import.
    profile_name: Option<String>,
    look_name: Option<String>,
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
        for s in TOOL_GROUPS.iter().flat_map(|t| t.sections) {
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
            PhotoEvent::ToolChanged => {
                if p.photo.read(cx).tool == Tool::Mask {
                    p.tab = MASK_TAB;
                } else if p.tab == MASK_TAB {
                    p.tab = 0;
                }
                cx.notify();
            }
            PhotoEvent::SnapshotsChanged => {}
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
        let retouch = cx.new(|cx| RetouchPanel::new(photo.clone(), window, cx));
        let mut expanded = [false; TOOL_GROUPS.len()];
        expanded[0] = true;
        expanded[4] = true;
        let mut panel = Panel {
            photo,
            rows,
            tab: 0,
            expanded,
            histogram: None,
            curve,
            masks,
            retouch,
            gesture: None,
            job: None,
            import_job: None,
            lens_name: None,
            profile_name: None,
            look_name: None,
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
        label: impl AsRef<str>,
        ops: Vec<ParamOp>,
        gesture: Option<GestureId>,
        hint: Interaction,
        cx: &mut Context<Self>,
    ) {
        let r = self.photo.update(cx, |p, cx| {
            let mut cmd = EditCommand::user(p.doc.rev(), label.as_ref(), ops);
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
        let label = rust_i18n::t!(
            "重置 {}",
            arg0 = crate::i18n::history_label(&self.rows[i].history)
        )
        .into_owned();
        self.send(&label, ops, None, Interaction::Final, cx);
    }

    fn as_shot(&mut self, cx: &mut Context<Self>) {
        self.send(
            crate::i18n::t("白平衡 · 原照设置"),
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
                    p.send(
                        crate::i18n::t("白平衡 · 自动"),
                        ops,
                        None,
                        Interaction::Final,
                        cx,
                    );
                    if let Some(why) = wb.low_confidence_reason {
                        p.error = Some(
                            rust_i18n::t!("自动白平衡置信度低：{why}", why = why).into_owned(),
                        );
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
                                crate::i18n::t("镜头校正 · 启用配置文件"),
                                vec![ParamOp::set("lens.profile", value)],
                                None,
                                Interaction::Final,
                                cx,
                            );
                            p.lens_name = Some(format!("{} {}", m.lens_maker, m.lens_model));
                        }
                        None => p.error = Some(crate::i18n::t("镜头数据库中没有匹配的镜头")),
                    },
                    Err(e) => p.error = Some(e.to_string()),
                }
                cx.notify();
            });
        }));
    }

    fn lens_off(&mut self, cx: &mut Context<Self>) {
        self.send(
            crate::i18n::t("镜头校正 · 关闭"),
            resets(&["lens.profile"]),
            None,
            Interaction::Final,
            cx,
        );
    }

    fn set_json(&mut self, label: &str, path: &str, value: Value, cx: &mut Context<Self>) {
        self.send(
            label,
            vec![ParamOp::set(path, value)],
            None,
            Interaction::Final,
            cx,
        );
    }

    fn section_modified(&self, s: &Section, cx: &App) -> bool {
        let head = self.photo.read(cx).head().clone();
        let defaults = EditParams::default();
        s.paths()
            .any(|p| value_at(&head, p) != value_at(&defaults, p))
    }

    fn reset_section(&mut self, group: usize, section: usize, cx: &mut Context<Self>) {
        let s = &TOOL_GROUPS[group].sections[section];
        let paths: Vec<&str> = s.paths().collect();
        let label = rust_i18n::t!("重置 {}", arg0 = crate::i18n::t(s.title)).into_owned();
        self.send(&label, resets(&paths), None, Interaction::Final, cx);
    }

    /// Pick a DCP or `.cube` file, import it into the engine and select it.
    fn import(&mut self, kind: ImportKind, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(crate::i18n::t("导入").into()),
        });
        let engine = self.photo.read(cx).engine.clone();
        self.import_job = Some(cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = rx.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let read = cx
                .background_spawn(async move { std::fs::read(&path) })
                .await;
            let r = match read {
                Ok(bytes) => engine.import(kind, bytes).await.map_err(|e| e.to_string()),
                Err(e) => Err(e.to_string()),
            };
            let _ = this.update(cx, |p, cx| {
                let imported = match r {
                    Ok(i) => i,
                    Err(e) => {
                        p.error = Some(rust_i18n::t!("导入失败：{e}", e = e).into_owned());
                        cx.notify();
                        return;
                    }
                };
                match kind {
                    ImportKind::Dcp => {
                        let value = serde_json::to_value(CameraProfile::Dcp(imported.data_ref))
                            .expect("serialisable");
                        p.set_json(
                            &crate::i18n::t("配置文件 · 导入 DCP"),
                            "profile.camera",
                            value,
                            cx,
                        );
                        let model = p
                            .photo
                            .read(cx)
                            .info()
                            .and_then(|i| i.camera.as_ref())
                            .map(|c| c.model.clone());
                        if let (Some(made_for), Some(model)) = (&imported.camera_model, model)
                            && !made_for.to_lowercase().contains(&model.to_lowercase())
                        {
                            p.error = Some(
                                rust_i18n::t!("此 DCP 是为 {made_for} 制作的", made_for = made_for)
                                    .into_owned(),
                            );
                        }
                        p.profile_name = Some(imported.name);
                    }
                    ImportKind::LookLut => {
                        let value = serde_json::to_value(imported.data_ref).expect("serialisable");
                        p.set_json(
                            &crate::i18n::t("配置文件 · 导入 LUT"),
                            "profile.look",
                            value,
                            cx,
                        );
                        p.look_name = Some(imported.name);
                    }
                    ImportKind::DisplayProfile => {}
                }
                cx.notify();
            });
        }));
    }

    /// A row of buttons choosing one JSON value for `path`.
    fn choices(
        &self,
        id: &'static str,
        path: &'static str,
        history: String,
        options: Vec<(Value, String)>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let current = value_at(self.photo.read(cx).head(), path);
        h_flex()
            .gap_1()
            .flex_wrap()
            .children(options.into_iter().enumerate().map(|(i, (value, label))| {
                let history = history.clone();
                Button::new(SharedString::from(format!("{id}-{i}")))
                    .label(label)
                    .small()
                    .selected(value == current)
                    .on_click(
                        cx.listener(move |p, _, _, cx| {
                            p.set_json(&history, path, value.clone(), cx)
                        }),
                    )
            }))
    }

    fn render_raw_settings(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let muted = cx.theme().muted_foreground;
        let photo = self.photo.read(cx);
        if photo.info().is_some_and(|i| i.kind != SourceKind::Raw) {
            return div()
                .text_xs()
                .text_color(muted)
                .child(crate::i18n::t("仅适用于 RAW 文件"))
                .into_any_element();
        }
        let raw = photo.head().raw.clone();
        let history = |s: &str| format!("{} · {}", crate::i18n::t("解码"), crate::i18n::t(s));
        let heading = |s: &str| div().text_xs().child(crate::i18n::t(s));
        let note = |s: &str| div().text_xs().text_color(muted).child(crate::i18n::t(s));
        let demosaic = DEMOSAIC
            .iter()
            .map(|(d, l)| {
                (
                    serde_json::to_value(d).expect("serialisable"),
                    crate::i18n::t(l),
                )
            })
            .collect();
        let highlights = HIGHLIGHTS
            .iter()
            .map(|(h, l)| {
                (
                    serde_json::to_value(h).expect("serialisable"),
                    crate::i18n::t(l),
                )
            })
            .collect();
        let toggle =
            |id: &'static str, path: &'static str, label: &'static str, on: bool| {
                let history = history(label);
                Button::new(id)
                    .label(crate::i18n::t(label))
                    .small()
                    .selected(on)
                    .on_click(cx.listener(move |p, _, _, cx| {
                        p.set_json(&history, path, Value::Bool(!on), cx)
                    }))
            };
        let toggles = h_flex()
            .gap_1()
            .flex_wrap()
            .child(toggle(
                "raw-ca",
                "raw.ca_correction",
                "自动色差校正",
                raw.ca_correction,
            ))
            .child(toggle(
                "raw-hot-pixels",
                "raw.hot_pixels",
                "坏点去除",
                raw.hot_pixels,
            ));
        v_flex()
            .gap_2()
            .child(heading("去马赛克"))
            .child(self.choices(
                "raw-demosaic",
                "raw.demosaic",
                history("去马赛克"),
                demosaic,
                cx,
            ))
            .child(note(
                "RCD 与 AMaZE 仅用于拜耳传感器；X-Trans 始终使用 Markesteijn。",
            ))
            .child(heading("高光重建"))
            .child(self.choices(
                "raw-highlights",
                "raw.highlight_recon",
                history("高光重建"),
                highlights,
                cx,
            ))
            .child(toggles)
            .child(note("自动色差校正仅用于拜耳传感器。"))
            .into_any_element()
    }

    fn render_camera_profile(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let muted = cx.theme().muted_foreground;
        let photo = self.photo.read(cx);
        let is_raw = photo.info().is_some_and(|i| i.kind == SourceKind::Raw);
        let profile = photo.head().profile.clone();
        let matrix = matches!(profile.camera, CameraProfile::Matrix(_));
        let camera_status = if matrix {
            crate::i18n::t("标准颜色矩阵")
        } else {
            self.profile_name
                .clone()
                .unwrap_or_else(|| crate::i18n::t("已导入的 DCP"))
        };
        let look_status = match &profile.look {
            None => crate::i18n::t("无"),
            Some(_) => self
                .look_name
                .clone()
                .unwrap_or_else(|| crate::i18n::t("已导入的 LUT")),
        };
        let status = |s: String| div().text_xs().text_color(muted).child(s);
        let heading = |s: &str| div().text_xs().child(crate::i18n::t(s));
        v_flex()
            .gap_2()
            .when(is_raw, |d| {
                d.child(heading("相机"))
                    .child(
                        h_flex()
                            .gap_1()
                            .child(
                                Button::new("profile-matrix")
                                    .label(crate::i18n::t("标准"))
                                    .small()
                                    .selected(matrix)
                                    .on_click(cx.listener(|p, _, _, cx| {
                                        p.send(
                                            crate::i18n::t("配置文件 · 标准"),
                                            resets(&["profile.camera"]),
                                            None,
                                            Interaction::Final,
                                            cx,
                                        )
                                    })),
                            )
                            .child(
                                Button::new("profile-dcp")
                                    .label(crate::i18n::t("导入 DCP…"))
                                    .small()
                                    .selected(!matrix)
                                    .on_click(
                                        cx.listener(|p, _, _, cx| p.import(ImportKind::Dcp, cx)),
                                    ),
                            ),
                    )
                    .child(status(camera_status))
            })
            .child(heading("外观 LUT"))
            .child(
                h_flex()
                    .gap_1()
                    .child(
                        Button::new("profile-lut")
                            .label(crate::i18n::t("导入 LUT…"))
                            .small()
                            .on_click(cx.listener(|p, _, _, cx| p.import(ImportKind::LookLut, cx))),
                    )
                    .when(profile.look.is_some(), |d| {
                        d.child(
                            Button::new("profile-lut-off")
                                .label(crate::i18n::t("移除"))
                                .small()
                                .on_click(cx.listener(|p, _, _, cx| {
                                    p.send(
                                        crate::i18n::t("配置文件 · 移除 LUT"),
                                        resets(&["profile.look"]),
                                        None,
                                        Interaction::Final,
                                        cx,
                                    )
                                })),
                        )
                    }),
            )
            .child(status(look_status))
            .into_any_element()
    }

    fn render_extra(&self, extra: Extra, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        match extra {
            Extra::None => None,
            Extra::RawSettings => Some(self.render_raw_settings(cx)),
            Extra::CameraProfile => Some(self.render_camera_profile(cx)),
            Extra::Treatment => {
                let mono = self.photo.read(cx).head().bw.monochrome;
                let button = |id: &'static str, label: &'static str, on: bool| {
                    Button::new(id)
                        .label(crate::i18n::t(label))
                        .small()
                        .selected(mono == on)
                        .on_click(cx.listener(move |p, _, _, cx| {
                            if mono != on {
                                p.photo.update(cx, |ph, cx| ph.toggle_monochrome(cx))
                            }
                        }))
                };
                Some(
                    h_flex()
                        .gap_1()
                        .child(button("treatment-color", "彩色", false))
                        .child(
                            button("treatment-bw", "黑白", true)
                                .tooltip(crate::keymap::hint("转换为黑白（V）", cx)),
                        )
                        .into_any_element(),
                )
            }
            Extra::PointCurve => Some(self.curve.clone().into_any_element()),
            Extra::WhiteBalance => {
                let picking = self.photo.read(cx).wb_pick;
                Some(
                    h_flex()
                        .gap_2()
                        .child(
                            Button::new("wb-pick")
                                .label(crate::i18n::t("吸管"))
                                .small()
                                .selected(picking)
                                .tooltip(crate::keymap::hint("点击画面中应为中性灰的位置（W）", cx))
                                .on_click(cx.listener(move |p, _, _, cx| {
                                    p.photo.update(cx, |ph, cx| ph.set_wb_pick(!picking, cx))
                                })),
                        )
                        .child(
                            Button::new("wb-as-shot")
                                .label(crate::i18n::t("原照设置"))
                                .small()
                                .on_click(cx.listener(|p, _, _, cx| p.as_shot(cx))),
                        )
                        .child(
                            Button::new("wb-auto")
                                .label(crate::i18n::t("自动"))
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
                    (false, _) => crate::i18n::t("未启用配置文件").to_string(),
                    (true, Some(n)) => n.clone(),
                    (true, None) => crate::i18n::t("已启用配置文件").to_string(),
                };
                Some(
                    v_flex()
                        .gap_1()
                        .child(
                            h_flex()
                                .gap_2()
                                .child(
                                    Button::new("lens-match")
                                        .label(crate::i18n::t("自动匹配"))
                                        .small()
                                        .on_click(cx.listener(|p, _, _, cx| p.match_lens(cx))),
                                )
                                .child(
                                    Button::new("lens-off")
                                        .label(crate::i18n::t("关闭"))
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
                    .child(crate::i18n::t(r.label))
                    .child(div().text_color(cx.theme().muted_foreground).child(value)),
            )
            .child(
                Slider::new(&r.state).accessibility_label(crate::i18n::history_label(&r.history)),
            )
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
        let button = |id: &'static str, label: String, f: fn(&mut Photo, &mut Context<Photo>)| {
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
        let title = |s: String| {
            div()
                .text_sm()
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .child(s)
        };
        v_flex()
            .gap_4()
            .p_3()
            .child(title(crate::i18n::t("裁剪与拉直")))
            .child(
                v_flex()
                    .gap_2()
                    .child(title(crate::i18n::t("长宽比")))
                    .child(
                        h_flex()
                            .gap_1()
                            .flex_wrap()
                            .children(presets.into_iter().map(|(label, a)| {
                                let photo = photo.clone();
                                Button::new(SharedString::from(format!("aspect-{label}")))
                                    .label(crate::i18n::t(label))
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
                                        .label(crate::i18n::t("横竖互换"))
                                        .small()
                                        .tooltip(crate::keymap::hint("交换长宽比的方向（X）", cx))
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
                    .child(title(crate::i18n::t("拉直")))
                    .child(self.render_row(angle, cx))
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(crate::i18n::t(
                                "在裁剪框外拖动也可以旋转；裁剪框会自动缩小以避免空白角。",
                            )),
                    ),
            )
            .child(
                v_flex().gap_2().child(title(crate::i18n::t("方向"))).child(
                    h_flex()
                        .gap_1()
                        .flex_wrap()
                        .child(button(
                            "rot-ccw",
                            crate::i18n::t("向左旋转"),
                            |p, cx| p.rotate_quarter(false, cx),
                        ))
                        .child(button("rot-cw", crate::i18n::t("向右旋转"), |p, cx| {
                            p.rotate_quarter(true, cx)
                        }))
                        .child(button("mirror", crate::i18n::t("水平翻转"), Photo::mirror)),
                ),
            )
            .child(
                h_flex()
                    .gap_1()
                    .child(button(
                        "crop-reset",
                        crate::i18n::t("重置"),
                        Photo::reset_crop,
                    ))
                    .child(button("crop-done", crate::i18n::t("完成"), |p, cx| {
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
                    .child(crate::i18n::t("照片尚未加载")),
            );
        };
        let muted = cx.theme().muted_foreground;
        for s in meta::sections(info, &photo.file) {
            let mut sec = v_flex().gap_1().child(
                div()
                    .text_sm()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .mb_1()
                    .child(crate::i18n::t(&s.title)),
            );
            for (k, v) in s.rows {
                sec = sec.child(
                    h_flex()
                        .gap_3()
                        .items_start()
                        .text_xs()
                        .child(
                            div()
                                .w(px(76.0))
                                .flex_none()
                                .text_color(muted)
                                .child(crate::i18n::t(&k)),
                        )
                        .child(div().flex_1().min_w_0().child(v)),
                );
            }
            body = body.child(sec);
        }
        body
    }

    fn render_tab(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut body = v_flex().gap_4().p_3();
        for &group in GROUPS[self.tab] {
            let open = self.expanded[group];
            let mut content = v_flex().gap_4();
            let mut i: usize = TOOL_GROUPS[..group]
                .iter()
                .flat_map(|t| t.sections)
                .map(|s| s.rows.len())
                .sum();
            for (j, s) in TOOL_GROUPS[group].sections.iter().enumerate() {
                if s.hidden(self.photo.read(cx).head()) {
                    i += s.rows.len();
                    continue;
                }
                let modified = self.section_modified(s, cx);
                let mut sec = v_flex().gap_2().child(
                    h_flex()
                        .justify_between()
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(crate::i18n::t(s.title)),
                        )
                        .when(modified, |d| {
                            d.child(
                                Button::new(SharedString::from(format!("reset-{group}-{j}")))
                                    .label(crate::i18n::t("重置"))
                                    .xsmall()
                                    .ghost()
                                    .on_click(cx.listener(move |p, _, _, cx| {
                                        p.reset_section(group, j, cx)
                                    })),
                            )
                        }),
                );
                if let Some(e) = self.render_extra(s.extra, cx) {
                    sec = sec.child(e);
                }
                for _ in s.rows {
                    sec = sec.child(self.render_row(i, cx));
                    i += 1;
                }
                content = content.child(sec);
            }
            body = body.child(
                v_flex()
                    .gap_3()
                    .pb_3()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        Button::new(("adjustment-group", group))
                            .label(format!(
                                "{}  {}",
                                if open { "▾" } else { "▸" },
                                crate::i18n::t(TOOL_GROUPS[group].title)
                            ))
                            .small()
                            .w_full()
                            .on_click(cx.listener(move |p, _, _, cx| {
                                p.expanded[group] = !p.expanded[group];
                                cx.notify();
                            })),
                    )
                    .when(open, |d| d.child(content)),
            );
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
        let top = v_flex().size_full().min_h_0().child(
            div()
                .flex_none()
                .p_3()
                .child(histogram_element(self.histogram.clone())),
        );
        let tabs = TabBar::new("develop-tabs")
            .underline()
            .small()
            .selected_index(self.tab)
            .children(PANEL_TABS.map(crate::i18n::t))
            .on_click(cx.listener(|p, ix: &usize, _, cx| {
                p.tab = *ix;
                p.error = None;
                p.photo.update(cx, |ph, cx| {
                    ph.set_tool(
                        if *ix == MASK_TAB {
                            Tool::Mask
                        } else {
                            Tool::None
                        },
                        cx,
                    );
                });
                cx.notify();
            }));
        let body = if tool == Tool::Crop {
            self.render_crop(cx).into_any_element()
        } else if matches!(tool, Tool::Spot | Tool::RedEye) {
            self.retouch.clone().into_any_element()
        } else {
            match self.tab {
                0 | 1 => self.render_tab(cx).into_any_element(),
                MASK_TAB => self.masks.clone().into_any_element(),
                _ => self.render_info(cx).into_any_element(),
            }
        };
        top.child(div().flex_none().px_2().child(tabs))
            .child(
                div()
                    .id(("panel-scroll", self.tab))
                    .min_h_0()
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
        for s in TOOL_GROUPS.iter().flat_map(|t| t.sections) {
            for (path, _) in s.rows {
                assert!(EditParams::default().spec(path).is_some(), "{path}");
                assert!(seen.insert(*path), "{path} appears twice");
            }
        }
        assert_eq!(decimals(0.01), 2);
        assert_eq!(decimals(1.0), 0);
        assert_eq!(decimals(0.1), 1);
    }

    #[test]
    fn section_resets_restore_defaults() {
        use aigle_doc::PhotoDoc;
        let mut doc = PhotoDoc::new(EditParams::default()).unwrap();
        let ops = vec![
            ParamOp::set("raw.demosaic", "amaze"),
            ParamOp::set("raw.highlight_recon", "off"),
            ParamOp::set("raw.ca_correction", false),
            ParamOp::set("profile.look", "abc"),
            ParamOp::set("wb.mode", "custom"),
            ParamOp::set("light.exposure", 1.0),
        ];
        doc.apply(EditCommand::user(doc.rev(), "x", ops), Interaction::Final)
            .unwrap();
        assert_eq!(doc.head().raw.demosaic, Demosaic::Amaze);
        let paths: Vec<&str> = TOOL_GROUPS
            .iter()
            .flat_map(|t| t.sections)
            .flat_map(|s| s.paths())
            .collect();
        doc.apply(
            EditCommand::user(doc.rev(), "reset", resets(&paths)),
            Interaction::Final,
        )
        .unwrap();
        assert_eq!(**doc.head(), EditParams::default());
    }
}
