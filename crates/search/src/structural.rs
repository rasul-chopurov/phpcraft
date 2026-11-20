//! Structural search: a pattern written in PHP tokens with `$Name$` holes, matched
//! against the token stream of a file instead of its text.
//!
//! Matching tokens rather than characters makes the pattern ignore formatting and
//! comments, and lets a hole stand for a whole expression: `$A$->where($B$)` finds
//! `$query->where('id', 1)` however the call is laid out, and a hole never runs past
//! a `;` — a pattern cannot match across two statements.

use std::fs;
use std::ops::Range;
use std::path::{Path, PathBuf};

use crate::{FileSearch, LineMatch, MAX_FILE_BYTES, list_files, mask_matcher};

/// Tokens a single hole may swallow before the pattern is considered too greedy.
const MAX_HOLE_TOKENS: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
enum Tok {
    /// Identifier, keyword or class name; compared ignoring case.
    Word(String),
    /// `$name`, compared exactly.
    Var(String),
    Num(String),
    /// A quoted string, quotes included.
    Str(String),
    /// Punctuation or operator.
    Op(String),
    /// `$Name$` in a pattern: any balanced run of tokens.
    Hole(String),
}

#[derive(Debug, Clone)]
struct Token {
    tok: Tok,
    start: usize,
    end: usize,
}

fn is_word_start(c: char) -> bool {
    c.is_alphabetic() || c == '_' || c == '\\'
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '\\'
}

/// Longest first, so `?->` wins over `?` and `->`.
const OPERATORS: &[&str] = &[
    "?->", "<=>", "===", "!==", "**=", "...", "<<=", ">>=", "??=", "->", "::", "=>", "==", "!=",
    "<=", ">=", "&&", "||", "??", "+=", "-=", "*=", "/=", "%=", ".=", "&=", "|=", "^=", "**",
    "++", "--", "<<", ">>", "<?php", "?>",
];

/// `$Name$` starting at `at`, if there is one.
fn hole_at(source: &str, at: usize) -> Option<(String, usize)> {
    let rest = &source[at + 1..];
    let mut chars = rest.char_indices();
    let (_, first) = chars.next()?;
    if !(first.is_alphabetic() || first == '_') {
        return None;
    }
    for (ix, c) in chars {
        if c == '$' {
            return Some((rest[..ix].to_string(), at + 1 + ix + 1));
        }
        if !(c.is_alphanumeric() || c == '_') {
            return None;
        }
    }
    None
}

fn skip_string(source: &str, at: usize, quote: char) -> usize {
    let mut ix = at + quote.len_utf8();
    let bytes = source.as_bytes();
    while ix < bytes.len() {
        let c = source[ix..].chars().next().unwrap_or(' ');
        if c == '\\' {
            ix += c.len_utf8() + source[ix + c.len_utf8()..].chars().next().map_or(0, char::len_utf8);
            continue;
        }
        if c == quote {
            return ix + c.len_utf8();
        }
        ix += c.len_utf8();
    }
    source.len()
}

fn read_heredoc(source: &str, at: usize) -> usize {
    let rest = &source[at + 3..];
    let label: String = rest
        .trim_start()
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    if label.is_empty() {
        return at + 3;
    }
    let terminator = format!("\n{label}");
    match source[at + 3..].find(&terminator) {
        Some(found) => {
            let line_end = at + 3 + found + terminator.len();
            let after = source[line_end..].find('\n').map_or(source.len(), |e| line_end + e + 1);
            after
        }
        None => source.len(),
    }
}

