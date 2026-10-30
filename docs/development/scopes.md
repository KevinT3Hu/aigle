# Image scopes

The Develop panel offers RGB and luminance histograms, separate channel histograms,
cumulative luminance, luminance waveform, RGB parade, and a Cb/Cr vectorscope. A compact
icon selector sits below the graph; tooltips identify each scope and its purpose. Histogram height is linear by default;
Log switches distribution/density scaling. Cumulative luminance always uses 0–100%.
R/G/B buttons control the RGB histograms and parade. Scaling, region selection,
and reset use icon buttons with tooltips. The information icon explains reference
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
not scan pixels or invoke the engine. The thumbnail submits at most roughly 32×32
rectangles. Region dragging re-analyzes the bounded samples.

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
cargo test -p aigle-app --bin aigle-gui scopes::tests
cargo test --release -p aigle-app --bin aigle-gui scope_analysis_benchmark -- --ignored --nocapture
```
