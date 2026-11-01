//! PHP parsing and syntax highlighting on top of tree-sitter-php.

use std::ops::Range;
use std::sync::OnceLock;

use streaming_iterator::StreamingIterator;
use tree_sitter::{Language, Parser, Query, QueryCursor, Tree};

/// Highlight category, roughly matching PhpStorm's Darcula token groups.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Highlight {
    Keyword,
    String,
    Comment,
    Number,
    Function,
    Method,
    Type,
    Variable,
    Property,
    Constant,
    Operator,
    Tag,
    Namespace,
}

impl Highlight {
    fn from_capture(name: &str) -> Option<Self> {
        let base = name.split('.').next().unwrap_or(name);
        Some(match (base, name) {
            (_, "function.method") => Self::Method,
            ("keyword", _) => Self::Keyword,
            ("string", _) => Self::String,
            ("comment", _) => Self::Comment,
            ("number", _) => Self::Number,
            ("function", _) | ("constructor", _) => Self::Function,
            ("type", _) => Self::Type,
            ("variable", _) => Self::Variable,
            ("property", _) => Self::Property,
            ("constant", _) => Self::Constant,
            ("operator", _) => Self::Operator,
            ("tag", _) => Self::Tag,
            ("module", _) => Self::Namespace,
            _ => return None,
        })
    }
}

/// A highlighted byte range in the source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub range: Range<usize>,
    pub highlight: Highlight,
}

pub fn language() -> Language {
    tree_sitter_php::LANGUAGE_PHP.into()
}

pub fn parse(source: &str) -> Option<Tree> {
    let mut parser = Parser::new();
    parser.set_language(&language()).ok()?;
    parser.parse(source, None)
}

struct HighlightQuery {
    query: Query,
    kinds: Vec<Option<Highlight>>,
}

fn highlight_query() -> &'static HighlightQuery {
    static QUERY: OnceLock<HighlightQuery> = OnceLock::new();
    QUERY.get_or_init(|| {
        let query = Query::new(&language(), tree_sitter_php::HIGHLIGHTS_QUERY)
            .expect("bundled PHP highlights query is valid");
        let kinds = query
            .capture_names()
            .iter()
            .map(|name| Highlight::from_capture(name))
            .collect();
        HighlightQuery { query, kinds }
    })
}

/// Returns non-overlapping highlight spans sorted by start offset.
///
/// When captures overlap, the first-matched (outermost pattern order) wins for
/// the overlapped bytes, which follows the precedence order of highlights.scm.
pub fn highlight(source: &str) -> Vec<Span> {
    let Some(tree) = parse(source) else {
        return Vec::new();
    };
    highlight_tree(&tree, source)
}

pub fn highlight_tree(tree: &Tree, source: &str) -> Vec<Span> {
    let hq = highlight_query();
    let mut cursor = QueryCursor::new();
    let mut raw: Vec<(Range<usize>, usize, Highlight)> = Vec::new();
    let mut captures = cursor.captures(&hq.query, tree.root_node(), source.as_bytes());
    while let Some((m, index)) = captures.next() {
        let capture = m.captures[*index];
        if let Some(kind) = hq.kinds[capture.index as usize] {
            raw.push((capture.node.byte_range(), m.pattern_index, kind));
        }
    }
    // Earlier patterns take precedence; sort by start, then pattern order.
    raw.sort_by_key(|(range, pattern, _)| (range.start, *pattern));

    let mut spans: Vec<Span> = Vec::with_capacity(raw.len());
    let mut covered_until = 0;
    for (range, _, highlight) in raw {
        let start = range.start.max(covered_until);
        if start >= range.end {
            continue;
        }
        spans.push(Span {
            range: start..range.end,
            highlight,
        });
        covered_until = range.end;
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kind_of(source: &str, needle: &str) -> Option<Highlight> {
        let start = source.find(needle).unwrap();
        highlight(source)
            .into_iter()
            .find(|s| s.range.start <= start && start < s.range.end)
            .map(|s| s.highlight)
    }

    #[test]
    fn highlights_basic_php() {
        let src = "<?php\n// hi\nfunction foo($bar) { return \"x\" . 42; }\n";
        assert_eq!(kind_of(src, "// hi"), Some(Highlight::Comment));
        assert_eq!(kind_of(src, "function"), Some(Highlight::Keyword));
        assert_eq!(kind_of(src, "foo"), Some(Highlight::Function));
        assert_eq!(kind_of(src, "$bar"), Some(Highlight::Variable));
        assert_eq!(kind_of(src, "\"x\""), Some(Highlight::String));
        assert_eq!(kind_of(src, "42"), Some(Highlight::Number));
    }

    #[test]
    fn spans_do_not_overlap() {
        let src = "<?php\nnamespace App;\nuse Foo\\Bar;\nclass A extends Bar { public int $x = 1; }\n";
        let spans = highlight(src);
        for pair in spans.windows(2) {
            assert!(pair[0].range.end <= pair[1].range.start);
        }
    }
}
