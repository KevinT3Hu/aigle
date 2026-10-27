//! `EngineImpl`: engine-wide state, standalone jobs (thumbnail, export) and shared helpers.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Weak};

use aigle_core::EditParams;
use aigle_core::geometry::{Size, fit_long_edge};
use aigle_engine_api::{
    BackendKind, CancelToken, DisplayProfileId, EngineConfig, EngineEvent, EngineImpl,
    EventReceiver, ExportOptions, ExportResult, ImportKind, Imported, Job, JobError, JobResult,
    Overlays, PixelBuffer, PixelFormat, Priority, SessionImpl, SourceRef, Thumbnail, broadcast,
};

use crate::backend::Backend;
use crate::budget;
use crate::cache::{CacheKey, TileCache};
use crate::encode;
use crate::ledger::MemLedger;
use crate::pipeline::{self, Prepared, RenderCtx, RenderTarget, Rendered};
use crate::scheduler::{Class, Scheduler, TaskSpec};
use crate::session::SessionShared;
use crate::source::{self, LoadError, SourceImage};

/// Share of the effective budget the stage-tile cache may hold (doc 03 §5).
const CACHE_SHARE: f64 = 0.5;

pub(crate) struct EngineInner {
    pub cfg: EngineConfig,
    pub ledger: Arc<MemLedger>,
    pub cache: Arc<TileCache>,
    pub scheduler: Scheduler,
    pub backend: Backend,
    /// Why this backend was chosen (or why the GPU was not), for `Engine::backend()`.
    backend_reason: String,
    events: broadcast::Sender<EngineEvent>,
    next_id: AtomicU64,
    /// Decoded sources by [`source::decode_identity`], shared by every session and job that
    /// needs the same decode (doc 03 §4.1). Weak: a source lives as long as its users.
    sources: parking_lot::Mutex<HashMap<CacheKey, Weak<SourceImage>>>,
    /// Rendered thumbnails and previews (doc 03 §7); `None` for the CLI (doc 01 §8).
    pub disk: Option<crate::disk_cache::DiskCache>,
}

impl Drop for EngineInner {
    fn drop(&mut self) {
        self.scheduler.shutdown();
    }
}

impl EngineInner {
    pub fn new(cfg: EngineConfig) -> Arc<EngineInner> {
        let budget = budget::engine_budget(cfg.process, cfg.memory_budget);
        let ledger = MemLedger::new(budget);
        let cache_dir = cfg.cache_dir.clone().or_else(crate::data::cache_dir);
        ledger.set_spill_dir(cache_dir.as_ref().map(|d| d.join("source")));
        let workers = cfg.worker_threads.unwrap_or_else(|| {
            let n = std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(4);
            (n / 4).clamp(2, 4)
        });
        let (events, _) = broadcast::channel(64);
        let (backend, fallback) = Backend::select(
            cfg.backend,
            cfg.deterministic,
            events.clone(),
            &ledger,
            cache_dir.as_deref(),
        );
        if let Some(why) = &fallback {
            tracing::warn!("GPU backend unavailable, using CPU: {why}");
        }
        let backend_reason = fallback.unwrap_or_else(|| backend.description.clone());
        tracing::info!(budget, workers, backend = %backend.description, "aigle engine starting");
        let cache = Arc::new(TileCache::new(ledger.clone(), CACHE_SHARE));
        let weak = Arc::downgrade(&cache);
        ledger.set_reclaimer(Box::new(move |bytes| {
            weak.upgrade().map_or(0, |c| c.reclaim(bytes))
        }));
        budget::PressureMonitor::spawn(ledger.clone(), Arc::downgrade(&cache), events.clone());
        let disk = (cfg.process == aigle_engine_api::ProcessKind::Gui)
            .then(|| cache_dir.clone().map(crate::disk_cache::DiskCache::new))
            .flatten();
        Arc::new(EngineInner {
            cache,
            scheduler: Scheduler::new(workers, ledger.clone()),
            backend,
            backend_reason,
            ledger,
            cfg,
            events,
            next_id: AtomicU64::new(1),
            sources: Default::default(),
            disk,
        })
    }

