//! wgpu/Vulkan compute backend (doc 04 §2, §8).
//!
//! Loads the committed rust-gpu SPIR-V (`crates/kernels/spv/`) through naga's SPIR-V
//! frontend. One headless device.
//!
//! Data residency (doc 03 §4.2–4.3): source pyramid levels are read through a tile atlas
//! ([`Gpu::run_atlas`]: 256² tile slots in one buffer, LRU, grown up to a share of the
//! budget); C1 tiles live in device buffers ([`DeviceBuf`], held by the engine's tile cache);
//! only BGRA8 output tiles are read back, through a pool of reused staging buffers.
//!
//! Memory (doc 03 §2): the atlas and the staging/scratch pools are accounted in the
//! engine's `MemLedger`. On an integrated GPU they count against the host budget (unified
//! memory); on a discrete GPU the ledger gets a device pool sized by `VK_EXT_memory_budget`,
//! and wgpu refuses allocations above 95 % of the driver budget.
//!
//! S-tier blocks run as chains of `s_*` dispatches over pooled workspaces ([`Chain`]); only
//! the block interiors are read back.
//!
//! Submissions (doc 05 §6): every submission takes the queue arbiter by scheduler class
//! (strict priority, starvation guard); lower classes are cut into chunks of ~3 ms of GPU
//! time from an EWMA of measured time per item, and hold the queue until their chunk has
//! finished, so interactive work waits behind at most one of them. The arbiter replaces the
//! single submitter thread of doc 05 §5 with the same ordering.
//!
//! Device loss (doc 04 §8.2): a lost device is reported by its callback; the backend then
//! re-creates the device (`Backend::gpu_failed`). Buffers carry the generation of their
//! device, so data of a lost device is never bound to the new one.
//!
//! E2 stays on the CPU (measured; doc 04 §4 note): it costs 3–15 ms of a ~1 s 1:1 render,
//! and read-back output tiles are what panning reuses from the RAM cache; keeping them on
//! the device would make every pan read the viewport back instead.
//!
//! Integrated GPUs clock down when idle: on the Intel UHD 770 the same S batch takes 60 ms
//! after a 20 ms gap and 120 ms after 1.5 s. Keeping it busy between frames (an empty
//! dispatch, or 8 ms of real work, every 100 ms) did not change that, so nothing is done
//! about it (doc 05 §4 note).

use std::sync::mpsc;
use std::sync::{Arc, OnceLock};

use aigle_kernels::tiles::ATLAS_TILE_WORDS;
use parking_lot::Mutex;
use rayon::prelude::*;
use wgpu::util::DeviceExt;

use super::atlas::{AtlasState, TileId};
use crate::ledger::{Allocation, MemClass, MemLedger};

/// rust-gpu output, rebuilt by `cargo xtask kernels`.
pub const SPIRV: &[u8] = include_bytes!("../../../kernels/spv/aigle_kernels.spv");

/// Staging buffers are allocated in multiples of this and reused.
const STAGING_QUANTUM: u64 = 16 << 20;
/// Upper bound of readback bytes per submission.
const MAX_READBACK: u64 = 128 << 20;
/// Upper bound of S-tier workspace bytes per submission.
const MAX_WORK: u64 = 256 << 20;

pub struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    layout: wgpu::BindGroupLayout,
    /// S passes: bindings 0–3 with dynamic offsets.
    s_layout: wgpu::BindGroupLayout,
    c1: wgpu::ComputePipeline,
    out: wgpu::ComputePipeline,
    /// S-tier entry points, indexed by `aigle_kernels::detail::ENTRY_*`.
    s: Vec<wgpu::ComputePipeline>,
    staging: Mutex<Vec<Pooled>>,
    scratch: Mutex<Vec<Pooled>>,
    /// Bound at binding 4 when a kernel has no auxiliary data.
    dummy: wgpu::Buffer,
    align: u64,
    pub max_binding: u64,
    pub adapter: String,
    /// Identity of this device (unique per process), so data of a lost device is recognised.
    pub generation: u64,
    /// Set by the device-lost callback (doc 04 §8.2).
    lost: Arc<std::sync::atomic::AtomicBool>,
    /// (adapter name, driver, driver info) for the self-test cache (doc 04 §8.3).
    pub driver_id: String,
    /// Integrated GPU: device memory is system RAM (doc 03 §1).
    pub unified: bool,
    /// Driver budget of the device-local heaps (`VK_EXT_memory_budget`), if reported.
    pub device_budget: Option<u64>,
    atlas: Mutex<Option<Atlas>>,
    /// Atlas capacity in slots, set by [`Gpu::configure_memory`].
    atlas_max: std::sync::atomic::AtomicU32,
    ledger: OnceLock<Arc<MemLedger>>,
    /// The queue, arbitrated by class (doc 05 §6): one submission at a time.
    queue_arb: crate::arbiter::Arbiter,
    /// EWMA of GPU time per item, per kernel (doc 05 §6 cost model), in seconds.
    cost: Mutex<std::collections::HashMap<CostKey, f64>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum CostKey {
    Tile(Kernel),
    SBlock,
}

