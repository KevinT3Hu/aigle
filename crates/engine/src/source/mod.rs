//! A tier: decode → raw prep → demosaic → highlight reconstruction → pyramid (doc 02 §3).
//!
//! * Raw files: LibRaw + librtprocess (`raw`), then sparse A4 (`highlights`), which can be
//!   re-run for a new WB / algorithm without re-decoding ([`SourceImage::with_highlights`]).
//! * Rendered files (JPEG/PNG/TIFF) and synthetic sources enter at A5 (doc 02 §15).
//!
//! TODO(A3 alt): AI-denoise asset replacing the demosaic output (doc 06 §4).

pub mod highlights;
mod pyramid;
mod raster;
mod raw;
mod synthetic;

use std::path::Path;
use std::sync::Arc;

use aigle_core::EditParams;
use aigle_core::color;
use aigle_core::geometry::Size;
use aigle_core::params::Demosaic;
use aigle_engine_api::{CancelToken, ImageInfo, NoiseEstimate, SourceKind, SourceRef, WbValuesDto};
use highlights::{HlBase, HlKey};
pub use pyramid::{Level, Level32, Pyramid};
pub use raw::embedded_preview;

use parking_lot::Mutex;

use crate::cache::{CacheKey, KeyBuilder};
use crate::ledger::{Allocation, MemClass, MemLedger};
use crate::noise::{self, NoiseLevel, NoiseModel, NoiseSource};
use crate::profile::CameraCalibration;

/// Bumped whenever decoder output changes (part of the A-tier key).
const DECODER_VERSION: u32 = 3;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoadError {
    Cancelled,
    Failed(String),
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoadError::Cancelled => f.write_str("cancelled"),
            LoadError::Failed(e) => f.write_str(e),
        }
    }
}

impl From<String> for LoadError {
    fn from(s: String) -> Self {
        LoadError::Failed(s)
    }
}

/// The decode slice of `EditParams` (doc 02 §4: A2, A3 rows); changing it re-decodes. A4
/// (`raw.highlight_recon` and the quantised user WB) is keyed separately ([`HlKey`]) and
/// re-derived from the decoded source.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ATier {
    pub demosaic: Demosaic,
    pub ca_correction: bool,
    pub hot_pixels: bool,
}

impl ATier {
    pub fn of(p: &EditParams) -> ATier {
        ATier {
            demosaic: p.raw.demosaic,
            ca_correction: p.raw.ca_correction,
            hot_pixels: p.raw.hot_pixels,
        }
    }

    /// A-tier params have no effect on non-raw sources.
    pub fn normalized_for(self, raw: bool) -> ATier {
        if raw {
            self
        } else {
            ATier::of(&EditParams::default())
        }
    }
}

/// Colour description of the stored camera RGB (doc 02 §6.2).
#[derive(Clone, Debug, PartialEq)]
pub struct SourceColor {
    /// Reference WB multipliers premultiplied into the stored data (as-shot for raws),
    /// normalised to green = 1.
    pub reference_wb: [f64; 3],
    /// The camera's colour calibration (camera RGB, reference WB undone).
    pub calibration: Arc<CameraCalibration>,
    /// Exact as-shot white chromaticity under `calibration` (used by `WbMode::AsShot`).
    pub as_shot_xy: (f64, f64),
    /// DNG BaselineExposure in EV, folded into C1 (doc 02 §6.2 step 6).
    pub baseline_exposure: f64,
}

impl SourceColor {
    /// Rendered and synthetic inputs: camera RGB = working primaries, identity reference WB,
    /// assumed D65 scene (doc 02 §15).
    pub fn working_space() -> Self {
        SourceColor {
            reference_wb: [1.0; 3],
            calibration: Arc::new(CameraCalibration::single(
                color::invert(&color::REC2020_TO_XYZ).expect("invertible"),
            )),
            as_shot_xy: color::D65_XY,
            baseline_exposure: 0.0,
        }
    }

    pub fn new(
        reference_wb: [f64; 3],
        calibration: CameraCalibration,
        baseline_exposure: f64,
    ) -> Self {
        let as_shot_xy = calibration.neutral_to_xy(reference_wb.map(|v| 1.0 / v));
        SourceColor {
            reference_wb,
            calibration: Arc::new(calibration),
            as_shot_xy,
            baseline_exposure,
        }
    }

