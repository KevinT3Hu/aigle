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

The Qt editor uses upstream QRhi/Vulkan HDR swapchains. Run
`target/qt/apps/aigle/aigle-gui --hdr PHOTO` or toggle **View → HDR Canvas**.
See [Qt frontend details and verification](../../apps/aigle/README.md) for
presentation formats, display requirements and probe commands. The legacy
Wayland subsurface frontend and patched GPUI renderer have been removed.

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
