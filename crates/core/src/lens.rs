//! Lens correction math (doc 02 §3 B1): lensfun-compatible distortion, TCA and vignetting
//! models, their interpolation across focal length / aperture / distance, and the
//! normalisation lensfun uses (ported from lensfun `modifier.cpp`, `mod-coord.cpp`,
//! `mod-subpix.cpp`, `mod-color.cpp`, `lens.cpp`; LGPL-3, re-implemented here).
//!
//! [`LensProfileData`] is the pinned, content-addressed profile (doc 02 §13.2): one lens's
//! calibration plus the camera crop factor. [`LensProfileData::model`] evaluates it for one
//! image (focal length, aperture, distance, size, user amounts) into a [`LensModel`] that maps
//! corrected Source coordinates to distorted ones, per channel, with a vignetting gain.
//!
//! Coordinates: continuous Source px (pixel `i` covers `[i, i+1)`), lens centre = image
//! centre (lensfun's deprecated centre offsets are not used).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::geometry::{Point, Size};

/// Diagonal of the 36×24 mm format.
const FF_DIAG: f64 = 43.266_615_305_567_87;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", tag = "model")]
pub enum DistModel {
    Poly3 { k1: f64 },
    Poly5 { k1: f64, k2: f64 },
    Ptlens { a: f64, b: f64, c: f64 },
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", tag = "model")]
pub enum TcaModel {
    Linear {
        kr: f64,
        kb: f64,
    },
    Poly3 {
        vr: f64,
        vb: f64,
        cr: f64,
        cb: f64,
        br: f64,
        bb: f64,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DistEntry {
    pub focal: f64,
    /// Real focal length if known (else = focal).
    pub real_focal: Option<f64>,
    #[serde(flatten)]
    pub model: DistModel,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TcaEntry {
    pub focal: f64,
    #[serde(flatten)]
    pub model: TcaModel,
}

/// `pa` vignetting model: gain = 1 + k1 r² + k2 r⁴ + k3 r⁶ (r = 1 at the corner).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct VigEntry {
    pub focal: f64,
    pub aperture: f64,
    pub distance: f64,
    pub k: [f64; 3],
}

/// A resolved, pinned lens profile (the content behind `LensProfile::Resolved(DataRef)`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct LensProfileData {
    pub lens_maker: String,
    pub lens_model: String,
    /// Crop factor and aspect ratio of the calibration sensor.
    pub calib_crop: f64,
    pub calib_aspect: f64,
    /// Crop factor of the camera the profile was resolved for.
    pub camera_crop: f64,
    pub min_focal: f64,
    pub max_focal: f64,
    pub distortion: Vec<DistEntry>,
    pub tca: Vec<TcaEntry>,
    pub vignetting: Vec<VigEntry>,
}

/// Image facts needed to evaluate a profile.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LensShot {
    pub focal: f64,
    pub aperture: f64,
    /// Focus distance in metres; lensfun applications use 1000 when unknown.
    pub distance: f64,
}

/// Cubic Hermite through y2..y3 with neighbour tangents (lensfun `_lf_interpolate`).
fn hermite(y1: Option<f64>, y2: f64, y3: f64, y4: Option<f64>, t: f64) -> f64 {
    let tg2 = y1.map_or(y3 - y2, |y1| (y3 - y1) * 0.5);
    let tg3 = y4.map_or(y3 - y2, |y4| (y4 - y2) * 0.5);
    let (t2, t3) = (t * t, t * t * t);
    (2.0 * t3 - 3.0 * t2 + 1.0) * y2
        + (t3 - 2.0 * t2 + t) * tg2
        + (-2.0 * t3 + 3.0 * t2) * y3
        + (t3 - t2) * tg3
}

/// The (up to) two calibration points below and two above `focal`, nearest first
/// (lensfun `__insert_spline`). Exact match returns `Err(index)`.
fn spline4(focals: &[f64], focal: f64) -> Result<[Option<usize>; 4], usize> {
    let mut s: [Option<usize>; 4] = [None; 4];
    let mut d = [f64::MIN, f64::MIN, f64::MAX, f64::MAX];
    for (i, &f) in focals.iter().enumerate() {
        let df = focal - f;
        if df == 0.0 {
            return Err(i);
        }
        if df < 0.0 {
            if df > d[1] {
                (d[0], s[0]) = (d[1], s[1]);
                (d[1], s[1]) = (df, Some(i));
            } else if df > d[0] {
                (d[0], s[0]) = (df, Some(i));
            }
        } else if df < d[2] {
            (d[3], s[3]) = (d[2], s[2]);
            (d[2], s[2]) = (df, Some(i));
        } else if df < d[3] {
            (d[3], s[3]) = (df, Some(i));
        }
    }
    // s[1], s[0]: nearest and next calibration focal above the image focal; s[2], s[3]:
    // nearest and next below (lensfun naming).
    Ok(s)
}

/// Interpolate per-entry coefficient vectors at `focal`; `scale[k]` is true for terms that
/// follow 1/f (multiplied by the entry focal before and divided after, lensfun
/// `__parameter_scales`).
fn interp_focal(
    focals: &[f64],
    terms: &[Vec<f64>],
    focal: f64,
    scale: &[bool],
) -> Option<Vec<f64>> {
    if focals.is_empty() {
        return None;
    }
    let s = match spline4(focals, focal) {
        Err(i) => return Some(terms[i].clone()),
        Ok(s) => s,
    };
    // lensfun: if one side is missing, use the nearest existing entry unchanged.
    let (lo, hi) = (s[2], s[1]); // lo: focal below image focal; hi: above
    match (lo, hi) {
        (None, None) => None,
        (Some(i), None) | (None, Some(i)) => Some(terms[i].clone()),
        (Some(a), Some(b)) => {
            // Interpolate from entry `a` (lower focal) to `b` (higher focal); outer neighbours
            // are s[3] (further below) and s[0] (further above).
            let t = (focal - focals[a]) / (focals[b] - focals[a]);
            let n = terms[a].len();
            let mut out = vec![0.0; n];
            for k in 0..n {
                let v = |i: usize| terms[i][k] * if scale[k] { focals[i] } else { 1.0 };
                let r = hermite(s[3].map(v), v(a), v(b), s[0].map(v), t);
                out[k] = if scale[k] { r / focal } else { r };
            }
            Some(out)
        }
    }
}

impl LensProfileData {
    pub fn has_distortion(&self) -> bool {
        !self.distortion.is_empty()
    }
    pub fn has_tca(&self) -> bool {
        !self.tca.is_empty()
    }
    pub fn has_vignetting(&self) -> bool {
        !self.vignetting.is_empty()
    }

