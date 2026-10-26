# aigle — UI ↔ Engine Communication Design

Status: draft, agreed at architecture level. Engine internals are covered in a separate document.

aigle is a non-destructive RAW photo editor written in Rust. The UI uses GPUI. The image engine wraps mature native libraries (LibRaw, librtprocess, lcms2, lensfun, libvips, ONNX Runtime) and runs its interactive stages in its own wgpu shaders. It is designed to be driven by the user, by a built-in LLM agent, and by external agents.

## 1. Principles

1. **The engine is a pixel service.** It owns decoded data, stage caches, GPU resources and pixels. It does not know about history, the library or the UI.
2. **Clients declare state; the engine converges on it.** Clients send "this image, these params, this viewport" instead of imperative commands. The engine diffs against what it already has, recomputes only the dirty stages, cancels stale work and returns results tagged with the revision they were rendered from.
3. **Edit state lives in a UI-agnostic document layer (`doc`).** The GPUI app and the built-in agent change edits only through `doc` commands, and neither calls the engine's `set_edits` directly. Undo, history, persistence and live preview therefore behave the same whatever the source of the edit. The stateless CLI (§8) is the exception: it builds params from a profile and passes them straight to the engine.
4. **Only `app` depends on GPUI.** Everything else, including the engine, runs headless. CI enforces this by building `cli` without GPUI.
5. **Every message is serializable** (`serde`). In-process calls never serialize, but serializability keeps three options open: a subprocess engine (crash isolation from native code), a JSON CLI, and MCP.
6. **No database. The XMP sidecar next to each photo is the only source of truth** (§9). Everything under `~/.cache/aigle/` is disposable and can be rebuilt from the photos and their sidecars.

## 2. Crate layout

```
crates/
├── core/        # EditParams, ParamPath, JSON Schema (schemars), geometry, color types
├── profile/     # editing profile format, parse/validate, ops → per-photo params
├── engine-api/  # interface only: handles, requests, events, Frame
├── engine/      # implementation of engine-api (native libs, wgpu, caches, workers)
├── kernels/     # aigle-kernels: no_std Rust kernels, built for host (CPU backend) and SPIR-V via rust-gpu (doc 04)
├── doc/         # per-photo document: history, pending proposals, rebase
├── library/     # folder scan, XMP sidecar + mask file read/write, file watching,
│                # disposable cache (~/.cache/aigle: thumbnails, previews, search index, AI results)
├── tools/       # tool registry: schema + risk level + handler
├── agent/       # LLM loop, provider adapters, approval gate
├── cli/         # `aigle` binary, no GPUI
├── mcp/         # later: thin MCP adapter over tools/
└── app/         # GPUI: canvas, panels, chat panel, review UI
```

```
GPUI app ──commands──┐
                     ├──► doc (EditParams, history, proposals) ──set_edits──► engine
agent ───tools───────┘        │   ▲                                            ▲
         └── tools also call ─┼───┴── library              analyze / offscreen render
                              └── debounced write ──► XMP sidecar (+ masks file)

CLI (stateless) ──► profile ──► engine     (no doc, no library, never reads/writes XMP)
```

Ownership:

| Concern | Owner |
|---|---|
| Edit params, in-memory undo history, pending proposals | `doc` |
| Selection, panel state, view layout | `app` |
| Folders, sidecar/mask-file I/O, ratings, labels, keywords, disposable cache | `library` |
| Decoded data, stage caches, GPU resources, pixels | `engine` |
| Pure math: geometry, coordinate mapping, param schema | `core` |

`engine-api` is a separate crate so that the UI can be developed against a mock engine, and so the engine can later be moved into a subprocess.

## 3. Engine API

There are two communication styles:

