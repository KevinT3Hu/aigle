//! The lensfun lens database (XML format version 1 or 2), loaded from disk at runtime, with camera
//! and lens lookup ported from lensfun's `database.cpp` / `auxfun.cpp` (fuzzy word matching,
//! crop-factor and mount scoring).
//!
//! The database is not compiled into aigle. [`Database::load`] reads every `*.xml` file of
//! the given directories; the engine passes the database that ships with aigle
//! (`data/lensfun/version_1`, CC-BY-SA-3.0, revision in `data/manifest.toml`) or a
//! user-provided one (`aigle_engine::data::reference`).
//!
//! Both formats carry the same data for aigle: the update server derives version 1 from 2 by
//! dropping `acm` models (unsupported here) and moving `real-focal` into
//! `<real-focal-length>` elements, which [`Database::parse`] reads back.
//!
//! Lookup results are [`aigle_core::lens::LensProfileData`]: the engine pins the resolved
//! profile as a content-addressed `DataRef` (doc 02 §13.2), so an edit keeps rendering the
//! same even if the database on disk is updated or removed.

use std::path::PathBuf;

use aigle_core::lens::{DistEntry, DistModel, LensProfileData, TcaEntry, TcaModel, VigEntry};

#[derive(Clone, Debug, PartialEq)]
pub struct Camera {
    pub makers: Vec<String>,
    pub models: Vec<String>,
    pub mount: String,
    pub crop: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Lens {
    pub makers: Vec<String>,
    pub models: Vec<String>,
    pub mounts: Vec<String>,
    pub crop: f64,
    pub aspect: f64,
    pub focal: (f64, f64),
    pub distortion: Vec<DistEntry>,
    pub tca: Vec<TcaEntry>,
    pub vignetting: Vec<VigEntry>,
}

impl Lens {
    pub fn maker(&self) -> &str {
        self.makers.first().map_or("", String::as_str)
    }
    pub fn model(&self) -> &str {
        self.models.first().map_or("", String::as_str)
    }

