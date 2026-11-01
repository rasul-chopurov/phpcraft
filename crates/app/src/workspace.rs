//! Main window: project tree, editor tabs and status bar.

mod navigation;
mod tools;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::{
    Context, Entity, FocusHandle, Focusable, MouseButton, SharedString, Window, actions, div,
    prelude::*, px,
};
use editor::Buffer;
use project::{Project, Row};

use crate::editor_view::EditorView;
use crate::intel::Intel;
use php_index::Location;
pub use navigation::{FindInFiles, GoToClass, GoToFile, GoToLine, GoToSymbol, RecentFiles, ReplaceInFiles, SearchEverywhere};
pub use tools::{GenerateHelperCode, OpenArtisan, OpenMake, OpenRouteSearch};
use crate::theme;

actions!(workspace, [CloseTab, NextTab, PrevTab, NavigateBack]);

pub struct Workspace {
    project: Project,
    rows: Vec<Row>,
    tabs: Vec<Entity<EditorView>>,
    active: usize,
    selected_path: Option<PathBuf>,
    focus_handle: FocusHandle,
    intel: Arc<Intel>,
    /// Positions to return to with ⌘[ (file, caret char index).
    history: Vec<(PathBuf, usize)>,
    modal: Option<gpui::AnyView>,
    modal_subscription: Option<gpui::Subscription>,
    output: Option<tools::Output>,
    artisan_commands: tools::CommandCache,
    laravel_menu_open: bool,
    recent: Vec<PathBuf>,
    files_cache: Option<Arc<Vec<crate::dialogs::NavItem>>>,
    shift_clean: bool,
    last_shift_tap: Option<std::time::Instant>,
}

impl Workspace {
    pub fn new(project: Project, cx: &mut Context<Self>) -> Self {
        let rows = project.rows();
        let intel = Intel::new(project.root());
        let scanning = intel.clone();
        cx.spawn(async move |this, cx| {
            cx.background_executor().spawn(async move { scanning.scan() }).await;
            this.update(cx, |_, cx| cx.notify()).ok();
        })
        .detach();
        Self {
            intel,
            history: Vec::new(),
            modal: None,
            modal_subscription: None,
            output: None,
            artisan_commands: None,
            laravel_menu_open: false,
            recent: Vec::new(),
            files_cache: None,
            shift_clean: false,
            last_shift_tap: None,
            project,
            rows,
            tabs: Vec::new(),
            active: 0,
            selected_path: None,
            focus_handle: cx.focus_handle(),
        }
    }

    pub fn title(&self) -> String {
        format!("{} – PhpCraft", self.project.name())
    }

    fn click_row(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self.rows.get(ix) else { return };
        let path = row.entry.path.clone();
        self.selected_path = Some(path.clone());
        if row.entry.is_dir {
            self.project.toggle(&path);
            self.rows = self.project.rows();
        } else {
            self.open_file(&path, window, cx);
        }
        cx.notify();
    }

    pub fn open_file(&mut self, path: &Path, window: &mut Window, cx: &mut Context<Self>) {
        self.remember_recent(path);
        if let Some(ix) = self
            .tabs
            .iter()
            .position(|tab| tab.read(cx).buffer.path() == Some(path))
        {
            self.activate(ix, window, cx);
            return;
        }
        let buffer = match Buffer::open(path) {
            Ok(buffer) => buffer,
            Err(err) => {
                eprintln!("cannot open {}: {err:#}", path.display());
                return;
            }
        };
        let intel = self.intel.clone();
        let workspace = cx.weak_entity();
        let editor = cx.new(|cx| EditorView::new(buffer, intel, workspace, cx));
        cx.observe(&editor, |_, _, cx| cx.notify()).detach();
        self.tabs.push(editor);
        self.activate(self.tabs.len() - 1, window, cx);
    }

    /// Opens `location` (a byte range), remembering `from` for ⌘[.
    pub fn open_location(
        &mut self,
        location: &Location,
        from: Option<(PathBuf, usize)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(from) = from {
            self.history.push(from);
        }
        self.open_file(&location.path, window, cx);
        let start = location.range.start;
        if let Some(tab) = self.active_tab_for(&location.path, cx) {
            tab.update(cx, |editor, cx| editor.reveal_byte(start, cx));
        }
        self.selected_path = Some(location.path.clone());
    }

