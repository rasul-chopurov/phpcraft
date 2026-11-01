//! Refactor ▸ Introduce Variable (⌘⌥V), Constant (⌘⌥C), Field (⌘⌥F), Extract Method (⌘⌥M),
//! Inline Variable (⌘⌥N) and Safe Delete (⌘⌦).
//! The new name is selected at every occurrence afterwards, so typing renames it in place.
//! Where nothing applies, the shortcut keeps its other meaning (⌘⌥M toggles the Markdown
//! preview, ⌘⌥N creates a Git branch).

use gpui::{Context, Window};

use super::{EditorView, ExtractMethod, InlineVariable, IntroduceConstant, IntroduceField, IntroduceVariable, SafeDelete, ToggleMarkdownPreview};
use super::info::Info;

impl EditorView {
    fn selection_bytes(&self) -> std::ops::Range<usize> {
        let r = self.buffer.selection.range();
        self.buffer.char_to_byte(r.start)..self.buffer.char_to_byte(r.end)
    }

    /// Applies refactoring edits as one undo step, then selects every occurrence of `name`.
    fn apply_refactoring(&mut self, edits: Vec<php_analysis::TextEdit>, name: Option<&str>, cx: &mut Context<Self>) {
        let caret = self.caret_byte();
        self.buffer.apply_edits(edits.into_iter().map(|e| (e.range, e.text)).collect(), caret);
        if let Some(name) = name
            && let Some(at) = self.buffer.text().find(name)
        {
            let start = self.buffer.byte_to_char(at);
            self.buffer.move_to(start + name.chars().count() / 2, false);
            self.buffer.select_all_occurrences();
        }
        self.changed(cx);
    }

    pub(super) fn introduce_variable(&mut self, _: &IntroduceVariable, _: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.path().filter(|_| self.buffer.is_php()) else { return };
        let selection = self.selection_bytes();
        if let Some((edits, name)) = self.intel.with_doc(&path, &self.buffer.text(), |doc, _| php_analysis::introduce_variable(doc, selection)).flatten() {
            self.apply_refactoring(edits, Some(&name), cx);
        }
    }

    pub(super) fn extract_method(&mut self, _: &ExtractMethod, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_markdown() {
            return self.toggle_markdown_preview(&ToggleMarkdownPreview, window, cx);
        }
        let Some(path) = self.path().filter(|_| self.buffer.is_php() && !self.buffer.selection.is_empty()) else { return };
        let selection = self.selection_bytes();
        let name = "extracted";
        if let Some(edits) = self.intel.with_doc(&path, &self.buffer.text(), |doc, _| php_analysis::extract_method(doc, selection, name)).flatten() {
            self.apply_refactoring(edits, Some(name), cx);
        }
    }

    pub(super) fn inline_variable(&mut self, _: &InlineVariable, window: &mut Window, cx: &mut Context<Self>) {
        let at = self.caret_byte();
        let edits = self.path().filter(|_| self.buffer.is_php()).and_then(|path| self.intel.with_doc(&path, &self.buffer.text(), |doc, _| php_analysis::inline_variable(doc, at)).flatten());
        match edits {
            Some(edits) => self.apply_refactoring(edits, None, cx),
            None => {
                if let Ok(action) = cx.build_action("workspace::GitNewBranch", None) {
                    window.dispatch_action(action, cx);
                }
            }
        }
    }

    pub(super) fn introduce_constant(&mut self, _: &IntroduceConstant, _: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.path().filter(|_| self.buffer.is_php()) else { return };
        let selection = self.selection_bytes();
        if let Some((edits, name)) = self.intel.with_doc(&path, &self.buffer.text(), |doc, _| php_analysis::introduce_constant(doc, selection)).flatten() {
            self.apply_refactoring(edits, Some(&name), cx);
        }
    }

    pub(super) fn introduce_field(&mut self, _: &IntroduceField, _: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.path().filter(|_| self.buffer.is_php()) else { return };
        let selection = self.selection_bytes();
        if let Some((edits, name)) = self.intel.with_doc(&path, &self.buffer.text(), |doc, _| php_analysis::introduce_field(doc, selection)).flatten() {
            self.apply_refactoring(edits, Some(&name), cx);
        }
    }

    /// Safe Delete: removes the declaration at the caret when nothing uses it; otherwise
    /// lists the usages and leaves the code alone.
    pub(super) fn safe_delete(&mut self, _: &SafeDelete, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.path().filter(|_| self.buffer.is_php()) else { return };
        let text = self.buffer.text();
        let Some(symbol) = self.intel.symbol_at(&path, &text, self.caret_byte()) else { return };
        if symbol.declaration.path != path {
            self.info = Some(Info::Message(format!("'{}' is declared in another file: open it there to delete it", symbol.name)));
            return cx.notify();
        }
        self.save_now(cx);
        let intel = self.intel.clone();
        let workspace = self.workspace.clone();
        cx.spawn_in(window, async move |this, cx| {
            let declaration = symbol.declaration.clone();
            let search = symbol.clone();
            let usages: Vec<php_index::Location> = cx
                .background_executor()
                .spawn(async move { intel.find_usages(&search, &[]).into_iter().filter(|l| !(l.path == declaration.path && l.range == declaration.range)).collect() })
                .await;
            if !usages.is_empty() {
                let count = usages.len();
                workspace.update_in(cx, |ws, window, cx| ws.show_usages(&symbol.name, usages, window, cx)).ok();
                this.update(cx, |this, cx| {
                    this.info = Some(Info::Message(format!("'{}' has {count} usage(s): not deleted", symbol.name)));
                    cx.notify();
                })
                .ok();
                return;
            }
            this.update_in(cx, |this, window, cx| {
                let text = this.buffer.text();
                let range = symbol.declaration.range.clone();
                // A class alone in its own file: the file goes to the Trash.
                let stem_matches = path.file_stem().is_some_and(|s| s.to_string_lossy() == symbol.name);
                let only_class = this.intel.with_doc(&path, &text, |doc, _| doc.symbols.classes.len() == 1 && doc.symbols.functions.is_empty()).unwrap_or(false);
                let is_class = this.intel.with_doc(&path, &text, |doc, _| doc.symbols.classes.iter().any(|c| c.range == range)).unwrap_or(false);
                if is_class && stem_matches && only_class {
                    let path = path.clone();
                    this.workspace.update(cx, |ws, cx| ws.delete_file(&path, window, cx)).ok();
                    return;
                }
                if let Some(Some(edit)) = this.intel.with_doc(&path, &text, |doc, _| php_analysis::delete_declaration(doc, range)) {
                    let caret = edit.range.start;
                    this.buffer.apply_edits(vec![(edit.range, edit.text)], caret);
                    this.changed(cx);
                }
            })
            .ok();
        })
        .detach();
    }
}
