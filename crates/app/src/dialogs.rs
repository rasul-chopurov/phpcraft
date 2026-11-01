//! Modal dialogs: Laravel Make (code generation) and Artisan command runner.

mod find_files;
mod git;
mod make;
mod navigate;
mod prefs;
mod datasource;
mod input;
mod projects;
mod run;

use std::path::Path;
use std::sync::Arc;

use gpui::{
    App, Context, Entity, EventEmitter, FocusHandle, Focusable, MouseButton, SharedString,
    Subscription, Window, actions, div, prelude::*, px,
};
use laravel::artisan::{self, ArtisanCommand};
use laravel::make::MakeKind;
use laravel::routes::RouteInfo;

pub use find_files::FindInFilesDialog;
pub use git::{BranchDialog, ConfirmDialog, GitCommand, LocalHistoryDialog};
pub use make::{GeneratorDialog, MakeChooser, MakeDialog};
pub use navigate::{GoToLineDialog, NavItem, NavMode, NavSources, NavTarget, NavigateDialog};
pub use prefs::{KeymapDialog, SettingsDialog, ThemeDialog};
pub use datasource::DataSourceDialog;
pub use input::{FileOp, InputDialog};
pub use projects::{ProjectDialog, avatar, project_name, tilde};
pub use run::RunDialog;

use crate::text_field::{FieldEvent, TextField};
use crate::theme;

actions!(
    dialog,
    [
        Confirm,
        Cancel,
        FocusNext,
        FocusPrev,
        SelectPrev,
        SelectNext,
        Complete,
        MoveRowUp,
        MoveRowDown,
        DeleteRow
    ]
);

pub enum DialogEvent {
    Dismiss,
    /// The chooser picked a generator; open its dialog.
    OpenMakeKind(MakeKind),
    /// A generator that only needs a name (Laravel Idea's simple Create… actions).
    OpenGenerator(laravel::generators::Simple),
    Created(Vec<std::path::PathBuf>),
    RunArtisan(String),
    /// Open the small arguments/options dialog for a command.
    ConfigureArtisan(ArtisanCommand),
    OpenRoute(RouteInfo),
    OpenLocation(php_index::Location),
    /// Shift+Enter in Search Everywhere.
    OpenLocationInSplit(php_index::Location),
    ReplaceInFiles {
        query: search::TextQuery,
        replacement: String,
        paths: Vec<std::path::PathBuf>,
    },
    /// The project file list was computed; the workspace caches it.
    FilesListed(Arc<Vec<NavItem>>),
    RunAction(Box<dyn gpui::Action>),
    /// 0-based line and column.
    GoToLine(usize, usize),
    SettingsSaved,
    Git(GitCommand),
    /// Local History: put this content back into the file.
    RevertFile { path: std::path::PathBuf, content: String },
    GenerateHelpers,
    ModelFromTable(String),
    CommandsLoaded(Arc<Vec<ArtisanCommand>>),
    /// Project switcher: choose a folder to open in a new window.
    PickProjectFolder,
    OpenProject(std::path::PathBuf, OpenMode),
    CloneRepository { url: String, parent: std::path::PathBuf },
    /// Run a configuration; `save` stores it in `.phpcraft/run.json`.
    Run { config: crate::run_configs::RunConfig, save: bool },
    EditRunConfigurations,
    /// From [`InputDialog`]: `name` is empty for Delete.
    FileOp { op: FileOp, name: String },
    DataSourceSaved { source: laravel::datasource::DataSource, password: String },
    /// Run a shell command in a new terminal tab (agent sign-in from Settings).
    RunInTerminal(String),
}

/// Where a project opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenMode {
    /// Instead of the current project.
    Replace,
    NewTab,
    NewWindow,
}

/// A one-line tooltip.
pub struct TextTooltip(pub SharedString);

impl Render for TextTooltip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_2()
            .py_1()
            .rounded_md()
            .bg(theme::panel_bg())
            .border_1()
            .border_color(theme::border_strong())
            .text_size(px(12.))
            .text_color(theme::ui_text())
            .child(self.0.clone())
    }
}

