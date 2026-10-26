//! `SessionImpl`: one open image, its current edit state and its views (doc 01 §3).
//!
//! The session keeps one decoded source, produced for the A-tier slice of the head params
//! (`raw.demosaic`, `raw.ca_correction`, …). When `set_edits` changes that slice, the source
//! is re-derived in the background; views keep rendering the previous source until the new
//! one lands, then re-render. A change of only the A4 key (highlight algorithm or quantised
//! WB) re-runs A4 on the current source instead of re-decoding, and only on `Final` edits:
//! continuous gestures (a WB drag) keep the last reconstruction (doc 02 §7). Jobs whose
//! params need a different A tier than the session's source get it from the engine's source
//! registry (`EngineInner::source_for`), which shares decodes across sessions and jobs
//! (doc 03 §4.1).

use std::sync::{Arc, Weak};

use aigle_core::geometry::{ImagePoint, ImageRect, Size};
use aigle_core::{EditParams, MaskAssetId, Revision};
use aigle_engine_api::{
    Analysis, AnalysisPrecision, AnalysisSet, CancelToken, DisplayProfileId, EncodedImage,
    EngineEvent, Interaction, Job, JobError, JobResult, LensMatch, LoadState, MaskRequest,
    OffscreenOpts, Overlays, ParamsSource, PixelSample, SampleStage, SessionImpl, SourceRef,
    ViewImpl, ViewSpec, WbEstimate, watch,
};
use parking_lot::Mutex;

use crate::analysis;
use crate::encode;
use crate::engine::{EngineInner, load_err, sanitized};
use crate::pipeline::{self, Prepared, RenderCtx, RenderTarget};
use crate::scheduler::{Class, TaskSpec};
use crate::source::highlights::HlKey;
use crate::source::{self, ATier, LoadError, SourceImage};
use crate::view::ViewShared;

type Waiter = Box<dyn FnOnce(Result<Arc<SourceImage>, String>) + Send>;

/// What a background source task produces.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Want {
    /// Full decode (A4 for the params of that moment).
    Decode(ATier),
    /// A4 re-run on the current source.
    Highlights(ATier, HlKey),
}

struct State {
    source: Option<Arc<SourceImage>>,
    failed: Option<String>,
    /// A-tier load in flight and its cancel token.
    loading: Option<(Want, CancelToken)>,
    rev: Revision,
    params: Arc<EditParams>,
    /// Committed ⊕ pending proposal, for views on `ParamsSource::Pending` (doc 01 §5).
    pending: Option<(Revision, Arc<EditParams>)>,
    /// Named snapshots, for views on `ParamsSource::Snapshot`.
    snapshots: std::collections::HashMap<Revision, Arc<EditParams>>,
    views: Vec<Weak<ViewShared>>,
    waiters: Vec<Waiter>,
    closed: bool,
}

pub(crate) struct SessionShared {
    pub engine: Arc<EngineInner>,
    pub id: u64,
    src: SourceRef,
    raw: bool,
    me: Weak<SessionShared>,
    info_tx: watch::Sender<LoadState>,
    state: Mutex<State>,
}

impl SessionShared {
    pub fn open(engine: Arc<EngineInner>, src: SourceRef) -> Arc<SessionShared> {
        let (info_tx, _) = watch::channel(LoadState::Loading);
        let s = Arc::new_cyclic(|me| SessionShared {
            id: engine.next_id(),
            engine,
            raw: source::is_raw_source(&src),
            src,
            me: me.clone(),
            info_tx,
            state: Mutex::new(State {
                source: None,
                failed: None,
                loading: None,
                rev: Revision::ZERO,
                params: Arc::new(EditParams::default()),
                pending: None,
                snapshots: Default::default(),
                views: Vec::new(),
                waiters: Vec::new(),
                closed: false,
            }),
        });
        s.ensure_source(&EditParams::default(), Interaction::Final);
        s
    }

    fn wanted(&self, p: &EditParams) -> ATier {
        ATier::of(p).normalized_for(self.raw)
    }

