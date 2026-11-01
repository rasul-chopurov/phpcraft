//! Local History wiring: one store per machine in the platform data directory.

use std::path::Path;
use std::sync::OnceLock;

use history::History;

/// Saves within this window are merged into one version.
const SAVE_WINDOW_SECS: u64 = 60;

fn store() -> &'static History {
    static STORE: OnceLock<History> = OnceLock::new();
    STORE.get_or_init(|| {
        let dir = dirs::data_local_dir().unwrap_or_else(std::env::temp_dir).join("phpcraft").join("history");
        History::new(dir, crate::config::Settings::load().local_history_versions)
    })
}

pub fn record(path: &Path, content: &str, label: &str) {
    let window = if label == "Save" { SAVE_WINDOW_SECS } else { 0 };
    if let Err(err) = store().record_coalesced(path, content, label, window) {
        eprintln!("local history: {err}");
    }
}

/// Records the files' current contents on disk (before a git operation rewrites them).
pub fn record_disk(paths: &[std::path::PathBuf], label: &str) {
    for path in paths {
        if let Ok(content) = std::fs::read_to_string(path) {
            record(path, &content, label);
        }
    }
}

pub fn versions(path: &Path) -> Vec<history::Version> {
    store().versions(path)
}

pub fn content(version: &history::Version) -> Option<String> {
    store().content(version)
}