/// The square of a checkbox, without a label.
pub(crate) fn checkbox_box(checked: bool) -> gpui::Div {
    div()
        .size(px(14.))
        .rounded_sm()
        .border_1()
        .border_color(if checked { theme::accent() } else { theme::border_strong() })
        .when(checked, |d| d.bg(theme::accent()))
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(10.))
        .text_color(theme::editor_bg())
        .child(if checked { "✓" } else { "" })
}

pub(crate) fn checkbox(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    checked: bool,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(gpui::ElementId::Name(id.into()))
        .flex()
        .items_center()
        .gap_1()
        .cursor_pointer()
        .child(checkbox_box(checked))
        .child(label.into())
}

pub(crate) fn button(id: &'static str, text: &'static str, primary: bool) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .px_3()
        .py_1()
        .rounded_sm()
        .cursor_pointer()
        .when(primary, |d| d.bg(theme::accent()).text_color(gpui::white()))
        .when(!primary, |d| {
            d.border_1().border_color(theme::border_strong())
        })
        .hover(|d| d.opacity(0.85))
        .child(text)
}

fn card(width: f32) -> gpui::Div {
    div()
        .w(px(width))
        .bg(theme::panel_bg())
        .border_1()
        .border_color(theme::border_strong())
        .rounded_lg()
        .overflow_hidden()
        .shadow_lg()
        .text_size(px(13.))
        .text_color(theme::ui_text())
        // Clicks inside the card must not reach the dismissing overlay.
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
}

// --- Artisan ------------------------------------------------------------------

pub struct ArtisanDialog {
    query: Entity<TextField>,
    commands: Option<Arc<Vec<ArtisanCommand>>>,
    error: Option<String>,
    selected: usize,
    focus_handle: FocusHandle,
    _subscription: Subscription,
}

impl EventEmitter<DialogEvent> for ArtisanDialog {}

