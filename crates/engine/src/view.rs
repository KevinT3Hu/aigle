//! `ViewImpl`: one latest-wins render slot (doc 01 §4, doc 05 §3.2, §4).
//!
//! Each view runs at most one render at a time. Updates replace the target; when a render
//! finishes, the loop picks up the newest target. Draft renders run to completion (they are
//! budgeted to ~33 ms) so a continuous drag always produces frames; an in-flight Refined
//! render is cancelled at the next tile boundary when the target changes.
//!
//! Tiles are computed centre-out; after a settled frame at ≥ 1:1 the one-tile ring around
//! the viewport is pre-rendered at `Background` class so that short pans hit the cache
//! (doc 05 §4.3).
//!
//! Views on `ParamsSource::Pending` / `Snapshot(rev)` render the layer `doc` registered with
//! `Session::set_pending` / `set_snapshot`, and the head while there is none.

use std::sync::{Arc, Weak};
use std::time::Duration;

use aigle_core::geometry::Size;
use aigle_core::{EditParams, Revision};
use aigle_engine_api::{
    CancelToken, DisplayProfileId, Frame, Histogram, Interaction, Overlays, ParamsSource,
    PixelBuffer, PixelFormat, Quality, ViewImpl, ViewSpec, Viewport, watch,
};
use parking_lot::Mutex;

use crate::analysis;
use crate::pipeline::{self, Prepared, RenderCtx, RenderTarget};
use crate::scheduler::{Class, TaskSpec};
use crate::session::SessionShared;

/// Draft frame budget (doc 05 Q31).
const DRAFT_BUDGET: Duration = Duration::from_millis(33);
/// Settle time before Refined after the last Continuous update (doc 05 Q32).
const SETTLE: Duration = Duration::from_millis(150);
const MAX_DRAFT_OFFSET: i32 = 6;

struct State {
    viewport: Option<Viewport>,
    fit_size: (u32, u32),
    params_source: ParamsSource,
    overlays: Overlays,
    uncropped: bool,
    /// Bumped on every target change.
    generation: u64,
    want: Option<Quality>,
    rendering: bool,
    inflight_refined: Option<CancelToken>,
    /// Background pre-render of the tile ring.
    ring: Option<CancelToken>,
    draft_offset: i32,
    fast_streak: u32,
    /// Histogram waiting to be computed, latest wins.
    hist_want: Option<HistRequest>,
    /// What the latest requested histogram is for.
    hist_for: Option<HistRequest>,
    hist_busy: bool,
    closed: bool,
}

#[derive(Clone)]
struct HistRequest {
    src: Arc<crate::source::SourceImage>,
    params: Arc<EditParams>,
    long_edge: u32,
}

/// Overview long edge for the histogram during a gesture.
const DRAFT_HIST_EDGE: u32 = 384;

pub(crate) struct ViewShared {
    session: Arc<SessionShared>,
    me: Weak<ViewShared>,
    frames_tx: watch::Sender<Option<Arc<Frame>>>,
    hist_tx: watch::Sender<Option<Arc<Histogram>>>,
    histogram_cancel: CancelToken,
    state: Mutex<State>,
}

impl ViewShared {
    pub fn new(session: Arc<SessionShared>, spec: ViewSpec) -> Arc<ViewShared> {
        Arc::new_cyclic(|me| ViewShared {
            session,
            me: me.clone(),
            frames_tx: watch::channel(None).0,
            hist_tx: watch::channel(None).0,
            histogram_cancel: CancelToken::new(),
            state: Mutex::new(State {
                viewport: spec.viewport,
                fit_size: spec.fit_size,
                params_source: spec.params,
                overlays: spec.overlays,
                uncropped: false,
                generation: 0,
                want: None,
                rendering: false,
                inflight_refined: None,
                ring: None,
                draft_offset: 0,
                fast_streak: 0,
                hist_want: None,
                hist_for: None,
                hist_busy: false,
                closed: false,
            }),
        })
    }

