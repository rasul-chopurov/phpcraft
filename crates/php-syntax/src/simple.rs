//! Small hand-written highlighters for formats common in web projects that don't need a
//! full grammar: Laravel logs, Markdown, SQL, INI/TOML, shell, Dockerfile, ignore files.

use super::{Highlight, Span};

fn push(out: &mut Vec<Span>, start: usize, end: usize, highlight: Highlight) {
    if start < end {
        out.push(Span { range: start..end, highlight });
    }
}

/// Lines with their byte offsets (without line terminators).
fn lines(source: &str) -> impl Iterator<Item = (usize, &str)> {
    let mut at = 0;
    source.split_inclusive('\n').map(move |line| {
        let start = at;
        at += line.len();
        (start, line.trim_end_matches(['\n', '\r']))
    })
}

/// Quoted strings, `--`/`#`/`//` comments and numbers within one line, plus keyword matching.
fn tokens(line: &str, offset: usize, keywords: &[&str], case_insensitive: bool, comment: &[&str], out: &mut Vec<Span>) {
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let rest = &line[i..];
        if let Some(marker) = comment.iter().find(|m| rest.starts_with(**m)) {
            let _ = marker;
            push(out, offset + i, offset + line.len(), Highlight::Comment);
            return;
        }
        let c = bytes[i];
        if c == b'\'' || c == b'"' || c == b'`' {
            let end = line[i + 1..].find(c as char).map_or(line.len(), |e| i + 2 + e);
            push(out, offset + i, offset + end, Highlight::String);
            i = end;
            continue;
        }
        if c.is_ascii_digit() && (i == 0 || !bytes[i - 1].is_ascii_alphanumeric()) {
            let end = rest.find(|ch: char| !(ch.is_ascii_digit() || ch == '.')).map_or(line.len(), |e| i + e);
            push(out, offset + i, offset + end, Highlight::Number);
            i = end;
            continue;
        }
        if c.is_ascii_alphabetic() || c == b'_' {
            let end = rest.find(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_')).map_or(line.len(), |e| i + e);
            let word = &line[i..end];
            let is_keyword = if case_insensitive {
                keywords.iter().any(|k| k.eq_ignore_ascii_case(word))
            } else {
                keywords.contains(&word)
            };
            if is_keyword {
                push(out, offset + i, offset + end, Highlight::Keyword);
            }
            i = end;
            continue;
        }
        i += rest.chars().next().map_or(1, char::len_utf8);
    }
}

/// Laravel / Monolog logs: `[2024-05-01 12:00:00] production.ERROR: message {"context"}`
/// plus stack frames `#0 /path/File.php(42): Class->method()`.
pub fn log(source: &str) -> Vec<Span> {
    let mut out = Vec::new();
    for (at, line) in lines(source) {
        if line.starts_with('[') && let Some(close) = line.find(']') {
            push(&mut out, at, at + close + 1, Highlight::Comment);
            let rest = &line[close + 1..];
            if let Some(colon) = rest.find(':') {
                let channel = &rest[..colon];
                let level = channel.rsplit('.').next().unwrap_or("").trim().to_ascii_uppercase();
                let kind = match level.as_str() {
                    "EMERGENCY" | "ALERT" | "CRITICAL" | "ERROR" => Highlight::Error,
                    "WARNING" | "NOTICE" => Highlight::Warning,
                    "INFO" | "DEBUG" => Highlight::Info,
                    _ => Highlight::Keyword,
                };
                let start = at + close + 1;
                push(&mut out, start, start + colon + 1, kind);
                // Trailing JSON context.
                if let Some(brace) = rest.find(" {") {
                    push(&mut out, start + brace + 1, at + line.len(), Highlight::String);
                }
            }
            continue;
        }
        let trimmed = line.trim_start();
        let indent = line.len() - trimmed.len();
        if trimmed.starts_with('#') && trimmed[1..].starts_with(|c: char| c.is_ascii_digit()) {
            let num_end = trimmed.find(' ').unwrap_or(trimmed.len());
            push(&mut out, at + indent, at + indent + num_end, Highlight::Number);
            // `/path/File.php(42)`
            if let Some(paren) = trimmed.find(".php(") {
                let path_start = num_end + 1;
                push(&mut out, at + indent + path_start.min(paren), at + indent + paren + 4, Highlight::Function);
            }
            continue;
        }
        if trimmed.starts_with("Stack trace") || trimmed.starts_with("[stacktrace]") || trimmed.starts_with("[previous exception]") {
            push(&mut out, at + indent, at + line.len(), Highlight::Keyword);
        }
    }
    out
}

