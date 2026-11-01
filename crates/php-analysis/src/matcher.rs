//! Completion matching in the style of IntelliJ's `CamelHumpMatcher`: the typed text
//! matches a prefix of the name, or the starts of its "humps" in order — `gGSEQ`, `getGSE`
//! and `ggseq` all match `getGlobalSearchEloquentQuery`. Matching ignores case; the first
//! letter's case only affects ranking (as with the default "first letter" setting).

use std::ops::Range;

/// Positions (char indices) where a new word starts: the first char, an uppercase letter
/// after a lowercase one or a digit, and any char after `_`, `\`, `-`, `$` or `.`.
fn hump_starts(chars: &[char]) -> Vec<bool> {
    let mut starts = vec![false; chars.len()];
    for i in 0..chars.len() {
        starts[i] = i == 0
            || matches!(chars[i - 1], '_' | '\\' | '-' | '$' | '.' | ':')
            || (chars[i].is_uppercase() && !chars[i - 1].is_uppercase())
            || (chars[i].is_ascii_digit() != chars[i - 1].is_ascii_digit());
    }
    starts
}

fn eq(a: char, b: char) -> bool {
    a == b || a.to_lowercase().eq(b.to_lowercase())
}

/// Recursive matcher with backtracking: `t` typed chars from `l` in the name, where the
/// previous typed char matched at `l - 1` (so continuing the fragment is allowed).
fn match_from(name: &[char], starts: &[bool], typed: &[char], t: usize, l: usize, continuing: bool, out: &mut Vec<usize>, budget: &mut u32) -> bool {
    if t == typed.len() {
        return true;
    }
    if *budget == 0 {
        return false;
    }
    *budget -= 1;
    // Continue the current fragment (lowercase or same-case typing).
    if continuing && l < name.len() && eq(typed[t], name[l]) && !(typed[t].is_uppercase() && !starts[l]) {
        out.push(l);
        if match_from(name, starts, typed, t + 1, l + 1, true, out, budget) {
            return true;
        }
        out.pop();
    }
    // Jump to a later hump.
    for j in l..name.len() {
        if starts[j] && eq(typed[t], name[j]) && !(continuing && j == l) {
            out.push(j);
            if match_from(name, starts, typed, t + 1, j + 1, true, out, budget) {
                return true;
            }
            out.pop();
        }
    }
    false
}

/// Char indices of `name` matched by `typed`, or `None`.
pub fn match_positions(name: &str, typed: &str) -> Option<Vec<usize>> {
    let typed: Vec<char> = typed.trim_start_matches('$').chars().collect();
    let offset = name.chars().take_while(|c| *c == '$').count();
    let name: Vec<char> = name.chars().skip(offset).collect();
    if typed.is_empty() {
        return Some(Vec::new());
    }
    if typed.len() > name.len() {
        return None;
    }
    // The first typed char must match the start of the name.
    if !eq(typed[0], name[0]) {
        return None;
    }
    let starts = hump_starts(&name);
    let mut out = vec![0];
    let mut budget = 2000;
    match_from(&name, &starts, &typed, 1, 1, true, &mut out, &mut budget).then(|| out.into_iter().map(|i| i + offset).collect())
}

pub fn matches(name: &str, typed: &str) -> bool {
    match_positions(name, typed).is_some()
}

/// Matched chars as byte ranges of `name`, merged into runs (for bold highlighting).
pub fn match_ranges(name: &str, typed: &str) -> Vec<Range<usize>> {
    let Some(positions) = match_positions(name, typed) else { return Vec::new() };
    let byte_of: Vec<(usize, char)> = name.char_indices().collect();
    let mut ranges: Vec<Range<usize>> = Vec::new();
    for p in positions {
        let Some(&(start, c)) = byte_of.get(p) else { continue };
        let end = start + c.len_utf8();
        match ranges.last_mut() {
            Some(last) if last.end == start => last.end = end,
            _ => ranges.push(start..end),
        }
    }
    ranges
}

/// How well `typed` matches, lower is better: exact-case prefix, case-insensitive prefix,
/// then hump matches with fewer fragments first.
pub fn degree(name: &str, typed: &str) -> u32 {
    let bare = name.trim_start_matches('$');
    let typed = typed.trim_start_matches('$');
    if typed.is_empty() || bare.starts_with(typed) {
        return 0;
    }
    if bare.to_lowercase().starts_with(&typed.to_lowercase()) {
        return 1;
    }
    // First-letter case differs from what was typed (IntelliJ's FIRST_LETTER sensitivity).
    let first_case_penalty = match (bare.chars().next(), typed.chars().next()) {
        (Some(a), Some(b)) if a != b => 10,
        _ => 0,
    };
    let fragments = match_ranges(name, typed).len() as u32;
    2 + fragments + first_case_penalty
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn camel_humps() {
        let name = "getGlobalSearchEloquentQuery";
        for typed in ["get", "getG", "gGSEQ", "getGSE", "ggseq", "getGlobalSearch", "gsq"] {
            assert!(matches(name, typed), "{typed}");
        }
        for typed in ["etG", "gx", "getGlobalSearchEloquentQueryX", "Q"] {
            assert!(!matches(name, typed), "{typed}");
        }
        assert!(matches("TextInput", "TI"));
        assert!(matches("TextInput", "tein"));
        assert!(matches("HasManyThrough", "hmt"));
        assert!(matches("$users", "us"));
        assert!(matches("snake_case_name", "scn"));
        assert_eq!(match_ranges("getGlobalSearch", "gGS"), [0..1, 3..4, 9..10]);
        assert_eq!(match_ranges("getGlobalSearch", "getGl"), [0..5]);
        assert_eq!(match_ranges("$users", "us"), [1..3]);
    }

    #[test]
    fn degrees() {
        assert!(degree("getName", "get") < degree("GetName", "get"));
        assert!(degree("GetName", "get") < degree("getGlobalName", "gGN"));
        assert!(degree("getGlobalName", "gGN") < degree("getGlobalNameX", "gGlN") + 1);
    }
}
