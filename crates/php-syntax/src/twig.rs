//! Twig templates: HTML highlighting with the `{{ … }}`, `{% … %}` and `{# … #}` parts on top.

use crate::{Highlight, Span, highlight_html, overlay};

/// Tags and operators highlighted as keywords inside `{% %}` and `{{ }}`.
pub const KEYWORDS: &[&str] = &[
    "if", "elseif", "else", "endif", "for", "endfor", "in", "block", "endblock", "extends", "include", "embed", "endembed",
    "import", "from", "as", "macro", "endmacro", "set", "endset", "with", "only", "ignore", "missing", "use", "apply", "endapply",
    "autoescape", "endautoescape", "verbatim", "endverbatim", "do", "flush", "sandbox", "endsandbox", "deprecated", "guard", "endguard",
    "cache", "endcache", "trans", "endtrans", "not", "and", "or", "is", "defined", "empty", "null", "true", "false", "starts", "ends",
    "with", "matches", "b-and", "b-or", "b-xor", "same", "divisible", "by", "odd", "even", "iterable",
];

/// Twig's built-in filters, for completion after `|`.
pub const FILTERS: &[&str] = &[
    "abs", "batch", "capitalize", "column", "convert_encoding", "country_name", "currency_name", "currency_symbol", "data_uri",
    "date", "date_modify", "default", "e", "escape", "filter", "find", "first", "format", "format_currency", "format_date",
    "format_datetime", "format_number", "format_time", "html_to_markdown", "inky_to_html", "inline_css", "join", "json_encode",
    "keys", "language_name", "last", "length", "locale_name", "lower", "map", "markdown_to_html", "merge", "nl2br",
    "number_format", "plural", "raw", "reduce", "replace", "reverse", "round", "shuffle", "singular", "slice", "slug", "sort",
    "spaceless", "split", "striptags", "timezone_name", "title", "trans", "trim", "u", "upper", "url_encode",
];

fn push(out: &mut Vec<Span>, start: usize, end: usize, highlight: Highlight) {
    if end > start {
        out.push(Span { range: start..end, highlight });
    }
}

/// Spans for the Twig parts of `source`.
fn twig_spans(source: &str) -> Vec<Span> {
    let bytes = source.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i + 1 < bytes.len() {
        let open = &source[i..i + 2];
        let close = match open {
            "{#" => "#}",
            "{{" => "}}",
            "{%" => "%}",
            _ => {
                i += 1;
                continue;
            }
        };
        let end = source[i + 2..].find(close).map_or(source.len(), |e| i + 2 + e + 2);
        if open == "{#" {
            push(&mut out, i, end, Highlight::Comment);
            i = end;
            continue;
        }
        push(&mut out, i, i + 2, Highlight::Tag);
        let inner_end = end.saturating_sub(2).max(i + 2);
        let mut j = i + 2;
        let mut after_pipe = false;
        while j < inner_end {
            let c = bytes[j] as char;
            if c == '\'' || c == '"' {
                let close = source[j + 1..inner_end].find(c).map_or(inner_end, |e| j + 1 + e + 1);
                push(&mut out, j, close, Highlight::String);
                j = close;
            } else if c.is_ascii_digit() {
                let len = source[j..inner_end].find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(inner_end - j);
                push(&mut out, j, j + len, Highlight::Number);
                j += len;
            } else if c.is_alphabetic() || c == '_' {
                let len = source[j..inner_end].find(|c: char| !(c.is_alphanumeric() || c == '_')).unwrap_or(inner_end - j);
                let word = &source[j..j + len];
                let highlight = if after_pipe {
                    Highlight::Function
                } else if KEYWORDS.contains(&word) {
                    Highlight::Keyword
                } else if source[j + len..inner_end].trim_start().starts_with('(') {
                    Highlight::Function
                } else if j > 0 && bytes[j - 1] == b'.' {
                    Highlight::Property
                } else {
                    Highlight::Variable
                };
                push(&mut out, j, j + len, highlight);
                after_pipe = false;
                j += len;
            } else {
                if c == '|' {
                    after_pipe = true;
                } else if !c.is_whitespace() {
                    after_pipe = false;
                }
                j += 1;
            }
        }
        if end >= 2 && end <= source.len() && source[..end].ends_with(close) {
            push(&mut out, end - 2, end, Highlight::Tag);
        }
        i = end.max(i + 2);
    }
    out
}

pub(crate) fn highlight(source: &str) -> Vec<Span> {
    overlay(highlight_html(source), twig_spans(source))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn highlights_twig_parts() {
        let src = "<p>{% if user %}{{ user.name|upper }}{% endif %}{# note #}</p>";
        let spans = highlight(src);
        let kind = |needle: &str| spans.iter().find(|s| &src[s.range.clone()] == needle).map(|s| s.highlight);
        assert_eq!(kind("if"), Some(Highlight::Keyword));
        assert_eq!(kind("upper"), Some(Highlight::Function));
        assert_eq!(kind("name"), Some(Highlight::Property));
        assert_eq!(kind("{# note #}"), Some(Highlight::Comment));
        assert!(spans.windows(2).all(|w| w[0].range.end <= w[1].range.start), "sorted, no overlaps");
    }
}