impl ArtisanDialog {
    pub fn new(
        root: &Path,
        cached: Option<Arc<Vec<ArtisanCommand>>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let query = cx.new(|cx| {
            TextField::new(
                "Type a command, e.g. make:model Post -mf, migrate, route:list",
                cx,
            )
        });
        let subscription = cx.subscribe(&query, |this: &mut Self, _, _: &FieldEvent, cx| {
            this.selected = 0;
            cx.notify();
        });
        window.focus(&query.focus_handle(cx));
        if cached.is_none() {
            let root = root.to_path_buf();
            cx.spawn(async move |this, cx| {
                let result = cx
                    .background_executor()
                    .spawn(async move { artisan::list_commands(&root) })
                    .await;
                this.update(cx, |this, cx| {
                    match result {
                        Ok(list) => {
                            let list = Arc::new(list);
                            this.commands = Some(list.clone());
                            cx.emit(DialogEvent::CommandsLoaded(list));
                        }
                        Err(err) => this.error = Some(err),
                    }
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
        Self {
            query,
            commands: cached,
            error: None,
            selected: 0,
            focus_handle: cx.focus_handle(),
            _subscription: subscription,
        }
    }

    fn matches(&self, cx: &App) -> Vec<ArtisanCommand> {
        let Some(commands) = &self.commands else {
            return Vec::new();
        };
        let query = self.query.read(cx).text();
        let word = query
            .split_whitespace()
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        let mut found: Vec<_> = commands
            .iter()
            .filter(|c| {
                let name = c.name.to_ascii_lowercase();
                let mut chars = name.chars();
                word.chars().all(|w| chars.any(|c| c == w))
            })
            .cloned()
            .collect();
        found.sort_by_key(|c| {
            (
                !c.name.starts_with(&word),
                !c.name.contains(&word),
                c.name.len(),
            )
        });
        found
    }

    fn select_prev(&mut self, _: &SelectPrev, _: &mut Window, cx: &mut Context<Self>) {
        self.selected = self.selected.saturating_sub(1);
        cx.notify();
    }

    fn select_next(&mut self, _: &SelectNext, _: &mut Window, cx: &mut Context<Self>) {
        let count = self.matches(cx).len();
        if count > 0 {
            self.selected = (self.selected + 1).min(count - 1);
        }
        cx.notify();
    }

    /// Tab: put the selected command name into the field so arguments can be typed.
    fn complete(&mut self, _: &Complete, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(command) = self.matches(cx).get(self.selected) {
            let text = format!("{} ", command.name);
            self.query.update(cx, |q, cx| q.set_text(text, cx));
        }
    }

    fn confirm(&mut self, _: &Confirm, _: &mut Window, cx: &mut Context<Self>) {
        let query = self.query.read(cx).text().trim().to_string();
        let first = query.split_whitespace().next().unwrap_or("");
        let exact = self
            .commands
            .as_ref()
            .is_some_and(|c| c.iter().any(|c| c.name == first));
        // Typed arguments mean "run as written"; otherwise configure the command first.
        if query.contains(' ') {
            cx.emit(DialogEvent::RunArtisan(query));
            return;
        }
        let command = match (exact, &self.commands) {
            (true, Some(commands)) => commands.iter().find(|c| c.name == first).cloned(),
            _ => self.matches(cx).get(self.selected).cloned(),
        };
        match command {
            Some(command) => cx.emit(DialogEvent::ConfigureArtisan(command)),
            None if !query.is_empty() => cx.emit(DialogEvent::RunArtisan(query)),
            None => {}
        }
    }

    fn cancel(&mut self, _: &Cancel, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(DialogEvent::Dismiss);
    }
}

impl Focusable for ArtisanDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ArtisanDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let matches = self.matches(cx);
        let selected = self.selected.min(matches.len().saturating_sub(1));
        let first = selected.saturating_sub(13);
        let rows = matches
            .iter()
            .enumerate()
            .skip(first)
            .take(14)
            .map(|(ix, command)| {
                div()
                    .id(("cmd", ix))
                    .flex()
                    .gap_3()
                    .px_3()
                    .h(px(24.))
                    .items_center()
                    .when(ix == selected, |d| d.bg(theme::selection()))
                    .hover(|d| d.bg(theme::hover()))
                    .cursor_pointer()
                    .child(div().text_color(theme::error()).child("◈"))
                    .child(
                        div()
                            .w(px(220.))
                            .flex_none()
                            .font_family(theme::editor_font())
                            .text_color(theme::syntax(php_syntax::Highlight::Function))
                            .child(command.name.clone()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .overflow_hidden()
                            .text_color(theme::muted())
                            .child(command.description.clone()),
                    )
                    .on_click(
                        cx.listener(move |this, event: &gpui::ClickEvent, window, cx| {
                            this.selected = ix;
                            if event.click_count() >= 2 {
                                this.confirm(&Confirm, window, cx);
                            }
                            cx.notify();
                        }),
                    )
            });
        let status = match (&self.commands, &self.error) {
            (_, Some(err)) => Some(
                div()
                    .px_3()
                    .py_2()
                    .text_color(theme::error())
                    .child(err.clone()),
            ),
            (None, None) => Some(
                div()
                    .px_3()
                    .py_2()
                    .text_color(theme::muted())
                    .child("Loading artisan commands…"),
            ),
            _ => None,
        };
        card(720.)
            .key_context("Dialog ArtisanDialog")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::cancel))
            .on_action(cx.listener(Self::select_prev))
            .on_action(cx.listener(Self::select_next))
            .on_action(cx.listener(Self::complete))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .p_2()
                    .border_b_1()
                    .border_color(theme::border())
                    .child(
                        div()
                            .font_family(theme::editor_font())
                            .text_color(theme::muted())
                            .child("php artisan"),
                    )
                    .child(div().flex_1().child(self.query.clone())),
            )
            .children(status)
            .child(div().py_1().children(rows))
            .child(
                div()
                    .px_3()
                    .py_1()
                    .border_t_1()
                    .border_color(theme::border())
                    .text_size(px(11.))
                    .text_color(theme::muted())
                    .child("↑↓ select · Tab complete · ↵ configure & run · Esc close"),
            )
    }
}

// --- Artisan: run one command -------------------------------------------------

enum OptionInput {
    Flag(bool),
    Value(Entity<TextField>),
}

/// Small form for one command's arguments and options, opened before running it.
pub struct ArtisanRunDialog {
    command: ArtisanCommand,
    arguments: Vec<Entity<TextField>>,
    options: Vec<OptionInput>,
    focus_handle: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<DialogEvent> for ArtisanRunDialog {}

impl ArtisanRunDialog {
    pub fn new(command: ArtisanCommand, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut subscriptions = Vec::new();
        let mut watch = |field: &Entity<TextField>, cx: &mut Context<Self>| {
            subscriptions
                .push(cx.subscribe(field, |_: &mut Self, _, _: &FieldEvent, cx| cx.notify()));
        };
        let arguments: Vec<_> = command
            .arguments
            .iter()
            .map(|a| {
                let placeholder = a.default.clone().unwrap_or_else(|| a.description.clone());
                let field = cx.new(|cx| TextField::new(placeholder, cx));
                watch(&field, cx);
                field
            })
            .collect();
        let options = command
            .options
            .iter()
            .map(|o| {
                if o.accepts_value {
                    let placeholder = o.default.clone().unwrap_or_default();
                    let field = cx.new(|cx| TextField::new(placeholder, cx));
                    watch(&field, cx);
                    OptionInput::Value(field)
                } else {
                    OptionInput::Flag(false)
                }
            })
            .collect();
        let focus_handle = cx.focus_handle();
        match arguments.first() {
            Some(first) => window.focus(&first.focus_handle(cx)),
            None => window.focus(&focus_handle),
        }
        Self {
            command,
            arguments,
            options,
            focus_handle,
            _subscriptions: subscriptions,
        }
    }

