//! Session-only sequential exports. Pause takes effect at file boundaries.
use crate::export::{self, Request, Settings};
use aigle_engine_api::Engine;
use gpui::{
    Context, EventEmitter, IntoElement, ParentElement, Render, Styled, Task, Window, div,
    prelude::*, px,
};
use gpui_component::{ActiveTheme, Disableable, Sizable, h_flex, v_flex};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Pending,
    Running,
    Done,
    Failed,
    Cancelled,
}
impl State {
    fn label(self) -> String {
        crate::i18n::t(match self {
            Self::Pending => "queue.pending",
            Self::Running => "queue.running",
            Self::Done => "queue.done",
            Self::Failed => "queue.failed",
            Self::Cancelled => "queue.cancelled",
        })
    }
}
#[derive(Clone)]
struct Entry {
    request: Request,
    settings: Settings,
    path: PathBuf,
    state: State,
    error: Option<String>,
}
pub enum Event {
    Closed,
    Notice(String),
}
pub struct ExportQueue {
    engine: Engine,
    entries: Vec<Entry>,
    paused: bool,
    task: Option<Task<()>>,
}
impl EventEmitter<Event> for ExportQueue {}
impl ExportQueue {
    pub fn new(engine: Engine) -> Self {
        Self {
            engine,
            entries: Vec::new(),
            paused: true,
            task: None,
        }
    }
    pub fn counts(&self) -> (usize, usize) {
        (
            self.entries
                .iter()
                .filter(|e| e.state == State::Done)
                .count(),
            self.entries
                .iter()
                .filter(|e| e.state != State::Cancelled)
                .count(),
        )
    }
    pub fn enqueue(
        &mut self,
        jobs: Vec<(Request, PathBuf)>,
        settings: Settings,
        cx: &mut Context<Self>,
    ) {
        let mut reserved: std::collections::HashSet<_> =
            self.entries.iter().map(|e| e.path.clone()).collect();
        reserved.extend(jobs.iter().map(|(_, path)| path.clone()));
        for (request, path) in jobs {
            if settings.paired_sdr && settings.format.encoded(settings.quality).is_hdr() {
                let mut sdr = settings.clone();
                sdr.paired_sdr = false;
                sdr.format = export::Format::Jpeg;
                sdr.output = aigle_engine_api::DisplayProfileId::Srgb;
                let sdr_path = paired_path(&path, &reserved);
                reserved.insert(sdr_path.clone());
                self.entries.push(Entry {
                    request: request.clone(),
                    settings: sdr,
                    path: sdr_path,
                    state: State::Pending,
                    error: None,
                });
            }
            self.entries.push(Entry {
                request,
                settings: settings.clone(),
                path,
                state: State::Pending,
                error: None,
            });
        }
        self.paused = false;
        self.start_next(cx);
        cx.notify();
    }
    fn start_next(&mut self, cx: &mut Context<Self>) {
        if self.paused || self.task.is_some() {
            return;
        }
        let Some(index) = self.entries.iter().position(|e| e.state == State::Pending) else {
            return;
        };
        self.entries[index].state = State::Running;
        let entry = self.entries[index].clone();
        let engine = self.engine.clone();
        self.task = Some(cx.spawn(async move |this, cx| {
            // Single-file destinations were confirmed by the save picker. The engine
            // replaces them atomically; batch and paired outputs already use unique names.
            let result: Result<(), String> =
                match export::run(&engine, &entry.request, &entry.settings, entry.path.clone())
                    .await
                {
                    Ok(_) if entry.settings.format.is_raw() => {
                        cx.background_spawn(async move {
                            export::write_dng_sidecar(&entry.request, &entry.path)
                        })
                        .await
                    }
                    Ok(_) => Ok(()),
                    Err(e) => Err(e.to_string()),
                };
            let _ = this.update(cx, |q, cx| {
                let entry = &mut q.entries[index];
                entry.state = if result.is_ok() {
                    State::Done
                } else {
                    State::Failed
                };
                entry.error = result.err();
                q.task = None;
                q.start_next(cx);
                if q.task.is_none() {
                    let (done, total) = q.counts();
                    cx.emit(Event::Notice(format!(
                        "{}: {done}/{total}",
                        crate::i18n::t("queue.title")
                    )));
                }
                cx.notify();
            });
        }));
        cx.notify();
    }
}
fn paired_path(path: &std::path::Path, reserved: &std::collections::HashSet<PathBuf>) -> PathBuf {
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    (1..)
        .map(|n| {
            path.with_file_name(if n == 1 {
                format!("{stem}-sdr.jpg")
            } else {
                format!("{stem}-sdr-{n}.jpg")
            })
        })
        .find(|p| !reserved.contains(p) && !p.exists())
        .expect("unbounded file names")
}