    /// The source for `params`: a live one with the same decode (re-running A4 if only its
    /// key differs), else a new decode. Concurrent first loads of one file both decode.
    pub fn source_for(
        &self,
        src: &SourceRef,
        params: &EditParams,
        cancel: Option<&CancelToken>,
    ) -> Result<Arc<SourceImage>, LoadError> {
        let id = source::decode_identity(src, params)?;
        let live = {
            let mut m = self.sources.lock();
            m.retain(|_, w| w.strong_count() > 0);
            m.get(&id).and_then(Weak::upgrade)
        };
        if let Some(s) = live {
            if s.matches(params) {
                return Ok(s);
            }
            if s.decode_matches(params) {
                return s.with_highlights(params, &self.ledger, cancel);
            }
        }
        let s = Arc::new(source::load(src, params, &self.ledger, cancel)?);
        self.sources.lock().insert(id, Arc::downgrade(&s));
        Ok(s)
    }

    pub fn next_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    pub fn emit(&self, ev: EngineEvent) {
        let _ = self.events.send(ev);
    }

    /// Run `f` as a scheduled task completing a [`Job`].
    pub fn spawn_job<T: Send + 'static>(
        self: &Arc<Self>,
        spec: TaskSpec,
        f: impl FnOnce(&CancelToken) -> JobResult<T> + Send + 'static,
    ) -> Job<T> {
        let (job, completer) = Job::new();
        let token = completer.cancel_token();
        self.scheduler.spawn(spec.cancel(token.clone()), move || {
            if completer.is_cancelled() {
                completer.complete(Err(JobError::Cancelled));
                return;
            }
            completer.complete(f(&token));
        });
        job
    }

    /// Export: full-resolution render (doc 02 §5.6) streamed to libvips in bands of one tile
    /// row, resized in linear light if `opts.long_edge` asks for it, and written atomically.
    /// Returns the output size.
    pub fn export_streamed(
        &self,
        owner: u64,
        src_ref: &SourceRef,
        src: &SourceImage,
        params: &EditParams,
        opts: &ExportOptions,
        cancel: &CancelToken,
    ) -> JobResult<(u32, u32)> {
        let prep = Prepared::new(src, params, &opts.output);
        let (cw, ch) = prep.crop_extent();
        let (w, h) = (
            (cw.round() as u32).clamp(1, cw.ceil().max(1.0) as u32),
            (ch.round() as u32).clamp(1, ch.ceil().max(1.0) as u32),
        );
        let out = match opts.long_edge {
            Some(le) if le < w.max(h) => fit_long_edge((w as f64, h as f64), le),
            _ => Size::new(w, h),
        };
        let sixteen = encode::sixteen_bit(opts.format);
        let full = if sixteen {
            Some(
                source::full_precision(src_ref, src, &self.ledger, Some(cancel))
                    .map_err(load_err)?,
            )
        } else {
            None
        };
        let ctx = RenderCtx {
            cache: &self.cache,
            backend: &self.backend,
            owner,
            full: full.as_ref().map(|f| &f.0),
        };
        let band = pipeline::TILE;
        type Band = Arc<std::sync::OnceLock<Option<Arc<Vec<u8>>>>>;
        let bands: parking_lot::Mutex<HashMap<u32, Band>> = Default::default();
        let task = crate::arbiter::current();
        let bpp = if sixteen { 6 } else { 3 };
        let band_rows = |k: u32| -> Option<Arc<Vec<u8>>> {
            let cell = {
                let mut m = bands.lock();
                // libvips reads roughly top to bottom; bands two behind are done.
                m.retain(|&b, _| b + 2 >= k);
                m.entry(k).or_default().clone()
            };
            cell.get_or_init(|| {
                let y0 = k * band;
                let rows = (h - y0).min(band);
                crate::arbiter::with_task(task, || {
                    pipeline::export_rows(&ctx, src, &prep, (y0, rows, w), Some(cancel))
                })
                .ok()
                .map(Arc::new)
            })
            .clone()
        };
        let fill = |x: u32, y: u32, rw: u32, rh: u32, buf: &mut [u8], stride: usize| {
            let mut j = 0;
            while j < rh {
                let gy = y + j;
                let k = gy / band;
                let Some(rows) = band_rows(k) else {
                    return false;
                };
                let n = (rh - j).min((k + 1) * band - gy);
                for r in 0..n {
                    let s = ((gy + r - k * band) as usize * w as usize + x as usize) * bpp;
                    let d = (j + r) as usize * stride;
                    buf[d..d + rw as usize * bpp].copy_from_slice(&rows[s..s + rw as usize * bpp]);
                }
                j += n;
            }
            true
        };
        let exif = encode::export_exif(src.info.camera.as_ref(), opts);
        let vopts = aigle_vips_sys::SaveOpts {
            width: w,
            height: h,
            sixteen_bit: sixteen,
            out_size: (out.w, out.h),
            icc: Some(encode::icc_name(&opts.output)),
            exif: exif.as_ref(),
            suffix: &encode::suffix(opts.format),
            sharpen: encode::sharpen(opts.sharpening),
        };
        encode::save_streamed(&opts.path, &vopts, &fill).map_err(|e| {
            if cancel.is_cancelled() {
                JobError::Cancelled
            } else {
                JobError::Failed(format!("export: {e}"))
            }
        })?;
        Ok((out.w, out.h))
    }

    /// Render the whole crop. `long_edge = None` renders at full resolution (render scale 1).
    pub fn render_full(
        &self,
        owner: u64,
        src: &SourceImage,
        params: &EditParams,
        long_edge: Option<u32>,
        display: &DisplayProfileId,
        cancel: Option<&CancelToken>,
    ) -> JobResult<Rendered> {
        let prep = Prepared::new(src, params, display);
        let (cw, ch) = prep.crop_extent();
        let out = match long_edge {
            Some(le) => fit_long_edge((cw, ch), le),
            None => Size::new(cw.round().max(1.0) as u32, ch.round().max(1.0) as u32),
        };
        let region = prep.gm.crop_bounds();
        let target = RenderTarget {
            region,
            out,
            scale_offset: 0,
            overlays: Overlays::default(),
            lanczos: true,
        };
        pipeline::render(
            &RenderCtx {
                cache: &self.cache,
                backend: &self.backend,
                owner,
                full: None,
            },
            src,
            &prep,
            &target,
            cancel,
        )
        .map_err(|_| JobError::Cancelled)
    }
}

