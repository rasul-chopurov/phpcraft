//! Refactor ▸ Introduce Variable (⌘⌥V), Extract Method (⌘⌥M) and Inline Variable (⌘⌥N).
//! The new name is selected at every occurrence afterwards, so typing renames it in place.
//! Where nothing applies, the shortcut keeps its other meaning (⌘⌥M toggles the Markdown
//! preview, ⌘⌥N creates a Git branch).

use gpui::{Context, Window};

use super::{EditorView, ExtractMethod, InlineVariable, IntroduceVariable, ToggleMarkdownPreview};

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
}
