// Translations are wired into the UI in a later step.
#![allow(dead_code)]
//! UI translations. Strings are keyed by their English text, so untranslated strings fall
//! back to English and code stays readable: `tr("Commit")`.
//!
//! Built-in locales live in `assets/locales/<code>.json`; users can add or override one with
//! `<config dir>/locales/<code>.json` (same format: `{"name": "...", "strings": {...}}`).

use std::collections::HashMap;
use std::sync::RwLock;

use gpui::SharedString;
use serde::Deserialize;

const BUILTIN: &[(&str, &str)] = &[("ru", include_str!("../assets/locales/ru.json"))];

#[derive(Debug, Default, Deserialize)]
struct LocaleFile {
    #[serde(default)]
    name: String,
    #[serde(default)]
    strings: HashMap<String, String>,
}

struct Active {
    code: String,
    strings: HashMap<String, String>,
}

static ACTIVE: RwLock<Option<Active>> = RwLock::new(None);

/// `(code, native name)` of every available language, English first.
pub fn languages() -> Vec<(String, String)> {
    let mut out = vec![("en".to_string(), "English".to_string())];
    for (code, text) in BUILTIN {
        let file: LocaleFile = serde_json::from_str(text).unwrap_or_default();
        out.push((code.to_string(), file.name));
    }
    if let Ok(entries) = std::fs::read_dir(crate::config::dir().join("locales")) {
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(code) = path.file_stem().and_then(|s| s.to_str()).map(str::to_string) else { continue };
            if path.extension().is_some_and(|e| e == "json") && !out.iter().any(|(c, _)| *c == code) {
                let name = std::fs::read_to_string(&path)
                    .ok()
                    .and_then(|t| serde_json::from_str::<LocaleFile>(&t).ok())
                    .map(|f| f.name)
                    .filter(|n| !n.is_empty())
                    .unwrap_or_else(|| code.clone());
                out.push((code, name));
            }
        }
    }
    out
}

/// The OS language as a two-letter code.
pub fn system_language() -> String {
    sys_locale::get_locale().map(|l| l.chars().take(2).collect::<String>().to_ascii_lowercase()).unwrap_or_else(|| "en".into())
}

/// Loads `code` (`"system"` follows the OS). Unknown codes fall back to English.
pub fn set_language(code: &str) {
    let code = if code.is_empty() || code == "system" { system_language() } else { code.to_string() };
    let mut strings = HashMap::new();
    if let Some((_, text)) = BUILTIN.iter().find(|(c, _)| *c == code) {
        strings = serde_json::from_str::<LocaleFile>(text).map(|f| f.strings).unwrap_or_default();
    }
    if let Some(user) = std::fs::read_to_string(crate::config::dir().join("locales").join(format!("{code}.json")))
        .ok()
        .and_then(|t| serde_json::from_str::<LocaleFile>(&t).ok())
    {
        strings.extend(user.strings);
    }
    *ACTIVE.write().unwrap() = Some(Active { code, strings });
}

pub fn current() -> String {
    ACTIVE.read().unwrap().as_ref().map(|a| a.code.clone()).unwrap_or_else(|| "en".into())
}

/// The translation of an English UI string.
pub fn tr(english: &str) -> SharedString {
    let active = ACTIVE.read().unwrap();
    match active.as_ref().and_then(|a| a.strings.get(english)) {
        Some(text) => SharedString::from(text.clone()),
        None => SharedString::from(english.to_string()),
    }
}

/// [`tr`] with `{}` placeholders filled in order: `trf("{} files", &[&n.to_string()])`.
pub fn trf(english: &str, args: &[&str]) -> String {
    let mut out = tr(english).to_string();
    for arg in args {
        if let Some(at) = out.find("{}") {
            out.replace_range(at..at + 2, arg);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translates_with_fallback() {
        set_language("ru");
        assert_eq!(tr("Commit").as_ref(), "Коммит");
        assert_eq!(tr("Some string nobody translated").as_ref(), "Some string nobody translated");
        assert_eq!(trf("{} files", &["3"]), "файлов: 3");
        set_language("en");
        assert_eq!(tr("Commit").as_ref(), "Commit");
        // Every built-in locale parses and has no placeholder mismatches.
        for (code, text) in BUILTIN {
            let file: LocaleFile = serde_json::from_str(text).unwrap_or_else(|e| panic!("{code}: {e}"));
            for (en, translated) in &file.strings {
                assert_eq!(en.matches("{}").count(), translated.matches("{}").count(), "{code}: {en}");
            }
        }
    }
}