    fn line(&self, cx: &App) -> String {
        let arguments: Vec<String> = self
            .arguments
            .iter()
            .map(|f| f.read(cx).text().to_string())
            .collect();
        let options: Vec<(String, Option<String>)> = self
            .command
            .options
            .iter()
            .zip(&self.options)
            .filter_map(|(option, input)| match input {
                OptionInput::Flag(true) => Some((option.name.clone(), None)),
                OptionInput::Flag(false) => None,
                OptionInput::Value(field) => {
                    let value = field.read(cx).text().trim().to_string();
                    (!value.is_empty()).then(|| (option.name.clone(), Some(value)))
                }
            })
            .collect();
        artisan::command_line(&self.command, &arguments, &options)
    }

    fn missing_required(&self, cx: &App) -> bool {
        self.command
            .arguments
            .iter()
            .zip(&self.arguments)
            .any(|(a, f)| a.required && f.read(cx).text().trim().is_empty())
    }

    fn confirm(&mut self, _: &Confirm, _: &mut Window, cx: &mut Context<Self>) {
        if !self.missing_required(cx) {
            cx.emit(DialogEvent::RunArtisan(self.line(cx)));
        }
    }

    fn cancel(&mut self, _: &Cancel, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(DialogEvent::Dismiss);
    }

    fn cycle(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let mut fields: Vec<&Entity<TextField>> = self.arguments.iter().collect();
        fields.extend(self.options.iter().filter_map(|o| match o {
            OptionInput::Value(f) => Some(f),
            OptionInput::Flag(_) => None,
        }));
        if fields.is_empty() {
            return;
        }
        let current = fields
            .iter()
            .position(|f| f.focus_handle(cx).is_focused(window))
            .unwrap_or(0);
        let next = (current as isize + delta).rem_euclid(fields.len() as isize) as usize;
        window.focus(&fields[next].focus_handle(cx));
    }

    fn focus_next(&mut self, _: &FocusNext, window: &mut Window, cx: &mut Context<Self>) {
        self.cycle(1, window, cx);
    }

