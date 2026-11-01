//! Key bindings: a default table (macOS and Windows/Linux variants, PhpStorm-style) plus the
//! user's `keymap.json` in the config directory, Zed-style:
//!
//! ```json
//! [{ "context": "Editor", "bindings": { "secondary-d": "editor::Copy", "ctrl-space": null } }]
//! ```
//!
//! Binding an action replaces its default keys in that context; `null` removes a default.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::rc::Rc;

use gpui::{App, KeyBinding, KeyBindingContextPredicate};
use serde::{Deserialize, Serialize};

use crate::config;

struct Default {
    mac: &'static str,
    other: &'static str,
    action: &'static str,
    context: Option<&'static str>,
}

const fn both(keys: &'static str, action: &'static str, context: Option<&'static str>) -> Default {
    Default {
        mac: keys,
        other: keys,
        action,
        context,
    }
}

const fn per(
    mac: &'static str,
    other: &'static str,
    action: &'static str,
    context: Option<&'static str>,
) -> Default {
    Default {
        mac,
        other,
        action,
        context,
    }
}

const ED: Option<&str> = Some("Editor");
const PANE: Option<&str> = Some("EditorPane");
const FIND: Option<&str> = Some("FindBar");
const FIELD: Option<&str> = Some("TextField");
const DIALOG: Option<&str> = Some("Dialog");
const LIST: Option<&str> = Some("ListDialog");
const ARTISAN: Option<&str> = Some("ArtisanDialog");
const MENU: Option<&str> = Some("Editor && menu");
const GHOST: Option<&str> = Some("Editor && ghost");
const KEYMAP: Option<&str> = Some("KeymapDialog");

