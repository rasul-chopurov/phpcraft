//! Git dialogs: side-by-side diff, branches, confirmation.

use gpui::{App, Context, Entity, EventEmitter, FocusHandle, Focusable, SharedString, Subscription, Window, div, prelude::*, px};

use super::{Cancel, Confirm, DialogEvent, SelectNext, SelectPrev, button, card};
use crate::text_field::{FieldEvent, TextField};
use crate::theme;

/// What a git dialog asks the workspace to run.
#[derive(Debug, Clone)]
pub enum GitCommand {
    Checkout(String),
    CreateBranch { name: String, from: Option<String> },
    Merge(String),
    Rename { old: String, new: String },
    Delete(String),
    Pull,
    Push,
    Fetch,
    Rollback(Vec<git::FileStatus>),
}

const MAX_ROWS: usize = 5000;

// --- Diff -----------------------------------------------------------------------------

pub struct DiffDialog {
    title: String,
    rows: Vec<git::DiffRow>,
    open_path: Option<std::path::PathBuf>,
    focus_handle: FocusHandle,
}

impl EventEmitter<DialogEvent> for DiffDialog {}

impl DiffDialog {
    pub fn new(title: String, old: &str, new: &str, open_path: Option<std::path::PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        window.focus(&focus_handle);
        Self { title, rows: git::side_by_side(old, new), open_path, focus_handle }
    }

    fn cancel(&mut self, _: &Cancel, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(DialogEvent::Dismiss);
    }

    fn confirm(&mut self, _: &Confirm, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(DialogEvent::Dismiss);
    }
}

impl Focusable for DiffDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

fn side(line: &Option<(usize, String)>, bg: Option<gpui::Hsla>) -> gpui::Div {
    let (number, text) = match line {
        Some((n, text)) => ((n + 1).to_string(), text.replace('\t', "    ")),
        None => (String::new(), String::new()),
    };
    div()
        .flex_1()
        .flex()
        .min_w(px(0.))
        .overflow_hidden()
        .when_some(bg, |d, bg| d.bg(bg))
        .when(line.is_none(), |d| d.bg(theme::panel_bg()))
        .child(div().w(px(44.)).flex_none().pr_2().text_right().text_color(theme::line_number()).child(number))
        .child(div().flex_1().whitespace_nowrap().overflow_hidden().text_color(theme::text()).child(SharedString::from(text)))
}

impl Render for DiffDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let added = theme::success().opacity(0.18);
        let removed = theme::error().opacity(0.18);
        let changed = theme::accent().opacity(0.16);
        let (mut adds, mut dels) = (0, 0);
        for row in &self.rows {
            match row.tag {
                git::RowTag::Added => adds += 1,
                git::RowTag::Removed => dels += 1,
                git::RowTag::Changed => {
                    adds += usize::from(row.right.is_some());
                    dels += usize::from(row.left.is_some());
                }
                git::RowTag::Equal => {}
            }
        }
        let rows = self.rows.iter().take(MAX_ROWS).enumerate().map(|(ix, row)| {
            let (left_bg, right_bg) = match row.tag {
                git::RowTag::Equal => (None, None),
                git::RowTag::Removed => (Some(removed), None),
                git::RowTag::Added => (None, Some(added)),
                git::RowTag::Changed => (Some(changed), Some(changed)),
            };
            div()
                .id(("diff-row", ix))
                .flex()
                .h(px(19.))
                .child(side(&row.left, left_bg))
                .child(div().w(px(1.)).h_full().bg(theme::border()))
                .child(side(&row.right, right_bg))
        });
        let open_path = self.open_path.clone();
        card(1100.)
            .key_context("Dialog")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::cancel))
            .on_action(cx.listener(Self::confirm))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(theme::border())
                    .child(SharedString::from(self.title.clone()))
                    .child(div().text_color(theme::success()).child(format!("+{adds}")))
                    .child(div().text_color(theme::error()).child(format!("−{dels}")))
                    .child(div().flex_1())
                    .when_some(open_path, |d, path| {
                        d.child(button("diff-open", "Open in Editor", false).on_click(cx.listener(move |_, _, _, cx| {
                            cx.emit(DialogEvent::OpenLocation(php_index::Location { path: path.clone(), range: 0..0 }))
                        })))
                    })
                    .child(button("diff-close", "Close", false).on_click(cx.listener(|_, _, _, cx| cx.emit(DialogEvent::Dismiss)))),
            )
            .child(
                div()
                    .flex()
                    .text_size(px(11.))
                    .text_color(theme::muted())
                    .border_b_1()
                    .border_color(theme::border())
                    .child(div().flex_1().px_3().py_1().child("Base (HEAD)"))
                    .child(div().flex_1().px_3().py_1().child("Your version")),
            )
            .child(
                div()
                    .id("diff-rows")
                    .h(px(620.))
                    .overflow_y_scroll()
                    .font_family(theme::editor_font())
                    .text_size(px(12.))
                    .children(rows),
            )
    }
}

