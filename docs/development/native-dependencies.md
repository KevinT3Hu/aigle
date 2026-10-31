# Managed native dependencies and patch audit

Audit date: 2026-10-31. The subsequent vcpkg migration implements the packaging
recommendation below. See [native setup](../../native/README.md). Source patches
are now named files in `native/ports`; the source-level audit and measurements
below describe the changes carried by those patches.

## Packaging recommendation

Repository-local vcpkg overlay ports can supply missing libraries, replace stock
ports, apply source patches, and set explicit CMake/Meson build options. Missing
registry packages do not require choosing a different package manager.

Use stock ports for ordinary dependencies and overlays for:

| Port | Project-owned packaging |
| --- | --- |
| libvips | Extend codec features/dependencies to include libheif, JPEG XL and libultrahdr. Require the codecs explicitly instead of relying on Meson auto-detection. |
| libultrahdr | Add a port; pin source/version/checksum, enable both ISO and XMP gain-map metadata for the intended compatibility coverage. |
| librtprocess | Add a port for the pinned source plus the justified patches below and per-source OpenMP/float settings. |
| libraw | Override the stock port to retain the Nikon optimization and current reentrant decoder configuration. |

The Rust/C ABI shims stay in their Cargo crates. Those crates compile their shims
and link the managed libraries rather than rebuilding vendored implementations. The installed headers must be from the same patched
library build: librtprocess's added error value and CA signature are part of our ABI.

Keep library-specific flags in the port's targets, rather than imposing them on
every dependency through a triplet. Use triplets for platform/compiler/linkage
policy. Pin the vcpkg revision, upstream revisions and archive checksums, commit
the overlay sources and selected features, and cache the resulting binaries.
Overlay recipes are pinned by the project Git revision; the stock registry baseline
does not independently version those overlays. A custom Git registry is useful
later if multiple repositories need independently versioned copies of the ports.

## Compared upstream sources

* librtprocess: `9a858270acb2096e2e403d932760ee688fcac425`. Upstream HEAD
  was still this same 2023-06-27 commit at audit time. Five vendored files differ:
  `librtprocess.h`, `rcd.cc`, `amaze.cc`, `markesteijn.cc`, and `CA_correct.cc`.
* LibRaw: `24fa7e5463cbf8b8615dbd2b16c933a294d52400` (0.22.2).
  After normalizing CRLF/LF differences, only `src/decoders/decoders_dcraw.cpp`
  has a substantive modification. That upstream file is unchanged between the
  pinned version, 0.22.3 and inspected HEAD
  `2caab57e269cc7fce17c3189e4d4d7e3e5861875`.

## Patch decisions

| Change | Assessment | Decision |
| --- | --- | --- |
| RCD partial-tile scratch initialization | Correctness. Reused scratch padding otherwise changes pixel results depending on which tile a worker processed previously. Removing this hunk makes the existing serial/parallel equality test fail. | Keep as a separate correctness patch. |
| CA disjoint green-scratch writes | Correctness under parallel execution. Upstream adjacent tiles write overlapping output regions; the patch assigns the overlap to the later tile, matching serial overwrite order. | Keep whenever CA is parallel. Existing fixtures alone do not demonstrate its necessity; see test limits below. |
| CA raster-order accumulation | Reproducibility under parallel execution. Upstream sums per-thread partial accumulators in scheduling-dependent order; the patch sums stored tile shifts in a fixed raster order. | Keep to preserve deterministic processing. |
| Cancellation in RCD, AMaZE, Markesteijn and CA; `RP_CANCELLED` | Required behavior for the editor's cancellable decode jobs. Upstream calls the progress callbacks but ignores their boolean return at the patched sites. A wrapper could suppress delivery but could not stop this native work promptly. | Keep; preserve the matching header and error mapping. |
| RCD polling every eight rather than 32 tiles | Responsiveness policy, not image correctness. | Keep initially; tune only with cancellation latency measurements. |
| Bounded teams and CA `multiThread` parameter | CPU-budget policy and selectable serial reference execution. A process-global `OMP_NUM_THREADS` setting does not express our per-operation control. | Keep bounded operation-level control. The specific eight-worker cap is tunable. |
| LibRaw Nikon buffered input, local bit state and paired predictors | Optional performance optimization, not required to decode correctly. This audit measured a material improvement with identical outputs on one local NEF. | Keep as a separate performance patch, with upstream/multi-camera validation required when rebasing. |
| GPUI native surface ownership and transparent-hole rendering | Required by the current embedded Wayland HDR canvas architecture. | Keep in Cargo's patched crates; these are not C/C++ packages to move into vcpkg. |

There is no demonstrated redundant source patch to delete as part of packaging.
Separate these concerns into named patches so upstream upgrades can retire one
change at a time. Do not combine a library version upgrade with the initial
packaging migration: first reproduce the pinned behavior.

## Build policy that must survive migration

These are build settings, not reasons to carry additional algorithm patches:

* Optimize image-processing native code even when the Rust editor is built in dev
  mode. Preserve the existing native optimization policy during the first migration.