    /// Start (or restart) the A-tier load if the current source does not match `p`.
    fn ensure_source(&self, p: &EditParams, hint: Interaction) {
        let want_a = self.wanted(p);
        let (want, cancel, from) = {
            let mut st = self.state.lock();
            if st.closed || st.failed.is_some() {
                return;
            }
            let have = st.source.clone().filter(|s| s.a_params == want_a);
            let want = match &have {
                Some(src) => match src.hl_key(p) {
                    Some(k) if Some(k) != src.applied_hl() => Want::Highlights(want_a, k),
                    _ => {
                        // Already satisfied: drop any other load in flight.
                        if let Some((_, c)) = st.loading.take() {
                            c.cancel();
                        }
                        return;
                    }
                },
                None => Want::Decode(want_a),
            };
            if st.loading.as_ref().is_some_and(|(w, _)| *w == want) {
                return;
            }
            if matches!(want, Want::Highlights(..)) && hint == Interaction::Continuous {
                return; // Draft frames reuse the last reconstruction.
            }
            if let Some((_, c)) = st.loading.take() {
                c.cancel();
            }
            let c = CancelToken::new();
            st.loading = Some((want, c.clone()));
            (want, c, have)
        };
        // A-tier work of the photo being opened is Interactive (doc 05 §2).
        let weak = self.me.clone();
        let params = p.clone();
        let name = match want {
            Want::Decode(_) => "load-source",
            Want::Highlights(..) => "highlights",
        };
        self.engine.scheduler.spawn(
            TaskSpec::new(name, Class::Interactive).cancel(cancel.clone()),
            move || {
                let Some(s) = weak.upgrade() else { return };
                let res = match &from {
                    Some(src) => src.with_highlights(&params, &s.engine.ledger, Some(&cancel)),
                    None => s.engine.source_for(&s.src, &params, Some(&cancel)),
                };
                s.finish_load(want, &cancel, res);
            },
        );
    }

    fn finish_load(
        &self,
        want: Want,
        cancel: &CancelToken,
        res: Result<Arc<SourceImage>, LoadError>,
    ) {
        let (waiters, views, first, head) = {
            let mut st = self.state.lock();
            let current = st
                .loading
                .as_ref()
                .is_some_and(|(w, c)| *w == want && !c.is_cancelled());
            if !current || cancel.is_cancelled() {
                return; // superseded
            }
            st.loading = None;
            let first = st.source.is_none();
            match &res {
                Ok(src) => st.source = Some(src.clone()),
                Err(LoadError::Cancelled) => return,
                Err(LoadError::Failed(e)) if first => st.failed = Some(e.clone()),
                Err(LoadError::Failed(_)) => {}
            }
            (
                std::mem::take(&mut st.waiters),
                st.views.clone(),
                first,
                st.params.clone(),
            )
        };
        match (&res, first) {
            (Ok(src), true) => {
                self.info_tx
                    .send_replace(LoadState::Ready(src.info.clone()));
            }
            (Err(LoadError::Failed(e)), true) => {
                self.info_tx.send_replace(LoadState::Failed(e.clone()));
            }
            (Err(LoadError::Failed(e)), false) => {
                self.engine.emit(EngineEvent::Error {
                    message: format!("re-deriving source failed: {e}"),
                });
            }
            _ => {}
        }
        let shared = res.map_err(|e| e.to_string());
        for w in waiters {
            w(shared.clone());
        }
        if shared.is_ok() {
            // The head may have moved on while this ran (e.g. WB changed during a decode).
            self.ensure_source(&head, Interaction::Final);
            for v in views.iter().filter_map(Weak::upgrade) {
                v.retarget(Interaction::Final);
            }
        }
    }

    pub fn source(&self) -> Option<Arc<SourceImage>> {
        self.state.lock().source.clone()
    }

    pub fn current(&self) -> (Revision, Arc<EditParams>) {
        let st = self.state.lock();
        (st.rev, st.params.clone())
    }

    /// Params a view on `ps` renders: the pending layer or a snapshot when set, else the
    /// head (and defaults for `Original`).
    pub fn params_for(&self, ps: &ParamsSource) -> (Revision, Arc<EditParams>) {
        let st = self.state.lock();
        match ps {
            ParamsSource::Original => (st.rev, Arc::new(st.params.original_framing())),
            ParamsSource::Pending => st.pending.clone().unwrap_or((st.rev, st.params.clone())),
            ParamsSource::Snapshot(r) => match st.snapshots.get(r) {
                Some(p) => (*r, p.clone()),
                None => (st.rev, st.params.clone()),
            },
            ParamsSource::Current => (st.rev, st.params.clone()),
        }
    }

