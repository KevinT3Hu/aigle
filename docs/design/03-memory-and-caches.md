# aigle — Memory and Cache Design

Status: agreed. Builds on `01-ui-engine-communication.md` and `02-engine-pipeline.md`.

Scope: memory budgets, the size of each pipeline product, the cache tiers (GPU, RAM, disk), residency and eviction, admission control for jobs, prefetch, and the disposable disk cache. The CPU/GPU split is doc 04 and the scheduler is doc 05; this document only defines the memory contract they must respect.

All sizes below are computed from buffer formats. Timings are estimates that must be measured on the reference machine.

## 1. Constraints

- **Minimum spec:** 16 GB RAM with an integrated GPU. **Reference:** Ryzen 7 8845HS (Radeon 780M) with 32 GB.
- **Unified memory.** On an iGPU, "GPU memory" is system RAM. On Linux/Vulkan the 780M exposes two heaps:
  - a device-local heap, which is the BIOS UMA carve-out (often 512 MB–8 GB depending on the laptop)
  - a host-visible heap (GTT) backed by ordinary system RAM

  Both count against the same physical RAM as the CPU side.
- **No double residency.** A buffer lives in one place. After a CPU-produced product (for example a pyramid tile) has been uploaded and no CPU stage still needs it, the CPU copy is dropped. On unified memory, keeping both copies costs real RAM and buys nothing.
- **Professional quality first** (doc 01 §11). Memory pressure may slow aigle down, by evicting and recomputing, but must never lower the quality of Refined/Full renders or exports.
- **GPU API limits.** wgpu's default limits (8192 px texture dimension, 128–256 MB buffer bindings) are below one level-0 image of a 100 MP raw. Tiling (§4) is therefore required, not just an optimization. Adapter limits are requested at device creation, but correctness never depends on them being raised.

## 2. Budgets

### 2.1 Engine memory budget

One budget `B` covers every engine allocation, CPU and GPU together:

```
B = clamp(0.25 × physical RAM, 3 GB, 24 GB)        // GUI process
B = clamp(0.50 × physical RAM, 3 GB, 48 GB)        // CLI process (usually runs alone)
```

At the minimum spec this gives 4 GB (GUI). On the reference machine it gives 8 GB (GUI) and 16 GB (CLI). Both values can be overridden in settings and with `--memory`.

`B` is a soft target. The engine also reacts to the system:

- **Linux PSI** (`/proc/pressure/memory`). When the "some" stall share exceeds a threshold, the effective budget shrinks to 75% of `B` until pressure clears.
- **cgroup limits.** If the process runs under a cgroup memory limit (`memory.max`/`memory.high`), `B` is capped below it.
- **Vulkan budget** (`VK_EXT_memory_budget`), where the driver reports it. wgpu does not expose this query as far as I know; reading it needs `wgpu-hal`/`ash` interop. To be verified in doc 04.

### 2.2 Ledger

Every engine allocation goes through a `MemLedger`. Each entry records:

- **class:** `SourceCpu`, `SourceGpu`, `StageTile`, `Field`, `Asset`, `Transient`, `Frame`
- **owner:** session, view or job
- **size**
- **recompute cost:** an estimate of the time needed to rebuild it

The ledger is the single source of truth for eviction (§5) and admission (§6). A debug HUD shows it live. Native libraries (LibRaw, librtprocess) allocate their own memory, so their peak use is accounted as a `Transient` reservation made before the call (§6).

### 2.3 Disk cache budget

`~/.cache/aigle/` has a default limit of min(20 GB, 10% of the free space on its filesystem). It is configurable, and the user can clear it at any time (doc 01 §9.7).

## 3. Product sizes

`N` is the sensor size in megapixels (MP). GPU-side colour buffers are RGBA, because Vulkan has no generally usable 3-channel float storage format. The CPU side stores RGB.

