//! Syntax ranges for the editor: foldable regions and the nested node ranges that Extend
//! Selection walks through. Languages without a tree-sitter grammar fold on brackets.

use std::ops::Range;

use tree_sitter::Node;

use crate::{Grammar, Highlight, Lang, compiled, highlight_lang, parse_with};

/// A foldable region as byte offsets: text up to and including `open` stays visible, then a
/// placeholder, then the text from `close` on (`{⋯}`, `/*⋯*/`, `<div>⋯</div>`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fold {
    pub open: usize,
    pub close: usize,
}

fn grammar_for(lang: Lang) -> Option<Grammar> {
    Some(match lang {
        Lang::Php => Grammar::Php,
        Lang::Html | Lang::Xml => Grammar::Html,
        Lang::JavaScript => Grammar::JavaScript,
        Lang::TypeScript => Grammar::TypeScript,
        Lang::Tsx => Grammar::Tsx,
        Lang::Css => Grammar::Css,
        Lang::Json => Grammar::Json,
        Lang::Yaml => Grammar::Yaml,
        _ => return None,
    })
}

fn walk(node: Node, out: &mut impl FnMut(Node)) {
    out(node);
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk(child, out);
    }
}

/// Foldable regions spanning more than one line, at most one per starting line (the
/// outermost), sorted by `open`.
pub fn fold_ranges(lang: Lang, source: &str) -> Vec<Fold> {
    let bytes = source.as_bytes();
    let line_of = |b: usize| bytecount_lines(&bytes[..b.min(bytes.len())]);
    let mut folds = Vec::new();
    match grammar_for(lang).and_then(|g| Some((compiled(g)?, g))) {
        Some((compiled, grammar)) => {
            if let Some(tree) = parse_with(&compiled.language, source) {
                walk(tree.root_node(), &mut |node| {
                    let r = node.byte_range();
                    if r.len() < 2 || line_of(r.start) == line_of(r.end - 1) {
                        return;
                    }
                    let first = bytes[r.start];
                    let last = bytes[r.end - 1];
                    let pair = matches!((first, last), (b'{', b'}') | (b'[', b']') | (b'(', b')'));
                    if pair {
                        folds.push(Fold { open: r.start, close: r.end - 1 });
                    } else if node.kind().contains("comment") && source[r.clone()].starts_with("/*") {
                        folds.push(Fold { open: r.start + 1, close: r.end - 2 });
                    } else if grammar == Grammar::Html && node.kind() == "element" {
                        let mut cursor = node.walk();
                        let children: Vec<Node> = node.children(&mut cursor).collect();
                        if let (Some(start), Some(end)) = (children.first(), children.last())
                            && start.kind() == "start_tag"
                            && end.kind() == "end_tag"
                            && line_of(start.end_byte()) != line_of(end.start_byte())
                        {
                            folds.push(Fold { open: start.end_byte() - 1, close: end.start_byte() });
                        }
                    }
                });
            }
        }
        None => folds.extend(bracket_folds(lang, source)),
    }
    folds.extend(region_folds(source));
    folds.sort_by_key(|f| (f.open, std::cmp::Reverse(f.close)));
    // One fold per starting line: the first (outermost) wins.
    let mut last_line = usize::MAX;
    folds.retain(|f| {
        let line = line_of(f.open);
        let keep = line != last_line;
        last_line = line;
        keep
    });
    folds
}

fn bytecount_lines(bytes: &[u8]) -> usize {
    bytes.iter().filter(|&&b| b == b'\n').count()
}