fn tokenize(source: &str, pattern: bool) -> Vec<Token> {
    let mut out = Vec::new();
    let mut ix = 0;
    while ix < source.len() {
        let c = source[ix..].chars().next().unwrap_or(' ');
        let next = source[ix + c.len_utf8()..].chars().next();
        if c.is_whitespace() {
            ix += c.len_utf8();
            continue;
        }
        if c == '/' && next == Some('/') {
            ix = source[ix..].find('\n').map_or(source.len(), |e| ix + e + 1);
            continue;
        }
        if c == '#' && next != Some('[') {
            ix = source[ix..].find('\n').map_or(source.len(), |e| ix + e + 1);
            continue;
        }
        if c == '/' && next == Some('*') {
            ix = source[ix + 2..].find("*/").map_or(source.len(), |e| ix + 2 + e + 2);
            continue;
        }
        let start = ix;
        if c == '\'' || c == '"' {
            ix = skip_string(source, ix, c);
            out.push(Token { tok: Tok::Str(source[start..ix].to_string()), start, end: ix });
            continue;
        }
        if source[start..].starts_with("<<<") {
            ix = read_heredoc(source, start);
            out.push(Token { tok: Tok::Str(source[start..ix].to_string()), start, end: ix });
            continue;
        }
        if c == '$' {
            if pattern
                && let Some((name, end)) = hole_at(source, start)
            {
                out.push(Token { tok: Tok::Hole(name), start, end });
                ix = end;
                continue;
            }
            ix += 1;
            while ix < source.len() {
                let Some(ch) = source[ix..].chars().next() else { break };
                if !is_word_char(ch) {
                    break;
                }
                ix += ch.len_utf8();
            }
            out.push(Token { tok: Tok::Var(source[start..ix].to_string()), start, end: ix });
            continue;
        }
        if is_word_start(c) {
            ix += c.len_utf8();
            while ix < source.len() {
                let Some(ch) = source[ix..].chars().next() else { break };
                if !is_word_char(ch) {
                    break;
                }
                ix += ch.len_utf8();
            }
            out.push(Token { tok: Tok::Word(source[start..ix].to_string()), start, end: ix });
            continue;
        }
        if c.is_ascii_digit() {
            ix += 1;
            while ix < source.len() {
                let Some(ch) = source[ix..].chars().next() else { break };
                if !(ch.is_ascii_alphanumeric() || ch == '.' || ch == '_') {
                    break;
                }
                ix += ch.len_utf8();
            }
            out.push(Token { tok: Tok::Num(source[start..ix].to_string()), start, end: ix });
            continue;
        }
        let op = OPERATORS
            .iter()
            .find(|op| source[start..].starts_with(**op))
            .map(|op| (*op).to_string())
            .unwrap_or_else(|| c.to_string());
        ix += op.len();
        out.push(Token { tok: Tok::Op(op), start, end: ix });
    }
    out
}

/// A compiled structural pattern.
#[derive(Debug, Clone)]
pub struct Pattern {
    tokens: Vec<Tok>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub start: usize,
    pub end: usize,
    /// Hole name and the byte range it matched.
    pub captures: Vec<(String, Range<usize>)>,
}

impl Pattern {
    pub fn compile(pattern: &str) -> Result<Self, String> {
        let tokens: Vec<Tok> = tokenize(pattern, true).into_iter().map(|t| t.tok).collect();
        if tokens.is_empty() {
            return Err("The pattern is empty.".to_string());
        }
        if tokens.len() < 2 && matches!(tokens.first(), Some(Tok::Hole(_))) {
            return Err("A pattern needs something besides a hole, e.g. `$A$->save()'.".to_string());
        }
        Ok(Self { tokens })
    }

    /// Hole names in the order they first appear, for the replacement hint.
    pub fn holes(&self) -> Vec<&str> {
        let mut names: Vec<&str> = Vec::new();
        for tok in &self.tokens {
            if let Tok::Hole(name) = tok
                && !names.contains(&name.as_str())
            {
                names.push(name);
            }
        }
        names
    }

    pub fn find(&self, source: &str, limit: usize) -> Vec<Hit> {
        let tokens = tokenize(source, false);
        let mut hits = Vec::new();
        let mut at = 0;
        while at < tokens.len() {
            let mut captures = Vec::new();
            match step(&self.tokens, 0, &tokens, at, source, &mut captures) {
                Some(end) => {
                    hits.push(Hit {
                        start: tokens[at].start,
                        end: tokens[end - 1].end,
                        captures,
                    });
                    if hits.len() >= limit {
                        break;
                    }
                    at = end;
                }
                None => at += 1,
            }
        }
        hits
    }
}

