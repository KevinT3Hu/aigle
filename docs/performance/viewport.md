# Detailed viewport latency

Measured on 2026-10-29, AMD Ryzen 7 8745HS, Rust 1.99.1 release build.
The benchmark uses a 6000×4000 synthetic gradient, 1920×1080 viewport, a 4 GiB
engine budget, default worker count, and a separate 1024×683 whole-image backing
matching the canvas. Loading and the initial backing render finish before timing.
It measures `set_viewport` through publication of the matching non-Draft frame;
it excludes GUI image copying, upload, composition and display presentation.

The first traversal visits ten pan positions, then ten zoom/position combinations
at scales 0.5–2.0. The second traversal repeats them. These are mixed-cache first
visits, not ten independent cold renders: the whole-image backing, overlapping
viewports and normal background tile-ring work can populate the shared cache.
The second traversal demonstrates the cached path. No artificial delay is inserted
between requests, and normal histogram/ring work remains enabled.

## Profile and optimization

`perf record -F 499 -g` on the original CPU implementation attributed 22.87% of
sampled cycles to `Px::get`, 13.61% to Lanczos sampling, 10.44% to `powf`, 6.99%
to Gaussian detail filtering, and 4.39% to hardware half-to-float conversion.
The repeated half-float loads were primarily reached from Lanczos sampling.
This is a whole-benchmark CPU profile, including background work, not wall-time
percentages for a single frame.

The CPU C1 tile path now decodes its source footprint to float once, instead of
converting half-float values at every Lanczos filter tap. Source coordinates,
mirror boundaries, tap weights and accumulation order are preserved. It uses
only temporary storage; final preview tiles retain their existing half precision.
Decoding is capped at 2,097,152 pixels (24 MiB of temporary RGB storage per active
tile). Larger footprints, Draft bilinear sampling, full-float input and GPU jobs
keep their original paths. Multiple concurrent tiles can each hold scratch.

## Results

Three before/after pairs were run sequentially, alternating execution order.
Each cell below is the median of the three run statistics, in milliseconds.
For ten samples, the benchmark reports the upper median and nearest-rank p90.
The reference executable was built before changing the CPU sampler, with the
same benchmark and compiler. The later Continuous-mode option does not change
these default Final-mode requests.

| Detail settings | Traversal | Motion | Median before → after | p90 before → after | Max before → after |
|---|---|---|---:|---:|---:|
| Disabled | First | Pan | 42.33 → 34.32 | 103.14 → 82.49 | 265.01 → 210.65 |
| Disabled | First | Zoom | 125.83 → 109.97 | 354.06 → 286.14 | 559.58 → 433.63 |
| Default | First | Pan | 19.85 → 15.67 | 209.92 → 204.22 | 908.53 → 883.88 |
| Default | First | Zoom | 313.67 → 253.29 | 575.87 → 543.29 | 1009.10 → 862.93 |
| Disabled | Repeat | Pan | 16.23 → 19.13 | 22.93 → 25.66 | 34.85 → 30.53 |
| Disabled | Repeat | Zoom | 13.96 → 11.92 | 25.59 → 23.60 | 44.98 → 29.46 |
| Default | Repeat | Pan | 20.63 → 20.52 | 31.81 → 24.24 | 46.23 → 24.25 |
| Default | Repeat | Zoom | 14.91 → 15.17 | 26.41 → 18.56 | 34.29 → 26.80 |

Default-detail first-traversal zoom median improves about 19%; its median run
maximum improves about 14%. Cached requests do not invoke the optimized C1
sampler, and their differences reflect background scheduling/noise rather than
an expected speedup. These synthetic-image results do not establish latency on
real RAW files or all edit combinations. Default detail filtering can still
produce long first-visit frames; this change does not eliminate every stall.

A separate optimized Continuous-mode run includes the engine's existing 150 ms
settle timer: default-detail first-traversal zoom median was 359.50 ms, and repeat
pan/zoom medians were 162.94/162.60 ms. These are isolated requests allowed to
settle, not a sustained stream of pointer events. The timer accounts for most
of the cached gesture-to-detail delay; it remains unchanged.

## Correctness and backend limits

All eight scenario hashes match across all six Final-mode runs (480 frames in
aggregate), and the Continuous run produces the same hashes. The regression test
compares optimized output bits against the original per-pixel sampler for affine,
rotated/minified, warped/TCA, mirrored-edge and narrow-tile cases.

The 94 applicable engine unit tests pass. Two unrelated AVIF tests are excluded
because the installed libvips lacks the AVIF saver. Formatting and Clippy pass.

Auto mode selects CPU on this machine because of the existing GPU startup
self-test failure: `out_tile (device input)` differs from the CPU reference by
four code values. This was also reproduced on the original implementation in
`math-kernels.md`. No GPU speedup is measured or claimed, and the self-test is
not relaxed. Restoring a correct GPU path remains a separate opportunity.

## Reproduction

```sh
cargo run --release -p aigle-engine --example viewport_bench -- cpu
cargo run --release -p aigle-engine --example viewport_bench -- auto
# Optional real image; viewport coordinates remain the same fixed workload:
cargo run --release -p aigle-engine --example viewport_bench -- cpu /path/to/photo.raw
# Include the Draft → Refined settle interval:
VIEWPORT_BENCH_CONTINUOUS=1 cargo run --release -p aigle-engine --example viewport_bench -- cpu
# Stage timings and CPU samples:
RUST_LOG=aigle_engine::pipeline=debug target/release/examples/viewport_bench cpu
perf record -F 499 -g -- target/release/examples/viewport_bench cpu
cargo test -p aigle-engine --lib decoded_lanczos_matches_original_sampler_bits
```
