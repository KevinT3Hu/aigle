//! aigle engine interface (doc 01 §3).
//!
//! This crate contains no implementation. The handle types [`Engine`], [`Session`] and
//! [`View`] wrap trait objects ([`EngineImpl`], [`SessionImpl`], [`ViewImpl`]) so that the UI
//! can be built against a mock engine and the engine can later move into a subprocess.
//!
//! Two communication styles:
//! * continuous state (the open image's views) flows through latest-value `watch` channels;
//! * one-shot work returns a [`Job`], which cancels on drop.
//!
//! No call blocks the caller. Channels are `tokio::sync` primitives that do not need a tokio
//! runtime, so they work with GPUI's executor.

mod job;
mod types;

use std::sync::Arc;

use aigle_core::geometry::ImagePoint;
use aigle_core::geometry::ImageRect;
use aigle_core::{EditParams, MaskAssetId, Revision};
pub use job::{CancelToken, Job, JobCompleter, JobError, JobResult};
pub use tokio::sync::{broadcast, watch};
pub use types::*;

pub type EventReceiver = broadcast::Receiver<EngineEvent>;

/// Implementation side of [`Engine`].
pub trait EngineImpl: Send + Sync + 'static {
    fn open(&self, src: SourceRef) -> Arc<dyn SessionImpl>;
    fn thumbnail(
        &self,
        src: SourceRef,
        params: Option<Arc<EditParams>>,
        max_px: u32,
        prio: Priority,
    ) -> Job<Thumbnail>;
    fn export(
        &self,
        src: SourceRef,
        params: Arc<EditParams>,
        opts: ExportOptions,
    ) -> Job<ExportResult>;
    fn events(&self) -> EventReceiver;
    /// Compute backend in use and a human-readable description / fallback reason.
    fn backend(&self) -> (BackendKind, String);
    fn import(&self, kind: ImportKind, bytes: Vec<u8>) -> Job<Imported>;
}

/// Implementation side of [`Session`].
pub trait SessionImpl: Send + Sync + 'static {
    fn info(&self) -> watch::Receiver<LoadState>;
    fn set_edits(&self, rev: Revision, params: Arc<EditParams>, hint: Interaction);
    /// The pending proposal layer (committed ⊕ pending, composed by `doc`) rendered by views
    /// on `ParamsSource::Pending`; `None` when no proposal is open.
    fn set_pending(&self, layer: Option<(Revision, Arc<EditParams>)>, hint: Interaction);
    /// Register (`Some`) or drop (`None`) the params of snapshot `rev`, rendered by views on
    /// `ParamsSource::Snapshot(rev)`.
    fn set_snapshot(&self, rev: Revision, params: Option<Arc<EditParams>>);
    fn create_view(&self, spec: ViewSpec) -> Arc<dyn ViewImpl>;
    fn sample(&self, at: ImagePoint, radius: f32, stage: SampleStage) -> Job<PixelSample>;
    fn auto_white_balance(&self, region: Option<ImageRect>) -> Job<WbEstimate>;
    fn neutral_white_balance(&self, at: ImagePoint, radius: f32) -> Job<WbEstimate>;
    fn match_lens(&self) -> Job<Vec<LensMatch>>;
    fn compute_mask(&self, req: MaskRequest) -> Job<MaskAssetId>;
    fn render_offscreen(&self, params: Arc<EditParams>, opts: OffscreenOpts) -> Job<EncodedImage>;
    fn analyze(&self, params: Arc<EditParams>, what: AnalysisSet) -> Job<Analysis>;
    /// Called when the last [`Session`] handle is dropped: release caches.
    fn close(&self);
}

/// Implementation side of [`View`].
pub trait ViewImpl: Send + Sync + 'static {
    fn set_viewport(&self, vp: Viewport, hint: Interaction);
    fn set_params_source(&self, src: ParamsSource);
    fn set_overlays(&self, o: Overlays);
    /// Render the whole straightened frame, ignoring the crop (crop tool). Viewport regions
    /// are then in that frame's coordinates.
    fn set_uncropped(&self, on: bool);
    fn frames(&self) -> watch::Receiver<Option<Arc<Frame>>>;
    fn histogram(&self) -> watch::Receiver<Option<Arc<Histogram>>>;
    /// Called when the [`View`] handle is dropped.
    fn close(&self);
}

/// The pixel service. Cheap to clone.
#[derive(Clone)]
pub struct Engine {
    inner: Arc<dyn EngineImpl>,
}

impl Engine {
    pub fn from_impl(inner: Arc<dyn EngineImpl>) -> Self {
        Engine { inner }
    }

    /// Async load; returns immediately. Dropping the Session releases its caches.
    pub fn open(&self, src: SourceRef) -> Session {
        Session {
            inner: Arc::new(SessionHandle(self.inner.open(src))),
        }
    }

    /// A thumbnail or preview of `src` with long edge `max_px`. `params = None` (unedited): a
    /// raw's embedded preview where there is one. Rendered results are kept in the disk cache
    /// by file identity, params and engine version (doc 01 §6).
    pub fn thumbnail(
        &self,
        src: SourceRef,
        params: Option<Arc<EditParams>>,
        max_px: u32,
        prio: Priority,
    ) -> Job<Thumbnail> {
        self.inner.thumbnail(src, params, max_px, prio)
    }

    pub fn export(
        &self,
        src: SourceRef,
        params: Arc<EditParams>,
        opts: ExportOptions,
    ) -> Job<ExportResult> {
        self.inner.export(src, params, opts)
    }

