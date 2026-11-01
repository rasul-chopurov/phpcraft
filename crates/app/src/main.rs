mod config;
mod dialogs;
mod diff_view;
mod editor_view;
mod i18n;
mod intel;
mod keymap;
mod local_history;
mod markdown_preview;
mod run_configs;
mod settings_import;
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

use workspace::Workspace;

actions!(phpcraft, [Quit]);

/// Opens a project in a new window and remembers it in Recent Projects.
pub fn open_project_window(root: &std::path::Path, cx: &mut App) -> anyhow::Result<()> {
    let project = Project::open(root)?;
    config::remember_project(project.root());
    let bounds = Bounds::centered(None, size(px(1280.), px(800.)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions {
            title: Some(format!("{} – PhpCraft", project.name()).into()),
            ..Default::default()
        }),
        ..Default::default()
    };
    cx.open_window(options, |window, cx| {
        let workspace = cx.new(|cx| {
            let mut workspace = Workspace::new(project, window, cx);
            workspace.start_git_polling(cx);
            workspace
        });
        window.set_window_title(&workspace.read(cx).title());
        workspace
    })?;
    Ok(())
}

fn main() {
    // `phpcraft <folder>` opens a project; without arguments the welcome window shows.
    let root = std::env::args().nth(1);
    if let Some(root) = &root
        && let Err(err) = Project::open(root)
    {
        eprintln!("cannot open project {root}: {err:#}");
        std::process::exit(1);
    }

    Application::new().run(move |cx: &mut App| {
        theme::init(cx);
        keymap::apply(cx);
        cx.on_action(|_: &Quit, cx| cx.quit());
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
        cx.activate(true);
    });
}
