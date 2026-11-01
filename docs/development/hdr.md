# HDR import, editing, export and native preview

HDR working pixels are linear Rec.2020. Diffuse white is **203 cd/m²** and the
current HDR rendering/output intent has a **1000 cd/m²** mastering peak. Values
above diffuse white survive edits, tile caching, linear-light viewport resampling
and float16 frame transport. The HDR tone mapper has a smooth highlight shoulder;
it does not use the SDR sigmoid's unit-white bound. Bounded creative LUTs/curves
operate on normalized RGB and restore highlight scale afterward. Exposure, white
balance, detail, masks, retouching, framing, creative color and effects share the
existing editing session. HDR histogram bins use PQ normalized to 1000 cd/m²;
203 cd/m² is not treated as highlight clipping.

## Use

Open a photo normally. The existing image preview canvas automatically uses native
HDR when Wayland and Vulkan support it. **View → HDR Canvas** or **Ctrl+Shift+H**
toggles HDR for that same canvas. Edits, zoom, pan, crop, before/after split and
histograms follow the selected output mode. Tool handles, menus and other editor
controls remain GPUI overlays above the image. No companion window is created.

The image pixels bypass GPUI's BGRA8 atlas. A native Wayland subsurface sits below
the editor surface, with a transparent hole painted into GPUI at the canvas's
position. The child has an empty input region, so interaction stays with the
editor. Enable HDR for the intended output in your compositor and move the editor
onto that display; the application does not change desktop HDR settings.

The native Wayland client follows output enter/leave events, output image
description changes and the surface's preferred color description. Its status
names the current output and distinguishes HDR targets, SDR targets and missing
luminance metadata. The reported target range is the compositor's description,
**not a measurement of panel luminance**. Physical brightness and calibration
remain the responsibility of the compositor/display configuration.

## Files

| Input | HDR handling |
| --- | --- |
| Raw | Existing scene-linear processing; HDR is selected at output/view time. |
| OpenEXR | Float RGB, embedded chromaticities and whiteLuminance, adapted to D65. Without these attributes, linear BT.709/sRGB and relative diffuse white are assumed. First flat RGB(A) layer; deep data is unsupported. |
| Float TIFF | Linear float data; embedded RGB ICC transforms run directly through lcms float transforms, retaining values above 1 and negative wide-gamut components. Untagged float TIFF follows libvips' scRGB interpretation. |
| PNG | PQ/HLG CICP, full-range RGB; inverse transfer and primaries conversion before editing. |
| HEIF/AVIF | PQ/HLG NCLX/CICP; reads primary-item property associations even with libvips versions which discard NCLX. Requires the corresponding libvips decoder/codec plugin. Ambiguous derived-item profiles fail explicitly. |
| JPEG XL | libvips float/ICC decoding and HDR cICP in ICC profiles; requires its decoder. |
| Radiance HDR | libvips Radiance-to-float/XYZ conversion. |
| Gain-map images | Reconstructed with `vips_uhdr2scRGB` when libvips exposes a gain map (libvips ≥8.18 with the relevant gain-map decoder). Builds without that codec may only expose the embedded SDR rendition. |

The export dialog offers **Ultra HDR JPEG (SDR + gain map)**, **PNG HDR (16-bit PQ)**, **TIFF HDR (float32)** and
**OpenEXR HDR (float32)**. HDR formats select their encoding automatically:

* Ultra HDR JPEG: an SDR base JPEG plus an embedded gain map, generated from the
  rendered linear HDR image by libvips/libultrahdr. Ordinary JPEG viewers display
  the SDR base; compatible HDR viewers reconstruct HDR. The quality control applies
  to both JPEG layers. Requires libvips ≥8.18 built with libultrahdr; the option is
  hidden when that optional encoder is unavailable, and CLI export fails explicitly.
  The SDR base is tone-mapped by libultrahdr, so it can differ from the editor’s
  separate SDR rendering. Gain-map encoding currently requires full-image memory
  inside the codec despite the engine’s streamed rendering.
