//! Multiple carets: occurrences (⌃G / ⌃⌘G / ⌃⇧G), carets at line ends, clone caret
//! above/below, and Escape back to one caret. Mouse: ⌥⇧-click adds a caret, ⌥-drag selects
//! a column.

use gpui::{Context, Window};

use super::{
    AddNextOccurrence, CaretsAtLineEnds, ClearCarets, CloneCaretAbove, CloneCaretBelow, EditorView,
    SelectAllOccurrences, UnselectOccurrence,
};

impl EditorView {
    pub(super) fn add_next_occurrence(&mut self, _: &AddNextOccurrence, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.add_next_occurrence();
        self.moved(cx);
    }

    pub(super) fn unselect_occurrence(&mut self, _: &UnselectOccurrence, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.unselect_occurrence();
        self.moved(cx);
    }

    pub(super) fn select_all_occurrences(&mut self, _: &SelectAllOccurrences, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.select_all_occurrences();
        self.moved(cx);
    }

    pub(super) fn carets_at_line_ends(&mut self, _: &CaretsAtLineEnds, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.carets_at_line_ends();
        self.moved(cx);
    }

    pub(super) fn clone_caret_above(&mut self, _: &CloneCaretAbove, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.clone_caret(-1);
        self.moved(cx);
    }

    pub(super) fn clone_caret_below(&mut self, _: &CloneCaretBelow, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.clone_caret(1);
        self.moved(cx);
    }

    pub(super) fn clear_carets(&mut self, _: &ClearCarets, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.clear_extra_carets();
        cx.notify();
    }
}
