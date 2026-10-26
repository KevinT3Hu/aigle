# aigle — CPU/GPU Split and Compute Backends

Status: agreed; two items wait on prototypes (§11). Builds on docs 01–03.

Scope: which stages run where, how kernels are written, how one kernel source serves both the GPU and the CPU fallback, storage precision per stage, data movement, the GPU device lifecycle, AI inference, and how correctness is tested when the CPU and GPU share code.

## 1. Decisions

- **Q24: GPU kernels are written in Rust and compiled to SPIR-V with rust-gpu.** The same kernel source compiles for the host CPU.
- **Q25: the A tier stays on the CPU** (LibRaw, librtprocess, highlight reconstruction, pyramid build). Tiers S–E run on the GPU.
- **Q26/Q30: AI inference runs on the GPU when that is feasible, otherwise on the CPU** (§7). AI denoise is a normal v1 feature, with progress and cancel.
- **Q27: the CPU fallback runs the same Rust kernels**, multithreaded with rayon. It is also the `--deterministic` path (doc 02 §14).
- **Q28: SPIR-V is prebuilt and committed**, rebuilt by `cargo xtask kernels`, and verified by CI (§9).
- **Q29: GPU-only kernel variants are written in GLSL** and compiled by `glslc` in the same xtask (§3.1).

Status of rust-gpu, checked October 2026: its README still describes it as technically usable but not production-ready, and each release pins a specific nightly toolchain (recent releases pin `nightly-2026-07-03`). §9 lists the mitigations.

## 2. Backends

```rust
pub trait ComputeBackend: Send + Sync {
    fn kind(&self) -> BackendKind;                       // Gpu | Cpu
    fn alloc(&self, desc: &BufferDesc) -> Result<BufHandle, AllocError>;   // goes through MemLedger (doc 03 §2.2)
    fn upload(&self, dst: &BufHandle, src: &HostTile);
    fn dispatch(&self, k: KernelId, args: &KernelArgs, grid: DispatchGrid);
    fn readback(&self, src: &BufHandle) -> Job<HostTile>;
    fn submit(&self) -> SubmitToken;                     // a batch of dispatches; cancellation point
}
```

- **`GpuBackend`:** wgpu on Vulkan. It loads the SPIR-V modules built from `aigle-kernels`.
- **`CpuBackend`:** calls the same kernel functions natively, parallelized over tiles with rayon.

Stages (doc 02 §5.1) only build dispatches against this trait. They never know which backend runs them.

Backend selection:
1. The GPU backend is used if a Vulkan adapter is available and passes a startup self-test (§8.3).
2. Otherwise, and whenever `--deterministic` is given, the CPU backend is used.
3. With several adapters (for example an iGPU plus an eGPU), the high-performance adapter is preferred and the user can override it in settings.

## 3. Kernel crate (`aigle-kernels`)

```
crates/
├── kernels/          # aigle-kernels: #![no_std], depends only on glam + spirv-std
│   ├── src/pointwise/   # C1, D1, D2, E1, mask-field accumulation
│   ├── src/stencil/     # NR, sharpening, texture, resampling (B1)
│   ├── src/pyramid/     # down/up-sampling, Laplacian bands (clarity, tone equalizer)
│   ├── src/reduce/      # histograms, statistics pass
│   ├── src/entry.rs     # #[spirv(compute)] entry points (GPU only)
│   ├── glsl/            # GPU-only GLSL variants (§3.1)
│   └── spv/             # committed SPIR-V: rust-gpu output + compiled GLSL variants
├── kernels-build/    # xtask: rust-gpu (pinned nightly + spirv-builder), glslc, spirv-val
└── engine/           # depends on aigle-kernels (host build, stable Rust) + the built .spv
```

Rules for kernel code:

