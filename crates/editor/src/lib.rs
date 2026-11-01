//! Text buffer with selection, undo and cached syntax highlighting.
//!
//! All positions are char indices into the rope unless named `*_byte` or `*_utf16`.

use std::fs;
use std::ops::Range;
use std::path::{Path, PathBuf};

use anyhow::Result;
use php_syntax::Span;
use ropey::Rope;

const INDENT: &str = "    ";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Selection {
    pub anchor: usize,
    pub head: usize,
}

impl Selection {
    pub fn caret(at: usize) -> Self {
        Self { anchor: at, head: at }
    }

    pub fn range(&self) -> Range<usize> {
        self.anchor.min(self.head)..self.anchor.max(self.head)
    }

    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }
}

#[derive(Debug, Clone)]
struct Edit {
    start: usize,
    old: String,
    new: String,
    before: Selection,
    after: Selection,
}

pub struct Buffer {
    rope: Rope,
    path: Option<PathBuf>,
    pub selection: Selection,
    goal_column: Option<usize>,
    undo: Vec<Edit>,
    redo: Vec<Edit>,
    saved_version: u64,
    version: u64,
    highlights: Option<(u64, Vec<Span>)>,
}

impl Buffer {
    pub fn new(text: &str) -> Self {
        Self {
            rope: Rope::from_str(text),
            path: None,
            selection: Selection::default(),
            goal_column: None,
            undo: Vec::new(),
            redo: Vec::new(),
            saved_version: 0,
            version: 0,
            highlights: None,
        }
    }

    pub fn open(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let text = fs::read_to_string(&path)?;
        let mut buffer = Self::new(&text);
        buffer.path = Some(path);
        Ok(buffer)
    }

    pub fn save(&mut self) -> Result<()> {
        if let Some(path) = &self.path {
            let mut file = fs::File::create(path)?;
            self.rope.write_to(&mut file)?;
            self.saved_version = self.version;
        }
        Ok(())
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn title(&self) -> String {
        self.path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "untitled".into())
    }

    pub fn is_dirty(&self) -> bool {
        self.version != self.saved_version
    }

    pub fn version(&self) -> u64 {
        self.version
    }

    pub fn text(&self) -> String {
        self.rope.to_string()
    }

    pub fn rope(&self) -> &Rope {
        &self.rope
    }

    pub fn len_chars(&self) -> usize {
        self.rope.len_chars()
    }

    pub fn len_lines(&self) -> usize {
        self.rope.len_lines()
    }

    /// Line text without its trailing line break.
    pub fn line(&self, line: usize) -> String {
        let mut text = self.rope.line(line).to_string();
        while text.ends_with('\n') || text.ends_with('\r') {
            text.pop();
        }
        text
    }

    pub fn line_len(&self, line: usize) -> usize {
        self.line(line).chars().count()
    }

    pub fn line_to_char(&self, line: usize) -> usize {
        self.rope.line_to_char(line)
    }

    pub fn line_to_byte(&self, line: usize) -> usize {
        self.rope.line_to_byte(line)
    }

    pub fn char_to_line_col(&self, char_idx: usize) -> (usize, usize) {
        let line = self.rope.char_to_line(char_idx);
        (line, char_idx - self.rope.line_to_char(line))
    }

    pub fn line_col_to_char(&self, line: usize, col: usize) -> usize {
        let line = line.min(self.len_lines().saturating_sub(1));
        self.rope.line_to_char(line) + col.min(self.line_len(line))
    }

    pub fn char_to_utf16(&self, char_idx: usize) -> usize {
        self.rope.char_to_utf16_cu(char_idx)
    }

    pub fn utf16_to_char(&self, utf16: usize) -> usize {
        self.rope.utf16_cu_to_char(utf16.min(self.rope.len_utf16_cu()))
    }

    pub fn slice(&self, range: Range<usize>) -> String {
        self.rope.slice(range).to_string()
    }

    pub fn selected_text(&self) -> String {
        self.slice(self.selection.range())
    }

    pub fn is_php(&self) -> bool {
        let by_ext = self
            .path
            .as_ref()
            .and_then(|p| p.extension())
            .is_some_and(|e| e == "php" || e == "phtml");
        by_ext || self.rope.slice(..self.len_chars().min(5)) == "<?php"
    }

    /// Highlight spans in byte offsets for the current version (computed lazily).
    pub fn highlights(&mut self) -> &[Span] {
        let stale = self.highlights.as_ref().is_none_or(|(v, _)| *v != self.version);
        if stale {
            let spans = if self.is_php() {
                php_syntax::highlight(&self.text())
            } else {
                Vec::new()
            };
            self.highlights = Some((self.version, spans));
        }
        &self.highlights.as_ref().unwrap().1
    }

    // --- editing -------------------------------------------------------

    /// Replaces `range` with `text` and puts the caret after the inserted text.
    pub fn replace(&mut self, range: Range<usize>, text: &str) {
        let before = self.selection;
        let caret = range.start + text.chars().count();
        let edit = Edit {
            start: range.start,
            old: self.slice(range),
            new: text.to_string(),
            before,
            after: Selection::caret(caret),
        };
        self.apply(&edit, false);
        self.push_undo(edit);
        self.redo.clear();
    }

    pub fn insert(&mut self, text: &str) {
        self.replace(self.selection.range(), text);
    }

    pub fn newline(&mut self) {
        let (line, _) = self.char_to_line_col(self.selection.range().start);
        let current = self.line(line);
        let mut indent: String = current.chars().take_while(|c| c.is_whitespace()).collect();
        let before_caret = self.slice(self.line_to_char(line)..self.selection.range().start);
        if before_caret.trim_end().ends_with(['{', '(', '[']) {
            indent.push_str(INDENT);
        }
        self.insert(&format!("\n{indent}"));
    }

