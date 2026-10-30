//! Durable sequential exports. Pause takes effect at file boundaries.
use crate::export::{self, Request, Settings};
use aigle_engine_api::Engine;
use gpui::{
    Context, EventEmitter, IntoElement, ParentElement, Render, Styled, Task, Window, div,
    prelude::*, px,
};
use gpui_component::{ActiveTheme, Sizable, h_flex, v_flex};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
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
#[derive(Clone, Serialize, Deserialize)]
struct Entry {
    request: Request,
    settings: Settings,
    path: PathBuf,
    state: State,
    error: Option<String>,
}
#[derive(Default, Serialize, Deserialize)]
struct Stored {
    entries: Vec<Entry>,
}
impl Stored {
    fn recover(&mut self) {
        for entry in &mut self.entries {
            if entry.state == State::Running {
                // The output might have been written before the durable completion record.
                entry.state = if entry.path.exists() {
                    State::Failed
                } else {
                    State::Pending
                };
                entry.error = Some(crate::i18n::t("queue.interrupted"));
            }
        }
    }
}
pub enum Event {
    Closed,
    Notice(String),
}
pub struct ExportQueue {
    engine: Engine,
    stored: Stored,
    paused: bool,
    error: Option<String>,
    task: Option<Task<()>>,
}
impl EventEmitter<Event> for ExportQueue {}
impl ExportQueue {
    pub fn new(engine: Engine) -> Self {
        let loaded = (|| -> anyhow::Result<Stored> {
            let Some(path) = queue_path() else {
                return Ok(Stored::default());
            };
            match std::fs::read(path) {
                Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Stored::default()),
                Err(e) => Err(e.into()),
            }
        })();
        let (mut stored, error) = match loaded {
            Ok(s) => (s, None),
            Err(e) => (Stored::default(), Some(e.to_string())),
        };
        stored.recover();
        Self {
            engine,
            stored,
            paused: true,
            error,
            task: None,
        }
    }
    pub fn counts(&self) -> (usize, usize) {
        (
            self.stored
                .entries
                .iter()
                .filter(|e| e.state == State::Done)
                .count(),
            self.stored.entries.len(),
        )
    }
    fn persist(&mut self) -> bool {
        let result = (|| -> anyhow::Result<()> {
            let path =
                queue_path().ok_or_else(|| anyhow::anyhow!("No application data directory"))?;
            std::fs::create_dir_all(path.parent().unwrap())?;
            let temp = path.with_extension("json.tmp");
            let mut file = std::fs::File::create(&temp)?;
            use std::io::Write;
            file.write_all(&serde_json::to_vec(&self.stored)?)?;
            file.sync_all()?;
            std::fs::rename(temp, path)?;
            Ok(())
        })();
        match result {
            Ok(()) => {
                self.error = None;
                true
            }
            Err(e) => {
                self.error = Some(e.to_string());
                self.paused = true;
                false
            }
        }
    }
    pub fn enqueue(
        &mut self,
        jobs: Vec<(Request, PathBuf)>,
        settings: Settings,
        cx: &mut Context<Self>,
    ) {
        let mut reserved: std::collections::HashSet<_> =
            self.stored.entries.iter().map(|e| e.path.clone()).collect();
        reserved.extend(jobs.iter().map(|(_, path)| path.clone()));
        for (request, path) in jobs {
            if settings.paired_sdr && settings.format.encoded(settings.quality).is_hdr() {
                let mut sdr = settings.clone();
                sdr.paired_sdr = false;
                sdr.format = export::Format::Jpeg;
                sdr.output = aigle_engine_api::DisplayProfileId::Srgb;
                let sdr_path = paired_path(&path, &reserved);
                reserved.insert(sdr_path.clone());
                self.stored.entries.push(Entry {
                    request: request.clone(),
                    settings: sdr,
                    path: sdr_path,
                    state: State::Pending,
                    error: None,
                });
            }
            self.stored.entries.push(Entry {
                request,
                settings: settings.clone(),
                path,
                state: State::Pending,
                error: None,
            });
        }
        self.paused = false;
        if self.persist() {
            self.start_next(cx);
        }
        cx.notify();
    }
    fn start_next(&mut self, cx: &mut Context<Self>) {
        if self.paused || self.task.is_some() {
            return;
        }
        let Some(index) = self
            .stored
            .entries
            .iter()
            .position(|e| e.state == State::Pending)
        else {
            return;
        };
        self.stored.entries[index].state = State::Running;
        if !self.persist() {
            self.stored.entries[index].state = State::Pending;
            cx.notify();
            return;
        }
        let entry = self.stored.entries[index].clone();
        let engine = self.engine.clone();
        self.task = Some(cx.spawn(async move |this, cx| {
            let path = entry.path.clone();
            let exists = cx.background_spawn(async move { path.try_exists() }).await;
            let result: Result<(), String> = match exists {
                Ok(false) => {
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
                    }
                }
                Ok(true) => Err(crate::i18n::t("queue.exists")),
                Err(e) => Err(e.to_string()),
            };
            let _ = this.update(cx, |q, cx| {
                let entry = &mut q.stored.entries[index];
                entry.state = if result.is_ok() {
                    State::Done
                } else {
                    State::Failed
                };
                entry.error = result.err();
                q.task = None;
                q.persist();
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

fn queue_path() -> Option<PathBuf> {
    crate::data_dir().map(|p| p.join("export-queue.json"))
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
                            if !q.paused && q.persist() {
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
                            for e in &mut q.stored.entries {
                                if e.state == State::Failed {
                                    e.state = State::Pending;
                                    e.error = None;
                                }
                            }
                            if q.persist() {
                                q.start_next(cx);
                            }
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
                        .on_click(cx.listener(|q, _, _, cx| {
                            // Keep stable indices while a task is running.
                            if q.task.is_none() {
                                q.stored
                                    .entries
                                    .retain(|e| !matches!(e.state, State::Done | State::Cancelled));
                                q.persist();
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
        if let Some(error) = &self.error {
            card = card.child(
                div()
                    .text_xs()
                    .text_color(cx.theme().danger)
                    .child(error.clone()),
            );
        }
        let rows = v_flex()
            .id("queue-files")
            .overflow_y_scroll()
            .gap_2()
            .children(self.stored.entries.iter().enumerate().map(|(i, e)| {
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
                            .when(e.state == State::Pending, |row| {
                                row.child(
                                    crate::ui::icon_button(
                                        gpui::SharedString::from(format!("queue-remove-{i}")),
                                        crate::ui::Glyph::Close,
                                        crate::i18n::t("common.delete"),
                                    )
                                    .xsmall()
                                    .on_click(cx.listener(
                                        move |q, _, _, cx| {
                                            if let Some(e) = q.stored.entries.get_mut(i)
                                                && e.state == State::Pending
                                            {
                                                e.state = State::Cancelled;
                                                e.error = Some(crate::i18n::t("queue.cancelled"));
                                                q.persist();
                                                cx.notify();
                                            }
                                        },
                                    )),
                                )
                            }),
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
            }));
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

    #[test]
    fn restart_recovers_incomplete_jobs_without_touching_completed_jobs() {
        let mut stored = Stored {
            entries: [State::Running, State::Pending, State::Failed, State::Done]
                .into_iter()
                .map(|state| Entry {
                    request: Request {
                        source: aigle_engine_api::SourceRef::Synthetic {
                            width: 16,
                            height: 16,
                            pattern: aigle_engine_api::SyntheticPattern::Gradient,
                        },
                        params: Default::default(),
                        name: "test".into(),
                        folder: None,
                    },
                    settings: Default::default(),
                    path: PathBuf::from("/nonexistent/aigle-export-queue-test.png"),
                    state,
                    error: None,
                })
                .collect(),
        };
        stored.recover();
        assert_eq!(
            stored.entries.iter().map(|e| e.state).collect::<Vec<_>>(),
            [State::Pending, State::Pending, State::Failed, State::Done]
        );
        stored.entries[0].state = State::Running;
        stored.entries[0].path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        stored.recover();
        assert_eq!(stored.entries[0].state, State::Failed);
        assert!(stored.entries[0].path.exists());
        let decoded: Stored =
            serde_json::from_slice(&serde_json::to_vec(&stored).unwrap()).unwrap();
        assert_eq!(decoded.entries.len(), 4);
        assert_eq!(decoded.entries[3].state, State::Done);
    }
}
