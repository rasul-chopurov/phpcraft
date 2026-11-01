//! Spell checking for comments and string literals: an English word list (Webster's Second,
//! public domain, from BSD's `/usr/share/dict/web2`) plus programming words, with simple
//! inflection rules since the list has base forms only, and a per-project dictionary in
//! `.phpcraft/dictionary.txt`.

use std::collections::HashSet;
use std::io::Read;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Words shorter than this aren't checked: mostly abbreviations (`str`, `cfg`, `env`).
const MIN_LEN: usize = 4;

fn bundled() -> &'static HashSet<&'static str> {
    static WORDS: OnceLock<HashSet<&'static str>> = OnceLock::new();
    WORDS.get_or_init(|| {
        let mut text = String::new();
        let _ = flate2::read::GzDecoder::new(&include_bytes!("../words.txt.gz")[..]).read_to_string(&mut text);
        let text: &'static str = Box::leak(text.into_boxed_str());
        text.lines().chain(include_str!("extra.txt").lines()).filter(|w| !w.is_empty()).collect()
    })
}

/// Loads the bundled word list (call off the UI thread before the first check).
pub fn warm_up() {
    bundled();
}

/// (suffix, what may replace it to get the stem): `stored` → `store`, `copies` → `copy`.
const SUFFIXES: &[(&str, &[&str])] = &[
    ("s", &[""]),
    ("es", &[""]),
    ("ies", &["y"]),
    ("ed", &["", "e"]),
    ("ied", &["y"]),
    ("ing", &["", "e"]),
    ("er", &["", "e"]),
    ("ers", &["", "e"]),
    ("est", &["", "e"]),
    ("ly", &[""]),
    ("ily", &["y"]),
    ("ally", &[""]),
    ("able", &["", "e"]),
    ("ible", &["", "e"]),
    ("ability", &["able"]),
    ("ment", &[""]),
    ("ness", &[""]),
    ("ful", &[""]),
    ("less", &[""]),
    ("ity", &["", "e"]),
    ("al", &["", "e"]),
    ("ation", &["", "e", "ate"]),
    ("ization", &["ize"]),
    ("ize", &["", "e"]),
    ("ise", &["ize"]),
    ("ised", &["ize"]),
    ("ising", &["ize"]),
    ("isation", &["ize"]),
];

/// Common misspellings the inflection rules would let through (`occur` + `ed`).
const MISSPELT: &[&str] = &["occured", "occuring", "refered", "refering", "prefered", "prefering", "transfered", "transfering", "commited", "commiting", "submited", "submiting", "admited", "controled", "controling", "begining", "forgoten", "writen", "untill", "wich", "teh", "alot"];

const PREFIXES: &[&str] = &["un", "re", "pre", "de", "dis", "non", "sub", "super", "multi", "auto", "over", "under", "inter", "mis", "co", "in", "im", "post", "anti", "semi"];

#[derive(Debug, Clone, Default)]
pub struct Dictionary {
    project: HashSet<String>,
    file: Option<PathBuf>,
}

/// A misspelt word: its byte range in the checked text and the word as written.
#[derive(Debug, Clone, PartialEq)]
pub struct Typo {
    pub range: Range<usize>,
    pub word: String,
}

impl Dictionary {
    /// The project's dictionary (`.phpcraft/dictionary.txt` under `root`) on top of the bundled one.
    pub fn load(root: &Path) -> Self {
        let file = root.join(".phpcraft").join("dictionary.txt");
        let project = std::fs::read_to_string(&file).unwrap_or_default().lines().map(|l| l.trim().to_lowercase()).filter(|l| !l.is_empty()).collect();
        Self { project, file: Some(file) }
    }

    /// Adds `word` to the project dictionary file.
    pub fn save_word(&mut self, word: &str) -> std::io::Result<()> {
        let word = word.to_lowercase();
        if let Some(file) = &self.file {
            if let Some(dir) = file.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let mut words: Vec<&str> = self.project.iter().map(String::as_str).chain([word.as_str()]).collect();
            words.sort_unstable();
            words.dedup();
            std::fs::write(file, words.join("\n") + "\n")?;
        }
        self.project.insert(word);
        Ok(())
    }

    fn base(&self, word: &str) -> bool {
        bundled().contains(word) || self.project.contains(word)
    }

    /// Whether the lowercase `word` is spelt correctly, inflections and prefixes included.
    pub fn knows(&self, word: &str) -> bool {
        if MISSPELT.contains(&word) && !self.project.contains(word) {
            return false;
        }
        self.knows_at(word, 2)
    }