* PNG: full-range BT.2100 PQ/Rec.2020, 16-bit, `cICP` and conservative `cLLi`
  metadata. Values are limited to the 1000-nit mastering peak.
* TIFF: float32 linear sRGB with a matching linear matrix/TRC ICC profile.
  Negative/out-of-unit sRGB values retain wide-gamut colors.
* EXR: lossless float32 linear sRGB, explicit BT.709 chromaticities and
  `whiteLuminance=203`.

All four use the same streamed float rendering, linear-light Lanczos resizing,
linear RGB output sharpening and diffuse-white watermarking. Existing SDR
formats retain their SDR rendering; requesting HDR pixels in an SDR export format
fails instead of silently quantizing them. EXR/PNG retain supported author,
copyright and software fields; TIFF and Ultra HDR JPEG use the existing EXIF
policy. HDR AVIF/HEIC and HDR JPEG XL export remain unsupported.

### Viewer compatibility

HDR PNG requires both a CICP/PQ-aware decoder and an HDR presentation path. Opening
its pixels as ordinary sRGB produces different brightness/colors, even when the
file has correct HDR data and the desktop has HDR enabled. The export dialog
mentions this requirement.

imv 5.0.1 is an SDR viewer for this purpose: its libpng backend calls
`png_set_strip_16`, and its Wayland EGL configuration requests 8-bit RGB without
negotiating an HDR image description. A live protocol trace confirmed that behavior
on the development system. An inspected 2048 × 1364 export had valid full-range
Rec.2020/PQ CICP, highlights at 1000 cd/m², and 44.6% of pixels above 203 cd/m²;
it still appeared SDR in imv. Exported luminance/color is now compared numerically
with the engine's native-HDR preview frame on neutral and saturated patches.

