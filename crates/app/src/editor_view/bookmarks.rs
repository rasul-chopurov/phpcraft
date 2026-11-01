//! Bookmarks in the editor: F3 toggles one on the caret line, ⌥F3 picks a mnemonic, Next /
//! Previous Bookmark move between them. Lines are anchors, so bookmarks follow edits.

use gpui::{Context, Window};

use super::intel_ui::{MenuAction, MenuItem};
use super::{EditorView, NextBookmark, PrevBookmark, ToggleBookmark, ToggleBookmarkWithMnemonic};

impl EditorView {
    /// Loads the file's bookmarks from the shared store.
    pub(super) fn load_bookmarks(&mut self) {
        let Some(path) = self.buffer.path().map(|p| p.to_path_buf()) else { return };
        for (line, mnemonic) in crate::bookmarks::for_file(&path) {
            if line < self.buffer.len_lines() {
                let id = self.buffer.add_anchor(self.buffer.line_to_char(line));
                self.bookmarks.push((id, mnemonic));
            }
        }
    }

    /// Bookmarked lines with their mnemonics, by line.
    pub(super) fn bookmark_lines(&self) -> Vec<(usize, Option<char>)> {
        let mut lines: Vec<(usize, Option<char>)> = self
            .bookmarks
            .iter()
            .filter_map(|&(id, m)| Some((self.buffer.char_to_line_col(self.buffer.anchor(id)?).0, m)))
            .collect();
        lines.sort();
        lines.dedup_by_key(|(l, _)| *l);
        lines
    }

    /// Writes the current bookmark lines back to the store.
    pub(super) fn save_bookmarks(&self) {
        if let Some(path) = self.buffer.path() {
            crate::bookmarks::set_file(path, &self.bookmark_lines());
        }
    }

    fn bookmark_on_line(&self, line: usize) -> Option<usize> {
        self.bookmarks.iter().position(|&(id, _)| {
            self.buffer.anchor(id).is_some_and(|c| self.buffer.char_to_line_col(c).0 == line)
        })
    }

    fn caret_line(&self) -> usize {
        self.buffer.char_to_line_col(self.buffer.selection.head).0
    }

    pub(super) fn toggle_bookmark(&mut self, _: &ToggleBookmark, _: &mut Window, cx: &mut Context<Self>) {
        let line = self.caret_line();
        match self.bookmark_on_line(line) {
            Some(ix) => {
                let (id, _) = self.bookmarks.remove(ix);
                self.buffer.remove_anchor(id);
            }
            None => {
                let id = self.buffer.add_anchor(self.buffer.line_to_char(line));
                self.bookmarks.push((id, None));
            }
        }
        self.save_bookmarks();
        cx.notify();
    }

    pub(super) fn toggle_bookmark_with_mnemonic(&mut self, _: &ToggleBookmarkWithMnemonic, _: &mut Window, cx: &mut Context<Self>) {
        let line = self.caret_line();
        if let Some(ix) = self.bookmark_on_line(line).filter(|&ix| self.bookmarks[ix].1.is_some()) {
            let (id, _) = self.bookmarks.remove(ix);
            self.buffer.remove_anchor(id);
            self.save_bookmarks();
            cx.notify();
            return;
        }
        let used: Vec<char> = crate::bookmarks::all().iter().filter_map(|b| b.mnemonic).collect();
        let items = crate::bookmarks::MNEMONICS
            .chars()
            .map(|c| {
                let detail = if used.contains(&c) { "in use" } else { "" };
                MenuItem::new(c.to_string(), detail, "◆", MenuAction::Mnemonic(c))
            })
            .collect();
        self.show_menu(items, false, cx);
    }

    /// Sets a bookmark with `mnemonic` on the caret line (from the chooser).
    pub(super) fn set_bookmark_mnemonic(&mut self, mnemonic: char, cx: &mut Context<Self>) {
        let line = self.caret_line();
        // The mnemonic moves here from any other line of this file.
        for entry in self.bookmarks.iter_mut() {
            if entry.1 == Some(mnemonic) {
                entry.1 = None;
            }
        }
        match self.bookmark_on_line(line) {
            Some(ix) => self.bookmarks[ix].1 = Some(mnemonic),
            None => {
                let id = self.buffer.add_anchor(self.buffer.line_to_char(line));
                self.bookmarks.push((id, Some(mnemonic)));
            }
        }
        self.save_bookmarks();
        cx.notify();
    }

    fn jump_bookmark(&mut self, forward: bool, cx: &mut Context<Self>) {
        let line = self.caret_line();
        let lines: Vec<usize> = self.bookmark_lines().into_iter().map(|(l, _)| l).collect();
        let target = if forward {
            lines.iter().find(|&&l| l > line).or(lines.first())
        } else {
            lines.iter().rev().find(|&&l| l < line).or(lines.last())
        };
        if let Some(&target) = target {
            let at = self.buffer.line_to_char(target);
            self.reveal_char(at, cx);
        }
    }

    pub(super) fn next_bookmark(&mut self, _: &NextBookmark, _: &mut Window, cx: &mut Context<Self>) {
        self.jump_bookmark(true, cx);
    }

    pub(super) fn prev_bookmark(&mut self, _: &PrevBookmark, _: &mut Window, cx: &mut Context<Self>) {
        self.jump_bookmark(false, cx);
    }
}