/// GPU time per lower-class submission (doc 05 Q33).
const LOW_CHUNK: f64 = 0.003;

/// A reused staging / scratch buffer, accounted as `Transient` (doc 03 §2.2).
struct Pooled {
    buf: wgpu::Buffer,
    _alloc: Option<Allocation>,
}

impl std::ops::Deref for Pooled {
    type Target = wgpu::Buffer;
    fn deref(&self) -> &wgpu::Buffer {
        &self.buf
    }
}

/// The source tile atlas: a pool of tile slots in one buffer (doc 03 §4.2).
struct Atlas {
    buf: DeviceBuf,
    state: AtlasState,
    _alloc: Option<Allocation>,
}

/// Bytes of one atlas slot.
pub const ATLAS_SLOT_BYTES: u64 = ATLAS_TILE_WORDS as u64 * 4;
/// The atlas starts with this many slots and doubles up to its capacity.
const ATLAS_MIN_SLOTS: u32 = 128;

/// One dispatch reading source tiles from the atlas: `up` without the slot table, and the
/// tile rectangle (`tx0, ty0, ntx, nty`) of the `(key, level)` it reads.
pub struct AtlasDispatch<'a> {
    pub fp: &'a [f32],
    pub up: &'a [u32],
    pub key: crate::cache::CacheKey,
    pub level: u32,
    pub tiles: (u32, u32, u32, u32),
    pub out_words: usize,
    pub w: u32,
    pub h: u32,
}

impl AtlasDispatch<'_> {
    fn ids(&self) -> impl Iterator<Item = TileId> + '_ {
        let (tx0, ty0, nx, ny) = self.tiles;
        (ty0..ty0 + ny)
            .flat_map(move |ty| (tx0..tx0 + nx).map(move |tx| (self.key, self.level, tx, ty)))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kernel {
    C1Tile,
    OutTile,
}

/// A device-resident buffer of `u32` words.
#[derive(Debug)]
pub struct DeviceBuf {
    buf: wgpu::Buffer,
    pub words: usize,
    /// Generation of the device that owns it (unique per device in the process).
    pub generation: u64,
}

pub enum Input<'a> {
    Host(&'a [u32]),
    Device(&'a DeviceBuf),
}

/// One tile dispatch: packed params, input pixels, output size and grid.
pub struct Dispatch<'a> {
    pub fp: &'a [f32],
    pub up: &'a [u32],
    pub input: Input<'a>,
    pub out_words: usize,
    pub w: u32,
    pub h: u32,
}

impl Gpu {
    /// Create a headless Vulkan device and the kernel pipelines. Blocks the calling thread.
    /// Software rasterisers (llvmpipe/lavapipe) are refused unless `allow_software`: they are
    /// slower than the native CPU backend and only useful for testing.
    pub fn new(allow_software: bool) -> Result<Gpu, String> {
        pollster(Self::new_async(allow_software))
    }

