//! Git dialogs: side-by-side diff, branches, confirmation.

use gpui::{App, Context, Entity, EventEmitter, FocusHandle, Focusable, SharedString, Subscription, Window, div, prelude::*, px};

use super::{Cancel, Confirm, DialogEvent, SelectNext, SelectPrev, button, card};
use crate::text_field::{FieldEvent, TextField};
use crate::theme;

/// What a git dialog asks the workspace to run.
#[derive(Debug, Clone)]
pub enum GitCommand {
    Checkout(String),
    CheckoutRevision(String),
    CreateBranch { name: String, from: Option<String> },
    Merge(String),
    Rename { old: String, new: String },
    Delete(String),
    Pull,
    Push,
    Fetch,
    Rollback(Vec<git::FileStatus>),
    /// Any other git command: a title for the output and its arguments.
    Raw { title: String, args: Vec<String>, changes_files: bool },
    /// Interactive rebase onto `base` with these steps.
    Rebase { base: String, steps: Vec<git::RebaseStep> },
}

const MAX_ROWS: usize = 5000;

// --- Diff -----------------------------------------------------------------------------

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

// --- Branches (title bar dropdown) -------------------------------------------------------

#[derive(Clone, PartialEq, Eq)]
enum BranchMode {
    Normal,
    NewBranch { from: Option<String> },
    Rename(String),
    Revision,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BranchAction {
    Checkout,
    NewFrom,
    Merge,
    Rename,
    Delete,
}

#[derive(Clone)]
enum BranchRow {
    Pull,
    Fetch,
    Commit,
    Push,
    NewBranch,
    Revision,
    Header(&'static str),
    Folder { key: String, name: String, depth: usize, collapsed: bool },
    Branch { name: String, label: String, depth: usize, upstream: Option<String> },
    Action { branch: String, action: BranchAction, depth: usize },
    /// Typed name/revision in a mode, confirmed with Enter.
    Input(String),
    Tag(String),
}

impl BranchRow {
    fn selectable(&self) -> bool {
        !matches!(self, BranchRow::Header(_))
    }
}

pub struct BranchDialog {
    current: Option<String>,
    local: Vec<String>,
    remote: Vec<String>,
    recent: Vec<String>,
    tags: Vec<String>,
    upstreams: std::collections::HashMap<String, String>,
    query: Entity<TextField>,
    selected: usize,
    mode: BranchMode,
    expanded: Option<String>,
    collapsed: std::collections::HashSet<String>,
    focus_handle: FocusHandle,
    _subscription: Subscription,
}

impl EventEmitter<DialogEvent> for BranchDialog {}

fn tree_rows(branches: &[String], section: &str, query: &str, collapsed: &std::collections::HashSet<String>, upstreams: &std::collections::HashMap<String, String>, out: &mut Vec<BranchRow>) {
    // Group by path segments: feature/archive/x -> feature/ > archive/ > x
    let mut seen_folders = std::collections::HashSet::new();
    let mut sorted: Vec<&String> = branches.iter().filter(|b| query.is_empty() || b.to_ascii_lowercase().contains(query)).collect();
    sorted.sort_by_key(|b| (b.split('/').count() == 1, b.to_ascii_lowercase()));
    sorted.sort_by(|a, b| {
        let fa: Vec<&str> = a.split('/').collect();
        let fb: Vec<&str> = b.split('/').collect();
        fa[..fa.len() - 1].cmp(&fb[..fb.len() - 1]).then(a.cmp(b))
    });
    for branch in sorted {
        let parts: Vec<&str> = branch.split('/').collect();
        let mut hidden = false;
        let mut key = section.to_string();
        for (depth, folder) in parts[..parts.len() - 1].iter().enumerate() {
            key = format!("{key}/{folder}");
            if hidden {
                continue;
            }
            let is_collapsed = collapsed.contains(&key) && query.is_empty();
            if seen_folders.insert(key.clone()) {
                out.push(BranchRow::Folder { key: key.clone(), name: folder.to_string(), depth: depth + 1, collapsed: is_collapsed });
            }
            hidden = is_collapsed;
        }
        if !hidden {
            out.push(BranchRow::Branch {
                name: branch.clone(),
                label: parts[parts.len() - 1].to_string(),
                depth: parts.len(),
                upstream: upstreams.get(branch).cloned(),
            });
        }
    }
}

impl BranchDialog {
    pub fn new(repo: &git::Repo, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (local, remote) = repo.branches().unwrap_or_default();
        let upstreams = local.iter().take(80).filter_map(|b| Some((b.clone(), repo.upstream(b)?))).collect();
        let query = cx.new(|cx| TextField::new("Search for branches and actions", cx));
        let subscription = cx.subscribe(&query, |this: &mut Self, _, _: &FieldEvent, cx| {
            this.selected = this.first_selectable(cx);
            cx.notify();
        });
        window.focus(&query.focus_handle(cx));
        Self {
            current: repo.current_branch(),
            recent: repo.recent_branches(5),
            tags: repo.tags(),
            local,
            remote,
            upstreams,
            query,
            selected: 0,
            mode: BranchMode::Normal,
            expanded: None,
            collapsed: std::collections::HashSet::new(),
            focus_handle: cx.focus_handle(),
            _subscription: subscription,
        }
    }