    /// The source for a view's params: the session's if it matches, else a shared decode
    /// (a pending layer or snapshot with a different A tier).
    pub fn source_matching(
        &self,
        have: Arc<SourceImage>,
        p: &EditParams,
        cancel: &CancelToken,
    ) -> Option<Arc<SourceImage>> {
        if have.matches(p) {
            return Some(have);
        }
        self.source_for(p, &have, cancel).ok()
    }

    fn retarget_views(&self, pred: impl Fn(&ParamsSource) -> bool, hint: Interaction) {
        let views = {
            let mut st = self.state.lock();
            st.views.retain(|v| v.strong_count() > 0);
            st.views.clone()
        };
        for v in views.iter().filter_map(Weak::upgrade) {
            if pred(&v.params_source()) {
                v.retarget(hint);
            }
        }
    }

    fn when_ready(&self, f: Waiter) {
        let mut st = self.state.lock();
        if st.closed {
            drop(st);
            f(Err("session closed".into()));
        } else if let Some(src) = st.source.clone() {
            drop(st);
            f(Ok(src));
        } else if let Some(e) = st.failed.clone() {
            drop(st);
            f(Err(e));
        } else {
            st.waiters.push(f);
        }
    }

    /// The session's source if it matches `p`, else the current session source if it does
    /// after re-checking, else a private decode for `p`.
    fn source_for(
        &self,
        p: &EditParams,
        have: &Arc<SourceImage>,
        cancel: &CancelToken,
    ) -> JobResult<Arc<SourceImage>> {
        if have.matches(p) {
            return Ok(have.clone());
        }
        if let Some(cur) = self.source().filter(|s| s.matches(p)) {
            return Ok(cur);
        }
        if have.decode_matches(p) {
            return have
                .with_highlights(p, &self.engine.ledger, Some(cancel))
                .map_err(load_err);
        }
        self.engine
            .source_for(&self.src, p, Some(cancel))
            .map_err(load_err)
    }

    /// A one-shot job that runs once the source is loaded.
    fn job<T: Send + 'static>(
        self: &Arc<Self>,
        spec: TaskSpec,
        f: impl FnOnce(&Arc<SessionShared>, &Arc<SourceImage>, &CancelToken) -> JobResult<T>
        + Send
        + 'static,
    ) -> Job<T> {
        let (job, completer) = Job::new();
        let token = completer.cancel_token();
        let me = self.clone();
        self.when_ready(Box::new(move |res| match res {
            Err(e) => completer.complete(Err(JobError::Failed(e))),
            Ok(src) => {
                let engine = me.engine.clone();
                engine.scheduler.spawn(spec.cancel(token.clone()), move || {
                    if completer.is_cancelled() {
                        completer.complete(Err(JobError::Cancelled));
                        return;
                    }
                    completer.complete(f(&me, &src, &token));
                });
            }
        }));
        job
    }

    fn ctx(&self) -> RenderCtx<'_> {
        RenderCtx {
            cache: &self.engine.cache,
            backend: &self.engine.backend,
            owner: self.id,
            full: None,
        }
    }
}

impl SessionImpl for SessionShared {
    fn info(&self) -> watch::Receiver<LoadState> {
        self.info_tx.subscribe()
    }

    fn set_edits(&self, rev: Revision, params: Arc<EditParams>, hint: Interaction) {
        // `doc` already sanitises at commit; this is defensive (NaN would poison cache keys).
        let mut p = (*params).clone();
        if let Err(e) = p.sanitize() {
            self.engine.emit(EngineEvent::Error {
                message: format!("set_edits rejected: {e}"),
            });
            return;
        }
        let views = {
            let mut st = self.state.lock();
            if rev < st.rev || st.closed {
                return; // stale
            }
            let old = st.params.clone();
            st.rev = rev;
            st.params = Arc::new(p);
            st.views.retain(|v| v.strong_count() > 0);
            (st.views.clone(), st.params.clone(), old)
        };
        let (views, params, old) = views;
        self.ensure_source(&params, hint);
        for v in views.iter().filter_map(Weak::upgrade) {
            v.head_changed(&old, &params, hint);
        }
    }