// --- Branches ---------------------------------------------------------------------------

pub struct BranchDialog {
    current: Option<String>,
    local: Vec<String>,
    remote: Vec<String>,
    query: Entity<TextField>,
    selected: usize,
    focus_handle: FocusHandle,
    _subscription: Subscription,
}

impl EventEmitter<DialogEvent> for BranchDialog {}

#[derive(Clone)]
enum BranchRow {
    Create(String),
    Local(String),
    Remote(String),
}

impl BranchDialog {
    pub fn new(repo: &git::Repo, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (local, remote) = repo.branches().unwrap_or_default();
        let query = cx.new(|cx| TextField::new("Search branches, or type a new branch name", cx));
        let subscription = cx.subscribe(&query, |this: &mut Self, _, _: &FieldEvent, cx| {
            this.selected = 0;
            cx.notify();
        });
        window.focus(&query.focus_handle(cx));
        Self { current: repo.current_branch(), local, remote, query, selected: 0, focus_handle: cx.focus_handle(), _subscription: subscription }
    }

    fn rows(&self, cx: &App) -> Vec<BranchRow> {
        let query = self.query.read(cx).text().trim().to_string();
        let matches = |b: &String| query.is_empty() || b.to_ascii_lowercase().contains(&query.to_ascii_lowercase());
        let mut rows = Vec::new();
        if !query.is_empty() && !self.local.iter().any(|b| *b == query) && !query.contains(' ') {
            rows.push(BranchRow::Create(query.clone()));
        }
        rows.extend(self.local.iter().filter(|b| matches(b)).cloned().map(BranchRow::Local));
        rows.extend(self.remote.iter().filter(|b| matches(b)).cloned().map(BranchRow::Remote));
        rows
    }

    fn selected_row(&self, cx: &App) -> Option<BranchRow> {
        self.rows(cx).into_iter().nth(self.selected)
    }

    fn run(&mut self, command: GitCommand, cx: &mut Context<Self>) {
        cx.emit(DialogEvent::Git(command));
    }

    fn checkout_selected(&mut self, cx: &mut Context<Self>) {
        match self.selected_row(cx) {
            Some(BranchRow::Create(name)) => self.run(GitCommand::CreateBranch { name, from: None }, cx),
            Some(BranchRow::Local(name) | BranchRow::Remote(name)) => self.run(GitCommand::Checkout(name), cx),
            None => {}
        }
    }

    fn select_prev(&mut self, _: &SelectPrev, _: &mut Window, cx: &mut Context<Self>) {
        self.selected = self.selected.saturating_sub(1);
        cx.notify();
    }

    fn select_next(&mut self, _: &SelectNext, _: &mut Window, cx: &mut Context<Self>) {
        let count = self.rows(cx).len();
        self.selected = (self.selected + 1).min(count.saturating_sub(1));
        cx.notify();
    }

    fn confirm(&mut self, _: &Confirm, _: &mut Window, cx: &mut Context<Self>) {
        self.checkout_selected(cx);
    }

    fn cancel(&mut self, _: &Cancel, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(DialogEvent::Dismiss);
    }
}