/// Order matters: later bindings win over earlier ones in the same context.
const DEFAULTS: &[Default] = &[
    // Editor movement and editing.
    both("left", "editor::MoveLeft", ED),
    both("right", "editor::MoveRight", ED),
    both("up", "editor::MoveUp", ED),
    both("down", "editor::MoveDown", ED),
    both("shift-left", "editor::SelectLeft", ED),
    both("shift-right", "editor::SelectRight", ED),
    both("shift-up", "editor::SelectUp", ED),
    both("shift-down", "editor::SelectDown", ED),
    both("home", "editor::MoveHome", ED),
    both("end", "editor::MoveEnd", ED),
    per("cmd-left", "home", "editor::MoveHome", ED),
    per("cmd-right", "end", "editor::MoveEnd", ED),
    both("shift-home", "editor::SelectHome", ED),
    both("shift-end", "editor::SelectEnd", ED),
    per("cmd-shift-left", "shift-home", "editor::SelectHome", ED),
    per("cmd-shift-right", "shift-end", "editor::SelectEnd", ED),
    both("pageup", "editor::PageUp", ED),
    both("pagedown", "editor::PageDown", ED),
    per("cmd-up", "ctrl-home", "editor::DocStart", ED),
    per("cmd-down", "ctrl-end", "editor::DocEnd", ED),
    both("backspace", "editor::Backspace", ED),
    both("delete", "editor::Delete", ED),
    both("enter", "editor::Newline", ED),
    both("tab", "editor::Tab", ED),
    both("secondary-a", "editor::SelectAll", ED),
    both("secondary-c", "editor::Copy", ED),
    both("secondary-x", "editor::Cut", ED),
    both("secondary-v", "editor::Paste", ED),
    both("secondary-z", "editor::Undo", ED),
    per("cmd-shift-z", "ctrl-shift-z", "editor::Redo", ED),
    both("secondary-/", "editor::ToggleComment", ED),
    both("secondary-d", "editor::DuplicateLine", ED),
    per("cmd-backspace", "ctrl-y", "editor::DeleteLine", ED),
    both("alt-shift-up", "editor::MoveLineUp", ED),
    both("alt-shift-down", "editor::MoveLineDown", ED),
    per("cmd-alt-a", "ctrl-alt-a", "editor::ToggleBlame", ED),
    // Multiple carets.
    per("ctrl-g", "alt-j", "editor::AddNextOccurrence", ED),
    per("ctrl-shift-g", "alt-shift-j", "editor::UnselectOccurrence", ED),
    per("ctrl-cmd-g", "ctrl-alt-shift-j", "editor::SelectAllOccurrences", ED),
    per("alt-shift-g", "alt-shift-g", "editor::CaretsAtLineEnds", ED),
    per("ctrl-shift-up", "ctrl-alt-shift-up", "editor::CloneCaretAbove", ED),
    per("ctrl-shift-down", "ctrl-alt-shift-down", "editor::CloneCaretBelow", ED),
    both("escape", "editor::ClearCarets", Some("Editor && multicaret")),
    // Text operations.
    both("ctrl-shift-j", "editor::JoinLines", ED),
    both("secondary-shift-u", "editor::ToggleCharCase", ED),
    both("shift-tab", "editor::Unindent", ED),
    per("cmd-shift-m", "ctrl-shift-m", "editor::GoToMatchingBrace", ED),
    both("secondary-shift-v", "editor::PasteFromHistory", ED),
    per("cmd-alt-m", "ctrl-alt-m", "editor::ToggleMarkdownPreview", Some("EditorPane")),
    both("secondary-s", "editor::Save", ED),
    // Code intelligence.
    both("secondary-b", "editor::GoToDeclaration", ED),
    both("ctrl-space", "editor::ShowCompletions", ED),
    both("alt-enter", "editor::ShowIntentions", ED),
    both("ctrl-alt-o", "editor::OptimizeImports", ED),
    // Find / replace.
    both("secondary-f", "editor::ShowFind", PANE),
    per("cmd-r", "ctrl-r", "editor::ShowReplace", PANE),
    per("cmd-g", "f3", "editor::FindNext", PANE),
    both("f3", "editor::FindNext", PANE),
    per("cmd-shift-g", "shift-f3", "editor::FindPrev", PANE),
    both("shift-f3", "editor::FindPrev", PANE),
    both("enter", "editor::FindNext", FIND),
    both("shift-enter", "editor::FindPrev", FIND),
    both("escape", "editor::CloseFind", FIND),
    both("alt-c", "editor::ToggleCase", FIND),
    both("alt-w", "editor::ToggleWord", FIND),
    both("alt-x", "editor::ToggleRegex", FIND),
    both("enter", "editor::ReplaceNext", Some("ReplaceField")),
    both("secondary-alt-enter", "editor::ReplaceAll", FIND),
    // Workspace navigation.
    per("cmd-[", "ctrl-alt-left", "workspace::NavigateBack", None),
    per("cmd-alt-left", "alt-left", "workspace::NavigateBack", None),
    both("secondary-shift-f", "workspace::FindInFiles", None),
    both("secondary-shift-r", "workspace::ReplaceInFiles", None),
    per("cmd-shift-o", "ctrl-shift-n", "workspace::GoToFile", None),
    per("cmd-o", "ctrl-n", "workspace::GoToClass", None),
    per(
        "cmd-alt-o",
        "ctrl-alt-shift-n",
        "workspace::GoToSymbol",
        None,
    ),
    both("secondary-e", "workspace::RecentFiles", None),
    per("cmd-l", "ctrl-g", "workspace::GoToLine", None),
    both("secondary-shift-a", "workspace::SearchEverywhere", None),
    both("secondary-w", "workspace::CloseTab", None),
    per("ctrl-tab", "ctrl-pagedown", "workspace::NextTab", None),
    per("ctrl-shift-tab", "ctrl-pageup", "workspace::PrevTab", None),
    // Laravel.
    per("cmd-n", "alt-insert", "workspace::OpenMake", None),
    both("secondary-shift-,", "workspace::OpenMake", None),
    both("secondary-shift-.", "workspace::GenerateHelperCode", None),
    both("ctrl-shift-r", "workspace::OpenRouteSearch", None),
    both("ctrl-shift-x", "workspace::OpenArtisan", None),
    // Git (PhpStorm: ⌘0 Commit, ⌘9 Git log, ⌘T update, ⇧⌘K push).
    per("cmd-0", "alt-0", "workspace::ShowCommit", None),
    per("cmd-1", "alt-1", "workspace::ShowProject", None),
    per("cmd-9", "alt-9", "workspace::ToggleGitLog", None),
    per("cmd-t", "ctrl-t", "workspace::GitPull", None),
    both("secondary-shift-k", "workspace::GitPush", None),
    both("secondary-enter", "workspace::CommitChanges", None),
    both("alt-shift-h", "workspace::ShowLocalHistory", None),
    both("alt-f12", "workspace::ToggleTerminal", None),
    per("ctrl-cmd-]", "ctrl-alt-pagedown", "phpcraft::NextProject", None),
    per("ctrl-cmd-[", "ctrl-alt-pageup", "phpcraft::PrevProject", None),
    per("cmd-\\", "ctrl-\\", "workspace::SplitRight", None),
    per("cmd-shift-\\", "ctrl-shift-\\", "workspace::UnsplitPane", None),
    per("cmd-alt-]", "ctrl-alt-]", "workspace::FocusNextPane", None),
    per("cmd-7", "alt-7", "workspace::ToggleStructure", None),
    per("cmd-alt-i", "ctrl-alt-i", "workspace::ToggleAgent", None),
    both("secondary-enter", "workspace::SendAgentMessage", Some("AgentInput")),
    both("secondary-enter", "workspace::RunSql", Some("DbConsole")),
    both("secondary-enter", "workspace::RunHttpRequest", Some("HttpFile")),
    both("secondary-enter", "workspace::RunSqlFile", Some("SqlFile")),
    // Project tree: arrows, Enter, speed search (type or ⌘F).
    both("up", "workspace::TreeUp", Some("ProjectTree")),
    both("down", "workspace::TreeDown", Some("ProjectTree")),
    both("left", "workspace::TreeCollapse", Some("ProjectTree")),
    both("right", "workspace::TreeExpand", Some("ProjectTree")),
    both("enter", "workspace::TreeOpen", Some("ProjectTree")),
    both("secondary-f", "workspace::TreeSearch", Some("ProjectTree")),
    both("escape", "workspace::TreeCancel", Some("ProjectTree")),
    both("up", "workspace::TreeUp", Some("TreeSearch")),
    both("down", "workspace::TreeDown", Some("TreeSearch")),
    both("enter", "workspace::TreeOpen", Some("TreeSearch")),
    both("escape", "workspace::TreeCancel", Some("TreeSearch")),
    both("enter", "dialog::Confirm", Some("TerminalRename")),
    // Git menu.
    per("cmd-alt-n", "ctrl-alt-n", "workspace::GitNewBranch", None),
    per("ctrl-v", "alt-`", "workspace::VcsOperations", Some("Workspace && !Terminal")),
    both("escape", "dialog::Cancel", Some("TerminalRename")),
    per("cmd-alt-shift-x", "ctrl-alt-shift-x", "workspace::UploadToServer", None),
    per("cmd-t", "ctrl-shift-t", "workspace::NewTerminal", Some("Terminal")),
    per("cmd-w", "ctrl-shift-w", "workspace::CloseTerminal", Some("Terminal")),
    per("ctrl-r", "shift-f10", "workspace::RunLast", None),
    per("ctrl-alt-r", "alt-shift-f10", "workspace::RunPopup", None),
    both("ctrl-shift-f10", "workspace::RunContext", None),
    per("cmd-f2", "ctrl-f2", "workspace::StopRun", None),
    // Diff tab.
    both("alt-up", "diff::PrevChange", Some("DiffView")),
    both("alt-down", "diff::NextChange", Some("DiffView")),
    per("cmd-shift-[", "alt-left", "diff::PrevFile", Some("DiffView")),
    per("cmd-shift-]", "alt-right", "diff::NextFile", Some("DiffView")),
    both("up", "diff::PrevChange", Some("DiffView")),
    both("down", "diff::NextChange", Some("DiffView")),
    // Preferences.
    both("secondary-,", "workspace::OpenSettings", None),
    both("secondary-k secondary-s", "workspace::OpenKeymap", None),
    both("secondary-k secondary-t", "workspace::OpenThemes", None),
    per("cmd-q", "ctrl-q", "phpcraft::Quit", None),
    // Text fields in dialogs.
    both("backspace", "text_field::Backspace", FIELD),
    both("delete", "text_field::Delete", FIELD),
    both("left", "text_field::Left", FIELD),
    both("right", "text_field::Right", FIELD),
    both("home", "text_field::Home", FIELD),
    both("end", "text_field::End", FIELD),
    per("cmd-left", "home", "text_field::Home", FIELD),
    per("cmd-right", "end", "text_field::End", FIELD),
    both("secondary-a", "text_field::SelectAll", FIELD),
    both("secondary-v", "text_field::Paste", FIELD),
    both("secondary-c", "text_field::Copy", FIELD),
    // Dialogs.
    both("enter", "dialog::Confirm", DIALOG),
    both("enter", "dialog::Confirm", Some("DbCellEdit")),
    both("escape", "dialog::Cancel", Some("DbCellEdit")),
    both("enter", "dialog::Confirm", Some("TableFilter")),
    both("escape", "dialog::Cancel", DIALOG),
    both("tab", "dialog::FocusNext", DIALOG),
    both("shift-tab", "dialog::FocusPrev", DIALOG),
    both("ctrl-[", "dialog::MoveRowUp", DIALOG),
    both("ctrl-]", "dialog::MoveRowDown", DIALOG),
    both("ctrl-backspace", "dialog::DeleteRow", DIALOG),
    both("up", "dialog::SelectPrev", ARTISAN),
    both("down", "dialog::SelectNext", ARTISAN),
    both("tab", "dialog::Complete", ARTISAN),
    both("enter", "dialog::Confirm", LIST),
    both("escape", "dialog::Cancel", LIST),
    both("up", "dialog::SelectPrev", LIST),
    both("down", "dialog::SelectNext", LIST),
    both("tab", "navigate::NextMode", Some("NavigateDialog")),
    both("shift-tab", "navigate::PrevMode", Some("NavigateDialog")),
    both("up", "dialog::SelectPrev", KEYMAP),
    both("down", "dialog::SelectNext", KEYMAP),
    // Last so they win over the editor bindings while a popup is open.
    both("tab", "editor::AcceptGhost", GHOST),
    both("escape", "editor::DismissGhost", GHOST),
    both("up", "editor::MenuUp", MENU),
    both("down", "editor::MenuDown", MENU),
    both("enter", "editor::MenuConfirm", MENU),
    both("tab", "editor::MenuConfirm", MENU),
    both("escape", "editor::MenuCancel", MENU),
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Section {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    pub bindings: BTreeMap<String, Option<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub keys: String,
    pub action: String,
    pub context: Option<String>,
    /// Comes from the user's keymap.json.
    pub user: bool,
}

pub fn user_path() -> PathBuf {
    config::dir().join("keymap.json")
}

pub fn load_user() -> Vec<Section> {
    std::fs::read_to_string(user_path())
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

pub fn save_user(sections: &[Section]) -> std::io::Result<()> {
    std::fs::create_dir_all(config::dir())?;
    std::fs::write(
        user_path(),
        serde_json::to_string_pretty(sections).unwrap_or_default(),
    )
}

fn defaults() -> Vec<Entry> {
    let mac = cfg!(target_os = "macos");
    let mut out: Vec<Entry> = DEFAULTS
        .iter()
        .map(|d| Entry {
            keys: (if mac { d.mac } else { d.other }).to_string(),
            action: d.action.to_string(),
            context: d.context.map(str::to_string),
            user: false,
        })
        .collect();
    out.dedup_by(|a, b| a.keys == b.keys && a.action == b.action && a.context == b.context);
    out
}

/// Defaults with user overrides applied, in binding order.
pub fn effective(user: &[Section]) -> Vec<Entry> {
    let mut entries = defaults();
    let mut added = Vec::new();
    for section in user {
        for (keys, action) in &section.bindings {
            match action {
                // Rebinding an action replaces its default keys in that context.
                Some(action) => {
                    entries.retain(|e| {
                        !(e.user == false && &e.action == action && e.context == section.context)
                    });
                    added.push(Entry {
                        keys: keys.clone(),
                        action: action.clone(),
                        context: section.context.clone(),
                        user: true,
                    });
                }
                None => entries.retain(|e| !(&e.keys == keys && e.context == section.context)),
            }
        }
    }
    // Keep the menu bindings last so popups still win.
    let split = entries
        .iter()
        .position(|e| e.context.as_deref() == MENU)
        .unwrap_or(entries.len());
    let tail = entries.split_off(split);
    entries.extend(added);
    entries.extend(tail);
    entries
}

/// Rebinds everything from the default table and keymap.json. Returns problems found.
pub fn apply(cx: &mut App) -> Vec<String> {
    let mut errors = Vec::new();
    let mut bindings = Vec::new();
    for entry in effective(&load_user()) {
        let action = match cx.build_action(&entry.action, None) {
            Ok(action) => action,
            Err(err) => {
                errors.push(format!("{}: {err}", entry.action));
                continue;
            }
        };
        let predicate = match entry
            .context
            .as_deref()
            .map(KeyBindingContextPredicate::parse)
        {
            Some(Ok(p)) => Some(Rc::new(p)),
            Some(Err(err)) => {
                errors.push(format!("context {:?}: {err}", entry.context));
                continue;
            }
            None => None,
        };
        match KeyBinding::load(
            &entry.keys,
            action,
            predicate,
            false,
            None,
            cx.keyboard_mapper().as_ref(),
        ) {
            Ok(binding) => bindings.push(binding),
            Err(err) => errors.push(format!("{}: {err}", entry.keys)),
        }
    }
    cx.clear_key_bindings();
    cx.bind_keys(bindings);
    for error in &errors {
        eprintln!("keymap: {error}");
    }
    errors
}

/// The context an action is bound in by default (`Some(None)` = global), if it has a default.
pub fn default_context(action: &str) -> Option<Option<String>> {
    DEFAULTS.iter().find(|d| d.action == action).map(|d| d.context.map(str::to_string))
}

/// Sets the keys for an action in a context (replacing earlier user keys for it).
/// `keys: None` removes the user override, restoring the default.
pub fn set_binding(action: &str, context: Option<&str>, keys: Option<&str>) -> std::io::Result<()> {
    let mut sections = load_user();
    for section in &mut sections {
        if section.context.as_deref() == context {
            section.bindings.retain(|_, a| a.as_deref() != Some(action));
        }
    }
    if let Some(keys) = keys {
        match sections
            .iter_mut()
            .find(|s| s.context.as_deref() == context)
        {
            Some(section) => {
                section
                    .bindings
                    .insert(keys.to_string(), Some(action.to_string()));
            }
            None => sections.push(Section {
                context: context.map(str::to_string),
                bindings: BTreeMap::from([(keys.to_string(), Some(action.to_string()))]),
            }),
        }
    }
    sections.retain(|s| !s.bindings.is_empty());
    save_user(&sections)
}

/// `secondary-shift-f` -> `⌘⇧F` on macOS, `Ctrl+Shift+F` elsewhere.
pub fn display(keys: &str) -> String {
    let mac = cfg!(target_os = "macos");
    keys.split_whitespace()
        .map(|stroke| {
            let parts: Vec<&str> = stroke.split('-').collect();
            let (mods, key) = parts.split_at(parts.len().saturating_sub(1));
            let key = key.first().copied().unwrap_or("");
            let key = match key {
                "" => "-".to_string(),
                "enter" => {
                    if mac {
                        "↵".into()
                    } else {
                        "Enter".into()
                    }
                }
                "escape" => "Esc".into(),
                "backspace" => {
                    if mac {
                        "⌫".into()
                    } else {
                        "Backspace".into()
                    }
                }
                "left" => "←".into(),
                "right" => "→".into(),
                "up" => "↑".into(),
                "down" => "↓".into(),
                "space" => "Space".into(),
                "tab" => {
                    if mac {
                        "⇥".into()
                    } else {
                        "Tab".into()
                    }
                }
                k if k.len() == 1 => k.to_ascii_uppercase(),
                k => {
                    let mut c = k.chars();
                    c.next()
                        .map(|f| f.to_uppercase().chain(c).collect())
                        .unwrap_or_default()
                }
            };
            let symbols: Vec<&str> = mods
                .iter()
                .map(|m| match (*m, mac) {
                    ("secondary" | "cmd", true) => "⌘",
                    ("ctrl", true) => "⌃",
                    ("alt", true) => "⌥",
                    ("shift", true) => "⇧",
                    ("secondary" | "ctrl", false) => "Ctrl+",
                    ("alt", false) => "Alt+",
                    ("shift", false) => "Shift+",
                    ("cmd", false) => "Win+",
                    _ => "",
                })
                .collect();
            if mac {
                // macOS order: ⌃⌥⇧⌘
                let order = ["⌃", "⌥", "⇧", "⌘"];
                let mut sorted: Vec<&str> = order
                    .iter()
                    .copied()
                    .filter(|o| symbols.contains(o))
                    .collect();
                sorted.dedup();
                format!("{}{key}", sorted.concat())
            } else {
                format!("{}{key}", symbols.concat())
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// `editor::GoToDeclaration` -> `Go To Declaration`.
pub fn humanize(action: &str) -> String {
    let name = action.rsplit("::").next().unwrap_or(action);
    let mut out = String::new();
    for (i, c) in name.chars().enumerate() {
        if c.is_uppercase() && i > 0 {
            out.push(' ');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overrides_and_display() {
        let user: Vec<Section> = serde_json::from_str(
            r#"[{"context":"Editor","bindings":{"secondary-d":"editor::GoToDeclaration","ctrl-space":null}}]"#,
        )
        .unwrap();
        let entries = effective(&user);
        let goto: Vec<_> = entries
            .iter()
            .filter(|e| e.action == "editor::GoToDeclaration")
            .collect();
        assert_eq!(goto.len(), 1);
        assert_eq!(goto[0].keys, "secondary-d");
        assert!(goto[0].user);
        assert!(
            !entries
                .iter()
                .any(|e| e.keys == "ctrl-space" && e.context.as_deref() == Some("Editor"))
        );
        assert_eq!(
            entries.last().unwrap().context.as_deref(),
            Some("Editor && menu")
        );
        assert_eq!(humanize("editor::GoToDeclaration"), "Go To Declaration");
        if cfg!(target_os = "macos") {
            assert_eq!(display("secondary-shift-f"), "⇧⌘F");
            assert_eq!(display("secondary-k secondary-s"), "⌘K ⌘S");
        } else {
            assert_eq!(display("secondary-shift-f"), "Ctrl+Shift+F");
        }
    }
}