    fn set_pending(&self, layer: Option<(Revision, Arc<EditParams>)>, hint: Interaction) {
        let layer = match layer {
            Some((rev, p)) => {
                let mut q = (*p).clone();
                if let Err(e) = q.sanitize() {
                    self.engine.emit(EngineEvent::Error {
                        message: format!("set_pending rejected: {e}"),
                    });
                    return;
                }
                Some((rev, Arc::new(q)))
            }
            None => None,
        };
        self.state.lock().pending = layer;
        self.retarget_views(|ps| *ps == ParamsSource::Pending, hint);
    }

    fn set_snapshot(&self, rev: Revision, params: Option<Arc<EditParams>>) {
        let params = match params {
            Some(p) => {
                let mut q = (*p).clone();
                if let Err(e) = q.sanitize() {
                    self.engine.emit(EngineEvent::Error {
                        message: format!("set_snapshot rejected: {e}"),
                    });
                    return;
                }
                Some(Arc::new(q))
            }
            None => None,
        };
        {
            let mut st = self.state.lock();
            match params {
                Some(p) => st.snapshots.insert(rev, p),
                None => st.snapshots.remove(&rev),
            };
        }
        self.retarget_views(|ps| *ps == ParamsSource::Snapshot(rev), Interaction::Final);
    }

    fn create_view(&self, spec: ViewSpec) -> Arc<dyn ViewImpl> {
        let me = self.self_arc();
        let v = ViewShared::new(me, spec);
        let loaded = {
            let mut st = self.state.lock();
            st.views.push(Arc::downgrade(&v));
            st.source.is_some()
        };
        if loaded {
            v.retarget(Interaction::Final);
        }
        v
    }

    fn sample(&self, at: ImagePoint, radius: f32, stage: SampleStage) -> Job<PixelSample> {
        let (_, params) = self.current();
        self.self_arc().job(
            TaskSpec::new("sample", Class::Interactive),
            move |s, src, cancel| {
                let src = &s.source_for(&params, src, cancel)?;
                let prep = Prepared::new(src, &params, &DisplayProfileId::Srgb);
                let rgb = match stage {
                    SampleStage::SceneLinear => {
                        pipeline::sample_scene(&s.ctx(), src, &prep, at, radius)
                    }
                    SampleStage::Display => {
                        let n = (2.0 * radius.max(0.0)).ceil().max(1.0);
                        let region = ImageRect::new(
                            at.x - n as f64 / 2.0,
                            at.y - n as f64 / 2.0,
                            n as f64,
                            n as f64,
                        );
                        let target = RenderTarget {
                            region,
                            out: Size::new(n as u32, n as u32),
                            scale_offset: 0,
                            overlays: Overlays::default(),
                            lanczos: true,
                        };
                        let r = pipeline::render(&s.ctx(), src, &prep, &target, Some(cancel))
                            .map_err(|_| JobError::Cancelled)?;
                        let mut acc = [0f64; 3];
                        let mut count = 0.0;
                        for px in r.data.as_chunks::<4>().0.iter().filter(|p| p[3] > 0) {
                            acc[0] += px[2] as f64;
                            acc[1] += px[1] as f64;
                            acc[2] += px[0] as f64;
                            count += 1.0;
                        }
                        if count == 0.0 {
                            return Err(JobError::Failed(
                                "sample point is outside the crop".into(),
                            ));
                        }
                        acc.map(|v| (v / count / 255.0) as f32)
                    }
                };
                Ok(PixelSample {
                    at,
                    radius,
                    stage,
                    rgb,
                })
            },
        )
    }

    /// Doc 06 §3.1 (`crate::awb`). `region` is in crop space under the current params.
    fn auto_white_balance(&self, region: Option<ImageRect>) -> Job<WbEstimate> {
        let (_, params) = self.current();
        self.self_arc().job(
            TaskSpec::new("auto_white_balance", Class::Interactive),
            move |_, src, _| {
                let r = wb_estimate(src, &params, region)?;
                tracing::debug!(candidates = ?r.candidates, share = r.usable_share, "auto WB");
                Ok(r.estimate)
            },
        )
    }

