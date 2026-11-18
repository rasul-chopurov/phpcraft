//! Import settings and keybindings from VS Code (and Cursor/VSCodium) and Zed.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::config::{AutoSave, Settings};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    VsCode,
    Zed,
}

impl Source {
    pub const ALL: [Source; 2] = [Source::VsCode, Source::Zed];

    pub fn name(self) -> &'static str {
        match self {
            Source::VsCode => "VS Code",
            Source::Zed => "Zed",
        }
    }

    /// The installed configuration directory, if any.
    pub fn locate(self) -> Option<PathBuf> {
        let config = dirs::config_dir()?;
        match self {
            Source::VsCode => ["Code", "Cursor", "VSCodium", "Code - Insiders"]
                .iter()
                .map(|app| config.join(app).join("User"))
                .find(|p| p.join("settings.json").exists() || p.join("keybindings.json").exists()),
            Source::Zed => {
                let candidates = [dirs::home_dir().map(|h| h.join(".config/zed")), Some(config.join("Zed")), Some(config.join("zed"))];
                candidates.into_iter().flatten().find(|p| p.join("settings.json").exists() || p.join("keymap.json").exists())
            }
        }
    }
}

#[derive(Debug, Default, PartialEq)]
pub struct Report {
    /// Human-readable list of applied settings.
    pub settings: Vec<String>,
    /// `(keys, action)` bindings applied.
    pub bindings: Vec<(String, String)>,
}

