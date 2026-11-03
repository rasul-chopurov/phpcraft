//! Git in the workspace: the Commit tool window (left panel), the Git Log panel, the branch
//! widget in the status bar, and running git commands in the background.

use std::collections::HashSet;
use std::time::Duration;

use gpui::{AppContext, Context, Entity, Focusable, MouseButton, SharedString, Window, actions, div, prelude::*, px};

use super::Workspace;
use super::tools::Output;
use crate::dialogs::{BranchDialog, ConfirmDialog, GitCommand};
use crate::text_field::TextField;
use crate::theme;

actions!(workspace, [ShowProject, ShowCommit, ToggleGitLog, OpenBranches, CommitChanges, CommitAndPush, GitPull, GitPush]);

pub(super) struct GitState {
    pub status: Vec<git::FileStatus>,
    pub branch: Option<String>,
    pub ahead_behind: Option<(usize, usize)>,
    /// Files excluded from the next commit (PhpStorm-style checkboxes; included by default).
    pub excluded: HashSet<String>,
    pub message: Entity<crate::editor_view::EditorView>,
    /// Collapsed groups in the Commit window ("changes", "unversioned").
    pub groups_collapsed: HashSet<&'static str>,
    pub amend: bool,
    pub log: Vec<git::Commit>,
    pub log_graph: Vec<git::GraphRow>,
    pub log_selected: Option<usize>,
    pub log_files: Vec<(git::Change, String)>,
    pub log_details: Option<git::CommitDetails>,
    /// Git tool window: filters, branch tree and console.
    pub filter_text: Entity<TextField>,
    pub filter_user: Entity<TextField>,
    pub filter_path: Entity<TextField>,
    pub filter_branch: String,
    pub tree_query: Entity<TextField>,
    pub tree_collapsed: HashSet<String>,
    pub branches: (Vec<String>, Vec<String>),
    pub tab: GitTab,
    pub console: Vec<String>,
    pub _subscriptions: Vec<gpui::Subscription>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum GitTab {
    Log,
    Console,
}

impl GitState {
    pub fn new(intel: std::sync::Arc<crate::intel::Intel>, cx: &mut Context<Workspace>) -> Self {
        let workspace = cx.weak_entity();
        Self {
            groups_collapsed: HashSet::new(),
            status: Vec::new(),
            branch: None,
            ahead_behind: None,
            excluded: HashSet::new(),
            message: cx.new(|cx| crate::editor_view::EditorView::plain(intel, workspace, cx)),
            amend: false,
            log: Vec::new(),
            log_graph: Vec::new(),
            log_selected: None,
            log_files: Vec::new(),
            log_details: None,
            filter_text: cx.new(|cx| TextField::new("Text or hash", cx)),
            filter_user: cx.new(|cx| TextField::new("User", cx)),
            filter_path: cx.new(|cx| TextField::new("Path", cx)),
            filter_branch: String::new(),
            tree_query: cx.new(|cx| TextField::new("Branch or tag", cx)),
            tree_collapsed: HashSet::new(),
            branches: (Vec::new(), Vec::new()),
            tab: GitTab::Log,
            console: Vec::new(),
            _subscriptions: Vec::new(),
        }
    }