    fn neutral_white_balance(&self, at: ImagePoint, radius: f32) -> Job<WbEstimate> {
        let (_, params) = self.current();
        self.self_arc().job(
            TaskSpec::new("neutral_white_balance", Class::Interactive),
            move |_, src, _| {
                let (dcp, _) = pipeline::color::dcp(&params);
                let cal = dcp
                    .as_ref()
                    .map_or(&*src.color.calibration, |d| &d.calibration);
                let prep = Prepared::new(src, &params, &DisplayProfileId::Srgb);
                let (cw, ch) = prep.crop_extent();
                if !(0.0..cw).contains(&at.x) || !(0.0..ch).contains(&at.y) {
                    return Err(JobError::Failed("point outside the image".into()));
                }
                let r = (radius as f64).max(0.5);
                let rect = ImageRect::new(at.x - r, at.y - r, 2.0 * r, 2.0 * r);
                let region =
                    crate::awb::region_to_source(rect, |q| prep.crop_to_stored_rgb(q).0[1])
                        .ok_or_else(|| JobError::Failed("empty white balance region".into()))?;
                crate::awb::neutral(src, cal, region)
                    .ok_or_else(|| JobError::Failed("no usable pixels: clipped or too dark".into()))
            },
        )
    }

    fn match_lens(&self) -> Job<Vec<LensMatch>> {
        self.self_arc().job(
            TaskSpec::new("match_lens", Class::Interactive),
            move |_, src, _| {
                let Some(cam) = src.info.camera.as_ref() else {
                    return Ok(Vec::new());
                };
                crate::lens::match_lens(cam).map_err(JobError::Failed)
            },
        )
    }

    /// TODO(doc 06 §5): semantic masks via the inference backend.
    fn compute_mask(&self, _req: MaskRequest) -> Job<MaskAssetId> {
        Job::ready(Err(JobError::Unsupported("semantic masks".into())))
    }

    fn render_offscreen(&self, params: Arc<EditParams>, opts: OffscreenOpts) -> Job<EncodedImage> {
        self.self_arc().job(
            TaskSpec::new("render_offscreen", Class::Agent),
            move |s, src, cancel| {
                let params = sanitized(&params)?;
                let src = &s.source_for(&params, src, cancel)?;
                let le = Some(opts.long_edge.max(1));
                let d = DisplayProfileId::Srgb;
                let after = s
                    .engine
                    .render_full(s.id, src, &params, le, &d, Some(cancel))?;
                let (w, h, data) = if opts.before_after {
                    let original = params.original_framing();
                    let before_src = s.source_for(&original, src, cancel)?;
                    let before =
                        s.engine
                            .render_full(s.id, &before_src, &original, le, &d, Some(cancel))?;
                    encode::side_by_side(
                        (before.width, before.height, &before.data),
                        (after.width, after.height, &after.data),
                    )
                } else {
                    (after.width, after.height, after.data)
                };
                let bytes =
                    encode::encode(w, h, &data, opts.format, &d, None).map_err(JobError::Failed)?;
                Ok(EncodedImage {
                    format: opts.format,
                    width: w,
                    height: h,
                    bytes: bytes.into(),
                })
            },
        )
    }

    fn analyze(&self, params: Arc<EditParams>, what: AnalysisSet) -> Job<Analysis> {
        self.self_arc().job(
            TaskSpec::new("analyze", Class::Agent),
            move |s, src, cancel| {
                let params = sanitized(&params)?;
                let src = &s.source_for(&params, src, cancel)?;
                let d = what.output.clone();
                let (le, precision) = if what.exact {
                    (None, AnalysisPrecision::Exact)
                } else {
                    (
                        Some(s.engine.cfg.overview_long_edge),
                        AnalysisPrecision::Overview,
                    )
                };
                let r = s
                    .engine
                    .render_full(s.id, src, &params, le, &d, Some(cancel))?;
                let (hist, mut clip) = analysis::stats(&analysis::histogram(&r.data));
                clip.sensor_clipped_pct = src.sensor_clipped.map(|f| f * 100.0);
                Ok(Analysis {
                    rev_key: Prepared::new(src, &params, &d).state_key().hex(),
                    precision,
                    histogram: what.histogram.then_some(hist),
                    clipping: what.clipping.then_some(clip),
                    wb_estimate: if what.wb {
                        Some(wb_estimate(src, &params, None)?.estimate)
                    } else {
                        None
                    },
                    noise_estimate: what.noise.then(|| src.noise_estimate().0),
                })
            },
        )
    }