pub(crate) fn load_err(e: LoadError) -> JobError {
    match e {
        LoadError::Cancelled => JobError::Cancelled,
        LoadError::Failed(m) => JobError::Failed(m),
    }
}

pub(crate) fn sanitized(p: &EditParams) -> JobResult<EditParams> {
    let mut p = p.clone();
    p.sanitize()
        .map_err(|e| JobError::Failed(format!("invalid params: {e}")))?;
    Ok(p)
}

/// Releases the tiles of a standalone job (no session) when it ends.
struct OwnerGuard<'a>(&'a EngineInner, u64);

impl Drop for OwnerGuard<'_> {
    fn drop(&mut self) {
        self.0.cache.remove_owner(self.1);
    }
}

/// The engine handle given to `aigle_engine_api::Engine`.
pub(crate) struct AigleEngine(pub Arc<EngineInner>);

impl EngineImpl for AigleEngine {
    fn open(&self, src: SourceRef) -> Arc<dyn SessionImpl> {
        SessionShared::open(self.0.clone(), src)
    }

    /// Unedited raws use the embedded preview (doc 01 §6); everything else is rendered, and
    /// rendered results of files are kept in the disk cache.
    fn thumbnail(
        &self,
        src: SourceRef,
        params: Option<Arc<EditParams>>,
        max_px: u32,
        prio: Priority,
    ) -> Job<Thumbnail> {
        let engine = self.0.clone();
        let max_px = max_px.max(1);
        self.0.spawn_job(
            TaskSpec::new("thumbnail", Class::Background).prio(prio.0),
            move |cancel| {
                if let SourceRef::File(path) = &src
                    && params.is_none()
                    && source::is_raw_source(&src)
                {
                    match source::embedded_preview(path) {
                        Ok((w, h, rgb)) => {
                            return Ok(embedded_thumbnail(w, h, &rgb, max_px));
                        }
                        Err(e) => {
                            tracing::debug!(path = %path.display(), "no embedded preview: {e}")
                        }
                    }
                }
                let params = match params {
                    Some(p) => sanitized(&p)?,
                    None => EditParams::default(),
                };
                let area = crate::disk_cache::Area::for_size(max_px);
                let key = match (&engine.disk, &src) {
                    (Some(_), SourceRef::File(_)) => {
                        source::decode_identity(&src, &params).ok().map(|id| {
                            crate::cache::KeyBuilder::new("thumbnail")
                                .key(&id)
                                .json(&params)
                                .u32(crate::pipeline::keys::C1_IMPL)
                                .u32(crate::pipeline::keys::OUT_IMPL)
                                .u32(crate::detail::S_IMPL)
                                .u32(max_px)
                                .finish()
                        })
                    }
                    _ => None,
                };
                if let (Some(disk), Some(key)) = (&engine.disk, &key)
                    && let Some(bytes) = disk.get(area, key)
                {
                    match aigle_vips_sys::decode_rgb8(&bytes) {
                        Ok((w, h, rgb)) => {
                            let mut t = embedded_thumbnail(w, h, &rgb, max_px);
                            t.embedded = false;
                            return Ok(t);
                        }
                        Err(e) => tracing::warn!("cached thumbnail unreadable: {e}"),
                    }
                }
                let owner = engine.next_id();
                let _guard = OwnerGuard(&engine, owner);
                let source = engine
                    .source_for(&src, &params, Some(cancel))
                    .map_err(load_err)?;
                let r = engine.render_full(
                    owner,
                    &source,
                    &params,
                    Some(max_px),
                    &DisplayProfileId::Srgb,
                    Some(cancel),
                )?;
                if let (Some(disk), Some(key)) = (&engine.disk, &key) {
                    match encode::encode(
                        r.width,
                        r.height,
                        &r.data,
                        aigle_engine_api::EncodedFormat::WebP { quality: 85 },
                        &DisplayProfileId::Srgb,
                        None,
                    ) {
                        Ok(bytes) => disk.put(area, key, &bytes),
                        Err(e) => tracing::warn!("thumbnail not cached: {e}"),
                    }
                }
                Ok(Thumbnail {
                    pixels: PixelBuffer {
                        width: r.width,
                        height: r.height,
                        stride: r.width * 4,
                        format: PixelFormat::Bgra8,
                        data: r.data.into(),
                    },
                    embedded: false,
                })
            },
        )
    }

