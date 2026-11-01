//! Import settings and keybindings from VS Code (and Cursor/VSCodium), PhpStorm and Zed.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::config::{AutoSave, Settings};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    VsCode,
    PhpStorm,
    Zed,
}

impl Source {
    pub const ALL: [Source; 3] = [Source::PhpStorm, Source::VsCode, Source::Zed];

    pub fn name(self) -> &'static str {
        match self {
            Source::VsCode => "VS Code",
            Source::PhpStorm => "PhpStorm",
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
            Source::PhpStorm => newest_jetbrains(&config.join("JetBrains"), "PhpStorm"),
            Source::Zed => {
                let candidates = [dirs::home_dir().map(|h| h.join(".config/zed")), Some(config.join("Zed")), Some(config.join("zed"))];
                candidates.into_iter().flatten().find(|p| p.join("settings.json").exists() || p.join("keymap.json").exists())
            }
        }
    }
}

/// `PhpStorm2026.2` over `PhpStorm2025.3`.
fn newest_jetbrains(dir: &Path, product: &str) -> Option<PathBuf> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir() && p.file_name().is_some_and(|n| n.to_string_lossy().starts_with(product)))
        .collect();
    found.sort();
    found.pop()
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
    if lower.contains("darcula") {
        "Darcula"
    } else if lower.contains("light") || lower.contains("day") || lower.contains("latte") {
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

const PHPSTORM_ACTIONS: &[(&str, &str)] = &[
    ("GotoFile", "workspace::GoToFile"),
    ("GotoClass", "workspace::GoToClass"),
    ("GotoSymbol", "workspace::GoToSymbol"),
    ("SearchEverywhere", "workspace::SearchEverywhere"),
    ("FindInPath", "workspace::FindInFiles"),
    ("ReplaceInPath", "workspace::ReplaceInFiles"),
    ("GotoLine", "workspace::GoToLine"),
    ("RecentFiles", "workspace::RecentFiles"),
    ("CloseContent", "workspace::CloseTab"),
    ("NextTab", "workspace::NextTab"),
    ("PreviousTab", "workspace::PrevTab"),
    ("ShowSettings", "workspace::OpenSettings"),
    ("ActivateTerminalToolWindow", "workspace::ToggleTerminal"),
    ("ActivateCommitToolWindow", "workspace::ShowCommit"),
    ("ActivateProjectToolWindow", "workspace::ShowProject"),
    ("ActivateVersionControlToolWindow", "workspace::ToggleGitLog"),
    ("CheckinProject", "workspace::CommitChanges"),
    ("Vcs.UpdateProject", "workspace::GitPull"),
    ("Vcs.Push", "workspace::GitPush"),
    ("LocalHistory.ShowHistory", "workspace::ShowLocalHistory"),
    ("Back", "workspace::NavigateBack"),
    ("Run", "workspace::RunLast"),
    ("ChooseRunConfiguration", "workspace::RunPopup"),
    ("RunClass", "workspace::RunContext"),
    ("Stop", "workspace::StopRun"),
    ("Find", "editor::ShowFind"),
    ("Replace", "editor::ShowReplace"),
    ("GotoDeclaration", "editor::GoToDeclaration"),
    ("CommentByLineComment", "editor::ToggleComment"),
    ("EditorDuplicate", "editor::DuplicateLine"),
    ("EditorDeleteLine", "editor::DeleteLine"),
    ("MoveLineUp", "editor::MoveLineUp"),
    ("MoveLineDown", "editor::MoveLineDown"),
    ("CodeCompletion", "editor::ShowCompletions"),
    ("ShowIntentionActions", "editor::ShowIntentions"),
    ("OptimizeImports", "editor::OptimizeImports"),
    ("SaveAll", "workspace::SaveAll"),
    ("Annotate", "editor::ToggleBlame"),
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

/// PhpStorm keystroke `meta shift O` -> `cmd-shift-o`.
fn jetbrains_stroke(stroke: &str) -> Option<String> {
    let mut parts = Vec::new();
    for word in stroke.split_whitespace() {
        let part = match word {
            "meta" => if cfg!(target_os = "macos") { "cmd" } else { "super" }.to_string(),
            "control" | "ctrl" => "ctrl".into(),
            "alt" => "alt".into(),
            "shift" => "shift".into(),
            "altGraph" => return None,
            key => match key {
                "ENTER" => "enter".into(),
                "BACK_SPACE" => "backspace".into(),
                "DELETE" => "delete".into(),
                "ESCAPE" => "escape".into(),
                "TAB" => "tab".into(),
                "SPACE" => "space".into(),
                "UP" | "DOWN" | "LEFT" | "RIGHT" | "HOME" | "END" => key.to_ascii_lowercase(),
                "PAGE_UP" => "pageup".into(),
                "PAGE_DOWN" => "pagedown".into(),
                "SLASH" | "DIVIDE" => "/".into(),
                "BACK_SLASH" => "\\".into(),
                "OPEN_BRACKET" => "[".into(),
                "CLOSE_BRACKET" => "]".into(),
                "COMMA" => ",".into(),
                "PERIOD" => ".".into(),
                "MINUS" | "SUBTRACT" => "-".into(),
                "EQUALS" | "ADD" => "=".into(),
                "SEMICOLON" => ";".into(),
                "QUOTE" => "'".into(),
                "BACK_QUOTE" => "`".into(),
                k if k.len() == 1 || (k.starts_with('F') && k[1..].parse::<u8>().is_ok()) => k.to_ascii_lowercase(),
                _ => return None,
            },
        };
        parts.push(part);
    }
    (!parts.is_empty()).then(|| parts.join("-"))
}

/// Attribute `name="…"` from one XML tag.
fn attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let at = tag.find(&format!("{name}=\""))? + name.len() + 2;
    let end = tag[at..].find('"')?;
    Some(&tag[at..at + end])
}

/// `<option name="X" value="Y"/>` values in an XML file.
fn xml_option(text: &str, name: &str) -> Option<String> {
    text.split('<').find(|tag| tag.starts_with("option") && attr(tag, "name") == Some(name)).and_then(|tag| attr(tag, "value")).map(str::to_string)
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
        Source::PhpStorm => {
            let options = dir.join("options");
            if let Ok(text) = std::fs::read_to_string(options.join("editor-font.xml")) {
                if let Some(font) = xml_option(&text, "FONT_FAMILY") {
                    note(format!("Editor font: {font}"));
                    settings.editor_font = font;
                }
                if let Some(size) = xml_option(&text, "FONT_SIZE_2D").or_else(|| xml_option(&text, "FONT_SIZE")).and_then(|s| s.parse::<f32>().ok()) {
                    settings.editor_font_size = size;
                    note(format!("Font size: {size}"));
                }
            }
            let scheme = std::fs::read_to_string(options.join("colors.scheme.xml"))
                .ok()
                .and_then(|t| t.split('<').find(|tag| tag.starts_with("global_color_scheme")).and_then(|tag| attr(tag, "name")).map(str::to_string))
                .or_else(|| std::fs::read_to_string(options.join("laf.xml")).ok().and_then(|t| t.split('<').find_map(|tag| attr(tag, "themeId").map(str::to_string))));
            if let Some(scheme) = scheme {
                settings.theme = theme_for(&scheme).into();
                note(format!("Theme: {scheme} → {}", settings.theme));
            }
            if let Ok(text) = std::fs::read_to_string(options.join("general.xml")) {
                if xml_option(&text, "autoSaveIfInactive").as_deref() == Some("true") {
                    settings.auto_save = AutoSave::AfterDelay;
                    if let Some(secs) = xml_option(&text, "inactiveTimeout").and_then(|s| s.parse::<u64>().ok()) {
                        settings.auto_save_delay_ms = secs * 1000;
                    }
                    note("Auto save: when idle".into());
                }
            }
            // The active custom keymap holds the user's changes to its parent keymap.
            let active = std::fs::read_to_string(options.join("keymap.xml"))
                .ok()
                .and_then(|t| t.split('<').find(|tag| tag.starts_with("active_keymap")).and_then(|tag| attr(tag, "name")).map(str::to_string));
            if let Some(name) = active
                && let Ok(text) = std::fs::read_to_string(dir.join("keymaps").join(format!("{name}.xml")))
            {
                let mut action: Option<String> = None;
                for tag in text.split('<') {
                    if tag.starts_with("action ") {
                        action = attr(tag, "id").and_then(|id| map_action(PHPSTORM_ACTIONS, id));
                    } else if tag.starts_with("keyboard-shortcut")
                        && let Some(action) = &action
                    {
                        let first = attr(tag, "first-keystroke").and_then(jetbrains_stroke);
                        let second = attr(tag, "second-keystroke").map(jetbrains_stroke);
                        if let Some(first) = first {
                            let keys = match second {
                                Some(Some(second)) => format!("{first} {second}"),
                                Some(None) => continue,
                                None => first,
                            };
                            report.bindings.push((keys, action.clone()));
                        }
                    }
                }
                note(format!("Keymap: {name}"));
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
    fn phpstorm() {
        let dir = temp(
            "phpstorm",
            &[
                ("options/editor-font.xml", r#"<application><component name="DefaultFont"><option name="FONT_SIZE" value="14" /><option name="FONT_FAMILY" value="JetBrains Mono" /></component></application>"#),
                ("options/colors.scheme.xml", r#"<application><component name="EditorColorsManagerImpl"><global_color_scheme name="Darcula" /></component></application>"#),
                ("options/keymap.xml", r#"<application><component name="KeymapManager"><active_keymap name="macOS copy" /></component></application>"#),
                ("keymaps/macOS copy.xml", r#"<keymap version="1" name="macOS copy" parent="Mac OS X 10.5+"><action id="GotoFile"><keyboard-shortcut first-keystroke="meta shift O" /></action><action id="CommentByLineComment"><keyboard-shortcut first-keystroke="meta K" second-keystroke="meta SLASH" /></action></keymap>"#),
            ],
        );
        let mut settings = Settings::default();
        let report = read(Source::PhpStorm, &dir, &mut settings);
        assert_eq!(settings.editor_font, "JetBrains Mono");
        assert_eq!(settings.editor_font_size, 14.);
        assert_eq!(settings.theme, "Darcula");
        let cmd = if cfg!(target_os = "macos") { "cmd" } else { "super" };
        assert_eq!(
            report.bindings,
            [
                (format!("{cmd}-shift-o"), "workspace::GoToFile".to_string()),
                (format!("{cmd}-k {cmd}-/"), "editor::ToggleComment".to_string())
            ]
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