    fn close(&self) {
        let (views, waiters, source, pending, snapshots) = {
            let mut st = self.state.lock();
            if st.closed {
                return;
            }
            st.closed = true;
            if let Some((_, c)) = st.loading.take() {
                c.cancel();
            }
            (
                std::mem::take(&mut st.views),
                std::mem::take(&mut st.waiters),
                st.source.take(),
                st.pending.take(),
                std::mem::take(&mut st.snapshots),
            )
        };
        for v in views.iter().filter_map(Weak::upgrade) {
            v.close();
        }
        // A waiter may own an Arc back to this session. Complete it outside the lock
        // so cancellation cannot leave a reference cycle or a job waiting forever.
        for waiter in waiters {
            waiter(Err("session closed".into()));
        }
        drop((source, pending, snapshots));
        self.engine.cache.remove_owner(self.id);
    }
}

// In-flight work can finish after close and briefly populate the cache again. The
// last internal view/job reference performs a final sweep once no producer remains.
impl Drop for SessionShared {
    fn drop(&mut self) {
        self.engine.cache.remove_owner(self.id);
    }
}

/// Auto WB of `src` under the calibration `params` render with (its DCP, if any).
fn wb_estimate(
    src: &SourceImage,
    params: &EditParams,
    region: Option<ImageRect>,
) -> JobResult<crate::awb::AwbResult> {
    let (dcp, _) = pipeline::color::dcp(params);
    let cal = dcp
        .as_ref()
        .map_or(&*src.color.calibration, |d| &d.calibration);
    let region = match region {
        None => None,
        Some(r) => {
            let prep = Prepared::new(src, params, &DisplayProfileId::Srgb);
            Some(
                crate::awb::region_to_source(r, |q| prep.crop_to_stored_rgb(q).0[1])
                    .ok_or_else(|| JobError::Failed("empty white balance region".into()))?,
            )
        }
    };
    Ok(crate::awb::estimate(src, cal, region))
}

impl SessionShared {
    /// `SessionImpl` methods take `&self`, but views and jobs need an owning handle.
    fn self_arc(&self) -> Arc<SessionShared> {
        self.me
            .upgrade()
            .expect("session alive while a method is called on it")
    }
}

#[cfg(test)]
mod lifetime_tests {
    use super::*;
    use aigle_engine_api::{BackendPreference, EngineConfig, SyntheticPattern};
    use std::time::{Duration, Instant};

    fn wait(mut ready: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !ready() {
            assert!(Instant::now() < deadline, "session cleanup timed out");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn closed_session_releases_source_even_if_closed_view_is_retained() {
        let engine = EngineInner::new(EngineConfig {
            backend: BackendPreference::Cpu,
            worker_threads: Some(2),
            overview_long_edge: 32,
            ..Default::default()
        });
        let session = SessionShared::open(
            engine.clone(),
            SourceRef::Synthetic {
                pattern: SyntheticPattern::Gradient,
                width: 64,
                height: 48,
            },
        );
        wait(|| session.source().is_some());
        let source = Arc::downgrade(&session.source().unwrap());
        let view = session.create_view(ViewSpec::fit(32, 24));
        let frames = view.frames();
        let histogram = view.histogram();
        wait(|| frames.borrow().is_some() && histogram.borrow().is_some());
        session.close();
        assert!(
            session.source().is_none(),
            "closed session retains decoded source"
        );
        assert!(
            frames.borrow().is_none(),
            "closed view retains full preview frame"
        );
        assert!(histogram.borrow().is_none());
        wait(|| source.upgrade().is_none());
        wait(|| engine.ledger.used() == 0);
        // The session/view handles above remain alive throughout the assertions.
    }

    #[test]
    fn closing_drains_waiters_that_retain_the_session() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let engine = EngineInner::new(EngineConfig {
            backend: BackendPreference::Cpu,
            ..Default::default()
        });
        let session = SessionShared::open(
            engine,
            SourceRef::Synthetic {
                pattern: SyntheticPattern::Gradient,
                width: 16,
                height: 12,
            },
        );
        wait(|| session.source().is_some());
        let called = Arc::new(AtomicBool::new(false));
        let flag = called.clone();
        let retained = session.clone();
        session.state.lock().waiters.push(Box::new(move |result| {
            assert!(result.is_err());
            drop(retained);
            flag.store(true, Ordering::Release);
        }));
        session.close();
        assert!(called.load(Ordering::Acquire));
        assert!(session.state.lock().waiters.is_empty());
        session.when_ready(Box::new(|result| assert!(result.is_err())));
        assert!(
            session.state.lock().waiters.is_empty(),
            "closed session accepted another waiter"
        );
    }
}
