//! What the library filters and sorts by that only the photo file knows: camera, lens and
//! capture date, read from its EXIF.

use std::path::Path;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Facts {
    /// `Nikon D3200`, without repeating the make.
    pub camera: Option<String>,
    pub lens: Option<String>,
    /// Capture time as recorded by the camera, `YYYY-MM-DD HH:MM:SS`.
    pub captured: Option<String>,
}

impl Facts {
    /// `YYYY-MM-DD` of [`Facts::captured`].
    pub fn day(&self) -> Option<&str> {
        self.captured.as_deref().map(|c| &c[..10])
    }
}

/// The facts of `photo`; unknown ones (or all, if Exiv2 cannot read the file) are `None`.
pub fn read_facts(photo: &Path) -> Facts {
    let exif = match crate::worker::exif_facts(photo) {
        Ok(e) => e,
        Err(e) => {
            tracing::debug!(path = %photo.display(), "no EXIF: {e}");
            return Facts::default();
        }
    };
    Facts {
        camera: camera_name(exif.make.as_deref(), exif.model.as_deref()),
        lens: exif.lens.filter(|l| known_lens(l)),
        captured: exif.date_time.as_deref().and_then(capture_time),
    }
}

/// `make model`, using only the first word of the make (`NIKON CORPORATION` → `NIKON`) and
/// leaving it out when the model already starts with it.
pub fn camera_name(make: Option<&str>, model: Option<&str>) -> Option<String> {
    let make = make.unwrap_or("").split_whitespace().next().unwrap_or("");
    let model = model.unwrap_or("").trim();
    let name = if make.is_empty() || model.to_lowercase().starts_with(&make.to_lowercase()) {
        model.to_string()
    } else if model.is_empty() {
        make.to_string()
    } else {
        format!("{make} {model}")
    };
    (!name.is_empty()).then_some(name)
}

/// Exiv2 prints lens IDs it cannot name as `(123)` or `Unknown …`.
fn known_lens(l: &str) -> bool {
    let lower = l.to_lowercase();
    !(l.starts_with('(')
        || lower.starts_with("unknown")
        || lower == "n/a"
        || l.chars().all(|c| c.is_ascii_digit() || c == ' '))
}

/// `2017:01:08 11:18:13` → `2017-01-08 11:18:13`; `None` for blank or malformed dates.
fn capture_time(exif: &str) -> Option<String> {
    let b = exif.trim().as_bytes();
    if b.len() < 19 {
        return None;
    }
    let digits = [0, 1, 2, 3, 5, 6, 8, 9, 11, 12, 14, 15, 17, 18];
    let seps = [(4, b':'), (7, b':'), (10, b' '), (13, b':'), (16, b':')];
    if !digits.iter().all(|&i| b[i].is_ascii_digit())
        || !seps
            .iter()
            .all(|&(i, c)| b[i] == c || (i < 10 && b[i] == b'-'))
    {
        return None;
    }
    let s = std::str::from_utf8(&b[..19]).ok()?;
    let (month, day) = (&s[5..7], &s[8..10]);
    if s.starts_with("0000") || month == "00" || day == "00" {
        return None;
    }
    Some(format!("{}-{month}-{day}{}", &s[..4], &s[10..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_are_normalised() {
        assert_eq!(
            capture_time("2017:01:08 11:18:13").as_deref(),
            Some("2017-01-08 11:18:13")
        );
        assert_eq!(capture_time("0000:00:00 00:00:00"), None);
        assert_eq!(capture_time("    :  :     :  :  "), None);
        assert_eq!(
            camera_name(Some("Canon"), Some("Canon EOS 6D")).as_deref(),
            Some("Canon EOS 6D")
        );
        assert_eq!(
            camera_name(Some("NIKON CORPORATION"), Some("NIKON D3200")).as_deref(),
            Some("NIKON D3200")
        );
        assert_eq!(
            camera_name(Some("OLYMPUS IMAGING CORP."), Some("E-M5")).as_deref(),
            Some("OLYMPUS E-M5")
        );
        assert_eq!(camera_name(None, Some(" ")), None);
        assert!(known_lens("EF50mm f/1.8 STM"));
        assert!(!known_lens("(65535)"));
        assert!(!known_lens("Unknown (0x1234)"));
        assert!(!known_lens("0 0"));
    }
}