    pub fn tab(&mut self) {
        self.insert(INDENT);
    }

    pub fn backspace(&mut self) {
        if self.selection.is_empty() {
            let head = self.selection.head;
            if head == 0 {
                return;
            }
            self.replace(head - 1..head, "");
        } else {
            self.insert("");
        }
    }

    pub fn delete(&mut self) {
        if self.selection.is_empty() {
            let head = self.selection.head;
            if head >= self.len_chars() {
                return;
            }
            self.replace(head..head + 1, "");
        } else {
            self.insert("");
        }
    }

    pub fn undo(&mut self) {
        if let Some(edit) = self.undo.pop() {
            self.apply(&edit, true);
            self.redo.push(edit);
        }
    }

    pub fn redo(&mut self) {
        if let Some(edit) = self.redo.pop() {
            self.apply(&edit, false);
            self.undo.push(edit);
        }
    }

    fn apply(&mut self, edit: &Edit, reverse: bool) {
        let (remove, insert, selection) = if reverse {
            (&edit.new, &edit.old, edit.before)
        } else {
            (&edit.old, &edit.new, edit.after)
        };
        let end = edit.start + remove.chars().count();
        self.rope.remove(edit.start..end);
        self.rope.insert(edit.start, insert);
        self.selection = selection;
        self.goal_column = None;
        self.version += 1;
    }

    /// Merges consecutive single-word typing into one undo step.
    fn push_undo(&mut self, edit: Edit) {
        if let Some(last) = self.undo.last_mut() {
            let typing = last.old.is_empty()
                && edit.old.is_empty()
                && last.start + last.new.chars().count() == edit.start
                && !edit.new.contains(char::is_whitespace)
                && !last.new.contains(char::is_whitespace);
            if typing {
                last.new.push_str(&edit.new);
                last.after = edit.after;
                return;
            }
        }
        self.undo.push(edit);
    }

    // --- movement ------------------------------------------------------

    fn set_head(&mut self, head: usize, select: bool) {
        self.selection.head = head.min(self.len_chars());
        if !select {
            self.selection.anchor = self.selection.head;
        }
    }

    pub fn move_to(&mut self, char_idx: usize, select: bool) {
        self.goal_column = None;
        self.set_head(char_idx, select);
    }

    pub fn move_left(&mut self, select: bool) {
        let target = if !select && !self.selection.is_empty() {
            self.selection.range().start
        } else {
            self.selection.head.saturating_sub(1)
        };
        self.move_to(target, select);
    }

    pub fn move_right(&mut self, select: bool) {
        let target = if !select && !self.selection.is_empty() {
            self.selection.range().end
        } else {
            self.selection.head + 1
        };
        self.move_to(target, select);
    }

    pub fn move_vertical(&mut self, delta: isize, select: bool) {
        let (line, col) = self.char_to_line_col(self.selection.head);
        let goal = *self.goal_column.get_or_insert(col);
        let last = self.len_lines().saturating_sub(1) as isize;
        let target_line = line as isize + delta;
        let head = if target_line < 0 {
            0
        } else if target_line > last {
            self.len_chars()
        } else {
            self.line_col_to_char(target_line as usize, goal)
        };
        self.set_head(head, select);
    }

    /// Smart home: first non-blank character, then column 0.
    pub fn move_home(&mut self, select: bool) {
        let (line, col) = self.char_to_line_col(self.selection.head);
        let indent = self.line(line).chars().take_while(|c| c.is_whitespace()).count();
        let target = if col == indent { 0 } else { indent };
        self.move_to(self.line_to_char(line) + target, select);
    }

    pub fn move_end(&mut self, select: bool) {
        let (line, _) = self.char_to_line_col(self.selection.head);
        self.move_to(self.line_to_char(line) + self.line_len(line), select);
    }

    pub fn select_all(&mut self) {
        self.selection = Selection {
            anchor: 0,
            head: self.len_chars(),
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typing_and_undo() {
        let mut b = Buffer::new("<?php\n");
        b.move_to(b.len_chars(), false);
        for c in ["e", "c", "h", "o", " ", "1"] {
            b.insert(c);
        }
        assert_eq!(b.text(), "<?php\necho 1");
        b.undo();
        assert_eq!(b.text(), "<?php\necho ");
        b.undo();
        b.undo();
        assert_eq!(b.text(), "<?php\n");
        b.redo();
        assert_eq!(b.text(), "<?php\necho");
        assert!(b.is_dirty());
    }

    #[test]
    fn newline_keeps_and_increases_indent() {
        let mut b = Buffer::new("    if (x) {");
        b.move_end(false);
        b.newline();
        assert_eq!(b.text(), "    if (x) {\n        ");
    }

    #[test]
    fn vertical_movement_keeps_goal_column() {
        let mut b = Buffer::new("abcdef\nab\nabcdef");
        b.move_to(5, false);
        b.move_vertical(1, false);
        assert_eq!(b.char_to_line_col(b.selection.head), (1, 2));
        b.move_vertical(1, false);
        assert_eq!(b.char_to_line_col(b.selection.head), (2, 5));
    }

    #[test]
    fn backspace_deletes_selection() {
        let mut b = Buffer::new("hello world");
        b.selection = Selection { anchor: 5, head: 11 };
        b.backspace();
        assert_eq!(b.text(), "hello");
        assert_eq!(b.selection, Selection::caret(5));
    }

    #[test]
    fn highlights_php_buffers() {
        let mut b = Buffer::new("<?php echo 1;");
        assert!(!b.highlights().is_empty());
        let mut t = Buffer::new("plain text");
        assert!(t.highlights().is_empty());
    }
}