impl Focusable for BranchDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for BranchDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = self.rows(cx);
        let selected = self.selected.min(rows.len().saturating_sub(1));
        let selected_row = rows.get(selected).cloned();
        let mut elements: Vec<gpui::AnyElement> = Vec::new();
        let mut last_section = "";
        for (ix, row) in rows.iter().enumerate() {
            let (section, label, detail) = match row {
                BranchRow::Create(name) => ("", format!("+ New branch '{name}'"), self.current.clone().map(|c| format!("from {c}")).unwrap_or_default()),
                BranchRow::Local(name) => ("LOCAL", name.clone(), if Some(name) == self.current.as_ref() { "current".to_string() } else { String::new() }),
                BranchRow::Remote(name) => ("REMOTE", name.clone(), String::new()),
            };
            if section != last_section && !section.is_empty() {
                elements.push(div().px_3().pt_2().pb_1().text_size(px(11.)).text_color(theme::muted()).child(section).into_any_element());
            }
            last_section = section;
            let is_current = matches!(row, BranchRow::Local(n) if Some(n) == self.current.as_ref());
            elements.push(
                div()
                    .id(("branch", ix))
                    .flex()
                    .gap_2()
                    .px_3()
                    .h(px(26.))
                    .items_center()
                    .cursor_pointer()
                    .when(ix == selected, |d| d.bg(theme::selection()))
                    .hover(|d| d.bg(theme::hover()))
                    .child(div().w(px(14.)).text_color(theme::accent()).child(if is_current { "✓" } else { "⎇" }))
                    .child(div().flex_1().child(label))
                    .child(div().text_size(px(12.)).text_color(theme::muted()).child(detail))
                    .on_click(cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                        this.selected = ix;
                        if event.click_count() >= 2 {
                            this.checkout_selected(cx);
                        }
                        cx.notify();
                    }))
                    .into_any_element(),
            );
        }
        let query = self.query.read(cx).text().trim().to_string();
        let target = match &selected_row {
            Some(BranchRow::Local(name) | BranchRow::Remote(name)) => Some(name.clone()),
            _ => None,
        };
        let is_other_local = matches!(&selected_row, Some(BranchRow::Local(n)) if Some(n) != self.current.as_ref());
        let action = |id: &'static str, label: &'static str| button(id, label, false);
        card(560.)
            .key_context("ListDialog")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::cancel))
            .on_action(cx.listener(Self::select_prev))
            .on_action(cx.listener(Self::select_next))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(theme::border())
                    .child(div().flex_1().child(format!("⎇ {}", self.current.clone().unwrap_or_else(|| "detached".into()))))
                    .child(action("git-fetch", "Fetch").on_click(cx.listener(|this, _, _, cx| this.run(GitCommand::Fetch, cx))))
                    .child(action("git-pull", "Pull").on_click(cx.listener(|this, _, _, cx| this.run(GitCommand::Pull, cx))))
                    .child(action("git-push", "Push").on_click(cx.listener(|this, _, _, cx| this.run(GitCommand::Push, cx)))),
            )
            .child(div().p_2().child(self.query.clone()))
            .child(div().id("branch-list").max_h(px(380.)).overflow_y_scroll().pb_1().text_size(px(13.)).children(elements))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .p_2()
                    .border_t_1()
                    .border_color(theme::border())
                    .text_size(px(12.))
                    .child(action("branch-checkout", "Checkout").on_click(cx.listener(|this, _, _, cx| this.checkout_selected(cx))))
                    .when_some(target.clone(), |d, target| {
                        let from = target.clone();
                        d.child(action("branch-new-from", "New Branch from…").on_click(cx.listener(move |this, _, _, cx| {
                            let name = this.query.read(cx).text().trim().to_string();
                            if name.is_empty() || name == from {
                                this.query.update(cx, |q, cx| q.set_placeholder(format!("Type the new branch name, then New Branch from {from}"), cx));
                            } else {
                                this.run(GitCommand::CreateBranch { name, from: Some(from.clone()) }, cx);
                            }
                        })))
                        .child(action("branch-merge", "Merge into Current").on_click(cx.listener(move |this, _, _, cx| {
                            this.run(GitCommand::Merge(target.clone()), cx)
                        })))
                    })
                    .when(is_other_local, |d| {
                        let name = target.clone().unwrap_or_default();
                        let rename_from = name.clone();
                        let new_name = query.clone();
                        d.child(action("branch-rename", "Rename to Typed Name").on_click(cx.listener(move |this, _, _, cx| {
                            if !new_name.is_empty() && new_name != rename_from {
                                this.run(GitCommand::Rename { old: rename_from.clone(), new: new_name.clone() }, cx);
                            }
                        })))
                        .child(action("branch-delete", "Delete").on_click(cx.listener(move |this, _, _, cx| this.run(GitCommand::Delete(name.clone()), cx))))
                    }),
            )
    }
}

// --- Confirm ----------------------------------------------------------------------------

pub struct ConfirmDialog {
    message: String,
    command: GitCommand,
    focus_handle: FocusHandle,
}

impl EventEmitter<DialogEvent> for ConfirmDialog {}

impl ConfirmDialog {
    pub fn new(message: String, command: GitCommand, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        window.focus(&focus_handle);
        Self { message, command, focus_handle }
    }

    fn confirm(&mut self, _: &Confirm, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(DialogEvent::Git(self.command.clone()));
    }

    fn cancel(&mut self, _: &Cancel, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(DialogEvent::Dismiss);
    }
}

impl Focusable for ConfirmDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ConfirmDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        card(460.)
            .key_context("Dialog")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::cancel))
            .child(div().p_3().child(SharedString::from(self.message.clone())))
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .p_3()
                    .child(button("confirm-cancel", "Cancel", false).on_click(cx.listener(|this, _, window, cx| this.cancel(&Cancel, window, cx))))
                    .child(button("confirm-ok", "Rollback", true).on_click(cx.listener(|this, _, window, cx| this.confirm(&Confirm, window, cx)))),
            )
    }
}
