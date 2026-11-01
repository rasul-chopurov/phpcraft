//! Opening files and folders handed to PhpCraft from outside: command-line arguments (how
//! Windows and Linux file associations launch it), Finder / Dock "open with" events on macOS
//! and drag and drop onto a window. A folder opens as a project; a file opens in the window of
//! its project (the nearest folder with composer.json, package.json or .git), which is opened
//! if needed.

use std::path::{Path, PathBuf};

use gpui::App;

use crate::project_window::ProjectWindow;
use crate::welcome::Welcome;

/// The project folder a file belongs to.
pub fn project_root_for(file: &Path) -> PathBuf {
    let parent = file.parent().unwrap_or(file);
    parent
        .ancestors()
        .find(|dir| ["composer.json", "package.json", ".git"].iter().any(|marker| dir.join(marker).exists()))
        .unwrap_or(parent)
        .to_path_buf()
}

/// `file:///Users/me/My%20App` → `/Users/me/My App`; plain paths pass through.
pub fn path_from_url(url: &str) -> Option<PathBuf> {
    let Some(rest) = url.strip_prefix("file://") else {
        return (!url.contains("://")).then(|| PathBuf::from(url));
    };
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    let bytes = rest.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(byte) = u8::from_str_radix(&rest[i + 1..i + 3], 16)
        {
            out.push(byte);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    let path = String::from_utf8(out).ok()?;
    // `file:///C:/x` on Windows.
    let path = if path.len() > 2 && path.as_bytes()[0] == b'/' && path.as_bytes()[2] == b':' { path[1..].to_string() } else { path };
    Some(PathBuf::from(path))
}

/// Opens each path: folders as projects, files in their project's window.
pub fn open(paths: Vec<PathBuf>, cx: &mut App) {
    let mut opened = false;
    for path in paths {
        let path = std::fs::canonicalize(&path).unwrap_or(path);
        if path.is_dir() {
            opened |= focus_or_open(&path, None, cx);
        } else if path.is_file() {
            let root = project_root_for(&path);
            opened |= focus_or_open(&root, Some(&path), cx);
        }
    }
    if opened {
        close_welcome(cx);
    }
}

/// Shows `root` (in the window that already has it, or a new one) and opens `file` there.
fn focus_or_open(root: &Path, file: Option<&Path>, cx: &mut App) -> bool {
    if !show_in_existing(root, file, cx) {
        if let Err(err) = crate::open_project_window(root, cx) {
            eprintln!("open {}: {err:#}", root.display());
            return false;
        }
        show_in_existing(root, file, cx);
    }
    true
}

fn show_in_existing(root: &Path, file: Option<&Path>, cx: &mut App) -> bool {
    for handle in cx.windows() {
        let Some(window) = handle.downcast::<ProjectWindow>() else { continue };
        let found = window
            .update(cx, |project_window, window, cx| {
                let shown = project_window.show_project(root, window, cx);
                if let (Some(workspace), Some(file)) = (&shown, file) {
                    workspace.update(cx, |workspace, cx| workspace.open_file(file, window, cx));
                }
                if shown.is_some() {
                    window.activate_window();
                }
                shown.is_some()
            })
            .unwrap_or(false);
        if found {
            return true;
        }
    }
    false
}

fn close_welcome(cx: &mut App) {
    for handle in cx.windows() {
        if let Some(welcome) = handle.downcast::<Welcome>() {
            let _ = welcome.update(cx, |_, window, _| window.remove_window());
        }
    }
}

/// Forwards macOS "open documents" events (Finder, Dock, `open -a`) to `open`.
pub fn listen(app: &gpui::Application) -> async_channel::Receiver<Vec<PathBuf>> {
    let (tx, rx) = async_channel::unbounded();
    app.on_open_urls(move |urls| {
        let paths: Vec<PathBuf> = urls.iter().filter_map(|u| path_from_url(u)).collect();
        if !paths.is_empty() {
            let _ = tx.try_send(paths);
        }
    });
    rx
}

/// Runs `open` for every batch of paths that arrives on `rx`.
pub fn handle(rx: async_channel::Receiver<Vec<PathBuf>>, cx: &mut App) {
    cx.spawn(async move |cx| {
        while let Ok(paths) = rx.recv().await {
            let _ = cx.update(|cx| open(paths, cx));
        }
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_and_roots() {
        assert_eq!(path_from_url("file:///Users/me/My%20App/a.php"), Some(PathBuf::from("/Users/me/My App/a.php")));
        assert_eq!(path_from_url("file:///C:/www/index.php"), Some(PathBuf::from("C:/www/index.php")));
        assert_eq!(path_from_url("/tmp/x.sql"), Some(PathBuf::from("/tmp/x.sql")));
        assert_eq!(path_from_url("https://example.com"), None);
        let dir = std::env::temp_dir().join(format!("phpcraft-root-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("app/Models")).unwrap();
        std::fs::write(dir.join("composer.json"), "{}").unwrap();
        assert_eq!(project_root_for(&dir.join("app/Models/User.php")), dir);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