    /// Pinnable profile of this lens for a camera with `camera_crop`.
    pub fn profile(&self, camera_crop: f64) -> LensProfileData {
        LensProfileData {
            lens_maker: self.maker().to_string(),
            lens_model: self.model().to_string(),
            calib_crop: self.crop,
            calib_aspect: self.aspect,
            camera_crop,
            min_focal: self.focal.0,
            max_focal: self.focal.1,
            distortion: self.distortion.clone(),
            tca: self.tca.clone(),
            vignetting: self.vignetting.clone(),
        }
    }
}

#[derive(Default, Debug)]
pub struct Database {
    pub cameras: Vec<Camera>,
    pub lenses: Vec<Lens>,
    /// Mount → compatible mounts.
    pub mounts: Vec<(String, Vec<String>)>,
}

fn names(n: roxmltree::Node<'_, '_>, tag: &str) -> Vec<String> {
    // Untranslated name first, then translations.
    let mut v: Vec<(bool, String)> = n
        .children()
        .filter(|c| c.has_tag_name(tag))
        .filter_map(|c| {
            c.text()
                .map(|t| (c.attribute("lang").is_some(), t.trim().to_string()))
        })
        .collect();
    v.sort_by_key(|(lang, _)| *lang);
    v.into_iter().map(|(_, s)| s).collect()
}

fn text<'a>(n: roxmltree::Node<'a, '_>, tag: &str) -> Option<&'a str> {
    n.children()
        .find(|c| c.has_tag_name(tag))
        .and_then(|c| c.text())
        .map(str::trim)
}

fn attr(n: roxmltree::Node<'_, '_>, a: &str) -> Option<f64> {
    n.attribute(a).and_then(|v| v.trim().parse().ok())
}

fn parse_aspect(s: &str) -> Option<f64> {
    match s.split_once(':') {
        Some((a, b)) => Some(a.trim().parse::<f64>().ok()? / b.trim().parse::<f64>().ok()?),
        None => s.trim().parse().ok(),
    }
}

fn parse_lens(n: roxmltree::Node<'_, '_>) -> Option<Lens> {
    let mut l = Lens {
        makers: names(n, "maker"),
        models: names(n, "model"),
        mounts: n
            .children()
            .filter(|c| c.has_tag_name("mount"))
            .filter_map(|c| c.text())
            .map(|s| s.trim().to_string())
            .collect(),
        crop: text(n, "cropfactor")
            .and_then(|v| v.parse().ok())
            .unwrap_or(1.0),
        aspect: text(n, "aspect-ratio")
            .and_then(parse_aspect)
            .unwrap_or(1.5),
        focal: (0.0, 0.0),
        distortion: Vec::new(),
        tca: Vec::new(),
        vignetting: Vec::new(),
    };
    if l.models.is_empty() {
        return None;
    }
    if let Some(f) = n.children().find(|c| c.has_tag_name("focal")) {
        let v = attr(f, "value");
        l.focal = (
            attr(f, "min").or(v).unwrap_or(0.0),
            attr(f, "max").or(v).unwrap_or(0.0),
        );
    }
    for cal in n.children().filter(|c| c.has_tag_name("calibration")) {
        // Format 1 stores the real focal length in separate elements (format 2: an attribute
        // of <distortion>, see lensfun tools/update_database/xml_converter.py).
        let real_focals: Vec<(f64, f64)> = cal
            .children()
            .filter(|c| c.has_tag_name("real-focal-length"))
            .filter_map(|c| Some((attr(c, "focal")?, attr(c, "real-focal")?)))
            .collect();
        for e in cal.children().filter(|c| c.is_element()) {
            let Some(focal) = attr(e, "focal") else {
                continue;
            };
            let z = |a| attr(e, a).unwrap_or(0.0);
            match (e.tag_name().name(), e.attribute("model")) {
                ("distortion", Some(m)) => {
                    let model = match m {
                        "poly3" => DistModel::Poly3 { k1: z("k1") },
                        "poly5" => DistModel::Poly5 {
                            k1: z("k1"),
                            k2: z("k2"),
                        },
                        "ptlens" => DistModel::Ptlens {
                            a: z("a"),
                            b: z("b"),
                            c: z("c"),
                        },
                        _ => continue,
                    };
                    let real_focal = attr(e, "real-focal").or_else(|| {
                        real_focals
                            .iter()
                            .find(|(f, _)| *f == focal)
                            .map(|(_, r)| *r)
                    });
                    l.distortion.push(DistEntry {
                        focal,
                        real_focal,
                        model,
                    });
                }
                ("tca", Some(m)) => {
                    let model = match m {
                        "linear" => TcaModel::Linear {
                            kr: attr(e, "kr").unwrap_or(1.0),
                            kb: attr(e, "kb").unwrap_or(1.0),
                        },
                        "poly3" => TcaModel::Poly3 {
                            vr: attr(e, "vr").unwrap_or(1.0),
                            vb: attr(e, "vb").unwrap_or(1.0),
                            cr: z("cr"),
                            cb: z("cb"),
                            br: z("br"),
                            bb: z("bb"),
                        },
                        _ => continue,
                    };
                    l.tca.push(TcaEntry { focal, model });
                }
                ("vignetting", Some("pa")) => {
                    l.vignetting.push(VigEntry {
                        focal,
                        aperture: z("aperture"),
                        distance: attr(e, "distance").unwrap_or(1000.0),
                        k: [z("k1"), z("k2"), z("k3")],
                    });
                }
                _ => {}
            }
        }
    }
    if l.focal == (0.0, 0.0) {
        let fs = l
            .distortion
            .iter()
            .map(|e| e.focal)
            .chain(l.tca.iter().map(|e| e.focal))
            .chain(l.vignetting.iter().map(|e| e.focal));
        let (lo, hi) = fs.fold((f64::MAX, 0.0f64), |(a, b), f| (a.min(f), b.max(f)));
        if hi > 0.0 {
            l.focal = (lo, hi);
        }
    }
    Some(l)
}

impl Database {
    pub fn parse(files: &[(&str, &str)]) -> Result<Database, String> {
        let mut db = Database::default();
        for (name, xml) in files {
            let opts = roxmltree::ParsingOptions {
                allow_dtd: true,
                ..Default::default()
            };
            let doc = roxmltree::Document::parse_with_options(xml, opts)
                .map_err(|e| format!("{name}: {e}"))?;
            match doc.root_element().attribute("version") {
                Some("1") | Some("2") => {}
                v => {
                    return Err(format!(
                        "{name}: unsupported lensfun database version {v:?} (need 1 or 2)"
                    ));
                }
            }
            for n in doc.root_element().children().filter(|c| c.is_element()) {
                match n.tag_name().name() {
                    "camera" => {
                        if let (Some(mount), Some(crop)) = (
                            text(n, "mount"),
                            text(n, "cropfactor").and_then(|v| v.parse().ok()),
                        ) {
                            db.cameras.push(Camera {
                                makers: names(n, "maker"),
                                models: names(n, "model"),
                                mount: mount.to_string(),
                                crop,
                            });
                        }
                    }
                    "lens" => db.lenses.extend(parse_lens(n)),
                    "mount" => {
                        if let Some(name) = text(n, "name") {
                            let compat = n
                                .children()
                                .filter(|c| c.has_tag_name("compat"))
                                .filter_map(|c| c.text())
                                .map(|s| s.trim().to_string())
                                .collect();
                            db.mounts.push((name.to_string(), compat));
                        }
                    }
                    _ => {}
                }
            }
        }
        Ok(db)
    }