    fn export(
        &self,
        src: SourceRef,
        params: Arc<EditParams>,
        opts: ExportOptions,
    ) -> Job<ExportResult> {
        let engine = self.0.clone();
        self.0
            .spawn_job(TaskSpec::new("export", Class::Background), move |cancel| {
                let params = sanitized(&params)?;
                let owner = engine.next_id();
                let _guard = OwnerGuard(&engine, owner);
                let source = engine
                    .source_for(&src, &params, Some(cancel))
                    .map_err(load_err)?;
                let (w, h) =
                    engine.export_streamed(owner, &src, &source, &params, &opts, cancel)?;
                Ok(ExportResult {
                    path: opts.path,
                    width: w,
                    height: h,
                })
            })
    }

    fn events(&self) -> EventReceiver {
        self.0.events.subscribe()
    }

    fn backend(&self) -> (BackendKind, String) {
        (self.0.backend.kind(), self.0.backend_reason.clone())
    }

    fn import(&self, kind: ImportKind, bytes: Vec<u8>) -> Job<Imported> {
        Job::ready(crate::data::import(kind, &bytes).map_err(JobError::Failed))
    }
}

fn embedded_thumbnail(w: u32, h: u32, rgb: &[u8], max_px: u32) -> Thumbnail {
    let mut bgra = Vec::with_capacity(w as usize * h as usize * 4);
    for px in rgb.as_chunks::<3>().0 {
        bgra.extend_from_slice(&[px[2], px[1], px[0], 255]);
    }
    let (w, h, data) = if w.max(h) > max_px {
        let s = fit_long_edge((w as f64, h as f64), max_px);
        (s.w, s.h, encode::resize_area(w, h, &bgra, s.w, s.h))
    } else {
        (w, h, bgra)
    };
    Thumbnail {
        pixels: PixelBuffer {
            width: w,
            height: h,
            stride: w * 4,
            format: PixelFormat::Bgra8,
            data: data.into(),
        },
        embedded: true,
    }
}

#[cfg(test)]
mod source_registry_tests {
    use super::*;
    use aigle_engine_api::SyntheticPattern;

    #[test]
    fn sources_are_shared_while_alive() {
        let e = EngineInner::new(EngineConfig {
            backend: aigle_engine_api::BackendPreference::Cpu,
            ..Default::default()
        });
        let src = SourceRef::Synthetic {
            pattern: SyntheticPattern::Gradient,
            width: 300,
            height: 200,
        };
        let p = EditParams::default();
        let a = e.source_for(&src, &p, None).unwrap();
        let b = e.source_for(&src, &p, None).unwrap();
        assert!(Arc::ptr_eq(&a, &b));
        let used = e.ledger.used();
        drop((a, b));
        assert!(e.ledger.used() < used, "released with its last user");
    }
}