pub fn markdown(source: &str) -> Vec<Span> {
    let mut out = Vec::new();
    let mut in_fence = false;
    for (at, line) in lines(source) {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            push(&mut out, at, at + line.len(), Highlight::Comment);
            continue;
        }
        if in_fence {
            push(&mut out, at, at + line.len(), Highlight::String);
            continue;
        }
        if trimmed.starts_with('#') {
            push(&mut out, at, at + line.len(), Highlight::Keyword);
            continue;
        }
        if trimmed.starts_with('>') {
            push(&mut out, at, at + line.len(), Highlight::Comment);
            continue;
        }
        let indent = line.len() - trimmed.len();
        if trimmed.starts_with(['-', '*', '+']) && trimmed[1..].starts_with(' ') {
            push(&mut out, at + indent, at + indent + 1, Highlight::Number);
        }
        // Inline code and links.
        let bytes = line.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            match bytes[i] {
                b'`' => {
                    let end = line[i + 1..].find('`').map_or(line.len(), |e| i + e + 2);
                    push(&mut out, at + i, at + end, Highlight::String);
                    i = end;
                }
                b'[' => {
                    if let Some(close) = line[i..].find("](").map(|c| i + c)
                        && let Some(paren) = line[close..].find(')').map(|p| close + p)
                    {
                        push(&mut out, at + i, at + close + 1, Highlight::Function);
                        push(&mut out, at + close + 1, at + paren + 1, Highlight::Comment);
                        i = paren + 1;
                    } else {
                        i += 1;
                    }
                }
                b'*' if bytes.get(i + 1) == Some(&b'*') => {
                    let end = line[i + 2..].find("**").map_or(line.len(), |e| i + e + 4);
                    push(&mut out, at + i, at + end, Highlight::Constant);
                    i = end;
                }
                _ => i += 1,
            }
        }
    }
    out
}

const SQL_KEYWORDS: &[&str] = &[
    "select", "from", "where", "and", "or", "not", "insert", "into", "values", "update", "set", "delete", "create",
    "table", "drop", "alter", "add", "column", "index", "primary", "key", "foreign", "references", "join", "left",
    "right", "inner", "outer", "on", "as", "group", "by", "order", "having", "limit", "offset", "distinct", "union",
    "all", "null", "is", "in", "like", "between", "exists", "case", "when", "then", "else", "end", "default",
    "unique", "constraint", "if", "begin", "commit", "rollback", "transaction", "database", "view", "trigger",
    "unsigned", "auto_increment", "engine", "charset", "collate", "asc", "desc", "true", "false", "with", "returning",
];

pub fn sql(source: &str) -> Vec<Span> {
    let mut out = Vec::new();
    for (at, line) in lines(source) {
        tokens(line, at, SQL_KEYWORDS, true, &["--", "#"], &mut out);
    }
    out
}

/// INI / TOML / .editorconfig: `[section]`, `key = value`, `;`/`#` comments.
pub fn ini(source: &str) -> Vec<Span> {
    let mut out = Vec::new();
    for (at, line) in lines(source) {
        let trimmed = line.trim_start();
        let indent = line.len() - trimmed.len();
        if trimmed.starts_with([';', '#']) {
            push(&mut out, at + indent, at + line.len(), Highlight::Comment);
        } else if trimmed.starts_with('[') {
            push(&mut out, at + indent, at + line.len(), Highlight::Tag);
        } else if let Some(eq) = line.find(['=', ':']) {
            push(&mut out, at + indent, at + eq, Highlight::Property);
            let value = &line[eq + 1..];
            let value_start = eq + 1 + (value.len() - value.trim_start().len());
            let kind = match value.trim().to_ascii_lowercase().as_str() {
                v if v.parse::<f64>().is_ok() => Highlight::Number,
                "true" | "false" | "on" | "off" | "yes" | "no" => Highlight::Keyword,
                _ => Highlight::String,
            };
            push(&mut out, at + value_start, at + line.len(), kind);
        }
    }
    out
}

const SHELL_KEYWORDS: &[&str] = &[
    "if", "then", "else", "elif", "fi", "for", "in", "do", "done", "while", "until", "case", "esac", "function",
    "return", "export", "local", "readonly", "set", "unset", "source", "exit", "echo", "cd",
];