    fn active_tab_for(&self, path: &Path, cx: &gpui::App) -> Option<Entity<EditorView>> {
        self.tabs
            .iter()
            .find(|tab| tab.read(cx).buffer.path() == Some(path))
            .cloned()
    }

    fn navigate_back(&mut self, _: &NavigateBack, window: &mut Window, cx: &mut Context<Self>) {
        let Some((path, char_ix)) = self.history.pop() else { return };
        self.open_file(&path, window, cx);
        if let Some(tab) = self.active_tab_for(&path, cx) {
            tab.update(cx, |editor, cx| editor.reveal_char(char_ix, cx));
        }
    }

    fn focus_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.tabs.get(self.active) {
            Some(tab) => window.focus(&tab.focus_handle(cx)),
            None => window.focus(&self.focus_handle),
        }
    }

    fn activate(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.active = ix;
        if let Some(tab) = self.tabs.get(ix) {
            window.focus(&tab.focus_handle(cx));
        }
        cx.notify();
    }

    fn close_tab_at(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if ix >= self.tabs.len() {
            return;
        }
        self.tabs.remove(ix);
        let next = self.active.min(self.tabs.len().saturating_sub(1));
        if self.tabs.is_empty() {
            window.focus(&self.focus_handle);
            cx.notify();
        } else {
            self.activate(next, window, cx);
        }
    }

    fn close_tab(&mut self, _: &CloseTab, window: &mut Window, cx: &mut Context<Self>) {
        self.close_tab_at(self.active, window, cx);
    }

    fn next_tab(&mut self, _: &NextTab, window: &mut Window, cx: &mut Context<Self>) {
        if !self.tabs.is_empty() {
            self.activate((self.active + 1) % self.tabs.len(), window, cx);
        }
    }

    fn prev_tab(&mut self, _: &PrevTab, window: &mut Window, cx: &mut Context<Self>) {
        if !self.tabs.is_empty() {
            let len = self.tabs.len();
            self.activate((self.active + len - 1) % len, window, cx);
        }
    }

    fn render_tree(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = self.rows.iter().enumerate().map(|(ix, row)| {
            let selected = self.selected_path.as_deref() == Some(row.entry.path.as_path());
            let icon = match (row.entry.is_dir, row.expanded) {
                (true, true) => "▾ ",
                (true, false) => "▸ ",
                (false, _) => "  ",
            };
            let label: SharedString = format!("{icon}{}", row.entry.name).into();
            let color = if row.entry.is_dir {
                theme::ui_text()
            } else if row.entry.name.ends_with(".php") {
                theme::syntax(php_syntax::Highlight::Number)
            } else {
                theme::ui_text()
            };
            div()
                .id(ix)
                .pl(px(8. + 14. * row.depth as f32))
                .h(px(22.))
                .flex()
                .items_center()
                .text_color(color)
                .when(selected, |d| d.bg(theme::active_row()))
                .hover(|d| d.bg(theme::hover()))
                .child(label)
                .on_click(cx.listener(move |this, _, window, cx| this.click_row(ix, window, cx)))
        });
        div()
            .id("project-tree")
            .w(px(260.))
            .h_full()
            .flex_none()
            .overflow_y_scroll()
            .bg(theme::panel_bg())
            .border_r_1()
            .border_color(theme::border())
            .text_size(px(13.))
            .child(
                div()
                    .px_2()
                    .h(px(26.))
                    .flex()
                    .items_center()
                    .text_color(theme::muted())
                    .child("Project"),
            )
            .children(rows)
    }

    fn render_tabs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let tabs = self.tabs.iter().enumerate().map(|(ix, tab)| {
            let buffer = &tab.read(cx).buffer;
            let title: SharedString =
                format!("{}{}", buffer.title(), if buffer.is_dirty() { " •" } else { "" }).into();
            let active = ix == self.active;
            div()
                .id(("tab", ix))
                .flex()
                .items_center()
                .gap_2()
                .px_3()
                .h_full()
                .border_r_1()
                .border_color(theme::border())
                .when(active, |d| {
                    d.bg(theme::tab_active()).border_b_2().border_color(theme::accent())
                })
                .hover(|d| d.bg(theme::hover()))
                .child(title)
                .child(
                    div()
                        .id(("close", ix))
                        .text_color(theme::muted())
                        .hover(|d| d.text_color(theme::ui_text()))
                        .child("×")
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.close_tab_at(ix, window, cx)
                        })),
                )
                .on_click(cx.listener(move |this, _, window, cx| this.activate(ix, window, cx)))
        });
        div()
            .id("tabs")
            .flex()
            .flex_none()
            .h(px(30.))
            .w_full()
            .overflow_x_scroll()
            .bg(theme::panel_bg())
            .border_b_1()
            .border_color(theme::border())
            .text_size(px(13.))
            .text_color(theme::ui_text())
            .children(tabs)
    }

    fn render_status(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let info: SharedString = match self.tabs.get(self.active) {
            Some(tab) => {
                let buffer = &tab.read(cx).buffer;
                let (line, col) = buffer.char_to_line_col(buffer.selection.head);
                let lang = buffer.lang().name();
                format!("{}:{}    LF    UTF-8    {lang}", line + 1, col + 1).into()
            }
            None => "".into(),
        };
        div()
            .flex()
            .flex_none()
            .justify_between()
            .items_center()
            .px_3()
            .h(px(24.))
            .bg(theme::panel_bg())
            .border_t_1()
            .border_color(theme::border())
            .text_size(px(12.))
            .text_color(theme::muted())
            .child(SharedString::from(self.project.root().display().to_string()))
            .child(SharedString::from(self.index_status()))
            .child(info)
    }
}

