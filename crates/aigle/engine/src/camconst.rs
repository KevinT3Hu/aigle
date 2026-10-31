//! Per-camera white levels for A2 from RawTherapee's `camconst.json` (doc 02 §3).
//!
//! The table ships with aigle as `data/camconst/camconst.json` (RawTherapee, GPL-3.0-or-later;
//! revision in `data/manifest.toml`) and is read at runtime, never compiled in; lookup:
//! `$AIGLE_CAMCONST`, then `camconst/camconst.json` under the user data dir and the bundled
//! data dir (`crate::data::reference`). Only `ranges.white` (per ISO, per channel),
//! `white_max` and `aperture_scaling` are used; black levels come from LibRaw. For a camera
//! the table does not list, LibRaw's nominal maximum is used.

use std::sync::OnceLock;

use serde_json::Value;

/// White level per CFA colour index (R, G, B, G2), absolute raw units.
pub type WhiteLevels = [f32; 4];

#[derive(Clone, Debug, Default, PartialEq)]
struct Entry {
    /// Lower-case "make model" names.
    names: Vec<String>,
    /// `(isos, levels)`; empty `isos` = any ISO.
    white: Vec<(Vec<f32>, WhiteLevels)>,
    white_max: Option<f32>,
    /// `(f-number, scale)`, sorted by f-number.
    aperture_scaling: Vec<(f32, f32)>,
}

/// A parsed camconst table.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Table {
    entries: Vec<Entry>,
}

/// Strip `//` and `/* */` comments outside strings, and trailing commas.
fn strip_comments(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    let mut in_str = false;
    while i < b.len() {
        let c = b[i];
        if in_str {
            out.push(c);
            if c == b'\\' && i + 1 < b.len() {
                out.push(b[i + 1]);
                i += 1;
            } else if c == b'"' {
                in_str = false;
            }
            i += 1;
        } else if c == b'"' {
            in_str = true;
            out.push(c);
            i += 1;
        } else if b[i..].starts_with(b"//") {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
        } else if b[i..].starts_with(b"/*") {
            i += 2;
            while i < b.len() && !b[i..].starts_with(b"*/") {
                i += 1;
            }
            i += 2;
        } else {
            out.push(c);
            i += 1;
        }
    }
    // Trailing commas before `]` / `}` (outside strings: commas never precede them inside
    // the strings of this format).
    let mut res = Vec::with_capacity(out.len());
    let mut in_str = false;
    let mut k = 0;
    while k < out.len() {
        let c = out[k];
        if in_str {
            if c == b'\\' {
                res.push(c);
                k += 1;
                if k < out.len() {
                    res.push(out[k]);
                }
                k += 1;
                continue;
            }
            in_str = c != b'"';
        } else if c == b'"' {
            in_str = true;
        } else if c == b',' {
            let next = out[k + 1..].iter().find(|c| !c.is_ascii_whitespace());
            if matches!(next, Some(b']') | Some(b'}')) {
                k += 1;
                continue;
            }
        }
        res.push(c);
        k += 1;
    }
    String::from_utf8_lossy(&res).into_owned()
}

fn num(v: &Value) -> Option<f32> {
    v.as_f64().map(|f| f as f32)
}

/// `levels`: one value, `[R, G, B]` (G2 = G) or `[R, G1, B, G2]`.
fn levels(v: &Value) -> Option<WhiteLevels> {
    if let Some(l) = num(v) {
        return Some([l; 4]);
    }
    let a: Vec<f32> = v.as_array()?.iter().filter_map(num).collect();
    match a.len() {
        1 => Some([a[0]; 4]),
        3 => Some([a[0], a[1], a[2], a[1]]),
        4 => Some([a[0], a[1], a[2], a[3]]),
        _ => None,
    }
}

fn entry(v: &Value) -> Option<Entry> {
    let names = match v.get("make_model")? {
        Value::String(s) => vec![s.to_lowercase()],
        Value::Array(a) => a
            .iter()
            .filter_map(|s| s.as_str().map(str::to_lowercase))
            .collect(),
        _ => return None,
    };
    let ranges = v.get("ranges");
    let mut white = Vec::new();
    match ranges.and_then(|r| r.get("white")) {
        Some(Value::Array(a)) if a.iter().all(Value::is_object) => {
            for r in a {
                let isos = match r.get("iso") {
                    Some(Value::Array(i)) => i.iter().filter_map(num).collect(),
                    Some(i) => num(i).into_iter().collect(),
                    None => Vec::new(),
                };
                if let Some(l) = r.get("levels").and_then(levels) {
                    white.push((isos, l));
                }
            }
        }
        Some(w) => white.extend(levels(w).map(|l| (Vec::new(), l))),
        None => {}
    }
    let white_max = ranges.and_then(|r| r.get("white_max")).and_then(num);
    let mut aperture_scaling: Vec<(f32, f32)> = ranges
        .and_then(|r| r.get("aperture_scaling"))
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|e| Some((num(e.get("aperture")?)?, num(e.get("scale_factor")?)?)))
                .collect()
        })
        .unwrap_or_default();
    aperture_scaling.sort_by(|a, b| a.0.total_cmp(&b.0));
    Some(Entry {
        names,
        white,
        white_max,
        aperture_scaling,
    })
}