    fn first_selectable(&self, cx: &App) -> usize {
        self.rows(cx).iter().position(BranchRow::selectable).unwrap_or(0)
    }

    fn rows(&self, cx: &App) -> Vec<BranchRow> {
        let query = self.query.read(cx).text().trim().to_string();
        let lower = query.to_ascii_lowercase();
        let mut rows = Vec::new();
        match &self.mode {
            BranchMode::NewBranch { from } => {
                rows.push(BranchRow::Header(if from.is_some() { "New branch from selected" } else { "New branch from current" }));
                rows.push(BranchRow::Input(query));
                return rows;
            }
            BranchMode::Rename(old) => {
                rows.push(BranchRow::Header("Rename branch"));
                rows.push(BranchRow::Input(if query.is_empty() { old.clone() } else { query }));
                return rows;
            }
            BranchMode::Revision => {
                rows.push(BranchRow::Header("Checkout tag or revision (detached)"));
                if !query.is_empty() {
                    rows.push(BranchRow::Input(query.clone()));
                }
                rows.extend(self.tags.iter().filter(|t| lower.is_empty() || t.to_ascii_lowercase().contains(&lower)).take(50).cloned().map(BranchRow::Tag));
                return rows;
            }
            BranchMode::Normal => {}
        }
        let action_matches = |label: &str| lower.is_empty() || label.to_ascii_lowercase().contains(&lower);
        for (row, label) in [
            (BranchRow::Pull, "Update Project"),
            (BranchRow::Fetch, "Fetch"),
            (BranchRow::Commit, "Commit"),
            (BranchRow::Push, "Push"),
            (BranchRow::NewBranch, "New Branch"),
            (BranchRow::Revision, "Checkout Tag or Revision"),
        ] {
            if action_matches(label) {
                rows.push(row);
            }
        }
        let mut branch_rows = Vec::new();
        let recent: Vec<String> = self.recent.iter().filter(|b| lower.is_empty() || b.to_ascii_lowercase().contains(&lower)).cloned().collect();
        if !recent.is_empty() && lower.is_empty() {
            branch_rows.push(BranchRow::Header("Recent"));
            for b in &recent {
                branch_rows.push(BranchRow::Branch { name: b.clone(), label: b.clone(), depth: 1, upstream: self.upstreams.get(b).cloned() });
            }
        }
        let mut local = Vec::new();
        tree_rows(&self.local, "local", &lower, &self.collapsed, &self.upstreams, &mut local);
        if !local.is_empty() {
            branch_rows.push(BranchRow::Header("Local"));
            branch_rows.extend(local);
        }
        let mut remote = Vec::new();
        tree_rows(&self.remote, "remote", &lower, &self.collapsed, &self.upstreams, &mut remote);
        if !remote.is_empty() {
            branch_rows.push(BranchRow::Header("Remote"));
            branch_rows.extend(remote);
        }
        // Inline actions under the expanded branch.
        for row in branch_rows {
            let expanded = matches!(&row, BranchRow::Branch { name, .. } if Some(name) == self.expanded.as_ref());
            let depth = if let BranchRow::Branch { depth, .. } = &row { *depth } else { 0 };
            rows.push(row);
            if expanded {
                let branch = self.expanded.clone().unwrap_or_default();
                let is_current = Some(&branch) == self.current.as_ref();
                let is_local = self.local.contains(&branch);
                let mut actions = Vec::new();
                if !is_current {
                    actions.push(BranchAction::Checkout);
                }
                actions.push(BranchAction::NewFrom);
                if !is_current {
                    actions.push(BranchAction::Merge);
                }
                if is_local {
                    actions.push(BranchAction::Rename);
                }
                if is_local && !is_current {
                    actions.push(BranchAction::Delete);
                }
                rows.extend(actions.into_iter().map(|action| BranchRow::Action { branch: branch.clone(), action, depth: depth + 1 }));
            }
        }
        rows
    }