pub fn shell(source: &str) -> Vec<Span> {
    let mut out = Vec::new();
    for (at, line) in lines(source) {
        tokens(line, at, SHELL_KEYWORDS, false, &["#"], &mut out);
        // $VAR and ${VAR}
        let bytes = line.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'$' {
                let rest = &line[i + 1..];
                let len = if rest.starts_with('{') {
                    rest.find('}').map_or(rest.len(), |e| e + 1)
                } else {
                    rest.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).unwrap_or(rest.len())
                };
                if len > 0 {
                    push(&mut out, at + i, at + i + 1 + len, Highlight::Variable);
                }
                i += 1 + len;
            } else {
                i += 1;
            }
        }
    }
    out.sort_by_key(|s| s.range.start);
    // Variables found inside strings overlap them; keep the first span at each position.
    let mut merged: Vec<Span> = Vec::new();
    for span in out {
        if merged.last().is_none_or(|last| span.range.start >= last.range.end) {
            merged.push(span);
        }
    }
    merged
}

const DOCKER_KEYWORDS: &[&str] = &[
    "FROM", "RUN", "CMD", "COPY", "ADD", "ENV", "ARG", "EXPOSE", "WORKDIR", "ENTRYPOINT", "USER", "VOLUME", "LABEL",
    "HEALTHCHECK", "SHELL", "STOPSIGNAL", "ONBUILD", "AS",
];

pub fn dockerfile(source: &str) -> Vec<Span> {
    let mut out = Vec::new();
    for (at, line) in lines(source) {
        tokens(line, at, DOCKER_KEYWORDS, true, &["#"], &mut out);
    }
    out
}

/// `.gitignore`-style files: comments and negations.
pub fn ignore_file(source: &str) -> Vec<Span> {
    let mut out = Vec::new();
    for (at, line) in lines(source) {
        if line.trim_start().starts_with('#') {
            push(&mut out, at, at + line.len(), Highlight::Comment);
        } else if line.starts_with('!') {
            push(&mut out, at, at + 1, Highlight::Keyword);
        }
    }
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
    fn laravel_log() {
        let src = "[2026-11-01 10:00:00] local.ERROR: Boom {\"exception\":\"x\"}\n[stacktrace]\n#0 /app/Http/Kernel.php(42): handle()\n[2026-11-01 10:00:01] local.INFO: ok\n";
        let spans = log(src);
        assert_eq!(kind_in(&spans, src, "[2026"), Some(Highlight::Comment));
        assert_eq!(kind_in(&spans, src, "local.ERROR"), Some(Highlight::Error));
        assert_eq!(kind_in(&spans, src, "{\"exception"), Some(Highlight::String));
        assert_eq!(kind_in(&spans, src, "#0"), Some(Highlight::Number));
        assert_eq!(kind_in(&spans, src, "/app/Http"), Some(Highlight::Function));
        assert_eq!(kind_in(&spans, src, "local.INFO"), Some(Highlight::Info));
    }

    #[test]
    fn others() {
        let md = "# Title\nSome `code` and [link](http://x).\n```php\necho 1;\n```\n";
        let spans = markdown(md);
        assert_eq!(kind_in(&spans, md, "# Title"), Some(Highlight::Keyword));
        assert_eq!(kind_in(&spans, md, "`code`"), Some(Highlight::String));
        assert_eq!(kind_in(&spans, md, "[link]"), Some(Highlight::Function));
        assert_eq!(kind_in(&spans, md, "echo"), Some(Highlight::String));

        let q = "SELECT id FROM users WHERE name = 'a' -- note\n";
        let spans = sql(q);
        assert_eq!(kind_in(&spans, q, "SELECT"), Some(Highlight::Keyword));
        assert_eq!(kind_in(&spans, q, "'a'"), Some(Highlight::String));
        assert_eq!(kind_in(&spans, q, "-- note"), Some(Highlight::Comment));

        let i = "; c\n[PHP]\nmemory_limit = 128M\ndisplay_errors = On\n";
        let spans = ini(i);
        assert_eq!(kind_in(&spans, i, "[PHP]"), Some(Highlight::Tag));
        assert_eq!(kind_in(&spans, i, "memory_limit"), Some(Highlight::Property));
        assert_eq!(kind_in(&spans, i, "On"), Some(Highlight::Keyword));

        let sh = "#!/bin/sh\nif [ -n \"$HOME\" ]; then echo $USER; fi\n";
        let spans = shell(sh);
        assert_eq!(kind_in(&spans, sh, "if"), Some(Highlight::Keyword));
        assert_eq!(kind_in(&spans, sh, "$USER"), Some(Highlight::Variable));

        let docker = "FROM php:8.4-fpm\nRUN apt-get update # deps\n";
        assert_eq!(kind_in(&dockerfile(docker), docker, "FROM"), Some(Highlight::Keyword));
    }
}