    /// Re-runs the log when a filter field changes.
    pub fn watch_filters(&mut self, cx: &mut Context<Workspace>) {
        for field in [self.filter_text.clone(), self.filter_user.clone(), self.filter_path.clone()] {
            self._subscriptions.push(cx.subscribe(&field, |this: &mut Workspace, _, _: &crate::text_field::FieldEvent, cx| this.refresh_log(cx)));
        }
        let tree = self.tree_query.clone();
        self._subscriptions.push(cx.subscribe(&tree, |_: &mut Workspace, _, _: &crate::text_field::FieldEvent, cx| cx.notify()));
    }
}

fn change_color(change: git::Change) -> gpui::Hsla {
    match change {
        git::Change::Added => theme::success(),
        git::Change::Untracked => theme::syntax(php_syntax::Highlight::Warning),
        git::Change::Deleted => theme::muted(),
        git::Change::Conflicted => theme::error(),
        git::Change::Modified | git::Change::Renamed => theme::accent(),
    }
}

impl Workspace {
    /// Polls `git status` every few seconds while the window is open.
    pub(crate) fn start_git_polling(&mut self, cx: &mut Context<Self>) {
        if self.intel.repo().is_none() {
            return;
        }
        self.refresh_git(cx);
        self._git_poll = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(3)).await;
                // Every refresh starts three git processes: not while PhpCraft is in the
                // background (on Windows each one can flash a console).
                let poll = |this: &mut Self, cx: &mut Context<Self>| {
                    if this.window_active {
                        this.refresh_git(cx);
                    }
                };
                if this.update(cx, poll).is_err() {
                    break;
                }
            }
        }));
    }

    pub(super) fn refresh_git(&mut self, cx: &mut Context<Self>) {
        let Some(repo) = self.intel.repo().cloned() else { return };
        cx.spawn(async move |this, cx| {
            let (status, branch, ahead) = cx
                .background_executor()
                .spawn(async move { (repo.status().unwrap_or_default(), repo.current_branch(), repo.ahead_behind()) })
                .await;
            this.update(cx, |this, cx| {
                let changed = this.git.status != status || this.git.branch != branch || this.git.ahead_behind != ahead;
                // Like PhpStorm, new unversioned files start out of the commit.
                for file in status.iter().filter(|f| f.change == git::Change::Untracked) {
                    if !this.git.status.iter().any(|old| old.path == file.path) {
                        this.git.excluded.insert(file.path.clone());
                    }
                }
                this.git.status = status;
                this.git.branch = branch;
                this.git.ahead_behind = ahead;
                if changed {
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn refresh_log(&mut self, cx: &mut Context<Self>) {
        let Some(repo) = self.intel.repo().cloned() else { return };
        let filter = git::LogFilter {
            text: self.git.filter_text.read(cx).text().to_string(),
            author: self.git.filter_user.read(cx).text().to_string(),
            branch: self.git.filter_branch.clone(),
            path: self.git.filter_path.read(cx).text().to_string(),
        };
        cx.spawn(async move |this, cx| {
            let (log, branches) = cx
                .background_executor()
                .spawn(async move { (repo.log_filtered(&filter, 1000).unwrap_or_default(), repo.branches().unwrap_or_default()) })
                .await;
            this.update(cx, |this, cx| {
                this.git.log_graph = git::graph(&log);
                this.git.log = log;
                this.git.branches = branches;
                this.git.log_selected = None;
                this.git.log_details = None;
                this.git.log_files.clear();
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn show_project(&mut self, _: &ShowProject, _: &mut Window, cx: &mut Context<Self>) {
        self.layout.show(super::layout::ToolWindow::Project);
        self.layout.save();
        cx.notify();
    }

    pub(super) fn show_commit(&mut self, _: &ShowCommit, window: &mut Window, cx: &mut Context<Self>) {
        self.layout.show(super::layout::ToolWindow::Commit);
        self.layout.save();
        self.refresh_git(cx);
        window.focus(&self.git.message.focus_handle(cx));
        cx.notify();
    }

    pub(super) fn toggle_git_log(&mut self, _: &ToggleGitLog, _: &mut Window, cx: &mut Context<Self>) {
        let open = !self.layout.is_active(super::layout::ToolWindow::Git);
        if open {
            self.layout.show(super::layout::ToolWindow::Git);
        } else {
            self.layout.hide(super::layout::ToolWindow::Git);
        }
        if open {
            self.refresh_log(cx);
        }
        cx.notify();
    }

    pub(super) fn open_branches(&mut self, _: &OpenBranches, window: &mut Window, cx: &mut Context<Self>) {
        self.open_branches_dropdown(window, cx);
    }

    /// The branch popup under the title bar's `⎇ branch` button.
    pub(super) fn open_branches_dropdown(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(repo) = self.intel.repo().cloned() else { return };
        let dialog = cx.new(|cx| BranchDialog::new(&repo, window, cx));
        let left = 30. + self.project.name().len() as f32 * 7.5 + 20.;
        self.show_dropdown(dialog, left, window, cx);
    }

    pub(super) fn git_pull(&mut self, _: &GitPull, _: &mut Window, cx: &mut Context<Self>) {
        self.run_git(GitCommand::Pull, cx);
    }

    pub(super) fn git_push(&mut self, _: &GitPush, _: &mut Window, cx: &mut Context<Self>) {
        self.run_git(GitCommand::Push, cx);
    }

    fn included_paths(&self) -> Vec<String> {
        self.git.status.iter().filter(|f| !self.git.excluded.contains(&f.path)).map(|f| f.path.clone()).collect()
    }

    pub(super) fn commit_changes(&mut self, _: &CommitChanges, _: &mut Window, cx: &mut Context<Self>) {
        self.commit(false, cx);
    }

    pub(super) fn commit_and_push(&mut self, _: &CommitAndPush, _: &mut Window, cx: &mut Context<Self>) {
        self.commit(true, cx);
    }

    fn commit(&mut self, push: bool, cx: &mut Context<Self>) {
        let Some(repo) = self.intel.repo().cloned() else { return };
        self.save_all(cx);
        let message = self.git.message.read(cx).buffer.text().trim().to_string();
        let paths = self.included_paths();
        let amend = self.git.amend;
        let task = self.begin_task(if push { "Commit and push…" } else { "Committing…" }, cx);
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let summary = repo.commit(&message, &paths, amend)?;
                    if push {
                        repo.push().map(|out| format!("Committed {summary}\n\n{out}"))
                    } else {
                        Ok(format!("Committed {summary}"))
                    }
                })
                .await;
            this.update(cx, |this, cx| {
                this.end_task(task, cx);
                let ok = result.is_ok();
                if ok {
                    this.git.message.update(cx, |m, cx| m.replace_all_text("", cx));
                    this.git.amend = false;
                    this.git.excluded.clear();
                }
                this.after_git(cx);
                this.output = Some(Output { title: "Commit".into(), text: result.unwrap_or_else(|e| e), running: false, ok });
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Runs a git command from a dialog or menu in the background and reports its output.
    pub(super) fn run_git(&mut self, command: GitCommand, cx: &mut Context<Self>) {
        let Some(repo) = self.intel.repo().cloned() else { return };
        let title = match &command {
            GitCommand::Checkout(b) => format!("Checkout {b}"),
            GitCommand::CheckoutRevision(r) => format!("Checkout {r}"),
            GitCommand::CreateBranch { name, .. } => format!("New branch {name}"),
            GitCommand::Merge(b) => format!("Merge {b}"),
            GitCommand::Rename { old, new } => format!("Rename {old} → {new}"),
            GitCommand::Delete(b) => format!("Delete branch {b}"),
            GitCommand::Pull => "Pull".into(),
            GitCommand::Push => "Push".into(),
            GitCommand::Fetch => "Fetch".into(),
            GitCommand::Rollback(files) => format!("Rollback {} file(s)", files.len()),
            GitCommand::Raw { title, .. } => title.clone(),
            GitCommand::Rebase { .. } => "Interactive rebase".into(),
            GitCommand::CheckoutPullRequest { number, .. } => format!("Checkout #{number}"),
        };
        let raw_changes = matches!(&command, GitCommand::Raw { changes_files: true, .. } | GitCommand::Rebase { .. } | GitCommand::CheckoutPullRequest { .. });
        if raw_changes {
            self.save_all(cx);
        }
        // Switching branches rewrites files: save edits first.
        if matches!(command, GitCommand::Checkout(_) | GitCommand::CheckoutRevision(_) | GitCommand::CreateBranch { .. } | GitCommand::Merge(_) | GitCommand::Pull) {
            self.save_all(cx);
        }
        let rescan = raw_changes || matches!(command, GitCommand::Checkout(_) | GitCommand::CheckoutRevision(_) | GitCommand::Merge(_) | GitCommand::Pull);
        // Snapshot what is about to be overwritten into Local History.
        let root = repo.root().to_path_buf();
        let at_risk: Vec<std::path::PathBuf> = match &command {
            GitCommand::Rollback(files) => files.iter().map(|f| root.join(&f.path)).collect(),
            GitCommand::Checkout(_) | GitCommand::CheckoutRevision(_) | GitCommand::Merge(_) | GitCommand::Pull | GitCommand::CreateBranch { .. } | GitCommand::Raw { changes_files: true, .. } | GitCommand::Rebase { .. } | GitCommand::CheckoutPullRequest { .. } => self
                .git
                .status
                .iter()
                .map(|f| root.join(&f.path))
                .chain(self.tabs.iter().filter_map(|t| t.read(cx).buffer.path().map(|p| p.to_path_buf())))
                .collect(),
            _ => Vec::new(),
        };
        let label = if matches!(command, GitCommand::Rollback(_)) { "Before Rollback" } else { "Before git" };
        crate::local_history::record_disk(&at_risk, label);
        let task = self.begin_task(format!("git: {title}…"), cx);
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    match command {
                        GitCommand::Checkout(branch) => repo.checkout(&branch),
                        GitCommand::CheckoutRevision(rev) => repo.checkout_revision(&rev),
                        GitCommand::CreateBranch { name, from: Some(from) } => repo.create_branch_from(&name, &from),
                        GitCommand::CreateBranch { name, from: None } => repo.create_branch(&name),
                        GitCommand::Merge(branch) => repo.merge(&branch),
                        GitCommand::Rename { old, new } => repo.rename_branch(&old, &new),
                        GitCommand::Delete(branch) => repo.delete_branch(&branch, false),
                        GitCommand::Pull => repo.pull(),
                        GitCommand::Push => repo.push(),
                        GitCommand::Fetch => repo.fetch(),
                        GitCommand::Rollback(files) => repo.discard(&files).map(|_| "Changes rolled back.".to_string()),
                        GitCommand::Raw { args, .. } => repo.run(&args),
                        GitCommand::Rebase { base, steps } => repo.rebase_interactive(&base, &steps).map_err(|e| {
                            format!("{e}\n\nThe rebase stopped: resolve the conflicts (Git ▸ Resolve Conflicts…), then Git ▸ Rebase ▸ Continue — or Abort.")
                        }),
                        GitCommand::CheckoutPullRequest { host, number } => crate::pull_requests::checkout(repo.root(), host, number),
                    }
                })
                .await;
            this.update(cx, |this, cx| {
                this.end_task(task, cx);
                let ok = result.is_ok();
                let text = match result {
                    Ok(out) if out.trim().is_empty() => "Done.".to_string(),
                    Ok(out) | Err(out) => out,
                };
                this.git.console.push(format!("$ git {title}\n{}", text.trim_end()));
                this.output = Some(Output { title: format!("git {title}"), text, running: false, ok });
                this.after_git(cx);
                if rescan && ok {
                    this.rescan(cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// After commits/checkouts: reload clean editors, refresh markers, status and log.
    fn after_git(&mut self, cx: &mut Context<Self>) {
        let paths: Vec<_> = self.tabs.iter().filter_map(|t| t.read(cx).buffer.path().map(|p| p.to_path_buf())).collect();
        self.reload_tabs(&paths, cx);
        for tab in self.tabs.clone() {
            tab.update(cx, |editor, cx| editor.refresh_git_base(cx));
        }
        self.rows = self.project.rows();
        self.refresh_git(cx);
        if self.layout.is_active(super::layout::ToolWindow::Git) {
            self.refresh_log(cx);
        }
    }

    /// Re-indexes the project after files changed on disk (checkout, merge, pull).
    pub(super) fn rescan(&mut self, cx: &mut Context<Self>) {
        let intel = self.intel.clone();
        let task = self.begin_task("Indexing…", cx);
        cx.spawn(async move |this, cx| {
            cx.background_executor().spawn(async move { intel.scan() }).await;
            this.update(cx, |this, cx| this.end_task(task, cx)).ok();
        })
        .detach();
    }

    /// Diff of working-tree changes as an editor tab; ←/→ walks the other changed files.
    pub(super) fn show_file_diff(&mut self, path: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(repo) = self.intel.repo().cloned() else { return };
        let files: Vec<crate::diff_view::DiffSource> = self
            .git
            .status
            .iter()
            .map(|status| {
                let full = repo.root().join(&status.path);
                let (repo2, rel, disk) = (repo.clone(), status.path.clone(), full.clone());
                crate::diff_view::DiffSource {
                    title: status.path.rsplit('/').next().unwrap_or(&status.path).to_string(),
                    subtitle: status.path.clone(),
                    load: std::sync::Arc::new(move || (repo2.show("HEAD", &rel).unwrap_or_default(), std::fs::read_to_string(&disk).unwrap_or_default())),
                    left_label: "HEAD".into(),
                    right_label: "Your version".into(),
                    open_path: Some(full),
                }
            })
            .collect();
        let index = self.git.status.iter().position(|s| s.path == path).unwrap_or(0);
        self.open_diff_tab("Changes", files, index, window, cx);
    }

    pub(super) fn show_commit_diff_public(&mut self, hash: &str, path: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.show_commit_diff(hash, path, window, cx);
    }

    fn show_commit_diff(&mut self, hash: &str, path: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(repo) = self.intel.repo().cloned() else { return };
        let short = &hash[..hash.len().min(8)];
        let files: Vec<crate::diff_view::DiffSource> = self
            .git
            .log_files
            .iter()
            .map(|(_, file)| crate::diff_view::DiffSource {
                title: file.rsplit('/').next().unwrap_or(file).to_string(),
                subtitle: file.clone(),
                load: {
                    let (repo, hash, file) = (repo.clone(), hash.to_string(), file.clone());
                    std::sync::Arc::new(move || (repo.show(&format!("{hash}^"), &file).unwrap_or_default(), repo.show(&hash, &file).unwrap_or_default()))
                },
                left_label: format!("{short}^"),
                right_label: short.to_string(),
                open_path: Some(repo.root().join(file)),
            })
            .collect();
        let index = self.git.log_files.iter().position(|(_, f)| f == path).unwrap_or(0);
        self.open_diff_tab("Commit", files, index, window, cx);
    }

    pub(super) fn open_diff_tab(&mut self, kind: &str, files: Vec<crate::diff_view::DiffSource>, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if files.is_empty() {
            return;
        }
        let kind = kind.to_string();
        let view = cx.new(|cx| crate::diff_view::DiffView::new(kind, files, index, cx));
        self._diff_subscription = Some(cx.subscribe_in(&view, window, |this, _, event, window, cx| match event {
            crate::diff_view::DiffEvent::Open(path) => {
                let path = path.clone();
                this.open_file(&path, window, cx);
            }
        }));
        window.focus(&view.focus_handle(cx));
        self.diff = Some(view);
        self.diff_active = true;
        cx.notify();
    }

    fn confirm_rollback(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let files: Vec<git::FileStatus> = self.git.status.iter().filter(|f| !self.git.excluded.contains(&f.path)).cloned().collect();
        if files.is_empty() {
            return;
        }
        let untracked = files.iter().filter(|f| f.change == git::Change::Untracked).count();
        let message = if untracked > 0 {
            format!("Roll back changes in {} file(s)? {untracked} new file(s) will be deleted.", files.len())
        } else {
            format!("Roll back changes in {} file(s)?", files.len())
        };
        let dialog = cx.new(|cx| ConfirmDialog::new(message, GitCommand::Rollback(files), window, cx));
        self.show_modal(dialog, window, cx);
    }

    pub(super) fn render_commit_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let file_row = |ix: usize, file: &git::FileStatus, cx: &mut Context<Self>| {
                let included = !self.git.excluded.contains(&file.path);
                let (dir, name) = file.path.rsplit_once('/').map_or(("", file.path.as_str()), |(d, n)| (d, n));
                let path = file.path.clone();
                let toggle_path = file.path.clone();
                div()
                    .id(("change", ix))
                    .mx_1()
                    .pl(px(28.))
                    .pr_2()
                    .h(px(24.))
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .rounded_md()
                    .cursor_pointer()
                    .hover(|d| d.bg(theme::hover()))
                    .child(
                        div()
                            .id(("include", ix))
                            .child(crate::dialogs::checkbox_box(included))
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if !this.git.excluded.remove(&toggle_path) {
                                    this.git.excluded.insert(toggle_path.clone());
                                }
                                cx.notify();
                            })),
                    )
                    .child(div().w(px(12.)).text_size(px(11.)).text_color(change_color(file.change)).child(file.change.letter()))
                    .child(div().text_color(change_color(file.change)).child(SharedString::from(name.to_string())))
                    .child(div().flex_1().overflow_hidden().whitespace_nowrap().text_size(px(11.)).text_color(theme::muted()).child(SharedString::from(dir.to_string())))
                    .on_click(cx.listener(move |this, event: &gpui::ClickEvent, window, cx| {
                        if event.click_count() >= 2 {
                            let full = this.intel.repo().map(|r| r.root().join(&path));
                            if let Some(full) = full {
                                this.open_file(&full, window, cx);
                            }
                        } else {
                            this.show_file_diff(&path, window, cx);
                        }
                    }))
                    .into_any_element()
        };
        // PhpStorm groups: tracked changes, then unversioned files.
        let mut rows: Vec<gpui::AnyElement> = Vec::new();
        for (group, title) in [("changes", "Changes"), ("unversioned", "Unversioned Files")] {
            let files: Vec<(usize, &git::FileStatus)> = self
                .git
                .status
                .iter()
                .enumerate()
                .filter(|(_, f)| (f.change == git::Change::Untracked) == (group == "unversioned"))
                .collect();
            if files.is_empty() {
                continue;
            }
            let collapsed = self.git.groups_collapsed.contains(group);
            let paths: Vec<String> = files.iter().map(|(_, f)| f.path.clone()).collect();
            let all_in = paths.iter().all(|p| !self.git.excluded.contains(p));
            rows.push(
                div()
                    .id(gpui::ElementId::Name(format!("group-{group}").into()))
                    .mx_1()
                    .px_2()
                    .h(px(24.))
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .rounded_md()
                    .cursor_pointer()
                    .hover(|d| d.bg(theme::hover()))
                    .child(div().w(px(10.)).text_size(px(9.)).text_color(theme::muted()).child(if collapsed { "▶" } else { "▼" }))
                    .child(
                        div()
                            .id(gpui::ElementId::Name(format!("group-include-{group}").into()))
                            .child(crate::dialogs::checkbox_box(all_in))
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                for path in &paths {
                                    if all_in {
                                        this.git.excluded.insert(path.clone());
                                    } else {
                                        this.git.excluded.remove(path);
                                    }
                                }
                                cx.notify();
                            })),
                    )
                    .child(div().text_color(theme::ui_text()).child(title))
                    .child(div().text_color(theme::muted()).child(format!("{} files", files.len())))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !this.git.groups_collapsed.remove(group) {
                            this.git.groups_collapsed.insert(group);
                        }
                        cx.notify();
                    }))
                    .into_any_element(),
            );
            if !collapsed {
                for (ix, file) in files {
                    rows.push(file_row(ix, file, cx));
                }
            }
        }
        let included = self.included_paths().len();
        let all_included = self.git.excluded.is_empty();
        let small = |id: &'static str, label: &'static str| {
            div().id(id).px_1p5().h(px(20.)).flex().items_center().rounded_md().cursor_pointer().text_color(theme::muted()).hover(|d| d.bg(theme::hover()).text_color(theme::ui_text())).child(label)
        };
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .px_3()
                    .h(px(26.))
                    .text_size(px(12.))
                    .child(
                        div()
                            .id("include-all")
                            .child(crate::dialogs::checkbox_box(all_included))
                            .on_click(cx.listener(|this, _, _, cx| {
                                if this.git.excluded.is_empty() {
                                    this.git.excluded = this.git.status.iter().map(|f| f.path.clone()).collect();
                                } else {
                                    this.git.excluded.clear();
                                }
                                cx.notify();
                            })),
                    )
                    .child(div().flex_1().text_color(theme::muted()).child(format!("Changes {included}/{}", self.git.status.len())))
                    .child(small("git-refresh", "⟳").on_click(cx.listener(|this, _, _, cx| this.refresh_git(cx))))
                    .child(small("git-diff-all", "⇄ Diff").on_click(cx.listener(|this, _, window, cx| {
                        // All changes in one diff tab; ←/→ walks the files.
                        if let Some(first) = this.git.status.first().map(|f| f.path.clone()) {
                            this.show_file_diff(&first, window, cx);
                        }
                    })))
                    .child(small("git-rollback", "⟲ Rollback").on_click(cx.listener(|this, _, window, cx| this.confirm_rollback(window, cx)))),
            )
            .child(
                div()
                    .id("changes")
                    .flex_1()
                    .overflow_y_scroll()
                    .text_size(px(13.))
                    .when(rows.is_empty(), |d| d.child(div().px_3().py_2().text_color(theme::muted()).child("No changes")))
                    .children(rows),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .p_2()
                    .border_t_1()
                    .border_color(theme::border())
                    .child(
                        div()
                            .h(px(110.))
                            .rounded_md()
                            .border_1()
                            .border_color(theme::border_strong())
                            .overflow_hidden()
                            .child(self.git.message.clone()),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .text_size(px(12.))
                            .child(crate::dialogs::checkbox("amend", "Amend", self.git.amend).on_click(cx.listener(|this, _, _, cx| {
                                this.git.amend ^= true;
                                // Like PhpStorm: amending starts from the last commit message.
                                if this.git.amend && this.git.message.read(cx).buffer.text().is_empty()
                                    && let Some(last) = this.intel.repo().and_then(|r| r.last_commit_message())
                                {
                                    this.git.message.update(cx, |m, cx| m.replace_all_text(&last, cx));
                                }
                                cx.notify();
                            })))
                            .child(div().flex_1())
                            .child(crate::dialogs::button("commit", "Commit", true).on_click(cx.listener(|this, _, w, cx| this.commit_changes(&CommitChanges, w, cx))))
                            .child(crate::dialogs::button("commit-push", "Commit and Push", false).on_click(cx.listener(|this, _, w, cx| this.commit_and_push(&CommitAndPush, w, cx)))),
                    ),
            )
    }

    pub(super) fn render_branch_widget(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let branch = self.git.branch.clone()?;
        let sync = match self.git.ahead_behind {
            Some((ahead, behind)) if ahead + behind > 0 => format!("  ↑{ahead} ↓{behind}"),
            _ => String::new(),
        };
        Some(
            div()
                .id("branch-widget")
                .px_1p5()
                .rounded_md()
                .cursor_pointer()
                .hover(|d| d.bg(theme::hover()).text_color(theme::ui_text()))
                .flex()
                .items_center()
                .gap_1()
                .child(crate::icons::icon(crate::icons::Icon::branch, 12., theme::muted()))
                .child(format!("{branch}{sync}"))
                .on_click(cx.listener(|this, _, w, cx| {
                    // From the status bar: open the branch popup above it.
                    this.open_branches_dropdown(w, cx);
                    this.modal_at_bottom = true;
                    this.modal_anchor = Some(8.);
                })),
        )
    }
}
