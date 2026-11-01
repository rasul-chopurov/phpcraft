// Release builds on Windows are GUI apps: no console window.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod bookmarks;
mod config;
mod dialogs;
mod deployment;
mod diff_view;
mod editor_view;
mod http_client;
mod i18n;
mod icons;
mod intel;
mod keymap;
mod local_history;
mod markdown_preview;
mod menus;
mod open_paths;
mod modules;
mod project_info;
mod project_window;
mod quality;
mod run_configs;
mod settings_import;
mod splash;
mod structure;
mod table_view;
mod templates;
mod sync;
mod terminal_view;
mod text_field;
mod theme;
mod updates;
mod welcome;
mod window_state;
mod workspace;

use gpui::{
    App, Application, Bounds, TitlebarOptions, WindowBounds, WindowOptions, actions, prelude::*,
    px, size,
};
use project::Project;


actions!(phpcraft, [Quit]);

/// Opens a project in a new window and remembers it in Recent Projects.
pub fn open_project_window(root: &std::path::Path, cx: &mut App) -> anyhow::Result<()> {
    // Fail before opening a window when the folder can't be opened.
    Project::open(root)?;
    let root = root.to_path_buf();
    let bounds = window_state::restore(&root, cx).unwrap_or_else(|| WindowBounds::Windowed(Bounds::centered(None, size(px(1280.), px(800.)), cx)));
    let options = WindowOptions {
        window_bounds: Some(bounds),
        titlebar: Some(TitlebarOptions { title: Some("PhpCraft".into()), ..Default::default() }),
        ..Default::default()
    };
    let mut failure = None;
    cx.open_window(options, |window, cx| {
        let workspace = match project_window::new_workspace(&root, window, cx) {
            Ok(workspace) => workspace,
            Err(err) => {
                failure = Some(err);
                project_window::new_workspace(std::path::Path::new("."), window, cx).expect("current folder opens")
            }
        };
        cx.new(|cx| project_window::ProjectWindow::new(workspace, window, cx))
    })?;
    failure.map_or(Ok(()), Err)
}

/// Apps started from Finder/Dock get a minimal PATH; take the login shell's so php,
/// composer, node and AI agents installed with Homebrew etc. are found. The shell runs
/// interactive too: installers (opencode, bun, Herd…) add themselves to ~/.zshrc, which a
/// non-interactive login shell skips.
fn inherit_shell_path() {
    if cfg!(windows) || std::env::var_os("TERM_PROGRAM").is_some() {
        return;
    }
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
    let path = shell_path(&shell, &["-l", "-i", "-c"]).or_else(|| shell_path(&shell, &["-l", "-c"]));
    let mut dirs: Vec<std::path::PathBuf> = path.as_deref().or(std::env::var("PATH").ok().as_deref()).map(|p| std::env::split_paths(p).collect()).unwrap_or_default();
    // Where installers put their binaries, in case the shell config doesn't say.
    if let Some(home) = std::env::var_os("HOME").map(std::path::PathBuf::from) {
        for extra in [".opencode/bin", ".local/bin", ".bun/bin", ".npm-global/bin", ".cargo/bin", "Library/Application Support/Herd/bin"] {
            let dir = home.join(extra);
            if dir.is_dir() && !dirs.contains(&dir) {
                dirs.push(dir);
            }
        }
    }
    for dir in ["/opt/homebrew/bin", "/usr/local/bin"].map(std::path::PathBuf::from) {
        if dir.is_dir() && !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }
    if let Ok(path) = std::env::join_paths(dirs) {
        // SAFETY: called first in main, before any other thread starts.
        unsafe { std::env::set_var("PATH", path) };
    }
}

/// `$PATH` as `shell args…` sees it, between markers (rc files may print), or None after 3 s.
fn shell_path(shell: &str, args: &[&str]) -> Option<String> {
    use std::io::Read;
    use std::process::Stdio;
    let mut child = std::process::Command::new(shell)
        .args(args)
        .arg("printf '\\n__PHPCRAFT_PATH__%s__PHPCRAFT_END__\\n' \"$PATH\"")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let started = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() < std::time::Duration::from_secs(3) => std::thread::sleep(std::time::Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let mut out = String::new();
    child.stdout.take()?.read_to_string(&mut out).ok()?;
    let start = out.find("__PHPCRAFT_PATH__")? + "__PHPCRAFT_PATH__".len();
    let path = &out[start..start + out[start..].find("__PHPCRAFT_END__")?];
    (!path.trim().is_empty()).then(|| path.trim().to_string())
}

fn main() {
    inherit_shell_path();
    // Ask `php` for its version off the UI thread (the status bar shows it).
    std::thread::spawn(|| project_info::php_version());
    // `phpcraft <folder or files…>` opens them (file associations on Windows and Linux pass the
    // file this way); without arguments the welcome window shows.
    let args: Vec<std::path::PathBuf> = std::env::args().skip(1).filter(|a| !a.starts_with('-')).map(Into::into).collect();
    if let Some(missing) = args.iter().find(|p| !p.exists()) {
        eprintln!("cannot open {}: no such file or folder", missing.display());
        std::process::exit(1);
    }

    let app = Application::new().with_assets(icons::Assets);
    // Files and folders opened from Finder, the Dock or `open -a PhpCraft` on macOS.
    let opened = open_paths::listen(&app);
    app.run(move |cx: &mut App| {
        theme::init(cx);
        keymap::apply(cx);
        cx.on_action(|_: &Quit, cx| cx.quit());
        updates::init(cx);
        menus::set(cx);
        cx.activate(true);
        splash::show(cx, move |cx| {
            let has_project = cx.windows().iter().any(|w| w.downcast::<project_window::ProjectWindow>().is_some());
            if !args.is_empty() {
                open_paths::open(args, cx);
            } else if !has_project {
                welcome::open(cx);
            }
            open_paths::handle(opened, cx);
            updates::check_at_startup(cx);
                // Closing the last window quits, except after Close Project, which goes back to
            // the welcome window.
            cx.on_window_closed(|cx| {
                if cx.windows().is_empty() {
                    if project_window::take_return_to_welcome() {
                        welcome::open(cx);
                    } else {
                        cx.quit();
                    }
                }
            })
            .detach();
        });
    });
}
