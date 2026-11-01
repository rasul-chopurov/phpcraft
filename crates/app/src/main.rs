mod dialogs;
mod editor_view;
mod intel;
mod text_field;
mod theme;
mod workspace;

use gpui::{
    App, Application, Bounds, KeyBinding, TitlebarOptions, WindowBounds, WindowOptions, actions,
    prelude::*, px, size,
};
use project::Project;

use editor_view::*;
use workspace::{
    CloseTab, FindInFiles, GoToClass, GoToFile, GoToLine, GoToSymbol, NavigateBack, NextTab, OpenArtisan, OpenMake,
    OpenRouteSearch, PrevTab, RecentFiles, ReplaceInFiles, SearchEverywhere, Workspace,
};

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
        KeyBinding::new("cmd-shift-f", FindInFiles, None),
        KeyBinding::new("cmd-shift-r", ReplaceInFiles, None),
        KeyBinding::new("cmd-shift-o", GoToFile, None),
        KeyBinding::new("cmd-o", GoToClass, None),
        KeyBinding::new("cmd-alt-o", GoToSymbol, None),
        KeyBinding::new("cmd-e", RecentFiles, None),
        KeyBinding::new("cmd-l", GoToLine, None),
        KeyBinding::new("cmd-shift-a", SearchEverywhere, None),
        KeyBinding::new("cmd-n", OpenMake, None),
        KeyBinding::new("cmd-shift-,", OpenMake, None),
        KeyBinding::new("ctrl-shift-r", OpenRouteSearch, None),
        KeyBinding::new("ctrl-shift-x", OpenArtisan, None),
        KeyBinding::new("cmd-w", CloseTab, None),
        KeyBinding::new("ctrl-tab", NextTab, None),
        KeyBinding::new("ctrl-shift-tab", PrevTab, None),
        KeyBinding::new("cmd-q", Quit, None),
    ]);
    let field = Some("TextField");
    cx.bind_keys([
        KeyBinding::new("backspace", text_field::Backspace, field),
        KeyBinding::new("delete", text_field::Delete, field),
        KeyBinding::new("left", text_field::Left, field),
        KeyBinding::new("right", text_field::Right, field),
        KeyBinding::new("home", text_field::Home, field),
        KeyBinding::new("end", text_field::End, field),
        KeyBinding::new("cmd-left", text_field::Home, field),
        KeyBinding::new("cmd-right", text_field::End, field),
        KeyBinding::new("cmd-a", text_field::SelectAll, field),
        KeyBinding::new("cmd-v", text_field::Paste, field),
        KeyBinding::new("cmd-c", text_field::Copy, field),
    ]);
    let dialog = Some("Dialog");
    cx.bind_keys([
        KeyBinding::new("enter", dialogs::Confirm, dialog),
        KeyBinding::new("escape", dialogs::Cancel, dialog),
        KeyBinding::new("tab", dialogs::FocusNext, dialog),
        KeyBinding::new("shift-tab", dialogs::FocusPrev, dialog),
        KeyBinding::new("ctrl-[", dialogs::MoveRowUp, dialog),
        KeyBinding::new("ctrl-]", dialogs::MoveRowDown, dialog),
        KeyBinding::new("ctrl-backspace", dialogs::DeleteRow, dialog),
        KeyBinding::new("up", dialogs::SelectPrev, Some("ArtisanDialog")),
        KeyBinding::new("down", dialogs::SelectNext, Some("ArtisanDialog")),
        KeyBinding::new("tab", dialogs::Complete, Some("ArtisanDialog")),
        KeyBinding::new("enter", dialogs::Confirm, Some("ListDialog")),
        KeyBinding::new("escape", dialogs::Cancel, Some("ListDialog")),
        KeyBinding::new("up", dialogs::SelectPrev, Some("ListDialog")),
        KeyBinding::new("down", dialogs::SelectNext, Some("ListDialog")),
        KeyBinding::new("tab", dialogs::NextMode, Some("NavigateDialog")),
        KeyBinding::new("shift-tab", dialogs::PrevMode, Some("NavigateDialog")),
    ]);
    let pane = Some("EditorPane");
    let find = Some("FindBar");
    cx.bind_keys([
        KeyBinding::new("cmd-f", ShowFind, pane),
        KeyBinding::new("cmd-r", ShowReplace, pane),
        KeyBinding::new("cmd-g", FindNext, pane),
        KeyBinding::new("f3", FindNext, pane),
        KeyBinding::new("cmd-shift-g", FindPrev, pane),
        KeyBinding::new("shift-f3", FindPrev, pane),
        KeyBinding::new("enter", FindNext, find),
        KeyBinding::new("shift-enter", FindPrev, find),
        KeyBinding::new("escape", CloseFind, find),
        KeyBinding::new("alt-c", ToggleCase, find),
        KeyBinding::new("alt-w", ToggleWord, find),
        KeyBinding::new("alt-x", ToggleRegex, find),
        KeyBinding::new("enter", ReplaceNext, Some("ReplaceField")),
        KeyBinding::new("cmd-alt-enter", ReplaceAll, find),
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
