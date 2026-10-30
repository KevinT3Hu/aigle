//! An input-transparent Wayland subsurface beneath the GPUI image canvas.
//! It owns no toplevel, focus or input devices. GPUI clears the canvas background
//! above it, then paints the existing tool overlays and menus normally.
use crate::hdr_surface::{CanvasScene, Presenter};
use aigle_engine_api::watch;
use anyhow::{Context, bail};
use gpui::{Bounds, Pixels, Window};
use std::{
    collections::{HashMap, HashSet},
    os::fd::OwnedFd,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
};
use wayland_client::{
    Connection, Dispatch, Proxy, QueueHandle, delegate_noop,
    protocol::{
        wl_compositor, wl_output, wl_region, wl_registry, wl_subcompositor, wl_subsurface,
        wl_surface,
    },
};
use wayland_protocols::wp::{
    color_management::v1::client::{
        wp_color_management_output_v1 as cm_output,
        wp_color_management_surface_feedback_v1 as feedback, wp_color_manager_v1 as cm,
        wp_image_description_info_v1 as info, wp_image_description_v1 as description,
    },
    viewporter::client::{wp_viewport, wp_viewporter},
};

#[derive(Clone, Debug)]
pub struct Status {
    pub available: bool,
    pub presented: bool,
    pub message: String,
}
struct Shared {
    stop: AtomicBool,
    available: AtomicBool,
    presented: AtomicBool,
    hole_painted: AtomicBool,
    scenes: watch::Sender<Option<CanvasScene>>,
    wake: OwnedFd,
}
impl Shared {
    fn wake(&self) {
        let _ = rustix::io::write(&self.wake, &1u64.to_ne_bytes());
    }
}
struct UiSurface {
    conn: Connection,
    child: wl_surface::WlSurface,
    subsurface: wl_subsurface::WlSubsurface,
    viewport: wp_viewport::WpViewport,
    last: Mutex<Option<(i32, i32, i32, i32)>>,
}
impl Drop for UiSurface {
    fn drop(&mut self) {
        self.viewport.destroy();
        self.subsurface.destroy();
        self.child.destroy();
        let _ = self.conn.flush();
    }
}
#[derive(Clone)]
pub struct Handle {
    ui: Arc<UiSurface>,
    shared: Arc<Shared>,
}
impl Handle {
    pub fn available(&self) -> bool {
        self.shared.available.load(Ordering::Acquire)
    }
    pub fn note_hole_painted(&self) {
        if !self.shared.hole_painted.swap(true, Ordering::AcqRel) {
            tracing::info!("GPUI canvas hole painted above native HDR child");
        }
    }
    pub fn presented(&self) -> bool {
        self.shared.presented.load(Ordering::Acquire)
    }
    /// Subsurface position is parent-double-buffered. Issue it in GPUI's paint
    /// phase, before the parent's own commit, rather than racing from a worker.
    pub fn layout(&self, bounds: Bounds<Pixels>, sf: f32) -> (Bounds<Pixels>, (u32, u32)) {
        let rect = (
            f32::from(bounds.origin.x).round() as i32,
            f32::from(bounds.origin.y).round() as i32,
            f32::from(bounds.size.width).round().max(1.0) as i32,
            f32::from(bounds.size.height).round().max(1.0) as i32,
        );
        let mut last = self.ui.last.lock().unwrap();
        if *last != Some(rect) {
            self.ui.subsurface.set_position(rect.0, rect.1);
            self.ui.viewport.set_destination(rect.2, rect.3);
            *last = Some(rect);
        }
        let bounds = Bounds::new(
            gpui::point(gpui::px(rect.0 as f32), gpui::px(rect.1 as f32)),
            gpui::size(gpui::px(rect.2 as f32), gpui::px(rect.3 as f32)),
        );
        (
            bounds,
            (
                (rect.2 as f32 * sf).round().max(1.0) as u32,
                (rect.3 as f32 * sf).round().max(1.0) as u32,
            ),
        )
    }
    pub fn submit(&self, scene: CanvasScene) {
        self.shared.scenes.send_replace(Some(scene));
        self.shared.wake();
    }
}
pub struct CanvasSurface {
    handle: Handle,
    thread: Option<JoinHandle<()>>,
}
impl Drop for CanvasSurface {
    fn drop(&mut self) {
        self.handle.shared.stop.store(true, Ordering::Release);
        self.handle.shared.wake();
        // Join before destroying wl_surface: Vulkan holds its raw child handle.
        // The owned GPUI connection clone also outlives all worker GPU objects.
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
impl CanvasSurface {
    pub fn handle(&self) -> Handle {
        self.handle.clone()
    }
    pub fn new(window: &Window) -> anyhow::Result<(Self, watch::Receiver<Status>)> {
        let owner = window
            .native_surface_owner()
            .context("Wayland native canvas is unavailable")?;
        let owner = Arc::downcast::<(Connection, wl_surface::WlSurface)>(owner)
            .map_err(|_| anyhow::anyhow!("unsupported native canvas backend"))?;
        let (conn, parent) = (&owner.0, &owner.1);
        let mut queue = conn.new_event_queue::<State>();
        let qh = queue.handle();
        conn.display().get_registry(&qh, ());
        let mut state = State::default();
        queue.roundtrip(&mut state)?;
        queue.roundtrip(&mut state)?;
        let manager = state
            .manager
            .clone()
            .context("compositor has no color-management-v1")?;
        if !state
            .transfers
            .contains(&(cm::TransferFunction::St2084Pq as u32))
            || !state.primaries.contains(&(cm::Primaries::Bt2020 as u32))
        {
            bail!("compositor does not accept Rec.2020 PQ");
        }
        let child = state
            .compositor
            .as_ref()
            .context("no wl_compositor")?
            .create_surface(&qh, ());
        let subsurface = state
            .subcompositor
            .as_ref()
            .context("no wl_subcompositor")?
            .get_subsurface(&child, parent, &qh, ());
        subsurface.set_desync();
        subsurface.place_below(parent);
        let empty = state.compositor.as_ref().unwrap().create_region(&qh, ());
        child.set_input_region(Some(&empty));
        empty.destroy();
        let viewport = state
            .viewporter
            .as_ref()
            .context("no wp_viewporter")?
            .get_viewport(&child, &qh, ());
        for (&id, output) in &state.outputs {
            let color = manager.get_output(&output.output, &qh, id);
            color.get_image_description(&qh, id + 2);
            state.color_outputs.push(color);
        }
        let feedback = manager.get_surface_feedback(&child, &qh, ());
        feedback.get_preferred(&qh, 1);
        state.feedback = Some(feedback);
        child.commit();
        conn.flush()?;
        let (scenes, rx) = watch::channel(None);
        let shared = Arc::new(Shared {
            stop: AtomicBool::new(false),
            available: AtomicBool::new(false),
            presented: AtomicBool::new(false),
            hole_painted: AtomicBool::new(false),
            scenes,
            wake: rustix::event::eventfd(
                0,
                rustix::event::EventfdFlags::CLOEXEC | rustix::event::EventfdFlags::NONBLOCK,
            )?,
        });
        let (status, status_rx) = watch::channel(Status {
            available: false,
            presented: false,
            message: "Starting native canvas".into(),
        });
        let worker_shared = shared.clone();
        let connection = conn.clone();
        let parent = parent.clone();
        let surface = child.clone();
        let thread = std::thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run(
                    connection,
                    parent,
                    surface,
                    queue,
                    state,
                    rx,
                    &worker_shared,
                    &status,
                )
            }));
            worker_shared.available.store(false, Ordering::Release);
            worker_shared.presented.store(false, Ordering::Release);
            let message = match result {
                Ok(Ok(())) => "Native canvas closed".into(),
                Ok(Err(e)) => format!("SDR fallback: {e}"),
                Err(_) => "SDR fallback: native renderer failed".into(),
            };
            status.send_replace(Status {
                available: false,
                presented: false,
                message,
            });
        });
        let handle = Handle {
            ui: Arc::new(UiSurface {
                conn: conn.clone(),
                child,
                subsurface,
                viewport,
                last: Mutex::new(None),
            }),
            shared,
        };
        Ok((
            Self {
                handle,
                thread: Some(thread),
            },
            status_rx,
        ))
    }
}
struct Output {
    output: wl_output::WlOutput,
    name: String,
    peak: Option<u32>,
    reference: Option<u32>,
}
#[derive(Default)]
struct State {
    compositor: Option<wl_compositor::WlCompositor>,
    subcompositor: Option<wl_subcompositor::WlSubcompositor>,
    viewporter: Option<wp_viewporter::WpViewporter>,
    manager: Option<cm::WpColorManagerV1>,
    feedback: Option<feedback::WpColorManagementSurfaceFeedbackV1>,
    outputs: HashMap<u32, Output>,
    color_outputs: Vec<cm_output::WpColorManagementOutputV1>,
    active: Vec<wl_output::WlOutput>,
    transfers: HashSet<u32>,
    primaries: HashSet<u32>,
    target: Option<u32>,
    reference: Option<u32>,
}
impl State {
    fn message(&self) -> String {
        let outputs: Vec<_> = self
            .outputs
            .values()
            .filter(|o| self.active.contains(&o.output))
            .collect();
        let names = outputs
            .iter()
            .map(|o| o.name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        let target = outputs
            .iter()
            .filter_map(|o| o.peak.map(|p| (p, o.reference.unwrap_or(203))))
            .max()
            .or_else(|| self.target.map(|p| (p, self.reference.unwrap_or(203))));
        match target {
            Some((peak, white)) if peak > white => format!("HDR canvas · {names}"),
            Some(_) => format!("Native canvas · {names} (SDR display target)"),
            None => "Native HDR canvas (display luminance not reported)".into(),
        }
    }
}
#[allow(clippy::too_many_arguments)]
fn run(
    conn: Connection,
    parent: wl_surface::WlSurface,
    surface: wl_surface::WlSurface,
    mut queue: wayland_client::EventQueue<State>,
    mut state: State,
    mut scenes: watch::Receiver<Option<CanvasScene>>,
    shared: &Shared,
    status: &watch::Sender<Status>,
) -> anyhow::Result<()> {
    let mut presenter = Presenter::new(conn.clone(), surface)?;
    tracing::info!(format=?presenter.format(),"native canvas attached beneath GPUI parent");
    shared.available.store(true, Ordering::Release);
    status.send_replace(Status {
        available: true,
        presented: false,
        message: "Native canvas ready".into(),
    });
    let mut scene = None;
    let mut dirty = false;
    let mut message = String::new();
    let mut presented = false;
    loop {
        queue.dispatch_pending(&mut state)?;
        if shared.stop.load(Ordering::Acquire) || !parent.is_alive() {
            break;
        }
        if scenes.has_changed().unwrap_or(false) {
            scene = scenes.borrow_and_update().clone();
            dirty = true;
        }
        if dirty
            && let Some(scene) = scene.as_ref()
            && scene.has_images()
            && presenter.present(scene)?
        {
            if !presented {
                tracing::info!(size=?scene.size, layers=scene.frames.iter().filter(|f|f.is_some()).count(),
                "first native HDR frame presented inside editor canvas");
            }
            dirty = false;
            presented = true;
            shared.presented.store(true, Ordering::Release);
        }
        let next = state.message();
        if next != message || status.borrow().presented != presented {
            status.send_replace(Status {
                available: true,
                presented,
                message: next.clone(),
            });
            message = next;
        }
        conn.flush()?;
        // GPUI's WaylandSource owns socket reading. Another Rust reader could
        // consume input/frame events without waking its event loop. Our private
        // queue receives messages routed by that reader; only UI submissions
        // wake this worker, with a short timeout for display metadata changes.
        let mut fds = [rustix::event::PollFd::new(
            &shared.wake,
            rustix::event::PollFlags::IN,
        )];
        let timeout = rustix::event::Timespec {
            tv_sec: 0,
            tv_nsec: if dirty { 8_000_000 } else { 100_000_000 },
        };
        rustix::event::poll(&mut fds, Some(&timeout))?;
        if !fds[0].revents().is_empty() {
            let _ = rustix::io::read(&shared.wake, &mut [0u8; 8]);
        }
    }
    Ok(())
}
impl Dispatch<wl_registry::WlRegistry, ()> for State {
    fn event(
        s: &mut Self,
        r: &wl_registry::WlRegistry,
        e: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match e {
            wl_registry::Event::Global {
                name,
                interface,
                version,
            } => match interface.as_str() {
                "wl_compositor" => s.compositor = Some(r.bind(name, version.min(4), qh, ())),
                "wl_subcompositor" => s.subcompositor = Some(r.bind(name, 1, qh, ())),
                "wp_viewporter" => s.viewporter = Some(r.bind(name, 1, qh, ())),
                "wp_color_manager_v1" => s.manager = Some(r.bind(name, 1, qh, ())),
                "wl_output" => {
                    let output = r.bind(name, version.min(4), qh, name);
                    s.outputs.insert(
                        name,
                        Output {
                            output,
                            name: format!("output-{name}"),
                            peak: None,
                            reference: None,
                        },
                    );
                }
                _ => {}
            },
            wl_registry::Event::GlobalRemove { name } => {
                s.outputs.remove(&name);
            }
            _ => {}
        }
    }
}
impl Dispatch<cm::WpColorManagerV1, ()> for State {
    fn event(
        s: &mut Self,
        _: &cm::WpColorManagerV1,
        e: cm::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match e {
            cm::Event::SupportedPrimariesNamed { primaries } => {
                s.primaries.insert(primaries.into());
            }
            cm::Event::SupportedTfNamed { tf } => {
                s.transfers.insert(tf.into());
            }
            _ => {}
        }
    }
}
impl Dispatch<description::WpImageDescriptionV1, u32> for State {
    fn event(
        _: &mut Self,
        d: &description::WpImageDescriptionV1,
        e: description::Event,
        target: &u32,
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match e {
            description::Event::Ready { .. } => {
                d.get_information(qh, *target);
                d.destroy();
            }
            description::Event::Failed { .. } => d.destroy(),
            _ => {}
        }
    }
}
impl Dispatch<feedback::WpColorManagementSurfaceFeedbackV1, ()> for State {
    fn event(
        s: &mut Self,
        f: &feedback::WpColorManagementSurfaceFeedbackV1,
        e: feedback::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let feedback::Event::PreferredChanged { .. } = e {
            s.target = None;
            s.reference = None;
            f.get_preferred(qh, 1);
        }
    }
}
impl Dispatch<info::WpImageDescriptionInfoV1, u32> for State {
    fn event(
        s: &mut Self,
        _: &info::WpImageDescriptionInfoV1,
        e: info::Event,
        target: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if *target >= 2 {
            if let Some(output) = s.outputs.get_mut(&(*target - 2)) {
                match e {
                    info::Event::TargetLuminance { max_lum, .. } => output.peak = Some(max_lum),
                    info::Event::Luminances { reference_lum, .. } => {
                        output.reference = Some(reference_lum)
                    }
                    _ => {}
                }
            }
            return;
        }
        match e {
            info::Event::TargetLuminance { max_lum, .. } => s.target = Some(max_lum),
            info::Event::Luminances { reference_lum, .. } => s.reference = Some(reference_lum),
            _ => {}
        }
    }
}
impl Dispatch<cm_output::WpColorManagementOutputV1, u32> for State {
    fn event(
        s: &mut Self,
        o: &cm_output::WpColorManagementOutputV1,
        e: cm_output::Event,
        id: &u32,
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let cm_output::Event::ImageDescriptionChanged = e {
            if let Some(o) = s.outputs.get_mut(id) {
                o.peak = None;
                o.reference = None;
            }
            o.get_image_description(qh, id + 2);
        }
    }
}
impl Dispatch<wl_output::WlOutput, u32> for State {
    fn event(
        s: &mut Self,
        _: &wl_output::WlOutput,
        e: wl_output::Event,
        id: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_output::Event::Name { name } = e
            && let Some(o) = s.outputs.get_mut(id)
        {
            o.name = name;
        }
    }
}
impl Dispatch<wl_surface::WlSurface, ()> for State {
    fn event(
        s: &mut Self,
        _: &wl_surface::WlSurface,
        e: wl_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match e {
            wl_surface::Event::Enter { output } => {
                if !s.active.contains(&output) {
                    s.active.push(output);
                }
            }
            wl_surface::Event::Leave { output } => s.active.retain(|o| *o != output),
            _ => {}
        }
    }
}
delegate_noop!(State:ignore wl_compositor::WlCompositor);
delegate_noop!(State:ignore wl_subcompositor::WlSubcompositor);
delegate_noop!(State:ignore wl_subsurface::WlSubsurface);
delegate_noop!(State:ignore wl_region::WlRegion);
delegate_noop!(State:ignore wp_viewporter::WpViewporter);
delegate_noop!(State:ignore wp_viewport::WpViewport);