    /// Camera neutral of the as-shot white (green = 1).
    pub fn as_shot_neutral(&self) -> [f64; 3] {
        self.reference_wb.map(|v| 1.0 / v)
    }
}

/// A decoded, pyramided source. Shared by all views and jobs of a session.
pub struct SourceImage {
    pub info: Arc<ImageInfo>,
    /// A-tier key: source identity + decoder version + decode params + white levels, plus
    /// the A4 key if anything clipped (doc 02 §11).
    pub a_key: CacheKey,
    /// The decode part of `a_key`.
    decode_key: CacheKey,
    /// Decode params this source was produced with (normalised for non-raw sources).
    pub a_params: ATier,
    /// Pre-A4 store and the A4 key applied, if anything clipped.
    hl: Option<(Arc<HlBase>, HlKey)>,
    pub color: SourceColor,
    pub pyramid: Pyramid,
    /// Fraction of sensor pixels at the white level; `None` for non-raw sources.
    pub sensor_clipped: Option<f32>,
    /// Noise model from a DNG NoiseProfile (`crate::noise`).
    pub noise_model: Option<NoiseModel>,
    /// Per-level noise for the S tier, computed on first use.
    noise: Mutex<Vec<Option<NoiseLevel>>>,
    /// Measured level-0 estimate, computed on first use.
    measured: std::sync::OnceLock<NoiseEstimate>,
    _alloc: Allocation,
}

impl SourceImage {
    /// Oriented source size (before `orientation_delta`).
    pub fn size(&self) -> Size {
        self.info.source_size
    }

    /// Noise of level `level` in the S tier's stabilised luma: from the vendor model, else
    /// measured once from the level.
    pub fn noise_level(&self, level: usize) -> NoiseLevel {
        if let Some(m) = &self.noise_model {
            return m.level(level);
        }
        if let Some(Some(s)) = self.noise.lock().get(level) {
            return *s;
        }
        let s = noise::measure(&self.pyramid.levels[level]);
        let mut n = self.noise.lock();
        if n.len() <= level {
            n.resize(level + 1, None);
        }
        n[level] = Some(s);
        s
    }

    /// Level-0 noise at mid-grey (analysis, doc 06 §2) and where it comes from.
    pub fn noise_estimate(&self) -> (NoiseEstimate, NoiseSource) {
        match &self.noise_model {
            Some(m) => (m.estimate(), m.source),
            None => (
                self.measured
                    .get_or_init(|| noise::measure_estimate(&self.pyramid.levels[0]))
                    .clone(),
                NoiseSource::Measured,
            ),
        }
    }

    /// The A4 key `p` asks for, if this source has clipped highlights.
    pub fn hl_key(&self, p: &EditParams) -> Option<HlKey> {
        self.hl.as_ref().map(|_| hl_key_for(&self.color, p))
    }

    /// Sensor-clipped mask (dilated) of pyramid level `level`, if anything clipped.
    pub fn clip_mask(&self, level: usize) -> Option<Vec<bool>> {
        let l = &self.pyramid.levels[level];
        self.hl
            .as_ref()
            .map(|(b, _)| b.mask_at_level(level, l.width, l.height))
    }

    /// The A4 key applied to this source (`None` = nothing clipped).
    pub fn applied_hl(&self) -> Option<HlKey> {
        self.hl.as_ref().map(|h| h.1)
    }

    /// Whether this source has the decode slice of `p` (A4 may still differ).
    pub fn decode_matches(&self, p: &EditParams) -> bool {
        ATier::of(p).normalized_for(self.info.kind == SourceKind::Raw) == self.a_params
    }

    /// Whether this source already satisfies the A-tier slice of `p`, including A4.
    pub fn matches(&self, p: &EditParams) -> bool {
        self.decode_matches(p) && self.hl_key(p) == self.hl.as_ref().map(|h| h.1)
    }

