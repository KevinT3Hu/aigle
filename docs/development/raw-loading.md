# RAW loading profile and optimization

Current result: the 47 MiB Nikon NEF reaches a non-Draft 1800 × 1200 preview in
**1.498 s median** in the release CPU-engine benchmark on the Ryzen 7 8845HS. The
first run took 1.602 s; subsequent runs were 1.495–1.510 s. See the final section
for the latest changes and validation.

`raw_load_bench` measures a fresh session opening a file, source readiness (decode,
RAW preparation, demosaic, highlight reconstruction and pyramid), and the first
non-Draft engine preview at 1800 × 1200. It uses the CPU rendering backend, a 4 GiB
memory budget and the app's isolated decoder process. It does not measure window
startup, texture upload or compositor presentation. Each session decodes again;
there is no persistent decoded-source cache in this measurement. Filesystem caches
are left under normal OS control, rather than forcibly evicted.

```sh
RUST_LOG=aigle_engine::source=debug cargo run --release -p aigle-engine \
  --example raw_load_bench -- /path/to/photo.NEF 3
```

The logs separate open/unpack, CFA copy, hot-pixel repair, reference white-balance
premultiplication, automatic CA correction, demosaic, orientation/packing,
conversion/highlight reconstruction, and pyramid building. `perf record -g
--call-graph dwarf` follows the isolated worker as well as the parent.

On the Ryzen 7 8845HS, two local Nikon NEFs both decode to 8288 × 5520 (45.75 MP).
The initial 64 MiB sample took a median 4.263 s to become ready and 4.620 s to produce
a non-Draft preview. RAW preparation took about 1.63 s, RCD demosaic 1.06 s,
unpacking 0.64 s, and IPC about 0.35 s. CPU sampling also identified hot-pixel
neighbour scans and per-pixel orientation transforms as unnecessary work.

Three-run medians before and after the initial RAW optimization (before parallel CA):

| NEF sample | Ready before → after | Non-Draft preview before → after | Preview reduction |
| --- | --- | --- | --- |
| 64 MiB, 45.75 MP | 4.263 → 3.213 s | 4.620 → 3.571 s | 22.7% |
| 47 MiB, 45.75 MP | 3.846 → 2.794 s | 4.182 → 3.122 s | 25.3% |

The initial changes (later refinements are described below) were:

- RCD's independent native tiles run on up to four workers with GCC/libgomp.
  Decoder admission allows one decode per four hardware threads, additionally
  bounded by memory admission. CA now uses the same bounded team, as detailed below.
  Non-GCC toolchains retain serial native processing.
- Native librtprocess builds use optimization level 3, preserving strict floating
  point behavior and disabling contraction as before.
- Hot-pixel repair stops scanning neighbours once both hot and dead outlier cases
  have been ruled out. Pixels requiring repair retain the original summation order.
- RGB orientation computes a row origin and integer stride instead of applying a
  floating-point coordinate transform to every pixel.
- Partial RCD edge tiles clear scratch padding before reuse. Otherwise their output
  depended on the previous tile processed by the same worker. Serial and parallel
  output now match bit for bit across all Bayer arrangements and several odd,
  even, small and partial-tile dimensions. The decoder cache version is bumped.
- RCD cancellation uses a shared atomic flag, checks before allocating and polls
  every eight tiles per worker, preserving responsiveness with parallel work.

## Parallel CA correction

The standalone `ca_bench` isolates CA processing after RAW unpacking and CFA
normalization. It runs both serial and parallel correction on identical copies,
prints timings and pixel hashes, and asserts bit-for-bit equality. Its fixed
reference white-balance gains differ from the full editor, so its absolute CA times
should be compared within this benchmark, rather than directly to the load trace.

```sh
cargo run --release -p aigle-engine --example ca_bench -- /path/to/photo.NEF 3
```

Standalone three-run medians dropped from 1.398 s to 0.405 s on the first NEF
and from 1.299 s to 0.432 s on the second (about 3–3.5× faster). The corrected
pixel hashes are identical to the original implementation on both files.

