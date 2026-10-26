# aigle — Scheduler and Workers

Status: agreed (Q31–Q37). Builds on docs 01–04.

Scope: how engine work is represented, which threads and queues exist, how the single GPU queue is shared between priority classes, Draft/Refined/Full timing, cancellation, AI inference scheduling, and CLI batch pipelining. Memory admission is defined in doc 03 §6; the scheduler enforces it.

Timings are targets for the reference machine (Ryzen 7 8845HS, Radeon 780M). They must be measured, and the adaptive mechanisms below exist so that no fixed number has to be right.

## 1. Decisions

| # | Decision |
|---|---|
| Q31 | Draft frames target ≤ 33 ms. If a Draft exceeds it, the next Draft renders one quantized scale lower (doc 02 §12). Refined/Full quality is never reduced. |
| Q32 | Refined starts immediately when the change is final (§4.2) or the Draft was already at full render scale; otherwise after ~150 ms without changes. |
| Q33 | The GPU queue uses strict priority. Lower classes submit in chunks of ~2–4 ms. Starvation guard: a job without progress for ~2 s gets one chunk per frame. |
| Q34 | GPU AI inference runs one model tile per chunk, tile sized to ≤ ~50 ms (measured once per model), and is paused while the user interacts. |
| Q35 | AI jobs keep running when the user navigates away; results are cached. They are cancelled only explicitly or when admission needs the memory. |
| Q36 | User renders always come before agent renders on the same photo (class order, doc 01 §4). |
| Q37 | CLI batches are pipelined (CPU decode ∥ GPU render ∥ CPU encode), with photos in flight bounded by memory admission and an optional `-j N` cap. |

## 2. Classes

Highest first. Within a class, order is by the job's own priority, then FIFO.

| Class | Work |
|---|---|
| `Interactive` | Draft/Refined/Full frames of Views, statistics passes they need, A-tier work of the photo being opened |
| `Agent` | `render_offscreen`, `analyze`, `compute_mask` requested by the agent |
| `Batch` | CLI `profile apply/adapt` photos |
| `Background` | thumbnails, previews, prefetch, exports, AI denoise started by the user, disk-cache writes |

- A job inherits the class of its *most urgent* waiter. Example: if a View needs a semantic mask the agent started, that mask job is promoted to `Interactive`.
- AI denoise started by the user is `Background`, but the View of that photo shows progress and uses the result as soon as it lands.
- Exports in the GUI are `Background`; an explicit "export now and wait" in the UI promotes to `Agent` level. Interactive editing still comes first.

## 3. Work model

### 3.1 Tasks

Every unit of engine work is a `Task`:

```rust
struct Task {
    class: Class,
    prio: i32,                    // within class (e.g. thumbnail visibility)
    resource: Resource,           // CpuPool | NativeDecode | Gpu | Inference(backend) | Io
    est_cost: Duration,           // from cost model (§6)
    mem_peak: u64,                // for admission (doc 03 §6)
    deps: SmallVec<[TaskId; 4]>,
    cancel: CancelToken,
    run: Box<dyn FnOnce(&ExecCtx) -> TaskResult>,
}
```

- A render request is expanded into a task DAG from the stage graph (doc 02 §5.1): tiles × stages, minus everything whose cache key is already resident.
- **Tasks are deduplicated by cache key.** Two Views or a View and an agent render that need the same tile share one task, at the higher class (§2).
- GPU tasks are *chunks*: one submission of a few milliseconds (doc 04 §6). A tile render that is too long for one chunk is split along stage boundaries.

### 3.2 Slots and jobs

- **Views** hold one latest-wins slot (doc 01 §4). A new target replaces the old one; tasks of the old target that are not needed by the new one are cancelled.
- **Jobs** (`Job<T>`) own their task DAG. Dropping the `Job` cancels every task no other job or View shares.

## 4. Interactive timing

### 4.1 Interaction hints

The engine needs to know whether a change is part of a continuous gesture. `doc` and the app pass a hint with each update:

```rust
pub enum Interaction { Continuous, Final }

session.set_edits(rev, params, Interaction::Continuous); // slider drag, brush stroke in progress
session.set_edits(rev, params, Interaction::Final);      // slider released, value typed, undo
view.set_viewport(vp, Interaction::Continuous);          // pan/zoom gesture
```

Agent commits, profile application and external reloads are always `Final`.

### 4.2 Draft → Refined → Full

- `Continuous` updates render **Draft**. Draft targets ≤ 33 ms end to end (render + readback + handoff to GPUI).
- The render scale for the next Draft is chosen from an EWMA of recent Draft costs. Over budget → one quantized scale lower (`s_k = 2^(k/2)`, doc 02 §5.2). Under 50% of budget for 3 frames → one scale higher, up to the View's target scale.
- **Refined** starts:
  - immediately on a `Final` update;
  - immediately if the last Draft already ran at the View's full render scale with no Draft approximations (doc 02 §12); the Refined render then reuses all its tiles and typically only refreshes stale statistics or A4;
  - otherwise ~150 ms after the last `Continuous` update.
