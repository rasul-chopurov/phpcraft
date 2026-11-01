//! Application-wide configuration in the platform config directory:
//! `~/Library/Application Support/phpcraft` (macOS), `%APPDATA%\phpcraft` (Windows),
//! `~/.config/phpcraft` (Linux).

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub fn dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("phpcraft")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutoSave {
    Off,
    /// Save when switching tabs or when the window loses focus.
    OnFocusChange,
    /// Additionally save shortly after typing stops.
    AfterDelay,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub theme: String,
    /// Preferred editor font; falls back to installed monospace fonts.
    pub editor_font: String,
    pub auto_save: AutoSave,
    pub auto_save_delay_ms: u64,
    /// Open files from the project tree with a single click (otherwise double-click).
    pub open_on_single_click: bool,
    /// Highlight the active editor's file in the project tree.
    pub select_opened_file: bool,
    /// Versions kept per file in Local History.
    pub local_history_versions: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: crate::theme::DEFAULT_THEME.into(),
            editor_font: "JetBrains Mono".into(),
            auto_save: AutoSave::AfterDelay,
            auto_save_delay_ms: 1000,
            open_on_single_click: true,
            select_opened_file: true,
            local_history_versions: history::DEFAULT_KEEP,
        }
    }
}

impl Settings {
    fn path() -> PathBuf {
        dir().join("settings.json")
    }

    pub fn load() -> Self {
        fs::read_to_string(Self::path())
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    /// Settings read at most once per second (they're consulted on every edit).
    pub fn load_cached() -> Self {
        use std::sync::Mutex;
        use std::time::{Duration, Instant};
        static CACHE: Mutex<Option<(Instant, Settings)>> = Mutex::new(None);
        let mut cache = CACHE.lock().unwrap();
        if let Some((at, settings)) = cache.as_ref()
            && at.elapsed() < Duration::from_secs(1)
        {
            return settings.clone();
        }
        let settings = Self::load();
        *cache = Some((Instant::now(), settings.clone()));
        settings
    }

    pub fn save(&self) {
        let _ = fs::create_dir_all(dir());
        let _ = fs::write(
            Self::path(),
            serde_json::to_string_pretty(self).unwrap_or_default(),
        );
    }
}

/// Recently opened project folders, newest first.
pub fn recent_projects() -> Vec<PathBuf> {
    fs::read_to_string(dir().join("recent_projects.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<Vec<PathBuf>>(&text).ok())
        .unwrap_or_default()
        .into_iter()
        .filter(|p| p.is_dir())
        .collect()
}

pub fn remember_project(root: &std::path::Path) {
    let mut projects = recent_projects();
    projects.retain(|p| p != root);
    projects.insert(0, root.to_path_buf());
    projects.truncate(20);
    let _ = fs::create_dir_all(dir());
    let _ = fs::write(dir().join("recent_projects.json"), serde_json::to_string_pretty(&projects).unwrap_or_default());
}
