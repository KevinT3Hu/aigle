//! Request, response and event types of the engine API (doc 01 §3, doc 06 §2).
//! Everything is `serde`-serialisable so the engine can later move into a subprocess.

use std::path::PathBuf;
use std::sync::Arc;

use aigle_core::geometry::{ImagePoint, ImageRect, Size};
use aigle_core::masks::MaskTarget;
use aigle_core::{MaskId, Revision};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------------------------
// Engine configuration and events

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProcessKind {
    /// GUI: budget = 25% of RAM, 3–24 GB (doc 03 §2.1).
    #[default]
    Gui,
    /// CLI: budget = 50% of RAM, 3–48 GB.
    Cli,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum BackendPreference {
    #[default]
    Auto,
    Gpu,
    Cpu,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EngineConfig {
    pub process: ProcessKind,
    /// Override of the engine memory budget in bytes (`--memory`).
    pub memory_budget: Option<u64>,
    pub backend: BackendPreference,
    /// Force the bitwise-reproducible CPU reference path (doc 02 §14).
    pub deterministic: bool,
    /// Overview grid long edge in px; a UI setting, not part of the process version (doc 02 §5.2).
    pub overview_long_edge: u32,
    /// Engine worker threads; `None` = hardware threads − 1.
    pub worker_threads: Option<usize>,
    /// Disposable disk cache (doc 03 §7); `None` = `$AIGLE_CACHE_DIR`, else
    /// `$XDG_CACHE_HOME/aigle`, else `~/.cache/aigle`.
    pub cache_dir: Option<std::path::PathBuf>,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            process: ProcessKind::Gui,
            memory_budget: None,
            backend: BackendPreference::Auto,
            deterministic: false,
            overview_long_edge: 1536,
            worker_threads: None,
            cache_dir: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BackendKind {
    Gpu,
    Cpu,
}

/// Engine-wide events (doc 01 §4 "Errors").
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum EngineEvent {
    BackendSelected { kind: BackendKind, reason: String },
    DeviceLost { recovered: bool },
    MemoryPressure { used: u64, budget: u64 },
    Error { message: String },
}

// ---------------------------------------------------------------------------------------------
// Sources and sessions

/// Synthetic test images, for headless development and tests without sample files.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SyntheticPattern {
    /// Horizontal exposure ramp × vertical hue sweep.
    Gradient,
    /// 6×4 colour patches with a neutral row.
    ColorChart,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SourceRef {
    File(PathBuf),
    Synthetic {
        pattern: SyntheticPattern,
        width: u32,
        height: u32,
    },
}

impl From<PathBuf> for SourceRef {
    fn from(p: PathBuf) -> Self {
        SourceRef::File(p)
    }
}

/// File extensions (lower case) decoded as camera raws.
pub const RAW_EXTENSIONS: &[&str] = &[
    "3fr", "arw", "cr2", "cr3", "crw", "dng", "erf", "iiq", "kdc", "mef", "mos", "mrw", "nef",
    "nrw", "orf", "pef", "raf", "rw2", "rwl", "sr2", "srf", "srw", "x3f",
];

/// File extensions (lower case) of the rendered image formats the engine opens.
pub const RASTER_EXTENSIONS: &[&str] = &[
    "jpg", "jpeg", "tif", "tiff", "png", "webp", "heic", "heif", "avif",
];

fn extension_in(path: &std::path::Path, list: &[&str]) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| list.contains(&e.to_ascii_lowercase().as_str()))
}

/// `path` names a camera raw (by extension).
pub fn is_raw_path(path: &std::path::Path) -> bool {
    extension_in(path, RAW_EXTENSIONS)
}

/// `path` names a file the engine can open (by extension).
pub fn is_photo_path(path: &std::path::Path) -> bool {
    extension_in(path, RAW_EXTENSIONS) || extension_in(path, RASTER_EXTENSIONS)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SourceKind {
    Raw,
    /// JPEG/TIFF/PNG/HEIF: enters the pipeline at A5 (doc 02 §15).
    Rendered,
    Synthetic,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CameraInfo {
    pub make: String,
    pub model: String,
    pub iso: Option<f32>,
    pub exposure_time: Option<f32>,
    pub f_number: Option<f32>,
    pub focal_length: Option<f32>,
    /// 35 mm-equivalent focal length, if recorded.
    #[serde(default)]
    pub focal_length_35mm: Option<f32>,
    #[serde(default)]
    pub lens_make: Option<String>,
    #[serde(default)]
    pub lens_model: Option<String>,
    /// Lens focal range (min, max) in mm, if recorded.
    #[serde(default)]
    pub lens_focal_range: Option<(f32, f32)>,
    #[serde(default)]
    pub shot: ShotInfo,
}

/// Further EXIF facts about the capture, for display and for exported files. Codes are the
/// EXIF values (`ExposureProgram`, `MeteringMode`, `Flash`, `ExposureMode`, `WhiteBalance`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ShotInfo {
    /// `DateTimeOriginal` as recorded (`YYYY:MM:DD HH:MM:SS`, camera-local time).
    pub date_time: Option<String>,
    /// `OffsetTimeOriginal`, e.g. `+08:00`.
    pub utc_offset: Option<String>,
    /// EV.
    pub exposure_bias: Option<f32>,
    pub exposure_program: Option<u16>,
    pub metering_mode: Option<u16>,
    pub flash: Option<u16>,
    pub exposure_mode: Option<u16>,
    pub white_balance: Option<u16>,
    /// Maximum aperture of the lens at the focal length used, as an f-number.
    pub max_aperture: Option<f32>,
    pub body_serial: Option<String>,
    pub lens_serial: Option<String>,
    pub firmware: Option<String>,
    pub artist: Option<String>,
    pub copyright: Option<String>,
    pub gps: Option<GpsPosition>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct GpsPosition {
    /// Degrees, north positive.
    pub latitude: f64,
    /// Degrees, east positive.
    pub longitude: f64,
    /// Metres above sea level.
    pub altitude: Option<f32>,
}

/// Kind of user-imported colour data (doc 01 §9.5, doc 02 §13.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ImportKind {
    /// A DNG camera profile (`.dcp`): set `profile.camera = CameraProfile::Dcp(data_ref)`.
    Dcp,
    /// A `.cube` look LUT: set `profile.look = Some(data_ref)`.
    LookLut,
    /// A display ICC profile (matrix/TRC): render to `DisplayProfileId::Icc(data_ref)`.
    DisplayProfile,
}

/// Imported data, validated and pinned under the aigle data dir.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Imported {
    pub data_ref: aigle_core::DataRef,
    /// Profile name / LUT title, if the file has one.
    pub name: String,
    /// DCP: the camera model it was made for (`UniqueCameraModel`).
    pub camera_model: Option<String>,
}

/// A lens-profile candidate from the lensfun database, already pinned: set
/// `EditParams::lens.profile = LensProfile::Resolved(data_ref)` to use it (doc 02 §13.2).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LensMatch {
    pub data_ref: aigle_core::DataRef,
    pub lens_maker: String,
    pub lens_model: String,
    pub score: i32,
    pub has_distortion: bool,
    pub has_tca: bool,
    pub has_vignetting: bool,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ImageInfo {
    pub kind: SourceKind,
    /// Oriented size (Source space, before `orientation_delta`).
    pub source_size: Size,
    /// EXIF orientation applied to reach Source space.
    pub exif_orientation: u16,
    pub camera: Option<CameraInfo>,
    /// As-shot white as temperature/tint (informational; As Shot mode uses the exact white).
    pub as_shot: WbValuesDto,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct WbValuesDto {
    pub temperature: f32,
    pub tint: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum LoadState {
    Loading,
    Ready(Arc<ImageInfo>),
    Failed(String),
}

/// Hint whether an update is part of a continuous gesture (doc 05 §4.1).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Interaction {
    Continuous,
    #[default]
    Final,
}

/// Priority within a scheduler class; higher first.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Priority(pub i32);

// ---------------------------------------------------------------------------------------------
// Views and frames

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DisplayProfileId {
    #[default]
    Srgb,
    DisplayP3,
    /// Adobe RGB (1998). Output only.
    AdobeRgb,
    /// ProPhoto (ROMM) RGB. Output only; best with 16-bit formats.
    ProPhoto,
    /// ITU-R BT.2020 primaries with the BT.709 transfer curve. Output only.
    Rec2020,
    /// Content hash of a display ICC profile registered with the engine.
    Icc(String),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Viewport {
    /// Visible area in Crop-space (post-crop image) coordinates.
    pub region: ImageRect,
    /// Physical canvas pixels.
    pub output_size: (u32, u32),
    /// The engine performs the display colour transform (doc 01 §4).
    pub display: DisplayProfileId,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ParamsSource {
    #[default]
    Current,
    Pending,
    /// The unedited photo with the head's framing (`EditParams::original_framing`).
    Original,
    Snapshot(Revision),
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Overlays {
    pub clipping: bool,
    pub mask_overlay: Option<MaskId>,
    pub focus_peaking: bool,
    /// Soft proof: show colours as this output space would hold them (fitted to its gamut).
    #[serde(default)]
    pub proof: Option<DisplayProfileId>,
    /// With `proof`: mark colours outside the proof gamut.
    #[serde(default)]
    pub gamut_warning: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ViewSpec {
    /// `None` = fit the whole crop into `fit_size` once the image is loaded.
    pub viewport: Option<Viewport>,
    pub fit_size: (u32, u32),
    pub params: ParamsSource,
    pub overlays: Overlays,
}

impl ViewSpec {
    pub fn fit(w: u32, h: u32) -> Self {
        ViewSpec {
            viewport: None,
            fit_size: (w, h),
            params: ParamsSource::Current,
            overlays: Overlays::default(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Quality {
    Draft,
    Refined,
    Full,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PixelFormat {
    Bgra8,
    Rgba16F,
}

impl PixelFormat {
    pub fn bytes_per_pixel(self) -> usize {
        match self {
            PixelFormat::Bgra8 => 4,
            PixelFormat::Rgba16F => 8,
        }
    }
}

/// Pooled, immutable pixel storage.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PixelBuffer {
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub format: PixelFormat,
    pub data: Arc<[u8]>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Frame {
    /// Revision that produced it.
    pub rev: Revision,
    /// What it actually covers (may lag the request).
    pub viewport: Viewport,
    pub quality: Quality,
    pub pixels: PixelBuffer,
}

/// 256-bin display-referred histogram.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Histogram {
    pub red: Vec<u32>,
    pub green: Vec<u32>,
    pub blue: Vec<u32>,
    pub luma: Vec<u32>,
}

// ---------------------------------------------------------------------------------------------
// One-shot job requests and results

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Thumbnail {
    pub pixels: PixelBuffer,
    /// Taken from the embedded preview instead of rendered.
    pub embedded: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum EncodedFormat {
    Jpeg {
        quality: u8,
    },
    /// 8-bit, lossless.
    Png,
    /// Lossless (deflate). `sixteen_bit` renders from an f32 source with f32 intermediates
    /// (doc 03 §9).
    Tiff {
        sixteen_bit: bool,
    },
    WebP {
        quality: u8,
    },
    /// 8-bit AV1 in HEIF.
    Avif {
        quality: u8,
    },
    /// 8-bit HEVC in HEIF; needs a libheif HEVC encoder, so it may be unavailable
    /// (`aigle_engine::format_available`).
    Heic {
        quality: u8,
    },
    /// 8-bit JPEG XL.
    Jxl {
        quality: u8,
    },
    /// Export only: the raw sensor data as a DNG, with the edits left to the sidecar.
    Dng,
}

impl EncodedFormat {
    /// File extension, without the dot.
    pub fn extension(self) -> &'static str {
        match self {
            EncodedFormat::Jpeg { .. } => "jpg",
            EncodedFormat::Png => "png",
            EncodedFormat::Tiff { .. } => "tif",
            EncodedFormat::WebP { .. } => "webp",
            EncodedFormat::Avif { .. } => "avif",
            EncodedFormat::Heic { .. } => "heic",
            EncodedFormat::Jxl { .. } => "jxl",
            EncodedFormat::Dng => "dng",
        }
    }
}

impl Default for EncodedFormat {
    fn default() -> Self {
        EncodedFormat::Jpeg { quality: 90 }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EncodedImage {
    pub format: EncodedFormat,
    pub width: u32,
    pub height: u32,
    pub bytes: Arc<[u8]>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OffscreenOpts {
    /// Default ~1024 to limit vision-token cost.
    pub long_edge: u32,
    pub format: EncodedFormat,
    /// Side-by-side before (Original) | after.
    pub before_after: bool,
}

impl Default for OffscreenOpts {
    fn default() -> Self {
        Self {
            long_edge: 1024,
            format: EncodedFormat::default(),
            before_after: false,
        }
    }
}

/// Sharpening for the output medium, applied after resizing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum OutputSharpening {
    #[default]
    Off,
    Low,
    Standard,
    High,
}

/// Which metadata an export carries.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MetadataPolicy {
    /// Camera, lens, exposure, capture date, artist and copyright.
    #[default]
    All,
    /// Artist and copyright only.
    CopyrightOnly,
    /// No EXIF at all (the colour profile is still embedded).
    Strip,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ExportOptions {
    pub path: PathBuf,
    pub format: EncodedFormat,
    /// Resize after a full-resolution render (doc 02 §5.6); `None` = full size.
    pub long_edge: Option<u32>,
    pub output: DisplayProfileId,
    #[serde(default)]
    pub sharpening: OutputSharpening,
    #[serde(default)]
    pub metadata: MetadataPolicy,
    /// Replace the source's artist / copyright when set (ignored with `Strip`).
    #[serde(default)]
    pub artist: Option<String>,
    #[serde(default)]
    pub copyright: Option<String>,
    /// Drawn over the output after resizing and sharpening (not on DNG exports).
    #[serde(default)]
    pub watermark: Option<Watermark>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum WatermarkContent {
    /// One line of text, drawn with the system's sans-serif font.
    Text(String),
    /// An image file (PNG, SVG, …); its transparency is kept.
    Image(PathBuf),
}

/// Where the watermark sits in the output.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Anchor {
    TopLeft,
    Top,
    TopRight,
    Left,
    Center,
    Right,
    BottomLeft,
    Bottom,
    #[default]
    BottomRight,
}

impl Anchor {
    pub const ALL: [Anchor; 9] = [
        Anchor::TopLeft,
        Anchor::Top,
        Anchor::TopRight,
        Anchor::Left,
        Anchor::Center,
        Anchor::Right,
        Anchor::BottomLeft,
        Anchor::Bottom,
        Anchor::BottomRight,
    ];

    /// Horizontal and vertical position: 0 start, 1 centre, 2 end.
    pub fn grid(self) -> (u32, u32) {
        let i = Anchor::ALL.iter().position(|a| *a == self).unwrap_or(8) as u32;
        (i % 3, i / 3)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Watermark {
    pub content: WatermarkContent,
    /// Height of the watermark as a fraction of the output's short edge.
    pub size: f32,
    pub anchor: Anchor,
    /// Distance from the edges as a fraction of the output's short edge.
    pub margin: f32,
    /// 0 (invisible) to 1 (opaque).
    pub opacity: f32,
    /// Text colour, 8-bit sRGB.
    pub color: [u8; 3],
}

impl Default for Watermark {
    fn default() -> Self {
        Watermark {
            content: WatermarkContent::Text(String::new()),
            size: 0.04,
            anchor: Anchor::BottomRight,
            margin: 0.03,
            opacity: 0.6,
            color: [255, 255, 255],
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExportResult {
    pub path: PathBuf,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SampleStage {
    /// Scene-referred linear Rec.2020 after C1.
    SceneLinear,
    /// Display-encoded output values (0..1).
    Display,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PixelSample {
    pub at: ImagePoint,
    pub radius: f32,
    pub stage: SampleStage,
    pub rgb: [f32; 3],
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WbEstimate {
    pub temperature: f32,
    pub tint: f32,
    pub confidence: f32,
    pub low_confidence_reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MaskRequest {
    pub target: MaskTarget,
}

// ---------------------------------------------------------------------------------------------
// Analysis (doc 06 §2)

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AnalysisSet {
    pub histogram: bool,
    pub clipping: bool,
    pub wb: bool,
    pub noise: bool,
    /// Full-resolution statistics instead of the overview (doc 06 §2).
    pub exact: bool,
    /// Output profile the histogram and clipping are measured in (doc 06 §2).
    #[serde(default)]
    pub output: DisplayProfileId,
}

impl Default for AnalysisSet {
    fn default() -> Self {
        Self {
            histogram: true,
            clipping: true,
            wb: true,
            noise: true,
            exact: false,
            output: DisplayProfileId::Srgb,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AnalysisPrecision {
    Overview,
    Exact,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChannelStats {
    pub mean: f32,
    /// (percentile, value 0..1), e.g. (0.1, …), (50.0, …), (99.9, …).
    pub percentiles: Vec<(f32, f32)>,
    pub counts: Vec<u32>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HistogramStats {
    pub red: ChannelStats,
    pub green: ChannelStats,
    pub blue: ChannelStats,
    pub luma: ChannelStats,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClipStats {
    /// % of pixels at/over output white, per channel R, G, B.
    pub highlights_pct: [f32; 3],
    /// % of pixels at/under output black, per channel.
    pub shadows_pct: [f32; 3],
    /// % raw-clipped at the sensor; `None` for non-raw sources.
    pub sensor_clipped_pct: Option<f32>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NoiseEstimate {
    pub luma_sigma: f32,
    pub chroma_sigma: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Analysis {
    /// Hex cache key of the render the statistics describe.
    pub rev_key: String,
    pub precision: AnalysisPrecision,
    pub histogram: Option<HistogramStats>,
    pub clipping: Option<ClipStats>,
    pub wb_estimate: Option<WbEstimate>,
    pub noise_estimate: Option<NoiseEstimate>,
}
