//! Visual rows: what each screen line of the editor shows once folded regions are collapsed
//! and long lines are soft-wrapped. A row is one or more char ranges of the buffer; a folded
//! region shows as a placeholder between two of them (`function a() {⋯}`).

use std::ops::Range;

use editor::Buffer;

pub(super) const PLACEHOLDER: &str = "⋯";

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Row {
    /// Buffer line of the row's first segment.
    pub line: usize,
    /// The first row of `line` (soft-wrapped continuations don't repeat the line number).
    pub first: bool,
    /// Char ranges shown on this row, in order; a placeholder sits between consecutive ones.
    pub segments: Vec<Range<usize>>,
}

impl Row {
    pub fn start(&self) -> usize {
        self.segments.first().map_or(0, |s| s.start)
    }

    pub fn end(&self) -> usize {
        self.segments.last().map_or(0, |s| s.end)
    }

    /// Whether the char position is shown on this row (a caret may sit at a segment's end).
    pub fn contains(&self, ix: usize) -> bool {
        self.segments.iter().any(|s| s.start <= ix && ix <= s.end)
    }
}

/// Text of a row plus where each segment starts in it (display byte offset).
pub(super) struct RowText {
    pub text: String,
    pub starts: Vec<usize>,
}

impl RowText {
    pub fn new(buffer: &Buffer, row: &Row) -> Self {
        let mut text = String::new();
        let mut starts = Vec::with_capacity(row.segments.len());
        for (ix, segment) in row.segments.iter().enumerate() {
            if ix > 0 {
                text.push_str(PLACEHOLDER);
            }
            starts.push(text.len());
            text.push_str(&buffer.slice(segment.clone()));
        }
        Self { text, starts }
    }

    /// Display byte offset of a buffer byte offset, if the row shows it.
    pub fn display_byte(&self, buffer: &Buffer, row: &Row, byte: usize) -> Option<usize> {
        row.segments.iter().zip(&self.starts).find_map(|(seg, start)| {
            let (s, e) = (buffer.char_to_byte(seg.start), buffer.char_to_byte(seg.end));
            (s <= byte && byte <= e).then(|| start + byte - s)
        })
    }

    /// Display byte offset of a char position, if the row shows it.
    pub fn display_char(&self, buffer: &Buffer, row: &Row, ix: usize) -> Option<usize> {
        self.display_byte(buffer, row, buffer.char_to_byte(ix))
    }

    /// The buffer char position for a display byte offset; inside a placeholder it gives
    /// `Err(segment index after the placeholder)`.
    pub fn char_at(&self, buffer: &Buffer, row: &Row, display: usize) -> Result<usize, usize> {
        for (ix, (seg, start)) in row.segments.iter().zip(&self.starts).enumerate() {
            let len = buffer.char_to_byte(seg.end) - buffer.char_to_byte(seg.start);
            if display < *start {
                return Err(ix);
            }
            if display <= start + len {
                let byte = buffer.char_to_byte(seg.start) + (display - start);
                return Ok(buffer.byte_to_char(byte));
            }
        }
        Ok(row.end())
    }

    /// The segment that follows the placeholder covering display offset `display`, if any.
    pub fn placeholder_at(&self, display: usize) -> Option<usize> {
        (1..self.starts.len()).find(|&ix| self.starts[ix] - PLACEHOLDER.len() <= display && display < self.starts[ix])
    }

    /// Display byte ranges covering the buffer byte range `range` on this row.
    pub fn display_ranges(&self, buffer: &Buffer, row: &Row, range: &Range<usize>) -> Vec<Range<usize>> {
        row.segments
            .iter()
            .zip(&self.starts)
            .filter_map(|(seg, start)| {
                let (s, e) = (buffer.char_to_byte(seg.start), buffer.char_to_byte(seg.end));
                let (a, b) = (range.start.max(s), range.end.min(e));
                (a < b).then(|| start + a - s..start + b - s)
            })
            .collect()
    }
}

