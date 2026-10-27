//! Compute backends (doc 04 §2). Stages describe work as batches of tile jobs; the backend
//! runs the shared `aigle-kernels` functions natively (CPU, rayon) or as rust-gpu SPIR-V (GPU).
//!
//! Selection (doc 04 §2): `--deterministic` or `BackendPreference::Cpu` → CPU. Otherwise a
//! Vulkan device is created (software Vulkan only with an explicit `Gpu` preference) and must
//! pass the startup self-test (doc 04 §8.3) against the CPU on synthetic tiles. A pass is
//! remembered per (driver, build) in the cache dir, and the test is skipped next time. A
//! failing self-test falls back to the CPU.
//!
//! A runtime GPU error re-creates the device (doc 04 §8.2), up to three times per process;
//! the failed batch runs on the CPU. After that the CPU is used for the rest of the
//! process. Both are reported through `Engine::events()`.
//!
//! On the GPU, source data ([`DeviceSrc`]), C2 fields / curve tables ([`Aux`]) and C1 tiles
//! ([`C1Data::Device`]) stay resident; only output tiles are read back. A C1 tile made on a
//! device is only consumable by that device: after a fallback or a re-creation, such tiles
//! are recomputed.

mod atlas;
mod gpu;
mod px;

pub use px::{Fields, Px, PxVec};

use std::sync::atomic::Ordering;
use std::sync::{Arc, OnceLock};

use aigle_engine_api::{BackendKind, BackendPreference, EngineEvent, broadcast};
use aigle_kernels::Rgb;
use aigle_kernels::c2::{C2Params, FIELD_CH, FieldView};
use aigle_kernels::pointwise::{C1Params, CURVE_N, D1Params};
use aigle_kernels::resample::{Rgb2D, lanczos_radius};
use aigle_kernels::tiles::{self, ATLAS_TILE, Affine, OutParams};
use half::f16;
use rayon::prelude::*;

use crate::detail::{self, SPlan};
use crate::source::Level;

pub use gpu::DeviceBuf;

/// Lazily uploaded device copy of some host data, with the [`DeviceBuf::generation`] it was
/// made for.
pub type DeviceCell = parking_lot::Mutex<Option<(u64, Option<Arc<DeviceBuf>>)>>;

fn upload_px(g: &gpu::Gpu, px: Px<'_>) -> Option<Arc<DeviceBuf>> {
    if (px.len() * px.gpu_words() * 4) as u64 > g.max_binding {
        return None;
    }
    let mut words = Vec::with_capacity(px.len() * px.gpu_words());
    px.pack(&mut words);
    match g.upload(&words) {
        Ok(b) => Some(Arc::new(b)),
        Err(e) => {
            tracing::warn!("upload failed, using per-tile uploads: {e}");
            None
        }
    }
}

/// Host pixels of a rectangle of a pyramid level, addressed in full-level coordinates so that
/// sampling applies mirror boundaries at the level edges. The whole level is the rectangle
/// `(0, 0, full_w, full_h)`.
#[derive(Clone, Copy)]
pub struct LevelView<'a> {
    pub data: Px<'a>,
    pub x0: u32,
    pub y0: u32,
    pub w: u32,
    pub h: u32,
    pub full_w: u32,
    pub full_h: u32,
}

impl<'a> LevelView<'a> {
    pub fn whole(l: &'a Level) -> LevelView<'a> {
        LevelView {
            data: Px::H(&l.data),
            x0: 0,
            y0: 0,
            w: l.width,
            h: l.height,
            full_w: l.width,
            full_h: l.height,
        }
    }

    /// A full-precision level 0 (doc 03 §9).
    pub fn whole32(l: &'a crate::source::Level32) -> LevelView<'a> {
        LevelView {
            data: Px::F(&l.data),
            x0: 0,
            y0: 0,
            w: l.width,
            h: l.height,
            full_w: l.width,
            full_h: l.height,
        }
    }
}

impl Rgb2D for LevelView<'_> {
    fn width(&self) -> u32 {
        self.full_w
    }
    fn height(&self) -> u32 {
        self.full_h
    }
    #[inline]
    fn load(&self, x: u32, y: u32) -> Rgb {
        let lx = x.saturating_sub(self.x0).min(self.w - 1);
        let ly = y.saturating_sub(self.y0).min(self.h - 1);
        self.data.get((ly * self.w + lx) as usize)
    }
}

/// The same pixels as a [`LevelView`], resident on the device.
#[derive(Clone, Copy)]
pub struct DeviceSrc<'a> {
    pub buf: &'a DeviceBuf,
    pub x0: u32,
    pub y0: u32,
    pub w: u32,
    pub h: u32,
}

/// Auxiliary `f32` data shared by a whole batch (C2 field, curve tables), uploaded once.
pub struct Aux {
    pub data: Vec<f32>,
    device: DeviceCell,
}

impl Aux {
    pub fn new(data: Vec<f32>) -> Aux {
        assert!(!data.is_empty(), "aux data must not be empty");
        Aux {
            data,
            device: Default::default(),
        }
    }
}

/// A C2 field: [`FIELD_CH`] floats per pixel, `w × h`.
#[derive(Clone, Copy)]
pub struct FieldRef<'a> {
    pub aux: &'a Aux,
    pub w: u32,
    pub h: u32,
}

/// Host pixels of a C1 tile, plus the D-tier local fields per pixel (empty when no mask
/// uses them; `aigle_kernels::tiles::local`).
#[derive(Clone, Debug, PartialEq)]
pub struct HostC1 {
    pub rgb: PxVec,
    pub d: Fields,
}

/// Pixels of a C1 tile: on the host, or resident on the device (stride 2 or 5 words; 3 or
/// 8 at full precision).
pub enum C1Data {
    Host(HostC1),
    Device(DeviceBuf),
}

impl C1Data {
    /// Bytes held (for the cache ledger).
    pub fn bytes(&self) -> u64 {
        match self {
            C1Data::Host(v) => v.rgb.bytes() + v.d.bytes(),
            C1Data::Device(b) => b.words as u64 * 4,
        }
    }
}

/// B1 sampling geometry of one C1 tile, owned (the kernel view is [`tiles::Geo`]).
#[derive(Clone, Debug, PartialEq)]
pub struct TileGeo {
    pub affine: Affine,
    /// Empty (affine) or `[cell, nodes…]` with `wn` nodes per row ([`tiles::WARP_NODE`]
    /// floats each).
    pub warp: Vec<f32>,
    pub wn: u32,
    /// `tiles::GEO_*`.
    pub flags: u32,
}

impl TileGeo {
    pub fn affine(affine: Affine, flags: u32) -> TileGeo {
        TileGeo {
            affine,
            warp: Vec::new(),
            wn: 0,
            flags,
        }
    }

    pub fn view(&self) -> tiles::Geo<'_> {
        if self.wn == 0 {
            return tiles::Geo::affine(self.affine, self.flags);
        }
        tiles::Geo {
            affine: self.affine,
            warp: &self.warp,
            wbase: 1,
            wn: self.wn,
            cell: self.warp[0],
            flags: self.flags,
        }
    }

    /// Level rectangle (x0, y0, w, h) that every sample of a `w × h` tile reads, including
    /// the filter support and mirrored samples outside an `lw × lh` level.
    pub fn roi(&self, w: u32, h: u32, lw: u32, lh: u32) -> (u32, u32, u32, u32) {
        let lanczos = self.flags & tiles::GEO_LANCZOS != 0;
        if self.wn == 0 {
            let a = &self.affine.a;
            let k = (a[0] * a[4] - a[1] * a[3]).abs().sqrt();
            let m = if lanczos { lanczos_radius(k) } else { 1 };
            return source_roi_margin(&self.affine, w, h, lw, lh, m);
        }
        // Interpolation between nodes stays in their convex hull.
        let (mut u0, mut u1, mut v0, mut v1) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
        let mut k = 1f32;
        for n in self.warp[1..].as_chunks::<{ tiles::WARP_NODE }>().0 {
            for c in 0..3 {
                u0 = u0.min(n[2 * c]);
                u1 = u1.max(n[2 * c]);
                v0 = v0.min(n[2 * c + 1]);
                v1 = v1.max(n[2 * c + 1]);
            }
            k = k.max(n[7]);
        }
        let m = if lanczos { lanczos_radius(k) } else { 1 };
        roi_from_bounds(u0, u1, v0, v1, lw, lh, m)
    }
}

/// One C1 tile: `w × h` Crop-grid pixels sampled from `src` through `geo`, then C1 and C2.
#[derive(Clone, Copy)]
pub struct C1Job<'a> {
    /// Host pixels covering at least [`TileGeo::roi`] of this job.
    pub src: LevelView<'a>,
    /// The same pixels on the device, if resident (S-tier ROIs).
    pub device_src: Option<DeviceSrc<'a>>,
    /// `src` is a whole pyramid level, identified by (A-tier key, level): on the GPU it is
    /// read through the source tile atlas.
    pub atlas: Option<(crate::cache::CacheKey, u32)>,
    pub geo: &'a TileGeo,
    pub params: C1Params,
    pub c2: C2Params,
    /// Compiled C-tier mask program (`crate::masks`), `[0.0]` when there are no masks.
    pub prog: &'a [f32],
    /// C1 colour block (`aigle_kernels::pointwise::C1_BLOCK`), `[0.0]` when there is none.
    pub color: &'a [f32],
    /// Carry D-tier local fields in the tile.
    pub fields: bool,
    pub w: u32,
    pub h: u32,
}