    fn knows_at(&self, word: &str, depth: u8) -> bool {
        if self.base(word) {
            return true;
        }
        if depth == 0 {
            return false;
        }
        for (suffix, stems) in SUFFIXES {
            let Some(stem) = word.strip_suffix(suffix).filter(|s| s.len() >= 2) else { continue };
            for end in *stems {
                if self.knows_at(&format!("{stem}{end}"), depth - 1) {
                    return true;
                }
            }
            // `stopped` → `stop`, `running` → `run`.
            let b = stem.as_bytes();
            if b.len() >= 3 && b[b.len() - 1] == b[b.len() - 2] && self.base(&stem[..stem.len() - 1]) {
                return true;
            }
        }
        // Only onto listed words: the list has rare words (`trun`) that would excuse `retrun`.
        if PREFIXES.iter().any(|p| word.strip_prefix(p).is_some_and(|rest| rest.len() >= 5 && self.base(rest))) {
            return true;
        }
        // Compounds: `codebase`, `timestamp`.
        (4..word.len().saturating_sub(3)).any(|i| word.is_char_boundary(i) && self.base(&word[..i]) && self.knows_at(&word[i..], depth - 1))
    }

    /// Misspelt words in `text` (a comment or a string's content); ranges are relative to `text`.
    /// Text in another language is left alone: anything with non-ASCII letters, or where most
    /// of three or more words are unknown.
    pub fn check(&self, text: &str) -> Vec<Typo> {
        if text.chars().any(|c| c.is_alphabetic() && !c.is_ascii()) {
            return Vec::new();
        }
        let all: Vec<_> = words(text).collect();
        let typos: Vec<Typo> = all.iter().filter(|(_, w)| !self.knows(&w.to_lowercase())).map(|(range, word)| Typo { range: range.clone(), word: word.to_string() }).collect();
        if typos.len() >= 3 && typos.len() * 2 > all.len() {
            return Vec::new();
        }
        typos
    }

    /// Up to `max` corrections for `word`, closest first, in its capitalization.
    pub fn suggestions(&self, word: &str, max: usize) -> Vec<String> {
        let lower = word.to_lowercase();
        let near = edits(&lower);
        let mut out: Vec<String> = Vec::new();
        let push = |w: &str, out: &mut Vec<String>| {
            if !out.iter().any(|o| o == w) {
                out.push(w.to_string());
            }
        };
        let mut first: Vec<&String> = near.iter().filter(|w| self.base(w)).collect();
        first.sort_by_key(|w| (!w.starts_with(&lower[..1]), w.len().abs_diff(lower.len())));
        for w in first {
            push(w, &mut out);
        }
        for w in near.iter().filter(|w| self.knows(w)) {
            push(w, &mut out);
        }
        if out.len() < max {
            let mut far: Vec<String> = near.iter().flat_map(|w| edits(w)).filter(|w| self.base(w)).collect();
            far.sort_by_key(|w| (!w.starts_with(&lower[..1]), w.len().abs_diff(lower.len())));
            for w in far {
                push(&w, &mut out);
            }
        }
        out.truncate(max);
        out.into_iter().map(|w| with_case(word, &w)).collect()
    }
}

/// Everything one deletion, transposition, replacement or insertion away.
fn edits(word: &str) -> Vec<String> {
    let chars: Vec<char> = word.chars().collect();
    let mut out = Vec::new();
    let s = |v: &[char]| v.iter().collect::<String>();
    for i in 0..=chars.len() {
        if i < chars.len() {
            let mut v = chars.clone();
            v.remove(i);
            out.push(s(&v));
        }
        if i + 1 < chars.len() {
            let mut v = chars.clone();
            v.swap(i, i + 1);
            out.push(s(&v));
        }
        for c in 'a'..='z' {
            if i < chars.len() && chars[i] != c {
                let mut v = chars.clone();
                v[i] = c;
                out.push(s(&v));
            }
            let mut v = chars.clone();
            v.insert(i, c);
            out.push(s(&v));
        }
    }
    out.retain(|w| !w.is_empty());
    out
}

fn with_case(original: &str, word: &str) -> String {
    if original.len() > 1 && original.chars().all(|c| c.is_ascii_uppercase()) {
        word.to_uppercase()
    } else if original.starts_with(|c: char| c.is_ascii_uppercase()) {
        let mut c = word.chars();
        c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
    } else {
        word.to_string()
    }
}