Fresh full-load runs during the CA optimization, with the previous RCD optimization
already present in both binaries, gave these three-run medians:

| Sample | Ready before → after | Non-Draft preview before → after | Preview reduction |
| --- | --- | --- | --- |
| 64 MiB NEF | 3.592 → 2.630 s | 3.989 → 3.051 s | 23.5% |
| 47 MiB NEF | 2.989 → 2.118 s | 3.360 → 2.466 s | 26.6% |

Desktop load differs from the earlier RAW optimization measurements; compare each
before/after pair within its own table. Preview measurements still exclude upload
and compositor presentation.

CPU sampling identified native diagnostic/correction tiles and horizontal/vertical
Gaussian filtering as the expensive work. The implementation enables bounded
OpenMP for these loops, while keeping the following operations deterministic:

- Diagnostic tiles already store their measured shifts in a shared tile array.
  A serial raster-order pass sums these shifts in the original arithmetic order
  before fitting the correction. Dynamic tile scheduling cannot change the sum.
- Overlapping green interpolation writes now have disjoint ownership: the later
  tile owns the overlap, matching the original serial overwrite order.
- Correction tiles and Gaussian filtering partition output by tile, row or column.
- Cancellation uses an atomic flag and skips subsequent iterations and color-shift
  filtering when cancelled. The shared callback requires `Sync` in the Rust API.

Both correction iterations, the polynomial fit, and Gaussian color-shift
compensation remain enabled. Native serial entry points remain available for
reference comparisons. No lower-resolution approximation or quality reduction is
used. Real-file output hashes match the original pre-OpenMP implementation;
synthetic tests compare repeated serial/parallel runs on all Bayer arrangements,
including odd sizes and nonzero optical shifts.


Validation covers engine unit tests, exact serial/parallel RCD comparison, hot-pixel
comparison against the original full-neighbour algorithm, all eight orientation
mappings, native cancellation, and cancellation on both real NEFs. Two unrelated
AVIF tests require an encoder unavailable in the installed codec and are excluded.

## Direct shared RAW output

The RAW orientation/packing pass now fills a Linux memfd mapping directly, instead
of building a 550 MB RGB vector and sending its contents through a pipe. After the
parallel packing pass joins, the worker drops its writable mapping and seals the
file against writes, growth and truncation. It passes the descriptor over a Unix
socket. The parent validates the seals, exact byte length and pixel alignment,
then reads the immutable mapping while building the source pyramid. The mapping
remains valid when the worker is reused, crashes or exits.

`Call::payload()` returns either shared pages or heap pixels; `Call::blob()` retains
the existing writable-vector interface. Ordinary vector replies stay inline on
the socket. Non-Linux systems and failures to allocate shared storage retain heap
output and inline transport. Full-precision operations that need writable pixels
materialize the vector explicitly.

Copying an already-completed vector into shared storage was benchmarked and removed:
the extra memory copy did not consistently improve complete loading latency. Direct
producer output avoids that copy. LibRaw builds also use optimization level 3.
Testing its memory-buffer reader found no repeatable improvement over the existing
file reader, which is retained. RAW decompression itself remains about 0.6 s.

To compare shared and inline output on the same build:

```sh
cargo run --release -p aigle-engine --example raw_load_bench -- /path/to/photo.NEF 3
AIGLE_SHARED_BLOBS=0 cargo run --release -p aigle-engine \
  --example raw_load_bench -- /path/to/photo.NEF 3
cargo run --release -p aigle-engine --example raw_unpack_bench -- /path/to/photo.NEF 3
```

The load benchmark also prints preview pixel hashes. The unpack benchmark separates
open/unpack from CFA conversion and prints metadata, pixel and clipping-mask hashes.
The latter is useful for evaluating future decoder changes without conflating them
with demosaic, transfer or preview rendering.

