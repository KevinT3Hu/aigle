# aigle — Engine Pipeline Design

Status: agreed. Builds on `01-ui-engine-communication.md`.

Scope: this document covers coordinate spaces, the stage graph, how stages are evaluated (grids, regions of interest, tiles), color science, local adjustments, cache keys, quality tiers, process versioning, and the determinism policy. Later documents will cover:
- `03` memory and cache tiers (budgets, eviction, disk)
- `04` the CPU/GPU split and the CPU fallback
- `05` the scheduler and workers
- `06` analysis and the asset jobs (AI masks, AI denoise, auto-*)

## 1. Goals and non-goals

Goals:
- Edits stay interactive on 24–100 MP raws: a slider drag shows a frame within 1–2 display frames at fit-to-screen.
- A 1:1 preview matches the export within the determinism tolerance (§14).
- Re-rendering an old edit always gives the same result, because process and data are versioned (§13).
- The engine runs headless, and every stage has a CPU path. The CPU path must be correct, not fast.
- Professional quality first: Refined/Full renders and export are never approximated for speed; only Draft renders are (§12).
- Hardware: minimum spec 16 GB RAM with an integrated GPU (unified memory, ~60–120 GB/s); reference machine Ryzen 7 8845HS (Radeon 780M) with 32 GB. Memory bandwidth is the bottleneck, so stages are fused and intermediates kept in f16 wherever precision allows. Linux (Vulkan) first.

Deferred to after v1. The architecture keeps a hook for each:

| Feature | Hook |
|---|---|
| HDR/EDR display and export | E1 supports f16 extended-range output; D1 has an HDR rendering-transform variant |
| Soft proofing | a second output transform per View (`Overlays`/`ViewSpec`) |
| DNG GainMap / WarpRectilinear opcodes (phone DNGs) | GainMap goes in A2/A4; WarpRectilinear composes into B1 |
| Pixel shift, HDR/panorama merge | produce a linear DNG-like asset that enters the pipeline at A4 |
| Depth masks | an extra `MaskShape` component |

## 2. Coordinate spaces

| Space | Definition | Used by |
|---|---|---|
| Sensor | raw CFA pixel grid, unrotated | A1–A3 |
| Source | demosaiced grid with EXIF orientation applied (lossless 90°/flip); unit = level-0 source pixel | A4+, S tier, mask anchoring, assets |
| Crop | after lens correction, perspective, rotation and crop; the origin is the crop's top-left; unit = level-0 source pixel at the crop center | B1 output, all of tiers C–E |
| Grid | a raster sampling of a region of Source or Crop at a scale `s` (grid px per space unit) | every buffer |

- Masks, brush strokes, gradients and AI-mask assets are anchored in Source space, using coordinates normalized to the oriented source size. Crop, rotation and lens changes never shift a mask relative to its subject.
- `core` implements the forward and inverse geometry maps. The engine and the UI use the same code, so the UI can map cursors and overlays without a round trip to the engine.

## 3. Stage graph

```
file
 │
 ├─ A  SOURCE TIER — CPU, native libs, Sensor→Source grid, full resolution
 │   A1 Decode (LibRaw)          CFA u16, black/white levels, matrices, as-shot WB, metadata
 │   A2 Raw prep                 black/linearize, hot/dead pixels, raw CA correct, green eq.
 │                               → also emits a per-pixel clip mask (3 bits)
 │   A3 Demosaic (librtprocess)  Bayer: RCD (default) | AMaZE ; X-Trans: Markesteijn 3-pass (default)
│                               | passthrough (linear DNG)
 │        alt: AI-denoise asset  (replaces A3 output; raw model on A2 for Bayer, RGB model on A3 otherwise; doc 06 §4)
 │                               input premultiplied with REFERENCE WB (as-shot)
 │   A4 Highlight reconstruction sparse: only the dilated clip mask; keyed on quantized user WB (§7)
 │   A5 Pyramid                  area-filtered mip levels of camera RGB (reference WB)
 │
 ├─ S  SOURCE-GRID DETAIL — Source grid, at the pyramid level chosen by B1 (§8)
 │   S1 Noise reduction          AI-denoise blend amount, classic luma/chroma NR,
 │                               noise model scaled by pyramid level; local NR field
 │   S2 Capture sharpening       deconvolution; local sharpness field
 │   S3 Texture                  fine-scale log-luminance band (implementation note, §5.4)
 │
 ├─ B  GEOMETRY — single resample Source→Crop grid
 │   B1 orientation ∘ lens distortion/TCA (per channel) ∘ perspective ∘ rotation ∘ crop ∘ scale
 │      lens vignetting gain applied at source coordinates; Jacobian-aware filtering (§5.5)
 │
 ├─ M  MASK FIELDS — on whichever grid the consumer runs on (Source for S, Crop for C/D)
 │      rasterize mask alphas → accumulate sparse per-parameter fields (§9)
 │
 ├─ C  SCENE-REFERRED — Crop grid, linear Rec.2020, unbounded float
 │   C1 Input profile + WB + exposure  per-pixel; global matrix plus local exposure/temp/tint fields
 │   C2 Spatial tone & presence        highlights/shadows (tone equalizer), clarity, texture, dehaze
 │                                     computed at render resolution with scaled radii (§5.4)
 │
 ├─ D  RENDERING — Crop grid
 │   D1 Rendering transform     parametric sigmoid (contrast, whites, blacks; local fields),
 │                              hue-preserving, gamut compression → bounded display-referred
 │   D2 Creative color          tone curves, color mixer, grading, vibrance/saturation, look LUT;
 │                              evaluated directly per pixel in one fused kernel (§6.4)
 │   D3 Effects                 post-crop vignette, grain
 │
 └─ E  OUTPUT — Crop grid → output
     E1 Output transform        working → display/export profile; dithered quantization
     E2 Compose & overlays      tiles → exact viewport resample; clipping/mask overlays (views only)
     taps: histogram / analysis taken from the overview grid (§5.2)
```