    fn run(&mut self, command: GitCommand, cx: &mut Context<Self>) {
        cx.emit(DialogEvent::Git(command));
    }

    fn enter_mode(&mut self, mode: BranchMode, window: &mut Window, cx: &mut Context<Self>) {
        let placeholder = match &mode {
            BranchMode::NewBranch { .. } => "New branch name",
            BranchMode::Rename(_) => "New name",
            BranchMode::Revision => "Tag, branch or commit hash",
            BranchMode::Normal => "Search for branches and actions",
        };
        self.mode = mode;
        self.query.update(cx, |q, cx| {
            q.set_text("", cx);
            q.set_placeholder(placeholder, cx);
        });
        window.focus(&self.query.focus_handle(cx));
        self.selected = self.first_selectable(cx);
        cx.notify();
    }

    fn activate(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self.rows(cx).into_iter().nth(ix) else { return };
        let current = self.current.clone();
        match row {
            BranchRow::Pull => self.run(GitCommand::Pull, cx),
            BranchRow::Fetch => self.run(GitCommand::Fetch, cx),
            BranchRow::Push => self.run(GitCommand::Push, cx),
            BranchRow::Commit => cx.emit(DialogEvent::RunAction(Box::new(crate::workspace::ShowCommit))),
            BranchRow::NewBranch => self.enter_mode(BranchMode::NewBranch { from: None }, window, cx),
            BranchRow::Revision => self.enter_mode(BranchMode::Revision, window, cx),
            BranchRow::Header(_) => {}
            BranchRow::Folder { key, .. } => {
                if !self.collapsed.remove(&key) {
                    self.collapsed.insert(key);
                }
                cx.notify();
            }
            BranchRow::Branch { name, .. } => {
                self.expanded = if self.expanded.as_ref() == Some(&name) { None } else { Some(name) };
                cx.notify();
            }
            BranchRow::Action { branch, action, .. } => match action {
                BranchAction::Checkout => self.run(GitCommand::Checkout(branch), cx),
                BranchAction::NewFrom => self.enter_mode(BranchMode::NewBranch { from: Some(branch) }, window, cx),
                BranchAction::Merge => self.run(GitCommand::Merge(branch), cx),
                BranchAction::Rename => self.enter_mode(BranchMode::Rename(branch), window, cx),
                BranchAction::Delete => self.run(GitCommand::Delete(branch), cx),
            },
            BranchRow::Input(text) => {
                if text.is_empty() {
                    return;
                }
                match self.mode.clone() {
                    BranchMode::NewBranch { from } => self.run(GitCommand::CreateBranch { name: text, from }, cx),
                    BranchMode::Rename(old) if text != old => self.run(GitCommand::Rename { old, new: text }, cx),
                    BranchMode::Revision => self.run(GitCommand::CheckoutRevision(text), cx),
                    _ => {}
                }
                let _ = current;
            }
            BranchRow::Tag(tag) => self.run(GitCommand::CheckoutRevision(tag), cx),
        }
    }

    fn step(&mut self, delta: isize, cx: &mut Context<Self>) {
        let rows = self.rows(cx);
        let mut ix = self.selected as isize;
        loop {
            ix += delta;
            if ix < 0 || ix as usize >= rows.len() {
                return;
            }
            if rows[ix as usize].selectable() {
                self.selected = ix as usize;
                cx.notify();
                return;
            }
        }
    }

    fn select_prev(&mut self, _: &SelectPrev, _: &mut Window, cx: &mut Context<Self>) {
        self.step(-1, cx);
    }

    fn select_next(&mut self, _: &SelectNext, _: &mut Window, cx: &mut Context<Self>) {
        self.step(1, cx);
    }

    fn confirm(&mut self, _: &Confirm, window: &mut Window, cx: &mut Context<Self>) {
        self.activate(self.selected, window, cx);
    }

