//! Themes: JSON files with UI and syntax colors. Built-ins ship with the app; user themes
//! live in `<config dir>/phpcraft/themes/*.json` and use the same format.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use gpui::{App, Hsla, Rgba, SharedString};
use php_syntax::Highlight;
use serde::Deserialize;

use crate::config;

const BUILTIN: &[&str] = &[
    include_str!("../assets/themes/phpcraft-dark.json"),
    include_str!("../assets/themes/phpcraft-light.json"),
    include_str!("../assets/themes/darcula.json"),
];

pub const DEFAULT_THEME: &str = "PhpCraft Dark";

#[derive(Debug, Clone, Deserialize)]
pub struct ThemeFile {
    pub name: String,
    #[serde(default)]
    pub appearance: String,
    #[serde(default)]
    pub colors: HashMap<String, String>,
    #[serde(default)]
    pub syntax: HashMap<String, String>,
}

/// A theme with every color resolved (missing keys fall back to the default theme).
pub struct Theme {
    pub name: String,
    colors: HashMap<String, Hsla>,
    syntax: HashMap<String, Hsla>,
}

struct Fonts {
    editor: SharedString,
    editor_size: f32,
    ui: SharedString,
}

static CURRENT: RwLock<Option<Arc<Theme>>> = RwLock::new(None);
static FONTS: RwLock<Option<Fonts>> = RwLock::new(None);

fn parse_hex(hex: &str) -> Option<Hsla> {
    let hex = hex.trim().trim_start_matches('#');
    let value = u32::from_str_radix(hex, 16).ok()?;
    let rgba = match hex.len() {
        6 => Rgba {
            r: ((value >> 16) & 0xff) as f32 / 255.,
            g: ((value >> 8) & 0xff) as f32 / 255.,
            b: (value & 0xff) as f32 / 255.,
            a: 1.,
        },
        8 => Rgba {
            r: ((value >> 24) & 0xff) as f32 / 255.,
            g: ((value >> 16) & 0xff) as f32 / 255.,
            b: ((value >> 8) & 0xff) as f32 / 255.,
            a: (value & 0xff) as f32 / 255.,
        },
        _ => return None,
    };
    Some(rgba.into())
}

fn resolve(file: &ThemeFile, base: Option<&Theme>) -> Theme {
    let mut colors: HashMap<String, Hsla> = base.map(|b| b.colors.clone()).unwrap_or_default();
    let mut syntax: HashMap<String, Hsla> = base.map(|b| b.syntax.clone()).unwrap_or_default();
    colors.extend(
        file.colors
            .iter()
            .filter_map(|(k, v)| Some((k.clone(), parse_hex(v)?))),
    );
    syntax.extend(
        file.syntax
            .iter()
            .filter_map(|(k, v)| Some((k.clone(), parse_hex(v)?))),
    );
    Theme {
        name: file.name.clone(),
        colors,
        syntax,
    }
}

fn builtin_files() -> Vec<ThemeFile> {
    BUILTIN
        .iter()
        .filter_map(|json| serde_json::from_str(json).ok())
        .collect()
}

fn user_files() -> Vec<ThemeFile> {
    let Ok(entries) = std::fs::read_dir(config::dir().join("themes")) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| serde_json::from_str(&std::fs::read_to_string(e.path()).ok()?).ok())
        .collect()
}

/// Built-in themes first, then user themes (a user theme may shadow a built-in by name).
pub fn available() -> Vec<ThemeFile> {
    let mut themes = builtin_files();
    for theme in user_files() {
        themes.retain(|t| t.name != theme.name);
        themes.push(theme);
    }
    themes
}

/// Activates a theme by name (unknown names fall back to the default) and remembers it.
pub fn set(name: &str, cx: &mut App) {
    let files = available();
    let default = files
        .iter()
        .find(|t| t.name == DEFAULT_THEME)
        .map(|f| resolve(f, None));
    let chosen = files
        .iter()
        .find(|t| t.name == name)
        .or_else(|| files.iter().find(|t| t.name == DEFAULT_THEME));
    if let Some(file) = chosen {
        let theme = resolve(file, default.as_ref());
        *CURRENT.write().unwrap() = Some(Arc::new(theme));
    }
    let mut settings = config::Settings::load();
    settings.theme = name.to_string();
    settings.save();
    cx.refresh_windows();
}

/// First installed font from `candidates`, else the last one.
fn pick_font(cx: &App, candidates: &[&str]) -> SharedString {
    let installed = cx.text_system().all_font_names();
    candidates
        .iter()
        .find(|c| c.starts_with('.') || installed.iter().any(|f| f.eq_ignore_ascii_case(c)))
        .or(candidates.last())
        .map(|c| SharedString::from(c.to_string()))
        .unwrap_or_default()
}

