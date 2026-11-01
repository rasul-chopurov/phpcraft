//! Blade templates: HTML highlighting with Blade constructs laid on top — `{{ }}`, `{!! !!}`,
//! `{{-- --}}` comments, `@directives(...)` and `@php ... @endphp` — whose PHP parts are
//! highlighted with the PHP grammar.

use super::{Highlight, Span, highlight_html, highlight_php_fragment, offset, overlay};

pub fn highlight(source: &str) -> Vec<Span> {
    overlay(highlight_html(source), blade_spans(source))
}

fn keyword(range: std::ops::Range<usize>) -> Span {
    Span { range, highlight: Highlight::Keyword }
}

fn php(source: &str, range: std::ops::Range<usize>, out: &mut Vec<Span>) {
    if range.start < range.end {
        out.extend(offset(highlight_php_fragment(&source[range.clone()]), range.start));
    }
}

/// End of a balanced `( ... )` starting at `open`, skipping quoted strings.
fn matching_paren(source: &str, open: usize) -> Option<usize> {
    let bytes = source.as_bytes();
    let mut depth = 0;
    let mut quote: Option<u8> = None;
    let mut i = open;
    while i < bytes.len() {
        let b = bytes[i];
        match quote {
            Some(_) if b == b'\\' => i += 1,
            Some(q) if b == q => quote = None,
            Some(_) => {}
            None => match b {
                b'\'' | b'"' => quote = Some(b),
                b'(' => depth += 1,
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(i);
                    }
                }
                _ => {}
            },
        }
        i += 1;
    }
    None
}

fn blade_spans(source: &str) -> Vec<Span> {
    let bytes = source.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let rest = &source[i..];
        if rest.starts_with("{{--") {
            let end = rest.find("--}}").map_or(source.len(), |e| i + e + 4);
            out.push(Span { range: i..end, highlight: Highlight::Comment });
            i = end;
        } else if rest.starts_with("@{{") {
            // Escaped: rendered literally by Blade.
            i += 3;
        } else if let Some((open, close)) = [("{!!", "!!}"), ("{{", "}}")].into_iter().find(|(o, _)| rest.starts_with(o)) {
            let inner_start = i + open.len();
            let Some(end) = source[inner_start..].find(close).map(|e| inner_start + e) else {
                out.push(keyword(i..inner_start));
                break;
            };
            out.push(keyword(i..inner_start));
            php(source, inner_start..end, &mut out);
            out.push(keyword(end..end + close.len()));
            i = end + close.len();
        } else if bytes[i] == b'@'
            && bytes.get(i + 1).is_some_and(|b| b.is_ascii_alphabetic())
            && (i == 0 || !(bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'.' || bytes[i - 1] == b'@'))
        {
            let name_end = rest[1..]
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .map_or(source.len(), |e| i + 1 + e);
            let name = &source[i + 1..name_end];
            out.push(keyword(i..name_end));
            let after = name_end + (source[name_end..].len() - source[name_end..].trim_start_matches([' ', '\t']).len());
            if source[after..].starts_with('(') {
                if let Some(close) = matching_paren(source, after) {
                    php(source, after + 1..close, &mut out);
                    i = close + 1;
                    continue;
                }
            } else if name == "php" {
                let body_end = source[name_end..].find("@endphp").map_or(source.len(), |e| name_end + e);
                php(source, name_end..body_end, &mut out);
                if body_end < source.len() {
                    out.push(keyword(body_end..body_end + 7));
                }
                i = (body_end + 7).min(source.len());
                continue;
            }
            i = name_end;
        } else {
            i += rest.chars().next().map_or(1, char::len_utf8);
        }
    }
    out.sort_by_key(|s| s.range.start);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kind_in(spans: &[Span], source: &str, needle: &str) -> Option<Highlight> {
        let start = source.find(needle).unwrap();
        spans.iter().find(|s| s.range.start <= start && start < s.range.end).map(|s| s.highlight)
    }

    #[test]
    fn blade_constructs() {
        let src = "{{-- note --}}\n<div class=\"card\">\n  @foreach($users as $user)\n    {{ $user->name }} {!! $html !!}\n  @endforeach\n  @php $x = 1; @endphp\n  mail@example.com\n</div>\n";
        let spans = highlight(src);
        for pair in spans.windows(2) {
            assert!(pair[0].range.end <= pair[1].range.start, "{pair:?}");
        }
        assert_eq!(kind_in(&spans, src, "{{-- note"), Some(Highlight::Comment));
        assert_eq!(kind_in(&spans, src, "div"), Some(Highlight::Tag));
        assert_eq!(kind_in(&spans, src, "class"), Some(Highlight::Attribute));
        assert_eq!(kind_in(&spans, src, "@foreach"), Some(Highlight::Keyword));
        assert_eq!(kind_in(&spans, src, "$users"), Some(Highlight::Variable));
        assert_eq!(kind_in(&spans, src, "{{ $user"), Some(Highlight::Keyword));
        assert_eq!(kind_in(&spans, src, "$user->name"), Some(Highlight::Variable));
        assert_eq!(kind_in(&spans, src, "name }}"), Some(Highlight::Property));
        assert_eq!(kind_in(&spans, src, "@endforeach"), Some(Highlight::Keyword));
        assert_eq!(kind_in(&spans, src, "$x"), Some(Highlight::Variable));
        assert_eq!(kind_in(&spans, src, "@example"), None, "emails are not directives");
    }
}