* Preserve librtprocess's `-fno-fast-math` and `-ffp-contract=off` on supported
  compilers; avoid host-specific architecture flags in distributable binaries.
* Enable OpenMP only for the selected RCD and CA translation units as today.
  Upstream CMake's `OPTION_OMP` applies OpenMP to the whole library. Enabling it
  indiscriminately would also parallelize AMaZE/Markesteijn, whose patched
  cancellation flags are plain `bool` and are safe under our current serial build,
  not suitable as shared cancellation state for a new parallel build.
* Keep LibRaw decoder internals serial/reentrant and its separately compiled CFA
  normalization shim's bounded parallel behavior. A generic library-wide OpenMP
  feature is not equivalent.
* Use one consistent JPEG ABI across LibRaw, libvips and libultrahdr. Preserve
  unwind support in libjpeg for LibRaw's exception path through JPEG frames.
* Keep codec options and metadata settings explicit and test actual output; a
  successful library link is not evidence that an optional encoder is available.

## Verification performed

The existing four `aigle-rtprocess-sys` tests passed:

```sh
cargo test -p aigle-rtprocess-sys --lib --offline
```

Three isolated copies of the native crate were made under `/tmp`, each with one
change reverted while retaining the other patches and build settings:

| Reverted change | Existing regression test result |
| --- | --- |
| RCD scratch-padding initialization | Failed: Bayer output differs bit for bit between serial and parallel processing. |
| CA disjoint scratch ownership | Passed the current smooth synthetic fixtures. |
| CA ordered reduction | Passed the current smooth synthetic fixtures. |

The CA passes do not establish that those patches can be removed. Source inspection
still identifies overlapping writes and order-dependent floating-point sums.
Before claiming regression coverage for their removal, add a fixture that actually
exposes the affected behavior, exercise different thread counts/schedules, and use
a race-detection configuration compatible with the chosen OpenMP runtime.

For LibRaw, isolated builds used the same 0.22.2 source, shim and build flags,
changing only the Nikon decoder file back to upstream for the comparison. The
existing `raw_unpack_bench` code ran on one approximately 49 MiB local Nikon NEF;
LibRaw's `unpack_function_name()` confirmed that it selects `nikon_load_raw()`.
Four runs per variant alternated order; the first run per variant was excluded.
Timing covers open/unpack, as in the benchmark, and not the subsequent CFA copy.

| Variant | Median of three warm open/unpack runs |
| --- | --- |
| Upstream decoder | 512.35 ms |
| Project-patched decoder | 388.04 ms |

This is a 24.3% latency reduction on that sample. Metadata, normalized CFA,
clipping-mask hashes and clipping counts matched across all eight runs. This is
one local sample on one machine, not a claim about all Nikon or other RAW formats.
Both variants also rejected a copy truncated to 75% of the original file and
honored cancellation at the first progress callback. These checks do not establish
mid-unpack cancellation latency: this sample made only one callback during a
successful unpack, so an attempted 64th-callback cancellation did not trigger in
either variant. That existing cancellation limitation is separate from whether
the Nikon performance patch should be retained.

## References

* [vcpkg overlay ports](https://learn.microsoft.com/en-us/vcpkg/concepts/overlay-ports)
* [Pinned downloads and PATCHES in vcpkg_from_github](https://learn.microsoft.com/en-us/vcpkg/maintainers/functions/vcpkg_from_github)
* [Triplet variables](https://learn.microsoft.com/en-us/vcpkg/users/triplets)
* [Custom registries](https://learn.microsoft.com/en-us/vcpkg/concepts/registries)
* [librtprocess source baseline](https://github.com/CarVac/librtprocess/tree/9a858270acb2096e2e403d932760ee688fcac425)
* [LibRaw source baseline](https://github.com/LibRaw/LibRaw/tree/24fa7e5463cbf8b8615dbd2b16c933a294d52400)
* [Project RAW performance and validation history](raw-loading.md)
* [GPUI patch description](../../vendor/README.md)

## Migration validation

The managed Linux x86-64 build passed 125 native/engine unit tests, including
required codec availability, HDR round trips, SVG loading, cancellation and
serial/parallel equality. All 43 library/server/remote tests passed. The RAW
integration run passed eight cases on an unclipped NEF; the remaining highlight
reconstruction case passed after adding a clipped-highlight NEF fixture. No
production algorithm patch was removed to obtain these results.

The Aigle GUI, Aigle CLI, Thesauros and remote client all built successfully.
`ldd` on the CLI and server showed standard OS/compiler runtimes only, without
shared image-processing libraries. A real 2048 x 1364 PQ PNG was exported through
the rebuilt CLI to Ultra HDR JPEG with `LD_LIBRARY_PATH` unset; the managed
libvips tool recognized its embedded gain map. GUI platform libraries remain
host-provided as documented in the native setup guide.

GPUI patches were also independently reconstructed from the original 0.3.9 crate
archives and compared byte for byte with the current vendored files. Source
archives and compiler caches are build artifacts; the reviewable patch files,
archive checksums and overlay recipes are committed-source inputs.