/// One output tile over a C1 tile whose top-left grid pixel is (`gx0`, `gy0`).
pub struct OutJob<'a> {
    pub c1: &'a C1Data,
    /// The C1 tile carries D-tier local fields.
    pub fields: bool,
    /// Full precision: f32 C1 tile in, RGBA16 out (doc 03 §9).
    pub full: bool,
    pub params: OutParams,
    pub w: u32,
    pub h: u32,
    pub gx0: u32,
    pub gy0: u32,
}

/// One S-tier block of `level` (`crate::detail`).
#[derive(Clone, Copy)]
pub struct SJob<'a> {
    pub plan: &'a SPlan,
    /// The whole level the block reads.
    pub src: LevelView<'a>,
}

/// Error from a batch whose inputs live on a device that is no longer used: the caller must
/// recompute them on the CPU.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StaleDeviceData;

pub struct Backend {
    gpu: parking_lot::RwLock<Option<Arc<gpu::Gpu>>>,
    events: Option<broadcast::Sender<EngineEvent>>,
    /// For re-creating a lost device (doc 04 §8.2).
    recovery: Option<Recovery>,
    pub description: String,
}

struct Recovery {
    allow_software: bool,
    ledger: Arc<crate::ledger::MemLedger>,
    cache_dir: Option<std::path::PathBuf>,
    attempts: std::sync::atomic::AtomicU32,
}

/// Device re-creations per process after a device loss; then the CPU is used.
const MAX_RECOVERIES: u32 = 3;

const NO_FIELD: [f32; FIELD_CH] = [0.0; FIELD_CH];

/// Source tile atlas share of the engine budget on unified memory (doc 03 §4.2).
pub const ATLAS_SHARE: f64 = 0.2;
/// Source tile atlas share of the device budget on a discrete GPU.
pub const ATLAS_DEVICE_SHARE: f64 = 0.4;

/// Auxiliary data as bound for one batch.
enum AuxIn<'a> {
    Device(Arc<DeviceBuf>),
    Host(&'a [u32]),
}

impl AuxIn<'_> {
    fn input(&self) -> gpu::Input<'_> {
        match self {
            AuxIn::Device(b) => gpu::Input::Device(b),
            AuxIn::Host(h) => gpu::Input::Host(h),
        }
    }
}

/// Device copy of host data for the device `g`, uploaded on first use per device.
fn resident(
    cell: &DeviceCell,
    g: &gpu::Gpu,
    upload: impl FnOnce() -> Option<Arc<DeviceBuf>>,
) -> Option<Arc<DeviceBuf>> {
    let mut c = cell.lock();
    match &*c {
        Some((made_for, b)) if *made_for == g.generation => b.clone(),
        _ => {
            let b = upload();
            *c = Some((g.generation, b.clone()));
            b
        }
    }
}

/// Run GPU work `f` on `g`. wgpu panics in some calls on a lost device, so panics become
/// errors here, as does a device already reported lost.
fn on_gpu<T>(g: &gpu::Gpu, f: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    if g.is_lost() {
        return Err("device lost".into());
    }
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(r) => r,
        Err(p) => Err(format!(
            "GPU call panicked: {}",
            p.downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| p.downcast_ref::<&str>().copied())
                .unwrap_or("?")
        )),
    }
}

fn aux_input<'a>(g: &gpu::Gpu, aux: &'a Aux) -> AuxIn<'a> {
    let dev = resident(&aux.device, g, || {
        g.upload(bytemuck::cast_slice(&aux.data)).ok().map(Arc::new)
    });
    match dev {
        Some(b) => AuxIn::Device(b),
        None => AuxIn::Host(bytemuck::cast_slice(&aux.data)),
    }
}

/// Create a device and prepare it: memory configuration and the self-test, unless a
/// previous run passed it on the same driver with the same build (doc 04 §8.3).
fn open_gpu(
    allow_software: bool,
    ledger: &Arc<crate::ledger::MemLedger>,
    cache_dir: Option<&std::path::Path>,
) -> Result<gpu::Gpu, String> {
    let g = gpu::Gpu::new(allow_software)?;
    let atlas = match (g.unified, g.device_budget) {
        (false, Some(d)) => {
            ledger.set_device_budget(d);
            (d as f64 * ATLAS_DEVICE_SHARE) as u64
        }
        _ => (ledger.budget() as f64 * ATLAS_SHARE) as u64,
    };
    g.configure_memory(ledger.clone(), atlas);
    let cache = cache_dir.map(|d| SelfTestCache::new(d, &g.driver_id));
    if cache.as_ref().is_some_and(SelfTestCache::passed) {
        tracing::info!("GPU self-test passed before on this driver and build; skipped");
        return Ok(g);
    }
    let t0 = std::time::Instant::now();
    self_test(&g).map_err(|e| format!("self-test: {e}"))?;
    tracing::info!(elapsed = ?t0.elapsed(), "GPU self-test passed");
    if let Some(c) = cache {
        c.record();
    }
    Ok(g)
}

/// Passed self-tests by (driver, build), one key per line in `gpu-selftest` (doc 04 §8.3).
/// The build is identified by the crate version, the SPIR-V module and the executable's
/// size and mtime, so a rebuilt engine is tested again.
struct SelfTestCache {
    path: std::path::PathBuf,
    key: String,
}

impl SelfTestCache {
    fn new(dir: &std::path::Path, driver: &str) -> SelfTestCache {
        let exe = std::env::current_exe()
            .and_then(std::fs::metadata)
            .map(|m| {
                let t = m
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map_or(0, |d| d.as_nanos());
                format!("{}:{t}", m.len())
            })
            .unwrap_or_default();
        let key = crate::cache::KeyBuilder::new("gpu-selftest")
            .str(driver)
            .str(env!("CARGO_PKG_VERSION"))
            .bytes(gpu::SPIRV)
            .str(&exe)
            .finish()
            .hex();
        SelfTestCache {
            path: dir.join("gpu-selftest"),
            key,
        }
    }

    fn passed(&self) -> bool {
        std::fs::read_to_string(&self.path).is_ok_and(|s| s.lines().any(|l| l == self.key))
    }

    fn record(&self) {
        let mut lines: Vec<String> = std::fs::read_to_string(&self.path)
            .map(|s| s.lines().map(str::to_owned).collect())
            .unwrap_or_default();
        lines.push(self.key.clone());
        let keep = lines.len().saturating_sub(16);
        let text = lines[keep..].join("\n") + "\n";
        let res = std::fs::create_dir_all(self.path.parent().expect("in a dir"))
            .map_err(|e| e.to_string())
            .and_then(|_| crate::encode::write_atomic(&self.path, text.as_bytes()));
        if let Err(e) = res {
            tracing::debug!("self-test cache not written: {e}");
        }
    }
}

impl Backend {
    pub fn cpu(reason: &str) -> Backend {
        Backend {
            gpu: parking_lot::RwLock::new(None),
            events: None,
            recovery: None,
            description: format!("CPU ({reason})"),
        }
    }

    /// Select a backend per doc 04 §2. Returns the backend and, on a GPU fallback, why.
    /// Device memory (doc 03 §2.1, §4.2): on an integrated GPU device buffers count against
    /// `ledger`'s host budget and the atlas is [`ATLAS_SHARE`] of it; on a discrete GPU the
    /// ledger gets a device pool of the driver's budget and the atlas is
    /// [`ATLAS_DEVICE_SHARE`] of that.
    pub fn select(
        pref: BackendPreference,
        deterministic: bool,
        events: broadcast::Sender<EngineEvent>,
        ledger: &Arc<crate::ledger::MemLedger>,
        cache_dir: Option<&std::path::Path>,
    ) -> (Backend, Option<String>) {
        if deterministic {
            return (Backend::cpu("--deterministic"), None);
        }
        if pref == BackendPreference::Cpu {
            return (Backend::cpu("requested"), None);
        }
        let allow_software = pref == BackendPreference::Gpu;
        let g = match open_gpu(allow_software, ledger, cache_dir) {
            Ok(g) => g,
            Err(e) if e.starts_with("self-test: ") => {
                return (Backend::cpu("GPU failed self-test"), Some(e));
            }
            Err(e) => return (Backend::cpu("no usable GPU"), Some(e)),
        };
        let b = Backend {
            description: format!("GPU: {}", g.adapter),
            gpu: parking_lot::RwLock::new(Some(Arc::new(g))),
            events: Some(events),
            recovery: Some(Recovery {
                allow_software,
                ledger: ledger.clone(),
                cache_dir: cache_dir.map(Into::into),
                attempts: Default::default(),
            }),
        };
        (b, None)
    }

    pub fn kind(&self) -> BackendKind {
        if self.gpu_active().is_some() {
            BackendKind::Gpu
        } else {
            BackendKind::Cpu
        }
    }

