//! Text search: find/replace in a buffer, find/replace in files, and fuzzy matching for
//! Go to File / Class / Symbol.

use std::fs;
use std::ops::Range;
use std::path::{Path, PathBuf};

use regex::{Regex, RegexBuilder};

/// Files larger than this are skipped by Find in Files.
const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TextQuery {
    pub pattern: String,
    pub regex: bool,
    pub case_sensitive: bool,
    pub whole_word: bool,
}

impl TextQuery {
    pub fn new(pattern: impl Into<String>) -> Self {
        Self { pattern: pattern.into(), ..Self::default() }
    }

    pub fn compile(&self) -> Result<Regex, String> {
        let mut pattern = if self.regex { self.pattern.clone() } else { regex::escape(&self.pattern) };
        if self.whole_word {
            pattern = format!(r"\b(?:{pattern})\b");
        }
        RegexBuilder::new(&pattern)
            .case_insensitive(!self.case_sensitive)
            .multi_line(true)
            .build()
            .map_err(|e| e.to_string())
    }

    /// Byte ranges of all non-empty matches.
    pub fn find_all(&self, text: &str) -> Result<Vec<Range<usize>>, String> {
        if self.pattern.is_empty() {
            return Ok(Vec::new());
        }
        let regex = self.compile()?;
        Ok(regex.find_iter(text).map(|m| m.range()).filter(|r| !r.is_empty()).collect())
    }

    /// The replacement text for one match (expands `$1` / `${name}` in regex mode).
    pub fn expand(&self, matched: &str, replacement: &str) -> String {
        if !self.regex {
            return replacement.to_string();
        }
        let Ok(regex) = self.compile() else { return replacement.to_string() };
        match regex.captures(matched) {
            Some(caps) => {
                let mut out = String::new();
                caps.expand(replacement, &mut out);
                out
            }
            None => replacement.to_string(),
        }
    }

    /// Replaces every match; returns the new text and the number of replacements.
    pub fn replace_all(&self, text: &str, replacement: &str) -> Result<(String, usize), String> {
        if self.pattern.is_empty() {
            return Ok((text.to_string(), 0));
        }
        let regex = self.compile()?;
        let count = regex.find_iter(text).filter(|m| !m.is_empty()).count();
        let replaced = if self.regex {
            regex.replace_all(text, replacement).into_owned()
        } else {
            regex.replace_all(text, regex::NoExpand(replacement)).into_owned()
        };
        Ok((replaced, count))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineMatch {
    pub path: PathBuf,
    /// 0-based line number.
    pub line: usize,
    /// The line's text (trimmed of the trailing newline).
    pub text: String,
    /// Match ranges within `text`.
    pub ranges: Vec<Range<usize>>,
    /// Byte offset of the first match in the file.
    pub offset: usize,
}

#[derive(Debug, Clone, Default)]
pub struct FileSearch {
    /// Comma-separated globs like `*.php, *.blade.php`; empty means all files.
    pub mask: String,
    /// Also search `vendor/` (normally git-ignored).
    pub include_vendor: bool,
    pub max_results: usize,
}

fn mask_matcher(root: &Path, mask: &str) -> Option<ignore::overrides::Override> {
    let globs: Vec<&str> = mask.split(',').map(str::trim).filter(|g| !g.is_empty()).collect();
    if globs.is_empty() {
        return None;
    }
    let mut builder = ignore::overrides::OverrideBuilder::new(root);
    for glob in globs {
        builder.add(glob).ok()?;
    }
    builder.build().ok()
}

/// Project files (respecting `.gitignore`), relative paths sorted.
pub fn list_files(root: &Path, include_vendor: bool) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut walk = |dir: &Path, gitignore: bool| {
        for entry in ignore::WalkBuilder::new(dir).hidden(false).git_ignore(gitignore).require_git(false).build().flatten() {
            let path = entry.path();
            let skip = path.components().any(|c| matches!(c.as_os_str().to_str(), Some(".git" | "node_modules")));
            if entry.file_type().is_some_and(|t| t.is_file()) && !skip {
                files.push(path.to_path_buf());
            }
        }
    };
    walk(root, true);
    let vendor = root.join("vendor");
    if include_vendor && vendor.is_dir() {
        walk(&vendor, false);
    }
    files.sort();
    files.dedup();
    files
}

/// Find in Files. Results are grouped by file in path order, one entry per matching line.
pub fn search_files(root: &Path, query: &TextQuery, options: &FileSearch) -> Result<Vec<LineMatch>, String> {
    if query.pattern.is_empty() {
        return Ok(Vec::new());
    }
    let regex = query.compile()?;
    let mask = mask_matcher(root, &options.mask);
    let limit = if options.max_results == 0 { usize::MAX } else { options.max_results };
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
        let mut line_start = 0;
        for (line, content) in text.split_inclusive('\n').enumerate() {
            let trimmed = content.trim_end_matches(['\n', '\r']);
            let ranges: Vec<Range<usize>> = regex.find_iter(trimmed).map(|m| m.range()).filter(|r| !r.is_empty()).collect();
            if let Some(first) = ranges.first() {
                out.push(LineMatch {
                    path: path.clone(),
                    line,
                    text: trimmed.to_string(),
                    offset: line_start + first.start,
                    ranges,
                });
                if out.len() >= limit {
                    return Ok(out);
                }
            }
            line_start += content.len();
        }
    }
    Ok(out)
}