    fn cancel(&mut self, _: &Cancel, window: &mut Window, cx: &mut Context<Self>) {
        if self.mode != BranchMode::Normal {
            self.enter_mode(BranchMode::Normal, window, cx);
        } else {
            cx.emit(DialogEvent::Dismiss);
        }
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
        let current = self.current.clone();
        let elements: Vec<gpui::AnyElement> = rows
            .into_iter()
            .enumerate()
            .map(|(ix, row)| {
                let item = |icon: &'static str, label: String, hint: &'static str| {
                    div()
                        .id(("branch-row", ix))
                        .mx_1()
                        .px_2()
                        .h(px(26.))
                        .flex()
                        .items_center()
                        .gap_2()
                        .rounded_md()
                        .cursor_pointer()
                        .when(ix == selected, |d| d.bg(theme::selection()))
                        .hover(|d| d.bg(theme::hover()))
                        .child(div().w(px(16.)).text_color(theme::muted()).child(icon))
                        .child(div().flex_1().overflow_hidden().whitespace_nowrap().child(label))
                        .child(div().text_size(px(12.)).text_color(theme::muted()).child(hint))
                };
                let element = match row {
                    BranchRow::Pull => item("↙", "Update Project…".into(), "⌘T"),
                    BranchRow::Fetch => item("⟳", "Fetch".into(), ""),
                    BranchRow::Commit => item("⊸", "Commit…".into(), "⌘0"),
                    BranchRow::Push => item("↗", "Push…".into(), "⇧⌘K"),
                    BranchRow::NewBranch => item("+", "New Branch…".into(), "⌥⌘N"),
                    BranchRow::Revision => item("", "Checkout Tag or Revision…".into(), ""),
                    BranchRow::Header(title) => {
                        return div().mt_1().px_3().pt_1p5().pb_1().border_t_1().border_color(theme::border()).text_size(px(12.)).text_color(theme::muted()).child(title).into_any_element();
                    }
                    BranchRow::Folder { name, depth, collapsed, .. } => item(if collapsed { "▸" } else { "▾" }, format!("{}📁 {name}", "  ".repeat(depth.saturating_sub(1))), ""),
                    BranchRow::Branch { name, label, depth, upstream } => {
                        let is_current = current.as_deref() == Some(name.as_str());
                        let mut el = item(if is_current { "🏷" } else { "⎇" }, format!("{}{label}", "  ".repeat(depth.saturating_sub(1))), "");
                        if let Some(upstream) = upstream {
                            el = el.child(div().text_size(px(12.)).text_color(theme::muted()).child(upstream));
                        }
                        el.child(div().text_color(theme::muted()).child("›"))
                    }
                    BranchRow::Action { branch, action, depth } => {
                        let cur = current.clone().unwrap_or_default();
                        let label = match action {
                            BranchAction::Checkout => format!("Checkout '{branch}'"),
                            BranchAction::NewFrom => format!("New Branch from '{branch}'…"),
                            BranchAction::Merge => format!("Merge '{branch}' into '{cur}'"),
                            BranchAction::Rename => "Rename…".to_string(),
                            BranchAction::Delete => "Delete".to_string(),
                        };
                        item("", format!("{}{label}", "  ".repeat(depth)), "")
                    }
                    BranchRow::Input(text) => item("↵", if text.is_empty() { "Type a name…".into() } else { text }, "Enter"),
                    BranchRow::Tag(tag) => item("🏷", tag, ""),
                };
                element.on_click(cx.listener(move |this, _, window, cx| {
                    this.selected = ix;
                    this.activate(ix, window, cx);
                }))
                .into_any_element()
            })
            .collect();
        card(420.)
            .key_context("ListDialog")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::cancel))
            .on_action(cx.listener(Self::select_prev))
            .on_action(cx.listener(Self::select_next))
            .child(div().p_2().child(self.query.clone()))
            .child(div().id("branch-list").max_h(px(520.)).overflow_y_scroll().pb_1().text_size(px(13.)).children(elements))
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

// --- Local History ----------------------------------------------------------------------

pub struct LocalHistoryDialog {
    path: std::path::PathBuf,
    current: String,
    versions: Vec<history::Version>,
    selected: usize,
    rows: Vec<git::DiffRow>,
    focus_handle: FocusHandle,
}

impl EventEmitter<DialogEvent> for LocalHistoryDialog {}

impl LocalHistoryDialog {
    pub fn new(path: std::path::PathBuf, current: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        window.focus(&focus_handle);
        let versions = crate::local_history::versions(&path);
        let mut dialog = Self { path, current, versions, selected: 0, rows: Vec::new(), focus_handle };
        dialog.select(0);
        dialog
    }

