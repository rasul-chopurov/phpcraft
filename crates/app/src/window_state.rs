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
    let display = cx.displays().into_iter().find(|d| d.bounds().intersects(&title_bar))?;
    let screen = display.bounds();
    // Saved on a bigger screen (or a different scale factor): shrink and move it back inside,
    // or its edges can't be reached to resize it.
    let fitted = size(bounds.size.width.min(screen.size.width * 0.96), bounds.size.height.min(screen.size.height * 0.9));
    let origin = point(
        bounds.origin.x.max(screen.origin.x).min(screen.origin.x + screen.size.width - fitted.width),
        bounds.origin.y.max(screen.origin.y).min(screen.origin.y + screen.size.height - fitted.height),
    );
    let bounds = Bounds { origin, size: fitted };
    Some(if saved.maximized { WindowBounds::Maximized(bounds) } else { WindowBounds::Windowed(bounds) })
}

/// A window of `width` × `height`, centered on the main screen and no larger than ~90% of it
/// (small laptop screens with 125–150% scaling are only ~1100 × 650 logical pixels).
pub fn centered(width: f32, height: f32, cx: &App) -> Bounds<Pixels> {
    let wanted = size(px(width), px(height));
    let wanted = match cx.primary_display() {
        Some(display) => {
            let screen = display.bounds().size;
            size(wanted.width.min(screen.width * 0.94), wanted.height.min(screen.height * 0.88))
        }
        None => wanted,
    };
    Bounds::centered(None, wanted, cx)
}

/// The smallest size a resizable window can be dragged to.
pub fn min_size() -> gpui::Size<Pixels> {
    size(px(560.), px(380.))
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