    fn gpu_active(&self) -> Option<Arc<gpu::Gpu>> {
        self.gpu.read().clone()
    }

    /// A GPU call on `g` failed. The device is re-created (doc 04 §8.2: device loss, driver
    /// reset) up to [`MAX_RECOVERIES`] times per process; after that, or if re-creation fails,
    /// the engine uses the CPU for the rest of the process. Reported through the engine events.
    fn gpu_failed(&self, g: &gpu::Gpu, why: &str) {
        let mut cur = self.gpu.write();
        if cur.as_ref().is_none_or(|c| c.generation != g.generation) {
            return; // already handled by another caller
        }
        let send = |ev| {
            if let Some(tx) = &self.events {
                let _ = tx.send(ev);
            }
        };
        if let Some(r) = &self.recovery
            && r.attempts.fetch_add(1, Ordering::AcqRel) < MAX_RECOVERIES
        {
            tracing::warn!(
                lost = g.is_lost(),
                "GPU call failed ({why}); re-creating the device"
            );
            match open_gpu(r.allow_software, &r.ledger, r.cache_dir.as_deref()) {
                Ok(new) => {
                    *cur = Some(Arc::new(new));
                    send(EngineEvent::DeviceLost { recovered: true });
                    return;
                }
                Err(e) => tracing::error!("re-creating the GPU device failed: {e}"),
            }
        }
        *cur = None;
        tracing::error!("GPU backend disabled, falling back to CPU: {why}");
        send(EngineEvent::DeviceLost { recovered: false });
        send(EngineEvent::BackendSelected {
            kind: BackendKind::Cpu,
            reason: format!("GPU failed at runtime: {why}"),
        });
    }

    /// Device copy of `px`, uploaded on first use (per device) and remembered in `cell`.
    pub fn resident_px(&self, cell: &DeviceCell, px: Px<'_>) -> Option<Arc<DeviceBuf>> {
        let g = self.gpu_active()?;
        on_gpu(&g, || Ok(resident(cell, &g, || upload_px(&g, px))))
            .ok()
            .flatten()
    }

    /// Lose the current device (tests of recovery).
    #[cfg(test)]
    fn simulate_loss(&self) {
        if let Some(g) = self.gpu_active() {
            g.simulate_loss();
        }
    }

