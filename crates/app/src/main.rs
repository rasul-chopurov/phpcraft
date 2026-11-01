mod config;
mod dialogs;
mod editor_view;
mod intel;
mod keymap;
mod sync;
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

fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| ".".into());
    let project = match Project::open(&root) {
        Ok(project) => project,
        Err(err) => {
            eprintln!("cannot open project {root}: {err:#}");
            std::process::exit(1);
        }
    };

    Application::new().run(move |cx: &mut App| {
        theme::init(cx);
        keymap::apply(cx);
        cx.on_action(|_: &Quit, cx| cx.quit());

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
        })
        .expect("failed to open window");
        cx.activate(true);
    });
}