### Ordering rationale

- **Demosaic with a fixed reference WB (A3) and apply the user's WB as a correction in C1.** WB changes never invalidate demosaic or the pyramid. Only the sparse highlight reconstruction (A4) depends on the user's WB.
- **NR and capture sharpening come before geometry (S tier).** Resampling correlates noise and changes its scale, so both must run on the original sampling. Output sharpening is an export-only option applied in E1, after resizing.
- **One resample (B1).** It is the only interpolation in the pipeline, which avoids cumulative softening.
- **Scene-referred ops (C) come before the rendering transform (D1).** WB, exposure, highlight/shadow recovery, local exposure and dehaze are physically meaningful in linear light. Curves, the mixer and grading behave the way users expect on the bounded, rendered image (D2).

## 4. `EditParams` and the stage → parameter map

Parameter paths follow user concepts. Each stage declares the paths it reads, so the struct's layout is independent of the stage graph. Names deliberately follow common Lightroom vocabulary for photographers and LLMs, but their behavior is aigle's own.

```rust
pub struct EditParams {
    pub process_version: ProcessVersion,   // §13
    pub raw:      RawParams,      // demosaic, ca_correction, hot_pixels, highlight_recon,
                                  // ai_denoise: Option<{ asset: AssetId, amount }>
    pub lens:     LensParams,     // profile: Off | Resolved(DataRef) ; distortion, tca, vignetting amounts
    pub geometry: GeometryParams, // angle, perspective/upright, crop (normalized), aspect
    pub profile:  InputProfile,   // camera profile: Matrix(DataRef) [default] | Dcp(DataRef) [user-imported]; look: Option<DataRef>
    pub wb:       WhiteBalance,   // mode: AsShot | Auto(resolved values) | Custom ; temperature K, tint
    pub light:    LightParams,    // exposure EV, contrast, highlights, shadows, whites, blacks
    pub presence: PresenceParams, // texture, clarity, dehaze, vibrance, saturation
    pub curve:    ToneCurve,      // parametric + point curves (rgb, r, g, b)
    pub mixer:    ColorMixer,     // 8 hue bands × hue/sat/lum
    pub grading:  ColorGrading,   // shadows/midtones/highlights/global wheels, blending, balance
    pub detail:   DetailParams,   // sharpen {amount, radius, detail, masking}, noise {luma, detail, contrast, chroma, …}
    pub effects:  EffectsParams,  // post-crop vignette, grain
    pub masks:    Vec<LocalAdjustment>,   // §9
}
```

**Value conventions (decided).**
- Values are stored in user-facing units, Lightroom-like: exposure ±5 EV, most sliders −100..+100, temperature 2000–50000 K, tint ±150. Exact per-field ranges live in the `schemars` schema.
- Ranges are hard-clamped for every writer: UI, agent, profile and XMP load. An out-of-range value from an agent or profile is clamped and the tool result reports the clamp.
- Stored precision (decided): each field is rounded to its own step, as Lightroom and RawTherapee do. Exposure uses 0.01 EV; most sliders, temperature (K) and tint use 1. Geometry (crop, brush strokes, gradients, curve points) uses 6 significant decimals, which keeps sub-pixel accuracy on 100 MP images. Rounding happens at commit in `doc`, so the stored edit is exactly the one that was rendered. Each field's step is declared in the schema.

Orientation is not an edit parameter. It is part of the Source space definition, and user 90° rotations are stored as `geometry.orientation_delta`.

