//! Application translations. Catalog keys are the original Chinese UI strings so existing
//! edit labels remain readable. Unknown text (including user-authored names) is preserved.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::OnceLock;

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
    // rust-i18n owns the bundled catalog and performs locale fallback. Preserve unknown
    // history labels instead of displaying the library's missing-key diagnostic.
    crate::_rust_i18n_try_translate(language.code(), key)
        .map(|value| value.into_owned())
        .unwrap_or_else(|| key.to_owned())
}

pub fn t(key: &str) -> String {
    translate(language(), key)
}

/// Translate canonical or previously localized edit labels when rendering history.
/// No substitutions are made within arbitrary user text.
pub fn history_label(label: &str) -> String {
    history_label_in(language(), label)
}

fn history_label_in(lang: Language, label: &str) -> String {
    fn part(lang: Language, label: &str) -> String {
        static LEGACY_KEYS: OnceLock<HashMap<String, String>> = OnceLock::new();
        let legacy = LEGACY_KEYS.get_or_init(|| {
            crate::_rust_i18n_backend()
                .messages_for_locale("en")
                .unwrap_or_default()
                .into_iter()
                .filter(|(_, value)| !value.contains("%{"))
                .map(|(key, value)| (value.into_owned(), key.into_owned()))
                .collect()
        });
        let key = legacy.get(label).map(String::as_str).unwrap_or(label);
        translate(lang, key)
    }
    let direct = part(lang, label);
    if direct != label {
        return direct;
    }
    if let Some(rest) = label
        .strip_prefix("重置 ")
        .or_else(|| label.strip_prefix("Reset "))
    {
        return format!(
            "{} {}",
            translate(lang, "重置"),
            history_label_in(lang, rest)
        );
    }
    label
        .split(" · ")
        .map(|s| part(lang, s))
        .collect::<Vec<_>>()
        .join(" · ")
}

fn preference_path() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".config")))
        .map(|p| p.join("aigle/language"))
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
    use super::*;

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
        assert_eq!(
            history_label_in(Language::English, "阴影 · 色相"),
            "Shadows · Hue"
        );
        assert_eq!(
            history_label_in(Language::Chinese, "Shadows · Hue"),
            "阴影 · 色相"
        );
        assert_eq!(
            history_label_in(Language::English, "重置 阴影 · 色相"),
            "Reset Shadows · Hue"
        );
        assert_eq!(
            history_label_in(Language::Chinese, "Reset Shadows · Hue"),
            "重置 阴影 · 色相"
        );
        assert_eq!(
            history_label_in(Language::Chinese, "Lens Correction · Enable Profile"),
            "镜头校正 · 启用配置文件"
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
                "导出 {shown} 失败：{e}",
                locale = "en",
                shown = "photo.jpg",
                e = "disk full"
            ),
            "Failed to export photo.jpg: disk full"
        );
        assert_eq!(
            rust_i18n::t!(
                "导出 {shown} 失败：{e}",
                locale = "zh-CN",
                shown = "photo.jpg",
                e = "disk full"
            ),
            "导出 photo.jpg 失败：disk full"
        );
        assert_eq!(
            rust_i18n::t!("{w} × {h}（{:.1} MP）", locale = "en", w = 6000, h = 4000, arg0 = 24.0 : {:.1}),
            "6000 × 4000 (24.0 MP)"
        );
        assert_eq!(rust_i18n::t!("曝光", locale = "fr"), "Exposure");
    }

    #[test]
    fn catalogs_cover_both_languages_and_preserve_format_arguments() {
        let en: HashMap<String, String> =
            serde_json::from_str(include_str!("../locales/en.json")).unwrap();
        let zh: HashMap<String, String> =
            serde_json::from_str(include_str!("../locales/zh-CN.json")).unwrap();
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
            assert!(zh.contains_key(key), "{key}");
            assert!(!value.is_empty(), "{key}");
            assert_eq!(placeholders(&zh[key]), placeholders(value), "{key}");
        }
        assert_eq!(translate(Language::English, "曝光"), "Exposure");
        assert_eq!(translate(Language::Chinese, "曝光"), "曝光");
        assert_eq!(translate(Language::English, "a user name"), "a user name");
    }
}