/// Removes `//` and `/* */` comments and trailing commas (JSON with comments).
pub fn strip_jsonc(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut in_string = false;
    while let Some(c) = chars.next() {
        if in_string {
            out.push(c);
            if c == '\\' {
                if let Some(next) = chars.next() {
                    out.push(next);
                }
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        match (c, chars.peek()) {
            ('"', _) => {
                in_string = true;
                out.push(c);
            }
            ('/', Some('/')) => {
                for c in chars.by_ref() {
                    if c == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            ('/', Some('*')) => {
                chars.next();
                let mut prev = ' ';
                for c in chars.by_ref() {
                    if prev == '*' && c == '/' {
                        break;
                    }
                    prev = c;
                }
            }
            _ => out.push(c),
        }
    }
    // Trailing commas before } or ].
    let mut cleaned = String::with_capacity(out.len());
    let bytes: Vec<char> = out.chars().collect();
    let mut in_string = false;
    for (i, &c) in bytes.iter().enumerate() {
        if c == '"' && (i == 0 || bytes[i - 1] != '\\') {
            in_string = !in_string;
        }
        if c == ',' && !in_string {
            let next = bytes[i + 1..].iter().find(|c| !c.is_whitespace());
            if matches!(next, Some('}') | Some(']')) {
                continue;
            }
        }
        cleaned.push(c);
    }
    cleaned
}

fn read_jsonc(path: &Path) -> Option<Value> {
    serde_json::from_str(&strip_jsonc(&std::fs::read_to_string(path).ok()?)).ok()
}

fn theme_for(name: &str) -> &'static str {
    let lower = name.to_ascii_lowercase();
    if lower.contains("light") || lower.contains("day") || lower.contains("latte") {
        "PhpCraft Light"
    } else {
        crate::theme::DEFAULT_THEME
    }
}

/// VS Code `cmd+shift+p ctrl+k` -> `cmd-shift-p ctrl-k`.
fn vscode_keys(keys: &str) -> String {
    keys.split_whitespace().map(|stroke| stroke.to_ascii_lowercase().replace('+', "-")).collect::<Vec<_>>().join(" ")
}

const VSCODE_ACTIONS: &[(&str, &str)] = &[
    ("workbench.action.quickOpen", "workspace::GoToFile"),
    ("workbench.action.showAllSymbols", "workspace::GoToSymbol"),
    ("workbench.action.showCommands", "workspace::SearchEverywhere"),
    ("workbench.action.findInFiles", "workspace::FindInFiles"),
    ("workbench.action.replaceInFiles", "workspace::ReplaceInFiles"),
    ("workbench.action.gotoLine", "workspace::GoToLine"),
    ("workbench.action.openRecent", "workspace::RecentFiles"),
    ("workbench.action.closeActiveEditor", "workspace::CloseTab"),
    ("workbench.action.nextEditor", "workspace::NextTab"),
    ("workbench.action.previousEditor", "workspace::PrevTab"),
    ("workbench.action.openSettings", "workspace::OpenSettings"),
    ("workbench.action.openGlobalKeybindings", "workspace::OpenKeymap"),
    ("workbench.action.terminal.toggleTerminal", "workspace::ToggleTerminal"),
    ("workbench.action.terminal.new", "workspace::NewTerminal"),
    ("workbench.view.scm", "workspace::ShowCommit"),
    ("workbench.view.explorer", "workspace::ShowProject"),
    ("workbench.action.debug.run", "workspace::RunLast"),
    ("workbench.action.navigateBack", "workspace::NavigateBack"),
    ("actions.find", "editor::ShowFind"),
    ("editor.action.startFindReplaceAction", "editor::ShowReplace"),
    ("editor.action.revealDefinition", "editor::GoToDeclaration"),
    ("editor.action.commentLine", "editor::ToggleComment"),
    ("editor.action.copyLinesDownAction", "editor::DuplicateLine"),
    ("editor.action.deleteLines", "editor::DeleteLine"),
    ("editor.action.moveLinesUpAction", "editor::MoveLineUp"),
    ("editor.action.moveLinesDownAction", "editor::MoveLineDown"),
    ("editor.action.triggerSuggest", "editor::ShowCompletions"),
    ("editor.action.quickFix", "editor::ShowIntentions"),
    ("editor.action.organizeImports", "editor::OptimizeImports"),
    ("workbench.action.files.save", "editor::Save"),
];

const ZED_ACTIONS: &[(&str, &str)] = &[
    ("file_finder::Toggle", "workspace::GoToFile"),
    ("project_symbols::Toggle", "workspace::GoToSymbol"),
    ("command_palette::Toggle", "workspace::SearchEverywhere"),
    ("pane::DeploySearch", "workspace::FindInFiles"),
    ("workspace::NewSearch", "workspace::FindInFiles"),
    ("go_to_line::Toggle", "workspace::GoToLine"),
    ("tab_switcher::Toggle", "workspace::RecentFiles"),
    ("pane::CloseActiveItem", "workspace::CloseTab"),
    ("pane::ActivateNextItem", "workspace::NextTab"),
    ("pane::ActivatePreviousItem", "workspace::PrevTab"),
    ("zed::OpenSettings", "workspace::OpenSettings"),
    ("zed::OpenKeymap", "workspace::OpenKeymap"),
    ("terminal_panel::ToggleFocus", "workspace::ToggleTerminal"),
    ("workspace::NewTerminal", "workspace::NewTerminal"),
    ("git_panel::ToggleFocus", "workspace::ShowCommit"),
    ("project_panel::ToggleFocus", "workspace::ShowProject"),
    ("pane::GoBack", "workspace::NavigateBack"),
    ("task::Spawn", "workspace::RunPopup"),
    ("task::Rerun", "workspace::RunLast"),
    ("buffer_search::Deploy", "editor::ShowFind"),
    ("buffer_search::DeployReplace", "editor::ShowReplace"),
    ("editor::GoToDefinition", "editor::GoToDeclaration"),
    ("editor::ToggleComments", "editor::ToggleComment"),
    ("editor::DuplicateLineDown", "editor::DuplicateLine"),
    ("editor::DeleteLine", "editor::DeleteLine"),
    ("editor::MoveLineUp", "editor::MoveLineUp"),
    ("editor::MoveLineDown", "editor::MoveLineDown"),
    ("editor::ShowCompletions", "editor::ShowCompletions"),
    ("editor::ToggleCodeActions", "editor::ShowIntentions"),
    ("editor::OrganizeImports", "editor::OptimizeImports"),
    ("workspace::Save", "editor::Save"),
];

fn map_action(table: &[(&str, &str)], action: &str) -> Option<String> {
    table.iter().find(|(from, _)| *from == action).map(|(_, to)| to.to_string())
}

/// Reads settings from `source` at `dir` into `settings`; returns what changed and the
/// bindings to apply (not yet written).
pub fn read(source: Source, dir: &Path, settings: &mut Settings) -> Report {
    let mut report = Report::default();
    let mut note = |text: String| report.settings.push(text);
    match source {
        Source::VsCode => {
            if let Some(v) = read_jsonc(&dir.join("settings.json")) {
                if let Some(font) = v.get("editor.fontFamily").and_then(Value::as_str) {
                    let family = font.split(',').next().unwrap_or("").trim().trim_matches(['\'', '"']).to_string();
                    if !family.is_empty() {
                        note(format!("Editor font: {family}"));
                        settings.editor_font = family;
                    }
                }
                if let Some(size) = v.get("editor.fontSize").and_then(Value::as_f64) {
                    settings.editor_font_size = size as f32;
                    note(format!("Font size: {size}"));
                }
                if let Some(mode) = v.get("files.autoSave").and_then(Value::as_str) {
                    settings.auto_save = match mode {
                        "off" => AutoSave::Off,
                        "afterDelay" => AutoSave::AfterDelay,
                        _ => AutoSave::OnFocusChange,
                    };
                    note(format!("Auto save: {mode}"));
                }
                if let Some(delay) = v.get("files.autoSaveDelay").and_then(Value::as_u64) {
                    settings.auto_save_delay_ms = delay;
                }
                if let Some(theme) = v.get("workbench.colorTheme").and_then(Value::as_str) {
                    settings.theme = theme_for(theme).into();
                    note(format!("Theme: {theme} → {}", settings.theme));
                }
                let shell_key = if cfg!(target_os = "macos") { "osx" } else if cfg!(windows) { "windows" } else { "linux" };
                if let Some(profile) = v.get(format!("terminal.integrated.defaultProfile.{shell_key}")).and_then(Value::as_str)
                    && let Some(path) = v
                        .get(format!("terminal.integrated.profiles.{shell_key}"))
                        .and_then(|p| p.get(profile))
                        .and_then(|p| p.get("path"))
                        .and_then(Value::as_str)
                {
                    settings.terminal_shell = path.to_string();
                    note(format!("Terminal shell: {path}"));
                }
            }
            if let Some(Value::Array(items)) = read_jsonc(&dir.join("keybindings.json")) {
                for item in items {
                    let (Some(key), Some(command)) = (item.get("key").and_then(Value::as_str), item.get("command").and_then(Value::as_str)) else { continue };
                    if let Some(action) = map_action(VSCODE_ACTIONS, command) {
                        report.bindings.push((vscode_keys(key), action));
                    }
                }
            }
        }
        Source::Zed => {
            if let Some(v) = read_jsonc(&dir.join("settings.json")) {
                if let Some(font) = v.get("buffer_font_family").and_then(Value::as_str) {
                    note(format!("Editor font: {font}"));
                    settings.editor_font = font.to_string();
                }
                if let Some(size) = v.get("buffer_font_size").and_then(Value::as_f64) {
                    settings.editor_font_size = size as f32;
                    note(format!("Font size: {size}"));
                }
                let theme = match v.get("theme") {
                    Some(Value::String(name)) => Some(name.clone()),
                    Some(Value::Object(o)) => {
                        let mode = o.get("mode").and_then(Value::as_str).unwrap_or("dark");
                        o.get(if mode == "light" { "light" } else { "dark" }).and_then(Value::as_str).map(str::to_string)
                    }
                    _ => None,
                };
                if let Some(theme) = theme {
                    settings.theme = theme_for(&theme).into();
                    note(format!("Theme: {theme} → {}", settings.theme));
                }
                match v.get("autosave") {
                    Some(Value::String(s)) if s == "off" => settings.auto_save = AutoSave::Off,
                    Some(Value::String(_)) => settings.auto_save = AutoSave::OnFocusChange,
                    Some(Value::Object(o)) => {
                        if let Some(ms) = o.get("after_delay").and_then(|d| d.get("milliseconds")).and_then(Value::as_u64) {
                            settings.auto_save = AutoSave::AfterDelay;
                            settings.auto_save_delay_ms = ms;
                        } else {
                            settings.auto_save = AutoSave::OnFocusChange;
                        }
                    }
                    _ => {}
                }
                if let Some(program) = v.get("terminal").and_then(|t| t.get("shell")).and_then(|s| s.get("program")).and_then(Value::as_str) {
                    settings.terminal_shell = program.to_string();
                    note(format!("Terminal shell: {program}"));
                }
            }
            if let Some(Value::Array(sections)) = read_jsonc(&dir.join("keymap.json")) {
                for section in sections {
                    let Some(Value::Object(bindings)) = section.get("bindings") else { continue };
                    for (keys, action) in bindings {
                        let name = match action {
                            Value::String(name) => name.as_str(),
                            Value::Array(items) => items.first().and_then(Value::as_str).unwrap_or(""),
                            _ => continue,
                        };
                        if let Some(action) = map_action(ZED_ACTIONS, name) {
                            report.bindings.push((keys.clone(), action));
                        }
                    }
                }
            }
        }
    }
    report
}

/// Imports into the saved settings and keymap.json. Returns the report.
pub fn import(source: Source) -> Result<Report, String> {
    let dir = source.locate().ok_or_else(|| format!("{} settings were not found", source.name()))?;
    let mut settings = Settings::load();
    let report = read(source, &dir, &mut settings);
    settings.save();
    for (keys, action) in &report.bindings {
        let context = crate::keymap::default_context(action).flatten();
        crate::keymap::set_binding(action, context.as_deref(), Some(keys)).map_err(|e| e.to_string())?;
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str, files: &[(&str, &str)]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("phpcraft-import-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for (path, content) in files {
            let path = dir.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, content).unwrap();
        }
        dir
    }

    #[test]
    fn jsonc() {
        let text = "{\n // comment\n \"a\": \"http://x\", /* block */ \"b\": [1, 2,],\n}";
        let v: Value = serde_json::from_str(&strip_jsonc(text)).unwrap();
        assert_eq!(v["a"], "http://x");
        assert_eq!(v["b"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn vscode() {
        let dir = temp(
            "vscode",
            &[
                ("settings.json", r#"{ "editor.fontFamily": "'Fira Code', monospace", "editor.fontSize": 15, "files.autoSave": "onFocusChange", "workbench.colorTheme": "Default Light Modern", }"#),
                ("keybindings.json", r#"[ { "key": "cmd+shift+o", "command": "workbench.action.quickOpen" }, { "key": "ctrl+k ctrl+c", "command": "editor.action.commentLine" }, { "key": "f1", "command": "unknown.command" } ]"#),
            ],
        );
        let mut settings = Settings::default();
        let report = read(Source::VsCode, &dir, &mut settings);
        assert_eq!(settings.editor_font, "Fira Code");
        assert_eq!(settings.editor_font_size, 15.);
        assert_eq!(settings.auto_save, AutoSave::OnFocusChange);
        assert_eq!(settings.theme, "PhpCraft Light");
        assert_eq!(
            report.bindings,
            [("cmd-shift-o".to_string(), "workspace::GoToFile".to_string()), ("ctrl-k ctrl-c".to_string(), "editor::ToggleComment".to_string())]
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn zed() {
        let dir = temp(
            "zed",
            &[
                ("settings.json", r#"{ "buffer_font_family": "Zed Mono", "buffer_font_size": 16, "theme": { "mode": "light", "light": "One Light", "dark": "One Dark" }, "autosave": { "after_delay": { "milliseconds": 500 } } }"#),
                ("keymap.json", r#"[ { "context": "Workspace", "bindings": { "cmd-p": "file_finder::Toggle", "cmd-j": ["workspace::SendKeystrokes", "x"], "cmd-/": "editor::ToggleComments" } } ]"#),
            ],
        );
        let mut settings = Settings::default();
        let report = read(Source::Zed, &dir, &mut settings);
        assert_eq!(settings.editor_font, "Zed Mono");
        assert_eq!(settings.theme, "PhpCraft Light");
        assert_eq!((settings.auto_save, settings.auto_save_delay_ms), (AutoSave::AfterDelay, 500));
        assert_eq!(report.bindings.len(), 2);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