| Stage | Reads |
|---|---|
| A1 | — (source file only) |
| A2 | `raw.hot_pixels`, `raw.ca_correction` |
| A3 | `raw.demosaic`, `raw.ai_denoise.asset` |
| A4 | `raw.highlight_recon`, `wb` (quantized), `profile` (matrix only) |
| A5 | — |
| S1 | `raw.ai_denoise.amount`, `detail.noise.*`, mask fields `noise` |
| S2 | `detail.sharpen.*`, mask fields `sharpness` |
| S3 | `presence.texture` (see the implementation note in §5.4) |
| B1 | `geometry.*`, `lens.*` |
| M | `masks[*]` (shape, invert, opacity, adjust) |
| C1 | `profile.*` (except look), `wb.*`, `light.exposure`, fields `exposure`, `temperature`, `tint` |
| C2 | `light.highlights`, `light.shadows`, `presence.clarity`, `presence.dehaze`, matching fields |
| D1 | `light.contrast`, `light.whites`, `light.blacks`, matching fields |
| D2 | `curve.*`, `mixer.*`, `grading.*`, `presence.vibrance`, `presence.saturation`, `profile.look`, fields `saturation`, `hue` |
| D3 | `effects.*` |
| E1 | none from `EditParams`; the View or export options supply the profile, bit depth and output sharpening |

`DataRef` is a content hash of resolved external data (a lens profile, camera matrix, DCP or look LUT), pinned at first use (§13.2).

## 5. Evaluation model

### 5.1 Demand-driven evaluation with ROI propagation

Rendering is pull-based. A request is a set of output tiles on a grid. Each stage maps the region it must produce back to the input region it needs:

```rust
pub trait Stage {
    fn id(&self) -> StageId;
    fn impl_version(&self, pv: ProcessVersion) -> u32;         // §13
    fn params_key(&self, p: &EditParams) -> ParamsKey;         // canonical hash of its param slice
    fn input_roi(&self, out: &Roi, ctx: &GridCtx) -> Roi;      // margins, or the inverse map for B1
    fn deps(&self) -> &[Dep];                                  // upstream image, global statistics, fields, assets
    fn run(&self, inputs: &Inputs, out: &mut Output, ctx: &ExecCtx) -> Result<(), StageError>;
}
```

- Spatial stages add margins scaled to the grid (`radius_src_px × s`).
- B1 maps the expanded crop-space ROI through the inverse geometry, adding a margin for the filter kernel, and produces a Source-space ROI at a pyramid level. S stages then run only on that ROI.
- Spatial ops read real pixels beyond the crop rectangle when they exist, so cropping does not change the interior. Outside the transformed source bounds they use mirror boundary handling.
- Statistics (histogram, dehaze airlight, analysis) are computed over the crop only. The histogram and analysis come from the overview (§5.2). A View's live histogram is always of the whole crop and depends only on the source and the params: it is computed from an overview render on a separate latest-wins task as soon as the params change (coarse during gestures, full overview once settled), and pan and zoom never recompute it. Statistics that change pixels, such as dehaze airlight, come from the statistics pass (§5.4) and never from the overview.

### 5.2 Grids

Every session maintains these grids. Each is keyed by its own upstream key (§11).

1. **Overview grid (UI only, decided).** The whole crop, rendered at a configurable resolution (default 1536 px long edge, about 1.6 MP). It is a UI setting: it is not part of the process version, and no export or pixel-affecting stage reads it. Changing it never changes how an edit looks. Its products:
   - the histogram and `analyze()` data
   - the navigator image
   - the edited thumbnail
   - the fallback frame used to reproject immediately during zoom and pan

   It is an ordinary render of the pipeline at the overview's scale, so its large-support ops are approximate in the same way as any render below 1:1 (§5.4).
2. **Detail tiles.** Crop-space tiles, 512×512 grid px by default and larger when active margins require it (§5.3), at quantized render scales `s_k = 2^(k/2)`. A View renders at the smallest quantized scale ≥ its zoom, and E2 resamples the tiles to the exact viewport size. Tiles are cached, so a pan renders only newly exposed tiles.
3. **Source ROIs.** The S-tier results for the regions demanded by tiles, at the selected pyramid level. They are cached as source-space tiles so that neighboring crop tiles and other Views share them.

### 5.3 Margins and tile sizing

- Each spatial stage declares its support radius, converted to grid px at the render scale. There is no global margin cap.
- Tile size adapts to the total margin `m` of the active stages: the tile edge `T` is the smallest power of two ≥ 512 for which `(T + 2m)² / T² ≤ 1.6` (at most 60% overhead), capped by the memory budget (doc 03). Above the cap the overhead is accepted.
- Large-support ops must use algorithms whose cost per pixel does not depend on the radius (box/guided filters, Gaussian/Laplacian pyramids). A large radius then costs memory for margins, not time per pixel.
- At fit-to-screen, margins are small in grid px and tiles stay at 512. At 1:1 and in export, large-support ops can need margins of several hundred px, and tiles grow accordingly.

**Implementation note (first implementation; for review).** C1/output tiles are a fixed 512 px: C2 takes its large support from the coarse field (§5.4), so the only C1 margin is the resampling support (a few px). The S tier runs on source blocks whose edge `T ≥ 256` follows the rule above with an overhead bound of 2 instead of 1.6, and a 48 MiB workspace cap per block. Larger blocks also compute more pixels outside a viewport; at 1:1 with the default detail settings, 512 px blocks (overhead 1.64) were 20 % slower than 256 px blocks.

### 5.4 Large-support ops (decided: render resolution, scaled radii)

