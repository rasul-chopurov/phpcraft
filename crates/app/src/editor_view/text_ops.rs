//! Text operations: Join Lines, Sort / Reverse Lines, Toggle Case, Unindent, Move Caret to
//! Matching Brace and Paste from History (the last 20 copies, shared by all editors).

use std::sync::Mutex;

use gpui::{ClipboardItem, Context, Window};

use super::intel_ui::{MenuAction, MenuItem};
use super::{
    EditorView, GoToMatchingBrace, JoinLines, PasteFromHistory, ReverseLines, SortLines, ToggleCharCase,
    Unindent,
};

const HISTORY_LEN: usize = 20;

/// A copied fragment plus the classes it uses, so pasting it elsewhere can add the `use`s.
#[derive(Clone)]
pub(super) struct Clip {
    pub text: String,
    pub imports: Vec<String>,
}

static CLIPBOARD_HISTORY: Mutex<Vec<Clip>> = Mutex::new(Vec::new());

/// Remembers copied text for Paste from History (newest first, no duplicates).
pub(super) fn remember_copy(text: &str, imports: Vec<String>) {
    if text.is_empty() {
        return;
    }
    let mut history = CLIPBOARD_HISTORY.lock().unwrap();
    history.retain(|clip| clip.text != text);
    history.insert(0, Clip { text: text.to_string(), imports });
    history.truncate(HISTORY_LEN);
}

/// The class imports of the last copy of this exact text (empty for text from elsewhere).
fn remembered_imports(text: &str) -> Vec<String> {
    CLIPBOARD_HISTORY
        .lock()
        .unwrap()
        .iter()
        .find(|clip| clip.text == text)
        .map(|clip| clip.imports.clone())
        .unwrap_or_default()
}

impl EditorView {
    pub(super) fn join_lines(&mut self, _: &JoinLines, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.join_lines();
        self.moved(cx);
    }

    pub(super) fn sort_lines(&mut self, _: &SortLines, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.reorder_lines(false);
        self.moved(cx);
    }

    pub(super) fn reverse_lines(&mut self, _: &ReverseLines, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.reorder_lines(true);
        self.moved(cx);
    }

    pub(super) fn toggle_char_case(&mut self, _: &ToggleCharCase, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.toggle_case();
        self.moved(cx);
    }

    pub(super) fn unindent(&mut self, _: &Unindent, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.indent_lines(-1);
        self.moved(cx);
    }

    pub(super) fn go_to_matching_brace(&mut self, _: &GoToMatchingBrace, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.go_to_matching_bracket();
        self.moved(cx);
    }

    pub(super) fn paste_from_history(&mut self, _: &PasteFromHistory, _: &mut Window, cx: &mut Context<Self>) {
        let history = CLIPBOARD_HISTORY.lock().unwrap().clone();
        let items = history
            .into_iter()
            .map(|clip| {
                let first = clip.text.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
                let label: String = first.chars().take(60).collect();
                let lines = clip.text.lines().count();
                let detail = if lines > 1 { format!("{lines} lines") } else { String::new() };
                MenuItem::new(label, detail, "", MenuAction::Paste(clip.text))
            })
            .collect();
        self.show_menu(items, false, cx);
    }

    /// Replaces the whole buffer in one undo step (the merge window's Apply & Close).
    pub(crate) fn replace_text(&mut self, text: &str, cx: &mut Context<Self>) {
        if self.buffer.text() == text {
            return;
        }
        let len = self.buffer.text().len();
        self.buffer.apply_edits(vec![(0..len, text.to_string())], 0);
        self.changed(cx);
    }

    /// Pastes, and for PHP adds the `use` statements the copied code relied on.
    pub(super) fn paste_with_imports(&mut self, text: &str, cx: &mut Context<Self>) {
        let imports = remembered_imports(text);
        self.buffer.paste(text);
        self.changed(cx);
        if imports.is_empty() || !self.buffer.is_php() || !crate::config::Settings::load_cached().auto_import_on_paste {
            return;
        }
        let Some(path) = self.path() else { return };
        let head = self.buffer.selection.head;
        let start = head.saturating_sub(text.chars().count());
        let pasted = self.buffer.char_to_byte(start)..self.buffer.char_to_byte(head);
        let edits = self.intel.paste_imports(&path, &self.buffer.text(), &imports, pasted);
        if edits.is_empty() {
            return;
        }
        let caret = self.caret_byte();
        self.buffer
            .apply_edits(edits.into_iter().map(|edit| (edit.range, edit.text)).collect(), caret);
        self.changed(cx);
    }

    pub(super) fn paste_text(&mut self, text: &str, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(text.to_string()));
        remember_copy(text, remembered_imports(text));
        self.paste_with_imports(text, cx);
    }

    /// FQNs of the classes the current selection uses, stored with the copied text.
    pub(super) fn selection_imports(&self) -> Vec<String> {
        let Some(path) = self.path() else { return Vec::new() };
        if !self.buffer.is_php() {
            return Vec::new();
        }
        let ranges: Vec<_> = self
            .buffer
            .selections()
            .iter()
            .map(|selection| {
                let range = selection.range();
                self.buffer.char_to_byte(range.start)..self.buffer.char_to_byte(range.end)
            })
            .collect();
        self.intel.imports_in(&path, &self.buffer.text(), &ranges)
    }
}