    /// Global events: device lost, memory pressure, engine-wide errors.
    pub fn events(&self) -> EventReceiver {
        self.inner.events()
    }

    /// Compute backend currently in use (GPU or CPU fallback) and why (doc 04 §2).
    pub fn backend(&self) -> (BackendKind, String) {
        self.inner.backend()
    }

    /// Validate and pin a user DCP or look LUT (doc 02 §13.2); edits reference the returned
    /// `DataRef`. Importing the same bytes again returns the same reference.
    pub fn import(&self, kind: ImportKind, bytes: Vec<u8>) -> Job<Imported> {
        self.inner.import(kind, bytes)
    }
}

struct SessionHandle(Arc<dyn SessionImpl>);

impl Drop for SessionHandle {
    fn drop(&mut self) {
        self.0.close();
    }
}

/// One open image. Cheap to clone; the image is released when the last clone is dropped.
#[derive(Clone)]
pub struct Session {
    inner: Arc<SessionHandle>,
}

impl Session {
    fn imp(&self) -> &dyn SessionImpl {
        &*self.inner.0
    }

    /// `Loading` → `Ready(ImageInfo)` | `Failed`.
    pub fn info(&self) -> watch::Receiver<LoadState> {
        self.imp().info()
    }

    /// Called by `doc` only (doc 01 §1.3). `Continuous` during gestures, `Final` otherwise.
    pub fn set_edits(&self, rev: Revision, params: Arc<EditParams>, hint: Interaction) {
        self.imp().set_edits(rev, params, hint)
    }

    /// The pending proposal layer for views on [`ParamsSource::Pending`] (doc 01 §5); views
    /// on it render the committed head while it is `None`.
    pub fn set_pending(&self, layer: Option<(Revision, Arc<EditParams>)>, hint: Interaction) {
        self.imp().set_pending(layer, hint)
    }

    /// Register or drop the params of a named snapshot for views on
    /// [`ParamsSource::Snapshot`]; an unregistered snapshot renders the head.
    pub fn set_snapshot(&self, rev: Revision, params: Option<Arc<EditParams>>) {
        self.imp().set_snapshot(rev, params)
    }

    /// A View is one rendered output: main canvas, "before" pane, navigator, loupe…
    pub fn create_view(&self, spec: ViewSpec) -> View {
        View {
            inner: self.imp().create_view(spec),
            _session: self.clone(),
        }
    }

    pub fn sample(&self, at: ImagePoint, radius: f32, stage: SampleStage) -> Job<PixelSample> {
        self.imp().sample(at, radius, stage)
    }

    /// Lens profiles from the lensfun database matching the photo's EXIF lens, best first,
    /// each pinned as a `DataRef`. Empty if the photo has no EXIF lens or nothing matches;
    /// fails if no lensfun database is installed.
    pub fn match_lens(&self) -> Job<Vec<LensMatch>> {
        self.imp().match_lens()
    }

    pub fn auto_white_balance(&self, region: Option<ImageRect>) -> Job<WbEstimate> {
        self.imp().auto_white_balance(region)
    }

    /// White balance picker: the white that makes the disc of `radius` around `at` (Crop
    /// space) neutral. Fails when the disc has no usable (unclipped, not near-black) pixels;
    /// `low_confidence_reason` is set when the result had to be clamped to the param range.
    pub fn neutral_white_balance(&self, at: ImagePoint, radius: f32) -> Job<WbEstimate> {
        self.imp().neutral_white_balance(at, radius)
    }

    /// AI/semantic masks, cached.
    pub fn compute_mask(&self, req: MaskRequest) -> Job<MaskAssetId> {
        self.imp().compute_mask(req)
    }

    /// Render arbitrary params without touching doc state or any View.
    pub fn render_offscreen(
        &self,
        params: Arc<EditParams>,
        opts: OffscreenOpts,
    ) -> Job<EncodedImage> {
        self.imp().render_offscreen(params, opts)
    }

    /// Quantitative image facts; overview precision unless `what.exact`.
    pub fn analyze(&self, params: Arc<EditParams>, what: AnalysisSet) -> Job<Analysis> {
        self.imp().analyze(params, what)
    }
}

/// One rendered output of a session. Keeps its session alive.
pub struct View {
    inner: Arc<dyn ViewImpl>,
    _session: Session,
}

impl View {
    /// `Continuous` during pan/zoom gestures.
    pub fn set_viewport(&self, vp: Viewport, hint: Interaction) {
        self.inner.set_viewport(vp, hint)
    }
    pub fn set_params_source(&self, src: ParamsSource) {
        self.inner.set_params_source(src)
    }
    /// Clipping, mask overlay, focus peaking.
    pub fn set_overlays(&self, o: Overlays) {
        self.inner.set_overlays(o)
    }
    /// Ignore the crop and show the whole straightened frame (crop tool); the histogram
    /// still describes the cropped image.
    pub fn set_uncropped(&self, on: bool) {
        self.inner.set_uncropped(on)
    }
    /// Latest-wins frames.
    pub fn frames(&self) -> watch::Receiver<Option<Arc<Frame>>> {
        self.inner.frames()
    }
    pub fn histogram(&self) -> watch::Receiver<Option<Arc<Histogram>>> {
        self.inner.histogram()
    }
}

impl Drop for View {
    fn drop(&mut self) {
        self.inner.close();
    }
}
