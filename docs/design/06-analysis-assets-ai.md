# aigle — Analysis, Assets and AI Features

Status: agreed; model selection waits on a prototype (§9). Builds on docs 01–05.

Scope: `analyze()`, auto white balance and auto tone, AI denoise, semantic masks, the asset lifecycle, and how AI models are delivered and pinned. Backend selection and inference scheduling are covered in doc 04 §7 and doc 05 §7.

## 1. Decisions

| # | Decision |
|---|---|
| Q38 | AI denoise runs on raw CFA data (joint demosaic + denoise) for Bayer sensors. X-Trans and any sensor without a raw model use an RGB model on the demosaiced image. |
| Q39 | v1 semantic targets: subject, sky, background, and people with parts (face skin, body skin, hair, eyes, lips, clothes). Click or text-prompt selection is deferred. |
| Q40 | Auto WB is an image-statistics estimate guided by camera information. It falls back to As Shot when confidence is low. Auto tone uses classic statistics. The agent can refine either result. |
| Q41 | Models are downloaded on first use, checksummed and version-pinned, with an offline install option. |
| Q42 | `analyze()` returns histogram stats, clipping, the WB estimate and the noise estimate. It includes no scene detection and no scores. |
| Q43 | Rendered statistics come from the overview by default; `exact: true` renders at full resolution. The result states which was used (§2). |
| Q44 | The GUI asks once per model before its first download, showing size and license (§7). |

## 2. `analyze()`

```rust
pub struct Analysis {
    pub rev_key: CacheKey,            // which params/upstream this describes
    pub precision: AnalysisPrecision, // Overview | Exact: which one produced the rendered statistics
    pub histogram: HistogramStats,    // per channel + luminance: percentiles p0.1..p99.9, mean, 256-bin counts
    pub clipping: ClipStats,          // % pixels at/over output white and black, per channel; % raw-clipped (sensor)
    pub wb_estimate: Option<WbEstimate>,  // §3.1, with confidence
    pub noise_estimate: Option<NoiseEstimate>,
}
```

- **Rendered statistics** (histogram, output clipping) describe the given params after the output transform (E1), over the crop, in the output/display profile the caller names (sRGB by default for agents).
- **Sensor clipping** comes from the A2 clip mask at full resolution. It is always exact and costs nothing extra.
- **Noise estimate** comes from the raw noise model (doc 02 §8) evaluated at the photo's ISO. If no bundled profile exists for the camera, it is measured from flat regions of the A2 output. It is reported as the luma and chroma standard deviation at mid-grey in level-0 source pixels. It does not depend on the edit params.
- **WB estimate** is the Auto WB result (§3.1), so an agent can compare it with the current WB without applying it.
- **Cost and precision (decided).** By default, rendered statistics come from the overview render (doc 02 §5.2). This is fast, but it can miss small clipped areas such as specular highlights, and large-support ops are approximate there. With `AnalysisSet { exact: true, .. }` the whole pipeline is rendered at full resolution through the export tile path, at roughly the cost of an export, and the statistics are exact. `precision` reports which one was used. Sensor clipping, the noise estimate and the WB estimate do not depend on this flag. Agents are expected to iterate on overview results and confirm with `exact: true` before committing; the `analyze_photo` tool exposes the flag.

## 3. Auto adjustments

Auto adjustments are *parameter producers*. They run once, write concrete values into `EditParams`, and are stored like any other edit. A later algorithm change therefore never changes an existing edit. The Auto mode is recorded only as provenance (`wb.mode = Auto { resolved }`).

### 3.1 Auto white balance

Input: linear camera RGB with reference WB undone, from a pyramid level of about 2 MP. Pixels are excluded if they are sensor-clipped (A2 mask, dilated), within 2% of black, or saturated after the camera matrix. When the user drags a rectangle, the estimate is restricted to that region (`Session::auto_white_balance(region)`).

The white balance picker (`Session::neutral_white_balance(at, radius)`) is not an estimate: the user asserts that the clicked spot is neutral. It averages the usable camera RGB (reference WB undone, sensor-clipped and near-black pixels excluded) in the disc around the point, on the finest pyramid level where the disc covers at most 4096 px, and returns the white of that mean under the calibration C1 renders with. There is no locus band, estimator fusion or prior; the result is only clamped to the `wb.*` ranges, and `low_confidence_reason` reports the clamp. It fails if the disc has no usable pixels or the point lies outside the crop. The GUI writes the result as `wb.mode = custom`.

Estimation:

1. **Candidate illuminants.** Several statistics estimators run in parallel: shades-of-grey (p=6), grey-edge (first order), white-patch on robust bright percentiles, and a near-neutral-pixel vote. Each produces a camera-space illuminant.
2. **Physical constraint.** Each candidate is projected through the camera matrix to a chromaticity and constrained to the Planckian locus band: temperature 2000–15000 K, |Duv| ≤ 0.02.
3. **Camera prior.**
   - The as-shot multipliers act as a prior centred on the camera's own estimate.
   - If the maker notes contain a measured colour temperature or a light-source tag, that is used as an additional, tighter prior. Coverage depends on the camera and on what LibRaw exposes; to be verified per maker in the prototype.
   - The fused estimate is the prior-weighted robust mean of the candidates, computed in CCT/Duv space.
4. **Confidence** is computed from the agreement between candidates and the share of usable pixels. Below a threshold, the result is As Shot, and the estimate reports `low_confidence` together with a reason (for example "dominant single colour" or "too few neutral pixels").

Output: temperature (K) and tint, rounded to the param steps (doc 02 §4), plus confidence. The algorithm is deterministic and runs on the CPU backend kernels (doc 04), so its results are identical on every backend.

> **Implementation note.** `crates/engine/src/awb.rs`. Grey-edge uses p = 1 (p = 6 was dominated by a few strong colour edges). The near-neutral vote is the luminance-weighted mode of per-pixel chromaticities in 10 mired × 4 tint bins. Fusion is an iteratively reweighted (Cauchy) mean in mired/tint with the as-shot white as a half-weight prior; LibRaw exposes no maker-note colour temperature, so there is no second prior. Confidence = mean Cauchy agreement of the candidates × usable share (saturating at 20 %) × colour diversity (RMS log-chromaticity spread, saturating at 0.3); the diversity term is skipped for a user region, which is meant to be neutral. Threshold 0.3. The calibration is the one C1 renders with (DCP if selected).

### 3.2 Auto tone

Auto tone sets `light.exposure`, `contrast`, `highlights`, `shadows`, `whites` and `blacks` from statistics of the scene-referred image (C1 output with the current WB) at about 2 MP:

- Exposure places a robust, centre-weighted log-luminance mean at mid-grey. It accounts for exposure compensation and DNG BaselineExposure.
- Whites and blacks place the p99.9 and p0.1 percentiles just inside the output range after the rendering transform.
- Highlights and shadows are set from the share of energy in the top and bottom stops, with limits so that results stay moderate.
- Contrast is set from the interdecile spread.

It is deterministic and runs on the CPU kernels. The agent can then adjust the values through `apply_edits`. The exact mapping constants are part of the auto-tone algorithm version, which is recorded in the provenance field but has no effect on rendering.

## 4. AI denoise

### 4.1 Placement

```
Bayer:   A2 (CFA, black-subtracted, normalised) ──► raw model ──► linear camera RGB (f32)  ⟶ replaces A3
X-Trans / other / no raw model:
         A3 (demosaiced camera RGB)           ──► RGB model ──► linear camera RGB (f32)  ⟶ replaces A3
```

- **Inputs.** The model receives the noise level from the camera noise model at the photo's ISO, as a conditioning input or by input scaling, depending on the model. Inputs use the reference WB (doc 02 §3), so user WB changes never invalidate the asset.
- **Highlight reconstruction (A4)** runs on the asset afterwards, as on normal demosaic output.
- **`raw.demosaic`** has no effect while AI denoise is active.
- **Tiling.** Tiles overlap and seams are blended with raised-cosine weights. Tile size comes from the per-model calibration (doc 05 §7). The CFA phase is preserved at tile boundaries.
- **Strength.** `raw.ai_denoise.amount` blends the asset with the normal A3 output in S1 (doc 02 §4). At amount = 1 only the asset is needed. Below 1, both sources are needed for the tiles being rendered, and the admission estimate includes both (doc 03 §6).

### 4.2 Asset key

- Raw model: the A2 key + model id + model version + tiling version.
- RGB model: the A3 key + the same fields.

The asset lives in the disk cache (doc 02 §10). If it is evicted, it is recomputed from the pinned model version (§6.3).

## 5. Semantic masks

### 5.1 Targets

```rust
pub enum MaskTarget {
    Subject,
    Sky,
    Background,                       // = inverse of subject, refined separately
    People { which: PersonSel, part: PersonPart },
}
pub enum PersonSel  { All, Index(u32) }               // Index into detected people, §5.4
pub enum PersonPart { Whole, FaceSkin, BodySkin, Hair, Eyes, Lips, Clothes }
```

Profile and agent syntax: `"subject"`, `"sky"`, `"background"`, `"people"`, `"people.hair"`, `"person[1].face_skin"`, and so on. The schema lists the valid strings.

### 5.2 Input render

Masks are computed from a *standardised render*, not from the user's look (doc 02 §9.4):
- the A-tier output (including AI denoise if active)
- As Shot WB, default tone, no creative ops, no masks
- an sRGB output transform
- the source orientation, with no crop or geometry, so the mask stays anchored in Source space
- resized to the model's input resolution

