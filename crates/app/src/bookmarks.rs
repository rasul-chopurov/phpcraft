//! Bookmarks: marked lines, optionally with a mnemonic (0-9, A-Z), shared by all projects and
//! kept in `<config>/bookmarks.json`. Editors track their file's bookmarks through edits and
//! write them back here.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Bookmark {
    pub path: PathBuf,
    pub line: usize,
    pub mnemonic: Option<char>,
}

static STORE: Mutex<Option<Vec<Bookmark>>> = Mutex::new(None);

fn file() -> PathBuf {
    crate::config::dir().join("bookmarks.json")
}

fn with_store<T>(f: impl FnOnce(&mut Vec<Bookmark>) -> T) -> T {
    let mut store = STORE.lock().unwrap();
    let list = store.get_or_insert_with(|| {
        std::fs::read_to_string(file()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    });
    f(list)
}

/// Every bookmark, by file then line.
pub fn all() -> Vec<Bookmark> {
    with_store(|list| {
        let mut all = list.clone();
        all.sort_by(|a, b| a.path.cmp(&b.path).then(a.line.cmp(&b.line)));
        all
    })
}

/// The bookmarks of one file: (line, mnemonic).
pub fn for_file(path: &Path) -> Vec<(usize, Option<char>)> {
    with_store(|list| list.iter().filter(|b| b.path == path).map(|b| (b.line, b.mnemonic)).collect())
}

/// Replaces the bookmarks of `path` and saves. A mnemonic used elsewhere moves here.
pub fn set_file(path: &Path, marks: &[(usize, Option<char>)]) {
    with_store(|list| {
        let taken: Vec<char> = marks.iter().filter_map(|(_, m)| *m).collect();
        list.retain(|b| b.path != path);
        for b in list.iter_mut() {
            if b.mnemonic.is_some_and(|m| taken.contains(&m)) {
                b.mnemonic = None;
            }
        }
        list.extend(marks.iter().map(|&(line, mnemonic)| Bookmark { path: path.to_path_buf(), line, mnemonic }));
        if let Ok(json) = serde_json::to_string_pretty(list) {
            let _ = std::fs::create_dir_all(crate::config::dir());
            let _ = std::fs::write(file(), json);
        }
    });
}

pub fn by_mnemonic(mnemonic: char) -> Option<Bookmark> {
    with_store(|list| list.iter().find(|b| b.mnemonic == Some(mnemonic)).cloned())
}

/// Mnemonics in the order the chooser offers them.
pub const MNEMONICS: &str = "1234567890ABCDEFGHIJKLMNOPQRSTUVWXYZ";
