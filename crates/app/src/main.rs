// Release builds on Windows are GUI apps: no console window.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

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
mod modules;
mod project_info;
mod project_window;
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
mod welcome;
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
    let bounds = Bounds::centered(None, size(px(1280.), px(800.)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
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
/// composer, node and AI agents installed with Homebrew etc. are found.
fn inherit_shell_path() {
    if cfg!(windows) || std::env::var_os("TERM_PROGRAM").is_some() {
        return;
    }
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
    if let Ok(output) = std::process::Command::new(shell).args(["-l", "-c", "printf %s \"$PATH\""]).output()
        && output.status.success()
    {
        let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !path.is_empty() {
            // SAFETY: called first in main, before any other thread starts.
            unsafe { std::env::set_var("PATH", path) };
        }
    }
}

fn main() {
    inherit_shell_path();
    // Ask `php` for its version off the UI thread (the status bar shows it).
    std::thread::spawn(|| project_info::php_version());
    // `phpcraft <folder>` opens a project; without arguments the welcome window shows.
    let root = std::env::args().nth(1);
    if let Some(root) = &root
        && let Err(err) = Project::open(root)
    {
        eprintln!("cannot open project {root}: {err:#}");
        std::process::exit(1);
    }

    Application::new().with_assets(icons::Assets).run(move |cx: &mut App| {
        theme::init(cx);
        keymap::apply(cx);
        cx.on_action(|_: &Quit, cx| cx.quit());
        menus::set(cx);
        cx.activate(true);
        splash::show(cx, move |cx| {
            match &root {
                Some(root) => {
                    if let Err(err) = open_project_window(std::path::Path::new(root), cx) {
                        eprintln!("failed to open window: {err:#}");
                        cx.quit();
                    }
                }
                None => welcome::open(cx),
            }
            // Closing the last project brings the welcome window back.
            cx.on_window_closed(|cx| {
                if cx.windows().is_empty() {
                    welcome::open(cx);
                }
            })
            .detach();
        });
    });
}