    /// The target changed (params, viewport, source, overlays).
    pub fn retarget(self: &Arc<Self>, hint: Interaction) {
        let generation = {
            let mut st = self.state.lock();
            if st.closed {
                return;
            }
            st.generation += 1;
            st.want = Some(match hint {
                Interaction::Continuous => Quality::Draft,
                Interaction::Final => Quality::Refined,
            });
            if let Some(c) = st.inflight_refined.take() {
                c.cancel();
            }
            if let Some(c) = st.ring.take() {
                c.cancel();
            }
            st.generation
        };
        if hint == Interaction::Continuous {
            let weak = Arc::downgrade(self);
            self.session.engine.scheduler.spawn(
                TaskSpec::new("view-settle", Class::Interactive).delay(SETTLE),
                move || {
                    let Some(v) = weak.upgrade() else { return };
                    {
                        let mut st = v.state.lock();
                        if st.closed || st.generation != generation {
                            return; // superseded by a newer update, which has its own timer
                        }
                        st.want = Some(Quality::Refined);
                    }
                    v.kick();
                },
            );
        }
        self.kick();
    }

    fn kick(self: &Arc<Self>) {
        {
            let mut st = self.state.lock();
            if st.rendering || st.closed || st.want.is_none() || self.session.source().is_none() {
                return;
            }
            st.rendering = true;
        }
        let me = self.clone();
        self.session.engine.scheduler.spawn(
            TaskSpec::new("view-render", Class::Interactive),
            move || {
                me.render_loop();
            },
        );
    }

    fn render_loop(self: &Arc<Self>) {
        loop {
            let (quality, generation, vp, fit, psrc, overlays, uncropped, offset, cancel) = {
                let mut st = self.state.lock();
                let Some(q) = st.want.take().filter(|_| !st.closed) else {
                    st.rendering = false;
                    return;
                };
                let cancel = CancelToken::new();
                if q != Quality::Draft {
                    st.inflight_refined = Some(cancel.clone());
                }
                let offset = if q == Quality::Draft {
                    st.draft_offset
                } else {
                    0
                };
                (
                    q,
                    st.generation,
                    st.viewport.clone(),
                    st.fit_size,
                    st.params_source.clone(),
                    st.overlays.clone(),
                    st.uncropped,
                    offset,
                    cancel,
                )
            };

            let Some(src) = self.session.source() else {
                self.state.lock().rendering = false;
                return;
            };
            let (rev, params) = self.params_for(&psrc);
            // Layers other than the head may need another A tier (shared decode); the head
            // keeps rendering the previous source while the session re-derives it.
            let src = if psrc == ParamsSource::Current {
                src
            } else {
                match self.session.source_matching(src, &params, &cancel) {
                    Some(s) => s,
                    None => continue,
                }
            };
            let hist_edge = if quality == Quality::Draft {
                DRAFT_HIST_EDGE
            } else {
                self.session.engine.cfg.overview_long_edge
            };
            self.request_histogram(&src, &params, hist_edge);
            let params = if uncropped {
                Arc::new(params.uncropped())
            } else {
                params
            };
            let display = vp.as_ref().map(|v| v.display.clone()).unwrap_or_default();
            let prep = Prepared::new(&src, &params, &display);
            let viewport = vp.unwrap_or_else(|| Viewport {
                region: pipeline::fit_region(
                    prep.crop_extent(),
                    Size::new(fit.0.max(1), fit.1.max(1)),
                ),
                output_size: (fit.0.max(1), fit.1.max(1)),
                display,
            });
            let target = RenderTarget {
                region: viewport.region,
                out: Size::new(viewport.output_size.0.max(1), viewport.output_size.1.max(1)),
                scale_offset: offset,
                overlays,
                lanczos: quality != Quality::Draft,
            };
            let ctx = RenderCtx {
                cache: &self.session.engine.cache,
                backend: &self.session.engine.backend,
                owner: self.session.id,
                full: None,
            };
            let cancel_ref = (quality != Quality::Draft).then_some(&cancel);
            let Ok(r) = pipeline::render(&ctx, &src, &prep, &target, cancel_ref) else {
                continue; // cancelled: a newer target is waiting
            };

            let label = match quality {
                Quality::Draft => Quality::Draft,
                _ if r.render_scale >= 1.0 => Quality::Full,
                _ => Quality::Refined,
            };
            let display = viewport.display.clone();
            let frame = Frame {
                rev,
                viewport,
                quality: label,
                pixels: PixelBuffer {
                    width: r.width,
                    height: r.height,
                    stride: r.width * 4,
                    format: PixelFormat::Bgra8,
                    data: r.data.into(),
                },
            };
            let current = {
                let mut st = self.state.lock();
                if st.closed {
                    st.rendering = false;
                    return;
                }
                self.frames_tx.send_replace(Some(Arc::new(frame)));
                st.inflight_refined = None;
                if quality == Quality::Draft {
                    adapt_draft(&mut st, r.elapsed);
                }
                st.generation == generation
            };

            if quality != Quality::Draft && current && label == Quality::Full {
                self.spawn_ring(&src, &params, &display, &target);
            }
        }
    }

