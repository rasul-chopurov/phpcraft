//! Dotted key paths of a PHP array literal file (`config/*.php`, `lang/*/*.php`).

use std::ops::Range;

/// Returns `(dotted.key, byte range of the key's contents)` for every `'key' => ...`
/// entry, tracking nesting through `[` / `]` and `array(` / `)`.
pub fn array_keys(source: &str) -> Vec<(String, Range<usize>)> {
    let bytes = source.as_bytes();
    let mut out = Vec::new();
    // One entry per open bracket: the key that opened it, if any.
    let mut stack: Vec<Option<String>> = Vec::new();
    let mut pending: Option<(String, Range<usize>)> = None;
    let mut awaiting_value: Option<String> = None;
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i] as char;
        match c {
            '\'' | '"' => {
                let start = i + 1;
                let mut j = start;
                while j < bytes.len() && bytes[j] as char != c {
                    j += if bytes[j] == b'\\' { 2 } else { 1 };
                }
                let end = j.min(bytes.len());
                pending = Some((source[start..end].to_string(), start..end));
                i = end + 1;
                continue;
            }
            '/' if bytes.get(i + 1) == Some(&b'/') => {
                i = source[i..].find('\n').map_or(bytes.len(), |n| i + n);
                continue;
            }
            '#' => {
                i = source[i..].find('\n').map_or(bytes.len(), |n| i + n);
                continue;
            }
            '/' if bytes.get(i + 1) == Some(&b'*') => {
                i = source[i..].find("*/").map_or(bytes.len(), |n| i + n + 2);
                continue;
            }
            '=' if bytes.get(i + 1) == Some(&b'>') => {
                if let Some((key, range)) = pending.take() {
                    let path = stack
                        .iter()
                        .flatten()
                        .cloned()
                        .chain(std::iter::once(key))
                        .collect::<Vec<_>>()
                        .join(".");
                    out.push((path.clone(), range));
                    awaiting_value = Some(path);
                }
                i += 2;
                continue;
            }
            '[' | '(' => {
                let opened_by = awaiting_value
                    .take()
                    .map(|path| path.rsplit('.').next().unwrap_or(&path).to_string());
                stack.push(opened_by);
            }
            ']' | ')' => {
                stack.pop();
                awaiting_value = None;
            }
            c if c.is_whitespace() => {}
            ',' => {
                awaiting_value = None;
                pending = None;
            }
            _ => {
                // `array(` keeps the awaited key; any other token means a scalar value.
                if !source[i..].starts_with("array") {
                    awaiting_value = None;
                } else {
                    i += 5;
                    continue;
                }
            }
        }
        i += 1;
    }
    // The outermost `return [ ... ]` bracket has no key; paths are already relative to it.
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_keys() {
        let src = "<?php\n// comment 'x' => 1\nreturn [\n    'name' => env('APP_NAME', 'Laravel'),\n    'mail' => [\n        'from' => ['address' => 'a@b.c'],\n        'driver' => 'smtp',\n    ],\n    'old' => array('k' => 1),\n];\n";
        let keys: Vec<_> = array_keys(src).into_iter().map(|(k, _)| k).collect();
        assert_eq!(keys, ["name", "mail", "mail.from", "mail.from.address", "mail.driver", "old", "old.k"]);
    }
}