Validation includes shared and inline worker protocols, retained mappings across
worker crashes, rejection of unsealed/wrong-size files, engine tests, and DNG export,
metadata/preview preservation and reopening on both NEFs. Formatting, Clippy and the
editor build also pass; Clippy retains an unrelated generated-code warning.

Three-run medians against the preceding CA-optimized build, with default quality,
isolated decoding and the same 1800 × 1200 CPU preview:

| Sample | Ready before → after | Non-Draft preview before → after | Preview reduction |
| --- | --- | --- | --- |
| 64 MiB NEF | 2.715 → 2.327 s | 3.124 → 2.765 s | 11.5% |
| 47 MiB NEF | 2.213 → 1.889 s | 2.608 → 2.262 s | 13.3% |

The raw-complete → parent-decode-complete gap dropped from roughly 0.4 s to
22 ms on the first file (with clipping-mask data) and below 1 ms on the second. This includes IPC, serialization and worker scheduling overhead.
Shared-page packing adds some producer work, so the net end-to-end saving is
smaller than the reduction in that gap. Measurements exclude texture upload and
compositor presentation.

## 1.5-second target

A fresh baseline immediately before this phase measured 2.178 s to the first
non-Draft preview for the 47 MiB NEF, with 1.825 s to source readiness. The final
five-run median is 1.498 s (31.2% lower), with source readiness at 1.181 s.
Runs were 1.602, 1.497, 1.510, 1.495 and 1.498 s. Filesystem caches were left under
normal OS control. These are engine frame-publication timings; window startup,
texture upload and compositor presentation are excluded.

The output remains 1800 × 1200, with the same default edits and non-Draft quality.
The preview hash remains
`5ffcfac6c9e921357b33a50d8c008344fc9d652a351c93372b557b58a335d6bd`.
No embedded camera preview is substituted for the processed RAW image.

Current implementation:

- Nikon entropy decoding refills a 64 KiB byte buffer instead of dispatching virtual
  reads per compressed byte. Bit state stays local to the decode and is committed
  on normal return or exception unwinding. Read-ahead is rewound to preserve the
  consumed stream position. The two horizontal predictors use fixed indices while
  retaining their original sample order and 16-bit vertical-predictor behavior.
- CFA normalization caches the exact LibRaw filter pattern and processes independent
  rows in a bounded OpenMP team. Patterns retain G2 black-level handling, Bayer's
  eight-row period, X-Trans's six-row period and the legacy 16 × 16 layout.
- CA and RCD teams now use up to eight workers. Decoder admission permits one decode
  per eight hardware threads, with memory admission still bounding work in flight.
  Floating-point CA reductions remain in deterministic raster order.
- Hot/dead-pixel repair detects sparse replacements from the original CFA before
  applying any of them. It avoids the full-image snapshot copy and retains the
  original neighbour and summation order. Per-row vectors bound worst-case scratch.
- Float-to-half conversion uses vectorized conversions in bounded stack blocks and
  reuses the collected allocation, avoiding whole-image destination zero-filling.
  Tests cover chunk edges, signed zero, subnormals, rounding ties, infinities and NaNs.
- Initial histogram rendering starts after the frame is published, reusing cached
  preview work. Continuous and settled histogram updates remain covered by tests.
- Large CPU S-tier batches parallelize across blocks, keeping each block's small
  passes on one worker. Small batches retain pixel-level parallelism. Serial/batch
  and nested execution are compared bit for bit.

Validation: 106 engine unit tests, the shared-payload validation test, four native
processing tests, 31 engine integration tests, and six RAW integration tests passed.
The RAW tests include actual edit/redecode, cancellation and DNG export/reopen on
both local NEFs. Metadata, normalized CFA pixels, clipping masks and counts match
the pre-change decoder on both files. Original and optimized decoders both reject
a truncated Nikon entropy stream. Existing AVIF encoder checks are excluded because
the installed codec has no encoder. Formatting and Clippy pass with the existing
generated-code warning; the optimized editor binary is rebuilt.