- It is `#![no_std]`, has no allocation, no panics on the hot path, and no trait objects or recursion. This conservative subset of Rust is the part rust-gpu handles reliably.
- **All math is f32.** Storage precision (f16 or f32) is a property of the buffer, not the kernel (§5). Loads and stores go through accessor traits, implemented once for GPU storage textures and buffers and once for host slices (using the `half` crate for f16).
- The **host build of `aigle-kernels` compiles on stable Rust.** Only the SPIR-V build needs the pinned nightly, so the rest of the workspace stays on stable.
- **Kernel shapes:**

| Shape | Definition | Examples |
|---|---|---|
| Pointwise | `fn(px, params) -> px` | C1, D1, D2, E1 |
| Stencil | reads a neighborhood through a tile accessor with apron | NR, sharpening, B1 |
| Pyramid | one level → next level | downsample, Laplacian bands |
| Reduce | per-workgroup partial results, merged in a fixed order | histograms, statistics |

  Each shape has one CPU driver and one GPU entry-point template. A new kernel only implements the per-element function.

### 3.1 GPU-specialized variants

Some kernels are much faster on the GPU with workgroup shared memory or subgroup operations, for example separable large-radius filters and reductions. These may have a GPU-only variant. The shared portable version stays the reference, and a test (§10) checks that the two agree within tolerance. The CPU backend always runs the portable version.

GPU-only variants are written in **GLSL** (decided, Q29′) and compiled to SPIR-V by the same xtask as the rust-gpu kernels, using `glslc` from shaderc (Q29″). GLSL reaches every Vulkan feature Mesa RADV supports through `#extension`, which is the point of an escape hatch. WGSL is not used, because it is limited to what WebGPU and wgpu expose. The rules for GLSL variants:

- They live in `crates/kernels/glsl/`, one file per variant, next to the Rust kernel they specialize.
- The xtask compiles them, runs `spirv-val` on the output, and writes the `.spv` into the same committed directory as the rust-gpu output. CI rebuilds and diffs them (Q28).
- They are loaded with SPIR-V passthrough, the same as rust-gpu output (§8.1).
- `glslc` is needed only to run the xtask. Normal builds use the committed `.spv`.
- Only Vulkan is targeted. This adds no restriction beyond the rust-gpu path, which uses passthrough as well.

## 4. Stage placement

| Stage | Runs on | Notes |
|---|---|---|
| A1 decode | CPU (LibRaw) | one LibRaw instance per worker thread |
| A2 raw prep | CPU (Rust + librtprocess CA) | |
| A3 demosaic | CPU (librtprocess) | f32 planes internally (doc 03 §3) |
| A4 highlight reconstruction | CPU | sparse, only within the clip mask |
| A5 pyramid build | CPU | output tiles in f16 (previews) |
| f32 export source | CPU | re-derived for 16-bit/float export (doc 03 §9) |
| AI denoise / semantic masks | GPU if feasible, else CPU (ONNX Runtime) | §7 |
| S1 NR, S2 capture sharpening | GPU / CPU fallback | stencil + pyramid kernels |
| B1 geometry resample | GPU / CPU fallback | stencil; Jacobian-scaled kernel |
| M mask fields | GPU / CPU fallback | brush strokes are rasterized as SDF segments in a kernel; no CPU rasterizer on the GPU path |
| C1, D1, D2, D3 | GPU / CPU fallback | C1 and D1+D2+D3 are each one fused pointwise kernel where possible |
| C2 spatial tone/presence | GPU / CPU fallback | pyramid-based, cost independent of radius (doc 02 §5.3) |
| Statistics pass | GPU / CPU fallback | reduce kernels, full resolution (doc 02 §5.4) |
| E1 output transform | GPU / CPU fallback | ICC matrix/TRC exact; LUT profiles via a 65³ table built by lcms2 on the CPU |
| E2 compose, overlays | CPU (decided, see note) | |
| Encoding (JPEG/TIFF/…) | CPU (libvips) | after readback |

