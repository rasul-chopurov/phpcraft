//! Rename refactoring (⇧F6 in the editor): renames a class, method, property, constant,
//! function or variable everywhere it is used. Open files change through their editors (one
//! undo step each) and are saved; other files are rewritten on disk. Renaming a class whose
//! file is named after it (PSR-4) renames the file too.

use std::collections::HashMap;
use std::path::PathBuf;

use gpui::{AppContext, Context, Entity, Window};

use super::Workspace;
use crate::dialogs::{FileOp, InputDialog};
use crate::i18n::{tr, trf};
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

    /// Applies byte-range edits to open editors (one undo step each, saved) or to files on disk.
    fn apply_file_edits(
        &mut self,
        by_file: &HashMap<PathBuf, Vec<(std::ops::Range<usize>, String)>>,
        editors: &[Entity<EditorView>],
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        for (path, edits) in by_file {
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
        Ok(())
    }

    /// Refactor ▸ Move Class (F6): asks for the namespace the class moves to.
    pub(crate) fn start_move_class(&mut self, class: php_analysis::Moving, window: &mut Window, cx: &mut Context<Self>) {
        let dialog = cx.new(|cx| InputDialog::new(FileOp::MoveClass(class), window, cx));
        self.show_modal(dialog, window, cx);
    }

    /// Moves the declaration into a PSR-4 file of the new namespace, rewrites the project's
    /// `use` statements and imports what the moved body needs there.
    pub(super) fn move_class(&mut self, class: &php_analysis::Moving, namespace: &str, cx: &mut Context<Self>) -> Result<Option<PathBuf>, String> {
        let namespace = namespace.trim().trim_matches('\\').to_string();
        let segments: Vec<&str> = namespace.split('\\').filter(|s| !s.is_empty()).collect();
        let bad = segments.iter().any(|s| !s.chars().all(|c| c.is_alphanumeric() || c == '_') || s.starts_with(|c: char| c.is_ascii_digit()));
        if bad {
            return Err(format!("'{namespace}' is not a namespace"));
        }
        let new_fqn = if namespace.is_empty() { class.name.clone() } else { format!("{namespace}\\{}", class.name) };
        if new_fqn.eq_ignore_ascii_case(&class.fqn) {
            return Err("The class is already in that namespace".into());
        }
        let dir = crate::templates::dir_for_namespace(&namespace, self.project.root())
            .or_else(|| class.path.parent().map(|p| p.to_path_buf()))
            .ok_or("No folder for that namespace")?;
        let target = dir.join(format!("{}.php", class.name));
        if target == class.path {
            return Err("The class already lives in that file".into());
        }
        if target.exists() {
            return Err(format!("{} already exists", target.display()));
        }
        let editors = self.open_editors();
        let open: Vec<(PathBuf, String)> = editors
            .iter()
            .filter_map(|e| {
                let buffer = &e.read(cx).buffer;
                Some((buffer.path()?.to_path_buf(), buffer.text()))
            })
            .collect();
        let text_of = |path: &std::path::Path| {
            open.iter()
                .find(|(p, _)| p == path)
                .map(|(_, t)| t.clone())
                .or_else(|| std::fs::read_to_string(path).ok())
        };
        let source = text_of(&class.path).ok_or_else(|| format!("{} is gone", class.path.display()))?;
        // A trait or interface declared next to the class and used only by it travels along.
        let companions = self.intel.moved_companions(&class.path, &source, class);
        let new_source = self
            .intel
            .moved_class_source(&class.path, &source, class, &companions, &namespace)
            .ok_or("Could not parse the class")?;
        let mut moves: Vec<(&php_analysis::Moving, String)> = vec![(class, new_fqn.clone())];
        for companion in &companions {
            let fqn = if namespace.is_empty() { companion.name.clone() } else { format!("{namespace}\\{}", companion.name) };
            moves.push((companion, fqn));
        }
        let symbol = php_analysis::Symbol {
            name: class.name.clone(),
            declaration: php_index::Location {
                path: class.path.clone(),
                range: class.name_range.clone(),
            },
            local: false,
        };
        let mut files: Vec<PathBuf> = self.intel.find_usages(&symbol, &open).into_iter().map(|l| l.path).collect();
        // References to a companion come along with the references to the class.
        let mut companions_symbols = Vec::new();
        for companion in &companions {
            companions_symbols.push(php_analysis::Symbol {
                name: companion.name.clone(),
                declaration: php_index::Location { path: companion.path.clone(), range: companion.name_range.clone() },
                local: false,
            });
        }
        for companion_symbol in &companions_symbols {
            files.extend(self.intel.find_usages(companion_symbol, &open).into_iter().map(|l| l.path));
        }
        files.push(class.path.clone());
        files.sort();
        files.dedup();
        let mut by_file: HashMap<PathBuf, Vec<(std::ops::Range<usize>, String)>> = HashMap::new();
        for path in files {
            let Some(text) = text_of(&path) else { continue };
            let mut edits: Vec<(std::ops::Range<usize>, String)> = Vec::new();
            for (moved, fqn) in &moves {
                edits.extend(self.intel.move_class_edits(&path, &text, moved, fqn).into_iter().map(|e| (e.range, e.text)));
            }
            if path == class.path {
                for (moved, _) in &moves {
                    if let Some(removal) = self.intel.remove_moved_class(&path, &text, moved) {
                        edits.push((removal.range, removal.text));
                    }
                }
            }
            if !edits.is_empty() {
                by_file.insert(path, edits);
            }
        }
        let mut touched: Vec<PathBuf> = by_file.keys().cloned().collect();
        touched.push(target.clone());
        crate::local_history::record_disk(&touched, "Before Move Class");
        self.apply_file_edits(&by_file, &editors, cx)?;
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::write(&target, &new_source).map_err(|e| e.to_string())?;
        self.intel.update_file(&target, &new_source);
        // What changed, so the move is easy to review (Local History keeps the previous text).
        let root = self.project.root().to_path_buf();
        let mut listed: Vec<String> = by_file.keys().map(|path| path.strip_prefix(&root).unwrap_or(path).display().to_string()).collect();
        listed.sort();
        let moved = if companions.is_empty() {
            class.fqn.clone()
        } else {
            let names: Vec<String> = std::iter::once(class.name.clone()).chain(companions.iter().map(|c| c.name.clone())).collect();
            format!("{} ({})", class.fqn, names.join(", "))
        };
        self.output = Some(super::tools::Output {
            title: tr("Move Class").to_string(),
            text: trf("{} → {}\nFiles updated: {}\n{}", &[&moved, &new_fqn, &listed.len().to_string(), &listed.join("\n")]),
            running: false,
            ok: true,
        });
        Ok(Some(target))
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

    /// Renaming or moving a JS / TS module rewrites the import specifiers that point at it, in the
    /// style each one already uses. Open files change through their editors, the rest on disk.
    pub(super) fn retarget_js_imports(&mut self, old: &std::path::Path, new: &std::path::Path, cx: &mut Context<Self>) {
        if !js_analysis::is_js(old) && !js_analysis::is_js(new) {
            return;
        }
        let at = js_analysis::Resolve::load(self.project.root());
        let load = |path: &std::path::Path| std::fs::read_to_string(path).ok();
        let editors = self.open_editors();
        let files: Vec<PathBuf> = search::list_files(self.project.root(), false)
            .into_iter()
            .filter(|path| js_analysis::is_js(path))
            .collect();
        for path in files {
            let open = editors.iter().find(|editor| editor.read(cx).buffer.path() == Some(path.as_path())).cloned();
            let text = match &open {
                Some(editor) => editor.read(cx).buffer.text(),
                None => match std::fs::read_to_string(&path) {
                    Ok(text) => text,
                    Err(_) => continue,
                },
            };
            let Some(doc) = js_analysis::parse(&path, &text) else { continue };
            let mut edits = js_analysis::retarget(&doc, &at, old, new, &load);
            if edits.is_empty() {
                continue;
            }
            edits.sort_by_key(|(range, _)| std::cmp::Reverse(range.start));
            match open {
                Some(editor) => editor.update(cx, |editor, cx| {
                    let caret = editor.buffer.char_to_byte(editor.buffer.selection.head);
                    editor.buffer.apply_edits(edits, caret);
                    editor.save_now(cx);
                    cx.notify();
                }),
                None => {
                    let mut text = text;
                    for (range, replacement) in edits {
                        text.replace_range(range, &replacement);
                    }
                    let _ = std::fs::write(&path, &text);
                }
            }
        }
    }

    /// Refactor ▸ Extract Blade Component / Extract View: asks for the name.
    pub(crate) fn start_extract_blade(&mut self, component: bool, path: PathBuf, range: std::ops::Range<usize>, text: String, window: &mut Window, cx: &mut Context<Self>) {
        let dialog = cx.new(|cx| InputDialog::new(FileOp::ExtractBlade { component, path, range, text }, window, cx));
        self.show_modal(dialog, window, cx);
    }

    /// Writes the extracted view and puts its `<x-…>` / `@include` in place of the selection.
    pub(super) fn extract_blade(&mut self, component: bool, path: &std::path::Path, range: std::ops::Range<usize>, text: &str, name: &str, cx: &mut Context<Self>) -> Result<Option<PathBuf>, String> {
        let name = name.trim().trim_end_matches(".blade.php");
        if name.is_empty() || !name.chars().all(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.')) {
            return Err(format!("'{name}' is not a view name"));
        }
        let root = self.intel.laravel_root().unwrap_or_else(|| self.project.root().to_path_buf());
        let target = laravel::blade::extracted_path(&root.join("resources/views"), name, component);
        if target.exists() {
            return Err(format!("{} already exists", target.display()));
        }
        let (content, replacement) = laravel::blade::extract(text, name, component);
        std::fs::create_dir_all(target.parent().unwrap_or(&root)).map_err(|e| e.to_string())?;
        std::fs::write(&target, content).map_err(|e| e.to_string())?;
        // Keep the selection's own indentation in front of the replacement.
        let indent: String = text.chars().take_while(|c| *c == ' ' || *c == '\t').collect();
        let replacement = format!("{indent}{replacement}");
        let Some(editor) = self.open_editors().into_iter().find(|e| e.read(cx).buffer.path() == Some(path)) else {
            return Err("the file is no longer open".into());
        };
        editor.update(cx, |editor, cx| {
            let caret = range.start;
            editor.buffer.apply_edits(vec![(range, replacement)], caret);
            cx.notify();
        });
        Ok(None)
    }
}
