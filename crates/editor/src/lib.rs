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
    /// Edits sharing a group are undone together.
    group: u64,
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
    next_group: u64,
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
            next_group: 0,
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

    /// Follows a rename of the file (or of a folder containing it).
    pub fn rename_path(&mut self, from: &Path, to: &Path) {
        if let Some(path) = &self.path
            && let Ok(rest) = path.strip_prefix(from)
        {
            self.path = Some(if rest.as_os_str().is_empty() { to.to_path_buf() } else { to.join(rest) });
        }
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

    pub fn byte_to_char(&self, byte: usize) -> usize {
        self.rope.byte_to_char(byte.min(self.rope.len_bytes()))
    }

    pub fn char_to_byte(&self, char_idx: usize) -> usize {
        self.rope.char_to_byte(char_idx.min(self.len_chars()))
    }

    pub fn slice(&self, range: Range<usize>) -> String {
        self.rope.slice(range).to_string()
    }

    pub fn selected_text(&self) -> String {
        self.slice(self.selection.range())
    }

    /// Language for highlighting, from the path (or `<?php` for unsaved buffers).
    pub fn lang(&self) -> php_syntax::Lang {
        match &self.path {
            Some(path) => php_syntax::Lang::from_path(path),
            None if self.is_php() => php_syntax::Lang::Php,
            None => php_syntax::Lang::Plain,
        }
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
            let spans = php_syntax::highlight_lang(self.lang(), &self.text());
            self.highlights = Some((self.version, spans));
        }
        &self.highlights.as_ref().unwrap().1
    }

    // --- editing -------------------------------------------------------

    /// Replaces `range` with `text` and puts the caret after the inserted text.
    pub fn replace(&mut self, range: Range<usize>, text: &str) {
        self.next_group += 1;
        self.replace_in_group(range, text, self.next_group);
    }

    fn replace_in_group(&mut self, range: Range<usize>, text: &str, group: u64) {
        let before = self.selection;
        let caret = range.start + text.chars().count();
        let edit = Edit {
            start: range.start,
            old: self.slice(range),
            new: text.to_string(),
            before,
            after: Selection::caret(caret),
            group,
        };
        self.apply(&edit, false);
        self.push_undo(edit);
        self.redo.clear();
    }

    /// Applies non-overlapping byte-range edits as one undo step, then puts the caret at
    /// `caret` (a byte offset in the text before the edits, shifted by edits ending before it).
    pub fn apply_edits(&mut self, mut edits: Vec<(Range<usize>, String)>, caret: usize) {
        if edits.is_empty() {
            return;
        }
        let shift: isize = edits
            .iter()
            .filter(|(range, _)| range.end <= caret)
            .map(|(range, text)| text.len() as isize - range.len() as isize)
            .sum();
        let caret = (caret as isize + shift).max(0) as usize;
        edits.sort_by_key(|(range, _)| std::cmp::Reverse(range.start));
        self.next_group += 1;
        let group = self.next_group;
        for (range, text) in edits {
            let chars = self.byte_to_char(range.start)..self.byte_to_char(range.end);
            self.replace_in_group(chars, &text, group);
        }
        let caret = self.byte_to_char(caret);
        self.selection = Selection::caret(caret);
        if let Some(last) = self.undo.last_mut() {
            last.after = self.selection;
        }
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
        let Some(group) = self.undo.last().map(|e| e.group) else { return };
        while self.undo.last().is_some_and(|e| e.group == group) {
            let edit = self.undo.pop().unwrap();
            self.apply(&edit, true);
            self.redo.push(edit);
        }
    }

    pub fn redo(&mut self) {
        let Some(group) = self.redo.last().map(|e| e.group) else { return };
        while self.redo.last().is_some_and(|e| e.group == group) {
            let edit = self.redo.pop().unwrap();
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
            if typing && last.group + 1 == edit.group {
                last.new.push_str(&edit.new);
                last.group = edit.group;
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

    // --- words and lines -------------------------------------------------

    /// The identifier-like word around `ix` (PHP variables include `$`).
    pub fn word_range_at(&self, ix: usize) -> Range<usize> {
        let is_word = |c: char| c.is_alphanumeric() || c == '_' || c == '$';
        let len = self.len_chars();
        let mut start = ix.min(len);
        while start > 0 && is_word(self.rope.char(start - 1)) {
            start -= 1;
        }
        let mut end = ix.min(len);
        while end < len && is_word(self.rope.char(end)) {
            end += 1;
        }
        start..end
    }

    /// The whole line containing `ix`, including its line break.
    pub fn line_range_at(&self, ix: usize) -> Range<usize> {
        let line = self.rope.char_to_line(ix.min(self.len_chars()));
        let start = self.rope.line_to_char(line);
        start..start + self.rope.line(line).len_chars()
    }

    /// Lines touched by the selection (a selection ending at column 0 excludes that line).
    fn selected_lines(&self) -> Range<usize> {
        let range = self.selection.range();
        let first = self.rope.char_to_line(range.start);
        let mut last = self.rope.char_to_line(range.end);
        if last > first && self.char_to_line_col(range.end).1 == 0 {
            last -= 1;
        }
        first..last + 1
    }

    /// Toggles a line comment (`//`, `#`, `--`) or a wrapping comment (`<!-- -->`) on the
    /// selected lines: comments them unless all non-blank lines are already commented.
    pub fn toggle_comment(&mut self, prefix: &str, suffix: &str) {
        let lines = self.selected_lines();
        let texts: Vec<String> = lines.clone().map(|l| self.line(l)).collect();
        let non_blank: Vec<&String> = texts.iter().filter(|t| !t.trim().is_empty()).collect();
        if non_blank.is_empty() {
            return;
        }
        let commented = non_blank.iter().all(|t| {
            let t = t.trim();
            t.starts_with(prefix.trim_end()) && (suffix.is_empty() || t.ends_with(suffix.trim_start()))
        });
        let indent = non_blank.iter().map(|t| t.len() - t.trim_start().len()).min().unwrap_or(0);
        let mut edits = Vec::new();
        for (offset, text) in texts.iter().enumerate() {
            if text.trim().is_empty() {
                continue;
            }
            let line_byte = self.line_to_byte(lines.start + offset);
            let new = if commented {
                let lead = text.len() - text.trim_start().len();
                let body = &text[lead..];
                let body = body.strip_prefix(prefix).or_else(|| body.strip_prefix(prefix.trim_end())).unwrap_or(body);
                let body = if suffix.is_empty() {
                    body
                } else {
                    body.strip_suffix(suffix).or_else(|| body.strip_suffix(suffix.trim_start())).unwrap_or(body)
                };
                format!("{}{body}", &text[..lead])
            } else {
                format!("{}{prefix}{}{suffix}", &text[..indent], &text[indent..])
            };
            edits.push((line_byte..line_byte + text.len(), new));
        }
        let selection = self.selection;
        let caret = self.char_to_byte(selection.head);
        self.apply_edits(edits, caret);
        // Keep the same lines selected.
        let start = self.line_to_char(lines.start);
        let end_line = lines.end - 1;
        let end = self.line_to_char(end_line) + self.line_len(end_line);
        if !selection.is_empty() {
            self.selection = Selection { anchor: start, head: end };
        } else {
            let (_, col) = self.char_to_line_col(selection.head);
            let line_len = self.line_len(lines.start);
            self.selection = Selection::caret(start + col.min(line_len));
        }
    }

    /// Duplicates the selection, or the current line when nothing is selected.
    pub fn duplicate(&mut self) {
        if self.selection.is_empty() {
            let line = self.char_to_line_col(self.selection.head).0;
            let text = self.line(line);
            let end = self.line_to_char(line) + self.line_len(line);
            let col = self.selection.head - self.line_to_char(line);
            self.replace(end..end, &format!("\n{text}"));
            let start = self.line_to_char(line + 1);
            self.move_to(start + col, false);
        } else {
            let range = self.selection.range();
            let text = self.slice(range.clone());
            self.replace(range.end..range.end, &text);
            self.selection = Selection { anchor: range.end, head: range.end + text.chars().count() };
        }
    }

    /// Deletes the selected lines (or the current line).
    pub fn delete_lines(&mut self) {
        let lines = self.selected_lines();
        let start = self.line_to_char(lines.start);
        let end = if lines.end < self.len_lines() { self.line_to_char(lines.end) } else { self.len_chars() };
        // Deleting the last line also removes the break before it.
        let start = if end == self.len_chars() && start > 0 && lines.end >= self.len_lines() { start - 1 } else { start };
        self.replace(start..end, "");
        let line = self.char_to_line_col(start.min(self.len_chars())).0;
        let target = self.line_to_char(line);
        self.move_to(target, false);
    }

    /// Moves the selected lines up (`-1`) or down (`1`).
    pub fn move_lines(&mut self, delta: isize) {
        let lines = self.selected_lines();
        let total = self.len_lines();
        if (delta < 0 && lines.start == 0) || (delta > 0 && lines.end >= total) {
            return;
        }
        let block: Vec<String> = lines.clone().map(|l| self.line(l)).collect();
        let other_line = if delta < 0 { lines.start - 1 } else { lines.end };
        let other = self.line(other_line);
        let first = lines.start.min(other_line);
        let last = (lines.end - 1).max(other_line);
        let start = self.line_to_char(first);
        let end = self.line_to_char(last) + self.line_len(last);
        let new = if delta < 0 {
            format!("{}\n{other}", block.join("\n"))
        } else {
            format!("{other}\n{}", block.join("\n"))
        };
        let selection = self.selection;
        let (head_line, head_col) = self.char_to_line_col(selection.head);
        let (anchor_line, anchor_col) = self.char_to_line_col(selection.anchor);
        self.replace(start..end, &new);
        let shift = |line: usize| (line as isize + delta) as usize;
        self.selection = Selection {
            anchor: self.line_col_to_char(shift(anchor_line), anchor_col),
            head: self.line_col_to_char(shift(head_line), head_col),
        };
    }
}

/// Line comment markers for a language: (prefix, suffix); suffix is empty for line comments.
pub fn comment_tokens(lang: php_syntax::Lang) -> (&'static str, &'static str) {
    use php_syntax::Lang;
    match lang {
        Lang::Blade => ("{{-- ", " --}}"),
        Lang::Html | Lang::Xml | Lang::Markdown => ("<!-- ", " -->"),
        Lang::Css => ("/* ", " */"),
        Lang::Yaml | Lang::Env | Lang::Ini | Lang::Shell | Lang::Dockerfile | Lang::Ignore => ("# ", ""),
        Lang::Sql => ("-- ", ""),
        _ => ("// ", ""),
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
    fn grouped_edits_undo_together() {
        let src = "<?php\nnamespace A;\n\nPos";
        let mut b = Buffer::new(src);
        let caret = src.len();
        b.apply_edits(
            vec![(caret - 3..caret, "Post".into()), (18..18, "\nuse App\\Post;".into())],
            caret,
        );
        assert_eq!(b.text(), "<?php\nnamespace A;\nuse App\\Post;\n\nPost");
        assert_eq!(b.selection.head, b.len_chars());
        b.undo();
        assert_eq!(b.text(), src);
    }

    #[test]
    fn comments_lines() {
        let mut b = Buffer::new("<?php\n    $a = 1;\n\n    $b = 2;\n");
        b.selection = Selection { anchor: 6, head: b.line_to_char(3) + 3 };
        b.toggle_comment("// ", "");
        assert_eq!(b.text(), "<?php\n    // $a = 1;\n\n    // $b = 2;\n");
        b.toggle_comment("// ", "");
        assert_eq!(b.text(), "<?php\n    $a = 1;\n\n    $b = 2;\n");
        let mut html = Buffer::new("<div></div>");
        html.toggle_comment("<!-- ", " -->");
        assert_eq!(html.text(), "<!-- <div></div> -->");
        html.toggle_comment("<!-- ", " -->");
        assert_eq!(html.text(), "<div></div>");
    }

    #[test]
    fn line_operations() {
        let mut b = Buffer::new("one\ntwo\nthree");
        b.move_to(5, false); // "two", column 1
        b.duplicate();
        assert_eq!(b.text(), "one\ntwo\ntwo\nthree");
        assert_eq!(b.char_to_line_col(b.selection.head), (2, 1));
        b.move_lines(1);
        assert_eq!(b.text(), "one\ntwo\nthree\ntwo");
        assert_eq!(b.char_to_line_col(b.selection.head), (3, 1));
        b.move_lines(-1);
        b.move_lines(-1);
        assert_eq!(b.text(), "one\ntwo\ntwo\nthree");
        b.delete_lines();
        assert_eq!(b.text(), "one\ntwo\nthree");
        assert_eq!(b.word_range_at(5), 4..7);
        assert_eq!(b.line_range_at(5), 4..8);
        b.undo();
        assert_eq!(b.text(), "one\ntwo\ntwo\nthree");
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