    fn dist_at(&self, focal: f64) -> Option<(DistModel, f64)> {
        let first = self.distortion.first()?.model;
        let same = |m: &DistModel| std::mem::discriminant(m) == std::mem::discriminant(&first);
        let es: Vec<&DistEntry> = self.distortion.iter().filter(|e| same(&e.model)).collect();
        let focals: Vec<f64> = es.iter().map(|e| e.focal).collect();
        let terms: Vec<Vec<f64>> = es
            .iter()
            .map(|e| {
                let mut v = match e.model {
                    DistModel::Poly3 { k1 } => vec![k1, 0.0, 0.0],
                    DistModel::Poly5 { k1, k2 } => vec![k1, k2, 0.0],
                    DistModel::Ptlens { a, b, c } => vec![a, b, c],
                };
                v.push(e.real_focal.unwrap_or(e.focal));
                v
            })
            .collect();
        let v = interp_focal(&focals, &terms, focal, &[false, false, false, false])?;
        let m = match first {
            DistModel::Poly3 { .. } => DistModel::Poly3 { k1: v[0] },
            DistModel::Poly5 { .. } => DistModel::Poly5 { k1: v[0], k2: v[1] },
            DistModel::Ptlens { .. } => DistModel::Ptlens {
                a: v[0],
                b: v[1],
                c: v[2],
            },
        };
        // An exact/one-sided match keeps the entry's real focal; for an interpolated match the
        // real focal is interpolated too (lensfun); if the data had no real focal, it equals
        // the nominal focal of the image.
        let real = if es.iter().all(|e| e.real_focal.is_none()) {
            focal
        } else {
            v[3]
        };
        Some((m, real))
    }