impl Table {
    pub fn parse(text: &str) -> Result<Table, String> {
        let v: Value = serde_json::from_str(&strip_comments(text)).map_err(|e| e.to_string())?;
        let list = v
            .get("camera_constants")
            .and_then(Value::as_array)
            .ok_or("no camera_constants array")?;
        Ok(Table {
            entries: list.iter().filter_map(entry).collect(),
        })
    }

    /// White levels for a shot, or `None` if the camera has no white entry. ISO picks the
    /// closest listed ISO; aperture scaling uses the factor of the nearest listed f-number
    /// at or above the shot's (conservative), capped at `white_max`.
    pub fn white(&self, make: &str, model: &str, iso: f32, f_number: f32) -> Option<WhiteLevels> {
        let name = format!("{make} {model}").to_lowercase();
        let e = self
            .entries
            .iter()
            .find(|e| !e.white.is_empty() && e.names.contains(&name))?;
        let dist = |isos: &Vec<f32>| {
            if isos.is_empty() || iso <= 0.0 {
                f32::MAX / 2.0
            } else {
                isos.iter()
                    .map(|i| (i - iso).abs())
                    .fold(f32::MAX, f32::min)
            }
        };
        let (_, mut l) = e
            .white
            .iter()
            .min_by(|a, b| dist(&a.0).total_cmp(&dist(&b.0)))?
            .clone();
        if f_number > 0.0
            && let Some(&(_, s)) = e
                .aperture_scaling
                .iter()
                .find(|(a, _)| *a >= f_number - 0.05)
        {
            let cap = e.white_max.unwrap_or(f32::MAX);
            l = l.map(|v| (v * s).min(cap));
        }
        Some(l)
    }
}

/// The runtime table, loaded once (`None`, with a warning, if it is missing or does not
/// parse).
pub fn table() -> Option<&'static Table> {
    static T: OnceLock<Option<Table>> = OnceLock::new();
    T.get_or_init(|| {
        let Some(p) = crate::data::reference("AIGLE_CAMCONST", "camconst/camconst.json") else {
            tracing::warn!("camconst.json not found; using LibRaw's nominal white levels");
            return None;
        };
        let parsed = std::fs::read_to_string(&p)
            .map_err(|e| e.to_string())
            .and_then(|t| Table::parse(&t));
        match parsed {
            Ok(t) => {
                tracing::debug!(path = %p.display(), cameras = t.entries.len(), "camconst loaded");
                Some(t)
            }
            Err(e) => {
                tracing::warn!(path = %p.display(), "camconst unusable: {e}");
                None
            }
        }
    })
    .as_ref()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"/* header // with "quotes" */
    {"camera_constants": [
        { // a comment
            "make_model": [ "Canon EOS 6D", "Canon EOS 6D Alt" ],
            "ranges": {
                "white": [
                    { "iso": [ 100, 200 ], "levels": 15180 }, // typical 15283
                    { "iso": 160, "levels": [ 13100, 13200, 13000 ] },
                ],
                "white_max": 16383,
                "aperture_scaling": [
                    { "aperture": 1.4, "scale_factor": 1.090 },
                    { "aperture": 2.0, "scale_factor": 1.030 },
                ]
            }
        },
        { "make_model": "Foo Bar", "ranges": { "white": 4000 } },
        { "make_model": "Foo Matrix", "dcraw_matrix": [ 1, 2, 3 ] }
    ]}"#;

    #[test]
    fn parses_and_looks_up_white_levels() {
        let t = Table::parse(SAMPLE).unwrap();
        assert_eq!(t.white("Canon", "EOS 6D", 100.0, 8.0), Some([15180.0; 4]));
        assert_eq!(
            t.white("CANON", "eos 6d alt", 125.0, 0.0),
            Some([15180.0; 4])
        );
        assert_eq!(
            t.white("Canon", "EOS 6D", 160.0, 0.0),
            Some([13100.0, 13200.0, 13000.0, 13200.0])
        );
        // f/1.8 uses the f/2.0 factor.
        let w = t.white("Canon", "EOS 6D", 100.0, 1.8).unwrap();
        assert!((w[0] - 15180.0 * 1.03).abs() < 0.5, "{w:?}");
        // f/1.2 is capped at white_max.
        assert!(t.white("Canon", "EOS 6D", 100.0, 1.2).unwrap()[0] <= 16383.0);
        assert_eq!(t.white("Foo", "Bar", 800.0, 4.0), Some([4000.0; 4]));
        assert_eq!(t.white("Foo", "Matrix", 800.0, 4.0), None);
        assert_eq!(t.white("Nobody", "Else", 100.0, 4.0), None);
    }

    #[test]
    fn bundled_table_parses() {
        let t = table().expect("bundled camconst.json");
        assert!(t.entries.len() > 100);
        assert_eq!(t.white("FUJIFILM", "X-T20", 200.0, 4.0), Some([16100.0; 4]));
    }
}