    /// This source with A4 re-run for `p` (same decode): level 0 is copied, the clipped
    /// tiles are reconstructed from the pre-A4 store and only the pyramid tiles above them
    /// are rebuilt (doc 02 §7). Returns `self` if nothing changes.
    pub fn with_highlights(
        self: &Arc<Self>,
        p: &EditParams,
        ledger: &Arc<MemLedger>,
        cancel: Option<&CancelToken>,
    ) -> Result<Arc<SourceImage>, LoadError> {
        let Some((base, have)) = &self.hl else {
            return Ok(self.clone());
        };
        let key = hl_key_for(&self.color, p);
        if key == *have {
            return Ok(self.clone());
        }
        let mut pyramid = Pyramid {
            levels: self.pyramid.levels.clone(),
        };
        let rects = highlights::apply(&mut pyramid.levels[0], base, &key, cancel)?;
        let rects: Vec<_> = rects.iter().map(|r| (r.x0, r.y0, r.x1, r.y1)).collect();
        pyramid.update(&rects);
        let alloc = account_pyramid(&mut pyramid, 0, ledger);
        Ok(Arc::new(SourceImage {
            info: self.info.clone(),
            a_key: full_key(&self.decode_key, Some(&key)),
            decode_key: self.decode_key,
            a_params: self.a_params,
            hl: Some((base.clone(), key)),
            color: self.color.clone(),
            pyramid,
            sensor_clipped: self.sensor_clipped,
            noise_model: self.noise_model,
            noise: Mutex::new(Vec::new()),
            measured: Default::default(),
            _alloc: alloc,
        }))
    }
}

/// A4 key for `p`: its algorithm and its WB as camera multipliers (doc 02 §7).
fn hl_key_for(color: &SourceColor, p: &EditParams) -> HlKey {
    HlKey::new(
        p.raw.highlight_recon,
        crate::pipeline::color::user_multipliers(color, p),
    )
}

fn full_key(decode_key: &CacheKey, hl: Option<&HlKey>) -> CacheKey {
    match hl {
        None => *decode_key,
        Some(k) => KeyBuilder::new("AH").key(decode_key).json(k).finish(),
    }
}

pub fn is_raw_source(src: &SourceRef) -> bool {
    matches!(src, SourceRef::File(p) if is_raw(p))
}

/// Identity of the decode a source needs for `params` (file identity + A1–A3 params),
/// computable without decoding; A4 is keyed separately ([`SourceImage::matches`]).
pub fn decode_identity(src: &SourceRef, params: &EditParams) -> Result<CacheKey, LoadError> {
    let a = ATier::of(params).normalized_for(is_raw_source(src));
    Ok(a_key(src, &a)?.finish())
}

const MIB: u64 = 1 << 20;

/// A pyramid larger than this share of the effective budget is spilled (doc 03 §7.3).
pub const SPILL_SHARE: f64 = 0.5;

/// Spill `pyramid` if it is too large for the budget, then account its heap part plus
/// `extra` bytes. Spilled levels are paged by the OS and are not charged to the budget.
fn account_pyramid(pyramid: &mut Pyramid, extra: u64, ledger: &Arc<MemLedger>) -> Allocation {
    let bytes = pyramid.bytes();
    if bytes as f64 > SPILL_SHARE * ledger.effective() as f64 {
        match ledger.spill_dir() {
            Some(dir) => match pyramid.spill(dir) {
                Ok(()) => tracing::info!(
                    mib = bytes >> 20,
                    dir = %dir.display(),
                    "source pyramid spilled to memory-mapped files"
                ),
                Err(e) => tracing::warn!("spilling the source pyramid failed: {e}"),
            },
            None => tracing::warn!("no cache dir to spill the source pyramid"),
        }
    }
    ledger.alloc(MemClass::SourceCpu, pyramid.heap_bytes() + extra)
}

/// Declared peak of a raw decode (doc 03 §3, §6), from the image and raw sizes read before
/// unpacking. Measured peak RSS over the engine's baseline, A1–A5 inclusive: 24.8 B/px
/// (D3200 NEF), 25.4 (X-T20 RAF, X-Trans), 24.7 (6D CR2), 28 (K-5 DNG, small image, fixed
/// overheads dominate).
pub fn raw_peak(w: u32, h: u32, raw_w: u32, raw_h: u32) -> u64 {
    26 * w as u64 * h as u64 + 2 * raw_w as u64 * raw_h as u64 + 32 * MIB
}