/// Loads fonts and the saved theme. Call once at startup.
pub fn init(cx: &mut App) {
    let settings = config::Settings::load();
    let editor = pick_font(
        cx,
        &[
            &settings.editor_font,
            "JetBrains Mono",
            "Menlo",
            "SF Mono",
            "Consolas",
            "Cascadia Mono",
            "DejaVu Sans Mono",
            "Liberation Mono",
            "monospace",
        ],
    );
    let ui = if cfg!(target_os = "macos") {
        SharedString::from(".SystemUIFont")
    } else {
        pick_font(
            cx,
            &[
                "Segoe UI",
                "Inter",
                "Ubuntu",
                "Noto Sans",
                "Cantarell",
                "DejaVu Sans",
                "sans-serif",
            ],
        )
    };
    *FONTS.write().unwrap() = Some(Fonts { editor, editor_size: settings.editor_font_size.clamp(8., 32.), ui });
    crate::i18n::set_language(&settings.language);
    set(&settings.theme, cx);
}

pub fn current_name() -> String {
    CURRENT
        .read()
        .unwrap()
        .as_ref()
        .map(|t| t.name.clone())
        .unwrap_or_else(|| DEFAULT_THEME.into())
}

pub fn editor_font() -> SharedString {
    FONTS
        .read()
        .unwrap()
        .as_ref()
        .map(|f| f.editor.clone())
        .unwrap_or_else(|| "Menlo".into())
}

/// Editor (and terminal) font size in pixels.
pub fn editor_font_size() -> f32 {
    FONTS.read().unwrap().as_ref().map(|f| f.editor_size).unwrap_or(13.5)
}

/// Line height that fits [`editor_font_size`].
pub fn editor_line_height() -> f32 {
    (editor_font_size() * 1.48).round()
}

pub fn ui_font() -> SharedString {
    FONTS
        .read()
        .unwrap()
        .as_ref()
        .map(|f| f.ui.clone())
        .unwrap_or_else(|| ".SystemUIFont".into())
}

fn color(key: &str) -> Hsla {
    CURRENT
        .read()
        .unwrap()
        .as_ref()
        .and_then(|t| t.colors.get(key).copied())
        .unwrap_or_else(|| gpui::hsla(0., 0., 0.5, 1.))
}

macro_rules! colors {
    ($($name:ident),* $(,)?) => {
        $(pub fn $name() -> Hsla { color(stringify!($name)) })*
    };
}

colors!(
    editor_bg,
    panel_bg,
    border,
    border_strong,
    gutter_bg,
    line_number,
    current_line,
    selection,
    caret,
    text,
    ui_text,
    muted,
    hover,
    active_row,
    tab_active,
    success,
    overlay,
    find_match,
    find_current,
    error,
    link,
    accent,
);

pub fn syntax(highlight: Highlight) -> Hsla {
    let key = match highlight {
        Highlight::Keyword => "keyword",
        Highlight::String => "string",
        Highlight::Comment => "comment",
        Highlight::Number => "number",
        Highlight::Function => "function",
        Highlight::Method => "method",
        Highlight::Type => "type",
        Highlight::Variable => "variable",
        Highlight::Property => "property",
        Highlight::Constant => "constant",
        Highlight::Operator => "operator",
        Highlight::Tag => "tag",
        Highlight::Attribute => "attribute",
        Highlight::Namespace => "namespace",
        Highlight::Error => "error",
        Highlight::Warning => "warning",
        Highlight::Info => "info",
    };
    CURRENT
        .read()
        .unwrap()
        .as_ref()
        .and_then(|t| t.syntax.get(key).copied())
        .unwrap_or_else(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_parse_and_resolve() {
        let files = builtin_files();
        assert_eq!(files.len(), BUILTIN.len());
        let dark = files.iter().find(|t| t.name == DEFAULT_THEME).unwrap();
        let base = resolve(dark, None);
        for key in [
            "editor_bg",
            "panel_bg",
            "text",
            "accent",
            "overlay",
            "selection",
        ] {
            assert!(base.colors.contains_key(key), "{key}");
        }
        // A partial user theme inherits the rest from the default.
        let partial: ThemeFile =
            serde_json::from_str(r##"{"name":"Mine","colors":{"accent":"#ff0000"}}"##).unwrap();
        let mine = resolve(&partial, Some(&base));
        assert_eq!(mine.colors["accent"], parse_hex("#ff0000").unwrap());
        assert_eq!(mine.colors["editor_bg"], base.colors["editor_bg"]);
        assert!(parse_hex("#0000008c").unwrap().a < 0.6);
    }
}