| Product | Format | Size | 24 MP | 45 MP | 61 MP | 100 MP |
|---|---|---|---|---|---|---|
| CFA (A1/A2) | u16 | 2N MB | 48 MB | 90 MB | 122 MB | 200 MB |
| Demosaic transient (A3, librtprocess float planes) | f32 | ~16–20N MB | ~0.45 GB | ~0.85 GB | ~1.15 GB | ~1.9 GB |
| Source pyramid, CPU (A5, all levels) | RGB f16 | 8N MB | 192 MB | 360 MB | 488 MB | 800 MB |
| Source pyramid, GPU-resident if fully uploaded | RGBA f16 | 10.7N MB | 256 MB | 480 MB | 650 MB | 1.07 GB |
| Same pyramid in f32 | RGBA f32 | 21.3N MB | 512 MB | 960 MB | 1.3 GB | 2.13 GB |
| AI-denoise result (asset, decided f32) | RGB f32 | 12N MB | 288 MB | 540 MB | 732 MB | 1.2 GB |
| f32 source for 16-bit/float export (§9), level 0 only, tiled | RGB f32 | 12N MB | 288 MB | 540 MB | 732 MB | 1.2 GB |

The full pyramid is never uploaded to the GPU. Only the source tiles a request needs are uploaded (§4.2).

Interactive working set for one View on a 4K display (8.3 MP, RGBA16F = 66 MB per full-viewport intermediate):

- 3–4 live intermediates, because fused kernels (doc 02 §6.4, §9.2) keep this number small
- mask field planes
- source-ROI tiles plus margins

This is roughly 0.3–0.6 GB. A "before" View or a snapshot View adds about the same, unless it shares tiles with the main View (doc 02 §11).

## 4. Storage layout

### 4.1 Tiled source pyramid

- A5 stores each pyramid level as 256×256 source-space tiles, in CPU memory or memory-mapped (§7.3).
- Tiles carry a 16 px apron so that the B1 resampling kernel never needs neighbouring tiles.
- Tiles are content-addressed by A-tier key + level + tile index. Every View, job and export of the session shares them.

### 4.2 GPU source tile cache

The GPU keeps a fixed-size atlas of source tiles, which works like a virtual texture:

- A render request computes the set of source tiles it needs (from B1's inverse map, doc 02 §5.1), uploads the missing ones, and references the atlas through an indirection table.
- At fit-to-screen on a 100 MP raw, only a coarse pyramid level is resident (a few tens of MB). At 1:1, only the visible region of level 0 plus margins is resident.
- The atlas size is a share of `B` (default 20%) and grows only when a single request needs more.

### 4.3 Stage tiles

- Crop-space stage outputs are cached per tile (doc 02 §5.2) in the GPU heap.
- Only the outputs worth keeping are cached. Each stage declares a cache policy:
  - **always:** the C1 output (the base for most slider changes) and the final E1 tile per View
  - **never:** fused-kernel interiors
- Everything else is recomputed.

## 5. Residency and eviction

### 5.1 Pinning

The following are pinned and are never evicted:

- anything an in-flight render or job is reading
- the current frame of each View
- AI-produced assets that have not yet been written to disk (§7.2)

### 5.2 Eviction order

Unpinned entries are scored as

```
score = recompute_cost / size × recency_weight × class_weight
```

and evicted lowest score first. `class_weight` follows the scheduler classes (interactive > agent > batch > background). In practice the order is:

1. intermediates of background jobs (thumbnails, batch)
2. stage tiles at other render scales and off-screen tiles of the active View
3. tiles of inactive Views (before/after, snapshots), and agent offscreen renders
4. prefetched neighbour sessions (§8)
5. the CFA of the active session, which can be re-decoded from the file
6. GPU atlas tiles not used by the current viewport
7. pyramid levels of the active session that the current View doesn't use, which are rebuilt from level 0

Level 0 of the active session is evicted last. When it is evicted, it is re-derived from the file, or paged from the memory-mapped spill (§7.3).

Evicting an expensive asset (AI denoise) first writes it to the disk cache, if it isn't already there.

### 5.3 Over-budget behaviour

If the pinned set alone exceeds the effective budget:

- new jobs wait (§6)
- prefetch is cancelled
- the UI shows a "low memory" indicator

Renders already running complete. Quality is never lowered.

## 6. Admission control

- Every job declares an estimated peak memory before it starts, from the §3 formulas. Examples: decode + demosaic of a 100 MP raw needs about 2.1 GB transient. An export tile at f32 needs its tile size × the number of intermediates.
- The scheduler (doc 05) admits a job only if `ledger.used + reserved + peak ≤ effective budget`, after evicting what it can. Jobs that don't fit wait, highest class first.
- **Consequence at the minimum spec:** only one large-raw decode runs at a time, and in a batch, photos are processed one after another rather than in parallel. On the reference machine, the CLI can run 3–6 photos of up to 45 MP concurrently. The exact count depends on the budget.
- A job whose peak exceeds the whole budget, for example a very large raw at the minimum spec, runs alone with the budget temporarily lifted to the job's peak. It is not refused. If the system then runs out of memory, the OS handles it. This is preferable to refusing to open a photo.

## 7. Disk cache (`~/.cache/aigle/`)

### 7.1 Layout

```
~/.cache/aigle/
├── thumbs/      rendered thumbnails of edited photos (WebP, ~256 and ~1024 px)
├── previews/    rendered loupe previews for the library (WebP, long edge = display size)
├── assets/      AI-denoise results (zstd-compressed tiles)
├── source/      optional A-tier spills and demosaic caches (§7.3)
└── index/       search index (SQLite; disposable, rebuilt from sidecars)
```

Every entry is written atomically (temp file + rename) and checksummed, so a corrupt entry is detected and recomputed.

### 7.2 Keys

- **File identity for caching:** `(canonical path, size, mtime_ns)` hashed. Scanning a folder therefore never reads full raw files. When a photo is opened, the full-content BLAKE3 hash is computed during the decode read (the file is read anyway). The cache also stores this hash, so a renamed file can be matched again.
- **Rendered thumbnails and previews:** file identity + `EditParams` hash + `process_version`.
- **AI-denoise assets:** the A2 key + model id + version (doc 02 §10).

### 7.3 Spill and demosaic cache

- **Spill.** When a photo's source pyramid would take more than 50% of the effective budget, its tiles are written to `source/` and accessed through `mmap`. The OS pages them in and out, which gives out-of-core behaviour for very large files without separate code paths. This mainly concerns panoramas, large TIFFs and >100 MP raws at the minimum spec.
- **Demosaic cache.** The A-tier output is written to `source/` only when re-deriving it would take longer than reading it back. Example: X-Trans 3-pass or AMaZE on large files. The engine measures both times on each machine and decides from them. RCD on a 24 MP Bayer file is usually faster to recompute than to read back, but this is an estimate.

### 7.4 Eviction

- LRU by last access, weighted by recompute cost, so AI-denoise assets are evicted last.
- Eviction runs in the background, and when the budget is exceeded by more than 10%.

## 8. Prefetch

- **Develop:** the neighbours of the current photo in the filmstrip, in the direction of travel first. Only the A tier is prefetched (decode, demosaic, pyramid). Prefetch runs at background class and is cancelled when the user navigates elsewhere.
- **How many neighbours** is decided by the admission budget, not a fixed number. Neighbours are prefetched while `ledger.used + prefetch peak ≤ 0.7 × effective budget`. At the minimum spec this typically means one neighbour of ≤ 45 MP. On the reference machine it typically means 2–4.
- **Library grid:** unedited photos use the embedded JPEG previews. Edited photos use cached rendered thumbnails, which are rendered in the background after an edit is committed.

## 9. Precision of stored products

- Shot noise in a raw is always larger than f16 quantization: at full well, relative noise is about 3 × 10⁻³ against an f16 half-ULP of about 2.4 × 10⁻⁴. With black = 0 and white = 1, f16 subnormals still resolve far below 1 DN of a 16-bit raw. f16 storage of the source pyramid is therefore below the noise floor for previews and 8-bit exports.
- After strong denoising, smooth gradients keep less noise. One f16 step can then exceed one 16-bit output code value (about 3–4 codes in midtones).
- **16-bit and float exports use an f32 source (decided).** They re-decode and re-demosaic the photo at f32 into a level-0-only, tiled f32 source (no pyramid needed at full resolution). This costs one decode + demosaic per export (estimated 1–3 s) and about 12N MB extra during the export, admitted like any other job (§6). Previews and 8-bit exports keep using the f16 pyramid.
- **AI-denoise assets are stored in f32 (decided)**, so denoised photos also export at full f32 precision without re-running the model. Previews read the asset through the same f16 pyramid path as any source.
- GPU intermediates: f16 by default, f32 where an op needs it (accumulations, log-domain filters, statistics). The list is fixed in doc 04.

## 10. Testing invariants

- **Budget stress test.** Open, edit, pan and export a mix of 24–100 MP files with a 3 GB budget. Peak RSS + GPU allocation stays within the budget plus the declared transient reservations.
- **Eviction is transparent.** A render after forced eviction of every unpinned entry is identical (bitwise on the CPU path) to a render with warm caches.
- **Spill is transparent.** A spilled photo renders identically to an in-memory one.
- **Cache corruption.** Truncating or corrupting any disk-cache file causes a recompute, never an error or a wrong image.

## 10a. Implementation notes (first implementation; for review)

- **Budget (§2.1).** PSI is polled once per second; "some avg10" above 10 % enters the pressure state (effective budget 75 % of `B`, cache trimmed), below 5 % leaves it. `VK_EXT_memory_budget` is read through wgpu-hal (wgpu enables the extension but has no query). On an integrated GPU device buffers count against `B` (unified memory); on a discrete GPU the ledger gets a separate device pool of the driver's budget, and wgpu is set to refuse allocations above 95 % of it.
- **Ledger (§2.2).** No per-entry recompute cost or owner in the ledger itself; the stage-tile cache keeps both per entry. Admission asks the cache to evict the shortfall before waiting.
- **Cache capacity.** The stage-tile cache may hold 50 % of the effective budget, and never more than what other allocations and admitted reservations leave free.
- **Admission (§6).** Decodes reserve their peak once the header is read (LibRaw sizes before unpacking; image dimensions before decoding pixels), inside the worker task; a job waits while other reservations are outstanding and otherwise runs over budget (Q22). Declared peaks are measured: 24.7–25.4 B/px for 20–24 MP raws (Bayer and X-Trans), 28 B/px for a 16 MP raw; declared as 26 B/px + 2 B per raw px + 32 MiB.
- **CPU pyramid (§4.1).** Levels are contiguous, not 256² tiles with aprons: the CPU kernels address levels directly, the GPU reads them through its own tile atlas, and spilling whole levels gives the out-of-core behaviour of §7.3. Sources are shared by decode identity (file identity + A1–A3 params) across sessions and jobs, as long as one of them holds the source.
- **GPU atlas (§4.2).** One storage buffer of 256² RGBA f16 tile slots (512 KiB each), no apron (the kernel looks tiles up through a per-dispatch slot table). It starts at 128 slots and doubles on demand up to 20 % of `B` (unified) or 40 % of the device budget (discrete); then the least recently used tiles not needed by the current submission are replaced. A batch whose tiles exceed the atlas is split into several submissions. S-processed source ROIs are not in the atlas: they are uploaded per C1 tile.
- **Spill (§7.3).** A pyramid above 50 % of the effective budget moves level by level into files under `<cache>/source/`, mapped with `mmap` and unlinked at once (nothing to clean up, also after a crash). Spilled levels are not charged to the budget.
- **Not implemented:** eviction of unused pyramid levels of the active session (§5.2 item 7), the demosaic disk cache (§7.3).
- **Disk cache (§7).** `thumbs/` and `previews/` hold rendered thumbnails (lossy WebP) keyed by file identity, A-tier params, `EditParams` and the stage implementation versions. Each entry carries a BLAKE3 hash of its payload; a mismatch removes it and re-renders. LRU by file mtime (refreshed on hits) above min(20 GiB, 10 % of free space) + 10 %. Disabled in the CLI process (doc 01 §8).
- **16-bit export (§9).** The f32 level 0 re-runs A1–A3; pixels inside the A4 mask are taken from the f16 reconstruction (A4 is not re-run in f32: its output is an estimate, far coarser than f16).

## 11. Decisions

- **Q19:** 16-bit/float exports re-decode and re-demosaic at f32 (§9).
- **Q19b:** AI-denoise assets are stored in f32 (§3, §9).
- **Q20:** budget formula as in §2.1 (GUI 25% of RAM, 3–24 GB; CLI 50%, 3–48 GB), overridable.
- **Q21:** disk cache default min(20 GB, 10% of free space), configurable (§2.3).
- **Q22:** a job larger than the whole budget runs alone, temporarily over budget; it is never refused (§6).
- **Q23:** demosaic results are cached to disk only when the per-machine measurement shows reading back is faster than recomputing (§7.3).
