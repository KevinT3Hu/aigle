//! aigle image engine: the implementation behind `aigle-engine-api`.
//!
//! Scaffold status (see the design docs in `docs/design/`):
//!
//! | Area | State |
//! |---|---|
//! | Budget, `MemLedger`, admission (doc 03 §2, §6) | implemented: PSI pressure (75 % budget), cgroup cap, device pool from `VK_EXT_memory_budget` on discrete GPUs, decode admission from the raw/raster header (measured peaks), cache reclaim before admission |
//! | Cache keys, RAM stage-tile cache, eviction (doc 02 §11, doc 03 §5) | implemented; capacity follows the effective budget; mmap spill of large pyramids; sources shared across sessions/jobs; disk cache of rendered thumbnails/previews (checksummed, LRU, min(20 GiB, 10 % free)) |
//! | Scheduler (doc 05): classes, delayed tasks, cancellation, class-arbitrated GPU queue / decoders / I/O with starvation guard, CPU yielding at chunk boundaries, GPU chunk sizing from measured cost, dedup by cache key, centre-out tiles, pre-render ring, mid-demosaic cancellation | implemented; inference scheduling and CLI batch pipeline need those components |
//! | A tier (doc 02 §3) | LibRaw decode, black, camconst white levels (bundled RawTherapee table), Bayer + X-Trans hot pixels, CA, RCD/AMaZE/Markesteijn, embedded previews; rendered images (JPEG/PNG/TIFF/WebP/HEIF/AVIF via libvips) at A5 with their ICC profile (lcms2) and EXIF |
//! | A4 highlight reconstruction (doc 02 §7) | sparse (dilated clip mask, pre-A4 tile store), keyed on the 1 %-quantised user WB, re-run without re-decoding; off / inpaint-opposed / guided / colour propagation; CPU (A tier) |
//! | S tier (luma/chroma NR, RL capture sharpening, texture; cached source blocks) | implemented; noise model from DNG NoiseProfile (matched VST), else measured; CPU + GPU (`s_*` kernels, batches shared between both) |
//! | Analysis noise estimate (doc 06 §2) | implemented (same model as the S tier) |
//! | Bundled reference data (`data/`, `cargo xtask data`) | camconst, lensfun v1; upstream, revision, licence and hashes in `data/manifest.toml` |
//! | B1: orientation, straighten, crop, perspective, lensfun distortion/TCA/vignetting (bundled lensfun DB read at runtime, pinned `DataRef` profiles), constrain-crop fill; per-tile warp grid; Lanczos-3 footprint-scaled resampling for Refined/Full, bilinear for Draft | implemented, CPU + GPU |
//! | C1, C2 (dehaze, highlights/shadows, clarity via coarse field), D1 (RGC gamut compression, sigmoid, path to white; not frozen), D2 (curves, OkLab vib/sat, mixer, grading), E1 sRGB/P3 with luminance-preserving gamut fit, E2 | implemented, CPU + GPU |
//! | M local adjustments (brush/linear/radial/luminance+colour range/combine, invert, opacity; all 15 local params; mask overlay) | implemented, CPU + GPU; semantic masks need AI assets |
//! | D3 (post-crop vignette, scale-aware grain), blue-noise dither, Display P3 output, gamut-aware clipping overlay | implemented, CPU + GPU |
//! | C1 camera calibration: DNG dual-illuminant ColorMatrix/ForwardMatrix/CameraCalibration/AnalogBalance; user DCPs (HueSatMap, LookTable); D2 `.cube` look LUTs (tetrahedral); `Engine::import` pins DCPs/LUTs | implemented, CPU + GPU |
//! | Views: latest-wins, Draft → Refined → Full, adaptive Draft scale | implemented |
//! | Offscreen render, analyze (histogram/clipping in a named output profile), thumbnail | implemented |
//! | Export (doc 04 §6): streamed to libvips in tile-row bands, linear-light resize, JPEG/PNG/WebP/TIFF, embedded ICC (sRGB/P3) and EXIF; 16-bit TIFF from an f32 source with f32 intermediates (doc 03 §9), CPU + GPU | implemented |
//! | Views on Current / Pending / Original / Snapshot params | implemented |
//! | GPU backend (doc 04): wgpu/Vulkan + committed rust-gpu SPIR-V, self-test (cached per driver and build), CPU fallback, device re-creation after loss, source tile atlas, resident C1 tiles, batched submits; E2 on the CPU (decided) | implemented |
//! | Auto WB (doc 06 §3.1): 4 estimators, locus constraint, as-shot prior, confidence, region; also in analyze | implemented (CPU, deterministic) |
//! | Scene-linear `sample()` through the S tier | implemented |
//! | AI inference, semantic masks | TODO |

pub mod analysis;
pub mod arbiter;
pub mod awb;
pub mod backend;
pub mod budget;
pub mod cache;
pub mod camconst;
pub mod data;
pub mod detail;
pub mod disk_cache;
pub mod encode;
mod engine;
pub mod icc;
pub mod ledger;
mod lens;
pub mod look;
pub mod masks;
pub mod noise;
pub mod output_space;
pub mod pipeline;
pub mod profile;
pub mod scheduler;
mod session;
pub mod source;
mod view;

use std::sync::Arc;

use aigle_engine_api::{Engine, EngineConfig};

/// Create an engine. Works headless (no window or surface).
pub fn create(cfg: EngineConfig) -> Engine {
    Engine::from_impl(Arc::new(engine::AigleEngine(engine::EngineInner::new(cfg))))
}