/// Declared peak of a rendered-image decode: the float XYZ from libvips and the linear
/// Rec.2020 copy (12 B/px each), then the pyramid (8 B/px), plus libvips' strip buffers.
pub fn raster_peak(w: u32, h: u32) -> u64 {
    40 * w as u64 * h as u64 + 32 * MIB
}

/// Declared peak of a synthetic source: f32 RGB plus the pyramid.
pub fn synthetic_peak(w: u32, h: u32) -> u64 {
    20 * w as u64 * h as u64
}

/// A1–A3 output of any source, before A4 and the pyramid.
struct Decoded {
    kind: SourceKind,
    size: Size,
    exif: u16,
    /// Stored data in f32 (reference WB premultiplied).
    rgb: Vec<aigle_kernels::Rgb>,
    color: SourceColor,
    camera: Option<aigle_engine_api::CameraInfo>,
    sensor_clipped: Option<f32>,
    mask: Option<Vec<u8>>,
    noise_model: Option<NoiseModel>,
}

/// Decode `src` (A1–A3). Decoder outputs that shape later stages (white levels, noise
/// model) are added to `key`.
fn decode_rgb(
    src: &SourceRef,
    a: &ATier,
    key: &mut KeyBuilder,
    admit: &mut dyn FnMut(u64) -> Result<(), LoadError>,
    cancel: Option<&CancelToken>,
) -> Result<Decoded, LoadError> {
    let d = match src {
        SourceRef::Synthetic {
            pattern,
            width,
            height,
        } => {
            if *width == 0 || *height == 0 {
                return Err(LoadError::Failed(
                    "synthetic source must have a non-zero size".into(),
                ));
            }
            admit(synthetic_peak(*width, *height))?;
            Decoded {
                kind: SourceKind::Synthetic,
                size: Size::new(*width, *height),
                exif: 1,
                rgb: synthetic::render(*pattern, *width, *height),
                color: SourceColor::working_space(),
                camera: None,
                sensor_clipped: None,
                mask: None,
                noise_model: None,
            }
        }
        SourceRef::File(path) if is_raw_source(src) => {
            let _slot = crate::arbiter::decoders()
                .acquire(cancel)
                .ok_or(LoadError::Cancelled)?;
            let d = raw::decode(path, a, cancel, admit)?;
            tracing::debug!(
                path = %path.display(),
                demosaic = ?d.demosaic,
                white = ?d.white_levels,
                "raw decoded"
            );
            key.json(&d.white_levels);
            let noise_model = noise::raw_model(&d.noise_profile, d.color.reference_wb);
            // The model shapes S-tier output.
            key.json(&noise_model.map(|m| (m.luma, m.chroma)));
            Decoded {
                kind: SourceKind::Raw,
                size: d.size,
                exif: d.exif_orientation,
                rgb: d.rgb,
                color: d.color,
                camera: Some(d.camera),
                sensor_clipped: Some(d.sensor_clipped),
                mask: d.clip_mask,
                noise_model,
            }
        }
        SourceRef::File(path) => {
            let _slot = crate::arbiter::decoders()
                .acquire(cancel)
                .ok_or(LoadError::Cancelled)?;
            let d = raster::decode(path, &mut |w, h| admit(raster_peak(w, h)), cancel)?;
            Decoded {
                kind: SourceKind::Rendered,
                size: Size::new(d.width, d.height),
                exif: d.exif_orientation,
                rgb: d.rgb,
                color: SourceColor::working_space(),
                camera: d.camera,
                sensor_clipped: None,
                mask: None,
                noise_model: None,
            }
        }
    };
    Ok(d)
}

