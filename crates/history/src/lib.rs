//! Local History: the last N versions of every file, independent of git (like PhpStorm).
//!
//! Layout under the store directory:
//! - `objects/<hash>`: file contents, stored once per distinct content (deduplicated);
//! - `files/<path-hash>.json`: the versions of one file, newest last.
//!
//! Unreferenced objects are removed when versions are pruned.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

pub const DEFAULT_KEEP: usize = 10;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Version {
    /// Unix seconds.
    pub time: u64,
    /// Why it was recorded: "Save", "Opened", "Before Rollback", ...
    pub label: String,
    pub object: String,
    pub size: usize,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct FileIndex {
    path: String,
    versions: Vec<Version>,
}

pub struct History {
    dir: PathBuf,
    keep: usize,
}

/// FNV-1a 64-bit: stable across runs and platforms, plenty for de-duplication.
fn fnv(data: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in data {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

fn object_id(content: &str) -> String {
    format!("{:016x}{:08x}", fnv(content.as_bytes()), content.len() as u32)
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

impl History {
    /// A store in `dir` keeping up to `keep` versions per file.
    pub fn new(dir: impl Into<PathBuf>, keep: usize) -> Self {
        Self { dir: dir.into(), keep: keep.max(1) }
    }

    fn index_path(&self, file: &Path) -> PathBuf {
        let key = file.to_string_lossy();
        self.dir.join("files").join(format!("{:016x}.json", fnv(key.as_bytes())))
    }

    fn load(&self, file: &Path) -> FileIndex {
        fs::read_to_string(self.index_path(file))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_else(|| FileIndex { path: file.to_string_lossy().into_owned(), versions: Vec::new() })
    }

    fn store(&self, file: &Path, index: &FileIndex) -> std::io::Result<()> {
        let path = self.index_path(file);
        fs::create_dir_all(path.parent().unwrap())?;
        fs::write(path, serde_json::to_string(index).unwrap_or_default())
    }

    /// Records `content` as a version of `file`. Skips it when identical to the newest version.
    /// Returns whether a version was added.
    pub fn record(&self, file: &Path, content: &str, label: &str) -> std::io::Result<bool> {
        self.record_coalesced(file, content, label, 0)
    }

    /// Like [`History::record`], but a version with the same label recorded less than
    /// `window_secs` ago is replaced instead of adding another one — so frequent auto-saves
    /// don't push older history out.
    pub fn record_coalesced(&self, file: &Path, content: &str, label: &str, window_secs: u64) -> std::io::Result<bool> {
        let mut index = self.load(file);
        let object = object_id(content);
        if index.versions.last().is_some_and(|v| v.object == object) {
            return Ok(false);
        }
        let mut replaced = None;
        if window_secs > 0
            && index.versions.len() > 1
            && index.versions.last().is_some_and(|v| v.label == label && now().saturating_sub(v.time) < window_secs)
        {
            replaced = index.versions.pop();
        }
        let objects = self.dir.join("objects");
        fs::create_dir_all(&objects)?;
        let object_path = objects.join(&object);
        if !object_path.exists() {
            fs::write(&object_path, content)?;
        }
        index.versions.push(Version { time: now(), label: label.to_string(), object, size: content.len() });
        let removed: Vec<Version> = if index.versions.len() > self.keep {
            let excess = index.versions.len() - self.keep;
            index.versions.drain(..excess).collect()
        } else {
            Vec::new()
        };
        self.store(file, &index)?;
        for version in removed.into_iter().chain(replaced) {
            self.collect_object(&version.object);
        }
        Ok(true)
    }

    /// Versions of `file`, newest first.
    pub fn versions(&self, file: &Path) -> Vec<Version> {
        let mut versions = self.load(file).versions;
        versions.reverse();
        versions
    }

    pub fn content(&self, version: &Version) -> Option<String> {
        fs::read_to_string(self.dir.join("objects").join(&version.object)).ok()
    }

    /// Deletes an object no file index refers to anymore.
    fn collect_object(&self, object: &str) {
        let Ok(entries) = fs::read_dir(self.dir.join("files")) else { return };
        let referenced = entries.flatten().any(|entry| {
            fs::read_to_string(entry.path()).is_ok_and(|text| text.contains(&format!("\"object\":\"{object}\"")))
        });
        if !referenced {
            let _ = fs::remove_file(self.dir.join("objects").join(object));
        }
    }
}

/// `5 min ago`, `yesterday`, ...
pub fn ago(time: u64) -> String {
    let elapsed = now().saturating_sub(time);
    match elapsed {
        0..60 => "just now".into(),
        60..3600 => format!("{} min ago", elapsed / 60),
        3600..86_400 => format!("{} h ago", elapsed / 3600),
        86_400..172_800 => "yesterday".into(),
        _ => format!("{} days ago", elapsed / 86_400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_dedupes_and_prunes() {
        let dir = std::env::temp_dir().join(format!("phpcraft-history-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let history = History::new(&dir, 3);
        let file = Path::new("/project/app/User.php");
        assert!(history.record(file, "v1", "Opened").unwrap());
        assert!(!history.record(file, "v1", "Save").unwrap(), "unchanged content is not recorded twice");
        for v in ["v2", "v3", "v4"] {
            history.record(file, v, "Save").unwrap();
        }
        let versions = history.versions(file);
        let contents: Vec<String> = versions.iter().map(|v| history.content(v).unwrap()).collect();
        assert_eq!(contents, ["v4", "v3", "v2"], "newest first, only the last 3 kept");
        // v1's object was pruned; shared contents survive while referenced.
        assert!(!dir.join("objects").join(object_id("v1")).exists());
        let other = Path::new("/project/app/Post.php");
        history.record(other, "v4", "Opened").unwrap();
        for v in ["a", "b", "c"] {
            history.record(file, v, "Save").unwrap();
        }
        assert!(dir.join("objects").join(object_id("v4")).exists(), "still used by Post.php");
        assert_eq!(history.versions(other).len(), 1);

        // Auto-saves within the window collapse into one version.
        let edited = Path::new("/project/app/Edited.php");
        history.record(edited, "opened", "Opened").unwrap();
        for v in ["s1", "s2", "s3"] {
            history.record_coalesced(edited, v, "Save", 60).unwrap();
        }
        let labels: Vec<(String, String)> = history.versions(edited).iter().map(|v| (v.label.clone(), history.content(v).unwrap())).collect();
        assert_eq!(labels, [("Save".into(), "s3".into()), ("Opened".into(), "opened".into())]);
        fs::remove_dir_all(&dir).unwrap();
    }
}