This is the approach darktable and RawTherapee use. Highlights/shadows (tone equalizer), clarity, dehaze, and every other spatial op are computed at the render's own resolution. Their radii are converted to the grid scale. There is no shared coarse grid, and export does not depend on any preview product.

Consequences:
- At render scale ≥ 1:1, preview matches export within tolerance (§14).
- Below 1:1, these ops are approximations (§8). The difference is largest for strong clarity and highlights/shadows near high-contrast edges. The UI shows a "view at 1:1 to judge accurately" hint while such ops are active and the zoom is below 1:1.
- Export at a reduced size renders at full resolution and then resizes (§5.6), so the export result does not depend on the export size.

**Global statistics (decided).** Some ops need whole-image values, for example dehaze airlight, which no tile can compute alone. A *statistics pass* computes them over the whole crop, before tiles, at full resolution (pyramid level 0). The result is a handful of numbers, cached by the upstream key, and shared by every View and by export, so preview and export always use identical, exact values.
- Draft renders may use statistics from a coarse pyramid level, or the last full-resolution result, while a slider moves.
- Refined and Full renders wait for the full-resolution statistics.
- No resolution constant is involved, so nothing here needs process versioning beyond the algorithms themselves.

**Implementation note (first implementation; for review).** The engine departs from this section in three ways:
- **C2 large-support ops use a coarse field, not render-resolution filtering.** Dehaze, highlights/shadows and clarity get their large-support part from a small field computed once per (source, C1/C2 params) on the finest pyramid level whose long edge is ≤ 1024 px. For tone and clarity the field holds fast-guided-filter coefficients `a, b` of log luminance; for dehaze it holds transmission. The C1 tile kernel samples the field and applies it with the full-resolution pixel as guide. The field depends neither on the render scale nor on the tile grid, so preview at any zoom and export see the same field and C2 needs no tile margins. The cost is that these ops are a constant approximation of a full-resolution guided filter, rather than exact at ≥ 1:1.
- **Dehaze airlight is computed on the field level, not at level 0.** It is the mean colour of the 0.1 % brightest dark-channel pixels inside the crop. Preview and export still use identical values; the full-resolution statistics pass described above is not implemented (decided): at level 0 the 0.1 % brightest dark-channel pixels are biased upward by sensor noise, which the field level averages out, and the pass would add > 100 ms per C1 change on a 24 MP raw.
- **Texture runs in the S tier (S3), not in C2.** It is a fine-detail op in source-pixel units (§8), so it runs on the source grid after NR and sharpening, in reference-WB camera RGB.

### 5.5 Resampling (B1)

- The pyramid level is chosen per request from the Jacobian of the composed map. Distortion and perspective vary across the frame, so the choice is conservative: the finest level the region needs.
- The kernel footprint is scaled by the local Jacobian: Lanczos-3 for magnification and near 1:1, and an EWA or scaled kernel for minification. This prevents aliasing under strong distortion or perspective.
- TCA uses a separate map for each channel.

**Implementation note (first implementation; for review).**
- **Warp grid, not a per-pixel map.** Without lens or perspective correction the C1 tile map is affine and exact. Otherwise the host evaluates the full B1 map on a grid with a node every 16 grid px of each tile. Each node holds the R, G, B level positions, the vignetting gain, and the footprint √|det J|. The kernel interpolates bilinearly between nodes; the error is < 0.05 px for strong distortion plus perspective (test `warp_grid_is_accurate`).
- **Level choice.** One level per render, `level_for_scale(s · fill)`, where `fill` is the constrain-crop zoom. It is not chosen per region.
- **Filter.** Lanczos-3 widened by the footprint, with the footprint clamped to [1, 2]. A 2×2 anti-ringing clamp is applied. This is used for Refined, Full and export; Draft is bilinear. EWA is not used.
- **Out-of-source samples.** With `constrain` off, samples outside the source are black.
- **Lens data.** Profiles come from the lensfun database, which ships with aigle as files in `data/lensfun/version_1` and is read at runtime; nothing is compiled in. Each profile is pinned by content hash (§13.2) under the aigle data dir. Focus distance defaults to 1000 m and aperture to f/8 when EXIF lacks them.

### 5.6 Export

- Export uses the same tile machinery at full resolution, with adaptive tile sizes (§5.3) and the statistics pass (§5.4). It never reads the overview.
- By default it renders at full resolution, resamples to the requested size with a high-quality filter, and then applies optional output sharpening. A "fast" option renders directly at output scale, as previews do.
- GPU memory is bounded by tile size, so it does not depend on sensor size.

## 6. Color pipeline

### 6.1 Working space

Linear Rec.2020, D65 white, f32 on the CPU. GPU intermediates are f16 unless an op needs f32; this is finalized in doc 04. Values are unbounded: negative and >1 values are kept through tier C. Ops that take logs or ratios use a safe luminance `max(Y, ε)`.

### 6.2 C1: input profile, white balance, exposure

