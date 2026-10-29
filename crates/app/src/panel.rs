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

const PANEL_TABS: [&str; 4] = [
    "panel.tab.adjust",
    "panel.tab.retouch",
    "panel.tab.masks",
    "panel.tab.info",
];
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
            .copied()
            .chain(self.extra.reset_paths().iter().copied())
    }

    /// The colour mixer is replaced by the black & white mix in monochrome.
    fn hidden(&self, p: &EditParams) -> bool {
        let first = self.rows.first().copied().unwrap_or("");
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
    /// Parameter paths of the sliders; see [`label_key`].
    pub rows: &'static [&'static str],
}

pub struct GroupDef {
    pub title: &'static str,
    pub sections: &'static [Section],
}

/// Catalog key of the label of the slider for `path`.
pub fn label_key(path: &str) -> String {
    format!("param.{path}")
}

const fn section(title: &'static str, rows: &'static [&'static str]) -> Section {
    Section {
        title,
        extra: Extra::None,
        rows,
    }
}

pub const TOOL_GROUPS: &[GroupDef] = &[
    GroupDef {
        title: "group.basic",
        sections: &[
            Section {
                title: "section.profile",
                extra: Extra::CameraProfile,
                rows: &[],
            },
            Section {
                title: "section.white_balance",
                extra: Extra::WhiteBalance,
                rows: &["wb.temperature", "wb.tint"],
            },
            section(
                "section.tone",
                &[
                    "light.exposure",
                    "light.contrast",
                    "light.highlights",
                    "light.shadows",
                    "light.whites",
                    "light.blacks",
                ],
            ),
            section(
                "section.presence",
                &[
                    "presence.texture",
                    "presence.clarity",
                    "presence.dehaze",
                    "presence.vibrance",
                    "presence.saturation",
                ],
            ),
        ],
    },
    GroupDef {
        title: "group.curves",
        sections: &[
            Section {
                title: "section.point_curve",
                extra: Extra::PointCurve,
                rows: &[],
            },
            section(
                "section.parametric_curve",
                &[
                    "curve.parametric.highlights",
                    "curve.parametric.lights",
                    "curve.parametric.darks",
                    "curve.parametric.shadows",
                ],
            ),
        ],
    },
    GroupDef {
        title: "group.color",
        sections: &[
            Section {
                title: "section.treatment",
                extra: Extra::Treatment,
                rows: &[],
            },
            section(
                "section.bw_mix",
                &[
                    "bw.red",
                    "bw.orange",
                    "bw.yellow",
                    "bw.green",
                    "bw.aqua",
                    "bw.blue",
                    "bw.purple",
                    "bw.magenta",
                ],
            ),
            section(
                "section.mixer_hue",
                &[
                    "mixer.red.hue",
                    "mixer.orange.hue",
                    "mixer.yellow.hue",
                    "mixer.green.hue",
                    "mixer.aqua.hue",
                    "mixer.blue.hue",
                    "mixer.purple.hue",
                    "mixer.magenta.hue",
                ],
            ),
            section(
                "section.mixer_saturation",
                &[
                    "mixer.red.saturation",
                    "mixer.orange.saturation",
                    "mixer.yellow.saturation",
                    "mixer.green.saturation",
                    "mixer.aqua.saturation",
                    "mixer.blue.saturation",
                    "mixer.purple.saturation",
                    "mixer.magenta.saturation",
                ],
            ),
            section(
                "section.mixer_luminance",
                &[
                    "mixer.red.luminance",
                    "mixer.orange.luminance",
                    "mixer.yellow.luminance",
                    "mixer.green.luminance",
                    "mixer.aqua.luminance",
                    "mixer.blue.luminance",
                    "mixer.purple.luminance",
                    "mixer.magenta.luminance",
                ],
            ),
        ],
    },
    GroupDef {
        title: "group.grading",
        sections: &[
            section(
                "section.grading_shadows",
                &[
                    "grading.shadows.hue",
                    "grading.shadows.saturation",
                    "grading.shadows.luminance",
                ],
            ),
            section(
                "section.grading_midtones",
                &[
                    "grading.midtones.hue",
                    "grading.midtones.saturation",
                    "grading.midtones.luminance",
                ],
            ),
            section(
                "section.grading_highlights",
                &[
                    "grading.highlights.hue",
                    "grading.highlights.saturation",
                    "grading.highlights.luminance",
                ],
            ),
            section(
                "section.grading_global",
                &[
                    "grading.global.hue",
                    "grading.global.saturation",
                    "grading.global.luminance",
                ],
            ),
            section(
                "section.grading_blending",
                &["grading.blending", "grading.balance"],
            ),
        ],
    },
    GroupDef {
        title: "group.detail",
        sections: &[
            section(
                "section.sharpening",
                &[
                    "detail.sharpen.amount",
                    "detail.sharpen.radius",
                    "detail.sharpen.detail",
                    "detail.sharpen.masking",
                ],
            ),
            section(
                "section.noise_reduction",
                &[
                    "detail.noise.luma",
                    "detail.noise.detail",
                    "detail.noise.contrast",
                    "detail.noise.chroma",
                    "detail.noise.chroma_detail",
                ],
            ),
            section(
                "section.defringe",
                &[
                    "detail.defringe.purple_amount",
                    "detail.defringe.purple_hue",
                    "detail.defringe.green_amount",
                    "detail.defringe.green_hue",
                ],
            ),
        ],
    },
    GroupDef {
        title: "group.optics",
        sections: &[
            Section {
                title: "section.lens_correction",
                extra: Extra::LensProfile,
                rows: &["lens.distortion", "lens.tca", "lens.vignetting"],
            },
            section(
                "section.transform",
                &[
                    "geometry.angle",
                    "geometry.perspective_vertical",
                    "geometry.perspective_horizontal",
                ],
            ),
        ],
    },
    GroupDef {
        title: "group.effects",
        sections: &[
            section(
                "section.vignette",
                &[
                    "effects.vignette.amount",
                    "effects.vignette.midpoint",
                    "effects.vignette.roundness",
                    "effects.vignette.feather",
                ],
            ),
            section(
                "section.grain",
                &[
                    "effects.grain.amount",
                    "effects.grain.size",
                    "effects.grain.roughness",
                ],
            ),
        ],
    },
    GroupDef {
        title: "group.raw",
        sections: &[Section {
            title: "section.decoding",
            extra: Extra::RawSettings,
            rows: &[],
        }],
    },
];

const DEMOSAIC: [(Demosaic, &str); 3] = [
    (Demosaic::Auto, "panel.raw.demosaic.auto"),
    (Demosaic::Rcd, "RCD"),
    (Demosaic::Amaze, "AMaZE"),
];

const HIGHLIGHTS: [(HighlightRecon, &str); 4] = [
    (
        HighlightRecon::ColorPropagation,
        "panel.raw.highlight_recon.color_propagation",
    ),
    (
        HighlightRecon::InpaintOpposed,
        "panel.raw.highlight_recon.inpaint_opposed",
    ),
    (HighlightRecon::Guided, "panel.raw.highlight_recon.guided"),
    (HighlightRecon::Off, "panel.raw.highlight_recon.off"),
];

struct Row {
    path: &'static str,
    /// Catalog key of the label.
    label: String,
    /// History label, e.g. `section.mixer_saturation · param.mixer.blue.saturation`.
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
            for &path in s.rows {
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
                let label = label_key(path);
                rows.push(Row {
                    path,
                    history: crate::i18n::join(&[s.title, &label]),
                    label,
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
        let label = crate::i18n::reset_label(&self.rows[i].history);
        self.send(&label, ops, None, Interaction::Final, cx);
    }

    fn as_shot(&mut self, cx: &mut Context<Self>) {
        self.send(
            crate::i18n::join(&["section.white_balance", "panel.wb.as_shot"]),
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
                        crate::i18n::join(&["section.white_balance", "panel.wb.auto"]),
                        ops,
                        None,
                        Interaction::Final,
                        cx,
                    );
                    if let Some(why) = wb.low_confidence_reason {
                        p.error =
                            Some(rust_i18n::t!("panel.wb.low_confidence", why = why).into_owned());
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
                                crate::i18n::join(&[
                                    "section.lens_correction",
                                    "panel.lens.enable_profile",
                                ]),
                                vec![ParamOp::set("lens.profile", value)],
                                None,
                                Interaction::Final,
                                cx,
                            );
                            p.lens_name = Some(format!("{} {}", m.lens_maker, m.lens_model));
                        }
                        None => p.error = Some(crate::i18n::t("panel.lens.no_match")),
                    },
                    Err(e) => p.error = Some(e.to_string()),
                }
                cx.notify();
            });
        }));
    }

    fn lens_off(&mut self, cx: &mut Context<Self>) {
        self.send(
            crate::i18n::join(&["section.lens_correction", "panel.lens.disable"]),
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
        let label = crate::i18n::reset_label(s.title);
        self.send(&label, resets(&paths), None, Interaction::Final, cx);
    }

    /// Pick a DCP or `.cube` file, import it into the engine and select it.
    fn import(&mut self, kind: ImportKind, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(crate::i18n::t("panel.profile.import_prompt").into()),
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
                        p.error =
                            Some(rust_i18n::t!("panel.profile.import_failed", e = e).into_owned());
                        cx.notify();
                        return;
                    }
                };
                match kind {
                    ImportKind::Dcp => {
                        let value = serde_json::to_value(CameraProfile::Dcp(imported.data_ref))
                            .expect("serialisable");
                        p.set_json(
                            &crate::i18n::join(&["section.profile", "panel.profile.import_dcp"]),
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
                                rust_i18n::t!("panel.profile.dcp_made_for", made_for = made_for)
                                    .into_owned(),
                            );
                        }
                        p.profile_name = Some(imported.name);
                    }
                    ImportKind::LookLut => {
                        let value = serde_json::to_value(imported.data_ref).expect("serialisable");
                        p.set_json(
                            &crate::i18n::join(&["section.profile", "panel.profile.import_lut"]),
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
                .child(crate::i18n::t("panel.raw.raw_only"))
                .into_any_element();
        }
        let raw = photo.head().raw.clone();
        let history = |s: &str| crate::i18n::join(&["section.decoding", s]);
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
                "panel.raw.ca_correction",
                raw.ca_correction,
            ))
            .child(toggle(
                "raw-hot-pixels",
                "raw.hot_pixels",
                "panel.raw.hot_pixels",
                raw.hot_pixels,
            ));
        v_flex()
            .gap_2()
            .child(heading("panel.raw.demosaic"))
            .child(self.choices(
                "raw-demosaic",
                "raw.demosaic",
                history("panel.raw.demosaic"),
                demosaic,
                cx,
            ))
            .child(note("panel.raw.demosaic_note"))
            .child(heading("panel.raw.highlight_recon"))
            .child(self.choices(
                "raw-highlights",
                "raw.highlight_recon",
                history("panel.raw.highlight_recon"),
                highlights,
                cx,
            ))
            .child(toggles)
            .child(note("panel.raw.ca_note"))
            .into_any_element()
    }

    fn render_camera_profile(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let muted = cx.theme().muted_foreground;
        let photo = self.photo.read(cx);
        let is_raw = photo.info().is_some_and(|i| i.kind == SourceKind::Raw);
        let profile = photo.head().profile.clone();
        let matrix = matches!(profile.camera, CameraProfile::Matrix(_));
        let camera_status = if matrix {
            crate::i18n::t("panel.profile.matrix_status")
        } else {
            self.profile_name
                .clone()
                .unwrap_or_else(|| crate::i18n::t("panel.profile.imported_dcp"))
        };
        let look_status = match &profile.look {
            None => crate::i18n::t("panel.profile.no_look"),
            Some(_) => self
                .look_name
                .clone()
                .unwrap_or_else(|| crate::i18n::t("panel.profile.imported_lut")),
        };
        let status = |s: String| div().text_xs().text_color(muted).child(s);
        let heading = |s: &str| div().text_xs().child(crate::i18n::t(s));
        v_flex()
            .gap_2()
            .when(is_raw, |d| {
                d.child(heading("panel.profile.camera"))
                    .child(
                        h_flex()
                            .gap_1()
                            .child(
                                Button::new("profile-matrix")
                                    .label(crate::i18n::t("panel.profile.standard"))
                                    .small()
                                    .selected(matrix)
                                    .on_click(cx.listener(|p, _, _, cx| {
                                        p.send(
                                            crate::i18n::join(&[
                                                "section.profile",
                                                "panel.profile.standard",
                                            ]),
                                            resets(&["profile.camera"]),
                                            None,
                                            Interaction::Final,
                                            cx,
                                        )
                                    })),
                            )
                            .child(
                                Button::new("profile-dcp")
                                    .label(crate::i18n::t("panel.profile.choose_dcp"))
                                    .small()
                                    .selected(!matrix)
                                    .on_click(
                                        cx.listener(|p, _, _, cx| p.import(ImportKind::Dcp, cx)),
                                    ),
                            ),
                    )
                    .child(status(camera_status))
            })
            .child(heading("panel.profile.look"))
            .child(
                h_flex()
                    .gap_1()
                    .child(
                        Button::new("profile-lut")
                            .label(crate::i18n::t("panel.profile.choose_lut"))
                            .small()
                            .on_click(cx.listener(|p, _, _, cx| p.import(ImportKind::LookLut, cx))),
                    )
                    .when(profile.look.is_some(), |d| {
                        d.child(
                            Button::new("profile-lut-off")
                                .label(crate::i18n::t("panel.profile.remove"))
                                .small()
                                .on_click(cx.listener(|p, _, _, cx| {
                                    p.send(
                                        crate::i18n::join(&[
                                            "section.profile",
                                            "panel.profile.remove_lut",
                                        ]),
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
                        .child(button("treatment-color", "panel.treatment.color", false))
                        .child(
                            button("treatment-bw", "panel.treatment.bw", true)
                                .tooltip(crate::keymap::hint("panel.treatment.bw_tooltip", cx)),
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
                                .label(crate::i18n::t("panel.wb.picker"))
                                .small()
                                .selected(picking)
                                .tooltip(crate::keymap::hint("panel.wb.picker_tooltip", cx))
                                .on_click(cx.listener(move |p, _, _, cx| {
                                    p.photo.update(cx, |ph, cx| ph.set_wb_pick(!picking, cx))
                                })),
                        )
                        .child(
                            Button::new("wb-as-shot")
                                .label(crate::i18n::t("panel.wb.as_shot"))
                                .small()
                                .on_click(cx.listener(|p, _, _, cx| p.as_shot(cx))),
                        )
                        .child(
                            Button::new("wb-auto")
                                .label(crate::i18n::t("panel.wb.auto"))
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
                    (false, _) => crate::i18n::t("panel.lens.profile_off"),
                    (true, Some(n)) => n.clone(),
                    (true, None) => crate::i18n::t("panel.lens.profile_on"),
                };
                Some(
                    v_flex()
                        .gap_1()
                        .child(
                            h_flex()
                                .gap_2()
                                .child(
                                    Button::new("lens-match")
                                        .label(crate::i18n::t("panel.lens.auto_match"))
                                        .small()
                                        .on_click(cx.listener(|p, _, _, cx| p.match_lens(cx))),
                                )
                                .child(
                                    Button::new("lens-off")
                                        .label(crate::i18n::t("panel.lens.disable"))
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
                    .child(crate::i18n::t(&r.label))
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
            ("crop.aspect.free", Aspect::Free),
            ("crop.aspect.original", Aspect::Original),
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
            .child(title(crate::i18n::t("action.ToggleCrop")))
            .child(
                v_flex()
                    .gap_2()
                    .child(title(crate::i18n::t("crop.aspect_ratio")))
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
                                        .label(crate::i18n::t("crop.swap"))
                                        .small()
                                        .tooltip(crate::keymap::hint("crop.swap_tooltip", cx))
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
                    .child(title(crate::i18n::t("crop.straighten")))
                    .child(self.render_row(angle, cx))
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(crate::i18n::t("crop.rotate_note")),
                    ),
            )
            .child(
                v_flex()
                    .gap_2()
                    .child(title(crate::i18n::t("crop.orientation")))
                    .child(
                        h_flex()
                            .gap_1()
                            .flex_wrap()
                            .child(button(
                                "rot-ccw",
                                crate::i18n::t("crop.rotate_left"),
                                |p, cx| p.rotate_quarter(false, cx),
                            ))
                            .child(button(
                                "rot-cw",
                                crate::i18n::t("crop.rotate_right"),
                                |p, cx| p.rotate_quarter(true, cx),
                            ))
                            .child(button("mirror", crate::i18n::t("crop.flip"), Photo::mirror)),
                    ),
            )
            .child(
                h_flex()
                    .gap_1()
                    .child(button(
                        "crop-reset",
                        crate::i18n::t("common.reset"),
                        Photo::reset_crop,
                    ))
                    .child(button(
                        "crop-done",
                        crate::i18n::t("common.done"),
                        |p, cx| p.set_tool(Tool::None, cx),
                    )),
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
                    .child(crate::i18n::t("panel.info.not_loaded")),
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
                                    .label(crate::i18n::t("common.reset"))
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
            for path in s.rows {
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
