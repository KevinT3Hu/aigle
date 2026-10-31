//! Lens correction for B1: lensfun lookup → pinned profile → [`Optics`] for one image.

use std::sync::OnceLock;

use aigle_core::EditParams;
use aigle_core::geometry::{Optics, Size};
use aigle_core::lens::LensShot;
use aigle_core::params::LensProfile;
use aigle_engine_api::{CameraInfo, LensMatch};

use crate::data;

/// Focus distance assumed when unknown (lensfun convention).
const DEFAULT_DISTANCE: f64 = 1000.0;
/// Aperture assumed when unknown.
const DEFAULT_APERTURE: f64 = 8.0;

/// The lensfun database: `$AIGLE_LENSFUN_DIR`, else `lensfun/version_1` under the user data
/// dir or the bundled data (`crate::data::reference`), loaded on first use.
fn database() -> Result<&'static aigle_lensfun::Database, String> {
    static DB: OnceLock<Result<aigle_lensfun::Database, String>> = OnceLock::new();
    DB.get_or_init(|| {
        let dir = data::reference("AIGLE_LENSFUN_DIR", "lensfun/version_1")
            .ok_or("lensfun database not found (bundled data missing; set AIGLE_LENSFUN_DIR)")?;
        aigle_lensfun::Database::load(&[dir])
    })
    .as_ref()
    .map_err(String::clone)
}

/// Candidates for the photo's lens, best first, each pinned. `Err` if no database.
pub fn match_lens(cam: &CameraInfo) -> Result<Vec<LensMatch>, String> {
    let db = database()?;
    let model = cam.lens_model.clone().unwrap_or_default();
    if model.is_empty() {
        return Ok(Vec::new());
    }
    let camera = db.camera(&cam.make, &cam.model);
    // Crop factor: database camera, else the EXIF 35 mm-equivalent ratio.
    let crop = camera
        .map(|c| c.crop)
        .or_else(|| match (cam.focal_length, cam.focal_length_35mm) {
            (Some(f), Some(f35)) if f > 0.0 && f35 > 0.0 => Some((f35 / f) as f64),
            _ => None,
        });
    let range = cam
        .lens_focal_range
        .map_or((0.0, 0.0), |(a, b)| (a as f64, b as f64));
    let maker = cam.lens_make.clone().unwrap_or_default();
    let found = db.find_lenses(camera, &maker, &model, range);
    Ok(found
        .into_iter()
        .take(5)
        .map(|(l, score)| {
            let p = l.profile(crop.unwrap_or(l.crop));
            LensMatch {
                data_ref: data::pin_lens(&p),
                lens_maker: p.lens_maker.clone(),
                lens_model: p.lens_model.clone(),
                score,
                has_distortion: p.has_distortion(),
                has_tca: p.has_tca(),
                has_vignetting: p.has_vignetting(),
            }
        })
        .collect())
}

/// B1 optics for an edit: the evaluated lens model (if a pinned profile is set and found) and
/// the 35 mm-equivalent focal length for perspective. The bool is false when the edit
/// references a profile that cannot be found.
pub fn optics(cam: Option<&CameraInfo>, p: &EditParams, source: Size) -> (Optics, bool) {
    let focal = cam.and_then(|c| c.focal_length).map(|f| f as f64);
    let mut optics = Optics {
        lens: None,
        focal_35mm: cam.and_then(|c| c.focal_length_35mm).map(|f| f as f64),
    };
    let LensProfile::Resolved(r) = &p.lens.profile else {
        return (optics, true);
    };
    let Some(prof) = data::lens(r) else {
        tracing::warn!(data_ref = %r, "pinned lens profile not found; rendering without lens correction");
        return (optics, false);
    };
    let focal = focal.unwrap_or(prof.min_focal.max(1.0));
    if optics.focal_35mm.is_none() {
        optics.focal_35mm = Some(focal * prof.camera_crop);
    }
    let shot = LensShot {
        focal,
        aperture: cam
            .and_then(|c| c.f_number)
            .map_or(DEFAULT_APERTURE, |a| a as f64),
        distance: DEFAULT_DISTANCE,
    };
    let a = &p.lens.amounts;
    let m = prof.model(
        &shot,
        source,
        [
            a.distortion as f64 / 100.0,
            a.tca as f64 / 100.0,
            a.vignetting as f64 / 100.0,
        ],
    );
    optics.lens = Some(m);
    (optics, true)
}
