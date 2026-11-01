//! Remembers each project window's size, position and maximized state between runs.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use gpui::{App, Bounds, Pixels, WindowBounds, point, px, size};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
struct Saved {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    #[serde(default)]
    maximized: bool,
}

fn file() -> PathBuf {
    crate::config::dir().join("windows.json")
}

fn load_all() -> HashMap<String, Saved> {
    std::fs::read_to_string(file()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

/// The saved bounds for `root`, if they still fit on a connected display.
pub fn restore(root: &Path, cx: &App) -> Option<WindowBounds> {
    let saved = *load_all().get(&root.to_string_lossy().to_string())?;
    if saved.width < 400. || saved.height < 300. {
        return None;
    }
    let bounds = Bounds { origin: point(px(saved.x), px(saved.y)), size: size(px(saved.width), px(saved.height)) };
    // A window whose title bar isn't on any screen (a monitor was unplugged) opens centered.
    let title_bar = Bounds { origin: bounds.origin, size: size(bounds.size.width, px(30.)) };
    if !cx.displays().iter().any(|d| d.bounds().intersects(&title_bar)) {
        return None;
    }
    Some(if saved.maximized { WindowBounds::Maximized(bounds) } else { WindowBounds::Windowed(bounds) })
}

/// Saves the window for `root`: its origin, its content size (what `WindowOptions` takes,
/// without the title bar) and whether it's maximized. Full-screen windows aren't saved.
pub fn save(root: &Path, window: &gpui::Window) {
    let (b, maximized): (Bounds<Pixels>, bool) = match window.window_bounds() {
        WindowBounds::Windowed(b) => (b, false),
        WindowBounds::Maximized(b) => (b, true),
        WindowBounds::Fullscreen(_) => return,
    };
    let content = window.viewport_size();
    let saved = Saved { x: f32::from(b.origin.x), y: f32::from(b.origin.y), width: f32::from(content.width), height: f32::from(content.height), maximized };
    let mut all = load_all();
    let key = root.to_string_lossy().to_string();
    if all.get(&key) == Some(&saved) {
        return;
    }
    all.insert(key, saved);
    let _ = std::fs::create_dir_all(crate::config::dir());
    let _ = std::fs::write(file(), serde_json::to_string_pretty(&all).unwrap_or_default());
}