Implementation note (I/O): libvips ≥ 8.15 and libexif are system dependencies found with pkg-config (`apt install libvips-dev libexif-dev`; libvips brings lcms2 and its libheif module). Rendered images load through libvips with their ICC profile (lcms2) and EXIF; HEIF/AVIF colour given only as an nclx box (no ICC) is read as sRGB. Exports render one tile row at a time on demand from libvips (no E2: at render scale 1 grid pixels are output pixels), which resizes in linear light (Lanczos-3) and encodes JPEG, PNG, WebP or TIFF with an embedded sRGB/P3 profile and EXIF. 16-bit TIFF runs B1–E1 from the f32 source with f32 tiles on CPU and GPU (`c1_tile` / `out_tile` full-precision mode, `s_*` with an f32 source); 8-bit outputs and previews are unchanged.

Implementation note (E2): E2 runs on the CPU over read-back output tiles. It costs 3–15 ms of a ~1 s 1:1 render, and the read-back tiles are what panning reuses from the RAM cache; GPU-resident output tiles would make every pan read the viewport back.

Implementation note (S tier): a batch of S blocks is shared between the CPU and the GPU at the same time, taking blocks from one queue (the GPU in chunks). On an integrated GPU, which shares memory bandwidth and power with the CPU, the CPU takes most blocks; on a discrete GPU the GPU does. Blocks return to the host because they are cached and assembled into source ROIs.

## 5. Storage precision

Kernels compute in f32. The table lists what is *stored* between stages.

| Buffer | Preview / 8-bit export | 16-bit / float export |
|---|---|---|
| Source pyramid tiles | RGBA f16 | f32 source, level 0 only (doc 03 §9) |
| AI-denoise asset | f32 (doc 03 §3) | f32 |
| S-tier outputs | f16 | f32 |
| B1 output (Crop grid) | f16 | f32 |
| Mask alphas | r8unorm (brush, gradients); r16 for semantic assets | r16 / f32 |
| Mask fields (per parameter) | f16 | f32 |
| C1 output (cached, doc 03 §4.3) | f16 | f32 |
| Pyramid bands in C2 | f32: band differences lose too much precision in f16 | f32 |
| Statistics and histogram accumulators | u32 / f32, fixed-order merge | same |
| D-tier outputs (bounded [0,1]) | f16 | f32 |
| Final frame | BGRA8 (display) | RGBA16 / f32, then encoded |

Doc 02 §14 sets the tolerance. If the GPU-vs-CPU tests fail it for a preview buffer, that buffer moves to f32. The thresholds are not loosened.

## 6. Data movement

- **Uploads.** Source tiles are written into the GPU tile atlas (doc 03 §4.2) through `queue.write_texture` from the CPU tile. The CPU copy is dropped once no CPU stage needs it (doc 03 §1).
  - On unified memory a true zero-copy import (Vulkan host-pointer import) would avoid this copy. wgpu does not expose that as far as I know. It can be added later through `wgpu-hal` if profiling shows the uploads matter; the copy is bandwidth-bound and small at preview scales.
- **Readback.** Frames for GPUI are read back with `map_async` into pooled buffers, as decided in doc 01 §6. GPUI on Linux renders with its own GPU context. aigle's engine uses a separate wgpu device and does not share textures with it.
- **Export.** Tiles are read back one by one and streamed to libvips, so the whole output image never has to be resident on the GPU.
- **Command batching.** All dispatches for one tile and revision are recorded into one command buffer. Cancellation (doc 01 §4) takes effect between submissions. A submission is sized at a few milliseconds of GPU work, so a stale render stops quickly.

## 7. AI inference (GPU when feasible, CPU otherwise)

### 7.1 Backend selection

```rust
pub trait InferenceBackend: Send + Sync {
    fn kind(&self) -> InferenceKind;                          // Gpu(name) | Cpu
    /// Can this backend run this model fully, on this machine?
    fn probe(&self, model: &ModelId) -> Result<ProbeReport, ProbeError>;
    fn run(&self, model: &ModelId, input: TensorTile, ctx: &JobCtx) -> Job<TensorTile>;
}
```