    /// Histogram of the whole image (overview grid, doc 02 §5.2; sRGB for now, doc 06 §2),
    /// computed beside the render loop. It depends only on the source and the params, so pan
    /// and zoom never recompute it; a newer request replaces one that has not started.
    /// The head moved from `old` to `new`: re-render unless this view's output cannot change.
    /// `Original` follows only the framing; an uncropped view ignores the crop but keeps its
    /// histogram (of the cropped image) current.
    pub fn head_changed(
        self: &Arc<Self>,
        old: &EditParams,
        new: &Arc<EditParams>,
        hint: Interaction,
    ) {
        let (source, uncropped) = {
            let st = self.state.lock();
            (st.params_source.clone(), st.uncropped)
        };
        let same = match source {
            ParamsSource::Original => old.geometry == new.geometry && old.lens == new.lens,
            ParamsSource::Current if uncropped => old.uncropped() == new.uncropped(),
            _ => false,
        };
        if !same {
            self.retarget(hint);
        } else if source == ParamsSource::Current
            && let Some(src) = self.session.source()
        {
            let edge = if hint == Interaction::Continuous {
                DRAFT_HIST_EDGE
            } else {
                self.session.engine.cfg.overview_long_edge
            };
            self.request_histogram(&src, new, edge);
        }
    }

    fn request_histogram(
        self: &Arc<Self>,
        src: &Arc<crate::source::SourceImage>,
        params: &Arc<EditParams>,
        long_edge: u32,
    ) {
        {
            let mut st = self.state.lock();
            if st.closed {
                return;
            }
            let have = st.hist_for.as_ref().is_some_and(|h| {
                Arc::ptr_eq(&h.src, src) && *h.params == **params && h.long_edge >= long_edge
            });
            if have {
                return;
            }
            let req = HistRequest {
                src: src.clone(),
                params: params.clone(),
                long_edge,
            };
            st.hist_for = Some(req.clone());
            st.hist_want = Some(req);
            if st.hist_busy {
                return;
            }
            st.hist_busy = true;
        }
        let me = Arc::downgrade(self);
        self.session.engine.scheduler.spawn(
            TaskSpec::new("view-histogram", Class::Interactive)
                .cancel(self.histogram_cancel.clone()),
            move || {
                let Some(v) = me.upgrade() else { return };
                loop {
                    let req = {
                        let mut st = v.state.lock();
                        match st.hist_want.take().filter(|_| !st.closed) {
                            Some(r) => r,
                            None => {
                                st.hist_busy = false;
                                return;
                            }
                        }
                    };
                    if let Ok(o) = v.session.engine.render_full(
                        v.session.id,
                        &req.src,
                        &req.params,
                        Some(req.long_edge),
                        &DisplayProfileId::Srgb,
                        Some(&v.histogram_cancel),
                    ) {
                        let histogram = Arc::new(analysis::histogram(&o.data));
                        let st = v.state.lock();
                        if !st.closed {
                            v.hist_tx.send_replace(Some(histogram));
                        }
                    }
                }
            },
        );
    }