    async fn new_async(allow_software: bool) -> Result<Gpu, String> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            // Report OOM instead of oversubscribing device memory (doc 03 §2.1).
            memory_budget_thresholds: wgpu::MemoryBudgetThresholds {
                for_resource_creation: Some(95),
                for_device_loss: None,
            },
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                force_fallback_adapter: false,
                compatible_surface: None,
                ..Default::default()
            })
            .await
            .map_err(|e| format!("no Vulkan adapter: {e}"))?;
        let info = adapter.get_info();
        let name = format!(
            "{} ({:?}, {} {})",
            info.name, info.device_type, info.driver, info.driver_info
        );
        if info.device_type == wgpu::DeviceType::Cpu && !allow_software {
            return Err(format!(
                "only a software Vulkan device is available: {name}"
            ));
        }
        // Request the adapter's real limits (doc 04 §8.1); correctness never depends on them.
        let limits = adapter.limits();
        let max_binding = limits
            .max_storage_buffer_binding_size
            .min(limits.max_buffer_size);
        let align = limits.min_storage_buffer_offset_alignment.max(4) as u64;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("aigle"),
                required_features: wgpu::Features::empty(),
                required_limits: limits,
                ..Default::default()
            })
            .await
            .map_err(|e| format!("request device on {name}: {e}"))?;
        let lost = Arc::new(std::sync::atomic::AtomicBool::new(false));
        {
            let lost = lost.clone();
            device.set_device_lost_callback(move |reason, msg| {
                tracing::error!(?reason, "GPU device lost: {msg}");
                lost.store(true, std::sync::atomic::Ordering::Release);
            });
        }
        static GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let generation = GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let driver_id = format!("{}|{}|{}", info.name, info.driver, info.driver_info);

        let unified = info.device_type != wgpu::DeviceType::DiscreteGpu;
        let device_budget = device_local_budget(&device);
        tracing::info!(
            unified,
            device_budget_mib = device_budget.map(|b| b >> 20),
            "GPU memory"
        );
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("aigle_kernels"),
            source: wgpu::util::make_spirv(SPIRV),
        });
        let storage = |binding, read_only| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("tile"),
            entries: &[
                storage(0, true),
                storage(1, true),
                storage(2, true),
                storage(3, false),
                storage(4, true),
            ],
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("tile"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let dynamic = |binding, read_only| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only },
                has_dynamic_offset: true,
                min_binding_size: None,
            },
            count: None,
        };
        let s_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("s passes"),
            entries: &[
                dynamic(0, true),
                dynamic(1, true),
                dynamic(2, true),
                dynamic(3, false),
            ],
        });
        let s_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("s passes"),
            bind_group_layouts: &[Some(&s_layout)],
            immediate_size: 0,
        });
        let pipeline_in = |entry: &str, pl: &wgpu::PipelineLayout| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: Some(pl),
                module: &module,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let c1 = pipeline_in("c1_tile", &pl);
        let out = pipeline_in("out_tile", &pl);
        let s = aigle_kernels::detail::ENTRY_NAMES
            .iter()
            .map(|n| pipeline_in(n, &s_pl))
            .collect();
        if let Some(e) = scope.pop().await {
            return Err(format!("loading kernels on {name}: {e}"));
        }
        let dummy = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("dummy"),
            contents: &[0u8; 64],
            usage: wgpu::BufferUsages::STORAGE,
        });
        Ok(Gpu {
            dummy,
            device,
            queue,
            layout,
            s_layout,
            c1,
            out,
            s,
            staging: Mutex::new(Vec::new()),
            scratch: Mutex::new(Vec::new()),
            align,
            max_binding,
            adapter: name,
            generation,
            lost,
            driver_id,
            unified,
            device_budget,
            atlas: Mutex::new(None),
            atlas_max: std::sync::atomic::AtomicU32::new(ATLAS_MIN_SLOTS),
            ledger: OnceLock::new(),
            queue_arb: crate::arbiter::Arbiter::new("gpu", 1),
            cost: Mutex::new(Default::default()),
        })
    }

    /// Account device memory in `ledger` and size the atlas to `atlas_bytes` (clamped to
    /// [`ATLAS_MIN_SLOTS`] and the binding limit).
    pub fn configure_memory(&self, ledger: Arc<MemLedger>, atlas_bytes: u64) {
        let max = (self.max_binding / ATLAS_SLOT_BYTES) as u32;
        let slots = ((atlas_bytes / ATLAS_SLOT_BYTES) as u32).clamp(ATLAS_MIN_SLOTS, max.max(1));
        self.atlas_max
            .store(slots, std::sync::atomic::Ordering::Release);
        let _ = self.ledger.set(ledger);
    }

    /// Whether the driver reported the device lost.
    pub fn is_lost(&self) -> bool {
        self.lost.load(std::sync::atomic::Ordering::Acquire)
    }

    /// Lose the device (tests of recovery).
    #[cfg(test)]
    pub fn simulate_loss(&self) {
        self.device.destroy();
        let _ = self.device.poll(wgpu::PollType::Poll);
    }

    /// Override the atlas capacity (tests: force eviction and chunking).
    #[cfg(test)]
    pub fn set_atlas_slots(&self, slots: u32) {
        self.atlas_max
            .store(slots, std::sync::atomic::Ordering::Release);
    }

    /// Atlas (resident slots, capacity in slots).
    #[cfg(test)]
    pub fn atlas_slots(&self) -> (u32, u32) {
        let a = self.atlas.lock();
        (
            a.as_ref().map_or(0, |a| a.state.len()),
            self.atlas_max.load(std::sync::atomic::Ordering::Acquire),
        )
    }

    /// Make the atlas hold at least `slots` slots (≤ capacity), copying resident tiles.
    fn grow_atlas(&self, atlas: &mut Option<Atlas>, slots: u32) -> Result<(), String> {
        let cur = atlas.as_ref().map_or(0, |a| a.state.len());
        if slots <= cur {
            return Ok(());
        }
        let bytes = slots as u64 * ATLAS_SLOT_BYTES;
        let oom = self.device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("source atlas"),
            size: bytes,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_DST
                | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        if let Some(e) = pollster(oom.pop()) {
            return Err(format!("atlas of {} MiB: {e}", bytes >> 20));
        }
        let alloc = self
            .ledger
            .get()
            .map(|l| l.alloc(MemClass::SourceGpu, bytes));
        let state = match atlas.take() {
            Some(old) => {
                let mut enc = self.device.create_command_encoder(&Default::default());
                enc.copy_buffer_to_buffer(
                    &old.buf.buf,
                    0,
                    &buf,
                    0,
                    old.state.len() as u64 * ATLAS_SLOT_BYTES,
                );
                self.queue.submit([enc.finish()]);
                let mut st = old.state;
                st.grow(slots);
                st
            }
            None => AtlasState::new(slots),
        };
        tracing::debug!(slots, mib = bytes >> 20, "atlas grown");
        *atlas = Some(Atlas {
            buf: DeviceBuf {
                buf,
                words: slots as usize * ATLAS_TILE_WORDS,
                generation: self.generation,
            },
            state,
            _alloc: alloc,
        });
        Ok(())
    }

    /// Run `kernel` over `items` that read source tiles from the atlas. Missing tiles are
    /// filled by `fill(tile, words)` (RGBA f16, `ATLAS_TILE`² words × 2) and uploaded. Items
    /// are split into submissions whose tiles fit the atlas.
    pub fn run_atlas(
        &self,
        kernel: Kernel,
        items: &[AtlasDispatch<'_>],
        fill: &(dyn Fn(&TileId, &mut [u32]) + Sync),
        aux: Option<Input<'_>>,
        readback: bool,
    ) -> Result<Vec<Output>, String> {
        let cap = self.atlas_max.load(std::sync::atomic::Ordering::Acquire) as usize;
        let mut out = Vec::with_capacity(items.len());
        let mut start = 0;
        while start < items.len() {
            // Greedy chunk whose distinct tiles fit the atlas.
            let mut want: Vec<TileId> = Vec::new();
            let mut seen = std::collections::HashSet::new();
            let mut end = start;
            let cap_items = self.chunk_items(CostKey::Tile(kernel));
            while end < items.len() && end - start < cap_items {
                let new: Vec<TileId> = items[end].ids().filter(|t| !seen.contains(t)).collect();
                if want.len() + new.len() > cap {
                    break;
                }
                for t in new {
                    seen.insert(t);
                    want.push(t);
                }
                end += 1;
            }
            if end == start {
                return Err(format!(
                    "a dispatch needs {} atlas tiles, capacity {cap}",
                    items[start].ids().count()
                ));
            }
            let mut guard = self.atlas.lock();
            let cur = guard.as_ref().map_or(0, |a| a.state.len()) as usize;
            let misses = guard.as_ref().map_or(want.len(), |a| a.state.misses(&want));
            let fits = guard
                .as_ref()
                .is_some_and(|a| a.state.fits_without_eviction(misses));
            if !fits && cur < cap {
                // Fill to capacity before evicting.
                let need = (cur + misses).max(want.len());
                let target = need.max(cur * 2).max(ATLAS_MIN_SLOTS as usize).min(cap);
                debug_assert!(target >= want.len());
                self.grow_atlas(&mut guard, target as u32)?;
            }
            let atlas = guard.as_mut().expect("grown above");
            let (slots, upload) = atlas.state.assign(&want);
            let filled: Vec<(u32, Vec<u32>)> = upload
                .par_iter()
                .map(|&i| {
                    let mut w = vec![0u32; ATLAS_TILE_WORDS];
                    fill(&want[i], &mut w);
                    (slots[i], w)
                })
                .collect();
            for (slot, w) in &filled {
                self.queue.write_buffer(
                    &atlas.buf.buf,
                    *slot as u64 * ATLAS_SLOT_BYTES,
                    bytemuck::cast_slice(w),
                );
            }
            let index: std::collections::HashMap<TileId, u32> =
                want.iter().copied().zip(slots.iter().copied()).collect();
            let ups: Vec<Vec<u32>> = items[start..end]
                .iter()
                .map(|it| {
                    let mut up = it.up.to_vec();
                    up.extend(it.ids().map(|t| index[&t]));
                    up
                })
                .collect();
            let dispatches: Vec<Dispatch<'_>> = items[start..end]
                .iter()
                .zip(&ups)
                .map(|(it, up)| Dispatch {
                    fp: it.fp,
                    up,
                    input: Input::Device(&atlas.buf),
                    out_words: it.out_words,
                    w: it.w,
                    h: it.h,
                })
                .collect();
            let aux_ref = match &aux {
                None => None,
                Some(Input::Host(h)) => Some(Input::Host(h)),
                Some(Input::Device(d)) => Some(Input::Device(d)),
            };
            out.extend(self.run(kernel, &dispatches, aux_ref, readback)?);
            drop(guard);
            start = end;
        }
        Ok(out)
    }

    /// Upload `words` into a new device buffer (e.g. a source pyramid level).
    pub fn upload(&self, words: &[u32]) -> Result<DeviceBuf, String> {
        if (words.len() * 4) as u64 > self.max_binding {
            return Err(format!(
                "{} MiB exceeds the device's storage binding limit",
                words.len() >> 18
            ));
        }
        let scope = self.device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let buf = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("resident"),
                contents: bytemuck::cast_slice(words),
                usage: wgpu::BufferUsages::STORAGE,
            });
        if let Some(e) = pollster(scope.pop()) {
            return Err(format!("upload: {e}"));
        }
        Ok(DeviceBuf {
            buf,
            words: words.len(),
            generation: self.generation,
        })
    }

    /// Items per submission for the current task: unlimited for interactive work, else as
    /// many as fit [`LOW_CHUNK`] of measured GPU time (at least one).
    fn chunk_items(&self, key: CostKey) -> usize {
        if crate::arbiter::current().class == crate::scheduler::Class::Interactive {
            return usize::MAX;
        }
        match self.cost.lock().get(&key) {
            Some(&per) if per > 0.0 => ((LOW_CHUNK / per) as usize).max(1),
            _ => 1,
        }
    }

    fn record_cost(&self, key: CostKey, items: usize, gpu: std::time::Duration) {
        let per = gpu.as_secs_f64() / items.max(1) as f64;
        let mut c = self.cost.lock();
        let e = c.entry(key).or_insert(per);
        *e = 0.8 * *e + 0.2 * per;
    }

    /// Submit `cmd` holding the queue (strict class priority). Interactive work releases the
    /// queue once submitted; lower classes hold it until the GPU has finished, so an
    /// interactive submission waits behind at most one lower-class chunk. Returns the time
    /// from submission to completion when it was waited for.
    fn submit_arbitrated(
        &self,
        cmd: wgpu::CommandBuffer,
        wait: bool,
    ) -> Result<(wgpu::SubmissionIndex, Option<std::time::Duration>), String> {
        let grant = self.queue_arb.acquire(None).expect("no cancel token");
        let low = crate::arbiter::current().class != crate::scheduler::Class::Interactive;
        let index = self.queue.submit([cmd]);
        if !(wait || low) {
            return Ok((index, None));
        }
        let t = std::time::Instant::now();
        self.device
            .poll(wgpu::PollType::Wait {
                submission_index: Some(index.clone()),
                timeout: None,
            })
            .map_err(|e| format!("poll: {e}"))?;
        drop(grant);
        Ok((index, Some(t.elapsed())))
    }

    /// Run one kernel per item in a single submission. With `readback`, outputs are returned
    /// as host words; otherwise they stay resident on the device.
    pub fn run(
        &self,
        kernel: Kernel,
        items: &[Dispatch<'_>],
        aux: Option<Input<'_>>,
        readback: bool,
    ) -> Result<Vec<Output>, String> {
        let mut results = Vec::with_capacity(items.len());
        let chunk = self.chunk_items(CostKey::Tile(kernel));
        let mut start = 0;
        while start < items.len() {
            let mut end = start + 1;
            let mut bytes = items[start].out_words as u64 * 4;
            while readback
                && end < items.len()
                && bytes + items[end].out_words as u64 * 4 <= MAX_READBACK
            {
                bytes += items[end].out_words as u64 * 4;
                end += 1;
            }
            if !readback {
                end = items.len();
            }
            end = end.min(start.saturating_add(chunk));
            results.extend(self.submit(kernel, &items[start..end], aux.as_ref(), readback)?);
            start = end;
        }
        Ok(results)
    }

    fn take_pooled(
        &self,
        pool: &Mutex<Vec<Pooled>>,
        size: u64,
        usage: wgpu::BufferUsages,
    ) -> Pooled {
        let mut pool = pool.lock();
        // Smallest buffer that fits, so that small requests do not take the large buffers.
        let best = pool
            .iter()
            .enumerate()
            .filter(|(_, b)| b.size() >= size)
            .min_by_key(|(_, b)| b.size())
            .map(|(i, _)| i);
        if let Some(i) = best {
            return pool.swap_remove(i);
        }
        drop(pool);
        let bytes = size.div_ceil(STAGING_QUANTUM) * STAGING_QUANTUM;
        let buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("pooled"),
            size: bytes,
            usage,
            mapped_at_creation: false,
        });
        let _alloc = self
            .ledger
            .get()
            .map(|l| l.alloc(MemClass::Transient, bytes));
        if usage.contains(wgpu::BufferUsages::STORAGE) {
            // Initialise once so that wgpu does not zero it again on later submissions.
            let mut enc = self.device.create_command_encoder(&Default::default());
            enc.clear_buffer(&buf, 0, None);
            self.queue.submit([enc.finish()]);
        }
        Pooled { buf, _alloc }
    }

    fn return_pooled(&self, pool: &Mutex<Vec<Pooled>>, buf: Pooled) {
        let mut pool = pool.lock();
        pool.push(buf);
        // Keep at most 4 buffers; drop the smallest beyond that.
        if pool.len() > 4 {
            pool.sort_by_key(|b| std::cmp::Reverse(b.size()));
            pool.truncate(4);
        }
    }

    fn submit(
        &self,
        kernel: Kernel,
        items: &[Dispatch<'_>],
        aux: Option<&Input<'_>>,
        readback: bool,
    ) -> Result<Vec<Output>, String> {
        let d = &self.device;
        let t0 = std::time::Instant::now();
        let scope = d.push_error_scope(wgpu::ErrorFilter::Validation);
        let oom = d.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let init = |label, bytes: &[u8]| {
            d.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents: bytes,
                usage: wgpu::BufferUsages::STORAGE,
            })
        };
        // Read-back batches write into one pooled scratch buffer at aligned offsets.
        let mut offsets = Vec::with_capacity(items.len());
        let mut total = 0u64;
        for it in items {
            offsets.push(total);
            total += ((it.out_words * 4) as u64).div_ceil(self.align) * self.align;
        }
        let staging = readback.then(|| {
            self.take_pooled(
                &self.staging,
                total,
                wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            )
        });
        let scratch = readback.then(|| {
            self.take_pooled(
                &self.scratch,
                total,
                wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::COPY_SRC
                    | wgpu::BufferUsages::COPY_DST,
            )
        });
        let host_aux;
        let aux_buf = match aux {
            None => &self.dummy,
            Some(Input::Device(b)) => &b.buf,
            Some(Input::Host(w)) => {
                host_aux = init("aux", bytemuck::cast_slice(w));
                &host_aux
            }
        };
        let mut enc = d.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("tiles"),
        });
        let mut outs = Vec::with_capacity(items.len());
        let mut keep = Vec::with_capacity(items.len());
        {
            let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: None,
                timestamp_writes: None,
            });
            pass.set_pipeline(match kernel {
                Kernel::C1Tile => &self.c1,
                Kernel::OutTile => &self.out,
            });
            for (it, &offset) in items.iter().zip(&offsets) {
                let size = (it.out_words * 4) as u64;
                let fp_buf = init("fp", bytemuck::cast_slice(it.fp));
                let up_buf = init("up", bytemuck::cast_slice(it.up));
                let host_in;
                let in_buf = match &it.input {
                    Input::Host(w) => {
                        host_in = init("in", bytemuck::cast_slice(w));
                        &host_in
                    }
                    Input::Device(b) => &b.buf,
                };
                let owned = scratch.is_none().then(|| {
                    d.create_buffer(&wgpu::BufferDescriptor {
                        label: Some("out"),
                        size,
                        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                        mapped_at_creation: false,
                    })
                });
                let out_binding = match (&owned, &scratch) {
                    (Some(b), _) => b.as_entire_binding(),
                    (None, Some(sc)) => wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: sc,
                        offset,
                        size: std::num::NonZeroU64::new(size),
                    }),
                    (None, None) => unreachable!(),
                };
                let bind = d.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("tile"),
                    layout: &self.layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: fp_buf.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: up_buf.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: in_buf.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 3,
                            resource: out_binding,
                        },
                        wgpu::BindGroupEntry {
                            binding: 4,
                            resource: aux_buf.as_entire_binding(),
                        },
                    ],
                });
                pass.set_bind_group(0, &bind, &[]);
                pass.dispatch_workgroups(it.w.div_ceil(8), it.h.div_ceil(8), 1);
                if let Some(b) = owned {
                    outs.push(b);
                }
                keep.push(bind);
            }
        }
        if let (Some(st), Some(sc)) = (&staging, &scratch) {
            enc.copy_buffer_to_buffer(sc, 0, st, 0, total);
        }
        let (index, waited) = self.submit_arbitrated(enc.finish(), readback)?;
        if let Some(g) = waited {
            self.record_cost(CostKey::Tile(kernel), items.len(), g);
        }
        let t_record = t0.elapsed();
        if let Some(e) = pollster(oom.pop()) {
            return Err(format!("dispatch {kernel:?}: {e}"));
        }
        if let Some(e) = pollster(scope.pop()) {
            return Err(format!("dispatch {kernel:?}: {e}"));
        }
        let Some(staging) = staging else {
            drop(keep);
            return Ok(outs
                .into_iter()
                .zip(items)
                .map(|(buf, it)| {
                    Output::Device(DeviceBuf {
                        buf,
                        words: it.out_words,
                        generation: self.generation,
                    })
                })
                .collect());
        };

        let (tx, rx) = mpsc::channel();
        staging
            .slice(..total)
            .map_async(wgpu::MapMode::Read, move |r| {
                let _ = tx.send(r);
            });
        d.poll(wgpu::PollType::Wait {
            submission_index: Some(index),
            timeout: None,
        })
        .map_err(|e| format!("poll: {e}"))?;
        rx.recv()
            .map_err(|_| "readback callback dropped".to_string())?
            .map_err(|e| format!("map readback: {e}"))?;
        let t_wait = t0.elapsed();
        let view = staging
            .slice(..total)
            .get_mapped_range()
            .map_err(|e| format!("map readback: {e:?}"))?;
        let mut res = Vec::with_capacity(items.len());
        for (it, &o) in items.iter().zip(&offsets) {
            let (o, n) = (o as usize, it.out_words * 4);
            res.push(Output::Host(bytemuck::pod_collect_to_vec::<u8, u32>(
                &view[o..o + n],
            )));
        }
        drop(view);
        staging.unmap();
        self.return_pooled(&self.staging, staging);
        if let Some(sc) = scratch {
            self.return_pooled(&self.scratch, sc);
        }
        drop(keep);
        tracing::debug!(
            ?kernel,
            tiles = items.len(),
            mib = total as f64 / (1 << 20) as f64,
            t_record = ?t_record,
            t_gpu = ?(t_wait - t_record),
            t_read = ?(t0.elapsed() - t_wait),
            "gpu submit"
        );
        Ok(res)
    }
}