    fn focus_prev(&mut self, _: &FocusPrev, window: &mut Window, cx: &mut Context<Self>) {
        self.cycle(-1, window, cx);
    }
}

impl Focusable for ArtisanRunDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ArtisanRunDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let arguments =
            self.command
                .arguments
                .iter()
                .zip(&self.arguments)
                .map(|(argument, field)| {
                    let title = format!(
                        "{}{}",
                        argument.name,
                        if argument.required { " *" } else { "" }
                    );
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .w(px(140.))
                                .flex_none()
                                .font_family(theme::editor_font())
                                .child(title),
                        )
                        .child(div().flex_1().child(field.clone()))
                });
        let mut flags = Vec::new();
        let mut values = Vec::new();
        for (ix, (option, input)) in self.command.options.iter().zip(&self.options).enumerate() {
            let name = if option.shortcut.is_empty() {
                format!("--{}", option.name)
            } else {
                format!("{}, --{}", option.shortcut, option.name)
            };
            match input {
                OptionInput::Flag(checked) => flags.push(
                    checkbox(format!("flag-{ix}"), name, *checked)
                        .text_size(px(12.))
                        .font_family(theme::editor_font())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let OptionInput::Flag(checked) = &mut this.options[ix] {
                                *checked = !*checked;
                            }
                            cx.notify();
                        })),
                ),
                OptionInput::Value(field) => values.push(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .w(px(140.))
                                .flex_none()
                                .font_family(theme::editor_font())
                                .text_size(px(12.))
                                .child(format!("--{}=", option.name)),
                        )
                        .child(div().flex_1().child(field.clone())),
                ),
            }
        }
        let line = self.line(cx);
        let blocked = self.missing_required(cx);
        card(560.)
            .key_context("Dialog")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::cancel))
            .on_action(cx.listener(Self::focus_next))
            .on_action(cx.listener(Self::focus_prev))
            .child(
                div()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(theme::border())
                    .child(
                        div()
                            .font_family(theme::editor_font())
                            .child(self.command.name.clone()),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(theme::muted())
                            .child(self.command.description.clone()),
                    ),
            )
            .child(
                div()
                    .id("artisan-form")
                    .max_h(px(420.))
                    .overflow_y_scroll()
                    .p_3()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .children(arguments)
                    .children(values)
                    .when(!flags.is_empty(), |d| {
                        d.child(
                            div()
                                .flex()
                                .flex_wrap()
                                .gap_x_4()
                                .gap_y_2()
                                .mt_1()
                                .children(flags),
                        )
                    }),
            )
            .child(
                div()
                    .mx_3()
                    .p_2()
                    .rounded_sm()
                    .bg(theme::editor_bg())
                    .font_family(theme::editor_font())
                    .text_size(px(12.))
                    .text_color(theme::success())
                    .child(format!("php artisan {line}")),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .p_3()
                    .child(
                        button("artisan-cancel", "Cancel", false)
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(DialogEvent::Dismiss))),
                    )
                    .child(
                        button("artisan-run", "Run", true)
                            .when(blocked, |d| d.opacity(0.5))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.confirm(&Confirm, window, cx)
                            })),
                    ),
            )
    }
}

// --- Route Search -------------------------------------------------------------

/// Searchable list of registered routes; Enter opens the route's action.
pub struct RouteSearchDialog {
    query: Entity<TextField>,
    routes: Option<Arc<Vec<RouteInfo>>>,
    error: Option<String>,
    selected: usize,
    focus_handle: FocusHandle,
    _subscription: Subscription,
}

impl EventEmitter<DialogEvent> for RouteSearchDialog {}