- **Full** follows Refined when the View is at ≥ 1:1 (S tier at level 0).
- Frames are published as soon as each quality level completes. A Refined frame is never replaced by an older-revision frame.

### 4.3 Pan and zoom

- The UI reprojects the previous frame immediately (doc 01 §4).
- Tiles are ordered centre-out from the viewport centre, so the visible centre fills first.
- At ≥ 1:1, tiles in a one-tile ring around the viewport are rendered at `Background` class after the visible ones, so short pans hit the cache.

## 5. Resources and threads

```
GPUI main thread ── never runs engine work
Engine coordinator (1 thread) ── owns the ready queues, admission, cost model; dispatches tasks
CPU pool (rayon)          ── CPU-backend kernels, pyramid build, encoding prep
Native decode workers     ── LibRaw / librtprocess, 1 LibRaw instance per worker
Inference pool            ── ONNX Runtime CPU EP (intra-op threads), or driver thread for GPU inference
GPU submitter (1 thread)  ── the only thread that submits to the wgpu queue
IO pool (2 threads)       ── file reads, sidecar/masks writes (doc 01 §9.3), disk cache
```

- **CPU budget.** The 8845HS has 8 cores / 16 threads. The coordinator hands out CPU *tokens*, one per hardware thread minus one reserved for GPUI. Every CPU-using pool takes tokens before running, so pools never oversubscribe the CPU together.
  - `Interactive` CPU tasks may take tokens from lower classes at their next task boundary.
  - The ONNX intra-op pool is sized at (physical cores − 2) (doc 04 §7.2) and holds that many tokens while running a tile.
- **Single GPU submitter.** All engine GPU work goes through one thread and one queue, so ordering and priority are decided in one place. GPU inference backends with their own GPU context (doc 04 §7.3) are not submitted through wgpu, but the coordinator gates them: they may only start a tile when the scheduler grants an inference chunk (§7).

## 6. GPU arbitration

- **Strict priority.** When the GPU submitter is free, it takes the highest-class ready chunk.
- **Chunk size.** Lower-class chunks are sized to ~2–4 ms of GPU time, so an incoming interactive update waits at most one chunk. Interactive chunks may be larger (up to the Draft budget).
- **Cost model.** Each (kernel, tile size, format) has an EWMA of measured GPU time from timestamp queries, seeded by a short calibration at first launch. The model sizes chunks and predicts Draft cost (§4.2). Where timestamp queries are unavailable, CPU-side fence timing is used.
- **Starvation guard.** A non-interactive job that has made no progress for ~2 s is granted one chunk per published interactive frame until it progresses. This keeps exports and thumbnails moving during long editing sessions without visibly affecting interactivity.
- **Idle.** With no interactive work pending, lower classes run back to back.

## 7. AI inference scheduling

- **Interacting** means a `Continuous` update arrived in the last ~300 ms, or any Draft/Refined render is pending.
- **GPU backend:**
  - While interacting, no new inference tile is started. A tile already running finishes; it cannot be preempted.
  - While idle, one tile is granted at a time. Tile size per model is calibrated once so that a tile takes ≤ ~50 ms on this machine (cached with the backend choice, doc 04 §7.1). The worst-case delay for an interaction that starts during a tile is therefore ~50 ms, once per gesture.
- **CPU backend:** inference runs on the inference pool with its CPU tokens. It yields tokens to `Interactive` CPU work at tile boundaries. It does not pause for GPU-only interaction.
- **Navigation (Q35).** Leaving the photo does not cancel its AI jobs. They continue at `Background` class and write their asset to the disk cache. The A-tier inputs they need stay pinned until the job finishes. Cancel happens only:
  - on explicit user cancel
  - when admission for an `Interactive` task cannot be satisfied otherwise; the job checkpoints its completed tiles to disk first and can resume later
- Only one inference job runs at a time (doc 04 §7.2). Queued inference jobs are ordered by class, then by request time.

## 8. Cancellation

- **Tokens** are checked at every task boundary and before every GPU submission.
- **Native calls.** LibRaw and librtprocess run long single calls. Verified with LibRaw 0.22.2 and librtprocess 9a858270: LibRaw's progress handler aborts when it returns non-zero, but it is only called at stage transitions, not inside decoders. librtprocess calls its `setProgCancel` callback but ignores the return value, so demosaic, CA correction and highlight reconstruction cannot be cancelled mid-call. The engine checks cancellation between A-tier stages; a cancelled open finishes the current native call (about 0.3–2 s on 24 MP) and discards the result. Finer cancellation would need tiled calls (AMaZE and CA_correct accept a window; RCD and Markesteijn do not) or a patched librtprocess.
- **GPU.** Submitted chunks run to completion (a few ms); later chunks of a cancelled task are not submitted.
- **Partial results.** Tiles completed before cancellation stay in the cache if their keys are still valid for another consumer; otherwise they are eviction candidates as usual (doc 03 §5).

## 9. Admission and waiting