/// One pass of a [`Chain`]: an `s_pass` dispatch over a `gw × gh` grid.
pub struct ChainPass<'a> {
    pub fp: &'a [f32],
    pub up: &'a [u32],
    pub gw: u32,
    pub gh: u32,
}

/// A sequence of `s_pass` dispatches over one `f32` workspace of `work_words`, of which
/// `read` (start, len) is read back.
pub struct Chain<'a> {
    pub passes: Vec<ChainPass<'a>>,
    /// Source ROI, RGBA f16.
    pub src: &'a [u32],
    pub work_words: usize,
    pub read: (usize, usize),
}

impl Gpu {
    /// Run S-tier chains, several per submission; returns the read-back words of each.
    pub fn run_chains(&self, chains: &[Chain<'_>]) -> Result<Vec<Vec<u32>>, String> {
        let mut res = Vec::with_capacity(chains.len());
        let chunk = self.chunk_items(CostKey::SBlock);
        let mut start = 0;
        while start < chains.len() {
            let mut end = start + 1;
            let mut bytes = chains[start].work_words as u64 * 4;
            while end < chains.len()
                && end - start < chunk
                && bytes + chains[end].work_words as u64 * 4 <= MAX_WORK
            {
                bytes += chains[end].work_words as u64 * 4;
                end += 1;
            }
            res.extend(self.submit_chains(&chains[start..end])?);
            start = end;
        }
        Ok(res)
    }

    /// All chains of a submission share one bind group: parameters, source ROIs and
    /// workspaces are packed into one buffer each and selected per dispatch by dynamic
    /// offsets.
    fn submit_chains(&self, chains: &[Chain<'_>]) -> Result<Vec<Vec<u32>>, String> {
        let d = &self.device;
        let t0 = std::time::Instant::now();
        let aw = (self.align / 4) as usize;
        let up_stride = aigle_kernels::detail::UP_LEN.next_multiple_of(aw);
        let fp_len = chains
            .iter()
            .flat_map(|c| c.passes.iter().map(|p| p.fp.len()))
            .max()
            .unwrap_or(1);
        let fp_stride = fp_len.next_multiple_of(aw);
        let src_len = chains.iter().map(|c| c.src.len()).max().unwrap_or(1).max(1);
        let src_stride = src_len.next_multiple_of(aw);
        let work_len = chains.iter().map(|c| c.work_words).max().unwrap_or(1);
        let work_stride = work_len.next_multiple_of(aw);
        if (work_len * 4) as u64 > self.max_binding || (fp_len * 4) as u64 > self.max_binding {
            return Err("S workspace exceeds the storage binding limit".into());
        }
        let npass: usize = chains.iter().map(|c| c.passes.len()).sum();
        let mut fp = vec![0f32; npass * fp_stride];
        let mut up = vec![0u32; npass * up_stride];
        let mut src = vec![0u32; chains.len() * src_stride];
        let mut k = 0;
        for (ci, c) in chains.iter().enumerate() {
            src[ci * src_stride..ci * src_stride + c.src.len()].copy_from_slice(c.src);
            for p in &c.passes {
                fp[k * fp_stride..k * fp_stride + p.fp.len()].copy_from_slice(p.fp);
                up[k * up_stride..k * up_stride + p.up.len()].copy_from_slice(p.up);
                k += 1;
            }
        }
        let scope = d.push_error_scope(wgpu::ErrorFilter::Validation);
        let oom = d.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let init = |label, bytes: &[u8]| {
            d.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents: bytes,
                usage: wgpu::BufferUsages::STORAGE,
            })
        };
        let fp_buf = init("s fp", bytemuck::cast_slice(&fp));
        let up_buf = init("s up", bytemuck::cast_slice(&up));
        let src_buf = init("s src", bytemuck::cast_slice(&src));
        let work_bytes = (chains.len() * work_stride * 4) as u64;
        let work = self.take_pooled(
            &self.scratch,
            work_bytes,
            wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_SRC
                | wgpu::BufferUsages::COPY_DST,
        );
        let mut reads = Vec::with_capacity(chains.len());
        let mut total = 0u64;
        for c in chains {
            reads.push(total);
            total += c.read.1 as u64 * 4;
        }
        let staging = self.take_pooled(
            &self.staging,
            total.max(4),
            wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        );
        fn sized(b: &wgpu::Buffer, words: usize) -> wgpu::BindingResource<'_> {
            wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                buffer: b,
                offset: 0,
                size: std::num::NonZeroU64::new(words as u64 * 4),
            })
        }
        let bind = d.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("s passes"),
            layout: &self.s_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: sized(&fp_buf, fp_len),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: sized(&up_buf, aigle_kernels::detail::UP_LEN),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: sized(&src_buf, src_len),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: sized(&work, work_len),
                },
            ],
        });
        let mut enc = d.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("s blocks"),
        });
        {
            let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: None,
                timestamp_writes: None,
            });
            let mut current = u32::MAX;
            let mut k = 0;
            for (ci, c) in chains.iter().enumerate() {
                for p in &c.passes {
                    let e = aigle_kernels::detail::entry(p.up[aigle_kernels::detail::U_OP]);
                    if e != current {
                        pass.set_pipeline(&self.s[e as usize]);
                        current = e;
                    }
                    let offs = [
                        (k * fp_stride * 4) as u32,
                        (k * up_stride * 4) as u32,
                        (ci * src_stride * 4) as u32,
                        (ci * work_stride * 4) as u32,
                    ];
                    pass.set_bind_group(0, &bind, &offs);
                    pass.dispatch_workgroups(p.gw.div_ceil(8), p.gh.div_ceil(8), 1);
                    k += 1;
                }
            }
        }
        for (ci, (c, &o)) in chains.iter().zip(&reads).enumerate() {
            enc.copy_buffer_to_buffer(
                &work,
                ((ci * work_stride + c.read.0) * 4) as u64,
                &staging,
                o,
                c.read.1 as u64 * 4,
            );
        }
        let (index, waited) = self.submit_arbitrated(enc.finish(), true)?;
        if let Some(g) = waited {
            self.record_cost(CostKey::SBlock, chains.len(), g);
        }
        let t_record = t0.elapsed();
        if let Some(e) = pollster(oom.pop()) {
            return Err(format!("dispatch S passes: {e}"));
        }
        if let Some(e) = pollster(scope.pop()) {
            return Err(format!("dispatch S passes: {e}"));
        }
        let (tx, rx) = mpsc::channel();
        staging
            .slice(..total.max(4))
            .map_async(wgpu::MapMode::Read, move |r| {
                let _ = tx.send(r);
            });
        d.poll(wgpu::PollType::Wait {
            submission_index: Some(index),
            timeout: None,
        })
        .map_err(|e| format!("poll: {e}"))?;
        rx.recv()
            .map_err(|_| "readback callback dropped".to_string())?
            .map_err(|e| format!("map readback: {e}"))?;
        let t_wait = t0.elapsed();
        let view = staging
            .slice(..total.max(4))
            .get_mapped_range()
            .map_err(|e| format!("map readback: {e:?}"))?;
        let res = chains
            .iter()
            .zip(&reads)
            .map(|(c, &o)| {
                let o = o as usize;
                bytemuck::pod_collect_to_vec::<u8, u32>(&view[o..o + c.read.1 * 4])
            })
            .collect();
        drop(view);
        staging.unmap();
        self.return_pooled(&self.staging, staging);
        self.return_pooled(&self.scratch, work);
        tracing::debug!(
            blocks = chains.len(),
            mib = total as f64 / (1 << 20) as f64,
            t_record = ?t_record,
            t_gpu = ?(t_wait - t_record),
            t_read = ?(t0.elapsed() - t_wait),
            "gpu s submit"
        );
        Ok(res)
    }
}

