//! Application translations. Catalog keys are flat dotted paths such as `param.light.exposure`.
//! Unknown text (including user-authored names) is shown unchanged.

use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Language {
    English,
    Chinese,
}

impl Language {
    pub fn from_locale(locale: &str) -> Self {
        if locale
            .split(['_', '-', '.', ':'])
            .next()
            .is_some_and(|s| s.eq_ignore_ascii_case("zh"))
        {
            Self::Chinese
        } else {
            Self::English
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            Self::English => "en",
            Self::Chinese => "zh-CN",
        }
    }
}

pub fn language() -> Language {
    Language::from_locale(&rust_i18n::locale())
}

pub fn translate(language: Language, key: &str) -> String {
    // rust-i18n owns the bundled catalog and performs locale fallback. Show unknown text
    // as is instead of the library's missing-key diagnostic.
    crate::_rust_i18n_try_translate(language.code(), key)
        .map(|value| value.into_owned())
        .unwrap_or_else(|| key.to_owned())
}

pub fn t(key: &str) -> String {
    translate(language(), key)
}

/// Separates the parts of a history label.
const SEPARATOR: &str = " · ";
/// Prefix of the label of a command that resets what the rest of the label names.
const RESET: &str = "history.reset";

/// A history label made of catalog keys and user text, e.g. `["history.preset", name]`.
/// Labels are stored untranslated and translated by [`history_label`] when shown.
pub fn join(parts: &[&str]) -> String {
    parts.join(SEPARATOR)
}

/// The history label of resetting what `label` names.
pub fn reset_label(label: &str) -> String {
    format!("{RESET} {label}")
}

/// Translate a history label built with [`join`] or [`reset_label`].
pub fn history_label(label: &str) -> String {
    history_label_in(language(), label)
}

fn history_label_in(lang: Language, label: &str) -> String {
    if let Some(rest) = label.strip_prefix(RESET).and_then(|r| r.strip_prefix(' ')) {
        return format!(
            "{} {}",
            translate(lang, RESET),
            history_label_in(lang, rest)
        );
    }
    label
        .split(SEPARATOR)
        .map(|part| translate(lang, part))
        .collect::<Vec<_>>()
        .join(SEPARATOR)
}

fn preference_path() -> Option<PathBuf> {
    crate::config_dir().map(|p| p.join("language"))
}

pub fn init() {
    let saved = preference_path().and_then(|p| std::fs::read_to_string(p).ok());
    let locale = saved
        .or_else(|| {
            ["LC_ALL", "LANGUAGE", "LC_MESSAGES", "LANG"]
                .into_iter()
                .find_map(|name| std::env::var(name).ok().filter(|v| !v.is_empty()))
        })
        .unwrap_or_else(|| "en".into());
    set_language(Language::from_locale(locale.trim()));
}

pub fn set_language(language: Language) {
    rust_i18n::set_locale(language.code());
}