impl Workspace {
    fn index_status(&self) -> String {
        let framework = if self.intel.is_laravel() { "Laravel · " } else { "" };
        if self.intel.is_ready() {
            format!("{framework}{} classes indexed", self.intel.class_count())
        } else {
            format!("{framework}Indexing…")
        }
    }
}

impl Focusable for Workspace {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for Workspace {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let editor = match self.tabs.get(self.active) {
            Some(tab) => div().flex_1().overflow_hidden().child(tab.clone()),
            None => div()
                .flex_1()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_2()
                .bg(theme::editor_bg())
                .text_color(theme::muted())
                .child("Откройте файл в дереве проекта")
                .child("⌘W закрыть вкладку · ⌃Tab следующая · ⌘S сохранить"),
        };
        div()
            .key_context("Workspace")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::close_tab))
            .on_action(cx.listener(Self::next_tab))
            .on_action(cx.listener(Self::prev_tab))
            .on_action(cx.listener(Self::navigate_back))
            .on_action(cx.listener(Self::open_make))
            .on_action(cx.listener(Self::open_artisan))
            .on_action(cx.listener(Self::open_route_search))
            .on_action(cx.listener(Self::new_model))
            .on_action(cx.listener(Self::new_pivot_table))
            .on_action(cx.listener(Self::toggle_laravel_menu))
            .on_action(cx.listener(Self::open_laravel_settings))
            .on_action(cx.listener(Self::generate_helper_code))
            .on_action(cx.listener(Self::remove_model_phpdocs))
            .on_action(cx.listener(Self::model_from_table))
            .on_action(cx.listener(Self::find_unused_views))
            .on_action(cx.listener(Self::find_in_files))
            .on_action(cx.listener(Self::replace_in_files))
            .on_action(cx.listener(Self::search_everywhere))
            .on_action(cx.listener(Self::go_to_file))
            .on_action(cx.listener(Self::go_to_class))
            .on_action(cx.listener(Self::go_to_symbol))
            .on_action(cx.listener(Self::recent_files))
            .on_action(cx.listener(Self::go_to_line))
            .on_modifiers_changed(cx.listener(Self::on_modifiers))
            .capture_key_down(cx.listener(|this, _: &gpui::KeyDownEvent, _, _| this.on_any_key()))
            .on_action(cx.listener(Self::close_output))
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .font_family(theme::UI_FONT)
            .bg(theme::editor_bg())
            .child(self.render_toolbar(cx))
            .child(
                div()
                    .flex_1()
                    .flex()
                    .overflow_hidden()
                    .child(self.render_tree(cx))
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .overflow_hidden()
                            .child(self.render_tabs(cx))
                            .child(editor)
                            .children(self.render_output(cx)),
                    ),
            )
            .child(self.render_status(cx))
            .children(self.render_modal(cx))
    }
}