impl RouteSearchDialog {
    pub fn new(root: &Path, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let query = cx.new(|cx| TextField::new("Search by method, URI, name or action", cx));
        let subscription = cx.subscribe(&query, |this: &mut Self, _, _: &FieldEvent, cx| {
            this.selected = 0;
            cx.notify();
        });
        window.focus(&query.focus_handle(cx));
        let root = root.to_path_buf();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { laravel::routes::list_routes(&root) })
                .await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(routes) => this.routes = Some(Arc::new(routes)),
                    Err(err) => this.error = Some(err),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        Self {
            query,
            routes: None,
            error: None,
            selected: 0,
            focus_handle: cx.focus_handle(),
            _subscription: subscription,
        }
    }

    fn matches(&self, cx: &App) -> Vec<RouteInfo> {
        let Some(routes) = &self.routes else {
            return Vec::new();
        };
        let words: Vec<String> = self
            .query
            .read(cx)
            .text()
            .to_ascii_lowercase()
            .split_whitespace()
            .map(str::to_string)
            .collect();
        routes
            .iter()
            .filter(|r| {
                let haystack = r.haystack();
                words.iter().all(|w| haystack.contains(w.as_str()))
            })
            .cloned()
            .collect()
    }

    fn select_prev(&mut self, _: &SelectPrev, _: &mut Window, cx: &mut Context<Self>) {
        self.selected = self.selected.saturating_sub(1);
        cx.notify();
    }

    fn select_next(&mut self, _: &SelectNext, _: &mut Window, cx: &mut Context<Self>) {
        let count = self.matches(cx).len();
        self.selected = (self.selected + 1).min(count.saturating_sub(1));
        cx.notify();
    }

    fn confirm(&mut self, _: &Confirm, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(route) = self.matches(cx).get(self.selected) {
            cx.emit(DialogEvent::OpenRoute(route.clone()));
        }
    }

    fn cancel(&mut self, _: &Cancel, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(DialogEvent::Dismiss);
    }
}

impl Focusable for RouteSearchDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for RouteSearchDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let matches = self.matches(cx);
        let selected = self.selected.min(matches.len().saturating_sub(1));
        let first = selected.saturating_sub(17);
        let rows = matches
            .iter()
            .enumerate()
            .skip(first)
            .take(18)
            .map(|(ix, route)| {
                div()
                    .id(("route", ix))
                    .flex()
                    .gap_2()
                    .px_3()
                    .h(px(24.))
                    .items_center()
                    .when(ix == selected, |d| d.bg(theme::selection()))
                    .hover(|d| d.bg(theme::hover()))
                    .cursor_pointer()
                    .child(div().text_color(theme::error()).child("◈"))
                    .child(
                        div()
                            .text_color(theme::ui_text())
                            .child(route.method.replace('|', " | ")),
                    )
                    .child(
                        div()
                            .text_color(theme::text())
                            .child(format!("/{}", route.uri.trim_start_matches('/'))),
                    )
                    .children(
                        route
                            .name
                            .clone()
                            .map(|n| div().text_color(theme::muted()).child(format!("[{n}]"))),
                    )
                    .on_click(
                        cx.listener(move |this, event: &gpui::ClickEvent, window, cx| {
                            this.selected = ix;
                            if event.click_count() >= 2 {
                                this.confirm(&Confirm, window, cx);
                            }
                            cx.notify();
                        }),
                    )
            });
        let status = match (&self.routes, &self.error) {
            (_, Some(err)) => Some(
                div()
                    .px_3()
                    .py_2()
                    .text_color(theme::error())
                    .child(err.clone()),
            ),
            (None, None) => Some(
                div()
                    .px_3()
                    .py_2()
                    .text_color(theme::muted())
                    .child("Loading routes…"),
            ),
            (Some(_), None) if matches.is_empty() => Some(
                div()
                    .px_3()
                    .py_2()
                    .text_color(theme::muted())
                    .child("No routes match"),
            ),
            _ => None,
        };
        card(820.)
            .key_context("Dialog ArtisanDialog")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::cancel))
            .on_action(cx.listener(Self::select_prev))
            .on_action(cx.listener(Self::select_next))
            .child(
                div()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(theme::border())
                    .child("Route Search"),
            )
            .child(div().p_2().child(self.query.clone()))
            .children(status)
            .child(
                div()
                    .pb_2()
                    .font_family(theme::editor_font())
                    .text_size(px(12.))
                    .children(rows),
            )
    }
}

// --- Laravel Settings ------------------------------------------------------------