- **Candidates, in order:** one GPU backend (chosen by the prototype in §7.3), then ONNX Runtime with the CPU execution provider (`ort` crate), which is always available.
- **Selection is per model**, and the result is cached per (model id + version, driver version, aigle version). A GPU backend is used for a model only if all of the following hold:
  1. A usable GPU adapter exists, and the GPU compute backend has not been disabled (§8.2, §8.3).
  2. The backend can run the whole model on the GPU. A backend that would silently move some operators to the CPU counts as not feasible when that makes it slower than the CPU path.
  3. A one-time self-test on a small synthetic tile matches the CPU backend within a per-model tolerance. Model outputs are not required to be bitwise equal across backends.
  4. The model's memory peak fits the budget (doc 03 §6). If it doesn't fit even at the smallest tile size, the CPU backend is used.
- **Runtime fallback.** If the GPU backend fails during a job (device lost, out of memory, driver error), the job restarts on the CPU from its last completed tile. That model stays on the CPU for the rest of the session. The user sees which backend ran in the job's progress view.
- **Determinism.** `--deterministic` forces the CPU backend. Assets record which backend produced them; the asset key (doc 02 §10) does not include the backend, because results agree within the model tolerance.
- Users can override the choice per model in settings (Auto / GPU / CPU).

### 7.2 Execution

- **Threads (CPU backend).** Inference uses an intra-op thread pool capped at (physical cores − 2). That leaves the UI, the render worker and LibRaw responsive while a model runs. The scheduler (doc 05) runs at most one inference job at a time.
- **AI denoise** runs tiled with an overlap and blended seams, at full resolution, producing the f32 asset (doc 03 §9). Its input is the A2 CFA for Bayer raw models, or the A3 output for RGB models (doc 06 §4).
  - Its duration on the 8845HS is unknown until measured. On the CPU it is likely tens of seconds to minutes for 45 MP, depending on the model; the GPU path should be substantially faster.
  - The UI shows progress, and the job can be cancelled.
  - Results persist in the disk cache, so the cost is paid once per photo and model version.
- **Semantic masks** run at model resolution (around 1024 px) from a standardized render, so they are fast in comparison.
- Models and their weights are checked for AGPL-compatible licenses one by one (doc 01 §11).
- The GPU inference backend runs its own GPU work. Its memory is reserved in the ledger before each job, like native-library transients (doc 03 §2.2), and it is scheduled so it never runs concurrently with interactive renders on the same GPU (doc 05).

### 7.3 Choosing the GPU backend (prototype)

The GPU backend is chosen by prototyping on the reference machine (Radeon 780M, Linux, Mesa RADV) with the real candidate models:

| Candidate | Notes |
|---|---|
| ONNX Runtime + WebGPU execution provider (Dawn on Vulkan) | same model files and API as the CPU path; a second GPU stack in the process |
| Burn on wgpu/Vulkan (ONNX import at build time) | pure Rust, same ecosystem as the engine; operator coverage limits model choice |
| Reserve: denoise network written in `aigle-kernels` | only if both fail for the chosen model |

ROCm/MIGraphX is excluded, because the 780M is not officially supported. Criteria: the share of the model that runs on the GPU, speed, peak memory, output agreement with the CPU backend, and packaging cost. The choice is recorded here when the prototype is done. Until then the CPU backend is the only implementation.

## 8. GPU device lifecycle

### 8.1 Creation

- The device is created headless (no surface). It requests the adapter's real limits rather than the defaults, but correctness never depends on raised limits (doc 03 §1).
- Required features: storage textures in `rgba16float` and `rgba32float`, plus SPIR-V passthrough on Vulkan if naga's SPIR-V frontend does not accept rust-gpu's output for all kernels. First prototype (rust-gpu 0.11.0 / `nightly-2026-08-15`, wgpu 30.0.1): naga's SPIR-V frontend accepts the `c1_tile` and `out_tile` kernels, so passthrough is not needed yet; re-check as kernels are added. Kernels must avoid 8-bit integers (`OpCapability Int8` is not universal) and `f32::clamp` (adds a panic path); `aigle-kernels` uses `u32` and `max().min()` instead.