pub enum Output {
    Host(Vec<u32>),
    Device(DeviceBuf),
}

/// Sum of the driver budgets of the device-local heaps (`VK_EXT_memory_budget`), when the
/// extension is enabled. wgpu enables it where available but does not expose the query.
/// The engine's only `unsafe`: Vulkan interop through wgpu-hal.
#[allow(unsafe_code)]
fn device_local_budget(device: &wgpu::Device) -> Option<u64> {
    use ash::vk;
    // SAFETY: read-only queries on the live device's physical device; the hal device guard
    // keeps the device alive for the duration of the call.
    unsafe {
        let hal = device.as_hal::<wgpu::hal::api::Vulkan>()?;
        if !hal
            .enabled_device_extensions()
            .contains(&ash::ext::memory_budget::NAME)
        {
            return None;
        }
        let instance = hal.shared_instance().raw_instance();
        let mut budget = vk::PhysicalDeviceMemoryBudgetPropertiesEXT::default();
        let mut props = vk::PhysicalDeviceMemoryProperties2::default().push_next(&mut budget);
        instance.get_physical_device_memory_properties2(hal.raw_physical_device(), &mut props);
        let mp = props.memory_properties;
        let mut total = 0u64;
        for i in 0..mp.memory_heap_count as usize {
            if mp.memory_heaps[i]
                .flags
                .contains(vk::MemoryHeapFlags::DEVICE_LOCAL)
            {
                total += budget.heap_budget[i];
            }
        }
        (total > 0).then_some(total)
    }
}

/// Minimal executor for wgpu's futures; they complete after `Device::poll` or immediately
/// on native backends.
fn pollster<F: std::future::Future>(f: F) -> F::Output {
    futures::executor::block_on(f)
}
