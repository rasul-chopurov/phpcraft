//! Main window: project tree, editor tabs and status bar.

use std::path::{Path, PathBuf};

use gpui::{
    Context, Entity, FocusHandle, Focusable, MouseButton, SharedString, Window, actions, div,
    prelude::*, px,
};
use editor::Buffer;
use project::{Project, Row};

use crate::editor_view::EditorView;
use crate::theme;

actions!(workspace, [CloseTab, NextTab, PrevTab]);

pub struct Workspace {
    project: Project,
    rows: Vec<Row>,
    tabs: Vec<Entity<EditorView>>,
    active: usize,
    selected_path: Option<PathBuf>,
    focus_handle: FocusHandle,
}

impl Workspace {
    pub fn new(project: Project, cx: &mut Context<Self>) -> Self {
        let rows = project.rows();
        Self {
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
        let editor = cx.new(|cx| EditorView::new(buffer, cx));
        cx.observe(&editor, |_, _, cx| cx.notify()).detach();
        self.tabs.push(editor);
        self.activate(self.tabs.len() - 1, window, cx);
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
                let lang = if buffer.is_php() { "PHP" } else { "Text" };
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
            .child(info)
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
            .size_full()
            .flex()
            .flex_col()
            .font_family(theme::UI_FONT)
            .bg(theme::editor_bg())
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
                            .child(editor),
                    ),
            )
            .child(self.render_status(cx))
    }
}