### 8.2 Device loss

On `DeviceLost` (driver reset, GPU hang), the engine:
1. drops all GPU-side cache entries from the ledger
2. recreates the device
3. re-uploads only what the next request needs, and replays in-flight requests

If recreation fails twice, the engine switches to the CPU backend for the rest of the session and reports this through `Engine::events()`. Edits are never lost, because the sidecar is the source of truth.

### 8.3 Startup self-test

At startup, every kernel runs on a small synthetic tile on the GPU and is compared with the CPU backend. If a kernel fails the tolerance (a driver or compiler bug), the GPU backend is disabled. The user is told, and the CPU backend is used. The results are cached per (driver version, aigle version), so the test does not run on every launch.

## 9. rust-gpu risks and mitigations

| Risk | Mitigation |
|---|---|
| Not production-ready; compiler bugs | Kernels use a conservative subset of Rust (§3). The startup self-test (§8.3) catches miscompilations on the user's machine. CPU-vs-GPU tests run in CI. |
| Pinned nightly toolchain | Only the SPIR-V build uses it. The host build and the rest of the workspace use stable. |
| Contributors without the nightly setup | Decided (Q28): prebuilt `.spv` files are committed and rebuilt with `cargo xtask kernels`. CI rebuilds them with the pinned toolchain and fails if they differ. Normal builds never run rust-gpu. |
| Missing feature (e.g. a subgroup op or storage format) | The kernel gets a GLSL GPU-specialized variant (§3.1). The portable Rust version stays the reference and the CPU path. |
| Project stalls | The kernels are plain Rust functions. Moving to GLSL means re-writing only the GPU entry points, not the algorithms or the CPU path. |

## 10. Testing

Shared source changes what the CPU-vs-GPU test can catch:
- **It catches** backend differences: compiler bugs, f16 storage effects, FMA contraction, transcendental precision, reduction order.
- **It does not catch** algorithm bugs, because both backends run the same algorithm.

Algorithm correctness therefore needs independent checks:

- **Analytic tests per kernel:**
  - neutral params give identity, within f32
  - known inputs give known outputs, e.g. a matrix applied to primaries, or a sigmoid at mid-grey
  - monotonicity of tone ops
  - range invariants such as D1 output in [0,1]
- **Property tests** (`proptest`) over random params for the doc 02 §4 contracts: clamping, no NaN or Inf, and cache key completeness.
- **Reference comparisons** where an equivalent exists: demosaic and highlight reconstruction against RawTherapee output, and ICC conversions against lcms2 run directly on the CPU.
- **Golden images** per process version (doc 02 §16).
- **CPU vs GPU** within tolerance, per kernel and per whole pipeline, on the reference machine's 780M in CI or a nightly hardware runner.
- **Specialized vs portable** GPU variants within tolerance (§3.1).

The CPU backend must be bitwise deterministic for the same binary (doc 02 §14):
- Rust does not contract `a*b+c` into FMA unless asked, and kernels don't use fast-math.
- Reductions merge partial results in a fixed tile order.
- rayon is used only over independent tiles, never for floating-point reductions with variable order.
- SIMD use is limited to what the compiler auto-vectorizes at the pinned target level (`x86-64-v3` on release builds, with a `v2` build for older CPUs).

## 11. Open items

- Choice of GPU inference backend and of the AI models, after the prototype on the reference machine (§7.3, doc 06).
- Whether naga's SPIR-V frontend accepts all kernels, or passthrough is required everywhere (§8.1). The first two kernels load through naga (§8.1); keep checking as kernels are added.