    /// Run a batch of C1 tiles (one GPU submission, or rayon over tiles on the CPU).
    pub fn c1_tiles(&self, jobs: &[C1Job<'_>], field: Option<FieldRef<'_>>) -> Vec<C1Data> {
        if jobs.is_empty() {
            return Vec::new();
        }
        if let Some(g) = self.gpu_active() {
            let res = on_gpu(&g, || {
                let aux = field.map(|f| aux_input(&g, f.aux));
                c1_gpu(&g, jobs, field, aux.as_ref().map(AuxIn::input), false)
            });
            match res {
                Ok(t) => return t,
                Err(e) => self.gpu_failed(&g, &e),
            }
        }
        jobs.par_iter()
            .map(|j| C1Data::Host(c1_cpu(j, field)))
            .collect()
    }

    /// Run a batch of output tiles. Fails with [`StaleDeviceData`] if an input is resident on a
    /// device that is no longer used (the GPU was abandoned or re-created).
    pub fn out_tiles(
        &self,
        jobs: &[OutJob<'_>],
        lut: &Aux,
    ) -> Result<Vec<Vec<u8>>, StaleDeviceData> {
        if jobs.is_empty() {
            return Ok(Vec::new());
        }
        let g = self.gpu_active();
        let current = g.as_ref().map(|g| g.generation);
        if jobs
            .iter()
            .any(|j| matches!(j.c1, C1Data::Device(b) if Some(b.generation) != current))
        {
            return Err(StaleDeviceData);
        }
        if let Some(g) = g {
            let res = on_gpu(&g, || out_gpu(&g, jobs, aux_input(&g, lut).input()));
            match res {
                Ok(t) => return Ok(t),
                Err(e) => self.gpu_failed(&g, &e),
            }
        }
        jobs.par_iter()
            .map(|j| match j.c1 {
                C1Data::Host(px) => Ok(out_cpu(j, px, &lut.data)),
                C1Data::Device(_) => Err(StaleDeviceData),
            })
            .collect()
    }

    /// Run a batch of S-tier blocks. Blocks always return to the host: they are cached and
    /// assembled into source ROIs.
    ///
    /// With a GPU, the CPU and the GPU work on the batch at the same time, taking blocks from
    /// one queue (the GPU in chunks, to amortise its submissions), so the split follows their
    /// actual throughput: on an integrated GPU the CPU is often as fast for this tier. Results
    /// of both are within the GPU tolerance of each other (doc 02 §14).
    pub fn s_blocks(&self, jobs: &[SJob<'_>]) -> Vec<PxVec> {
        use std::sync::atomic::AtomicUsize;
        let cpu =
            |j: &SJob<'_>| PxVec::from_rgb(detail::run_cpu(j.plan, &j.src), j.src.data.is_f32());
        let gpu = self.gpu_active().filter(|g| {
            jobs.iter()
                .all(|j| j.plan.work_len() as u64 * 4 <= g.max_binding)
        });
        let Some(g) = gpu.clone().filter(|_| jobs.len() > 1) else {
            if let Some(g) = gpu {
                match on_gpu(&g, || s_gpu(&g, jobs)) {
                    Ok(v) => return v,
                    Err(e) => self.gpu_failed(&g, &e),
                }
            }
            return jobs.par_iter().map(cpu).collect();
        };
        let next = AtomicUsize::new(0);
        let slots: Vec<OnceLock<PxVec>> = jobs.iter().map(|_| OnceLock::new()).collect();
        let chunk = jobs.len().div_ceil(8).clamp(1, 16);
        let ctx = crate::arbiter::current();
        let failed = std::thread::scope(|sc| {
            let gpu_side = sc.spawn(|| {
                crate::arbiter::with_task(ctx, || {
                    loop {
                        let a = next.fetch_add(chunk, Ordering::AcqRel);
                        if a >= jobs.len() {
                            return None;
                        }
                        let b = (a + chunk).min(jobs.len());
                        match on_gpu(&g, || s_gpu(&g, &jobs[a..b])) {
                            Ok(v) => {
                                for (s, d) in slots[a..b].iter().zip(v) {
                                    let _ = s.set(d);
                                }
                            }
                            Err(e) => return Some(e),
                        }
                    }
                })
            });
            // One claimant per rayon thread; blocks are also internally parallel.
            (0..rayon::current_num_threads())
                .into_par_iter()
                .for_each(|_| {
                    loop {
                        let i = next.fetch_add(1, Ordering::AcqRel);
                        if i >= jobs.len() {
                            break;
                        }
                        let _ = slots[i].set(cpu(&jobs[i]));
                    }
                });
            gpu_side.join().expect("GPU S thread panicked")
        });
        if let Some(e) = failed {
            self.gpu_failed(&g, &e);
        }
        // Blocks claimed by a failed GPU submission are recomputed on the CPU.
        slots
            .into_iter()
            .zip(jobs)
            .map(|(s, j)| s.into_inner().unwrap_or_else(|| cpu(j)))
            .collect()
    }
}

/// Startup self-test (doc 04 §8.3): every kernel on synthetic tiles, GPU vs CPU, within the
/// GPU tolerance (doc 02 §14), with all C2 and D2 features enabled. Covers the resident and
/// the ROI upload paths and host/device auxiliary data.
fn self_test(g: &gpu::Gpu) -> Result<(), String> {
    {
        let (level, (affine, params)) = self_test_inputs();
        let (fw, fh) = (9u32, 7u32);
        let field_data: Vec<f32> = (0..fw * fh * FIELD_CH as u32)
            .map(|i| match i % 5 {
                0 => 0.6,
                1 => -1.5 + (i % 7) as f32 * 0.1,
                2 => 0.8,
                3 => -0.5,
                _ => 0.4 + (i % 3) as f32 * 0.1,
            })
            .collect();
        let field_aux = Aux::new(field_data);
        let field = FieldRef {
            aux: &field_aux,
            w: fw,
            h: fh,
        };
        let mut c2 = C2Params::new(40.0, 50.0, -40.0, 60.0);
        c2.airlight = [0.9, 0.95, 1.0];
        c2.fscale = 0.13;
        let mut words = Vec::new();
        Px::H(&level.data).pack(&mut words);
        let resident = g.upload(&words)?;
        let view = LevelView::whole(&level);
        let level32 = crate::source::Level32 {
            width: level.width,
            height: level.height,
            data: (0..level.data.len())
                .map(|i| Px::H(&level.data).get(i))
                .collect(),
        };
        let prog = self_test_masks(&level, crate::masks::Tier::C);
        let color_block = self_test_color();
        let geos = [TileGeo::affine(affine, 0), self_test_warp(&affine, 37, 29)];
        let mut cpu0 = None;
        for (gi, geo) in geos.iter().enumerate() {
            let cpu_job = C1Job {
                src: view,
                device_src: None,
                atlas: None,
                geo,
                params,
                c2,
                prog: &prog,
                color: if gi == 0 { &[0.0] } else { &color_block },
                fields: true,
                w: 37,
                h: 29,
            };
            let cpu = c1_cpu(&cpu_job, Some(field));
            let res_job = C1Job {
                device_src: Some(DeviceSrc {
                    buf: &resident,
                    x0: 0,
                    y0: 0,
                    w: level.width,
                    h: level.height,
                }),
                ..cpu_job
            };
            let atlas_job = C1Job {
                atlas: Some((crate::cache::KeyBuilder::new("self-test").finish(), 0)),
                ..cpu_job
            };
            let full_job = C1Job {
                src: LevelView::whole32(&level32),
                ..cpu_job
            };
            let cpu_full = c1_cpu(&full_job, Some(field));
            let field_dev = aux_input(g, &field_aux);
            for (name, job, aux, cpu) in [
                ("resident", res_job, field_dev.input(), &cpu),
                ("atlas", atlas_job, field_dev.input(), &cpu),
                (
                    "roi",
                    cpu_job,
                    gpu::Input::Host(bytemuck::cast_slice(&field_aux.data)),
                    &cpu,
                ),
                ("full precision", full_job, field_dev.input(), &cpu_full),
            ] {
                let gpu = match c1_gpu(g, std::slice::from_ref(&job), Some(field), Some(aux), true)?
                    .remove(0)
                {
                    C1Data::Host(v) => v,
                    C1Data::Device(_) => unreachable!("readback requested"),
                };
                if gpu.rgb.as_px().is_f32() != job.src.data.is_f32() {
                    return Err(format!("c1_tile ({name}) changed the precision"));
                }
                let mut worst = 0f32;
                for i in 0..cpu.rgb.len().min(gpu.rgb.len()) {
                    let (a, b) = (cpu.rgb.as_px().get(i), gpu.rgb.as_px().get(i));
                    for c in 0..3 {
                        worst = worst.max((a[c] - b[c]).abs() / a[c].abs().max(1e-2));
                    }
                }
                for i in 0..cpu.d.len().min(gpu.d.len()) {
                    let (a, b) = (cpu.d.get(i), gpu.d.get(i));
                    for c in 0..5 {
                        worst = worst.max((a[c] - b[c]).abs() / a[c].abs().max(1.0));
                    }
                }
                if gpu.d.len() != cpu.d.len() {
                    return Err(format!("c1_tile ({name}) lost the local fields"));
                }
                // f16 storage: one f16 ULP is ~1e-3 relative; allow two for rounding-mode and
                // transcendental differences.
                if worst > 2e-3 {
                    let kind = if gi == 0 { "affine" } else { "warp" };
                    return Err(format!(
                        "c1_tile ({name}, {kind}) differs from the CPU reference (worst relative error {worst:.2e})"
                    ));
                }
            }
            if gi == 0 {
                cpu0 = Some((cpu, res_job, cpu_full));
            }
        }
        self_test_s(g, &level)?;
        let (cpu, res_job, cpu_full) = cpu0.expect("first geometry");
        let mut out_params = self_test_out_params();
        let curves: Vec<f32> = (0..3 * CURVE_N)
            .map(|i| {
                ((i % CURVE_N) as f32 / (CURVE_N - 1) as f32).powf(1.0 + (i / CURVE_N) as f32 * 0.1)
            })
            .collect();
        let mut aux = crate::pipeline::output_aux(Some(&curves));
        out_params.d2.look = aux.len() as u32;
        aux.extend_from_slice(&self_test_look());
        let lut = Aux::new(aux);
        let host = C1Data::Host(cpu.clone());
        let device = c1_gpu(
            g,
            std::slice::from_ref(&res_job),
            Some(field),
            Some(aux_input(g, &field_aux).input()),
            false,
        )?
        .remove(0);
        let a = out_cpu(
            &OutJob {
                c1: &host,
                fields: true,
                full: false,
                params: out_params,
                w: 37,
                h: 29,
                gx0: 1000,
                gy0: 77,
            },
            &cpu,
            &lut.data,
        );
        let host_full = C1Data::Host(cpu_full.clone());
        let a_full = out_cpu(
            &OutJob {
                c1: &host_full,
                fields: true,
                full: true,
                params: out_params,
                w: 37,
                h: 29,
                gx0: 1000,
                gy0: 77,
            },
            &cpu_full,
            &lut.data,
        );
        for (name, c1, full) in [
            ("host input", &host, false),
            ("device input", &device, false),
            ("full precision", &host_full, true),
        ] {
            let job = OutJob {
                c1,
                fields: true,
                full,
                params: out_params,
                w: 37,
                h: 29,
                gx0: 1000,
                gy0: 77,
            };
            let b = out_gpu(g, std::slice::from_ref(&job), aux_input(g, &lut).input())?.remove(0);
            // 8-bit: ≤ 1 code value; 16-bit: ≤ a quarter of an 8-bit code value.
            let (a, tol) = if full {
                (
                    a_full
                        .as_chunks::<2>()
                        .0
                        .iter()
                        .map(|c| u16::from_le_bytes(*c) as u32)
                        .collect::<Vec<_>>(),
                    64,
                )
            } else {
                (a.iter().map(|&v| v as u32).collect(), 1)
            };
            let b: Vec<u32> = if full {
                b.as_chunks::<2>()
                    .0
                    .iter()
                    .map(|c| u16::from_le_bytes(*c) as u32)
                    .collect()
            } else {
                b.iter().map(|&v| v as u32).collect()
            };
            if a.len() != b.len() {
                return Err(format!("out_tile ({name}) has the wrong size"));
            }
            let max = a
                .iter()
                .zip(&b)
                .map(|(x, y)| x.abs_diff(*y))
                .max()
                .unwrap_or(0);
            if max > tol {
                return Err(format!(
                    "out_tile ({name}) differs from the CPU reference by {max} code values"
                ));
            }
        }
        Ok(())
    }
}

/// Every S-tier pass, local fields included, on a block whose margin crosses the level
/// edges (mirror boundary).
fn self_test_s(g: &gpu::Gpu, level: &Level) -> Result<(), String> {
    let sp = detail::SParams {
        luma: 60.0,
        luma_detail: 40.0,
        luma_contrast: 20.0,
        chroma: 40.0,
        chroma_detail: 50.0,
        sharpen: 80.0,
        radius: 1.0,
        sharpen_detail: 50.0,
        masking: 20.0,
        texture: 30.0,
        local: [20.0, 30.0, 20.0],
    };
    let noise = crate::noise::NoiseLevel {
        sigma: 0.01,
        offset: 1e-3,
    };
    let prog = self_test_masks(level, crate::masks::Tier::S);
    let c1 = crate::pipeline::color::C1Color {
        params: self_test_inputs().1.1,
        block: self_test_color(),
        dcp_ok: true,
    };
    let plan = detail::plan(
        level.width,
        level.height,
        0,
        &sp,
        noise,
        16,
        8,
        32,
        24,
        Some((&prog, &c1)),
    );
    let level32 = crate::source::Level32 {
        width: level.width,
        height: level.height,
        data: (0..level.data.len())
            .map(|i| Px::H(&level.data).get(i))
            .collect(),
    };
    for src in [LevelView::whole(level), LevelView::whole32(&level32)] {
        let cpu = detail::run_cpu(&plan, &src);
        let gpu = s_gpu(g, &[SJob { plan: &plan, src }])?.remove(0);
        if gpu.as_px().is_f32() != src.data.is_f32() || gpu.len() != cpu.len() {
            return Err("s_pass returned the wrong precision or size".into());
        }
        let mut worst = 0f32;
        for (i, a) in cpu.iter().enumerate() {
            let b = gpu.as_px().get(i);
            for c in 0..3 {
                worst = worst.max((a[c] - b[c]).abs() / a[c].abs().max(1e-2));
            }
        }
        if worst > 2e-3 {
            return Err(format!(
                "s_pass differs from the CPU reference (worst relative error {worst:.2e})"
            ));
        }
    }
    Ok(())
}

fn self_test_out_params() -> OutParams {
    let mut d2 = crate::pipeline::color::d2_params(&aigle_core::EditParams::default(), false);
    d2.curve = 1;
    d2.color = 1;
    d2.mixer_on = 1;
    d2.saturation = 1.3;
    d2.vibrance = 0.4;
    d2.mixer = [
        [10.0, 0.2, 0.1],
        [-5.0, -0.3, 0.0],
        [0.0, 0.5, -0.2],
        [20.0, 0.0, 0.3],
        [0.0, -0.5, 0.0],
        [-15.0, 0.4, 0.2],
        [5.0, 0.0, -0.1],
        [0.0, 0.1, 0.0],
    ];
    d2.wheels = [
        [0.01, -0.02, 0.01],
        [0.0, 0.01, 0.0],
        [-0.01, 0.02, -0.02],
        [0.005, 0.0, 0.0],
    ];
    OutParams {
        d1: D1Params::new(25.0, -10.0, 5.0),
        d2,
        e1: crate::output_space::e1_params(&Default::default()),
        d3: aigle_kernels::d3::D3Params::new(
            -40.0, 30.0, 20.0, 50.0, 40.0, 30.0, 60.0, 1.0, 600.0, 400.0,
        ),
        d1_sliders: [25.0, -10.0, 5.0],
        seed: 0x1234_5678,
    }
}

/// A C1 colour block with a 3D (value-divided, sRGB-encoded) HueSatMap and a 2D LookTable.
fn self_test_color() -> Vec<f32> {
    use crate::profile::HsvTable;
    use aigle_kernels::pointwise::{C1_BLOCK, C1_HSM, C1_LOOK};
    let table = |dims: [u32; 3], srgb: bool, seed: f32| {
        let n = (dims[0] * dims[1] * dims[2]) as usize;
        let data = (0..n)
            .flat_map(|i| {
                let t = i as f32 * 0.37 + seed;
                [
                    8.0 * t.sin(),
                    1.0 + 0.2 * (1.3 * t).cos(),
                    1.0 + 0.1 * (0.7 * t).sin(),
                ]
            })
            .collect();
        HsvTable { dims, srgb, data }
    };
    let mut b = vec![0.0; C1_BLOCK];
    b[0] = (C1_HSM | C1_LOOK) as f32;
    b[1] = 1.3;
    let m2 = [[1.1, -0.05, -0.05], [-0.02, 1.0, 0.02], [0.0, -0.1, 1.1]];
    for (i, v) in m2.iter().flatten().enumerate() {
        b[2 + i] = *v;
    }
    b[11] = b.len() as f32;
    table([18, 7, 5], true, 0.0).pack(&mut b);
    b[12] = b.len() as f32;
    table([36, 8, 1], false, 1.0).pack(&mut b);
    b
}

/// A 9³ look cube that is far from the identity.
fn self_test_look() -> Vec<f32> {
    let n = 9u32;
    let mut lut = crate::look::CubeLut {
        title: String::new(),
        n,
        domain_min: [0.0; 3],
        domain_max: [1.0; 3],
        data: Vec::new(),
    };
    for b in 0..n {
        for g in 0..n {
            for r in 0..n {
                let (r, g, b) = (r as f32 / 8.0, g as f32 / 8.0, b as f32 / 8.0);
                lut.data.extend_from_slice(&[
                    (r * 0.9 + 0.1 * g * g).min(1.0),
                    g.powf(1.2),
                    (0.8 * b + 0.15 * r).min(1.0),
                ]);
            }
        }
    }
    let mut v = Vec::new();
    lut.pack(&mut v);
    v
}

/// A program exercising every mask op and every local parameter of `tier`.
fn self_test_masks(level: &Level, tier: crate::masks::Tier) -> Vec<f32> {
    use aigle_core::masks::*;
    let mut p = aigle_core::EditParams::default();
    let pt = |x, y| Pt { x, y };
    let all = LocalParams {
        exposure: Some(0.5),
        temperature: Some(30.0),
        tint: Some(-20.0),
        contrast: Some(20.0),
        highlights: Some(-30.0),
        shadows: Some(40.0),
        whites: Some(10.0),
        blacks: Some(-10.0),
        clarity: Some(20.0),
        dehaze: Some(15.0),
        saturation: Some(-25.0),
        hue: Some(15.0),
        texture: Some(40.0),
        sharpness: Some(-30.0),
        noise: Some(25.0),
    };
    let shape = MaskShape::Combine {
        op: CombineOp::Add,
        parts: vec![
            MaskShape::Radial {
                center: pt(0.4, 0.5),
                radii: [0.3, 0.4],
                angle: 20.0,
                feather: 60.0,
            },
            MaskShape::Combine {
                op: CombineOp::Intersect,
                parts: vec![
                    MaskShape::Brush {
                        strokes: vec![Stroke {
                            points: vec![pt(0.1, 0.1), pt(0.6, 0.3), pt(0.9, 0.9)],
                            size: 0.08,
                            feather: 50.0,
                            flow: 80.0,
                            erase: false,
                        }],
                    },
                    MaskShape::Range {
                        range: RangeKind::Luminance {
                            low: 0.2,
                            high: 0.8,
                        },
                        smoothness: 40.0,
                    },
                ],
            },
        ],
    };
    p.masks.push(LocalAdjustment {
        id: aigle_core::MaskId(1),
        name: "a".into(),
        shape,
        invert: false,
        opacity: 0.8,
        adjust: all.clone(),
    });
    let shape = MaskShape::Combine {
        op: CombineOp::Subtract,
        parts: vec![
            MaskShape::Linear {
                from: pt(0.0, 0.0),
                to: pt(1.0, 0.7),
            },
            MaskShape::Range {
                range: RangeKind::Color {
                    samples: vec![[0.3, 0.2, 0.5]],
                    tolerance: 40.0,
                },
                smoothness: 30.0,
            },
        ],
    };
    p.masks.push(LocalAdjustment {
        id: aigle_core::MaskId(2),
        name: "b".into(),
        shape,
        invert: true,
        opacity: 1.0,
        adjust: all,
    });
    let map = crate::masks::LevelMap::new(
        aigle_core::geometry::Size::new(level.width, level.height),
        aigle_core::geometry::Orientation::IDENTITY,
        0,
    );
    crate::masks::compile(&p, tier, &map, None)
}

/// A warp grid over a `w × h` tile: `affine` plus barrel distortion, TCA, vignetting and a
/// footprint > 1, so the Lanczos path with per-channel positions is exercised.
fn self_test_warp(affine: &Affine, w: u32, h: u32) -> TileGeo {
    let cell = 16u32;
    let (wn, ny) = (w.div_ceil(cell) + 1, h.div_ceil(cell) + 1);
    let a = &affine.a;
    let mut warp = vec![cell as f32];
    for j in 0..ny {
        for i in 0..wn {
            let (x, y) = ((i * cell) as f32, (j * cell) as f32);
            let u = a[0] * x + a[1] * y + a[2];
            let v = a[3] * x + a[4] * y + a[5];
            let (du, dv) = (u - 32.0, v - 24.0);
            let r2 = (du * du + dv * dv) / 1600.0;
            let d = 1.0 + 0.08 * r2;
            let (gu, gv) = (32.0 + du * d, 24.0 + dv * d);
            let t = 1.0 + 0.004 * r2;
            warp.extend_from_slice(&[
                32.0 + du * d * t,
                24.0 + dv * d * t,
                gu,
                gv,
                32.0 + du * d / t,
                24.0 + dv * d / t,
                1.0 / (1.0 - 0.3 * r2.min(1.0)),
                1.7 * d,
            ]);
        }
    }
    TileGeo {
        affine: *affine,
        warp,
        wn,
        flags: tiles::GEO_LANCZOS | tiles::GEO_TCA,
    }
}

fn self_test_inputs() -> (Level, (Affine, C1Params)) {
    let (w, h) = (64u32, 48u32);
    let rgb: Vec<Rgb> = (0..w * h)
        .map(|i| {
            let (x, y) = ((i % w) as f32, (i / w) as f32);
            [
                0.01 + x / 40.0,
                0.02 + y / 30.0,
                0.5 + 0.4 * ((x * 0.37 + y * 0.11).sin()),
            ]
        })
        .collect();
    let level = Level::from_f32(w, h, &rgb);
    // Rotated, scaled and offset so that samples cross the level edges (mirror boundary).
    let (c, s) = (0.3f32.cos() * 1.7, 0.3f32.sin() * 1.7);
    let affine = Affine {
        a: [c, -s, -5.0, s, c, -3.0],
    };
    let params = C1Params {
        matrix: [[1.2, -0.1, -0.1], [-0.05, 1.1, -0.05], [0.0, -0.2, 1.2]],
    };
    (level, (affine, params))
}

// ---------------------------------------------------------------------------------------------
// CPU

fn field_view(field: Option<FieldRef<'_>>) -> FieldView<'_> {
    match field {
        Some(f) => FieldView {
            data: &f.aux.data,
            w: f.w,
            h: f.h,
        },
        None => FieldView {
            data: &NO_FIELD,
            w: 1,
            h: 1,
        },
    }
}

fn c1_cpu(job: &C1Job<'_>, field: Option<FieldRef<'_>>) -> HostC1 {
    let fv = field_view(field);
    let mut c2 = job.c2;
    if field.is_none() {
        c2.active = 0;
    }
    let n = job.w as usize * job.h as usize;
    let mut rgb = vec![[0f32; 3]; n];
    let mut d = vec![[0f32; 5]; if job.fields { n } else { 0 }];
    let w = job.w as usize;
    let geo = job.geo.view();
    // One slice for both tables, as on the GPU.
    let mut tdata = job.prog.to_vec();
    tdata.extend_from_slice(job.color);
    let tab = tiles::C1Tables {
        data: &tdata,
        prog: 0,
        color: job.prog.len(),
    };
    let fill = |y: usize, row: &mut [Rgb], drow: Option<&mut [[f32; 5]]>| {
        let mut drow = drow;
        for (x, px) in row.iter_mut().enumerate() {
            let (v, f) = tiles::c1_px(
                &job.src,
                &geo,
                x as u32,
                y as u32,
                &job.params,
                &c2,
                &fv,
                &tab,
            );
            *px = v;
            if let Some(dr) = drow.as_deref_mut() {
                dr[x] = f;
            }
        }
    };
    if job.fields {
        rgb.par_chunks_mut(w)
            .zip(d.par_chunks_mut(w))
            .enumerate()
            .for_each(|(y, (r, dr))| fill(y, r, Some(dr)));
    } else {
        rgb.par_chunks_mut(w)
            .enumerate()
            .for_each(|(y, r)| fill(y, r, None));
    }
    let full = job.src.data.is_f32();
    HostC1 {
        rgb: PxVec::from_rgb(rgb, full),
        d: Fields::from_f32(d, full),
    }
}

/// Bytes per output pixel: BGRA8, or RGBA16 at full precision.
pub fn out_bpp(full: bool) -> usize {
    if full { 8 } else { 4 }
}

fn out_cpu(job: &OutJob<'_>, c1: &HostC1, lut: &[f32]) -> Vec<u8> {
    let full = job.full;
    let bpp = out_bpp(full);
    let mut out = vec![0u8; job.w as usize * job.h as usize * bpp];
    out.par_chunks_mut(job.w as usize * bpp)
        .enumerate()
        .for_each(|(y, row)| {
            for (x, o) in row.chunks_exact_mut(bpp).enumerate() {
                let i = y * job.w as usize + x;
                let v = c1.rgb.as_px().get(i);
                let d = (!c1.d.is_empty()).then(|| c1.d.get(i));
                let (gx, gy) = (job.gx0 + x as u32, job.gy0 + y as u32);
                if full {
                    let lin = tiles::out_lin_local(v, d, gx, gy, &job.params, lut);
                    let w = tiles::encode_out16(lin, &job.params.e1);
                    o[..4].copy_from_slice(&w[0].to_le_bytes());
                    o[4..].copy_from_slice(&w[1].to_le_bytes());
                } else {
                    let packed = tiles::out_px_local(v, d, gx, gy, &job.params, lut);
                    o.copy_from_slice(&packed.to_le_bytes());
                }
            }
        });
    out
}

// ---------------------------------------------------------------------------------------------
// GPU

/// Level rectangle (x0, y0, w, h) that every bilinear sample of a `w × h` tile reads through
/// `affine`, including mirrored samples outside a `lw × lh` level.
pub fn source_roi(affine: &Affine, w: u32, h: u32, lw: u32, lh: u32) -> (u32, u32, u32, u32) {
    source_roi_margin(affine, w, h, lw, lh, 1)
}

fn source_roi_margin(
    affine: &Affine,
    w: u32,
    h: u32,
    lw: u32,
    lh: u32,
    margin: u32,
) -> (u32, u32, u32, u32) {
    let a = &affine.a;
    let corners = [
        (0.0, 0.0),
        (w as f32, 0.0),
        (0.0, h as f32),
        (w as f32, h as f32),
    ];
    let (mut u0, mut u1, mut v0, mut v1) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
    for (x, y) in corners {
        let u = a[0] * x + a[1] * y + a[2];
        let v = a[3] * x + a[4] * y + a[5];
        u0 = u0.min(u);
        u1 = u1.max(u);
        v0 = v0.min(v);
        v1 = v1.max(v);
    }
    roi_from_bounds(u0, u1, v0, v1, lw, lh, margin)
}

/// Level rectangle covering sample positions in `[u0, u1] × [v0, v1]` with filter support
/// `margin` (taps from `floor(u) - margin + 1 .. floor(u) + margin`), folding mirrored
/// taps back into the level.
fn roi_from_bounds(
    u0: f32,
    u1: f32,
    v0: f32,
    v1: f32,
    lw: u32,
    lh: u32,
    margin: u32,
) -> (u32, u32, u32, u32) {
    let m = margin as i64;
    let axis = |lo: f32, hi: f32, n: u32| -> (u32, u32) {
        let n_i = n as i64;
        if !lo.is_finite() || !hi.is_finite() {
            return (0, n);
        }
        let mut a = lo.floor() as i64 - m;
        let mut b = hi.ceil() as i64 + m + 1;
        if a < -n_i || b > 2 * n_i {
            return (0, n); // folds more than once: take the whole axis
        }
        if a < 0 {
            b = b.max(-a);
            a = 0;
        }
        if b > n_i {
            a = a.min(2 * n_i - b);
            b = n_i;
        }
        let a = a.clamp(0, n_i - 1) as u32;
        let b = (b.clamp(1, n_i) as u32).max(a + 1);
        (a, b - a)
    };
    let (x0, rw) = axis(u0, u1, lw);
    let (y0, rh) = axis(v0, v1, lh);
    (x0, y0, rw, rh)
}

fn c1_gpu(
    g: &gpu::Gpu,
    jobs: &[C1Job<'_>],
    field: Option<FieldRef<'_>>,
    aux: Option<gpu::Input<'_>>,
    readback: bool,
) -> Result<Vec<C1Data>, String> {
    struct Packed {
        fp: Vec<f32>,
        up: [u32; tiles::C1_U32],
        src: Vec<u32>,
    }
    let (fw, fh, on) = match field {
        Some(f) => (f.w, f.h, 1),
        None => (1, 1, 0),
    };
    let full = jobs.first().is_some_and(|j| j.src.data.is_f32());
    debug_assert!(jobs.iter().all(|j| j.src.data.is_f32() == full));
    let fu = full as u32;
    // Whole-level sources go through the source tile atlas (doc 03 §4.2); full-precision
    // sources upload each tile's ROI.
    let atlas = !full
        && jobs
            .iter()
            .all(|j| j.atlas.is_some() && j.device_src.is_none());
    let out_stride = |fields: bool| match (full, fields) {
        (false, false) => 2,
        (false, true) => 5,
        (true, false) => 3,
        (true, true) => 8,
    };
    let packed: Vec<Packed> = jobs
        .par_iter()
        .map(|job| {
            let mut fp = tiles::pack_c1(&job.geo.affine, &job.params, &job.c2).to_vec();
            fp.extend_from_slice(job.prog);
            let (woff, wn) = if job.geo.wn == 0 {
                (0, 0)
            } else {
                let o = fp.len() as u32;
                fp.extend_from_slice(&job.geo.warp);
                (o, job.geo.wn)
            };
            let coff = fp.len() as u32;
            fp.extend_from_slice(job.color);
            let gf = job.geo.flags;
            let df = job.fields as u32;
            let (lw, lh) = (job.src.full_w, job.src.full_h);
            let c2on = on & job.c2.active;
            if let Some(d) = job.device_src {
                return Packed {
                    fp,
                    up: [
                        job.w, job.h, d.x0, d.y0, d.w, d.h, lw, lh, fw, fh, c2on, df, gf, woff, wn,
                        coff, 0, fu,
                    ],
                    src: Vec::new(),
                };
            }
            if atlas {
                let (x0, y0, rw, rh) = job.geo.roi(job.w, job.h, lw, lh);
                let t = ATLAS_TILE;
                let (tx0, ty0) = (x0 / t, y0 / t);
                let (nx, ny) = ((x0 + rw - 1) / t - tx0 + 1, (y0 + rh - 1) / t - ty0 + 1);
                return Packed {
                    fp,
                    up: [
                        job.w, job.h, tx0, ty0, nx, ny, lw, lh, fw, fh, c2on, df, gf, woff, wn,
                        coff, 1, 0,
                    ],
                    src: Vec::new(),
                };
            }
            let v = &job.src;
            let (x0, y0, rw, rh) = job.geo.roi(job.w, job.h, lw, lh);
            debug_assert!(
                x0 >= v.x0 && y0 >= v.y0 && x0 + rw <= v.x0 + v.w && y0 + rh <= v.y0 + v.h
            );
            let mut src = Vec::with_capacity(rw as usize * rh as usize * v.data.gpu_words());
            for y in y0..y0 + rh {
                let row = ((y - v.y0) * v.w + (x0 - v.x0)) as usize;
                v.data.slice(row, row + rw as usize).pack(&mut src);
            }
            Packed {
                fp,
                up: [
                    job.w, job.h, x0, y0, rw, rh, lw, lh, fw, fh, c2on, df, gf, woff, wn, coff, 0,
                    fu,
                ],
                src,
            }
        })
        .collect();
    let out = if atlas {
        let items: Vec<gpu::AtlasDispatch<'_>> = jobs
            .iter()
            .zip(&packed)
            .map(|(job, p)| {
                let (key, level) = job.atlas.expect("atlas job");
                gpu::AtlasDispatch {
                    fp: &p.fp,
                    up: &p.up,
                    key,
                    level,
                    tiles: (p.up[2], p.up[3], p.up[4], p.up[5]),
                    out_words: job.w as usize * job.h as usize * out_stride(job.fields),
                    w: job.w,
                    h: job.h,
                }
            })
            .collect();
        let fill = |t: &atlas::TileId, words: &mut [u32]| {
            let j = jobs
                .iter()
                .find(|j| j.atlas == Some((t.0, t.1)))
                .expect("tile of a job in this batch");
            fill_atlas_tile(&j.src, t.2, t.3, words);
        };
        g.run_atlas(gpu::Kernel::C1Tile, &items, &fill, aux, readback)?
    } else {
        let items: Vec<gpu::Dispatch<'_>> = jobs
            .iter()
            .zip(&packed)
            .map(|(job, p)| gpu::Dispatch {
                fp: &p.fp,
                up: &p.up,
                input: match job.device_src {
                    Some(d) => gpu::Input::Device(d.buf),
                    None => gpu::Input::Host(&p.src),
                },
                out_words: job.w as usize * job.h as usize * out_stride(job.fields),
                w: job.w,
                h: job.h,
            })
            .collect();
        g.run(gpu::Kernel::C1Tile, &items, aux, readback)?
    };
    let fields = jobs.first().is_some_and(|j| j.fields);
    Ok(out
        .into_par_iter()
        .map(|o| match o {
            gpu::Output::Device(b) => C1Data::Device(b),
            gpu::Output::Host(words) => C1Data::Host(unpack_c1_words(&words, fields, full)),
        })
        .collect())
}

/// Pack atlas tile (`tx`, `ty`) of the level `v` (a whole level or a rectangle covering the
/// tile) as RGBA f16; pixels past the level edge stay zero (never sampled).
fn fill_atlas_tile(v: &LevelView<'_>, tx: u32, ty: u32, words: &mut [u32]) {
    let t = ATLAS_TILE;
    let (x0, y0) = (tx * t, ty * t);
    let n = (v.full_w - x0).min(t) as usize;
    let mut row = Vec::with_capacity(2 * n);
    for ly in 0..t.min(v.full_h - y0) {
        let y = y0 + ly;
        let s = ((y - v.y0) * v.w + (x0 - v.x0)) as usize;
        row.clear();
        v.data.slice(s, s + n).pack(&mut row);
        let d = (ly * t * 2) as usize;
        words[d..d + 2 * n].copy_from_slice(&row);
    }
}

fn s_gpu(g: &gpu::Gpu, jobs: &[SJob<'_>]) -> Result<Vec<PxVec>, String> {
    use aigle_kernels::detail::{U_GH, U_GW, U_ROI, UP_LEN};
    let packed: Vec<(Vec<u32>, Vec<[u32; UP_LEN]>)> = jobs
        .par_iter()
        .map(|j| {
            let (x0, y0, w, h) = j.plan.src_rect;
            let v = &j.src;
            let mut src = Vec::with_capacity(w as usize * h as usize * v.data.gpu_words());
            for y in y0..y0 + h {
                let row = (y * v.w + x0) as usize;
                v.data.slice(row, row + w as usize).pack(&mut src);
            }
            let fu = v.data.is_f32() as u32;
            let ups = j
                .plan
                .passes
                .iter()
                .map(|p| {
                    let mut up = p.up;
                    up[U_ROI..U_ROI + 7].copy_from_slice(&[x0, y0, w, h, v.full_w, v.full_h, fu]);
                    up
                })
                .collect();
            (src, ups)
        })
        .collect();
    let chains: Vec<gpu::Chain<'_>> = jobs
        .iter()
        .zip(&packed)
        .map(|(j, (src, ups))| gpu::Chain {
            passes: j
                .plan
                .passes
                .iter()
                .zip(ups)
                .map(|(p, up)| gpu::ChainPass {
                    fp: &p.fp,
                    up,
                    gw: p.up[U_GW],
                    gh: p.up[U_GH],
                })
                .collect(),
            src,
            work_words: j.plan.work_len(),
            read: j.plan.out_range(),
        })
        .collect();
    let out = g.run_chains(&chains)?;
    Ok(out
        .into_par_iter()
        .zip(jobs)
        .map(|(w, j)| {
            let v = w
                .as_chunks::<3>()
                .0
                .iter()
                .map(|c| c.map(f32::from_bits))
                .collect();
            PxVec::from_rgb(v, j.src.data.is_f32())
        })
        .collect())
}

fn h16(w: u32, hi: bool) -> f16 {
    f16::from_bits(if hi { (w >> 16) as u16 } else { w as u16 })
}

fn unpack_c1_words(words: &[u32], fields: bool, full: bool) -> HostC1 {
    if full {
        let stride = if fields { 8 } else { 3 };
        let mut rgb = Vec::with_capacity(words.len() / stride);
        let mut d = Vec::new();
        for p in words.chunks_exact(stride) {
            let f = |k: usize| f32::from_bits(p[k]);
            rgb.push([f(0), f(1), f(2)]);
            if fields {
                d.push([f(3), f(4), f(5), f(6), f(7)]);
            }
        }
        return HostC1 {
            rgb: PxVec::F(rgb),
            d: Fields::F(d),
        };
    }
    let stride = if fields { 5 } else { 2 };
    let mut rgb = Vec::with_capacity(words.len() / stride);
    let mut d = Vec::new();
    for p in words.chunks_exact(stride) {
        rgb.push([h16(p[0], false), h16(p[0], true), h16(p[1], false)]);
        if fields {
            d.push([
                h16(p[2], false),
                h16(p[2], true),
                h16(p[3], false),
                h16(p[3], true),
                h16(p[4], false),
            ]);
        }
    }
    HostC1 {
        rgb: PxVec::H(rgb),
        d: Fields::H(d),
    }
}

fn pack_c1_host(c: &HostC1, out: &mut Vec<u32>) {
    let bits = |h: f16| h.to_bits() as u32;
    match (&c.rgb, &c.d) {
        (PxVec::H(rgb), Fields::H(d)) => {
            for (i, p) in rgb.iter().enumerate() {
                out.push(bits(p[0]) | bits(p[1]) << 16);
                out.push(bits(p[2]));
                if let Some(d) = d.get(i) {
                    out.push(bits(d[0]) | bits(d[1]) << 16);
                    out.push(bits(d[2]) | bits(d[3]) << 16);
                    out.push(bits(d[4]));
                }
            }
        }
        (PxVec::F(rgb), Fields::F(d)) => {
            for (i, p) in rgb.iter().enumerate() {
                out.extend(p.map(f32::to_bits));
                if let Some(d) = d.get(i) {
                    out.extend(d.map(f32::to_bits));
                }
            }
        }
        _ => unreachable!("C1 colour and fields share one precision"),
    }
}

fn out_gpu(g: &gpu::Gpu, jobs: &[OutJob<'_>], aux: gpu::Input<'_>) -> Result<Vec<Vec<u8>>, String> {
    let packed: Vec<(Vec<u32>, [f32; tiles::OUT_F32], [u32; tiles::OUT_U32])> = jobs
        .par_iter()
        .map(|job| {
            let mut src = Vec::new();
            if let C1Data::Host(px) = job.c1 {
                debug_assert_eq!(px.d.is_empty(), !job.fields);
                debug_assert_eq!(px.rgb.as_px().is_f32(), job.full);
                pack_c1_host(px, &mut src);
            }
            (
                src,
                tiles::pack_out(&job.params),
                tiles::pack_out_u32(
                    &job.params,
                    job.w,
                    job.h,
                    job.gx0,
                    job.gy0,
                    job.fields,
                    job.full,
                ),
            )
        })
        .collect();
    let items: Vec<gpu::Dispatch<'_>> = jobs
        .iter()
        .zip(&packed)
        .map(|(job, (src, fp, up))| gpu::Dispatch {
            fp,
            up,
            input: match job.c1 {
                C1Data::Device(b) => gpu::Input::Device(b),
                C1Data::Host(_) => gpu::Input::Host(src),
            },
            out_words: job.w as usize * job.h as usize * if job.full { 2 } else { 1 },
            w: job.w,
            h: job.h,
        })
        .collect();
    let out = g.run(gpu::Kernel::OutTile, &items, Some(aux), true)?;
    Ok(out
        .into_iter()
        .map(|o| match o {
            gpu::Output::Host(w) => bytemuck::cast_slice::<u32, u8>(&w).to_vec(),
            gpu::Output::Device(_) => unreachable!("readback requested"),
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn self_test_cache_keys_on_driver() {
        let dir = std::env::temp_dir().join(format!("aigle-selftest-{}", std::process::id()));
        let a = SelfTestCache::new(&dir, "gpu|mesa|25.2");
        assert!(!a.passed());
        a.record();
        assert!(a.passed());
        assert!(!SelfTestCache::new(&dir, "gpu|mesa|25.3").passed());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A lost device is re-created: later batches run on the new device, data of the old
    /// one is reported stale, and the loss is announced as recovered (skipped without GPU).
    #[test]
    fn device_loss_is_recovered() {
        let (tx, mut rx) = broadcast::channel(8);
        let ledger = crate::ledger::MemLedger::new(1 << 30);
        let (b, why) = Backend::select(BackendPreference::Auto, false, tx, &ledger, None);
        if b.kind() != BackendKind::Gpu {
            eprintln!("no usable GPU, skipping: {why:?}");
            return;
        }
        let (level, (affine, params)) = self_test_inputs();
        let geo = TileGeo::affine(affine, 0);
        let job = C1Job {
            src: LevelView::whole(&level),
            device_src: None,
            atlas: Some((crate::cache::KeyBuilder::new("loss").finish(), 0)),
            geo: &geo,
            params,
            c2: C2Params::default(),
            prog: &[0.0],
            color: &[0.0],
            fields: false,
            w: 40,
            h: 30,
        };
        let old = b.c1_tiles(&[job], None).remove(0);
        assert!(matches!(old, C1Data::Device(_)));
        let gen0 = b.gpu_active().unwrap().generation;
        b.simulate_loss();
        // The first batch after the loss fails over to the CPU and re-creates the device.
        let _ = b.c1_tiles(&[job], None);
        let g = b.gpu_active().expect("re-created");
        assert_ne!(g.generation, gen0);
        assert!(matches!(
            rx.try_recv(),
            Ok(EngineEvent::DeviceLost { recovered: true })
        ));
        let curves: Vec<f32> = (0..3 * CURVE_N)
            .map(|i| (i % CURVE_N) as f32 / (CURVE_N - 1) as f32)
            .collect();
        let mut aux = crate::pipeline::output_aux(Some(&curves));
        let mut params = self_test_out_params();
        params.d2.look = aux.len() as u32;
        aux.extend_from_slice(&self_test_look());
        let lut = Aux::new(aux);
        let out = |c1| OutJob {
            c1,
            fields: false,
            full: false,
            params,
            w: 40,
            h: 30,
            gx0: 0,
            gy0: 0,
        };
        assert_eq!(b.out_tiles(&[out(&old)], &lut), Err(StaleDeviceData));
        let fresh = b.c1_tiles(&[job], None).remove(0);
        let gpu_px = b.out_tiles(&[out(&fresh)], &lut).unwrap();
        let cpu = C1Data::Host(c1_cpu(&job, None));
        let cpu_px = Backend::cpu("t").out_tiles(&[out(&cpu)], &lut).unwrap();
        let max = gpu_px[0]
            .iter()
            .zip(&cpu_px[0])
            .map(|(a, b)| a.abs_diff(*b))
            .max();
        assert!(max <= Some(1), "{max:?}");
        assert_eq!(b.kind(), BackendKind::Gpu);
    }

    /// C1 through a source atlas far smaller than the level: chunked submissions and LRU
    /// eviction must not change the result (skipped without GPU).
    #[test]
    fn atlas_eviction_is_transparent() {
        let g = match gpu::Gpu::new(false) {
            Ok(g) => g,
            Err(e) => {
                eprintln!("no usable GPU, skipping: {e}");
                return;
            }
        };
        g.configure_memory(crate::ledger::MemLedger::new(1 << 30), 0);
        g.set_atlas_slots(8);
        let (w, h) = (1400u32, 1000u32);
        let rgb: Vec<Rgb> = (0..w * h)
            .map(|i| {
                let (x, y) = ((i % w) as f32, (i / w) as f32);
                [
                    x / 1400.0,
                    y / 1000.0,
                    0.5 + 0.4 * (x * 0.05 + y * 0.03).sin(),
                ]
            })
            .collect();
        let level = Level::from_f32(w, h, &rgb);
        let key = crate::cache::KeyBuilder::new("atlas test").finish();
        let (_, (_, params)) = self_test_inputs();
        let geos: Vec<TileGeo> = (0..20)
            .map(|i| {
                let (ox, oy) = ((i % 5) as f32 * 300.0 - 20.0, (i / 5) as f32 * 260.0 - 10.0);
                TileGeo::affine(
                    Affine {
                        a: [1.1, 0.05, ox, -0.05, 1.1, oy],
                    },
                    tiles::GEO_LANCZOS,
                )
            })
            .collect();
        let jobs: Vec<C1Job<'_>> = geos
            .iter()
            .map(|geo| C1Job {
                src: LevelView::whole(&level),
                device_src: None,
                atlas: Some((key, 0)),
                geo,
                params,
                c2: C2Params::default(),
                prog: &[0.0],
                color: &[0.0],
                fields: false,
                w: 200,
                h: 180,
            })
            .collect();
        // Twice: the second pass hits partly resident tiles in a different order.
        for pass in 0..2 {
            let order: Vec<_> = if pass == 0 {
                jobs.clone()
            } else {
                jobs.iter().rev().copied().collect()
            };
            let gpu = c1_gpu(&g, &order, None, None, true).unwrap();
            for (j, out) in order.iter().zip(gpu) {
                let C1Data::Host(out) = out else {
                    panic!("readback requested")
                };
                let cpu = c1_cpu(j, None);
                for i in 0..cpu.rgb.len() {
                    let (a, b) = (cpu.rgb.as_px().get(i), out.rgb.as_px().get(i));
                    for c in 0..3 {
                        let (a, b) = (a[c], b[c]);
                        assert!((a - b).abs() <= 2e-3 * a.abs().max(1e-2), "{a} vs {b}");
                    }
                }
            }
        }
        assert_eq!(g.atlas_slots().0, 8);
    }

    /// Full-size S blocks with every op and local fields, GPU vs CPU (skipped without GPU).
    #[test]
    fn s_gpu_matches_cpu() {
        let g = match gpu::Gpu::new(false) {
            Ok(g) => g,
            Err(e) => {
                eprintln!("no usable GPU, skipping: {e}");
                return;
            }
        };
        let (w, h) = (600u32, 400u32);
        let rgb: Vec<Rgb> = (0..w * h)
            .map(|i| {
                let (x, y) = (i % w, i / w);
                let n = |s| (aigle_kernels::dither::noise(x, y, s) - 0.5) * 0.02;
                let base = if (x / 24 + y / 24) % 2 == 0 {
                    0.08
                } else {
                    0.35
                };
                [base + n(1), base * 0.9 + n(2), base * 1.2 + n(3)]
            })
            .collect();
        let level = Level::from_f32(w, h, &rgb);
        let mut sp = detail::SParams::of(&aigle_core::EditParams::default());
        sp.luma = 50.0;
        sp.chroma = 60.0;
        sp.sharpen = 90.0;
        sp.masking = 25.0;
        sp.texture = -30.0;
        sp.local = [30.0, 40.0, 20.0];
        let noise = crate::noise::NoiseLevel {
            sigma: 0.012,
            offset: 2e-3,
        };
        let prog = self_test_masks(&level, crate::masks::Tier::S);
        let c1 = crate::pipeline::color::C1Color {
            params: self_test_inputs().1.1,
            block: vec![0.0],
            dcp_ok: true,
        };
        let plans: Vec<_> = [(0, 0), (256, 0), (512, 256)]
            .iter()
            .map(|&(x0, y0)| {
                let (bw, bh) = ((w - x0).min(256), (h - y0).min(256));
                detail::plan(w, h, 0, &sp, noise, x0, y0, bw, bh, Some((&prog, &c1)))
            })
            .collect();
        let jobs: Vec<_> = plans
            .iter()
            .map(|plan| SJob {
                plan,
                src: LevelView::whole(&level),
            })
            .collect();
        let gpu = s_gpu(&g, &jobs).unwrap();
        let mut worst = 0f32;
        for (j, gb) in jobs.iter().zip(&gpu) {
            let cb = detail::run_cpu(j.plan, &LevelView::whole(&level));
            assert_eq!(cb.len(), gb.len());
            for (i, a) in cb.iter().enumerate() {
                let b = gb.as_px().get(i);
                for c in 0..3 {
                    worst = worst.max((a[c] - b[c]).abs() / a[c].abs().max(1e-2));
                }
            }
        }
        assert!(worst <= 2e-3, "worst relative error {worst:.2e}");
    }

    #[test]
    fn roi_covers_every_sample() {
        let (level, (affine, params)) = self_test_inputs();
        let c2 = C2Params::default();
        let fv = field_view(None);
        let mut packed = Vec::new();
        Px::H(&level.data).pack(&mut packed);
        for (w, h) in [(37, 29), (1, 1), (64, 64)] {
            let geos = [
                TileGeo::affine(affine, 0),
                TileGeo::affine(affine, tiles::GEO_LANCZOS),
                self_test_warp(&affine, w, h),
            ];
            for geo in &geos {
                let (x0, y0, rw, rh) = geo.roi(w, h, level.width, level.height);
                // Copy the ROI and sample through PackedRoi: must equal sampling the full level.
                let mut roi = Vec::new();
                for y in y0..y0 + rh {
                    let s = ((y * level.width + x0) * 2) as usize;
                    roi.extend_from_slice(&packed[s..s + rw as usize * 2]);
                }
                let r =
                    tiles::PackedRoi::contiguous(&roi, (x0, y0, rw, rh), level.width, level.height);
                let whole = LevelView::whole(&level);
                let g = geo.view();
                for y in 0..h {
                    for x in 0..w {
                        let t = tiles::NO_TABLES;
                        let a = tiles::c1_px(&whole, &g, x, y, &params, &c2, &fv, &t);
                        let b = tiles::c1_px(&r, &g, x, y, &params, &c2, &fv, &t);
                        assert_eq!(
                            a, b,
                            "({x},{y}) roi {x0},{y0} {rw}x{rh} flags {}",
                            geo.flags
                        );
                    }
                }
            }
        }
    }
}
