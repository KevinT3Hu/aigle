# Image scopes

The Qt Develop reference rail offers RGB and luminance histograms, separate channel histograms,
cumulative luminance, luminance waveform, RGB parade, and a Cb/Cr vectorscope. The scope selector sits above the graph; help explains the measurement limits. Histogram height is linear by default;
Log switches distribution/density scaling. Cumulative luminance always uses 0–100%.
R/G/B buttons control the RGB histograms and parade. Scaling, region selection,
and Whole crop controls sit below the graph. The help button explains reference
guides and sampled statistics. Hover shows levels, histogram
counts, cumulative population, image-column positions, or Cb/Cr coordinates.

HDR uses the existing PQ-normalized 0–1000 cd/m² scale. The gold guide marks
203 cd/m² diffuse white; the peak is 1000 cd/m². HDR waveform luminance is computed
in linear Rec.2020 before PQ encoding. Vectorscope chroma uses encoded RGB and
Rec.2020 coefficients for HDR, and encoded sRGB with Rec.709 coefficients for SDR.
The square vectorscope includes primary/secondary reference targets and a
conventional 123° skin-hue reference. These guides do not detect skin or prove gamut
membership. Color conversion coefficients follow the
[ITU-R BT.2020 specification](https://www.itu.int/rec/r-rec-bt.2020/en).

Enable Region and drag on the small, SDR-tonemapped thumbnail to measure a rectangle
in the edited crop. A click measures one sample cell; Whole crop resets the region.
The thumbnail is an analysis aid, not an HDR preview. Crop and local adjustments
are already included in the analyzed rendering. The region is independent of edit
masks and does not modify the document.

Full-crop histogram bins remain exact at the histogram render resolution. Spatial
scopes, region histograms, and percentages use at most 128×128 regularly spaced
samples; transparent pixels are excluded. The engine publishes these with the
existing asynchronous histogram result, without an additional image render. The
panel caches distributions and densities, so hovering and changing scope type do
not scan pixels or invoke the engine. The Qt thumbnail displays the bounded sample grid. Region dragging re-analyzes
the samples through the bridge with coalesced 60 ms requests; newer region/photo
requests supersede old replies. Changing the sample-grid dimensions resets a
selected region to the whole crop. Both frontends use `aigle_engine::scopes`.

Keyboard: focus the graph and use arrows/Home/End for cached hover measurements.
Focus the region thumbnail and use arrows to move, Shift+arrows to resize, Home
for Whole crop, and Escape to cancel a drag. These controls take priority over
image-navigation shortcuts while focused. Scope mode, channels, scaling and gamut
target persist in Qt settings; measurement regions never modify edits.

Black/Peak are approximate percentages touching output channel boundaries, with
half-float tolerance at the HDR ceiling. They are not sensor clipping or
pre-output/export clipping measurements. Gamut is the percentage with a negative
linear component after conversion to the chosen sRGB, Display P3, or Rec.2020 target;
it measures chromatic gamut independently of brightness. Choose the target matching
the intended export. These output samples cannot recover information already
clipped by SDR rendering, and tiny features can fall between spatial samples.

Checks:

```sh
cargo test -p aigle-engine --lib analysis::tests
cargo test -p aigle-engine --lib scopes::tests
cargo test -p aigle-qt-bridge --lib scopes_measure
ctest --test-dir target/qt -R qt-scopes --output-on-failure
cargo test --release -p aigle-engine --lib scope_analysis_benchmark -- --ignored --nocapture
```

Native screenshots and verification: [Qt review](../design/qt-review/native/README.md).
The SDR captures do not certify physical HDR presentation or display transitions.