[HDRView](https://github.com/wkjarosz/hdrview) documents CICP PNG decoding and native
Wayland HDR output through its patched GLFW backend. Use a build with that backend
for an external HDR comparison.

CLI examples:

```sh
cargo run --release -p aigle-cli -- export -f ultra-hdr -o /tmp/hdr-export photo.exr
cargo run --release -p aigle-cli -- export -f hdr-png -o /tmp/hdr-export photo.exr
cargo run --release -p aigle-cli -- export -f hdr-tiff -o /tmp/hdr-export photo.exr
cargo run --release -p aigle-cli -- export -f exr -o /tmp/hdr-export photo.exr
```

## Presentation and precision

The native canvas requires Linux Wayland, subsurfaces, viewporter and
color-management-v1 with Rec.2020/PQ descriptions. Vulkan WSI must expose an
explicit **BT.2100 PQ** surface with **Rgb10a2Unorm** or **Rgba16Float** format.
Vulkan owns its surface color-management object. RGBA16F image textures are
composed in linear light and encoded to PQ in a GPU shader. Unsupported systems
retain the SDR canvas and report HDR unavailability.

The app borrows an owned clone of GPUI's Wayland connection and parent surface.
Its display worker dispatches a private event queue; GPUI continues to own socket
reading. Latest-wins scene updates wake the worker through eventfd. Textures are
reused when frames are unchanged. Current detail and overview textures reproject
immediately during navigation, and the overview covers newly exposed regions
while detailed tiles arrive. Both sides of split comparison use the native path.
Canvas layout follows GPUI bounds and display scale. The worker shuts down before
the child surface is destroyed.

Three GPUI 0.3.10 crates in the pinned `vendor/gpui` fork submodule supply the
small platform/renderer changes:
an owned native-surface accessor and an ordered, clipped transparent-hole draw.
The latter replaces destination alpha rather than blending transparent black;
subsequent tool overlays remain visible. See `../../vendor/README.md`.

There
is still CPU transport between engine output and presentation: this is not shared
engine/display texture interop. HDR tone/color/output processing currently runs
on the host; C1 can run on the engine GPU with readback. Host C1 tiles are shared
with SDR where possible. Cached HDR output tiles occupy 16 bytes/pixel, frame
transport 8 bytes/pixel, versus 4 bytes/pixel for SDR; engine tiles are charged to
the existing cache ledger. The native presenter's small texture/swapchain working
set is outside the engine ledger, like GPUI's existing display textures.

## Reproducible scene and display test

```sh
cargo run --release -p aigle-engine --example hdr_fixture -- /tmp/aigle-hdr
AIGLE_HDR_PROBE_SECONDS=30 \
  cargo run -p aigle-app --bin aigle-gui -- --hdr-probe /tmp/aigle-hdr/studio-scene.exr
```

The deterministic ray-traced studio is 1536×960, has reflective metal/red objects,
a bright window, and scene-input luminance patches at 0, 0.1, 1, 10, 80, 203,
1000 and 4000 cd/m². 14.8% of its pixels exceed diffuse white. Outputs are
`studio-scene.exr`, `studio-hdr.png` and `studio-sdr.jpg`. Patch labels describe
**input scene values**; the editor's highlight shoulder changes their rendered
values. The PNG is a rendered 1000-nit export; the EXR retains the 4000-nit source.
These are synthetic test images, not camera photographs or display calibration
measurements.

`--hdr-probe` opens the actual editor with the supplied file (default:
`/tmp/aigle-hdr/studio-scene.exr`) and closes it after 30 seconds. Set
`AIGLE_HDR_PROBE_SECONDS` to 5–300 seconds to change this. On the development
machine, Umbriel/DP-1 accepted the embedded Vulkan **Rgb10a2Unorm / BT.2100 PQ**
surface. Logs confirmed the first native frame inside the editor and the GPUI
canvas hole. Umbriel reports a 10000 cd/m² target volume; that is not a measurement
of the panel's physical peak. DP-1 is configured for HDR, 3840×2160/120 Hz at scale
2; the HDMI output is configured separately without HDR.

## Verification and performance

Automated coverage includes PQ anchors/inverse transfer, HLG white/black,
HEIF primary versus auxiliary color descriptions, malformed metadata, float TIFF
wide-gamut/highlight retention, OpenEXR chromaticities/white luminance, HDR frame
format/histogram, and HDR export round trips with resizing, sharpening and
watermarks. With the optional Ultra HDR codec installed, tests also check embedded
gain-map metadata, legacy SDR JPEG decoding and reconstructed HDR highlights.
Without it, tests verify explicit failure without creating an output file. The GPUI GPU integration test checks that the native hole clears the
background, respects clipping and preserves subsequent tool overlays. The actual
editor probe exercises Vulkan shader compilation, swapchain negotiation and
embedded presentation. Earlier visual approval covered the former standalone
test; it does not establish visual approval of the embedded canvas. Codec-dependent
AVIF tests require an installed encoder/plugin.

The existing pan/zoom benchmark supports HDR:

```sh
VIEWPORT_BENCH_HDR=1 cargo run --release -p aigle-engine --example viewport_bench -- cpu
```

One development run on the 24 MP synthetic source, 1920×1080 output, default detail,
measured 184.66 ms median first-pass zoom, 14.78 ms median cached zoom and 21.30 ms
median cached pan. The first-pass maximum remained 766.09 ms (pan). This was a
single run with other development work in progress, not a controlled before/after
comparison. Timings stop at engine frame publication and exclude native upload,
presentation, refresh and physical display latency. The source is the benchmark's
existing synthetic gradient; see `../performance/viewport.md` for its cache and
background-work caveats. This establishes a reproducible HDR benchmark, not a
claim that all first-visit detail latency is eliminated.

References: [Wayland color management](https://wayland.freedesktop.org/docs/book/Color.html),
[color-management-v1](https://gitlab.freedesktop.org/wayland/wayland-protocols/-/blob/main/staging/color-management/color-management-v1.xml),
[wgpu surface color spaces](https://docs.rs/wgpu/30.0.1/wgpu/enum.SurfaceColorSpace.html).