/// Full-precision level 0 of `base` for 16-bit exports (doc 03 §9): A1–A3 are re-run on
/// `src` without f16 rounding; pixels inside the highlight-reconstruction mask are taken
/// from `base`'s reconstructed f16 level 0 (they are A4 estimates, not measurements). The
/// allocation accounts the level until it is dropped.
pub fn full_precision(
    src: &SourceRef,
    base: &SourceImage,
    ledger: &Arc<MemLedger>,
    cancel: Option<&CancelToken>,
) -> Result<(Level32, Allocation), LoadError> {
    let mut key = KeyBuilder::new("full precision");
    let mut reservation = None;
    let mut admit = |peak: u64| -> Result<(), LoadError> {
        reservation = Some(ledger.admit(peak, cancel).ok_or(LoadError::Cancelled)?);
        Ok(())
    };
    let d = decode_rgb(src, &base.a_params, &mut key, &mut admit, cancel)?;
    let l0 = &base.pyramid.levels[0];
    if d.size != base.size() {
        return Err(LoadError::Failed(
            "the source changed since it was opened".into(),
        ));
    }
    let mut rgb = d.rgb;
    if let Some(mask) = base.clip_mask(0) {
        use rayon::prelude::*;
        rgb.par_iter_mut()
            .zip(&mask)
            .enumerate()
            .filter(|(_, (_, m))| **m)
            .for_each(|(i, (p, _))| *p = crate::backend::Px::H(&l0.data).get(i));
    }
    let level = Level32 {
        width: d.size.w,
        height: d.size.h,
        data: rgb,
    };
    let alloc = ledger.alloc(MemClass::SourceCpu, level.bytes());
    drop(reservation);
    Ok((level, alloc))
}

/// Decode a source for the A-tier slice of `params` (runs on an engine worker).
///
/// Admission (doc 03 §6): once the image size is known (raw/raster header), the decode's
/// declared peak is reserved in `ledger`, waiting for memory if needed; the reservation is
/// held until the pyramid is accounted.
pub fn load(
    src: &SourceRef,
    params: &EditParams,
    ledger: &Arc<MemLedger>,
    cancel: Option<&CancelToken>,
) -> Result<SourceImage, LoadError> {
    let raw_input = is_raw_source(src);
    let a = ATier::of(params).normalized_for(raw_input);
    let mut key = a_key(src, &a)?;
    let mut reservation = None;
    let mut admit = |peak: u64| -> Result<(), LoadError> {
        let t0 = std::time::Instant::now();
        reservation = Some(ledger.admit(peak, cancel).ok_or(LoadError::Cancelled)?);
        tracing::debug!(peak_mib = peak >> 20, waited = ?t0.elapsed(), "decode admitted");
        Ok(())
    };

    let Decoded {
        kind,
        size,
        exif,
        rgb,
        color,
        camera,
        sensor_clipped,
        mask,
        noise_model,
    } = decode_rgb(src, &a, &mut key, &mut admit, cancel)?;
    let decode_key = key.finish();
    if cancel.is_some_and(|c| c.is_cancelled()) {
        return Err(LoadError::Cancelled);
    }

    let (t, tint) = color::xy_to_cct_tint(color.as_shot_xy.0, color.as_shot_xy.1);
    let info = ImageInfo {
        kind,
        source_size: size,
        exif_orientation: exif,
        camera,
        as_shot: WbValuesDto {
            temperature: t.round() as f32,
            tint: tint.round() as f32,
        },
    };
    let mut level0 = Level::from_f32(size.w, size.h, &rgb);
    drop(rgb);
    // A4: sparse, from the f16 pre-A4 data so that a later re-run is bit-identical.
    let hl = match mask {
        Some(mut m) => {
            highlights::dilate(&mut m, size);
            HlBase::build(&level0, &m, color.reference_wb).map(Arc::new)
        }
        None => None,
    };
    let hl = match hl {
        Some(base) => {
            let k = hl_key_for(&color, params);
            highlights::apply(&mut level0, &base, &k, cancel)?;
            Some((base, k))
        }
        None => None,
    };
    let mut pyramid = Pyramid::build(level0);
    let alloc = account_pyramid(&mut pyramid, hl.as_ref().map_or(0, |h| h.0.bytes()), ledger);
    drop(reservation);
    Ok(SourceImage {
        info: Arc::new(info),
        a_key: full_key(&decode_key, hl.as_ref().map(|h| &h.1)),
        decode_key,
        a_params: a,
        hl,
        color,
        pyramid,
        sensor_clipped,
        noise_model,
        noise: Mutex::new(Vec::new()),
        measured: Default::default(),
        _alloc: alloc,
    })
}