/// Languages & Frameworks › Laravel: directories, locale and controller base class.
pub struct LaravelSettingsDialog {
    root: std::path::PathBuf,
    settings: laravel::settings::LaravelSettings,
    fields: Vec<(&'static str, Entity<TextField>)>,
    status: String,
    error: Option<String>,
    focus_handle: FocusHandle,
}

impl EventEmitter<DialogEvent> for LaravelSettingsDialog {}

impl LaravelSettingsDialog {
    pub fn new(root: &Path, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let settings = laravel::settings::LaravelSettings::load(root);
        let values = [
            (
                "Laravel directory:",
                settings.laravel_dir.clone(),
                "(project root)",
            ),
            ("Default locale:", settings.default_locale.clone(), "en"),
            (
                "Controller base class:",
                settings.controller_base.clone(),
                "App\\Http\\Controllers\\Controller",
            ),
            ("Applications directory:", settings.app_dir.clone(), "app"),
            ("Config directory:", settings.config_dir.clone(), "config"),
            ("Public directory:", settings.public_dir.clone(), "public"),
        ];
        let fields: Vec<_> = values
            .into_iter()
            .map(|(label, value, placeholder)| {
                let field = cx.new(|cx| {
                    let mut f = TextField::new(placeholder, cx);
                    f.set_text(value, cx);
                    f
                });
                (label, field)
            })
            .collect();
        window.focus(&fields[0].1.focus_handle(cx));
        let laravel_root = settings.laravel_root(root);
        let status = if laravel_root.join("artisan").is_file() {
            let version = laravel::settings::framework_version(&laravel_root)
                .unwrap_or_else(|| "unknown".into());
            format!("Current state: Enabled. Framework: Laravel application. Version: {version}")
        } else {
            "Current state: Disabled. No artisan file in the Laravel directory.".to_string()
        };
        Self {
            root: root.to_path_buf(),
            settings,
            fields,
            status,
            error: None,
            focus_handle: cx.focus_handle(),
        }
    }

    fn confirm(&mut self, _: &Confirm, _: &mut Window, cx: &mut Context<Self>) {
        let value = |i: usize| self.fields[i].1.read(cx).text().trim().to_string();
        let mut settings = self.settings.clone();
        settings.laravel_dir = value(0);
        settings.default_locale = value(1);
        settings.controller_base = value(2).trim_start_matches('\\').to_string();
        settings.app_dir = value(3);
        settings.config_dir = value(4);
        settings.public_dir = value(5);
        match settings.save(&self.root) {
            Ok(()) => cx.emit(DialogEvent::SettingsSaved),
            Err(err) => {
                self.error = Some(format!("Failed to save: {err}"));
                cx.notify();
            }
        }
    }

    fn cancel(&mut self, _: &Cancel, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(DialogEvent::Dismiss);
    }

    fn cycle(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let current = self
            .fields
            .iter()
            .position(|(_, f)| f.focus_handle(cx).is_focused(window))
            .unwrap_or(0);
        let next = (current as isize + delta).rem_euclid(self.fields.len() as isize) as usize;
        window.focus(&self.fields[next].1.focus_handle(cx));
    }

    fn focus_next(&mut self, _: &FocusNext, window: &mut Window, cx: &mut Context<Self>) {
        self.cycle(1, window, cx);
    }

    fn focus_prev(&mut self, _: &FocusPrev, window: &mut Window, cx: &mut Context<Self>) {
        self.cycle(-1, window, cx);
    }
}

impl Focusable for LaravelSettingsDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for LaravelSettingsDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = self.fields.iter().map(|(label, field)| {
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(div().w(px(190.)).flex_none().child(*label))
                .child(div().flex_1().child(field.clone()))
        });
        card(680.)
            .key_context("Dialog")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::cancel))
            .on_action(cx.listener(Self::focus_next))
            .on_action(cx.listener(Self::focus_prev))
            .child(
                div()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(theme::border())
                    .child("Languages & Frameworks › Laravel › General"),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .p_3()
                    .child(div().text_color(theme::muted()).child(self.status.clone()))
                    .children(rows)
                    .children(
                        self.error
                            .clone()
                            .map(|e| div().text_color(theme::error()).child(e)),
                    ),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .p_3()
                    .border_t_1()
                    .border_color(theme::border())
                    .child(button("settings-cancel", "Cancel", false).on_click(
                        cx.listener(|this, _, window, cx| this.cancel(&Cancel, window, cx)),
                    ))
                    .child(button("settings-ok", "OK", true).on_click(
                        cx.listener(|this, _, window, cx| this.confirm(&Confirm, window, cx)),
                    )),
            )
    }
}