Creative edits therefore never make a mask outdated.

### 5.3 Output and refinement

- The model produces a soft alpha (u8 or u16) at model resolution in Source space.
- Edge refinement is a guided filter against the full-resolution luminance. It runs at sampling time on the render grid (doc 02 §9.4), so edges stay sharp at 1:1 without storing a full-resolution mask.
- One `compute_mask` job may produce several assets from one model run, for example all people parts. They share the inference cost.

### 5.4 People indexing

People are indexed by detection order sorted by area, largest first. Each asset stores its person's bounding box and centroid in Source space. When a mask is recomputed (§5.5), people are re-matched to the stored boxes by IoU, so `person[1]` keeps referring to the same person. An unmatched person makes the mask empty and is reported as a warning.

### 5.5 Outdated masks

A mask is outdated when the A-tier key of the image it was computed from no longer matches the current one, for example after toggling AI denoise or changing hot-pixel settings. An outdated mask keeps rendering with its old asset. The UI and `get_photo_state` flag it. "Update" recomputes it with the same target and the current model version, which produces a new `MaskAssetId`, as one undo step.

### 5.6 Empty results

If the model finds nothing (no sky, no people), the asset is a valid all-zero mask with a `found: false` flag. Profiles report this per photo in the CLI summary (doc 05 §10). It is not an error.

## 6. Assets

### 6.1 Masks file (`IMG_0001.aigle-masks`)

```
header: magic, format version
entries: [ MaskAssetId, model id + version, refine version, source A-tier key,
           target, person box (if any), width, height, bit depth, found,
           zstd-compressed alpha ]
```

- Writes are atomic and debounced together with the sidecar (doc 01 §9.3).
- **Pruning.** While a photo is open, unreferenced entries are kept, because undo may bring them back. On close, entries referenced neither by the head nor by any named snapshot are removed.

### 6.2 Disk-cache assets

AI-denoise results are stored as zstd-compressed f32 tiles under `~/.cache/aigle/assets/` (doc 03 §7). They are evicted last within the cache, and recomputed when missing.

### 6.3 Model pinning

- Every asset records the exact model id and version that produced it.
- Semantic masks are persisted, so rendering them never needs the model.
- AI denoise is recomputed from the recorded version if its cached asset is missing. If that version is not installed, aigle downloads it from the manifest (§7). If it can no longer be obtained, aigle warns and offers to re-run with the current version. The result may differ, so a new asset id is created and the user sees a before/after.

## 7. Model delivery

- **Manifest.** The app ships a signed manifest listing every model and version aigle has ever used, with: id, version, task (raw-denoise Bayer / RGB denoise / segmentation …), input domain, download URLs, SHA-256, size, license, and the minimum aigle version. Old versions stay in the manifest, so pinned assets remain reproducible.
- **Storage.** Models go to `~/.local/share/aigle/models/<id>/<version>/`. This is not the cache: recomputing a denoise asset depends on the pinned model. aigle never deletes models automatically. Settings lists installed models with sizes and allows removing them, with a warning that recomputing assets made with that version will need a re-download.
- **First use (GUI).** The first time a feature needs a model, aigle asks once per model, showing the size and the license, then downloads in the background. Progress appears in the job view. Downloads are resumable and verified against the SHA-256 before use. They fetch files only; no user data is sent.
- **Offline.** `aigle models install <archive>` and a settings import accept an archive containing the models and a manifest excerpt. The archive is verified by checksum.
- **CLI.** The CLI uses installed models and never downloads on its own, to stay stateless (doc 01 §8). If a profile needs a missing model, it fails with the exact `aigle models install` or `aigle models fetch <id>` command to run.
- **Licensing.** Only models whose weights and code have AGPL-compatible licenses are listed (doc 01 §11). The manifest records each license, and the About screen shows them.

## 8. Testing

- **Auto WB:**
  - a corpus of raws with measured neutral references (grey card or ColorChecker), scored by angular error against ground truth, compared with As Shot
  - regression thresholds per process version
  - low-confidence cases must return As Shot
- **Auto tone:** determinism, plus limit tests (no clipping beyond a threshold on a reference corpus).
- **AI denoise:**
  - CPU vs GPU backend agreement within the per-model tolerance (doc 04 §7.1)
  - tiled vs single-tile agreement, which catches seam bugs
  - CFA phase invariance (crop the raw by 1 px, compare the overlapping region)
- **Masks:**
  - anchoring under crop/rotate/lens changes (doc 02 §16)
  - outdated detection when the A-tier key changes
  - people re-matching on recompute
  - masks-file round trip and pruning
- **Manifest:** checksum verification, resume after interruption, refusal of a corrupted model file.

## 9. Open items

- Model selection waits on the inference-backend prototype (doc 04 §7.3).
- Which maker-note light-source / colour-temperature tags LibRaw exposes, per maker (§3.1).
