//! Code folding: collapsible regions come from the syntax tree (blocks, arrays, doc comments,
//! HTML elements, `// region`). Collapsed regions are kept as buffer anchors so they follow
//! edits, and are remembered per file while the app runs.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use gpui::{Context, Window};

use super::{CollapseAll, CollapseRegion, EditorView, ExpandAll, ExpandRegion, FoldSelection};

/// Collapsed regions per file, as char positions (restored when the file is opened again).
static REMEMBERED: Mutex<Option<HashMap<PathBuf, Vec<(usize, usize)>>>> = Mutex::new(None);

#[derive(Default)]
pub(super) struct Folding {
    /// Collapsed regions as (open, close) anchor ids.
    collapsed: Vec<(usize, usize)>,
    /// Foldable regions (char positions) for a buffer version.
    candidates: Option<(u64, Vec<(usize, usize)>)>,
    /// Restore remembered folds once the candidates are known.
    restored: bool,
}

impl EditorView {
    /// Foldable regions as char positions (open, close), sorted by open.
    pub(super) fn fold_candidates(&mut self) -> Vec<(usize, usize)> {
        let version = self.buffer.version();
        if self.folding.candidates.as_ref().is_none_or(|(v, _)| *v != version) {
            let text = self.buffer.text();
            let folds = php_syntax::fold_ranges(self.buffer.lang(), &text)
                .into_iter()
                .map(|f| (self.buffer.byte_to_char(f.open), self.buffer.byte_to_char(f.close)))
                .collect();
            self.folding.candidates = Some((version, folds));
        }
        if !self.folding.restored {
            self.folding.restored = true;
            self.restore_folds();
        }
        self.folding.candidates.as_ref().map(|(_, f)| f.clone()).unwrap_or_default()
    }

    /// Collapsed regions as char positions, sorted; regions that no longer make sense or that
    /// contain a caret are expanded.
    pub(super) fn collapsed_folds(&mut self) -> Vec<(usize, usize)> {
        let carets: Vec<usize> = self.buffer.selections().iter().map(|s| s.head).collect();
        let mut keep = Vec::new();
        let mut folds = Vec::new();
        for (open_id, close_id) in std::mem::take(&mut self.folding.collapsed) {
            let (Some(open), Some(close)) = (self.buffer.anchor(open_id), self.buffer.anchor(close_id)) else { continue };
            let multiline = self.buffer.char_to_line_col(open).0 != self.buffer.char_to_line_col(close).0;
            let hides_caret = carets.iter().any(|&c| c > open && c < close);
            if open < close && multiline && !hides_caret {
                keep.push((open_id, close_id));
                folds.push((open, close));
            } else {
                self.buffer.remove_anchor(open_id);
                self.buffer.remove_anchor(close_id);
            }
        }
        self.folding.collapsed = keep;
        folds.sort();
        // Nested collapsed regions inside a collapsed one don't add rows of their own.
        let mut outer: Vec<(usize, usize)> = Vec::new();
        for f in folds {
            if outer.last().is_none_or(|o| f.0 > o.1) {
                outer.push(f);
            }
        }
        outer
    }

    fn collapse(&mut self, open: usize, close: usize) {
        if self.is_collapsed(open) {
            return;
        }
        let ids = (self.buffer.add_anchor(open), self.buffer.add_anchor(close));
        self.folding.collapsed.push(ids);
        // Keep carets out of the collapsed text.
        for s in std::iter::once(&mut self.buffer.selection).chain(self.buffer.extra.iter_mut()) {
            if s.head > open && s.head < close {
                *s = editor::Selection::caret(open);
            }
        }
        self.remember_folds();
    }

    fn is_collapsed(&self, open: usize) -> bool {
        self.folding.collapsed.iter().any(|&(o, _)| self.buffer.anchor(o) == Some(open))
    }

    fn expand_at(&mut self, open: usize) {
        let buffer = &mut self.buffer;
        self.folding.collapsed.retain(|&(o, c)| {
            let hit = buffer.anchor(o) == Some(open);
            if hit {
                buffer.remove_anchor(o);
                buffer.remove_anchor(c);
            }
            !hit
        });
        self.remember_folds();
    }

    /// Gutter arrow: collapses or expands the region starting on `line`.
    pub(super) fn toggle_fold_on_line(&mut self, line: usize, cx: &mut Context<Self>) {
        let candidates = self.fold_candidates();
        let Some(&(open, close)) = candidates.iter().find(|(o, _)| self.buffer.char_to_line_col(*o).0 == line) else { return };
        if self.is_collapsed(open) {
            self.expand_at(open);
        } else {
            self.collapse(open, close);
        }
        cx.notify();
    }

    /// Expands the collapsed region whose placeholder was clicked (`open` is its position).
    pub(super) fn expand_fold(&mut self, open: usize, cx: &mut Context<Self>) {
        self.expand_at(open);
        cx.notify();
    }