    fn select(&mut self, ix: usize) {
        self.selected = ix.min(self.versions.len().saturating_sub(1));
        let old = self.versions.get(self.selected).and_then(crate::local_history::content).unwrap_or_default();
        self.rows = git::side_by_side(&old, &self.current);
    }

    fn select_prev(&mut self, _: &SelectPrev, _: &mut Window, cx: &mut Context<Self>) {
        self.select(self.selected.saturating_sub(1));
        cx.notify();
    }

    fn select_next(&mut self, _: &SelectNext, _: &mut Window, cx: &mut Context<Self>) {
        self.select(self.selected + 1);
        cx.notify();
    }

    fn revert(&mut self, cx: &mut Context<Self>) {
        if let Some(content) = self.versions.get(self.selected).and_then(crate::local_history::content) {
            cx.emit(DialogEvent::RevertFile { path: self.path.clone(), content });
        }
    }

    fn confirm(&mut self, _: &Confirm, _: &mut Window, cx: &mut Context<Self>) {
        self.revert(cx);
    }

    fn cancel(&mut self, _: &Cancel, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(DialogEvent::Dismiss);
    }
}

impl Focusable for LocalHistoryDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for LocalHistoryDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let list: Vec<_> = self
            .versions
            .iter()
            .enumerate()
            .map(|(ix, version)| {
                div()
                    .id(("history-version", ix))
                    .flex()
                    .flex_col()
                    .px_3()
                    .py_1()
                    .cursor_pointer()
                    .when(ix == self.selected, |d| d.bg(theme::selection()))
                    .hover(|d| d.bg(theme::hover()))
                    .child(div().text_color(theme::ui_text()).child(history::ago(version.time)))
                    .child(div().text_size(px(11.)).text_color(theme::muted()).child(format!("{} · {} bytes", version.label, version.size)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.select(ix);
                        cx.notify();
                    }))
            })
            .collect();
        let added = theme::success().opacity(0.18);
        let removed = theme::error().opacity(0.18);
        let changed = theme::accent().opacity(0.16);
        let rows = self.rows.iter().take(MAX_ROWS).enumerate().map(|(ix, row)| {
            let (left_bg, right_bg) = match row.tag {
                git::RowTag::Equal => (None, None),
                git::RowTag::Removed => (Some(removed), None),
                git::RowTag::Added => (None, Some(added)),
                git::RowTag::Changed => (Some(changed), Some(changed)),
            };
            div()
                .id(("history-row", ix))
                .flex()
                .h(px(19.))
                .child(side(&row.left, left_bg))
                .child(div().w(px(1.)).h_full().bg(theme::border()))
                .child(side(&row.right, right_bg))
        });
        let name = self.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        card(1180.)
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
                    .child(format!("Local History · {name}"))
                    .child(div().flex_1())
                    .when(!self.versions.is_empty(), |d| {
                        d.child(button("history-revert", "Revert", true).on_click(cx.listener(|this, _, _, cx| this.revert(cx))))
                    })
                    .child(button("history-close", "Close", false).on_click(cx.listener(|_, _, _, cx| cx.emit(DialogEvent::Dismiss)))),
            )
            .child(
                div()
                    .flex()
                    .h(px(620.))
                    .child(
                        div()
                            .id("history-versions")
                            .w(px(220.))
                            .flex_none()
                            .overflow_y_scroll()
                            .border_r_1()
                            .border_color(theme::border())
                            .text_size(px(13.))
                            .when(list.is_empty(), |d| d.child(div().p_3().text_color(theme::muted()).child("No versions yet. They are recorded on open and save.")))
                            .children(list),
                    )
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .min_w(px(0.))
                            .child(
                                div()
                                    .flex()
                                    .text_size(px(11.))
                                    .text_color(theme::muted())
                                    .border_b_1()
                                    .border_color(theme::border())
                                    .child(div().flex_1().px_3().py_1().child("Selected version"))
                                    .child(div().flex_1().px_3().py_1().child("Current")),
                            )
                            .child(div().id("history-diff").flex_1().overflow_y_scroll().font_family(theme::editor_font()).text_size(px(12.)).children(rows)),
                    ),
            )
    }
}