    /// Load all `*.xml` files of `dirs` (later directories add to earlier ones).
    pub fn load(dirs: &[PathBuf]) -> Result<Database, String> {
        let mut texts = Vec::new();
        for d in dirs {
            let mut files: Vec<PathBuf> = std::fs::read_dir(d)
                .map_err(|e| format!("{}: {e}", d.display()))?
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.extension().is_some_and(|e| e == "xml"))
                .collect();
            files.sort();
            for f in files {
                let t = std::fs::read_to_string(&f).map_err(|e| format!("{}: {e}", f.display()))?;
                texts.push((f.display().to_string(), t));
            }
        }
        if texts.is_empty() {
            return Err("no lensfun database files found".into());
        }
        let refs: Vec<(&str, &str)> = texts
            .iter()
            .map(|(n, t)| (n.as_str(), t.as_str()))
            .collect();
        Database::parse(&refs)
    }

    /// Camera by maker and model (case-insensitive, any translation; the model may repeat the
    /// maker, as in "Canon EOS 6D").
    pub fn camera(&self, maker: &str, model: &str) -> Option<&Camera> {
        let norm = |s: &str| {
            s.to_lowercase()
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
        };
        let (mk, md) = (norm(maker), norm(model));
        self.cameras.iter().find(|c| {
            let maker_ok = c.makers.iter().any(|m| {
                let m = norm(m);
                mk.starts_with(&m) || m.starts_with(&mk)
            });
            maker_ok
                && c.models.iter().any(|m| {
                    let m = norm(m);
                    m == md
                        || m.strip_prefix(&mk).map(str::trim) == Some(md.as_str())
                        || md.strip_prefix(&mk).map(str::trim) == Some(m.as_str())
                })
        })
    }

    fn compat_mounts(&self, mount: &str) -> Vec<&str> {
        self.mounts
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(mount))
            .map(|(_, c)| c.iter().map(String::as_str).collect())
            .unwrap_or_default()
    }

    /// Lenses matching an EXIF lens description for `camera`, best first (lensfun
    /// `FindLenses`: strict all-words matching, loose fallback).
    pub fn find_lenses(
        &self,
        camera: Option<&Camera>,
        maker: &str,
        model: &str,
        focal_range: (f64, f64),
    ) -> Vec<(&Lens, i32)> {
        if model.trim().is_empty() {
            return Vec::new();
        }
        let pattern = words(model);
        let (gmin, gmax) = guess_focal(model).unwrap_or(focal_range);
        let compat = camera
            .map(|c| self.compat_mounts(&c.mount))
            .unwrap_or_default();
        for strict in [true, false] {
            let mut out: Vec<(&Lens, i32)> = self
                .lenses
                .iter()
                .filter_map(|l| {
                    let mut score = 0;
                    if let Some(c) = camera {
                        let cs = crop_score(c.crop, l.crop);
                        if cs == 0 {
                            return None;
                        }
                        score += cs;
                        if c.mount.is_empty() || l.mounts.is_empty() {
                        } else if l.mounts.iter().any(|m| m.eq_ignore_ascii_case(&c.mount)) {
                            score += 10;
                        } else if l
                            .mounts
                            .iter()
                            .any(|m| compat.iter().any(|c| c.eq_ignore_ascii_case(m)))
                        {
                            score += 9;
                        } else {
                            return None;
                        }
                    }
                    for (p, m) in [(gmin, l.focal.0), (gmax, l.focal.1)] {
                        match cmp_num(p, m) {
                            -1 => return None,
                            1 => score += 10,
                            _ => {}
                        }
                    }
                    if !maker.trim().is_empty() && !l.makers.is_empty() {
                        if !l
                            .makers
                            .iter()
                            .any(|m| m.trim().eq_ignore_ascii_case(maker.trim()))
                        {
                            return None;
                        }
                        score += 10;
                    }
                    let ms = l
                        .models
                        .iter()
                        .map(|m| fuzzy(&pattern, &words(m), strict))
                        .max()
                        .unwrap_or(0);
                    if ms == 0 {
                        return None;
                    }
                    Some((l, score + ((ms * 4) / 10).max(1)))
                })
                .collect();
            if !out.is_empty() {
                out.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.model().cmp(b.0.model())));
                return out;
            }
        }
        Vec::new()
    }
}

