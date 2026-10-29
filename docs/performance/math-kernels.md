# CPU math profiling and optimization

The measured targets are local-adjustment masks, Lanczos resampling, and the source-grid
(S-tier) detail executor. The matrix/vector multiply remains the existing unrolled 3×3
operation. Removing unused color transforms and batching stencil work gives larger gains
without adding SIMD dependencies or changing floating-point arithmetic.

## Profiling

Exploratory `perf record -F 499 -g --call-graph dwarf` runs on a Ryzen 7 8745HS identified:

- Mask microbenchmark: 61.2% of sampled cycles in `libm::powf`, 13.5% in mask preparation,
  and 5.7% in loading its matrices. Geometric and luminance-only masks were computing
  unused OkLab values (including three cube roots).
- Detail benchmark (original executor, chroma and combined scenarios): 43.4% in `box_px`, 18.0% in
  `gauss_px`. Box filters repeatedly traverse overlapping windows.
- Lanczos benchmark: 33.0% in `libm::sinf`; horizontal mirror indices also repeat for
  every row of a sample's window.

These percentages describe the synthetic benchmark workloads, not an entire photo editor
session. Initial sampling used the system Rust 1.101 nightly; final timing uses the
repository's Rust 1.99.1 and default release settings, without `target-cpu=native`.

## Timings

Rust 1.99.1 release builds on the Ryzen 7 8745HS, with kernel benchmarks pinned to CPU 2
and the four-worker detail benchmark pinned to CPUs 2–5. Kernel cases use a 3× iteration
multiplier; detail cases use 20 measured runs after warmup. The old kernel baseline is
revision `4dcb923`, built with the identical benchmark source and libm 0.2.16.

| Workload | Before | After | Speedup |
| --- | ---: | ---: | ---: |
| Geometric radial mask | 143.43 ns/pixel | 23.48 ns/pixel | 6.11× |
| Luminance mask | 144.22 ns/pixel | 71.78 ns/pixel | 2.01× |
| Color mask | 151.15 ns/pixel | 141.88 ns/pixel | 1.07× |
| Lanczos, footprint 1 | 134.52 ns/pixel | 132.82 ns/pixel | 1.01× |
| Lanczos, footprint 2 | 293.15 ns/pixel | 274.82 ns/pixel | 1.07× |
| Chroma detail block, 13 passes | 6.801 ms | 4.169 ms | 1.63× |
| Combined detail block, 36 passes | 24.046 ms | 19.755 ms | 1.22× |

Kernel checksums match for all cases. The unchanged 3×3 matrix/vector microbenchmark
measures 2.66 ns/pixel in both builds. The small footprint-1 Lanczos difference is within
timing noise; the optimization is more useful with wider sampling windows.

## Changes

- Compute mask luminance and OkLab on first use and cache them across all masks for that
  pixel. The packed program format stays the same.
- Batch CPU box passes by row. Horizontal interiors process eight neighbouring outputs
  together; vertical passes traverse contiguous input rows. Every output accumulates taps
  in the scalar reference's order, including clipped boundaries. Prefix sums were avoided
  here because their different rounding can affect the guided filter's variance estimate.
- Reuse the detail executor's intermediate result allocation across passes.
- Compute Lanczos horizontal mirror indices once per sample instead of once per tap row.

No approximate transcendental functions, FMA changes, unsafe code, or new dependencies
are introduced. Box-pass asymptotic complexity remains proportional to window radius.

## Reproducing

Use the pinned toolchain (`rustup run 1.99.1 cargo ...` where the shell's `cargo` bypasses
rustup). Build first, then time without concurrent compilation or other heavy workloads.

```sh
cargo run --release -p aigle-kernels --example math_bench
cargo run --release -p aigle-engine --example detail_bench
```

Both examples accept a case-name substring and an iteration multiplier/count. The kernel
benchmark reports the median of five timed runs after warmup, consumes full RGB outputs,
and prints checksums. To reproduce the old kernel baseline, run the same benchmark source
against the parent revision's kernels with the same toolchain and libm version.

The detail benchmark includes the original scalar executor, alternates timing order in a
fixed four-worker pool, and compares every output float bit for every iteration. It measures
256×256 block interiors from a synthetic 512×512 RGB image; it includes workspace allocation
and pass copies, but excludes plan construction, decoding, geometry, output transforms,
GPU transfers, and application scheduling. Its results are not end-to-end render speedups.

For sampling, build with debug information and run the executable directly:

```sh
cargo rustc --release -p aigle-kernels --example math_bench -- -C debuginfo=1
perf record -F 499 -g --call-graph dwarf -- target/release/examples/math_bench mask 10
perf report --stdio --no-children --sort symbol
```

## Correctness

The box-pass regression compares every output bit with the portable scalar kernel across
one-pixel images, clipped windows, vector tails, missing planes, and one to four channels.
Mask/overlay and Lanczos fingerprints were first checked against the pre-optimization
implementations, then retained as regression fixtures. They cover nonzero packed-buffer
offsets, mixed geometric/luminance/color masks, negative RGB values, mirrored boundaries,
and multiple reconstruction footprints. The detail benchmark also checks full block output
against the original executor.

## Validation limits on this machine

Core and kernel library tests pass (50 tests), as do the two new kernel fingerprint tests.
The engine library suite passes 93 of 95 tests; its two AVIF tests fail because this
machine's libvips does not expose an AVIF saver. Formatting and Clippy checks pass.

The direct S-tier GPU/CPU comparison and C1 atlas-eviction tests also pass with the rebuilt
module. The device-recovery unit test skips its GPU path after the startup failure below.

The SPIR-V module was rebuilt with the pinned `nightly-2026-08-15`; the rebuild consistency
check passes. The full GPU integration suite cannot get past the startup output-stage
self-test: `out_tile (device input)` differs by four 8-bit code values. Repeating the same
test with both the original host code and original committed SPIR-V reproduces the identical
failure. This limits GPU
validation here; the optimization does not relax that self-test or its tolerances. No GPU
performance improvement is claimed.