/// The words of `text` worth checking, with byte ranges: whitespace-separated chunks of plain
/// letters (code-looking chunks with `$`, `\`, `/`, `@`, digits… are skipped), split at `_`,
/// `-`, `.`, `'` and camelCase humps; acronyms and short words are left out.
pub fn words(text: &str) -> impl Iterator<Item = (Range<usize>, &str)> {
    let mut out = Vec::new();
    let mut start = None;
    for (i, c) in text.char_indices().chain([(text.len(), ' ')]) {
        match (c.is_whitespace(), start) {
            (false, None) => start = Some(i),
            (true, Some(s)) => {
                chunk(text, s, i, &mut out);
                start = None;
            }
            _ => {}
        }
    }
    out.into_iter()
}

fn chunk<'a>(text: &'a str, mut start: usize, mut end: usize, out: &mut Vec<(Range<usize>, &'a str)>) {
    const TRIM: &[char] = &['"', '\'', '`', '(', ')', '[', ']', '{', '}', ',', '.', ';', ':', '!', '?', '*', '-', '<', '>', '&'];
    let raw = &text[start..end];
    let trimmed_start = raw.trim_start_matches(TRIM);
    start += raw.len() - trimmed_start.len();
    end = start + trimmed_start.trim_end_matches(TRIM).len();
    let word = &text[start..end];
    if word.is_empty() || !word.chars().all(|c| c.is_ascii_alphabetic() || matches!(c, '\'' | '-' | '_' | '.')) {
        return;
    }
    let mut part_start = start;
    for (i, c) in word.char_indices().map(|(i, c)| (start + i, c)).chain([(end, '_')]) {
        if matches!(c, '\'' | '-' | '_' | '.') {
            humps(text, part_start, i, out);
            part_start = i + 1;
        }
    }
}

/// `parseHTMLString` → `parse`, `HTML`, `String`.
fn humps<'a>(text: &'a str, start: usize, end: usize, out: &mut Vec<(Range<usize>, &'a str)>) {
    let b = text.as_bytes();
    let mut from = start;
    for i in start + 1..end {
        let (prev, cur) = (b[i - 1], b[i]);
        let next_lower = i + 1 < end && b[i + 1].is_ascii_lowercase();
        if cur.is_ascii_uppercase() && (prev.is_ascii_lowercase() || (prev.is_ascii_uppercase() && next_lower)) {
            push_word(text, from, i, out);
            from = i;
        }
    }
    push_word(text, from, end, out);
}

fn push_word<'a>(text: &'a str, start: usize, end: usize, out: &mut Vec<(Range<usize>, &'a str)>) {
    let word = &text[start..end];
    let acronym = word.len() > 1 && word.bytes().all(|c| c.is_ascii_uppercase());
    if word.len() >= MIN_LEN && !acronym {
        out.push((start..end, word));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dict() -> Dictionary {
        Dictionary::default()
    }

    #[test]
    fn knows_inflections_and_compounds() {
        let d = dict();
        for w in ["users", "stored", "running", "copies", "initializers", "unserialize", "codebase", "returns", "created", "database", "doesn"] {
            assert!(d.knows(w), "{w}");
        }
        for w in ["recieve", "occured", "seperate", "lenght", "retrun", "fucntion", "paramter"] {
            assert!(!d.knows(w), "{w}");
        }
    }

    #[test]
    fn finds_words_in_code_like_text() {
        let found: Vec<&str> = words("Get the userName's @param $id App\\Models\\User HTMLParser 'posts.show' v2 http://x.y/z (seperate).").map(|(_, w)| w).collect();
        assert_eq!(found, ["user", "Name", "Parser", "posts", "show", "seperate"]);
        let typos = dict().check("Fetch the recieved paylaod from users.");
        assert_eq!(typos.iter().map(|t| t.word.as_str()).collect::<Vec<_>>(), ["recieved", "paylaod"]);
        assert_eq!(&"Fetch the recieved paylaod from users."[typos[0].range.clone()], "recieved");
        assert!(dict().check("Qeydiyyat forumun eksperti yoxdur").is_empty());
        assert!(dict().check("İstifadəçi tapilmadi").is_empty());
    }

    #[test]
    fn suggests_corrections() {
        let d = dict();
        assert!(d.suggestions("recieve", 5).contains(&"receive".to_string()));
        assert_eq!(d.suggestions("Lenght", 1), ["Length"]);
        assert!(d.suggestions("recieved", 5).contains(&"received".to_string()));
    }

    #[test]
    fn project_dictionary() {
        let dir = std::env::temp_dir().join(format!("phpcraft-spell-{}", std::process::id()));
        let mut d = Dictionary::load(&dir);
        assert!(!d.knows("phpcraft"));
        d.save_word("PhpCraft").unwrap();
        assert!(Dictionary::load(&dir).knows("phpcraft"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