    /// The innermost foldable region around the caret.
    fn region_at_caret(&mut self) -> Option<(usize, usize)> {
        let caret = self.buffer.selection.head;
        self.fold_candidates()
            .into_iter()
            .filter(|&(o, c)| o <= caret && caret <= c + 1)
            .min_by_key(|&(o, c)| c - o)
    }

    pub(super) fn collapse_region(&mut self, _: &CollapseRegion, _: &mut Window, cx: &mut Context<Self>) {
        // Already collapsed here: collapse the enclosing region instead.
        let caret = self.buffer.selection.head;
        let candidates = self.fold_candidates();
        let target = candidates
            .iter()
            .filter(|&&(o, c)| o <= caret && caret <= c + 1 && !self.is_collapsed(o))
            .min_by_key(|&&(o, c)| c - o)
            .copied();
        if let Some((open, close)) = target {
            self.collapse(open, close);
        }
        cx.notify();
    }

    pub(super) fn expand_region(&mut self, _: &ExpandRegion, _: &mut Window, cx: &mut Context<Self>) {
        let line = self.buffer.char_to_line_col(self.buffer.selection.head).0;
        let on_line: Vec<usize> = self
            .folding
            .collapsed
            .iter()
            .filter_map(|&(o, _)| self.buffer.anchor(o))
            .filter(|&o| self.buffer.char_to_line_col(o).0 == line)
            .collect();
        match on_line.first() {
            Some(&open) => self.expand_at(open),
            None => {
                if let Some((open, _)) = self.region_at_caret() {
                    self.expand_at(open);
                }
            }
        }
        cx.notify();
    }

    pub(super) fn collapse_all(&mut self, _: &CollapseAll, _: &mut Window, cx: &mut Context<Self>) {
        let candidates = self.fold_candidates();
        // Outermost regions only, like PhpStorm (class bodies stay open so methods fold).
        let class_body = |b: &editor::Buffer, open: usize| b.line(b.char_to_line_col(open).0).contains("class ");
        for (open, close) in candidates {
            if !class_body(&self.buffer, open) {
                self.collapse(open, close);
            }
        }
        cx.notify();
    }

    pub(super) fn expand_all(&mut self, _: &ExpandAll, _: &mut Window, cx: &mut Context<Self>) {
        for (o, c) in std::mem::take(&mut self.folding.collapsed) {
            self.buffer.remove_anchor(o);
            self.buffer.remove_anchor(c);
        }
        self.remember_folds();
        cx.notify();
    }

    /// Fold Selection (⌘.): collapses the selected lines into one row.
    pub(super) fn fold_selection(&mut self, _: &FoldSelection, _: &mut Window, cx: &mut Context<Self>) {
        let range = self.buffer.selection.range();
        if range.is_empty() {
            return;
        }
        let first_line = self.buffer.char_to_line_col(range.start).0;
        let last_line = self.buffer.char_to_line_col(range.end).0;
        if first_line == last_line {
            return;
        }
        let open = self.buffer.line_to_char(first_line) + self.buffer.line_len(first_line).saturating_sub(1);
        let close = self.buffer.line_to_char(last_line) + self.buffer.line_len(last_line);
        self.buffer.move_to(range.start, false);
        self.collapse(open, close);
        cx.notify();
    }

    /// After moving up or down, a caret that landed inside a collapsed region goes to its
    /// visible edge instead of expanding it.
    pub(super) fn skip_folds(&mut self, down: bool) {
        let folds: Vec<(usize, usize)> = self
            .folding
            .collapsed
            .iter()
            .filter_map(|&(o, c)| Some((self.buffer.anchor(o)?, self.buffer.anchor(c)?)))
            .collect();
        let head = self.buffer.selection.head;
        if let Some(&(open, close)) = folds.iter().find(|&&(o, c)| head > o && head < c) {
            let target = if down { close } else { open };
            let select = !self.buffer.selection.is_empty();
            let anchor = self.buffer.selection.anchor;
            self.buffer.move_to(target, false);
            if select {
                self.buffer.selection.anchor = anchor;
            }
        }
    }

    fn remember_folds(&mut self) {
        let Some(path) = self.buffer.path().map(|p| p.to_path_buf()) else { return };
        let folds: Vec<(usize, usize)> = self
            .folding
            .collapsed
            .iter()
            .filter_map(|&(o, c)| Some((self.buffer.anchor(o)?, self.buffer.anchor(c)?)))
            .collect();
        REMEMBERED.lock().unwrap().get_or_insert_with(HashMap::new).insert(path, folds);
    }

    /// Re-collapses the regions remembered for this file that still exist.
    fn restore_folds(&mut self) {
        let Some(path) = self.buffer.path() else { return };
        let remembered = REMEMBERED.lock().unwrap().as_ref().and_then(|m| m.get(path).cloned()).unwrap_or_default();
        let candidates = self.folding.candidates.as_ref().map(|(_, c)| c.clone()).unwrap_or_default();
        for fold in remembered {
            if candidates.contains(&fold) {
                let ids = (self.buffer.add_anchor(fold.0), self.buffer.add_anchor(fold.1));
                self.folding.collapsed.push(ids);
            }
        }
    }
}
