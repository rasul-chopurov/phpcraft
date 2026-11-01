//! Text buffer with selection, undo and cached syntax highlighting.
//!
//! All positions are char indices into the rope unless named `*_byte` or `*_utf16`.

use std::fs;
use std::ops::Range;
use std::path::{Path, PathBuf};

use anyhow::Result;
use php_syntax::Span;
use ropey::Rope;


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
    /// Secondary carets before and after the edit.
    extra_before: Vec<Selection>,
    extra_after: Vec<Selection>,
    /// Edits sharing a group are undone together.
    group: u64,
}

pub struct Buffer {
    rope: Rope,
    path: Option<PathBuf>,
    /// The primary caret/selection.
    pub selection: Selection,
    /// Secondary carets (multi-caret editing); typing, deleting and moving act on all of them.
    pub extra: Vec<Selection>,
    goal_column: Option<usize>,
    /// One level of indentation (from .editorconfig: spaces or a tab).
    indent: String,
    /// Positions that follow edits (folds, bookmarks); `None` once removed.
    anchors: Vec<Option<usize>>,
    /// Extend Selection history for Shrink Selection: (before, after).
    expansions: Vec<(Selection, Selection)>,
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
            extra: Vec::new(),
            goal_column: None,
            indent: "    ".into(),
            anchors: Vec::new(),
            expansions: Vec::new(),
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

    /// Sets the indentation unit (`"\t"` or N spaces).
    pub fn set_indent(&mut self, indent: &str) {
        if !indent.is_empty() {
            self.indent = indent.to_string();
        }
    }