/// Replace in Files for the given files. Returns (files changed, replacements made).
pub fn replace_in_files(paths: &[PathBuf], query: &TextQuery, replacement: &str) -> Result<(usize, usize), String> {
    let mut files = 0;
    let mut total = 0;
    for path in paths {
        let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let (replaced, count) = query.replace_all(&text, replacement)?;
        if count > 0 {
            fs::write(path, replaced).map_err(|e| format!("{}: {e}", path.display()))?;
            files += 1;
            total += count;
        }
    }
    Ok((files, total))
}

/// Fuzzy subsequence match. Higher scores are better; returns matched char positions.
/// Rewards matches at the start, after separators (`/ _ - . \ :`) and camelCase humps,
/// and consecutive runs; penalizes gaps.
pub fn fuzzy_match(candidate: &str, query: &str) -> Option<(i64, Vec<usize>)> {
    let query: Vec<char> = query.chars().filter(|c| !c.is_whitespace()).collect();
    if query.is_empty() {
        return Some((0, Vec::new()));
    }
    let chars: Vec<char> = candidate.chars().collect();
    let mut positions = Vec::with_capacity(query.len());
    let mut score = 0i64;
    let mut qi = 0;
    let mut last: Option<usize> = None;
    for (i, &c) in chars.iter().enumerate() {
        if qi == query.len() {
            break;
        }
        if !c.eq_ignore_ascii_case(&query[qi]) {
            continue;
        }
        let prev = i.checked_sub(1).map(|p| chars[p]);
        let boundary = prev.is_none_or(|p| matches!(p, '/' | '_' | '-' | '.' | '\\' | ':' | ' '))
            || (c.is_uppercase() && prev.is_some_and(|p| p.is_lowercase()));
        score += 10;
        if boundary {
            score += 25;
        }
        if c == query[qi] {
            score += 2;
        }
        match last {
            Some(l) if l + 1 == i => score += 15,
            Some(l) => score -= (i - l - 1).min(10) as i64,
            None => score -= i.min(20) as i64,
        }
        positions.push(i);
        last = Some(i);
        qi += 1;
    }
    (qi == query.len()).then(|| (score - chars.len() as i64 / 4, positions))
}