/// Builds the rows for the whole buffer. `folds` are collapsed regions as char positions
/// `(open, close)` sorted by `open`; `wrap` is the soft-wrap width in columns.
pub(super) fn build(buffer: &Buffer, folds: &[(usize, usize)], wrap: Option<usize>) -> Vec<Row> {
    let lines = buffer.len_lines();
    let mut rows = Vec::with_capacity(lines);
    let mut line = 0;
    let mut fold_ix = 0;
    while line < lines {
        let line_start = buffer.line_to_char(line);
        let mut line_end = line_start + buffer.line_len(line);
        let mut segments = Vec::new();
        let mut seg_start = line_start;
        let mut last_line = line;
        loop {
            while fold_ix < folds.len() && folds[fold_ix].0 < seg_start {
                fold_ix += 1;
            }
            match folds.get(fold_ix) {
                Some(&(open, close)) if open < line_end && close > open => {
                    segments.push(seg_start..open + 1);
                    seg_start = close;
                    last_line = buffer.char_to_line_col(close).0;
                    line_end = buffer.line_to_char(last_line) + buffer.line_len(last_line);
                    fold_ix += 1;
                }
                _ => break,
            }
        }
        segments.push(seg_start..line_end);
        match wrap {
            Some(cols) if segments.len() == 1 && line_end - line_start > cols && cols > 8 => {
                for (ix, part) in wrap_line(&buffer.slice(line_start..line_end), cols).into_iter().enumerate() {
                    rows.push(Row { line, first: ix == 0, segments: vec![line_start + part.start..line_start + part.end] });
                }
            }
            _ => rows.push(Row { line, first: true, segments }),
        }
        line = last_line + 1;
    }
    rows
}

/// The row a char position is drawn on: the first row showing it, except that a position at
/// a soft-wrap boundary belongs to the next row.
pub(super) fn row_of(rows: &[Row], ix: usize) -> usize {
    let at = rows.partition_point(|r| r.end() < ix).min(rows.len().saturating_sub(1));
    match rows.get(at + 1) {
        Some(next) if rows[at].end() == ix && next.start() == ix && next.line == rows[at].line => at + 1,
        _ => at,
    }
}

/// Splits a line into chunks of at most `cols` chars, breaking after spaces when possible.
fn wrap_line(text: &str, cols: usize) -> Vec<Range<usize>> {
    let chars: Vec<char> = text.chars().collect();
    let mut parts = Vec::new();
    let mut start = 0;
    while chars.len() - start > cols {
        let limit = start + cols;
        let brk = (start + cols / 2..limit).rev().find(|&i| chars[i] == ' ').map_or(limit, |i| i + 1);
        parts.push(start..brk);
        start = brk;
    }
    parts.push(start..chars.len());
    parts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folds_and_wraps() {
        let b = Buffer::new("a {\n  x\n} b\nc");
        let open = 2;
        let close = b.line_to_char(2);
        let rows = build(&b, &[(open, close)], None);
        assert_eq!(rows.len(), 2);
        let text = RowText::new(&b, &rows[0]);
        assert_eq!(text.text, "a {⋯} b");
        assert_eq!(text.char_at(&b, &rows[0], text.text.len()), Ok(b.line_to_char(2) + 3));
        assert_eq!(text.char_at(&b, &rows[0], 4), Err(1));
        assert_eq!(text.placeholder_at(3), Some(1));
        assert_eq!(text.placeholder_at(2), None);
        assert_eq!(rows[1].line, 3);

        let b = Buffer::new("one two three four five six seven");
        let rows = build(&b, &[], Some(10));
        assert!(rows.len() > 2);
        assert!(rows.iter().skip(1).all(|r| !r.first));
        assert_eq!(rows.iter().map(|r| b.slice(r.start()..r.end())).collect::<String>(), b.text());
    }
}