/// lensfun `MatchScore` crop-factor score of a lens calibrated at `lens` on a camera `cam`.
fn crop_score(cam: f64, lens: f64) -> i32 {
    if cam > 0.01 && cam < lens * 0.96 {
        return 0;
    }
    match cam / lens {
        r if r >= 1.41 => 2,
        r if r >= 1.31 => 4,
        r if r >= 1.21 => 6,
        r if r >= 1.11 => 8,
        r if r >= 1.01 => 10,
        r if r >= 1.0 => 5,
        _ => 3,
    }
}

/// lensfun `_lf_compare_num`: 0 = unknown, 1 = equal (within 1 %), −1 = different.
fn cmp_num(a: f64, b: f64) -> i32 {
    if a <= 0.0 || b <= 0.0 {
        return 0;
    }
    let r = a / b;
    if (0.99..=1.01).contains(&r) { 1 } else { -1 }
}

/// Focal range from a lens name ("18-55mm", "EF85mm", "XF18-55mmF2.8").
fn guess_focal(model: &str) -> Option<(f64, f64)> {
    let lower = model.to_lowercase();
    let i = lower.find("mm")?;
    let head = &lower[..i];
    let start = head
        .rfind(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-'))
        .map_or(0, |p| p + 1);
    let range = &head[start..];
    let (a, b) = match range.split_once('-') {
        Some((a, b)) => (a.parse().ok()?, b.parse().ok()?),
        None => {
            let v: f64 = range.parse().ok()?;
            (v, v)
        }
    };
    Some((a, b))
}

/// lensfun `lfFuzzyStrCmp::Split`: words by character class, solitary punctuation and "f"
/// dropped, trailing ".0" stripped from numbers, case-folded, sorted.
fn words(s: &str) -> Vec<String> {
    let cs: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < cs.len() {
        while i < cs.len() && cs[i].is_whitespace() {
            i += 1;
        }
        if i >= cs.len() {
            break;
        }
        let start = i;
        i += 1;
        let mut strip = 0;
        if cs[start].is_ascii_digit() {
            while i < cs.len() && (cs[i].is_ascii_digit() || cs[i] == '.') {
                i += 1;
            }
            if i - start >= 2 && cs[i - 2] == '.' && cs[i - 1] == '0' {
                strip = 2;
            }
        } else if cs[start].is_ascii_punctuation() {
            while i < cs.len() && cs[i].is_ascii_punctuation() {
                i += 1;
            }
        } else {
            while i < cs.len()
                && !cs[i].is_whitespace()
                && !cs[i].is_ascii_digit()
                && !cs[i].is_ascii_punctuation()
            {
                i += 1;
            }
        }
        if i - start == 1
            && (cs[start].is_ascii_punctuation() || cs[start].eq_ignore_ascii_case(&'f'))
            && cs[start] != '*'
            && cs[start] != '+'
        {
            continue;
        }
        out.push(
            cs[start..i - strip]
                .iter()
                .collect::<String>()
                .to_lowercase(),
        );
    }
    out.sort();
    out
}

/// lensfun `lfFuzzyStrCmp::Compare`: 0 if (strict and) a pattern word is missing, else
/// `200 · matched / (|pattern| + |match|)`.
fn fuzzy(pattern: &[String], matchw: &[String], strict: bool) -> i32 {
    if pattern.is_empty() || matchw.is_empty() {
        return 0;
    }
    let mut score = 0;
    let mut used = vec![false; matchw.len()];
    for p in pattern {
        match matchw
            .iter()
            .enumerate()
            .position(|(i, m)| !used[i] && m == p)
        {
            Some(i) => {
                used[i] = true;
                score += 1;
            }
            None if strict => return 0,
            None => {}
        }
    }
    (score * 200) / (pattern.len() + matchw.len()) as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_splitting_matches_lensfun() {
        assert_eq!(words("EF85mm f/1.8 USM"), ["1.8", "85", "ef", "mm", "usm"]);
        assert_eq!(
            words("Canon EF 85mm f/1.8 USM"),
            ["1.8", "85", "canon", "ef", "mm", "usm"]
        );
        assert_eq!(
            words("XF18-55mmF2.8-4 R LM OIS"),
            ["18", "2.8", "4", "55", "lm", "mmf", "ois", "r", "xf"]
        );
        assert_eq!(guess_focal("XF18-55mmF2.8-4"), Some((18.0, 55.0)));
        assert_eq!(guess_focal("EF85mm f/1.8"), Some((85.0, 85.0)));
    }

    fn fixture() -> Database {
        Database::load(&[PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures"
        ))])
        .unwrap()
    }

    #[test]
    fn reads_version_1_real_focal_and_rejects_unknown_versions() {
        let xml = r#"<lensdatabase version="1"><lens><maker>X</maker><model>X 24mm</model><mount>M</mount>
            <calibration><distortion model="poly3" focal="24" k1="0.01"/>
            <real-focal-length focal="24" real-focal="23.4"/></calibration></lens></lensdatabase>"#;
        let db = Database::parse(&[("v1.xml", xml)]).unwrap();
        assert_eq!(db.lenses[0].distortion[0].real_focal, Some(23.4));
        let xml = r#"<lensdatabase version="3"></lensdatabase>"#;
        assert!(Database::parse(&[("v3.xml", xml)]).is_err());
    }

    #[test]
    fn finds_sample_cameras_and_lenses() {
        let db = fixture();
        let cam = db.camera("FUJIFILM", "X-T20").expect("X-T20");
        assert!((cam.crop - 1.53).abs() < 0.01);
        let l = db.find_lenses(
            Some(cam),
            "FUJIFILM",
            "XF18-55mmF2.8-4 R LM OIS",
            (18.0, 55.0),
        );
        let best = l.first().expect("XF 18-55").0;
        assert_eq!(best.model(), "XF18-55mmF2.8-4 R LM OIS");
        assert!(!best.distortion.is_empty() && !best.tca.is_empty() && !best.vignetting.is_empty());

        let cam = db.camera("Canon", "EOS 6D").expect("6D");
        let l = db.find_lenses(Some(cam), "", "EF85mm f/1.8 USM", (85.0, 85.0));
        assert_eq!(
            l.first().expect("EF 85").0.model(),
            "Canon EF 85mm f/1.8 USM"
        );
        // A Fujifilm lens never matches a Canon body (mount).
        assert!(
            db.find_lenses(Some(cam), "", "XF18-55mmF2.8-4 R LM OIS", (18.0, 55.0))
                .is_empty()
        );
    }
}

/// The database bundled in the source tree loads and resolves the sample lenses.
#[cfg(test)]
#[test]
fn loads_bundled_database() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/lensfun/version_1");
    let db = Database::load(&[dir]).expect("database loads");
    assert!(db.lenses.len() > 1000);
    eprintln!("{} cameras, {} lenses", db.cameras.len(), db.lenses.len());
    let cam = db.camera("FUJIFILM", "X-T20").expect("X-T20");
    let l = db.find_lenses(
        Some(cam),
        "FUJIFILM",
        "XF18-55mmF2.8-4 R LM OIS",
        (18.0, 55.0),
    );
    assert_eq!(
        l.first().expect("XF 18-55").0.model(),
        "XF18-55mmF2.8-4 R LM OIS"
    );
}