1. Undo the reference WB premultiply.
2. Convert the user's WB (temperature in K plus tint) to a chromaticity, using the Planckian locus with a tint offset along the isotherm (Robertson, DNG convention). As-shot multipliers map to temperature and tint through the camera matrix, iteratively as in the DNG spec.
3. Interpolate the camera matrices for the scene illuminant. With dual-illuminant DNG or DCP data, interpolate ColorMatrix/ForwardMatrix in inverse CCT. Otherwise use the single D65 matrix, as for most LibRaw cameras.
4. Convert camera → XYZ, adapt the scene illuminant → D65 with CAT16 (decided; chosen over Bradford for accuracy, since aigle does not aim to reproduce Adobe's WB behavior), then XYZ → linear Rec.2020.
5. Apply the DCP HueSatMap if present. It is a 3D table in HSV, applied per pixel.
6. Apply exposure as a scalar, including the DNG BaselineExposure.

**Implementation note (first implementation; for review).**
- **Calibration source.** DNG raws use the file's own IFD 0 calibration: ColorMatrix1/2, ForwardMatrix1/2, CameraCalibration1/2 and AnalogBalance. The embedded profile's HueSatMap/LookTable are not applied, since v1 uses standard matrices only. Other raws use LibRaw's single D65 matrix.
- **Dual-illuminant interpolation.** Interpolation is linear in inverse CCT and clamped, as in the DNG SDK. As Shot is found by iterating neutral → xy.
- **ForwardMatrix.** When present, it maps to D50, followed by CAT16 to D65. Otherwise the inverse ColorMatrix is used, with CAT16 from the scene white.
- **User DCPs.** These are imported through `Engine::import` and pinned as file bytes. With a HueSatMap or LookTable, the C1 matrix stops in linear ProPhoto (D50). A per-tile colour block then applies HueSatMap → exposure (including BaselineExposureOffset) → LookTable → Rec.2020. Interpolation follows the DNG SDK: hue wraps, and the table is bilinear in hue × saturation, or trilinear when there are value divisions, with sRGB value encoding if flagged. The value scale is applied as a ratio evaluated at min(V, 1), so scene values above 1 keep their headroom. The DCP ToneCurve is ignored, because D1 is the rendering transform.
- **Missing data.** A missing DCP renders with the camera matrix and does not share cache entries with the complete render.

Steps 1–4 and 6 fold into one 3×3 matrix and a scalar per pixel. Local temperature/tint fields are applied as a per-pixel diagonal adaptation in LMS on top of the global matrix: an approximation of a full re-adaptation that is exact at zero field.

### 6.3 D1: rendering transform

A parametric, hue-preserving sigmoid on a norm, with the scene-to-display mapping anchored at mid-grey. `contrast`, `whites` and `blacks` are sigmoid parameters and can vary per pixel through the fields. Gamut compression maps out-of-gamut working values smoothly into the rendering gamut, so the output is bounded [0,1] and display-referred in linear Rec.2020. The exact curve and gamut compressor are part of the process version.

> **Implementation note (not frozen yet).** D1 = ACES 1.3 reference gamut compression (distance-based, per-channel thresholds 0.815/0.803/0.880, limits 1.147/1.264/1.312, power 1.2) in Rec.2020 → log-logistic sigmoid on max(R,G,B) anchored at 0.18 → hue-preserving ratios with a path to white: ratios blend towards neutral by `(mapped/white)^8` (exponent 4 made +2.5 EV saturated colours pastel). E1 fits the output: the same gamut compression in the output primaries for negatives, then, for a component above 1, a blend towards the grey of equal luminance just far enough to reach 1; clip flags are computed before fitting. The curve becomes process version 1 at the first release. No side-by-side tuning against ACR or darktable was done (neither is available here); the defaults were checked visually on the sample raws.

### 6.4 D2: creative color, evaluated directly instead of baked (decided)

All D2 ops run in one fused per-pixel kernel. This is on the order of a few hundred flops per pixel, which is negligible on the GPU and acceptable on the CPU.
- 1D curves are sampled from 4096-entry 1D textures. The curve shapes (parametric bumps, Fritsch–Carlson monotone spline through the point curve) live in `aigle_core::params::tone`, so the GUI curve editor draws exactly the curve that is rendered.
- Mixer bands use smooth, periodic hue weights.
- Grading uses luminance-weighted wheels.

This replaces the earlier proposal to bake D2 into a 3D LUT. Direct evaluation avoids LUT precision problems with steep curves and narrow bands, gives preview and export the same code path, and lets mask fields (saturation, hue) vary per pixel.

3D LUTs are used only where the data is a LUT by definition: DCP LookTables and user look `.cube` files, applied with tetrahedral interpolation through a shaper.

**Implementation note (first implementation; for review).** The look `.cube` is applied first in D2, before curves, so that the user's D2 edits act on top of it. It is applied to the D1 output through an sRGB shaper on sRGB primaries, the convention for creative looks, which carry no colour-space tags. Out-of-sRGB parts of the colour pass around the LUT unchanged. `DOMAIN_MIN/MAX` and `LUT_1D_SIZE` are supported; a 1D LUT is expanded to a 33³ cube. DCP LookTables are HSV tables (§6.2), not 3D RGB LUTs.

### 6.5 E1: output

- Matrix/TRC ICC profiles are evaluated exactly in the shader (matrix plus curves).
- LUT-based profiles use an lcms2-built 65³ tetrahedral LUT.
- Quantizing to 8 bits uses position-seeded blue-noise dither, so it is deterministic.
- Clipping overlays are evaluated against the target gamut before encoding.

## 7. Highlight reconstruction and white balance

- A2 records which channels clipped at each pixel. A4 runs after demosaic, only inside the dilated clip mask. It reconstructs in WB-corrected camera space, using the user's WB quantized to 1% coefficient steps.
- **Draft** frames reuse the last reconstruction while WB is being dragged; only clipped areas can be slightly off. **Refined** recomputes A4 if the quantized WB changed, then rebuilds only the pyramid tiles that intersect the clip mask.
- The algorithm (color propagation, inpaint-opposed or a guided variant) is selected by `raw.highlight_recon`, and its behavior is versioned.

> **Implementation note.** A2 records clipped CFA sites per colour; after demosaic the mask is dilated to radius 2 ("clipped") and 5 ("ring"). The pre-A4 data of every 64² tile touching the mask is kept, so A4 re-runs from it on a copy of level 0 (≈40–100 ms on 24 MP) and only the pyramid tiles above it are rebuilt; a fresh decode and a re-run give bit-identical results. Changing only the A4 key never re-decodes; the session re-runs it on `Final` edits and keeps the previous result during continuous gestures. `Off` clips only masked pixels (to the lowest user-WB clip level), not the whole image. Inpaint-opposed uses cube-root chroma sampled in the ring (falling back to all rings when a channel's ring lies inside another channel's clip). Guided fits local linear models on the unclipped other channel(s) within 12 px and falls back to the opposed estimate. Colour propagation runs librtprocess on connected tile clusters with a 64 px margin. White levels come from RawTherapee's `camconst.json`, bundled as `data/camconst/camconst.json` and read at runtime (`$AIGLE_CAMCONST`, then the user data dir, then the bundled copy); it is not used for DNG files.

## 8. Noise, sharpening and scale awareness

- S1 and S2 run on Source-grid ROIs at the pyramid level that B1 samples from. Level 0 is used at ≥1:1 and for export.
- **Noise model.** Per-camera and per-ISO noise parameters (shot and read noise) come from bundled data, or are estimated from the raw if absent. At pyramid level `L` the noise variance is scaled by `4^-L`. A variance-stabilizing transform is applied before luma NR.
- Chroma NR runs in an opponent space derived from reference-WB camera RGB, which is near-neutral. It is therefore independent of later user WB changes.
- **Scale-aware ops** (NR, capture sharpening, texture, grain) specify radii in level-0 source pixels. When the effective radius on the grid falls below about 0.5 px, the op uses a perceptual approximation documented per op. Below 1:1 these ops are approximate by design. At ≥1:1 and in export they are exact.
- Large-support ops (highlights/shadows, clarity, dehaze) are scale-aware in the same way (§5.4), but their radii are specified as a fraction of the crop's long edge (decided). They look the same on a 24 MP and a 100 MP camera, and profiles transfer between cameras. Fine-detail ops (NR, sharpening, texture, grain) keep source-pixel units because they act on pixel-level detail.

> **Implementation note (noise model).** The raw noise model `(S, O)` is the DNG NoiseProfile per plane (IFD 0 or the main raw sub-IFD); it is part of the A-tier key. darktable's per-camera profiles were evaluated and are not bundled: on raw green CFA sites of three cameras the measured variance slope is a consistent 8.5–9× darktable's green `a`, and the reason is not established. It is carried into the stored data (reference WB, green = 1) and the S-tier luma assuming independent channels and a near-neutral pixel, giving `var(Y) = a·Y + b`. The luma NR runs on the matching VST `z = √(Y + b/a)` with `σ_z = √a / 2`, halved per pyramid level. Without a NoiseProfile (all non-DNG raws, most DNGs, rendered images) σ is measured from the pyramid level (robust MAD of a Laplacian, pure-Poisson VST), as doc 06 §2 allows. The analysis noise estimate reports level-0 σ of luma and chroma at mid-grey (0.18 stored), from the same source. `S_IMPL` = 2.

## 9. Local adjustments

```rust
pub struct LocalAdjustment {
    pub id: MaskId,
    pub name: String,
    pub shape: MaskShape,
    pub invert: bool,
    pub opacity: f32,
    pub adjust: LocalParams,  // sparse: exposure, temperature, tint, contrast, highlights, shadows,
                              // whites, blacks, texture, clarity, dehaze, saturation, hue, sharpness, noise
}

pub enum MaskShape {
    Brush { strokes: Vec<Stroke> },          // source-space vectors, size/feather/flow per stroke
    Linear { from: Pt, to: Pt },
    Radial { center: Pt, radii: Vec2, angle: f32, feather: f32 },
    Semantic { asset: AssetId, refine: EdgeRefine },
    Range { kind: Luminance | Color { samples, tolerance }, smoothness: f32 },
    Combine { op: Add | Subtract | Intersect, parts: Vec<MaskShape> },
}
```

### 9.1 Composition semantics

- **The alpha of one mask.** `Combine` evaluates as:
  - `Add` = max
  - `Subtract` = `a·(1−b)`
  - `Intersect` = `a·b`

  `invert` and `opacity` are applied after that, giving `αᵢ(x) ∈ [0,1]`.
- **Combining masks.** For each local parameter `p`, `field_p(x) = clamp(Σᵢ αᵢ(x)·Δᵢ,p, range_p)`. Overlapping masks stack additively, and the result does not depend on mask order; order is only a UI concern.
- **Combining with global params.** Each parameter's schema defines how its field combines with the global value. Most are additive in the parameter's natural unit (EV, slider units).

### 9.2 Cost

- Each mask costs one alpha rasterization (SDF for gradients, stroke rasterization for brushes, sampling for assets).
- Fields are sparse: a plane is allocated only for parameters with a non-zero Δ in at least one mask. One fused kernel accumulates them.
- The cost is O(masks × pixels) for alphas plus O(active params) planes, which scales to dozens of masks. No per-mask pipeline or LUT is built.
- Fields are rasterized on the grid of the consumer. `noise` and `sharpness` are rasterized on the Source grid, where mask anchoring makes the mapping trivial. All other fields are rasterized on the Crop grid.

### 9.3 Range components

Range components are evaluated on pixels transformed by the global C1, without local fields, so there is no circularity. On a Source grid, C1 is applied on the fly; it is a per-pixel matrix.

### 9.4 Semantic masks

- A semantic mask is an asset in Source space at model resolution, with guided-filter or matting edge refinement applied when sampled.
- The asset records the A-tier key of the image it was computed from. If that key changes (for example when AI denoise is toggled), the mask is marked outdated. It keeps rendering with the old asset until the user or agent updates it.
- Creative edits never make a mask outdated. Masks are computed from a standardized render, not the user's look.

## 10. Assets

Assets are content-addressed results of heavy jobs, referenced from params by `AssetId`:

| Asset | Key includes |
|---|---|
| AI denoise | A2 key (raw model, Bayer) or A3 key (RGB model, X-Trans/fallback), model id + version, tiling version (doc 06 §4) |
| Semantic mask | A-tier key, target, model id + version, refine version (doc 06 §5) |
| Auto WB / auto tone / upright | inputs only; these produce params values, not pixels |
| Resolved data (`DataRef`) | content hash of the lens profile, camera matrix, DCP or look |

- **AI-denoise strength** is the S1 blend `amount`. It is not part of the asset key, so moving the slider does not re-run the model.
- **Storage.** Semantic-mask pixels persist in the photo's `.aigle-masks` file (doc 01 §9). AI-denoise results are large and reproducible from the model id and version, so they live only in the disposable cache (`~/.cache/aigle/`) and are recomputed if the cache is evicted. RAM/disk budgets are covered in doc 03. Job execution is covered in docs 05 and 06.

## 11. Cache keys

```
key(stage, grid) = H( stage_id,
                      stage.impl_version(process_version),
                      key(upstream inputs, their grids),
                      stage.params_key(EditParams),
                      grid descriptor (space, level/scale, tile index or ROI),
                      quality tier,
                      referenced AssetIds / DataRefs,
                      global statistics consumed (§5.4) )
```

- H is BLAKE3 over a canonical serialization. Floats are hashed by bit pattern after normalizing `-0.0`, and NaN is rejected during params validation.
- Dirty tracking follows directly from the keys. Before/after Views, snapshots, pending proposals and agent candidate renders share every stage whose key matches.

## 12. Quality tiers

| Tier | Used for | Allowed approximations |
|---|---|---|
| Draft | while a param is changing | render one quantized scale lower if the previous frame exceeded budget; stale A4 during WB drags; stale global statistics if the statistics pass has not caught up |
| Refined | the state has been stable for about 150 ms | none at the View's render scale; full-resolution global statistics (§5.4) |
| Full | render scale ≥ 1:1, export | none; S tier at level 0 |

The frame budget and the adaptive Draft policy are defined in doc 05.

## 13. Process versioning

### 13.1 Process version

- `process_version` is set when a photo is first edited. New photos get the latest version. Existing photos keep theirs until the user (or an agent, through a Bulk-risk tool) upgrades them. The upgrade shows a before/after.
- Each stage has `impl_version(pv)`. A stage whose behavior changes gets a new implementation, and the old ones are kept. The engine supports every shipped process version.
- The parameter defaults and neutral values may depend on the process version.
- Native library upgrades (LibRaw, librtprocess, lcms2) are pinned. An upgrade that changes golden outputs beyond tolerance is handled as a process change.
- The dither pattern is part of the process version. The overview resolution is not (§5.2).
- The params *schema* version (serialization format) is separate from the process version and is migrated in place.

### 13.2 Data pinning

External data (lensfun entries, camera matrices, DCPs, noise profiles, looks) is resolved once and pinned in the photo's XMP sidecar as a `DataRef` (doc 01 §9.5). Small data is embedded in the XMP. Large data is referenced by content hash and looked up in the bundled data and `~/.local/share/aigle/data/`, and a missing item produces a warning rather than a silently different render. Updating a database does not change existing edits. Re-resolving is an explicit action.

> **Implementation note (bundled data).** Third-party reference data ships as plain files in `data/` and is read at runtime: `camconst/camconst.json` (RawTherapee, GPL-3.0-or-later) and `lensfun/version_1/` (lensfun, CC-BY-SA-3.0). `data/manifest.toml` records each dataset's upstream, pinned revision, licence, attribution and the SHA-256 of every file; `cargo xtask data --update` refreshes it from upstream and `--check` verifies it. Each item can be overridden by an environment variable or a copy at the same relative path under `~/.local/share/aigle/data/`; installations of other applications are never read. The lookup is in `crates/engine/src/data.rs`.

## 14. Determinism and consistency policy

| Path | Guarantee |
|---|---|
| A tier (CPU, native) | bitwise deterministic for the same binary and platform; verified in CI |
| CPU reference path (S–E) | bitwise deterministic for the same binary and platform, with SIMD dispatch level pinned; selected by `--deterministic` |
| GPU path | within tolerance of the CPU reference (decided): ≤ 1 code value in 8-bit output, ΔE2000 p99 ≤ 0.5, max ≤ 2. The code-value bound holds for all but ≤ 10⁻⁵ of channel values (never more than 2): an f16 rounding flip in an intermediate is amplified by the steep start of the sRGB curve on near-zero channels (observed: one pixel of 1.09 M, blue 2 vs 4, ΔE 0.3) |
| 16-bit / float export | GPU export to 16-bit TIFF or float formats runs all S–E intermediates in f32 and reads an f32 source re-derived at export time (decided, doc 03 §9). AI-denoise assets are stored in f32. Previews and 8-bit exports may use f16 intermediates and the f16 pyramid where precision allows |
| Preview vs export | at render scale ≥ 1:1, within GPU tolerance; below 1:1, the scale-aware ops (§8), including large-support ops (§5.4), are approximate |
| Tiled vs untiled | within GPU tolerance; seams are a bug |

- Export uses the GPU when available, and `aigle profile apply` follows the same rule.
- `--deterministic` forces the CPU reference path for bitwise-reproducible output.
- Random processes (grain, dither) are seeded from Crop-space position and the photo id, so panning and re-rendering are stable.
- Histograms use integer bins with no float atomics.

## 15. Non-raw and pre-demosaiced inputs

- **Linear DNG / already-demosaiced raw.** A3 runs in passthrough. A4 runs if clip information is available.
- **Monochrome raw.** No demosaic. C1 uses a luminance-only profile.
- **JPEG/TIFF/HEIF/PNG.** These enter at A5. The embedded ICC (or sRGB) is linearized into camera RGB = working primaries, and the reference WB is identity. User WB adapts relative to an assumed D65 scene. Highlight reconstruction is unavailable.

## 16. Testing invariants

- Golden images per process version and per camera family. Any change in output beyond tolerance fails CI.
- CPU reference vs GPU, within tolerance, for every stage in isolation and for the whole pipeline.
- Tiled vs untiled, and different tile sizes, give identical results within tolerance. This catches margin bugs.
- **Margin sufficiency.** Increasing a stage's declared margin must not change its output.
- At 1:1, a View tile matches the corresponding region of the export.
- **Cache key completeness.** A randomized param mutation must change the key of exactly the stages that read the mutated path. This is checked against the §4 table.
- **Mask anchoring.** Changing crop, rotation or lens leaves mask alphas fixed in Source space.

## 17. Decisions log (all resolved)

1. ~~Overview resolution / coarse fields.~~ Decided: the overview is UI-only and configurable (§5.2). Large-support ops are computed at render resolution with scaled radii (§5.4). Global statistics are computed at full resolution (§5.4). Large-support radii are relative to the crop's long edge (§8).
2. ~~Default CAT.~~ Decided: CAT16 (§6.2).
3. ~~Default demosaic.~~ Decided: Bayer → RCD, AMaZE selectable per photo; X-Trans → Markesteijn 3-pass (§3, A3).
4. ~~Bundled camera profiles.~~ Decided: v1 ships only standard color matrices (LibRaw / DNG data). Users can import their own DCPs (including Adobe's from their own install) into `~/.local/share/aigle/data/`; Adobe DCPs are never redistributed. Chart-based aigle profiles may come later.
5. ~~Whether local adjustments need curves or a color mixer.~~ Decided: no, in v1 (doc 01 §11). Masks are scalar-only, so the §9.2 cost model stands.
6. ~~Tolerance thresholds.~~ Decided (§14). If the first GPU/CPU comparison runs show they can't be met with f16 previews, the fix is more f32 stages, not looser thresholds.