// --- Generate Helper Code ------------------------------------------------------------

pub struct HelperCodeDialog {
    root: std::path::PathBuf,
    settings: laravel::settings::LaravelSettings,
    open: bool,
    focus_handle: FocusHandle,
}

impl EventEmitter<DialogEvent> for HelperCodeDialog {}

impl HelperCodeDialog {
    pub fn new(root: &Path, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        window.focus(&focus_handle);
        Self {
            root: root.to_path_buf(),
            settings: laravel::settings::LaravelSettings::load(root),
            open: false,
            focus_handle,
        }
    }

    fn confirm(&mut self, _: &Confirm, _: &mut Window, cx: &mut Context<Self>) {
        let _ = self.settings.save(&self.root);
        cx.emit(DialogEvent::GenerateHelpers);
    }

    fn cancel(&mut self, _: &Cancel, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(DialogEvent::Dismiss);
    }
}

impl Focusable for HelperCodeDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for HelperCodeDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        use laravel::settings::HelperSource;
        let helper = &self.settings.helper;
        let source_label = match helper.source {
            HelperSource::Migrations => "Migrations",
            HelperSource::Database => "Database (php artisan model:show)",
        };
        let options = self.open.then(|| {
            div()
                .flex()
                .flex_col()
                .mt_1()
                .border_1()
                .border_color(theme::border_strong())
                .rounded_sm()
                .children(
                    [HelperSource::Migrations, HelperSource::Database]
                        .into_iter()
                        .map(|source| {
                            div()
                                .id(if source == HelperSource::Migrations {
                                    "src-migrations"
                                } else {
                                    "src-database"
                                })
                                .px_2()
                                .h(px(24.))
                                .flex()
                                .items_center()
                                .cursor_pointer()
                                .hover(|d| d.bg(theme::selection()))
                                .child(if source == HelperSource::Migrations {
                                    "Migrations"
                                } else {
                                    "Database (php artisan model:show)"
                                })
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.settings.helper.source = source;
                                    this.open = false;
                                    cx.notify();
                                }))
                        }),
                )
        });
        card(520.)
            .key_context("Dialog")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::cancel))
            .child(div().px_3().py_2().border_b_1().border_color(theme::border()).child("Generate Helper Code"))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .p_3()
                    .child(
                        div()
                            .flex()
                            .items_start()
                            .gap_2()
                            .child(div().w(px(130.)).pt_1().flex_none().child("Fetch data from:"))
                            .child(
                                div()
                                    .flex_1()
                                    .child(
                                        div()
                                            .id("helper-source")
                                            .h(px(28.))
                                            .px_2()
                                            .flex()
                                            .items_center()
                                            .justify_between()
                                            .rounded_sm()
                                            .border_1()
                                            .border_color(theme::border_strong())
                                            .bg(theme::editor_bg())
                                            .cursor_pointer()
                                            .child(source_label)
                                            .child("⌄")
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.open = !this.open;
                                                cx.notify();
                                            })),
                                    )
                                    .children(options),
                            ),
                    )
                    .child(checkbox("helper-counts", "Add \"*_count\" properties for \"*Many\" relations", helper.relation_counts).on_click(cx.listener(|this, _, _, cx| {
                        this.settings.helper.relation_counts ^= true;
                        cx.notify();
                    })))
                    .child(checkbox("helper-where", "Add \"where{Column}()\" methods", helper.where_methods).on_click(cx.listener(|this, _, _, cx| {
                        this.settings.helper.where_methods ^= true;
                        cx.notify();
                    })))
                    .child(div().text_size(px(11.)).text_color(theme::muted()).child(
                        "Model columns, relations and casts are added to code insight (completion, ⌘+click, types). Models are not modified.",
                    )),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .p_3()
                    .border_t_1()
                    .border_color(theme::border())
                    .child(button("helper-cancel", "Cancel", false).on_click(cx.listener(|this, _, window, cx| this.cancel(&Cancel, window, cx))))
                    .child(button("helper-ok", "OK", true).on_click(cx.listener(|this, _, window, cx| this.confirm(&Confirm, window, cx)))),
            )
    }
}