    fn tca_at(&self, focal: f64) -> Option<TcaModel> {
        let first = self.tca.first()?.model;
        let same = |m: &TcaModel| std::mem::discriminant(m) == std::mem::discriminant(&first);
        let es: Vec<&TcaEntry> = self.tca.iter().filter(|e| same(&e.model)).collect();
        let focals: Vec<f64> = es.iter().map(|e| e.focal).collect();
        let terms: Vec<Vec<f64>> = es
            .iter()
            .map(|e| match e.model {
                TcaModel::Linear { kr, kb } => vec![kr, kb],
                TcaModel::Poly3 {
                    vr,
                    vb,
                    cr,
                    cb,
                    br,
                    bb,
                } => vec![vr, vb, cr, cb, br, bb],
            })
            .collect();
        let scale: Vec<bool> = (0..terms[0].len()).map(|k| k >= 2).collect();
        let v = interp_focal(&focals, &terms, focal, &scale)?;
        Some(match first {
            TcaModel::Linear { .. } => TcaModel::Linear { kr: v[0], kb: v[1] },
            TcaModel::Poly3 { .. } => TcaModel::Poly3 {
                vr: v[0],
                vb: v[1],
                cr: v[2],
                cb: v[3],
                br: v[4],
                bb: v[5],
            },
        })
    }

    /// Inverse-distance-weighted vignetting (lensfun `InterpolateVignetting`, p = 3.5).
    fn vig_at(&self, s: &LensShot) -> Option<[f64; 3]> {
        if self.vignetting.is_empty() {
            return None;
        }
        let df = self.max_focal - self.min_focal;
        let nf = |f: f64| {
            if df != 0.0 {
                (f - self.min_focal) / df
            } else {
                f - self.min_focal
            }
        };
        let dist = |e: &VigEntry| {
            ((nf(e.focal) - nf(s.focal)).powi(2)
                + (4.0 / e.aperture - 4.0 / s.aperture).powi(2)
                + (0.1 / e.distance - 0.1 / s.distance).powi(2))
            .sqrt()
        };
        let mut acc = [0.0; 3];
        let mut total = 0.0;
        let mut nearest = f64::MAX;
        for e in &self.vignetting {
            let d = dist(e);
            if d < 1e-4 {
                return Some(e.k);
            }
            nearest = nearest.min(d);
            let w = 1.0 / d.powf(3.5);
            for (a, k) in acc.iter_mut().zip(e.k) {
                *a += w * k;
            }
            total += w;
        }
        (nearest <= 1.0 && total > 0.0).then(|| acc.map(|v| v / total))
    }

