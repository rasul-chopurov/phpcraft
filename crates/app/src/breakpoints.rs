//! Line breakpoints, optionally conditional, shared by all projects and kept in
//! `<config>/breakpoints.json`. Editors track their file's breakpoints through edits.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Breakpoint {
    pub path: PathBuf,
    /// 0-based line.
    pub line: usize,
    /// PHP expression; the breakpoint only stops when it is true.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<String>,
}

static STORE: Mutex<Option<Vec<Breakpoint>>> = Mutex::new(None);

fn file() -> PathBuf {
    crate::config::dir().join("breakpoints.json")
}

fn with_store<T>(f: impl FnOnce(&mut Vec<Breakpoint>) -> T) -> T {
    let mut store = STORE.lock().unwrap();
    let list = store.get_or_insert_with(|| std::fs::read_to_string(file()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default());
    f(list)
}

pub fn all() -> Vec<Breakpoint> {
    with_store(|list| list.clone())
}

pub fn for_file(path: &Path) -> Vec<(usize, Option<String>)> {
    with_store(|list| list.iter().filter(|b| b.path == path).map(|b| (b.line, b.condition.clone())).collect())
}

/// Replaces the breakpoints of `path` and saves.
pub fn set_file(path: &Path, lines: &[(usize, Option<String>)]) {
    with_store(|list| {
        list.retain(|b| b.path != path);
        list.extend(lines.iter().map(|(line, condition)| Breakpoint { path: path.to_path_buf(), line: *line, condition: condition.clone() }));
        if let Ok(json) = serde_json::to_string_pretty(list) {
            let _ = std::fs::create_dir_all(crate::config::dir());
            let _ = std::fs::write(file(), json);
        }
    });
}
