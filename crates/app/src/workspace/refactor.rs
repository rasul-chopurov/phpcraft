//! Rename refactoring (⇧F6 in the editor): renames a class, method, property, constant,
//! function or variable everywhere it is used. Open files change through their editors (one
//! undo step each) and are saved; other files are rewritten on disk. Renaming a class whose
//! file is named after it (PSR-4) renames the file too.

use std::collections::HashMap;
use std::path::PathBuf;

use gpui::{AppContext, Context, Entity, Window};

use super::Workspace;
use crate::dialogs::{FileOp, InputDialog};
use crate::editor_view::EditorView;

impl Workspace {
    /// Asks for the new name of `symbol`.
    pub(crate) fn start_rename(&mut self, symbol: php_analysis::Symbol, window: &mut Window, cx: &mut Context<Self>) {
        let dialog = cx.new(|cx| InputDialog::new(FileOp::RenameSymbol(symbol), window, cx));
        self.show_modal(dialog, window, cx);
    }

    /// Every open editor, in all panes.
    fn open_editors(&self) -> Vec<Entity<EditorView>> {
        let mut editors = self.tabs.clone();
        for pane in self.panes.iter().flatten() {
            editors.extend(pane.tabs.iter().cloned());
        }
        editors
    }

    pub(super) fn rename_symbol(&mut self, symbol: &php_analysis::Symbol, new_name: &str, cx: &mut Context<Self>) -> Result<Option<PathBuf>, String> {
        let new_name = new_name.trim_start_matches('$');
        if new_name.is_empty() || !new_name.chars().all(|c| c.is_alphanumeric() || c == '_') || new_name.starts_with(|c: char| c.is_ascii_digit()) {
            return Err(format!("'{new_name}' is not a valid name"));
        }
        let editors = self.open_editors();
        let open: Vec<(PathBuf, String)> = editors
            .iter()
            .filter_map(|e| {
                let buffer = &e.read(cx).buffer;
                Some((buffer.path()?.to_path_buf(), buffer.text()))
            })
            .collect();
        let found = self.intel.find_usages(symbol, &open);
        if found.is_empty() {
            return Err("No usages found".into());
        }
        let mut by_file: HashMap<PathBuf, Vec<(std::ops::Range<usize>, String)>> = HashMap::new();
        for location in found {
            let text = open.iter().find(|(p, _)| *p == location.path).map(|(_, t)| t.clone())
                .or_else(|| std::fs::read_to_string(&location.path).ok())
                .unwrap_or_default();
            let old = text.get(location.range.clone()).unwrap_or("");
            // `$total` (property declaration, variables) keeps its `$`.
            let replacement = if old.starts_with('$') { format!("${new_name}") } else { new_name.to_string() };
            by_file.entry(location.path).or_default().push((location.range, replacement));
        }
        crate::local_history::record_disk(&by_file.keys().cloned().collect::<Vec<_>>(), "Before Rename");
        for (path, edits) in &by_file {
            match editors.iter().find(|e| e.read(cx).buffer.path() == Some(path.as_path())) {
                Some(editor) => editor.update(cx, |editor, cx| {
                    let caret = editor.buffer.char_to_byte(editor.buffer.selection.head);
                    editor.buffer.apply_edits(edits.clone(), caret);
                    editor.save_now(cx);
                    cx.notify();
                }),
                None => {
                    let mut text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
                    let mut sorted = edits.clone();
                    sorted.sort_by_key(|(r, _)| std::cmp::Reverse(r.start));
                    for (range, replacement) in sorted {
                        text.replace_range(range, &replacement);
                    }
                    std::fs::write(path, &text).map_err(|e| e.to_string())?;
                    self.intel.update_file(path, &text);
                }
            }
        }
        // A PSR-4 class file is named after its class.
        let decl = &symbol.declaration;
        let source = std::fs::read_to_string(&decl.path).unwrap_or_default();
        let before = source.get(..decl.range.start).unwrap_or("").trim_end();
        let is_class = ["class", "interface", "trait", "enum"].iter().any(|k| before.ends_with(k));
        let named_after = decl.path.file_stem().is_some_and(|s| s.to_string_lossy() == symbol.name);
        if is_class && named_after {
            let target = decl.path.with_file_name(format!("{new_name}.php"));
            if !target.exists() {
                std::fs::rename(&decl.path, &target).map_err(|e| e.to_string())?;
                for editor in &editors {
                    editor.update(cx, |editor, _| editor.buffer.rename_path(&decl.path, &target));
                }
                self.intel.remove_file(&decl.path);
                if let Ok(text) = std::fs::read_to_string(&target) {
                    self.intel.update_file(&target, &text);
                }
            }
        }
        Ok(None)
    }
}