/// End token indices a hole starting at `ti` may take: balanced, and never past a
/// statement end or an open/close tag.
fn hole_spans(tokens: &[Token], ti: usize) -> Vec<usize> {
    let mut ends = Vec::new();
    let mut depth = 0isize;
    let last = (ti + MAX_HOLE_TOKENS).min(tokens.len());
    for end in ti + 1..=last {
        match &tokens[end - 1].tok {
            Tok::Op(op) if matches!(op.as_str(), "(" | "[" | "{") => depth += 1,
            Tok::Op(op) if matches!(op.as_str(), ")" | "]" | "}") => {
                depth -= 1;
                if depth < 0 {
                    return ends;
                }
            }
            Tok::Op(op) if depth == 0 && matches!(op.as_str(), ";" | "<?php" | "?>") => return ends,
            _ => {}
        }
        if depth == 0 {
            ends.push(end);
        }
    }
    ends
}

/// Matches `pat[pi..]` against `tokens[ti..]`; returns the first token index after the match.
fn step(
    pat: &[Tok],
    pi: usize,
    tokens: &[Token],
    ti: usize,
    source: &str,
    captures: &mut Vec<(String, Range<usize>)>,
) -> Option<usize> {
    if pi == pat.len() {
        return Some(ti);
    }
    let Some(token) = tokens.get(ti) else { return None };
    match &pat[pi] {
        Tok::Hole(name) => {
            for end in hole_spans(tokens, ti) {
                let text = &source[tokens[ti].start..tokens[end - 1].end];
                // A hole named twice stands for the same text both times.
                if let Some((_, range)) = captures.iter().find(|(n, _)| n == name)
                    && text != &source[range.clone()]
                {
                    continue;
                }
                let known = captures.iter().any(|(n, _)| n == name);
                if !known {
                    captures.push((name.clone(), tokens[ti].start..tokens[end - 1].end));
                }
                if let Some(found) = step(pat, pi + 1, tokens, end, source, captures) {
                    return Some(found);
                }
                if !known {
                    captures.pop();
                }
            }
            None
        }
        want => {
            let same = match (want, &token.tok) {
                (Tok::Word(a), Tok::Word(b)) => a.eq_ignore_ascii_case(b),
                (Tok::Var(a), Tok::Var(b)) => a == b,
                (Tok::Num(a), Tok::Num(b)) => a == b,
                (Tok::Str(a), Tok::Str(b)) => a == b,
                (Tok::Op(a), Tok::Op(b)) => a == b,
                _ => false,
            };
            same.then(|| step(pat, pi + 1, tokens, ti + 1, source, captures))?
        }
    }
}

/// Replacement text for one hit: `$Name$` is replaced by what the hole matched.
pub fn expand(hit: &Hit, source: &str, replacement: &str) -> String {
    let mut out = String::new();
    let mut at = 0;
    while at < replacement.len() {
        let c = replacement[at..].chars().next().unwrap_or(' ');
        if c == '$'
            && let Some((name, end)) = hole_at(replacement, at)
        {
            match hit.captures.iter().find(|(n, _)| *n == name) {
                Some((_, range)) => out.push_str(&source[range.clone()]),
                None => out.push_str(&replacement[at..end]),
            }
            at = end;
            continue;
        }
        out.push(c);
        at += c.len_utf8();
    }
    out
}

/// Replaces every hit; returns the new text and how many hits there were.
pub fn replace(pattern: &Pattern, source: &str, replacement: &str) -> (String, usize) {
    let hits = pattern.find(source, usize::MAX);
    let mut out = String::with_capacity(source.len());
    let mut at = 0;
    for hit in &hits {
        out.push_str(&source[at..hit.start]);
        out.push_str(&expand(hit, source, replacement));
        at = hit.end;
    }
    out.push_str(&source[at..]);
    (out, hits.len())
}