- Before a task runs, the coordinator reserves its `mem_peak` (doc 03 §6). Tasks that don't fit wait in their class queue.
- When an `Interactive` task waits for memory, the coordinator first evicts per doc 03 §5.2, then pauses lower-class jobs at their next boundary to free their reservations, then cancels prefetch.
- A job larger than the whole budget runs alone (doc 03 §6, Q22). While it runs, other jobs that need memory wait.

## 10. CLI batch pipeline

```
 photos ─► [decode + demosaic, CPU] ─► [render tiles, GPU] ─► [encode + write, CPU/IO] ─► outputs
              photo N+2                    photo N+1                photo N
```

- Each stage is a bounded queue. The number of photos in flight is the largest number that fits the budget (doc 03 §6, CLI budget), optionally capped by `-j N`.
- Decode and demosaic for the next photo starts as soon as there is admission room, so the GPU is rarely idle waiting for the CPU.
- On the minimum spec with large raws this degenerates to one photo at a time, with decode of the next photo overlapping encode of the current one when memory allows.
- `profile adapt` adds agent calls per photo. LLM latency is network-bound, so several photos may be in the agent step concurrently (bounded by `--agent-concurrency`, default 4) while others render.
- Results are written in completion order. A summary (per photo: success, clamps, constraint violations, backend used) is printed at the end, and as JSON with `--json`.
- The CLI never uses the disk cache (doc 01 §8), so cross-run reuse doesn't exist; within a run, sessions are dropped as soon as their output is written.

## 11. Observability

- A debug HUD in the app shows: queue depths per class, the GPU timeline (chunks by class), Draft cost vs budget, current render scale, memory ledger (doc 03 §2.2), and the inference backend.
- `AIGLE_TRACE=path` writes a Chrome-trace/Perfetto JSON of tasks, chunks and frames.

## 12a. Implementation notes (first implementation; for review)

- **Resources.** No coordinator thread or per-resource worker pools. Engine worker threads drain one class-ordered ready queue; shared resources are arbitrated where they are used, by class with the §6 starvation guard: the GPU queue (1 holder), native decoders (half the hardware threads), file writes (2). The policies are pure functions of task metadata and a clock and are tested with a simulated clock.
- **CPU tokens.** All classes share one rayon pool. Every running task registers its class; lower-class work (exports, thumbnails, agent renders, the pre-render ring) is cut into chunks of 4 output tiles and waits at each chunk boundary while a higher class runs, unless starved (2 s). Separate rayon pools per class were tried and dropped: a pool thread blocked in a join can run another task's body nested on its stack, which deadlocks when that task waits for the first. Measured: with a 24 MP export running, a 1:1 edit settles in 345 ms against 388 ms idle.
- **GPU submitter.** Not a thread: the queue arbiter gives the same ordering. Lower-class submissions are sized to ~3 ms from an EWMA of measured GPU time per item and hold the queue until they finish, so an interactive submission waits behind at most one of them. No timestamp queries; fence timing on the CPU.
- **Dedup by cache key.** Output tiles and S blocks are claimed in the tile cache before computing; a task waits for products another task of the same or a higher class is computing, and computes products held only by lower classes itself (no priority inversion). No explicit task DAG.
- **Pan and zoom (§4.3).** Tiles are computed centre-out. Frames are published only when complete (no partial frames). After a settled frame at ≥ 1:1 the one-tile ring is pre-rendered at `Background`.
- **Integrated GPU clocks.** Measured on Intel UHD 770: the same S batch takes 60 ms after a 20 ms idle gap and 120 ms after 1.5 s. Keeping the GPU busy between frames (empty dispatches, or 8 ms of work every 100 ms) did not change it; nothing is done about it.
- **Native cancellation (§8).** The vendored librtprocess is patched so that demosaic and CA correction stop when their progress callback returns true (about every 1 MP). Measured cancel latency 24–89 ms against 1.7–3.3 s decodes. Highlight inpainting (small sparse regions) is not cancellable.
- **Device loss (doc 04 §8.2).** Any runtime GPU error re-creates the device (up to 3 times per process); the failed batch runs on the CPU, device-resident data of the old device is recomputed. The self-test result is cached per (driver, build) in the cache dir (doc 04 §8.3).
- **Not implemented:** inference scheduling (§7; there is no inference backend yet), the CLI batch pipeline (§10; there is no CLI), the debug HUD and `AIGLE_TRACE` (§11), job class promotion by waiter (§2).

## 12. Testing

- **Simulated clock.** The coordinator's policy is pure logic over task metadata and a clock. It runs under a simulated clock in unit tests, with scripted scenarios: slider drag during export, agent renders during editing, AI denoise while panning, memory exhaustion.
- **Invariants:**
  - an `Interactive` task never waits behind more than one lower-class chunk (or one inference tile)
  - no job starves for longer than the guard interval plus one chunk under continuous interaction
  - a cancelled job's tasks are never submitted after cancellation is observed
  - a published frame's revision never goes backwards per View
- **Latency benchmarks** on the reference machine in the nightly hardware run: Draft p50/p95 while dragging each slider on 24/45/100 MP files, Refined latency, pan fill time, export throughput with and without interaction.
