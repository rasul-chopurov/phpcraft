mod config;
mod dialogs;
mod diff_view;
mod editor_view;
mod intel;
mod keymap;
mod local_history;
mod markdown_preview;
mod run_configs;
mod sync;
mod terminal_view;
mod text_field;
mod theme;
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
    let root = std::env::args().nth(1).unwrap_or_else(|| ".".into());
    if let Err(err) = Project::open(&root) {
        eprintln!("cannot open project {root}: {err:#}");
        std::process::exit(1);
    }

    Application::new().run(move |cx: &mut App| {
        theme::init(cx);
        keymap::apply(cx);
        cx.on_action(|_: &Quit, cx| cx.quit());
        if let Err(err) = open_project_window(std::path::Path::new(&root), cx) {
            eprintln!("failed to open window: {err:#}");
            cx.quit();
        }
        cx.activate(true);
    });
}