    /// Evaluate for one image of `size` (oriented Source px) with user amounts (1 = 100 %).
    pub fn model(&self, shot: &LensShot, size: Size, amounts: [f64; 3]) -> LensModel {
        let crop = if self.camera_crop > 0.0 {
            self.camera_crop
        } else {
            self.calib_crop
        };
        let dist = self.dist_at(shot.focal);
        let real_focal = dist.map_or(shot.focal, |d| d.1);
        let (w, h) = (size.w.max(2) as f64, size.h.max(2) as f64);
        // lensfun: NormScale = hypot(36, 24) / crop / hypot(W, H) / RealFocal.
        let norm = FF_DIAG / crop / w.hypot(h) / real_focal;
        let hugin_mm = FF_DIAG / self.calib_crop / self.calib_aspect.hypot(1.0) / 2.0;
        let hs = real_focal / hugin_mm;
        let dist = dist.map(|(m, _)| match m {
            DistModel::Poly3 { k1 } => {
                let d = 1.0 - k1;
                RDist::Poly {
                    k1: k1 * hs.powi(2) / d.powi(3),
                    k2: 0.0,
                }
            }
            DistModel::Poly5 { k1, k2 } => RDist::Poly {
                k1: k1 * hs.powi(2),
                k2: k2 * hs.powi(4),
            },
            DistModel::Ptlens { a, b, c } => {
                let d = 1.0 - a - b - c;
                RDist::Ptlens {
                    a: a * hs.powi(3) / d.powi(4),
                    b: b * hs.powi(2) / d.powi(3),
                    c: c * hs / d.powi(2),
                }
            }
        });
        let tca = self.tca_at(shot.focal).map(|m| match m {
            TcaModel::Linear { kr, kb } => [kr, 0.0, 0.0, kb, 0.0, 0.0],
            TcaModel::Poly3 {
                vr,
                vb,
                cr,
                cb,
                br,
                bb,
            } => [vr, cr * hs, br * hs * hs, vb, cb * hs, bb * hs * hs],
        });
        let vhs = real_focal / (FF_DIAG / self.calib_crop / 2.0);
        let vig = self
            .vig_at(shot)
            .map(|k| [k[0] * vhs.powi(2), k[1] * vhs.powi(4), k[2] * vhs.powi(6)]);
        LensModel {
            center: Point::new(w / 2.0, h / 2.0),
            norm,
            dist: dist.filter(|_| amounts[0] != 0.0),
            tca: tca.filter(|_| amounts[1] != 0.0),
            vig: vig.filter(|_| amounts[2] != 0.0),
            amounts,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum RDist {
    /// Rd = Ru (1 + k1 Ru² + k2 Ru⁴)
    Poly { k1: f64, k2: f64 },
    /// Rd = Ru (a Ru³ + b Ru² + c Ru + 1)
    Ptlens { a: f64, b: f64, c: f64 },
}

/// A profile evaluated for one image, in lensfun's normalised coordinates (units of the real
/// focal length).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LensModel {
    center: Point,
    /// Source px → normalised.
    norm: f64,
    dist: Option<RDist>,
    /// `[vr, cr, br, vb, cb, bb]`: Rd = Ru (b Ru² + c Ru + v).
    tca: Option<[f64; 6]>,
    vig: Option<[f64; 3]>,
    amounts: [f64; 3],
}

impl LensModel {
    pub fn is_identity(&self) -> bool {
        self.dist.is_none() && self.tca.is_none() && self.vig.is_none()
    }

    pub fn has_tca(&self) -> bool {
        self.tca.is_some()
    }

    fn radial(&self, x: f64, y: f64) -> f64 {
        let r2 = x * x + y * y;
        match self.dist {
            None => 1.0,
            Some(RDist::Poly { k1, k2 }) => 1.0 + k1 * r2 + k2 * r2 * r2,
            Some(RDist::Ptlens { a, b, c }) => {
                let r = r2.sqrt();
                a * r2 * r + b * r2 + c * r + 1.0
            }
        }
    }

    /// Corrected Source point → distorted Source points for (R, G, B) and the vignetting
    /// gain to apply to the sample.
    pub fn to_source(&self, p: Point) -> ([Point; 3], f64) {
        let (x, y) = (
            (p.x - self.center.x) * self.norm,
            (p.y - self.center.y) * self.norm,
        );
        let k = 1.0 + self.amounts[0] * (self.radial(x, y) - 1.0);
        let (xd, yd) = (x * k, y * k);
        let back = |x: f64, y: f64| {
            Point::new(x / self.norm + self.center.x, y / self.norm + self.center.y)
        };
        let g = back(xd, yd);
        let (r, b) = match self.tca {
            None => (g, g),
            Some(t) => {
                let ru = (xd * xd + yd * yd).sqrt();
                let f = |v: f64, c: f64, bb: f64| {
                    1.0 + self.amounts[1] * (bb * ru * ru + c * ru + v - 1.0)
                };
                let (kr, kb) = (f(t[0], t[1], t[2]), f(t[3], t[4], t[5]));
                (back(xd * kr, yd * kr), back(xd * kb, yd * kb))
            }
        };
        let gain = match self.vig {
            None => 1.0,
            Some(k) => {
                let r2 = xd * xd + yd * yd;
                let c = 1.0 + k[0] * r2 + k[1] * r2 * r2 + k[2] * r2 * r2 * r2;
                (1.0 / c.max(0.05)).powf(self.amounts[2])
            }
        };
        ([r, g, b], gain)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile() -> LensProfileData {
        LensProfileData {
            lens_maker: "Test".into(),
            lens_model: "Zoom".into(),
            calib_crop: 1.5,
            calib_aspect: 1.5,
            camera_crop: 1.5,
            min_focal: 18.0,
            max_focal: 55.0,
            distortion: vec![
                DistEntry {
                    focal: 18.0,
                    real_focal: None,
                    model: DistModel::Ptlens {
                        a: 0.02,
                        b: -0.1,
                        c: 0.07,
                    },
                },
                DistEntry {
                    focal: 35.0,
                    real_focal: None,
                    model: DistModel::Ptlens {
                        a: 0.03,
                        b: -0.09,
                        c: 0.1,
                    },
                },
                DistEntry {
                    focal: 55.0,
                    real_focal: None,
                    model: DistModel::Ptlens {
                        a: 0.005,
                        b: -0.001,
                        c: 0.014,
                    },
                },
            ],
            tca: vec![TcaEntry {
                focal: 18.0,
                model: TcaModel::Linear {
                    kr: 1.0005,
                    kb: 0.9995,
                },
            }],
            vignetting: vec![
                VigEntry {
                    focal: 18.0,
                    aperture: 2.8,
                    distance: 1000.0,
                    k: [-0.9, 0.9, -0.6],
                },
                VigEntry {
                    focal: 18.0,
                    aperture: 8.0,
                    distance: 1000.0,
                    k: [-0.3, 0.0, 0.0],
                },
            ],
        }
    }

    #[test]
    fn interpolation_hits_entries_and_is_between() {
        let p = profile();
        let at = |f| match p.dist_at(f).unwrap().0 {
            DistModel::Ptlens { a, .. } => a,
            _ => unreachable!(),
        };
        assert_eq!(at(35.0), 0.03);
        let mid = at(45.0);
        assert!(mid < 0.03 + 0.01 && mid > 0.0, "{mid}");
        // Outside the range: nearest entry.
        assert_eq!(at(10.0), 0.02);
        assert_eq!(
            p.vig_at(&LensShot {
                focal: 18.0,
                aperture: 8.0,
                distance: 1000.0
            })
            .unwrap(),
            [-0.3, 0.0, 0.0]
        );
    }

    #[test]
    fn model_maps_centre_to_centre_and_corrects_vignetting() {
        let p = profile();
        let shot = LensShot {
            focal: 18.0,
            aperture: 2.8,
            distance: 1000.0,
        };
        let m = p.model(&shot, Size::new(6000, 4000), [1.0, 1.0, 1.0]);
        let (pts, g) = m.to_source(Point::new(3000.0, 2000.0));
        assert_eq!(pts[1], Point::new(3000.0, 2000.0));
        assert_eq!(g, 1.0);
        let (pts, g) = m.to_source(Point::new(10.0, 10.0));
        assert!(g > 1.5, "corner brightened: {g}");
        assert!(pts[0] != pts[2], "TCA separates channels");
        let off = p.model(&shot, Size::new(6000, 4000), [0.0, 0.0, 0.0]);
        assert!(off.is_identity());
    }
}
