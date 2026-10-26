//! Lightweight resource sampling for this process only. OS reads run off the UI thread.
use std::time::Duration;

use gpui::{Context, IntoElement, ParentElement, Render, Styled, Task, Window, div, prelude::*};
use gpui_component::{ActiveTheme, tooltip::Tooltip};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

const INTERVAL: Duration = Duration::from_secs(2);

#[derive(Clone, Copy, PartialEq)]
struct Usage {
    cpu_percent: Option<f32>,
    resident_bytes: u64,
}

struct Sampler {
    system: System,
    pid: Pid,
    primed: bool,
}

impl Sampler {
    fn new() -> Option<Self> {
        if !sysinfo::IS_SUPPORTED_SYSTEM {
            return None;
        }
        Some(Self {
            system: System::new(),
            pid: sysinfo::get_current_pid().ok()?,
            primed: false,
        })
    }

    fn sample(&mut self) -> Option<Usage> {
        let refreshed = self.system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[self.pid]),
            ProcessRefreshKind::new().with_cpu().with_memory(),
        );
        if refreshed == 0 {
            self.primed = false;
            return None;
        }
        let process = self.system.process(self.pid)?;
        let resident_bytes = process.memory();
        if resident_bytes == 0 {
            self.primed = false;
            return None;
        }
        // CPU is a delta between samples. Never display the initial zero as a measurement.
        // sysinfo uses 100% per logical core, so a multithreaded process may exceed 100%.
        let cpu_percent = self
            .primed
            .then(|| process.cpu_usage())
            .filter(|value| value.is_finite() && *value >= 0.0);
        self.primed = true;
        Some(Usage {
            cpu_percent,
            resident_bytes,
        })
    }
}

pub struct Resources {
    usage: Option<Usage>,
    _poll: Task<()>,
}

impl Resources {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let poll = cx.spawn(async move |this, cx| {
            let sampler = cx
                .background_executor()
                .spawn(async { Sampler::new() })
                .await;
            let Some(mut sampler) = sampler else {
                return;
            };
            loop {
                let (next, usage) = cx
                    .background_executor()
                    .spawn(async move {
                        let usage = sampler.sample();
                        (sampler, usage)
                    })
                    .await;
                sampler = next;
                if this
                    .update(cx, |meter, cx| {
                        if meter.usage != usage {
                            meter.usage = usage;
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    break;
                }
                cx.background_executor().timer(INTERVAL).await;
            }
        });
        Self {
            usage: None,
            _poll: poll,
        }
    }
}

impl Render for Resources {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(usage) = self.usage else {
            return div().into_any_element();
        };
        let mib = usage.resident_bytes as f64 / (1024.0 * 1024.0);
        let memory = if mib >= 1024.0 {
            format!("{:.1} GiB", mib / 1024.0)
        } else {
            format!("{mib:.0} MiB")
        };
        let ram = crate::i18n::t("内存");
        let label = match usage.cpu_percent {
            Some(cpu) => format!("CPU {cpu:.0}% · {ram} {memory}"),
            None => format!("{ram} {memory}"),
        };
        div().id("app-resources").flex_none().text_xs().text_color(cx.theme().muted_foreground)
            .child(label)
            .tooltip(|window, cx| Tooltip::new(crate::i18n::t("应用的 CPU 使用率和驻留内存，每两秒更新。CPU 的 100% 表示一个逻辑核心，多线程时可超过 100%。")).build(window, cx))
            .into_any_element()
    }
}
