//! `EngineImpl`: engine-wide state, standalone jobs (thumbnail, export) and shared helpers.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Weak};

use aigle_core::EditParams;
use aigle_core::geometry::{Size, fit_long_edge};
use aigle_engine_api::{
    BackendKind, CancelToken, DisplayProfileId, EncodedFormat, EngineConfig, EngineEvent,
    EngineImpl, EventReceiver, ExportOptions, ExportResult, ImportKind, Imported, Job, JobError,
    JobResult, Overlays, PixelBuffer, PixelFormat, Priority, SessionImpl, SourceRef, Thumbnail,
    broadcast,
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
                return source::guarded(|| s.with_highlights(params, &self.ledger, cancel));
            }
        }
        let s = Arc::new(source::guarded(|| {
            source::load(src, params, &self.ledger, cancel)
        })?);
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
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(&token)))
                .unwrap_or_else(|p| {
                    let msg = aigle_isolate::panic_message(p.as_ref());
                    tracing::error!("engine job panicked: {msg}");
                    Err(JobError::Failed(format!("internal error: {msg}")))
                });
            completer.complete(r);
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
        let hdr = opts.format.is_hdr();
        if opts.output == DisplayProfileId::HdrRec2020 && !hdr {
            return Err(JobError::Failed(
                "HDR output requires HDR PNG, floating-point TIFF or OpenEXR".into(),
            ));
        }
        let output = if hdr {
            DisplayProfileId::HdrRec2020
        } else {
            opts.output.clone()
        };
        let prep = Prepared::new(src, params, &output);
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
        let full = if sixteen || hdr {
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
        let bpp = if hdr {
            12
        } else if sixteen {
            6
        } else {
            3
        };
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
        let to_srgb = aigle_core::color::to_f32(&aigle_core::color::mul(
            &aigle_core::color::invert(&aigle_core::color::SRGB_TO_XYZ).unwrap(),
            &aigle_core::color::REC2020_TO_XYZ,
        ));
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
                    if hdr {
                        for pixel in buf[d..d + rw as usize * bpp].as_chunks_mut::<12>().0 {
                            let rgb = std::array::from_fn(|c| {
                                f32::from_le_bytes(pixel[c * 4..c * 4 + 4].try_into().unwrap())
                            });
                            let rgb = aigle_kernels::math::mat3_mul(&to_srgb, rgb);
                            for (c, v) in rgb.into_iter().enumerate() {
                                pixel[c * 4..c * 4 + 4].copy_from_slice(&v.to_ne_bytes());
                            }
                        }
                    }
                }
                j += n;
            }
            true
        };
        let exif = encode::export_exif(src.info.camera.as_ref(), opts);
        let icc = if hdr {
            crate::output_space::Icc::Data(crate::output_space::linear_srgb_icc().into())
        } else {
            crate::output_space::icc(&opts.output)
        };
        let (icc_name, icc_data) = encode::icc_args(&icc);
        let watermark = match &opts.watermark {
            Some(wm) if hdr => {
                crate::watermark::prepare_hdr(wm, (out.w, out.h)).map_err(JobError::Failed)?
            }
            Some(wm) => crate::watermark::prepare(wm, (out.w, out.h), &opts.output, sixteen)
                .map_err(JobError::Failed)?,
            None => None,
        };
        let vopts = aigle_vips_sys::SaveOpts {
            width: w,
            height: h,
            sixteen_bit: sixteen,
            floating_point: hdr,
            out_size: (out.w, out.h),
            icc: icc_name,
            exif: exif.as_ref(),
            suffix: &encode::suffix(opts.format),
            sharpen: encode::sharpen(opts.sharpening),
            icc_data,
            resize_gamma: crate::output_space::resize_gamma(&opts.output),
            overlay: watermark.as_ref().map(crate::watermark::Prepared::overlay),
        };
        if opts.format == EncodedFormat::OpenExr || opts.format == EncodedFormat::HdrPng {
            crate::hdr::save_encoded(&opts.path, &vopts, &fill, opts, cancel).map_err(|e| {
                if cancel.is_cancelled() {
                    JobError::Cancelled
                } else {
                    JobError::Failed(e)
                }
            })?;
            return Ok((out.w, out.h));
        }
        encode::save_streamed(&opts.path, &vopts, &fill).map_err(|e| {
            if cancel.is_cancelled() {
                JobError::Cancelled
            } else {
                JobError::Failed(format!("export: {e}"))
            }
        })?;
        Ok((out.w, out.h))
    }

    /// DNG export (`crate::dng`): the raw's sensor data, a preview rendered with `params`,
    /// written atomically. Returns the oriented size of the sensor data.
    pub fn export_dng(
        &self,
        owner: u64,
        src_ref: &SourceRef,
        params: &EditParams,
        opts: &ExportOptions,
        cancel: &CancelToken,
    ) -> JobResult<(u32, u32)> {
        let SourceRef::File(path) = src_ref else {
            return Err(JobError::Failed("DNG export needs a raw file".into()));
        };
        if !source::is_raw_source(src_ref) {
            return Err(JobError::Failed(format!(
                "{}: DNG export needs a raw file",
                path.display()
            )));
        }
        let preview = {
            let source = self
                .source_for(src_ref, params, Some(cancel))
                .map_err(load_err)?;
            self.render_full(
                owner,
                &source,
                params,
                Some(1024),
                &DisplayProfileId::Srgb,
                Some(cancel),
            )?
        };
        let mut reservation = None;
        let mut admit = |peak: u64| -> Result<(), LoadError> {
            reservation = Some(
                self.ledger
                    .admit(peak, Some(cancel))
                    .ok_or(LoadError::Cancelled)?,
            );
            Ok(())
        };
        let m = source::worker::mosaic(path, &mut admit, Some(cancel)).map_err(load_err)?;
        let o = aigle_core::geometry::Orientation::from_exif(m.orientation);
        let preview = sensor_preview(&preview, o);
        if cancel.is_cancelled() {
            return Err(JobError::Cancelled);
        }
        let bytes = crate::dng::build(&m, Some(&preview), opts);
        drop(reservation);
        encode::write_atomic(&opts.path, &bytes).map_err(JobError::Failed)?;
        let size = o.apply_size(Size::new(m.width, m.height));
        Ok((size.w, size.h))
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

/// A rendered BGRA image turned back into sensor orientation (undoing `o`), as RGB.
fn sensor_preview(r: &Rendered, o: aigle_core::geometry::Orientation) -> crate::dng::Preview {
    let from = Size::new(r.width, r.height);
    let inv = o.inverse();
    let to = inv.apply_size(from);
    let mut rgb = vec![0u8; to.area() as usize * 3];
    for y in 0..r.height {
        for x in 0..r.width {
            let (sx, sy) = inv.apply_pixel(x, y, from);
            let s = (y * r.width + x) as usize * 4;
            let d = (sy * to.w + sx) as usize * 3;
            rgb[d..d + 3].copy_from_slice(&[r.data[s + 2], r.data[s + 1], r.data[s]]);
        }
    }
    crate::dng::Preview {
        width: to.w,
        height: to.h,
        rgb,
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
                let (w, h) = if opts.format == aigle_engine_api::EncodedFormat::Dng {
                    engine.export_dng(owner, &src, &params, &opts, cancel)?
                } else {
                    let source = engine
                        .source_for(&src, &params, Some(cancel))
                        .map_err(load_err)?;
                    engine.export_streamed(owner, &src, &source, &params, &opts, cancel)?
                };
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

#[cfg(test)]
mod hdr_pipeline_tests {
    use super::*;
    #[test]
    fn hdr_png_matches_native_preview_luminance_and_colour() {
        use aigle_engine_api::BackendPreference;
        let dir = std::env::temp_dir().join(format!("aigle-hdr-match-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let input = dir.join("patches.exr");
        let colours = [
            [0.02f32; 3],
            [0.18; 3],
            [1.0; 3],
            [2.0; 3],
            [4.0; 3],
            [16.0; 3],
            [4.0, 0.3, 0.1],
            [0.1, 4.0, 0.3],
            [0.3, 0.1, 4.0],
        ];
        exr::prelude::write_rgb_file(&input, 144, 16, |x, _| {
            let v = colours[x / 16];
            (v[0], v[1], v[2])
        })
        .unwrap();
        let e = EngineInner::new(EngineConfig {
            backend: BackendPreference::Cpu,
            ..Default::default()
        });
        let source = SourceRef::File(input);
        let mut p = EditParams::default();
        p.hdr.diffuse_white = 120.0;
        p.hdr.peak = 600.0;
        p.hdr.shoulder = 40.0;
        p.detail.sharpen.amount = 0.0;
        p.detail.noise.chroma = 0.0;
        let src = e.source_for(&source, &p, None).unwrap();
        let preview = e
            .render_full(
                e.next_id(),
                &src,
                &p,
                Some(144),
                &DisplayProfileId::HdrRec2020,
                None,
            )
            .unwrap();
        let output = dir.join("hdr.png");
        let opts = ExportOptions {
            path: output.clone(),
            format: EncodedFormat::HdrPng,
            output: DisplayProfileId::HdrRec2020,
            ..Default::default()
        };
        e.export_streamed(e.next_id(), &source, &src, &p, &opts, &CancelToken::new())
            .unwrap();
        let mut png = png::Decoder::new(std::io::BufReader::new(
            std::fs::File::open(output).unwrap(),
        ))
        .read_info()
        .unwrap();
        assert_eq!(
            png.info().coding_independent_code_points,
            Some(png::CodingIndependentCodePoints {
                color_primaries: 9,
                transfer_function: 16,
                matrix_coefficients: 0,
                is_video_full_range_image: true,
            })
        );
        let mut bytes = vec![0; png.output_buffer_size().unwrap()];
        let info = png.next_frame(&mut bytes).unwrap();
        assert_eq!(info.bit_depth, png::BitDepth::Sixteen);
        for patch in 0..colours.len() {
            let i = 8 * 144 + patch * 16 + 8;
            for c in 0..3 {
                let at = i * 8 + c * 2;
                let linear = half::f16::from_bits(u16::from_le_bytes([
                    preview.data[at],
                    preview.data[at + 1],
                ]))
                .to_f32();
                let at = i * 6 + c * 2;
                let nits = crate::hdr::pq_decode(
                    u16::from_be_bytes([bytes[at], bytes[at + 1]]) as f32 / 65535.0,
                );
                let expected = linear * crate::hdr::REFERENCE_WHITE;
                assert!(
                    (nits - expected).abs() < expected.max(1.0) * 0.003,
                    "patch {patch} channel {c}: export {nits} nits, preview {expected} nits"
                );
            }
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn hdr_preview_and_all_exports_keep_highlights_through_resize_and_watermark() {
        use aigle_engine_api::{
            Anchor, BackendPreference, MetadataPolicy, OutputSharpening, Watermark,
            WatermarkContent,
        };
        let dir = std::env::temp_dir().join(format!("aigle-hdr-pipeline-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let input = dir.join("input.exr");
        exr::prelude::write_rgb_file(&input, 32, 16, |x, _| {
            let v = if x < 16 { 0.18f32 } else { 4.0 };
            (v, v, v)
        })
        .unwrap();
        let e = EngineInner::new(EngineConfig {
            backend: BackendPreference::Cpu,
            ..Default::default()
        });
        let source = SourceRef::File(input);
        let mut p = EditParams::default();
        p.detail.sharpen.amount = 0.0;
        p.detail.noise.chroma = 0.0;
        let src = e.source_for(&source, &p, None).unwrap();
        let hdr = e
            .render_full(
                e.next_id(),
                &src,
                &p,
                Some(32),
                &DisplayProfileId::HdrRec2020,
                None,
            )
            .unwrap();
        assert_eq!(hdr.format, PixelFormat::Rgba16F);
        assert_eq!(hdr.data.len(), 32 * 16 * 8);
        let at = (8 * 32 + 24) * 8;
        let v = half::f16::from_bits(u16::from_le_bytes([hdr.data[at], hdr.data[at + 1]])).to_f32();
        assert!(
            v > 2.0 && v < crate::hdr::HEADROOM,
            "preview must retain HDR: {v}"
        );
        let hist = crate::analysis::rendered_histogram(&hdr);
        assert_eq!(hist.red.iter().sum::<u32>(), 32 * 16);
        assert_eq!(hist.red[255], 0, "diffuse white is not HDR clipping");
        let sdr = e
            .render_full(
                e.next_id(),
                &src,
                &p,
                Some(32),
                &DisplayProfileId::Srgb,
                None,
            )
            .unwrap();
        assert_eq!(sdr.format, PixelFormat::Bgra8);
        for format in [
            EncodedFormat::HdrTiff,
            EncodedFormat::HdrPng,
            EncodedFormat::OpenExr,
        ] {
            let path = dir.join(format!("{format:?}.{}", format.extension()));
            let opts = ExportOptions {
                path: path.clone(),
                format,
                long_edge: Some(16),
                output: DisplayProfileId::HdrRec2020,
                sharpening: OutputSharpening::Standard,
                metadata: MetadataPolicy::CopyrightOnly,
                artist: Some("HDR test".into()),
                watermark: Some(Watermark {
                    content: WatermarkContent::Text("HDR".into()),
                    size: 0.2,
                    anchor: Anchor::BottomLeft,
                    margin: 0.0,
                    opacity: 1.0,
                    color: [255; 3],
                }),
                ..Default::default()
            };
            let size = e
                .export_streamed(e.next_id(), &source, &src, &p, &opts, &CancelToken::new())
                .unwrap();
            assert_eq!(size, (16, 8));
            let reloaded = e.source_for(&SourceRef::File(path), &p, None).unwrap();
            assert_eq!(
                (reloaded.info.source_size.w, reloaded.info.source_size.h),
                (16, 8)
            );
            let max = reloaded.pyramid.levels[0].data[4 * 16 + 12][0].to_f32();
            assert!(max > 1.5, "{format:?} clipped highlights: {max}");
        }
        let invalid = ExportOptions {
            path: dir.join("wrong.png"),
            format: EncodedFormat::Png,
            output: DisplayProfileId::HdrRec2020,
            ..Default::default()
        };
        assert!(
            e.export_streamed(
                e.next_id(),
                &source,
                &src,
                &p,
                &invalid,
                &CancelToken::new()
            )
            .is_err()
        );
        assert!(!invalid.path.exists());
        std::fs::remove_dir_all(dir).ok();
    }
}

#[cfg(test)]
mod hdr_live_tests {
    use super::*;
    use aigle_core::Revision;
    use aigle_engine_api::{
        BackendPreference, Frame, Interaction, LoadState, ParamsSource, Quality, SyntheticPattern,
        View, ViewSpec, Viewport,
    };
    use std::time::{Duration, Instant};
    fn wait(view: &View, revision: Option<Revision>) -> Arc<Frame> {
        let mut rx = view.frames();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(frame) = rx.borrow_and_update().clone()
                && frame.quality != Quality::Draft
                && revision.is_none_or(|r| frame.rev == r)
            {
                return frame;
            }
            assert!(Instant::now() < deadline, "HDR live frame timed out");
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    #[test]
    fn live_hdr_and_sdr_views_follow_exposure_edits_without_format_or_cache_mixups() {
        let engine = crate::create(EngineConfig {
            backend: BackendPreference::Cpu,
            ..Default::default()
        });
        let session = engine.open(SourceRef::Synthetic {
            pattern: SyntheticPattern::Gradient,
            width: 64,
            height: 48,
        });
        let mut info = session.info();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !matches!(*info.borrow_and_update(), LoadState::Ready(_)) {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
        let spec = |display| ViewSpec {
            viewport: Some(Viewport {
                region: aigle_core::geometry::Rect::new(0.0, 0.0, 64.0, 48.0),
                output_size: (64, 48),
                display,
            }),
            fit_size: (64, 48),
            params: ParamsSource::Current,
            overlays: Default::default(),
        };
        let hdr = session.create_view(spec(DisplayProfileId::HdrRec2020));
        let sdr = session.create_view(spec(DisplayProfileId::Srgb));
        let before = wait(&hdr, None);
        assert_eq!(before.pixels.format, PixelFormat::Rgba16F);
        let mut params = EditParams::default();
        params.light.exposure = 2.0;
        params.presence.saturation = 15.0;
        session.set_edits(Revision(2), Arc::new(params), Interaction::Final);
        let after = wait(&hdr, Some(Revision(2)));
        let sdr = wait(&sdr, Some(Revision(2)));
        assert_eq!(after.pixels.format, PixelFormat::Rgba16F);
        assert_eq!(after.pixels.stride, 64 * 8);
        assert_eq!(sdr.pixels.format, PixelFormat::Bgra8);
        assert_eq!(sdr.pixels.stride, 64 * 4);
        assert_ne!(before.pixels.data, after.pixels.data);
        let max = after
            .pixels
            .data
            .as_chunks::<8>()
            .0
            .iter()
            .flat_map(|p| {
                (0..3).map(move |c| {
                    half::f16::from_bits(u16::from_le_bytes([p[c * 2], p[c * 2 + 1]])).to_f32()
                })
            })
            .fold(0.0f32, f32::max);
        assert!(
            max > 1.0 && max <= crate::hdr::HEADROOM + 0.005,
            "HDR lost after creative color: {max}"
        );
    }
}