/// Bracket pairs outside strings and comments (languages without a grammar, and Blade).
fn bracket_folds(lang: Lang, source: &str) -> Vec<Fold> {
    let skip: Vec<Range<usize>> = highlight_lang(lang, source)
        .into_iter()
        .filter(|s| matches!(s.highlight, Highlight::String | Highlight::Comment))
        .map(|s| s.range)
        .collect();
    let mut in_skip = skip.iter().peekable();
    let mut stack: Vec<(u8, usize)> = Vec::new();
    let mut folds = Vec::new();
    for (ix, &b) in source.as_bytes().iter().enumerate() {
        while in_skip.peek().is_some_and(|r| r.end <= ix) {
            in_skip.next();
        }
        if in_skip.peek().is_some_and(|r| r.start <= ix) {
            continue;
        }
        match b {
            b'{' | b'[' | b'(' => stack.push((b, ix)),
            b'}' | b']' | b')' => {
                let open = match b { b'}' => b'{', b']' => b'[', _ => b'(' };
                if let Some(pos) = stack.iter().rposition(|(c, _)| *c == open) {
                    let (_, start) = stack[pos];
                    stack.truncate(pos);
                    if source[start..ix].contains('\n') {
                        folds.push(Fold { open: start, close: ix });
                    }
                }
            }
            _ => {}
        }
    }
    folds
}

/// `// region Name` … `// endregion` (also `#region` and `<!-- region -->`).
fn region_folds(source: &str) -> Vec<Fold> {
    let marker = |line: &str| -> Option<bool> {
        let t = line.trim_start().trim_start_matches(['/', '#', '<', '!', '-', '*', ' ']);
        if t.starts_with("endregion") {
            Some(false)
        } else if t.starts_with("region") {
            Some(true)
        } else {
            None
        }
    };
    let mut stack = Vec::new();
    let mut folds = Vec::new();
    let mut offset = 0;
    for line in source.split_inclusive('\n') {
        let text = line.trim_end_matches(['\n', '\r']);
        match marker(text) {
            Some(true) if !text.is_empty() => stack.push(offset + text.len() - 1),
            Some(false) => {
                if let Some(open) = stack.pop() {
                    folds.push(Fold { open, close: offset + (text.len() - text.trim_start().len()) });
                }
            }
            _ => {}
        }
        offset += line.len();
    }
    folds
}

/// Byte ranges of every syntax node, for Extend Selection: sorted, without duplicates.
pub fn node_ranges(lang: Lang, source: &str) -> Vec<Range<usize>> {
    let Some(compiled) = grammar_for(lang).and_then(compiled) else { return Vec::new() };
    let Some(tree) = parse_with(&compiled.language, source) else { return Vec::new() };
    let mut ranges = Vec::new();
    walk(tree.root_node(), &mut |node| ranges.push(node.byte_range()));
    ranges.sort_by_key(|r| (r.start, std::cmp::Reverse(r.end)));
    ranges.dedup();
    ranges
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folds_php_blocks_comments_and_regions() {
        let src = "<?php\n/**\n * Doc\n */\nfunction a() {\n    $x = [\n        1,\n    ];\n}\n// region Helpers\nfunction b() {}\n// endregion\n";
        let folds = fold_ranges(Lang::Php, src);
        let shown: Vec<String> = folds.iter().map(|f| format!("{}⋯{}", &src[f.open..=f.open], &src[f.close..f.close + 1])).collect();
        assert!(shown.contains(&"*⋯*".to_string()), "{shown:?}");
        assert!(shown.contains(&"{⋯}".to_string()));
        assert!(shown.contains(&"[⋯]".to_string()));
        assert!(folds.iter().any(|f| src[f.close..].starts_with("// endregion")));
        // `function b() {}` is on one line: not foldable.
        assert_eq!(folds.iter().filter(|f| src[f.open..=f.open] == *"{").count(), 1);
    }

    #[test]
    fn folds_without_grammar_skip_strings() {
        let src = "SELECT ('(',\n  1)\n";
        let folds = fold_ranges(Lang::Sql, src);
        assert_eq!(folds, vec![Fold { open: 7, close: 16 }]);
    }

    #[test]
    fn node_ranges_nest() {
        let src = "<?php $a = foo($b);";
        let ranges = node_ranges(Lang::Php, src);
        let b = src.find("$b").unwrap();
        assert!(ranges.contains(&(b..b + 2)));
        assert!(ranges.iter().any(|r| r.start < b && r.end > b + 2));
    }
}
