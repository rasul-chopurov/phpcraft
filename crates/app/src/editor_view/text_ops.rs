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

static CLIPBOARD_HISTORY: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Remembers copied text for Paste from History (newest first, no duplicates).
pub(super) fn remember_copy(text: &str) {
    if text.is_empty() {
        return;
    }
    let mut history = CLIPBOARD_HISTORY.lock().unwrap();
    history.retain(|t| t != text);
    history.insert(0, text.to_string());
    history.truncate(HISTORY_LEN);
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
            .map(|text| {
                let first = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
                let label: String = first.chars().take(60).collect();
                let lines = text.lines().count();
                let detail = if lines > 1 { format!("{lines} lines") } else { String::new() };
                MenuItem::new(label, detail, "", MenuAction::Paste(text))
            })
            .collect();
        self.show_menu(items, false, cx);
    }

    pub(super) fn paste_text(&mut self, text: &str, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(text.to_string()));
        remember_copy(text);
        self.buffer.paste(text);
        self.changed(cx);
    }
}
