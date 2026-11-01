mod editor_view;
mod intel;
mod theme;
mod workspace;

use gpui::{
    App, Application, Bounds, KeyBinding, TitlebarOptions, WindowBounds, WindowOptions, actions,
    prelude::*, px, size,
};
use project::Project;

use editor_view::*;
use workspace::{CloseTab, NavigateBack, NextTab, PrevTab, Workspace};

actions!(phpcraft, [Quit]);

fn bind_keys(cx: &mut App) {
    let editor = Some("Editor");
    cx.bind_keys([
        KeyBinding::new("left", MoveLeft, editor),
        KeyBinding::new("right", MoveRight, editor),
        KeyBinding::new("up", MoveUp, editor),
        KeyBinding::new("down", MoveDown, editor),
        KeyBinding::new("shift-left", SelectLeft, editor),
        KeyBinding::new("shift-right", SelectRight, editor),
        KeyBinding::new("shift-up", SelectUp, editor),
        KeyBinding::new("shift-down", SelectDown, editor),
        KeyBinding::new("home", MoveHome, editor),
        KeyBinding::new("end", MoveEnd, editor),
        KeyBinding::new("cmd-left", MoveHome, editor),
        KeyBinding::new("cmd-right", MoveEnd, editor),
        KeyBinding::new("shift-home", SelectHome, editor),
        KeyBinding::new("shift-end", SelectEnd, editor),
        KeyBinding::new("cmd-shift-left", SelectHome, editor),
        KeyBinding::new("cmd-shift-right", SelectEnd, editor),
        KeyBinding::new("pageup", PageUp, editor),
        KeyBinding::new("pagedown", PageDown, editor),
        KeyBinding::new("cmd-up", DocStart, editor),
        KeyBinding::new("cmd-down", DocEnd, editor),
        KeyBinding::new("backspace", Backspace, editor),
        KeyBinding::new("delete", Delete, editor),
        KeyBinding::new("enter", Newline, editor),
        KeyBinding::new("tab", Tab, editor),
        KeyBinding::new("cmd-a", SelectAll, editor),
        KeyBinding::new("cmd-c", Copy, editor),
        KeyBinding::new("cmd-x", Cut, editor),
        KeyBinding::new("cmd-v", Paste, editor),
        KeyBinding::new("cmd-z", Undo, editor),
        KeyBinding::new("cmd-shift-z", Redo, editor),
        KeyBinding::new("cmd-s", Save, editor),
        KeyBinding::new("cmd-b", GoToDeclaration, editor),
        KeyBinding::new("ctrl-space", ShowCompletions, editor),
        KeyBinding::new("alt-enter", ShowIntentions, editor),
        KeyBinding::new("ctrl-alt-o", OptimizeImports, editor),
        KeyBinding::new("cmd-[", NavigateBack, None),
        KeyBinding::new("cmd-alt-left", NavigateBack, None),
        KeyBinding::new("cmd-w", CloseTab, None),
        KeyBinding::new("ctrl-tab", NextTab, None),
        KeyBinding::new("ctrl-shift-tab", PrevTab, None),
        KeyBinding::new("cmd-q", Quit, None),
    ]);
    // Bound last so they win over the plain editor bindings while a popup is open.
    let menu = Some("Editor && menu");
    cx.bind_keys([
        KeyBinding::new("up", MenuUp, menu),
        KeyBinding::new("down", MenuDown, menu),
        KeyBinding::new("enter", MenuConfirm, menu),
        KeyBinding::new("tab", MenuConfirm, menu),
        KeyBinding::new("escape", MenuCancel, menu),
    ]);
}

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
        bind_keys(cx);
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
            let workspace = cx.new(|cx| Workspace::new(project, cx));
            window.set_window_title(&workspace.read(cx).title());
            workspace
        })
        .expect("failed to open window");
        cx.activate(true);
    });
}