    pub fn indent_unit(&self) -> &str {
        &self.indent
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

    /// Every caret, primary first.
    pub fn selections(&self) -> Vec<Selection> {
        std::iter::once(self.selection).chain(self.extra.iter().copied()).collect()
    }

    pub fn has_extra_carets(&self) -> bool {
        !self.extra.is_empty()
    }

    /// Drops the secondary carets (Escape).
    pub fn clear_extra_carets(&mut self) {
        self.extra.clear();
    }

    /// Adds a caret/selection; ones that coincide with an existing caret are merged.
    pub fn add_selection(&mut self, selection: Selection) {
        self.extra.push(selection);
        self.normalize_selections();
    }

    /// The text of every selection in document order, joined by line breaks (copy).
    pub fn selections_text(&self) -> String {
        let mut ranges: Vec<Range<usize>> = self.selections().iter().map(Selection::range).collect();
        ranges.sort_by_key(|r| r.start);
        ranges.into_iter().map(|r| self.slice(r)).collect::<Vec<_>>().join("\n")
    }

    /// Removes secondary carets that overlap the primary one or each other.
    fn normalize_selections(&mut self) {
        let primary = self.selection.range();
        let mut kept: Vec<Selection> = Vec::new();
        for s in std::mem::take(&mut self.extra) {
            let r = s.range();
            let overlaps = |o: &Range<usize>| r == *o || (r.start < o.end && o.start < r.end);
            if !overlaps(&primary) && !kept.iter().any(|k| overlaps(&k.range())) {
                kept.push(s);
            }
        }
        kept.sort_by_key(|s| s.range().start);
        self.extra = kept;
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
            extra_before: self.extra.clone(),
            extra_after: Vec::new(),
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

    /// Runs an edit at every caret as one undo step. `edit` sees the buffer before any change
    /// and returns the char range to replace and its text (None leaves that caret alone);
    /// each caret ends up after its inserted text.
    pub fn edit_each(&mut self, mut edit: impl FnMut(&Self, Selection) -> Option<(Range<usize>, String)>) {
        self.edit_each_select(|b, s| edit(b, s).map(|(r, t)| {
            let len = t.chars().count();
            (r, t, len..len)
        }));
    }

    /// Like [`edit_each`](Self::edit_each), but `edit` also says what to select afterwards,
    /// as a char range relative to the start of the inserted text.
    pub fn edit_each_select(&mut self, mut edit: impl FnMut(&Self, Selection) -> Option<(Range<usize>, String, Range<usize>)>) {
        let selections = self.selections();
        let mut planned: Vec<(usize, Selection, Option<(Range<usize>, String, Range<usize>)>)> =
            selections.iter().enumerate().map(|(ix, s)| (ix, *s, edit(self, *s))).collect();
        planned.sort_by_key(|(_, s, e)| e.as_ref().map_or(s.range().start, |(r, _, _)| r.start));
        // Drop edits that overlap an earlier one.
        let mut last_end = 0;
        for (_, _, e) in planned.iter_mut() {
            if let Some((r, _, _)) = e {
                if r.start < last_end {
                    *e = None;
                } else {
                    last_end = r.end;
                }
            }
        }
        // Where each caret lands, accounting for the length change of the edits before it.
        let mut shift: isize = 0;
        let mut after = vec![Selection::default(); selections.len()];
        for (ix, s, e) in &planned {
            after[*ix] = match e {
                Some((r, text, select)) => {
                    let len = text.chars().count();
                    let start = (r.start as isize + shift) as usize;
                    shift += len as isize - r.len() as isize;
                    Selection { anchor: start + select.start, head: start + select.end }
                }
                None => Selection {
                    anchor: (s.anchor as isize + shift).max(0) as usize,
                    head: (s.head as isize + shift).max(0) as usize,
                },
            };
        }
        let edits: Vec<(Range<usize>, String)> = planned.into_iter().filter_map(|(_, _, e)| e.map(|(r, t, _)| (r, t))).collect();
        if edits.is_empty() {
            return;
        }
        self.next_group += 1;
        let group = self.next_group;
        for (range, text) in edits.into_iter().rev() {
            self.replace_in_group(range, &text, group);
        }
        self.selection = after[0];
        self.extra = after[1..].to_vec();
        self.normalize_selections();
        if let Some(last) = self.undo.last_mut() {
            last.after = self.selection;
            last.extra_after = self.extra.clone();
        }
    }

    pub fn insert(&mut self, text: &str) {
        if self.extra.is_empty() {
            self.replace(self.selection.range(), text);
        } else {
            self.edit_each(|_, s| Some((s.range(), text.to_string())));
        }
    }

    /// Pastes `text`; with one clipboard line per caret, each caret gets its own line.
    pub fn paste(&mut self, text: &str) {
        let lines: Vec<&str> = text.split('\n').collect();
        if self.extra.is_empty() || lines.len() != self.extra.len() + 1 {
            return self.insert(text);
        }
        // Lines go to carets in document order.
        let mut order: Vec<Selection> = self.selections();
        order.sort_by_key(|s| s.range().start);
        self.edit_each(|_, s| {
            let ix = order.iter().position(|o| *o == s)?;
            Some((s.range(), lines[ix].to_string()))
        });
    }

    pub fn newline(&mut self) {
        self.edit_each_select(|b, s| {
            let range = s.range();
            let (line, _) = b.char_to_line_col(range.start);
            let current = b.line(line);
            let outer: String = current.chars().take_while(|c| c.is_whitespace()).collect();
            let before_caret = b.slice(b.line_to_char(line)..range.start);
            let opens = before_caret.trim_end().chars().last().filter(|c| matches!(c, '{' | '(' | '['));
            let indent = if opens.is_some() { format!("{outer}{}", b.indent) } else { outer.clone() };
            let first = format!("\n{indent}");
            let len = first.chars().count();
            // Enter between a pair (`{|}`) puts the closing bracket on its own line.
            let closes = b.char_at(range.end).zip(opens).is_some_and(|(next, open)| closing(open) == Some(next));
            let text = if closes { format!("{first}\n{outer}") } else { first };
            Some((range, text, len..len))
        });
    }

    /// Tab: indents the selected lines when the selection spans lines, otherwise inserts
    /// one indentation unit at every caret.
    pub fn tab(&mut self) {
        let multiline = self.selections().iter().any(|s| {
            let r = s.range();
            !r.is_empty() && self.rope.char_to_line(r.start) != self.rope.char_to_line(r.end)
        });
        if multiline {
            return self.indent_lines(1);
        }
        let unit = self.indent.clone();
        self.insert(&unit);
    }

    /// Indents (`1`) or unindents (`-1`) the selected lines by one unit, keeping them selected.
    pub fn indent_lines(&mut self, direction: isize) {
        let lines = self.selected_lines();
        let unit = self.indent.clone();
        let mut edits = Vec::new();
        for line in lines.clone() {
            let text = self.line(line);
            let start = self.line_to_byte(line);
            if direction > 0 {
                if !text.trim().is_empty() {
                    edits.push((start..start, unit.clone()));
                }
            } else {
                let remove = if text.starts_with('\t') {
                    1
                } else {
                    text.chars().take(unit.chars().count().max(1)).take_while(|c| *c == ' ').count()
                };
                if remove > 0 {
                    edits.push((start..start + remove, String::new()));
                }
            }
        }
        if edits.is_empty() {
            return;
        }
        let selection = self.selection;
        let (head_line, head_col) = self.char_to_line_col(selection.head);
        let unit_len = unit.chars().count();
        let caret = self.char_to_byte(selection.head);
        self.apply_edits(edits, caret);
        if selection.is_empty() {
            let col = if direction > 0 { head_col + unit_len } else { head_col.saturating_sub(unit_len) };
            self.selection = Selection::caret(self.line_col_to_char(head_line, col));
        } else {
            // Keep whole lines selected.
            let start = self.line_to_char(lines.start);
            let last = lines.end - 1;
            let end = self.line_to_char(last) + self.line_len(last);
            self.selection = if selection.head >= selection.anchor { Selection { anchor: start, head: end } } else { Selection { anchor: end, head: start } };
        }
        if let Some(last) = self.undo.last_mut() {
            last.after = self.selection;
        }
    }

    fn char_at(&self, ix: usize) -> Option<char> {
        (ix < self.len_chars()).then(|| self.rope.char(ix))
    }

    /// Typing with smart pairs: an opening bracket or quote also inserts its closing one
    /// (or wraps the selection), typing a closing character that is already next steps over
    /// it. Anything else is a plain insert. Acts on every caret.
    pub fn type_text(&mut self, text: &str) {
        let mut chars = text.chars();
        let (Some(c), None) = (chars.next(), chars.next()) else {
            return self.insert(text);
        };
        self.edit_each_select(|b, s| {
            let range = s.range();
            let next = b.char_at(range.end);
            let prev = range.start.checked_sub(1).and_then(|i| b.char_at(i));
            // Step over a closing character that is already there.
            if s.is_empty() && matches!(c, ')' | ']' | '}' | '"' | '\'' | '`') && next == Some(c) {
                return Some((range.start..range.start + 1, c.to_string(), 1..1));
            }
            if let Some(close) = closing(c) {
                if !s.is_empty() {
                    let inner = b.slice(range.clone());
                    let len = inner.chars().count();
                    return Some((range, format!("{c}{inner}{close}"), 1..1 + len));
                }
                let free_after = next.is_none_or(|n| n.is_whitespace() || matches!(n, ')' | ']' | '}' | ';' | ',' | '>'));
                let is_quote = c == close;
                let free_before = !is_quote || prev.is_none_or(|p| !(p.is_alphanumeric() || p == '_' || p == c || p == '\\'));
                if free_after && free_before {
                    return Some((range, format!("{c}{close}"), 1..1));
                }
            }
            Some((range, c.to_string(), 1..1))
        });
    }

    pub fn backspace(&mut self) {
        let in_pair = |b: &Self, s: &Selection| {
            s.is_empty() && s.head > 0 && b.char_at(s.head - 1).and_then(closing).is_some_and(|close| b.char_at(s.head) == Some(close))
        };
        if !self.extra.is_empty() || in_pair(self, &self.selection) {
            return self.edit_each(|b, s| match s.is_empty() {
                // Inside an empty pair: remove both halves.
                true if in_pair(b, &s) => Some((s.head - 1..s.head + 1, String::new())),
                true => (s.head > 0).then(|| (s.head - 1..s.head, String::new())),
                false => Some((s.range(), String::new())),
            });
        }
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
        if !self.extra.is_empty() {
            let len = self.len_chars();
            return self.edit_each(|_, s| match s.is_empty() {
                true => (s.head < len).then(|| (s.head..s.head + 1, String::new())),
                false => Some((s.range(), String::new())),
            });
        }
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
        let (remove, insert, selection, extra) = if reverse {
            (&edit.new, &edit.old, edit.before, &edit.extra_before)
        } else {
            (&edit.old, &edit.new, edit.after, &edit.extra_after)
        };
        let removed = remove.chars().count();
        let end = edit.start + removed;
        self.rope.remove(edit.start..end);
        self.rope.insert(edit.start, insert);
        let inserted = insert.chars().count();
        for pos in self.anchors.iter_mut().flatten() {
            if *pos >= end {
                *pos = *pos + inserted - removed;
            } else if *pos > edit.start {
                *pos = edit.start;
            }
        }
        self.selection = selection;
        self.extra = extra.clone();
        self.goal_column = None;
        self.version += 1;
    }

    /// Merges consecutive single-word typing into one undo step.
    fn push_undo(&mut self, edit: Edit) {
        if let Some(last) = self.undo.last_mut() {
            let typing = last.old.is_empty()
                && edit.old.is_empty()
                && edit.extra_before.is_empty()
                && last.extra_after.is_empty()
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

    /// Applies a movement to every caret (each secondary caret is moved as if it were primary).
    fn move_all(&mut self, mut f: impl FnMut(&mut Self)) {
        if self.extra.is_empty() {
            return f(self);
        }
        let primary = self.selection;
        let goal = self.goal_column;
        let mut moved = Vec::with_capacity(self.extra.len());
        for s in std::mem::take(&mut self.extra) {
            self.selection = s;
            self.goal_column = None;
            f(self);
            moved.push(self.selection);
        }
        self.selection = primary;
        self.goal_column = goal;
        f(self);
        self.extra = moved;
        self.normalize_selections();
    }

    fn set_head(&mut self, head: usize, select: bool) {
        self.selection.head = head.min(self.len_chars());
        if !select {
            self.selection.anchor = self.selection.head;
        }
    }

    /// Moves the primary caret (dropping any secondary ones).
    pub fn move_to(&mut self, char_idx: usize, select: bool) {
        self.extra.clear();
        self.goal_column = None;
        self.set_head(char_idx, select);
    }

    pub fn move_left(&mut self, select: bool) {
        self.move_all(|b| b.move_left_one(select));
    }

    fn move_left_one(&mut self, select: bool) {
        let target = if !select && !self.selection.is_empty() {
            self.selection.range().start
        } else {
            self.selection.head.saturating_sub(1)
        };
        self.move_to(target, select);
    }

    pub fn move_right(&mut self, select: bool) {
        self.move_all(|b| b.move_right_one(select));
    }

    fn move_right_one(&mut self, select: bool) {
        let target = if !select && !self.selection.is_empty() {
            self.selection.range().end
        } else {
            self.selection.head + 1
        };
        self.move_to(target, select);
    }

    pub fn move_vertical(&mut self, delta: isize, select: bool) {
        self.move_all(|b| b.move_vertical_one(delta, select));
    }

    fn move_vertical_one(&mut self, delta: isize, select: bool) {
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
        self.move_all(|b| b.move_home_one(select));
    }

    fn move_home_one(&mut self, select: bool) {
        let (line, col) = self.char_to_line_col(self.selection.head);
        let indent = self.line(line).chars().take_while(|c| c.is_whitespace()).count();
        let target = if col == indent { 0 } else { indent };
        self.move_to(self.line_to_char(line) + target, select);
    }

    pub fn move_end(&mut self, select: bool) {
        self.move_all(|b| b.move_end_one(select));
    }

    fn move_end_one(&mut self, select: bool) {
        let (line, _) = self.char_to_line_col(self.selection.head);
        self.move_to(self.line_to_char(line) + self.line_len(line), select);
    }

    pub fn select_all(&mut self) {
        self.extra.clear();
        self.selection = Selection {
            anchor: 0,
            head: self.len_chars(),
        };
    }

    // --- indentation ---------------------------------------------------

    /// Auto-Indent Lines: re-indents the selected lines (or all of them) by bracket depth,
    /// ignoring brackets inside strings and comments. Continuation lines starting with `->`
    /// get one extra level.
    pub fn reindent_lines(&mut self, whole_file: bool) {
        let lines = if whole_file { 0..self.len_lines() } else { self.selected_lines() };
        let unit = self.indent.clone();
        let mut depth: i32 = 0;
        let mut edits = Vec::new();
        let mut in_block_comment = false;
        let mut in_string: Option<char> = None;
        for line in 0..lines.end {
            let text = self.line(line);
            let trimmed = text.trim_start();
            let starts_closed = trimmed.starts_with(['}', ']', ')']);
            if lines.contains(&line) && in_string.is_none() && !in_block_comment {
                let level = (depth - starts_closed as i32).max(0) as usize;
                let continuation = trimmed.starts_with("->") || trimmed.starts_with("?->");
                let wanted = if trimmed.is_empty() { String::new() } else { unit.repeat(level + continuation as usize) };
                let current = &text[..text.len() - trimmed.len()];
                if current != wanted {
                    let start = self.line_to_byte(line);
                    edits.push((start..start + current.len(), wanted));
                }
            }
            // Track depth through this line.
            let chars: Vec<char> = text.chars().collect();
            let mut i = 0;
            while i < chars.len() {
                let c = chars[i];
                let next = chars.get(i + 1).copied();
                if in_block_comment {
                    if c == '*' && next == Some('/') {
                        in_block_comment = false;
                        i += 1;
                    }
                } else if let Some(q) = in_string {
                    if c == '\\' {
                        i += 1;
                    } else if c == q {
                        in_string = None;
                    }
                } else if c == '/' && next == Some('*') {
                    in_block_comment = true;
                    i += 1;
                } else if (c == '/' && next == Some('/')) || c == '#' {
                    break;
                } else if c == '"' || c == '\'' || c == '`' {
                    in_string = Some(c);
                } else if matches!(c, '{' | '[' | '(') {
                    depth += 1;
                } else if matches!(c, '}' | ']' | ')') {
                    depth -= 1;
                }
                i += 1;
            }
        }
        let caret = self.char_to_byte(self.selection.head);
        let selection = self.selection;
        self.apply_edits(edits, caret);
        if !selection.is_empty() {
            let start = self.line_to_char(lines.start);
            let last = lines.end.saturating_sub(1);
            self.selection = Selection { anchor: start, head: self.line_to_char(last) + self.line_len(last) };
        }
    }

    // --- anchors ---------------------------------------------------------

    /// A position that moves with edits before it; returns its id.
    pub fn add_anchor(&mut self, pos: usize) -> usize {
        self.anchors.push(Some(pos.min(self.len_chars())));
        self.anchors.len() - 1
    }

    pub fn anchor(&self, id: usize) -> Option<usize> {
        self.anchors.get(id).copied().flatten()
    }

    pub fn remove_anchor(&mut self, id: usize) {
        if let Some(slot) = self.anchors.get_mut(id) {
            *slot = None;
        }
    }

    // --- extend selection ----------------------------------------------

    /// Extend Selection (⌥↑): grows the selection to the word, then to each enclosing syntax
    /// node, then the line and the whole file.
    pub fn extend_selection(&mut self) {
        let current = self.selection.range();
        let text = self.text();
        let mut candidates: Vec<Range<usize>> = Vec::new();
        let word = self.word_range_at(self.selection.head);
        if current.is_empty() && !word.is_empty() {
            // From a caret, the word comes first (not a token that merely touches the caret).
            candidates.push(word);
        } else {
            candidates.push(word);
            for r in php_syntax::node_ranges(self.lang(), &text) {
                candidates.push(self.byte_to_char(r.start)..self.byte_to_char(r.end));
            }
            let line = self.line_range_at(current.start);
            let line_end = line.start + self.line_len(self.rope.char_to_line(current.start));
            candidates.push(line.start..line_end);
            candidates.push(0..self.len_chars());
        }
        let next = candidates
            .into_iter()
            .filter(|r| r.start <= current.start && r.end >= current.end && *r != current && !r.is_empty())
            .min_by_key(|r| r.len());
        if let Some(next) = next {
            let before = self.selection;
            self.extra.clear();
            self.selection = Selection { anchor: next.start, head: next.end };
            self.expansions.push((before, self.selection));
        }
    }

    /// Shrink Selection (⌥↓): undoes the last Extend Selection.
    pub fn shrink_selection(&mut self) {
        while let Some((before, after)) = self.expansions.pop() {
            if after == self.selection {
                self.selection = before;
                return;
            }
        }
    }

    // --- text operations -------------------------------------------------

    /// The bracket matching the one at or just before the caret: (bracket, its match).
    pub fn matching_bracket(&self, ix: usize) -> Option<(usize, usize)> {
        let at = [Some(ix), ix.checked_sub(1)].into_iter().flatten()
            .find(|&i| self.char_at(i).is_some_and(|c| "()[]{}".contains(c)))?;
        let c = self.char_at(at)?;
        let (open, close, forward) = match c {
            '(' => ('(', ')', true), '[' => ('[', ']', true), '{' => ('{', '}', true),
            ')' => ('(', ')', false), ']' => ('[', ']', false), _ => ('{', '}', false),
        };
        let mut depth = 0usize;
        let len = self.len_chars();
        let mut i = at;
        loop {
            let ch = self.rope.char(i);
            if ch == open { if forward { depth += 1 } else { depth -= 1 } }
            if ch == close { if forward { depth -= 1 } else { depth += 1 } }
            if depth == 0 {
                return Some((at, i));
            }
            if forward {
                i += 1;
                if i >= len { return None; }
            } else {
                if i == 0 { return None; }
                i -= 1;
            }
        }
    }

    /// Move Caret to Matching Brace.
    pub fn go_to_matching_bracket(&mut self) {
        if let Some((at, other)) = self.matching_bracket(self.selection.head) {
            // Land after a closing bracket, on an opening one.
            let target = if other > at { other + 1 } else { other };
            self.move_to(target, false);
        }
    }

    /// Join Lines: each caret's line (or the selected lines) with the next, one space between.
    pub fn join_lines(&mut self) {
        let lines = self.selected_lines();
        let last = if lines.len() > 1 { lines.end - 1 } else { lines.start + 1 };
        if last >= self.len_lines() {
            return;
        }
        let mut joined = self.line(lines.start).trim_end().to_string();
        for line in lines.start + 1..=last {
            let next = self.line(line);
            let next = next.trim_start();
            if !next.is_empty() {
                if !joined.is_empty() && !joined.ends_with(['(', '[']) && !next.starts_with([')', ']', ';', ',', '.']) && !next.starts_with("->") {
                    joined.push(' ');
                }
                joined.push_str(next);
            }
        }
        let start = self.line_to_char(lines.start);
        let end = self.line_to_char(last) + self.line_len(last);
        let caret = start + self.line(lines.start).trim_end().chars().count();
        self.replace(start..end, &joined);
        self.move_to(caret, false);
    }

    /// Sort Lines (or reverse them): the selected lines, or the whole file.
    pub fn reorder_lines(&mut self, reverse: bool) {
        let lines = if self.selection.is_empty() { 0..self.len_lines() } else { self.selected_lines() };
        let mut texts: Vec<String> = lines.clone().map(|l| self.line(l)).collect();
        // A trailing empty line (file ends with a newline) stays last.
        let trailing = texts.last().is_some_and(|t| t.is_empty()) && lines.end == self.len_lines();
        if trailing {
            texts.pop();
        }
        if reverse { texts.reverse() } else { texts.sort() }
        let last = lines.start + texts.len().saturating_sub(1);
        let start = self.line_to_char(lines.start);
        let end = self.line_to_char(last) + self.line_len(last);
        let selection = self.selection;
        self.replace(start..end, &texts.join("\n"));
        self.selection = selection;
    }

    /// Toggle Case: upper-cases the selection (or the word at the caret) unless it already
    /// is, then lower-cases it.
    pub fn toggle_case(&mut self) {
        self.edit_each_select(|b, s| {
            let range = if s.is_empty() { b.word_range_at(s.head) } else { s.range() };
            if range.is_empty() {
                return None;
            }
            let text = b.slice(range.clone());
            let new = if text.chars().any(char::is_lowercase) { text.to_uppercase() } else { text.to_lowercase() };
            let len = new.chars().count();
            let select = if s.is_empty() { let col = s.head - range.start; col..col } else { 0..len };
            Some((range, new, select))
        });
    }

    // --- multiple carets ------------------------------------------------

    /// The selected text to look for, selecting the word at the caret first if nothing is
    /// selected (returns false then: the first press only selects the word).
    fn occurrence_needle(&mut self) -> Option<(String, bool)> {
        if self.selection.is_empty() {
            let word = self.word_range_at(self.selection.head);
            if word.is_empty() {
                return None;
            }
            self.selection = Selection { anchor: word.start, head: word.end };
            return Some((self.slice(word), false));
        }
        Some((self.selected_text(), true))
    }

    /// Char ranges of every occurrence of `needle`.
    fn occurrences(&self, needle: &str) -> Vec<Range<usize>> {
        let text = self.text();
        let len = needle.chars().count();
        text.match_indices(needle).map(|(byte, _)| {
            let start = self.byte_to_char(byte);
            start..start + len
        }).collect()
    }

    /// Add Selection for Next Occurrence (⌃G): selects the word at the caret, then each press
    /// adds the next occurrence of the selected text (wrapping around).
    pub fn add_next_occurrence(&mut self) {
        let Some((needle, selected)) = self.occurrence_needle() else { return };
        if !selected {
            return;
        }
        let taken: Vec<Range<usize>> = self.selections().iter().map(Selection::range).collect();
        let last_end = taken.iter().map(|r| r.end).max().unwrap_or(0);
        let all = self.occurrences(&needle);
        let next = all.iter().find(|r| r.start >= last_end && !taken.contains(r))
            .or_else(|| all.iter().find(|r| !taken.contains(r)));
        if let Some(next) = next.cloned() {
            // The newest selection becomes primary so the view follows it.
            self.extra.push(self.selection);
            self.selection = Selection { anchor: next.start, head: next.end };
            self.normalize_selections();
        }
    }

    /// Unselect Occurrence (⌃⇧G): drops the most recently added selection.
    pub fn unselect_occurrence(&mut self) {
        if let Some(last) = self.extra.iter().enumerate().max_by_key(|(_, s)| s.range().start).map(|(ix, _)| ix) {
            self.selection = self.extra.remove(last);
        }
    }

    /// Select All Occurrences (⌃⌘G).
    pub fn select_all_occurrences(&mut self) {
        let Some((needle, _)) = self.occurrence_needle() else { return };
        let primary = self.selection.range();
        self.extra = self.occurrences(&needle).into_iter().filter(|r| *r != primary)
            .map(|r| Selection { anchor: r.start, head: r.end }).collect();
        self.normalize_selections();
    }

    /// Add Carets to Ends of Selected Lines (⌥⇧G).
    pub fn carets_at_line_ends(&mut self) {
        let lines = self.selected_lines();
        if lines.len() < 2 {
            return;
        }
        let end = |b: &Self, l: usize| Selection::caret(b.line_to_char(l) + b.line_len(l));
        self.selection = end(self, lines.start);
        self.extra = lines.skip(1).map(|l| end(self, l)).collect();
        self.normalize_selections();
    }

    /// Clone Caret Above / Below: a new caret on the next line up (`-1`) or down (`1`) from
    /// the outermost caret in that direction, at the same column.
    pub fn clone_caret(&mut self, delta: isize) {
        let all = self.selections();
        let edge = if delta < 0 { all.iter().min_by_key(|s| s.head) } else { all.iter().max_by_key(|s| s.head) };
        let Some(edge) = edge.copied() else { return };
        let (line, col) = self.char_to_line_col(edge.head);
        let target = line as isize + delta;
        if target < 0 || target >= self.len_lines() as isize {
            return;
        }
        let caret = Selection::caret(self.line_col_to_char(target as usize, col));
        self.extra.push(self.selection);
        self.selection = caret;
        self.normalize_selections();
    }

    /// Column (block) selection from `anchor` to `head`: one selection per line, between the
    /// two columns (short lines get a caret at their end).
    pub fn select_column(&mut self, anchor: usize, head: usize) {
        let (a_line, a_col) = self.char_to_line_col(anchor);
        let (h_line, h_col) = self.char_to_line_col(head);
        let lines: Vec<usize> = if a_line <= h_line { (a_line..=h_line).collect() } else { (h_line..=a_line).rev().collect() };
        let mut selections = lines.into_iter().map(|l| Selection {
            anchor: self.line_col_to_char(l, a_col),
            head: self.line_col_to_char(l, h_col),
        });
        let first = selections.next().unwrap_or_default();
        let rest: Vec<Selection> = selections.collect();
        // The line under the mouse is primary.
        match rest.last().copied() {
            Some(last) => {
                self.selection = last;
                self.extra = std::iter::once(first).chain(rest[..rest.len() - 1].iter().copied()).collect();
            }
            None => {
                self.selection = first;
                self.extra.clear();
            }
        }
        self.goal_column = None;
        self.normalize_selections();
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

/// The closing character for an opening bracket or quote.
fn closing(c: char) -> Option<char> {
    match c {
        '(' => Some(')'),
        '[' => Some(']'),
        '{' => Some('}'),
        '"' => Some('"'),
        '\'' => Some('\''),
        '`' => Some('`'),
        _ => None,
    }
}

/// Line comment markers for a language: (prefix, suffix); suffix is empty for line comments.
pub fn comment_tokens(lang: php_syntax::Lang) -> (&'static str, &'static str) {
    use php_syntax::Lang;
    match lang {
        Lang::Blade => ("{{-- ", " --}}"),
        Lang::Twig => ("{# ", " #}"),
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
    fn multiple_carets_type_delete_and_undo() {
        let mut b = Buffer::new("a\nb\nc");
        b.move_to(0, false);
        b.clone_caret(1);
        b.clone_caret(1);
        assert_eq!(b.selections().len(), 3);
        b.insert("x");
        assert_eq!(b.text(), "xa\nxb\nxc");
        b.move_end(false);
        b.insert(";");
        assert_eq!(b.text(), "xa;\nxb;\nxc;");
        b.backspace();
        assert_eq!(b.text(), "xa\nxb\nxc");
        b.undo();
        assert_eq!(b.text(), "xa;\nxb;\nxc;");
        assert_eq!(b.selections().len(), 3, "undo restores every caret");
        b.newline();
        assert_eq!(b.text(), "xa;\n\nxb;\n\nxc;\n");
    }

    #[test]
    fn occurrences() {
        let mut b = Buffer::new("$a = $b + $a; echo $a;");
        b.move_to(1, false);
        b.add_next_occurrence(); // selects `$a`
        assert_eq!(b.selected_text(), "$a");
        b.add_next_occurrence();
        b.add_next_occurrence();
        assert_eq!(b.selections().len(), 3);
        b.insert("$x");
        assert_eq!(b.text(), "$x = $b + $x; echo $x;");
        let mut b = Buffer::new("foo foo foo");
        b.move_to(0, false);
        b.select_all_occurrences();
        assert_eq!(b.selections().len(), 3);
        b.unselect_occurrence();
        assert_eq!(b.selections().len(), 2);
    }

    #[test]
    fn column_selection_and_paste() {
        let mut b = Buffer::new("abc\nde\nfghi");
        b.select_column(1, b.line_col_to_char(2, 3));
        assert_eq!(b.selections_text(), "bc\ne\ngh");
        b.paste("1\n2\n3");
        assert_eq!(b.text(), "a1\nd2\nf3i");
        let mut b = Buffer::new("one\ntwo");
        b.select_all();
        b.carets_at_line_ends();
        b.insert("!");
        assert_eq!(b.text(), "one!\ntwo!");
    }

    #[test]
    fn smart_pairs() {
        let mut b = Buffer::new("");
        b.type_text("f");
        b.type_text("(");
        assert_eq!(b.text(), "f()");
        b.type_text("$");
        b.type_text(")");
        assert_eq!(b.text(), "f($)", "typing ) steps over");
        let mut b = Buffer::new("x");
        b.select_all();
        b.type_text("[");
        assert_eq!(b.text(), "[x]");
        assert_eq!(b.selected_text(), "x");
        let mut b = Buffer::new("don");
        b.move_to(3, false);
        b.type_text("'");
        assert_eq!(b.text(), "don'", "no pair after a letter");
        let mut b = Buffer::new("");
        b.type_text("{");
        b.backspace();
        assert_eq!(b.text(), "", "backspace in an empty pair removes both");
        let mut b = Buffer::new("    if (x) {}");
        b.move_to(b.len_chars() - 1, false);
        b.newline();
        assert_eq!(b.text(), "    if (x) {\n        \n    }");
        assert_eq!(b.char_to_line_col(b.selection.head), (1, 8));
    }

    #[test]
    fn text_operations() {
        let b = Buffer::new("a(b[c]d)");
        assert_eq!(b.matching_bracket(1), Some((1, 7)));
        assert_eq!(b.matching_bracket(6), Some((5, 3)));
        let mut b = Buffer::new("$a = foo(\n    1,\n    2\n);");
        b.move_to(0, false);
        b.join_lines();
        assert_eq!(b.text(), "$a = foo(1,\n    2\n);");
        let mut b = Buffer::new("c\na\nb\n");
        b.reorder_lines(false);
        assert_eq!(b.text(), "a\nb\nc\n");
        b.reorder_lines(true);
        assert_eq!(b.text(), "c\nb\na\n");
        let mut b = Buffer::new("hello");
        b.move_to(2, false);
        b.toggle_case();
        assert_eq!(b.text(), "HELLO");
        b.toggle_case();
        assert_eq!(b.text(), "hello");
        let mut b = Buffer::new("a\nb");
        b.set_indent("\t");
        b.select_all();
        b.tab();
        assert_eq!(b.text(), "\ta\n\tb");
        b.indent_lines(-1);
        assert_eq!(b.text(), "a\nb");
    }

    #[test]
    fn anchors_follow_edits_and_extend_selection() {
        let mut b = Buffer::new("<?php\n$a = foo($bar, 2);");
        let id = b.add_anchor(10);
        b.move_to(0, false);
        b.insert("// x\n");
        assert_eq!(b.anchor(id), Some(15));
        b.undo();
        assert_eq!(b.anchor(id), Some(10));
        let at = b.text().find("bar").unwrap();
        b.move_to(at, false);
        b.extend_selection();
        assert_eq!(b.selected_text(), "$bar");
        b.extend_selection();
        b.extend_selection();
        assert!(b.selected_text().contains("$bar, 2"), "{}", b.selected_text());
        b.shrink_selection();
        b.shrink_selection();
        assert_eq!(b.selected_text(), "$bar");
    }

    #[test]
    fn reindents_by_brackets() {
        let mut b = Buffer::new("<?php\nfunction f() {\nif ($a) {\n$x = ['{',\n1];\n}\n  return $q\n->where();\n}\n");
        b.reindent_lines(true);
        assert_eq!(b.text(), "<?php\nfunction f() {\n    if ($a) {\n        $x = ['{',\n            1];\n    }\n    return $q\n        ->where();\n}\n");
    }

    #[test]
    fn highlights_php_buffers() {
        let mut b = Buffer::new("<?php echo 1;");
        assert!(!b.highlights().is_empty());
        let mut t = Buffer::new("plain text");
        assert!(t.highlights().is_empty());
    }
}