/// Structural search over the project. One entry per hit; a multi-line hit is
/// reported at the line it starts on.
pub fn search_files(
    root: &Path,
    pattern: &Pattern,
    options: &FileSearch,
    limit: usize,
) -> Vec<LineMatch> {
    let mask = mask_matcher(root, &options.mask);
    let mut out = Vec::new();
    for path in list_files(root, options.include_vendor) {
        if let Some(mask) = &mask
            && !mask.matched(&path, false).is_whitelist()
        {
            continue;
        }
        if fs::metadata(&path).map(|m| m.len() > MAX_FILE_BYTES).unwrap_or(true) {
            continue;
        }
        let Ok(text) = fs::read_to_string(&path) else { continue };
        for hit in pattern.find(&text, limit.saturating_sub(out.len())) {
            let line_start = text[..hit.start].rfind('\n').map_or(0, |ix| ix + 1);
            let line_end = text[hit.start..].find('\n').map_or(text.len(), |ix| hit.start + ix);
            let line = text[line_start..line_end].to_string();
            let end = hit.end.min(line_end);
            out.push(LineMatch {
                path: path.clone(),
                line: text[..line_start].matches('\n').count(),
                text: line,
                ranges: vec![hit.start - line_start..end - line_start],
                offset: hit.start,
            });
            if out.len() >= limit {
                return out;
            }
        }
    }
    out
}

/// Structural replace across the given files. Returns (files changed, replacements made).
pub fn replace_in_files(
    paths: &[PathBuf],
    pattern: &Pattern,
    replacement: &str,
) -> Result<(usize, usize), String> {
    let mut files = 0;
    let mut total = 0;
    for path in paths {
        let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let (replaced, count) = replace(pattern, &text, replacement);
        if count > 0 {
            fs::write(path, replaced).map_err(|e| format!("{}: {e}", path.display()))?;
            files += 1;
            total += count;
        }
    }
    Ok((files, total))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hits(pattern: &str, source: &str) -> Vec<String> {
        let pattern = Pattern::compile(pattern).expect("pattern");
        pattern
            .find(source, usize::MAX)
            .into_iter()
            .map(|hit| source[hit.start..hit.end].to_string())
            .collect()
    }

    #[test]
    fn holes_match_calls_with_their_arguments() {
        let source = "<?php\n$query->where('id', 1);\n$query->where('id', 1)->first();\n";
        assert_eq!(
            hits("$A$->where($B$)", source),
            vec!["$query->where('id', 1)", "$query->where('id', 1)"]
        );
    }

    #[test]
    fn matching_ignores_layout_and_case() {
        let source = "<?php\n    self::where(\n        'id',\n        1,\n    );";
        assert_eq!(hits("Self :: where($A$)", source), vec!["self::where(\n        'id',\n        1,\n    )"]);
    }

    #[test]
    fn a_hole_must_match_the_same_text_twice() {
        assert_eq!(hits("$A$ === $A$", "$a === $a;"), vec!["$a === $a"]);
        assert!(hits("$A$ === $A$", "$a === $b;").is_empty());
    }

    #[test]
    fn a_hole_does_not_cross_a_statement() {
        assert!(hits("$A$ save()", "$a; save()").is_empty());
        assert_eq!(hits("$A$->save()", "$a = 1; $b->save();"), vec!["$b->save()"]);
    }

    #[test]
    fn comments_and_strings_are_tokens() {
        let source = "<?php\n// $user->save()\n$text = '$user->save()';\n$user->save();\n";
        assert_eq!(hits("$X$->save()", source), vec!["$user->save()"]);
    }

    #[test]
    fn replacement_fills_the_holes() {
        let pattern = Pattern::compile("$A$->where($B$)").expect("pattern");
        let (text, count) = replace(&pattern, "$q->where(1)->get();", "$A$->filter($B$)");
        assert_eq!((text.as_str(), count), ("$q->filter(1)->get();", 1));
    }

    #[test]
    fn replacement_keeps_unknown_holes() {
        let pattern = Pattern::compile("time()").expect("pattern");
        let (text, count) = replace(&pattern, "time();", "$B$/");
        assert_eq!((text.as_str(), count), ("$B$/;", 1));
    }
}