- **Continuous state** (the open image's views) flows through latest-value `watch` channels. These never queue.
- **One-shot jobs** (thumbnails, exports, sampling, masks, analysis, offscreen renders) return a `Job<T>`. Dropping the `Job` cancels it.

No call blocks the caller.

```rust
pub struct Engine { /* Arc<Inner> */ }

impl Engine {
    pub fn new(cfg: EngineConfig) -> Self;            // works headless (no window/surface)

    /// Async load; returns immediately. Dropping the Session releases its caches.
    pub fn open(&self, src: SourceRef) -> Session;

    pub fn thumbnail(&self, src: SourceRef, max_px: u32, prio: Priority) -> Job<Thumbnail>;
    pub fn export(&self, src: SourceRef, params: Arc<EditParams>, opts: ExportOptions) -> Job<ExportResult>;

    /// Global events: device lost, memory pressure, engine-wide errors.
    pub fn events(&self) -> EventReceiver;
}

pub struct Session { /* id + engine ref */ }

impl Session {
    pub fn info(&self) -> watch::Receiver<LoadState>;          // Loading → Ready(ImageInfo) | Failed
    pub fn set_edits(&self, rev: Revision, params: Arc<EditParams>, hint: Interaction);  // called by doc only; Continuous | Final (doc 05 §4.1)

    /// A View is one rendered output: main canvas, "before" pane, navigator, loupe…
    pub fn create_view(&self, spec: ViewSpec) -> View;

    // One-shot queries
    pub fn sample(&self, at: ImagePoint, radius: f32, stage: SampleStage) -> Job<PixelSample>;
    pub fn auto_white_balance(&self, region: Option<ImageRect>) -> Job<WhiteBalance>;
    pub fn neutral_white_balance(&self, at: ImagePoint, radius: f32) -> Job<WhiteBalance>; // WB picker (doc 06 §3.1)
    pub fn compute_mask(&self, req: MaskRequest) -> Job<MaskAssetId>;   // AI/semantic masks, cached

    // Agent-oriented (also used by CLI)
    /// Render arbitrary params without touching doc state or any View (e.g. candidates).
    pub fn render_offscreen(&self, params: Arc<EditParams>, opts: OffscreenOpts) -> Job<EncodedImage>;
    /// Quantitative image facts. Overview precision by default; `what.exact = true` for full resolution (doc 06 §2).
    pub fn analyze(&self, params: Arc<EditParams>, what: AnalysisSet) -> Job<Analysis>;
}

pub struct View { /* … */ }

impl View {
    pub fn set_viewport(&self, vp: Viewport, hint: Interaction);  // Continuous during pan/zoom gestures
    pub fn set_params_source(&self, src: ParamsSource);
    pub fn set_overlays(&self, o: Overlays);                     // clipping, mask overlay, focus peaking
    pub fn frames(&self) -> watch::Receiver<Option<Arc<Frame>>>; // latest-wins
    pub fn histogram(&self) -> watch::Receiver<Option<Arc<Histogram>>>;
}
```

### Key types

```rust
pub struct Viewport {
    pub region: ImageRect,          // visible area in post-crop image coordinates
    pub output_size: (u32, u32),    // physical canvas pixels
    pub display: DisplayProfileId,  // engine performs the display color transform
}

pub enum Interaction {
    Continuous,                      // part of a gesture (slider drag, brush stroke, pan): render Draft
    Final,                           // gesture ended / discrete change: render Refined immediately
}

pub enum ParamsSource {
    Current,                         // committed head
    Pending,                         // committed ⊕ pending proposal (review mode)
    Original,                        // defaults / as-shot, framed like the head (its geometry and lens)
    Snapshot(Revision),
}

pub struct Frame {
    pub rev: Revision,               // revision that produced it
    pub viewport: Viewport,          // what it actually covers (may lag the request)
    pub quality: Quality,            // Draft | Refined | Full
    pub format: PixelFormat,         // Bgra8 initially; Rgba16F later
    pub pixels: PixelBuffer,         // pooled Arc buffer; later possibly a GPU texture handle
}

pub struct OffscreenOpts {
    pub long_edge: u32,              // default ~1024 to limit vision-token cost
    pub format: EncodedFormat,       // JPEG by default
    pub before_after: bool,
}

pub struct Analysis {
    pub histogram: HistogramStats,   // percentiles, mean luminance per channel
    pub clipping: ClipStats,         // % clipped highlights/shadows per channel
    pub wb_estimate: Option<WhiteBalance>,
    pub noise_estimate: Option<f32>,
    // No scene detection or scores (decided, Q42): the agent inspects render_preview images itself.
}
```

## 4. Engine behavior contract

- **Latest-wins.** Each View holds one slot of target state. A new request replaces the old one, and in-flight work is cancelled at the next stage boundary.
- **Progressive refinement.** During interaction the engine renders a `Draft` frame (reduced resolution, expensive stages skipped). After about 150 ms of stable state it renders `Refined`, and `Full` at 1:1 zoom.
- **Bounded memory.** Results are published on `watch` channels, so a slow consumer only ever sees the newest frame.
- **Instant pan and zoom.** Each frame records the region it covers. While a new render is in progress, the UI reprojects the old frame. The geometry math for this lives in `core`, so the UI maps cursors and overlays without engine round-trips.
- **Display transform in the engine.** Frames are display-referred for the given `DisplayProfileId`, because GPUI does not color-manage images.
- **Priorities**, highest first:
  1. interactive views
  2. agent offscreen renders / analysis
  3. batch (CLI profile apply/adapt)
  4. background thumbnails / exports
- **Errors.** Per-session failures (unsupported camera, corrupt file) are reported as `LoadState::Failed`. Engine-wide failures go to `Engine::events()`.
- **Runtime-agnostic.** Channels come from `futures` / `tokio::sync` primitives that don't need the tokio runtime, so they work with GPUI's executor.

## 5. Document layer (`doc`)

`doc` holds the edit state for each photo. It accepts patches from any writer and pushes full `EditParams` snapshots to the engine.

```rust
pub struct EditCommand {
    pub photo: PhotoId,
    pub base_rev: Revision,          // what the writer last saw
    pub ops: Vec<ParamOp>,
    pub origin: Origin,              // User | Agent { run_id, step } | Preset | Profile | ExternalReload
    pub label: String,               // shown in history
}

pub enum ParamOp {
    Set   { path: ParamPath, value: Value },     // "light.exposure" = 0.3
    Add   { path: ParamPath, delta: f64 },       // relative change
    Reset { path: ParamPath },
    AddMask { mask: MaskSpec },
    RemoveMask { id: MaskId },
}

pub struct PhotoDoc {
    head: Arc<EditParams>,           // persisted to XMP
    snapshots: Vec<NamedSnapshot>,   // persisted to XMP
    history: History,                // undo/redo, in memory only, dropped when the photo is closed
    pending: Option<Proposal>,       // in memory only; at most one open proposal per photo
}

pub struct Proposal {
    id: ProposalId,
    base_rev: Revision,
    ops: Vec<ParamOp>,
    rationale: String,               // agent's explanation, shown in review UI
    origin: Origin,
}
```

- **Merging.** Ops are field-level, so concurrent edits to different fields merge. If an op touches a field changed after its `base_rev`, it is rejected and the result includes the current values so the writer can retry.
- **Masks.** Brush and gradient masks are stored as vectors inside the params and rasterized by the engine, so undo covers them. Semantic and AI masks are engine jobs that produce a `MaskAssetId`. The params reference that id, and `library` persists the mask pixels in the photo's masks file (§9).
- **Engine sync.** When the head changes (or the pending layer, for views on `ParamsSource::Pending`), `doc` calls `Session::set_edits(rev, params, hint)`, passing `Continuous` during gestures and `Final` otherwise. Snapshots are small and `Arc`-shared.
- **Persistence.** Each committed change schedules a sidecar write, debounced to about 1 s (§9.3). Undo history and pending proposals are never written. Closing a photo with a pending proposal discards it, after confirmation.
- **Applying a profile in the GUI** replaces all of the photo's edits, including crop, rotation, perspective and lens settings, as one undo step.
- **History panel.** `history()` lists every entry oldest first, applied ones followed by undone ones that can still be redone; `goto(position)` undoes or redoes to any of them as a single new revision, so the engine renders once.

## 6. GPUI integration

One canvas entity per engine View:

```rust
struct DevelopCanvas {
    view: engine_api::View,
    current: Option<(Arc<Frame>, Arc<RenderImage>)>,
}

let mut frames = view.frames();
cx.spawn(async move |this, cx| {
    while frames.changed().await.is_ok() {
        let frame = frames.borrow().clone();
        this.update(cx, |canvas, cx| {
            canvas.current = frame.map(|f| (f.clone(), to_render_image(&f)));
            cx.notify();
        })?;
    }
    anyhow::Ok(())
}).detach();
```

Slider drag:

1. The slider sends an `EditCommand` to `doc`. `doc` commits it (or merges it into the current drag-coalesced history entry) and bumps the revision.
2. `doc` calls `session.set_edits(rev, params, Interaction::Continuous)`. This is cheap and happens on every mouse move.
3. The engine diffs the params, finds the dirty stages, renders a `Draft` from cached upstream results, and publishes it.
4. The canvas receives the frame, calls `notify`, and GPUI repaints.
5. When the slider is released, `doc` sends the final value with `Interaction::Final` and the engine renders `Refined` immediately (doc 05 §4.2).

Displaying frames: initially the engine outputs display-transformed BGRA8, which is wrapped or copied once into a GPUI image. GPUI has no supported path for external GPU textures. A shared-texture path, if it is ever needed, would change only `PixelBuffer` and `to_render_image`, behind a `PreviewRenderer` abstraction.

Before/after: `\` switches the main View to `ParamsSource::Original`. The split comparison (`Y`) adds a second View on `Original` that shares the main View's viewport; the canvas paints it left of a draggable divider and the head right of it. `Original` is the unedited photo with the head's geometry and lens correction (`EditParams::original_framing`), so both halves show the same region after a crop or straightening; `set_edits` retargets `Original` views only when those change, so the before half costs nothing while editing.

Crop tool (`R`): the main View is switched to `View::set_uncropped(true)`, which renders the whole straightened frame (viewport regions are then in that frame) while its histogram still describes the cropped image; an edit that only moves the crop does not re-render it. The canvas draws the crop rectangle, thirds and handles; dragging a handle or the inside commits `geometry.crop`, dragging outside commits `geometry.angle`, each drag one gesture. The crop is kept inside the image: handle drags stop at the image edge, and straightening or perspective changes that do not set the crop themselves refit the crop taken at the gesture start (scaled about its centre, aspect kept), so it grows back when the angle returns. Quarter turns and mirroring change `orientation_delta` and transform the crop and every mask shape with it. Aspect presets are stored in `geometry.aspect` and lock handle drags.

Mask tool (`M`): the panel adds linear, radial and brush adjustments and edits the selected one (opacity, invert, radial feather, the local parameters); the canvas draws its handles and dots for the others. A new gradient is placed by dragging; brush strokes append points while dragging (spacing ≈ 20 % of the brush radius). Shapes are edited as `Set masks.<i>.shape` in one gesture; `O` tints the selected mask through `Overlays::mask_overlay`. Mask handles are mapped with `GeometryMap` without the lens profile, so with strong distortion correction they are approximately placed.

Export (`Ctrl+E`): a dialog for format (JPEG/PNG/TIFF 8/16/WebP), quality, long edge and sRGB/Display P3, then the system save dialog; the job runs at background class and reports completion as a notification. In the grid it exports the selected photo with its sidecar edits.

Thumbnails: the grid requests them with a priority and drops the `Job` when a cell scrolls out of view. Unedited photos use the embedded JPEG preview. Rendered thumbnails for edited photos are kept in `~/.cache/aigle/`, keyed by the raw file plus the edit params hash.

## 7. Agent integration

### Approval policy

```rust
pub enum ApprovalPolicy {
    ReviewEachStep,   // every mutating tool call waits for the user
    TrustAll,         // auto-commit; user can undo afterwards
}

pub enum Risk { ReadOnly, Edit, Metadata, WritesFiles, Bulk }
```

- `ReadOnly` tools never wait for approval.
- Under `ReviewEachStep`, every other tool call becomes a `Proposal` and suspends on an approval future.
- Under `TrustAll`, it commits immediately.
- An optional hard limit (e.g. "ask before writing more than N files") exists in trust mode and is off by default.

While a proposal is pending, the canvas shows committed ⊕ pending (`ParamsSource::Pending`), with before/after available. The user can respond in four ways:

- **Approve.** The proposal is committed as one history entry, labelled with its rationale.
- **Reject (optional comment).** The model receives `rejected: <comment>` as the tool result.
- **Tweak then approve.** The user's final values are committed, and the tool result reports what the user changed.
- **Approve all remaining.** The run switches to `TrustAll`.

If the user edits the photo while a proposal is pending, the proposal is rebased. Conflicting fields are flagged in the review UI, never applied silently.

Undo granularity: one approved step equals one undo entry in review mode. In trust mode, one agent turn equals one undo entry, which can be expanded in the history panel.

### Tool registry (`tools`)

The registry is shared by the built-in agent, the CLI and (later) MCP. Each tool has a JSON Schema, a risk level and a handler. Tools are coarse and semantic rather than one per slider.

| Tool | Backed by | Risk |
|---|---|---|
| `get_photo_state` | doc + engine | ReadOnly |
| `analyze_photo` | engine `analyze` | ReadOnly |
| `render_preview` | engine `render_offscreen` (current or candidate params) | ReadOnly |
| `apply_edits` | doc `EditCommand` / `Proposal` | Edit |
| `create_mask` | engine `compute_mask` + doc | Edit |
| `list_presets` / `apply_preset` | profile files / doc | ReadOnly / Edit |
| `search_photos` | library (cache search index over XMP) | ReadOnly |
| `set_rating` / `add_keywords` | library (XMP standard fields) | Metadata |
| `copy_edits_to` | doc | Bulk |
| `export_photos` | engine `export` | WritesFiles |

The `EditParams` schema is generated from the Rust types with `schemars`, including units, ranges and descriptions. It is the single source of truth for agent documentation, validation and clamping.

### Agent runtime (`agent`)

- Runs on a background executor and streams tokens and tool events to the GPUI chat panel.
- Hosted providers only (Anthropic and OpenAI first) behind a `Provider` trait. Local models are out of scope, so reliable tool calling and vision input can be assumed.
- Automatic context: the active photo, the selection and a summary of non-default params. Images are sent only through `render_preview`.
- Typical loop: analyze → propose → render a candidate offscreen → inspect → refine → `apply_edits`.
- Cancelling a run cancels its jobs and discards its pending proposals.

## 8. Editing profiles and CLI (`profile`, `cli`)

A profile is a portable, versioned look. It is accompanied by a free-form Markdown document describing intent, what to preserve and how to adapt to different scenes. Ops can be absolute or relative, and masks are semantic, so a profile transfers between photos.

```toml
# golden-hour.profile.toml
version = 1
name = "Golden hour warm"
doc = "golden-hour.md"

[[ops]]
set = { "wb.temperature" = 5600, "wb.tint" = 8 }
[[ops]]
add = { "light.shadows" = 25, "light.highlights" = -30 }

[[masks]]
target = "sky"                    # resolved per photo via engine compute_mask
add = { "saturation" = 10, "exposure" = -0.3 }   # LocalParams paths

[constraints]
max_highlight_clip_pct = 0.5
preserve_skin_tones = true
```

CLI:

```
aigle profile apply  --profile p.toml photos/*.CR3            # deterministic, no LLM
aigle profile adapt  --profile p.toml --doc p.md photos/*     # built-in agent adapts per photo
                     [--propose out.proposals]                # write proposals for later GUI review
aigle profile schema                                          # JSON Schema + docs for agents
aigle profile check  --profile p.toml

# Low-level tool surface (JSON in/out), same registry as the agent:
aigle analyze  photo.CR3 [--params p.json]
aigle render   photo.CR3 --params p.json --size 1024 -o out.jpg
aigle export   photo.CR3 --params p.json --preset web -o out/
```

- **The CLI is stateless.** It never reads or writes XMP sidecars, masks files or the cache. It writes only the output files named on the command line.
- Profiles start from defaults (as-shot values). Any existing edits of the photo are ignored. Relative ops (`add`) apply on top of those defaults.
- `apply` is reproducible and works offline.
- `adapt` starts from `apply`'s result and runs in trust mode. With `--propose`, it writes proposals to the named file, which the GUI can later open as pending reviews.
- Semantic masks required by a profile are computed per run. In the CLI they are not cached across runs, because the CLI does not use the cache.

External agents have three tiers:

1. Write a profile and doc, then call `apply` or `adapt`.
2. Script the JSON subcommands.
3. Use MCP over the same registry (later).

## 9. Persistence (`library`)

### 9.1 Files

| Location | Contents | Status |
|---|---|---|
| `IMG_0001.xmp` next to the photo | standard XMP (rating, color label, keywords, title, …) plus aigle's edits and named snapshots in the `aigle:` namespace | **source of truth** |
| `IMG_0001.aigle-masks` next to the photo | semantic/AI mask pixels (compressed, source-space, at model resolution), addressed by `MaskAssetId` | source of truth |
| `~/.local/share/aigle/data/` | user-imported large data (DCPs, look LUTs), content-addressed; bundled data ships with the app | referenced by hash from XMP |
| `~/.local/share/aigle/fallback-sidecars/` | sidecars for photos in read-only folders (§9.4) | source of truth, not portable |
| `~/.cache/aigle/` | thumbnails, previews, search index, AI denoise results, demosaic caches | disposable |

- **Photo identity** is the file plus its sidecar. Moving or renaming both together keeps the edits. Moving the raw without its sidecar loses them.
- **Sidecar naming.** aigle reads both `IMG_0001.xmp` and `IMG_0001.CR3.xmp`. It writes `IMG_0001.xmp`, unless another photo in the same folder has the same stem (e.g. `IMG_0001.CR3` + `IMG_0001.JPG`), in which case it writes `IMG_0001.CR3.xmp`. The masks file follows the same stem as the sidecar.
- **Embedded XMP** (DNG, JPEG, TIFF, HEIF) is read but never written. aigle always writes a separate sidecar and never modifies an original file.

### 9.2 What goes in the XMP

- Standard fields (`xmp:Rating`, `xmp:Label`, `dc:subject`, …): read and written for interop with other apps.
- `aigle:` namespace: schema version, the current `EditParams`, named snapshots, and pinned external data (§9.5).
- Undo history is not stored. It lives in memory and is lost when the photo is closed.
- Fields and namespaces written by other apps are preserved byte-for-byte where possible (round-trip via Exiv2).

### 9.3 Writing

- A write is scheduled about 1 s after each committed change (debounced), plus a flush on closing the photo and on app exit.
- Writes are atomic: write a temp file in the same directory, `fsync`, rename. A crash never leaves a half-written sidecar.
- Before writing, aigle checks the sidecar's modification time and content hash against what it last read. If the file changed externally, the write is not done and §9.6 applies.

### 9.4 Read-only folders

If a photo's folder is not writable (SD card, NAS without permission), aigle keeps its sidecar in `~/.local/share/aigle/fallback-sidecars/`, keyed by the photo's content hash, and shows a warning that these edits are not portable. An "Import/copy to…" action copies photos to a writable location and moves their fallback sidecars next to them.

### 9.5 Pinned external data

- Small data (lens correction parameters, camera matrices, noise profile) is embedded in the XMP once resolved.
- Large data (DCPs, look LUTs) is referenced by content hash and looked up in the bundled data and `~/.local/share/aigle/data/`. If it is missing, aigle renders without it and shows a warning naming the missing item.

### 9.6 External changes

`library` watches open folders (inotify). When a sidecar of an open photo changes on disk:
- If aigle has no unwritten change for that photo, it reloads silently. The reload is a `doc` command with origin `ExternalReload`, so it can be undone.
- Otherwise it asks the user: keep aigle's version, or take the file's version (with before/after).

### 9.7 Cache

`~/.cache/aigle/` can be deleted at any time. aigle rebuilds the search index by scanning folders and parsing sidecars, and regenerates thumbnails and AI results on demand. Collections spanning folders are not supported in v1; keywords and saved searches cover that need.

> **Implementation note (library, first cut).** `crates/library` scans one folder (not recursive), reads and writes sidecars through a narrow C++ shim over the system Exiv2 (`crates/exiv2-sys`), and debounces writes on its own thread (1 s; flushed on switching photos, opening another folder and quitting). aigle owns `xmp:Rating`, `xmp:Label`, `aigle:SchemaVersion` and `aigle:Edits` (the `EditParams` as JSON; removed when the photo is back to unedited); every other property is kept through Exiv2's round trip. A sidecar is only created when there is something to store. Edits from a newer schema or that fail to parse are left untouched on write. The conflict check compares the BLAKE3 hash of the file with the one last read or written and skips the write, reporting it to the user. Not yet implemented: read-only-folder fallback sidecars (§9.4; the user is told the edits were not saved), file watching and reload (§9.6), keywords and snapshots, the masks file, and the search index. The GUI shows thumbnails for the index ranges visible in the grid and filmstrip only; leaving a range drops its jobs, which cancels them.

## 10. Requirements this places on the engine

1. **Headless operation:** a wgpu device without a surface, plus a CPU fallback. The fallback must be correct, not fast: it serves the CLI, servers and CI, and slow editing on machines without a usable GPU.
2. **Staged caching with param diffing:** a slider change recomputes only downstream stages.
3. **Cancellation at stage boundaries** and latest-wins per view.
4. **A four-class priority scheduler:** interactive > agent > batch > background.
5. **Offscreen renders of arbitrary params** that share the session's upstream caches.
6. **Quantitative analysis** (histogram stats, clipping, WB estimate, noise estimate).
7. **Semantic mask resolution** as jobs producing `MaskAssetId`s. In the GUI they persist in the photo's masks file; in the CLI they live for one run.
8. **Batch throughput mode:** several photos in parallel, bounded memory, no draft renders.
9. **Display color transform** to an arbitrary display profile.

## 11. Decisions

- **Platform:** Linux first.
- **Product principle:** aigle is a professional tool. When quality or correctness conflicts with speed, quality wins in the final result (Refined/Full renders and export); speed is recovered through Draft renders during interaction, never by approximating the final result.
- **Hardware:** the *minimum* spec is 16 GB RAM with an integrated GPU (unified memory); everything must work there, possibly slower. The *reference* machine is a Ryzen 7 8845HS (Radeon 780M iGPU) with 32 GB. Memory budgets scale with available RAM, with about 3 GB shared CPU+GPU at the minimum spec (doc 03).
- **CPU fallback:** correct but not optimized for speed.
- **Local adjustments:** Lightroom-style masks with scalar adjustments, including white balance and exposure. No local curves or HSL in v1. Masks are addressed by stable id, so ordered color layers can be added later without breaking edits.
- **Persistence:** XMP sidecars only, no database (§9).
- **No Lightroom develop-settings import** in v1.
- **License: AGPL-3.0-only.** GPLv3 and GPLv2-or-later libraries (librtprocess, Exiv2) can be used, because GPLv3 §13 / AGPLv3 §13 allow combining them. LGPL, MIT, BSD and Apache-2.0 dependencies are also fine. Dependencies that are GPLv2-only are **not** compatible. `cargo-deny` enforces the allow-list in CI. Model weights are checked one by one, since many have non-commercial or otherwise incompatible terms. Any mode that serves users over a network (for example a remote MCP server) has to offer the source code (AGPL §13).

## 12. Open items

- `EditParams` skeleton and path naming are proposed in `02-engine-pipeline.md` §4; field-level ranges/units still to be finalized (round 4).