    /// Pre-render the tile ring around a settled ≥ 1:1 viewport at `Background` class (doc 05
    /// §4.3); cancelled by the next target change.
    fn spawn_ring(
        self: &Arc<Self>,
        src: &Arc<crate::source::SourceImage>,
        params: &Arc<EditParams>,
        display: &DisplayProfileId,
        target: &RenderTarget,
    ) {
        let cancel = CancelToken::new();
        {
            let mut st = self.state.lock();
            if st.closed {
                return;
            }
            if let Some(old) = st.ring.replace(cancel.clone()) {
                old.cancel();
            }
        }
        let (me, src, params, display, target) = (
            Arc::downgrade(self),
            src.clone(),
            params.clone(),
            display.clone(),
            target.clone(),
        );
        let c = cancel.clone();
        self.session.engine.scheduler.spawn(
            TaskSpec::new("view-ring", Class::Background).cancel(cancel),
            move || {
                let Some(v) = me.upgrade() else { return };
                let prep = Prepared::new(&src, &params, &display);
                let ctx = RenderCtx {
                    cache: &v.session.engine.cache,
                    backend: &v.session.engine.backend,
                    owner: v.session.id,
                    full: None,
                };
                if let Ok(n) = pipeline::prerender_ring(&ctx, &src, &prep, &target, Some(&c)) {
                    tracing::debug!(tiles = n, "ring pre-rendered");
                }
            },
        );
    }

    fn params_for(&self, src: &ParamsSource) -> (Revision, Arc<EditParams>) {
        self.session.params_for(src)
    }

    pub fn params_source(&self) -> ParamsSource {
        self.state.lock().params_source.clone()
    }

    fn self_arc(&self) -> Arc<ViewShared> {
        self.me
            .upgrade()
            .expect("view alive while a method is called on it")
    }

    pub fn close(&self) {
        let mut st = self.state.lock();
        st.closed = true;
        st.want = None;
        st.hist_want = None;
        st.hist_for = None;
        self.histogram_cancel.cancel();
        self.frames_tx.send_replace(None);
        self.hist_tx.send_replace(None);
        if let Some(c) = st.inflight_refined.take() {
            c.cancel();
        }
        if let Some(c) = st.ring.take() {
            c.cancel();
        }
    }
}

/// Adaptive Draft scale (doc 05 §4.2): over budget → one quantised scale lower; under 50 %
/// of budget for 3 frames → one higher.
fn adapt_draft(st: &mut State, elapsed: Duration) {
    if elapsed > DRAFT_BUDGET {
        st.draft_offset = (st.draft_offset + 1).min(MAX_DRAFT_OFFSET);
        st.fast_streak = 0;
    } else if elapsed < DRAFT_BUDGET / 2 {
        st.fast_streak += 1;
        if st.fast_streak >= 3 {
            st.draft_offset = (st.draft_offset - 1).max(0);
            st.fast_streak = 0;
        }
    } else {
        st.fast_streak = 0;
    }
}

impl ViewImpl for ViewShared {
    fn set_viewport(&self, vp: Viewport, hint: Interaction) {
        self.state.lock().viewport = Some(vp);
        self.self_arc().retarget(hint);
    }

    fn set_params_source(&self, src: ParamsSource) {
        self.state.lock().params_source = src;
        self.self_arc().retarget(Interaction::Final);
    }

    fn set_overlays(&self, o: Overlays) {
        self.state.lock().overlays = o;
        self.self_arc().retarget(Interaction::Final);
    }

    fn set_uncropped(&self, on: bool) {
        {
            let mut st = self.state.lock();
            if st.uncropped == on {
                return;
            }
            st.uncropped = on;
        }
        self.self_arc().retarget(Interaction::Final);
    }

    fn frames(&self) -> watch::Receiver<Option<Arc<Frame>>> {
        self.frames_tx.subscribe()
    }

    fn histogram(&self) -> watch::Receiver<Option<Arc<Histogram>>> {
        self.hist_tx.subscribe()
    }

    fn close(&self) {
        ViewShared::close(self);
    }
}