impl Render for ExportQueue {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut card = v_flex()
            .id("export-queue-card")
            .w(px(720.0))
            .max_h(px(650.0))
            .p_4()
            .gap_3()
            .rounded_lg()
            .bg(cx.theme().popover)
            .border_1()
            .border_color(cx.theme().border)
            .shadow_lg()
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                h_flex()
                    .gap_2()
                    .child(div().flex_1().child(crate::i18n::t("queue.title")))
                    .child(
                        crate::ui::labeled_button(
                            "queue-pause",
                            crate::ui::Glyph::Disable,
                            crate::i18n::t(if self.paused {
                                "queue.resume"
                            } else {
                                "queue.pause"
                            }),
                        )
                        .small()
                        .tooltip(crate::i18n::t("queue.pause_hint"))
                        .on_click(cx.listener(|q, _, _, cx| {
                            q.paused = !q.paused;
                            if !q.paused {
                                q.start_next(cx);
                            }
                            cx.notify();
                        })),
                    )
                    .child(
                        crate::ui::icon_button(
                            "queue-retry",
                            crate::ui::Glyph::Reset,
                            crate::i18n::t("queue.retry"),
                        )
                        .small()
                        .on_click(cx.listener(|q, _, _, cx| {
                            for e in &mut q.entries {
                                if e.state == State::Failed {
                                    e.state = State::Pending;
                                    e.error = None;
                                }
                            }
                            q.start_next(cx);
                            cx.notify();
                        })),
                    )
                    .child(
                        crate::ui::icon_button(
                            "queue-clear",
                            crate::ui::Glyph::Clear,
                            crate::i18n::t("queue.clear"),
                        )
                        .small()
                        .disabled(self.task.is_some())
                        .on_click(cx.listener(|q, _, _, cx| {
                            // Keep stable indices while a task is running.
                            if q.task.is_none() {
                                q.entries
                                    .retain(|e| !matches!(e.state, State::Done | State::Cancelled));
                                cx.notify();
                            }
                        })),
                    )
                    .child(
                        crate::ui::icon_button(
                            "queue-close",
                            crate::ui::Glyph::Close,
                            crate::i18n::t("common.done"),
                        )
                        .small()
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(Event::Closed))),
                    ),
            );
        let rows = v_flex()
            .id("queue-files")
            .overflow_y_scroll()
            .gap_2()
            .children(
                self.entries
                    .iter()
                    .enumerate()
                    .filter(|(_, e)| e.state != State::Cancelled)
                    .map(|(i, e)| {
                        v_flex()
                            .gap_1()
                            .border_b_1()
                            .border_color(cx.theme().border)
                            .pb_2()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .child(div().flex_1().text_sm().child(e.request.name.clone()))
                                    .child(div().text_xs().child(e.state.label()))
                                    .when(
                                        matches!(e.state, State::Pending | State::Failed),
                                        |row| {
                                            row.child(
                                                crate::ui::icon_button(
                                                    gpui::SharedString::from(format!(
                                                        "queue-remove-{i}"
                                                    )),
                                                    crate::ui::Glyph::Close,
                                                    crate::i18n::t("common.delete"),
                                                )
                                                .xsmall()
                                                .on_click(cx.listener(move |q, _, _, cx| {
                                                    if let Some(e) = q.entries.get_mut(i)
                                                        && matches!(
                                                            e.state,
                                                            State::Pending | State::Failed
                                                        )
                                                    {
                                                        // Retain the slot until cleanup so running tasks
                                                        // keep their indices; cancelled rows are hidden.
                                                        e.state = State::Cancelled;
                                                        e.error =
                                                            Some(crate::i18n::t("queue.cancelled"));
                                                        cx.notify();
                                                    }
                                                })),
                                            )
                                        },
                                    ),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(e.path.to_string_lossy().into_owned()),
                            )
                            .when_some(e.error.clone(), |d, error| {
                                d.child(div().text_xs().text_color(cx.theme().danger).child(error))
                            })
                    }),
            );
        card = card.child(rows);
        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(gpui::rgba(0x00000080))
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|_, _, _, cx| cx.emit(Event::Closed)),
            )
            .child(card)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn paired_sdr_names_avoid_reserved_outputs() {
        let path = PathBuf::from("/nonexistent/edited.png");
        let reserved = [PathBuf::from("/nonexistent/edited-sdr.jpg")]
            .into_iter()
            .collect();
        assert_eq!(
            paired_path(&path, &reserved),
            PathBuf::from("/nonexistent/edited-sdr-2.jpg")
        );
    }
}