/// An in-memory source for unit tests: `rgb` is the stored data (reference WB of `color`
/// premultiplied), nothing clipped.
#[cfg(test)]
pub(crate) fn test_source(
    w: u32,
    h: u32,
    rgb: &[aigle_kernels::Rgb],
    color: SourceColor,
) -> SourceImage {
    let (t, tint) = aigle_core::color::xy_to_cct_tint(color.as_shot_xy.0, color.as_shot_xy.1);
    SourceImage {
        info: Arc::new(ImageInfo {
            kind: SourceKind::Raw,
            source_size: Size::new(w, h),
            exif_orientation: 1,
            camera: None,
            as_shot: WbValuesDto {
                temperature: t as f32,
                tint: tint as f32,
            },
        }),
        a_key: KeyBuilder::new("test").finish(),
        decode_key: KeyBuilder::new("test").finish(),
        a_params: ATier::of(&EditParams::default()),
        hl: None,
        color,
        pyramid: Pyramid::build(Level::from_f32(w, h, rgb)),
        sensor_clipped: Some(0.0),
        noise_model: None,
        noise: Mutex::new(Vec::new()),
        measured: Default::default(),
        _alloc: MemLedger::new(1 << 30).alloc(MemClass::SourceCpu, 0),
    }
}

fn is_raw(path: &Path) -> bool {
    aigle_engine_api::is_raw_path(path)
}

/// File identity for caching: canonical path, size, mtime (doc 03 §7.2), plus the decode
/// params (the caller adds decoder outputs such as white levels).
fn a_key(src: &SourceRef, a: &ATier) -> Result<KeyBuilder, String> {
    let mut k = KeyBuilder::new("A");
    k.u32(DECODER_VERSION);
    match src {
        SourceRef::File(path) => {
            let canon = path
                .canonicalize()
                .map_err(|e| format!("{}: {e}", path.display()))?;
            let meta = std::fs::metadata(&canon).map_err(|e| format!("{}: {e}", path.display()))?;
            let mtime = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_nanos() as u64)
                .unwrap_or(0);
            k.str("file")
                .str(&canon.to_string_lossy())
                .u64(meta.len())
                .u64(mtime);
        }
        SourceRef::Synthetic {
            pattern,
            width,
            height,
        } => {
            k.str("synthetic")
                .str(&format!("{pattern:?}"))
                .u32(*width)
                .u32(*height);
        }
    }
    k.json(&a.demosaic)
        .u32(a.ca_correction as u32)
        .u32(a.hot_pixels as u32);
    // TODO(A3 alt): raw.ai_denoise.asset.
    Ok(k)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aigle_engine_api::SyntheticPattern;

    #[test]
    fn decode_waits_for_admission() {
        let src = SourceRef::Synthetic {
            pattern: SyntheticPattern::Gradient,
            width: 400,
            height: 300,
        };
        let peak = synthetic_peak(400, 300);
        let ledger = MemLedger::new(peak * 3 / 2);
        let other = ledger.try_reserve(peak).unwrap();
        let l = ledger.clone();
        let s2 = src.clone();
        let t = std::thread::spawn(move || {
            load(&s2, &EditParams::default(), &l, None).map(|s| s.pyramid.bytes())
        });
        std::thread::sleep(std::time::Duration::from_millis(100));
        assert!(!t.is_finished(), "the decode must wait for memory");
        drop(other);
        let bytes = t.join().unwrap().unwrap();
        let snap = ledger.snapshot();
        assert_eq!(snap.reserved, 0, "reservation released after load");
        assert_eq!(snap.used, 0, "the source was dropped");
        assert!(bytes < peak);
        // Cancelled while waiting.
        let _other = ledger.try_reserve(peak).unwrap();
        let c = CancelToken::new();
        c.cancel();
        assert!(matches!(
            load(&src, &EditParams::default(), &ledger, Some(&c)),
            Err(LoadError::Cancelled)
        ));
    }
}