pub fn save_language(language: Language) -> std::io::Result<()> {
    set_language(language);
    if let Some(path) = preference_path() {
        std::fs::create_dir_all(path.parent().expect("preference has a parent"))?;
        std::fs::write(path, language.code())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeSet, HashMap};
    use std::path::Path;

    use super::*;

    fn catalog(source: &str) -> HashMap<String, String> {
        serde_json::from_str(source).unwrap()
    }

    fn en() -> HashMap<String, String> {
        catalog(include_str!("../locales/en.json"))
    }

    #[test]
    fn detects_locales_and_falls_back_to_english() {
        for locale in ["zh_CN.UTF-8", "zh-Hans", "zh_TW", "ZH-cn", "zh:en"] {
            assert_eq!(Language::from_locale(locale), Language::Chinese);
        }
        for locale in ["en_US.UTF-8", "de_DE", "C", "", "zhx"] {
            assert_eq!(Language::from_locale(locale), Language::English);
        }
    }

    #[test]
    fn history_labels_follow_the_selected_language() {
        let hue = join(&["section.grading_shadows", "param.grading.shadows.hue"]);
        assert_eq!(history_label_in(Language::English, &hue), "Shadows · Hue");
        assert_eq!(history_label_in(Language::Chinese, &hue), "阴影 · 色相");
        assert_eq!(
            history_label_in(Language::English, &reset_label(&hue)),
            "Reset Shadows · Hue"
        );
        assert_eq!(
            history_label_in(Language::Chinese, &reset_label(&hue)),
            "重置 阴影 · 色相"
        );
        assert_eq!(
            history_label_in(Language::English, &join(&["history.preset", "My look"])),
            "Preset · My look"
        );
        assert_eq!(
            history_label_in(Language::English, "My custom edit"),
            "My custom edit"
        );
    }

    #[test]
    fn library_interpolates_both_languages_without_changing_global_locale() {
        assert_eq!(
            rust_i18n::t!(
                "export.failed_one",
                locale = "en",
                shown = "photo.jpg",
                e = "disk full"
            ),
            "Failed to export photo.jpg: disk full"
        );
        assert_eq!(
            rust_i18n::t!(
                "export.failed_one",
                locale = "zh-CN",
                shown = "photo.jpg",
                e = "disk full"
            ),
            "导出 photo.jpg 失败：disk full"
        );
        assert_eq!(
            rust_i18n::t!("meta.dimensions_value", locale = "en", w = 6000, h = 4000, arg0 = 24.0 : {:.1}),
            "6000 × 4000 (24.0 MP)"
        );
        assert_eq!(
            rust_i18n::t!("param.light.exposure", locale = "fr"),
            "Exposure"
        );
    }

    #[test]
    fn catalogs_cover_both_languages_and_preserve_format_arguments() {
        let en = en();
        let zh = catalog(include_str!("../locales/zh-CN.json"));
        assert_eq!(en.len(), zh.len());
        fn placeholders(s: &str) -> Vec<&str> {
            let mut result: Vec<_> = s
                .split('{')
                .skip(1)
                .map(|s| s.split('}').next().unwrap())
                .collect();
            result.sort_unstable();
            result
        }
        for (key, value) in &en {
            assert!(is_key(key), "{key} is not a dotted key");
            assert!(zh.contains_key(key), "{key}");
            assert!(!value.is_empty(), "{key}");
            assert_eq!(placeholders(&zh[key]), placeholders(value), "{key}");
        }
        assert_eq!(
            translate(Language::English, "param.light.exposure"),
            "Exposure"
        );
        assert_eq!(translate(Language::Chinese, "param.light.exposure"), "曝光");
        assert_eq!(translate(Language::English, "a user name"), "a user name");
    }

    fn is_key(s: &str) -> bool {
        s.contains('.')
            && s.split('.').all(|seg| {
                !seg.is_empty() && seg.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            })
    }

    /// String literals of a Rust source file, without comments and char literals.
    fn string_literals(src: &str) -> Vec<String> {
        let b: Vec<char> = src.chars().collect();
        let mut out = Vec::new();
        let mut i = 0;
        while i < b.len() {
            let c = b[i];
            let next = b.get(i + 1).copied();
            if c == '/' && next == Some('/') {
                while i < b.len() && b[i] != '\n' {
                    i += 1;
                }
            } else if c == '/' && next == Some('*') {
                i += 2;
                while i + 1 < b.len() && !(b[i] == '*' && b[i + 1] == '/') {
                    i += 1;
                }
                i += 2;
            } else if c == '\'' {
                i += match (next, b.get(i + 2)) {
                    (Some('\\'), _) => {
                        let mut j = i + 2;
                        while j < b.len() && b[j] != '\'' {
                            j += 1;
                        }
                        j + 1 - i
                    }
                    (Some(_), Some('\'')) => 3,
                    _ => 1,
                };
            } else if c == 'r'
                && matches!(next, Some('"' | '#'))
                && (i == 0 || !(b[i - 1].is_alphanumeric() || b[i - 1] == '_'))
            {
                let mut j = i + 1;
                let mut hashes = 0;
                while b.get(j) == Some(&'#') {
                    hashes += 1;
                    j += 1;
                }
                if b.get(j) != Some(&'"') {
                    i += 1;
                    continue;
                }
                let start = j + 1;
                let mut k = start;
                while k < b.len()
                    && !(b[k] == '"' && (1..=hashes).all(|h| b.get(k + h) == Some(&'#')))
                {
                    k += 1;
                }
                out.push(b[start..k.min(b.len())].iter().collect());
                i = k + 1 + hashes;
            } else if c == '"' {
                let mut s = String::new();
                i += 1;
                while i < b.len() && b[i] != '"' {
                    if b[i] == '\\' {
                        i += 1;
                    }
                    if let Some(&ch) = b.get(i) {
                        s.push(ch);
                    }
                    i += 1;
                }
                out.push(s);
                i += 1;
            } else {
                i += 1;
            }
        }
        out
    }

    /// The non-test code of every source file of the app.
    fn sources() -> Vec<(String, String)> {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut out = Vec::new();
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_some_and(|e| e == "rs") {
                let src = std::fs::read_to_string(&path).unwrap();
                let code = match src.find("\n#[cfg(test)]\nmod tests") {
                    Some(end) => src[..end].to_owned(),
                    None => src,
                };
                out.push((
                    path.file_name().unwrap().to_string_lossy().into_owned(),
                    code,
                ));
            }
        }
        out
    }

    #[test]
    fn string_literals_skip_comments_and_chars() {
        let src = r####"
            // "in a comment"
            /* "in a block" */
            let q = '"';
            let e = '\'';
            fn f<'a>(x: &'a str) {}
            let s = "plain \"quoted\"";
            let r = r#"raw "inner""#;
        "####;
        assert_eq!(string_literals(src), ["plain \"quoted\"", "raw \"inner\""]);
    }

    #[test]
    fn ui_text_comes_from_the_catalog() {
        let en = en();
        let prefixes: BTreeSet<&str> = en.keys().map(|k| k.split('.').next().unwrap()).collect();
        let mut problems = Vec::new();
        for (file, code) in sources() {
            for s in string_literals(&code) {
                if s.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c)) {
                    problems.push(format!("{file}: untranslated text {s:?}"));
                } else if is_key(&s)
                    && prefixes.contains(s.split('.').next().unwrap())
                    && !en.contains_key(&s)
                {
                    problems.push(format!("{file}: unknown key {s:?}"));
                }
            }
        }
        assert!(problems.is_empty(), "{}", problems.join("\n"));
    }

    #[test]
    fn derived_keys_exist() {
        let en = en();
        let mut keys: Vec<String> = crate::panel::TOOL_GROUPS
            .iter()
            .flat_map(|g| g.sections)
            .flat_map(|s| s.rows)
            .map(|path| crate::panel::label_key(path))
            .collect();
        keys.extend(
            aigle_core::masks::LocalParams::SPECS
                .iter()
                .map(|s| format!("param.local.{}", s.name)),
        );
        let actions = include_str!("actions.rs");
        let list = &actions[actions.find('[').unwrap() + 1..actions.find(']').unwrap()];
        for name in list.split(',').map(str::trim).filter(|n| !n.is_empty()) {
            let name = ["Rate", "Label"]
                .into_iter()
                .find(|p| name.starts_with(p))
                .unwrap_or(name);
            keys.push(format!("action.{name}"));
        }
        for key in keys {
            assert!(en.contains_key(&key), "{key}");
        }
    }
}