/// Ranks `items` by fuzzy score of `key(item)` against `query`, best first.
pub fn fuzzy_rank<T, F: Fn(&T) -> &str>(items: impl IntoIterator<Item = T>, query: &str, key: F, limit: usize) -> Vec<(T, Vec<usize>)> {
    let mut scored: Vec<(i64, T, Vec<usize>)> = items
        .into_iter()
        .filter_map(|item| {
            let (score, positions) = fuzzy_match(key(&item), query)?;
            Some((score, item, positions))
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0));
    scored.into_iter().take(limit).map(|(_, item, positions)| (item, positions)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_and_replace_in_text() {
        let text = "User user USER userName";
        assert_eq!(TextQuery::new("user").find_all(text).unwrap().len(), 4);
        let case = TextQuery { case_sensitive: true, ..TextQuery::new("user") };
        assert_eq!(case.find_all(text).unwrap(), [5..9, 15..19]);
        let word = TextQuery { whole_word: true, ..TextQuery::new("user") };
        assert_eq!(word.find_all(text).unwrap().len(), 3);
        let regex = TextQuery { regex: true, case_sensitive: true, ..TextQuery::new(r"(\w+)Name") };
        assert_eq!(regex.replace_all(text, "${1}_name").unwrap(), ("User user USER user_name".to_string(), 1));
        assert_eq!(regex.expand("userName", "$1!"), "user!");
        let literal = TextQuery::new("a.b");
        assert_eq!(literal.replace_all("a.b axb", "$0").unwrap().0, "$0 axb");
        assert!(TextQuery { regex: true, ..TextQuery::new("(") }.compile().is_err());
    }

    #[test]
    fn find_and_replace_in_files() {
        let root = std::env::temp_dir().join(format!("phpcraft-search-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("app")).unwrap();
        fs::create_dir_all(root.join("vendor/pkg")).unwrap();
        fs::write(root.join(".gitignore"), "/vendor\n").unwrap();
        fs::write(root.join("app/User.php"), "<?php\nclass User {}\n// User again\n").unwrap();
        fs::write(root.join("app/view.blade.php"), "{{ $user }}\n").unwrap();
        fs::write(root.join("vendor/pkg/User.php"), "class User {}\n").unwrap();

        let query = TextQuery::new("user");
        let all = search_files(&root, &query, &FileSearch::default()).unwrap();
        assert_eq!(all.len(), 3, "{all:?}");
        let php_only = search_files(&root, &query, &FileSearch { mask: "*.php, !*.blade.php".into(), ..Default::default() }).unwrap();
        assert_eq!(php_only.len(), 2);
        assert_eq!(php_only[0].line, 1);
        assert_eq!(php_only[0].ranges, [6..10]);
        let with_vendor = search_files(&root, &query, &FileSearch { include_vendor: true, ..Default::default() }).unwrap();
        assert_eq!(with_vendor.len(), 4);

        let case = TextQuery { case_sensitive: true, whole_word: true, ..TextQuery::new("User") };
        let (files, count) = replace_in_files(&[root.join("app/User.php")], &case, "Member").unwrap();
        assert_eq!((files, count), (1, 2));
        assert_eq!(fs::read_to_string(root.join("app/User.php")).unwrap(), "<?php\nclass Member {}\n// Member again\n");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn fuzzy_ranking() {
        let files = ["app/Models/User.php", "app/Http/Controllers/UserController.php", "resources/views/users/index.blade.php", "config/auth.php"];
        let ranked: Vec<&str> = fuzzy_rank(files, "usercon", |f| f, 10).into_iter().map(|(f, _)| f).collect();
        assert_eq!(ranked[0], "app/Http/Controllers/UserController.php");
        let ranked: Vec<&str> = fuzzy_rank(files, "user", |f| f, 10).into_iter().map(|(f, _)| f).collect();
        assert_eq!(ranked.len(), 3);
        assert!(fuzzy_match("UserController", "uc").is_some());
        assert!(fuzzy_match("User", "xyz").is_none());
    }
}
