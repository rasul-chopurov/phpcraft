//! Git in the workspace: the Commit tool window (left panel), the Git Log panel, the branch
//! widget in the status bar, and running git commands in the background.

use std::collections::HashSet;
use std::time::Duration;

use gpui::{AppContext, Context, Entity, Focusable, MouseButton, SharedString, Window, actions, div, prelude::*, px};

use super::Workspace;
use super::tools::Output;
use crate::dialogs::{BranchDialog, ConfirmDialog, DiffDialog, GitCommand};
use crate::text_field::TextField;
use crate::theme;

actions!(workspace, [ShowProject, ShowCommit, ToggleGitLog, OpenBranches, CommitChanges, CommitAndPush, GitPull, GitPush]);

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum LeftPanel {
    Project,
    Commit,
}

pub(super) struct GitState {
    pub status: Vec<git::FileStatus>,
    pub branch: Option<String>,
    pub ahead_behind: Option<(usize, usize)>,
    /// Files excluded from the next commit (PhpStorm-style checkboxes; included by default).
    pub excluded: HashSet<String>,
    pub message: Entity<TextField>,
    pub amend: bool,
    pub log_open: bool,
    pub log: Vec<git::Commit>,
    pub log_selected: Option<usize>,
    pub log_files: Vec<(git::Change, String)>,
}

impl GitState {
    pub fn new(cx: &mut Context<Workspace>) -> Self {
        Self {
            status: Vec::new(),
            branch: None,
            ahead_behind: None,
            excluded: HashSet::new(),
            message: cx.new(|cx| TextField::new("Commit message", cx)),
            amend: false,
            log_open: false,
            log: Vec::new(),
            log_selected: None,
            log_files: Vec::new(),
        }
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
                if this.update(cx, |this, cx| this.refresh_git(cx)).is_err() {
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

    fn refresh_log(&mut self, cx: &mut Context<Self>) {
        let Some(repo) = self.intel.repo().cloned() else { return };
        cx.spawn(async move |this, cx| {
            let log = cx.background_executor().spawn(async move { repo.log(300).unwrap_or_default() }).await;
            this.update(cx, |this, cx| {
                this.git.log = log;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn show_project(&mut self, _: &ShowProject, _: &mut Window, cx: &mut Context<Self>) {
        self.left_panel = LeftPanel::Project;
        cx.notify();
    }

    pub(super) fn show_commit(&mut self, _: &ShowCommit, window: &mut Window, cx: &mut Context<Self>) {
        self.left_panel = LeftPanel::Commit;
        self.refresh_git(cx);
        window.focus(&self.git.message.focus_handle(cx));
        cx.notify();
    }

    pub(super) fn toggle_git_log(&mut self, _: &ToggleGitLog, _: &mut Window, cx: &mut Context<Self>) {
        self.git.log_open = !self.git.log_open;
        if self.git.log_open {
            self.refresh_log(cx);
        }
        cx.notify();
    }

    pub(super) fn open_branches(&mut self, _: &OpenBranches, window: &mut Window, cx: &mut Context<Self>) {
        let Some(repo) = self.intel.repo().cloned() else { return };
        let dialog = cx.new(|cx| BranchDialog::new(&repo, window, cx));
        self.show_modal(dialog, window, cx);
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
        let message = self.git.message.read(cx).text().trim().to_string();
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
                    this.git.message.update(cx, |m, cx| m.set_text("", cx));
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
            GitCommand::CreateBranch { name, .. } => format!("New branch {name}"),
            GitCommand::Merge(b) => format!("Merge {b}"),
            GitCommand::Rename { old, new } => format!("Rename {old} → {new}"),
            GitCommand::Delete(b) => format!("Delete branch {b}"),
            GitCommand::Pull => "Pull".into(),
            GitCommand::Push => "Push".into(),
            GitCommand::Fetch => "Fetch".into(),
            GitCommand::Rollback(files) => format!("Rollback {} file(s)", files.len()),
        };
        // Switching branches rewrites files: save edits first.
        if matches!(command, GitCommand::Checkout(_) | GitCommand::CreateBranch { .. } | GitCommand::Merge(_) | GitCommand::Pull) {
            self.save_all(cx);
        }
        let rescan = matches!(command, GitCommand::Checkout(_) | GitCommand::Merge(_) | GitCommand::Pull);
        let task = self.begin_task(format!("git: {title}…"), cx);
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    match command {
                        GitCommand::Checkout(branch) => repo.checkout(&branch),
                        GitCommand::CreateBranch { name, from: Some(from) } => repo.create_branch_from(&name, &from),
                        GitCommand::CreateBranch { name, from: None } => repo.create_branch(&name),
                        GitCommand::Merge(branch) => repo.merge(&branch),
                        GitCommand::Rename { old, new } => repo.rename_branch(&old, &new),
                        GitCommand::Delete(branch) => repo.delete_branch(&branch, false),
                        GitCommand::Pull => repo.pull(),
                        GitCommand::Push => repo.push(),
                        GitCommand::Fetch => repo.fetch(),
                        GitCommand::Rollback(files) => repo.discard(&files).map(|_| "Changes rolled back.".to_string()),
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
        if self.git.log_open {
            self.refresh_log(cx);
        }
    }

    /// Re-indexes the project after files changed on disk (checkout, merge, pull).
    fn rescan(&mut self, cx: &mut Context<Self>) {
        let intel = self.intel.clone();
        let task = self.begin_task("Indexing…", cx);
        cx.spawn(async move |this, cx| {
            cx.background_executor().spawn(async move { intel.scan() }).await;
            this.update(cx, |this, cx| this.end_task(task, cx)).ok();
        })
        .detach();
    }

    fn show_file_diff(&mut self, path: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(repo) = self.intel.repo().cloned() else { return };
        let full = repo.root().join(path);
        let old = repo.show("HEAD", path).unwrap_or_default();
        let new = std::fs::read_to_string(&full).unwrap_or_default();
        let title = path.to_string();
        let dialog = cx.new(|cx| DiffDialog::new(title, &old, &new, Some(full), window, cx));
        self.show_modal(dialog, window, cx);
    }

    fn show_commit_diff(&mut self, hash: &str, path: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(repo) = self.intel.repo().cloned() else { return };
        let old = repo.show(&format!("{hash}^"), path).unwrap_or_default();
        let new = repo.show(hash, path).unwrap_or_default();
        let title = format!("{path} @ {}", &hash[..hash.len().min(8)]);
        let dialog = cx.new(|cx| DiffDialog::new(title, &old, &new, None, window, cx));
        self.show_modal(dialog, window, cx);
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

    pub(super) fn render_left_tabs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let tab = |id: &'static str, label: String, active: bool| {
            div()
                .id(id)
                .px_2()
                .h(px(22.))
                .flex()
                .items_center()
                .rounded_md()
                .cursor_pointer()
                .text_size(px(11.))
                .when(active, |d| d.bg(theme::hover()).text_color(theme::ui_text()))
                .when(!active, |d| d.text_color(theme::muted()))
                .hover(|d| d.bg(theme::hover()))
                .child(label)
        };
        let changes = self.git.status.len();
        div()
            .flex()
            .gap_1()
            .px_2()
            .h(px(32.))
            .items_center()
            .child(tab("left-project", "PROJECT".into(), self.left_panel == LeftPanel::Project).on_click(cx.listener(|this, _, w, cx| this.show_project(&ShowProject, w, cx))))
            .when(self.intel.repo().is_some(), |d| {
                d.child(
                    tab("left-commit", if changes > 0 { format!("COMMIT · {changes}") } else { "COMMIT".into() }, self.left_panel == LeftPanel::Commit)
                        .on_click(cx.listener(|this, _, w, cx| this.show_commit(&ShowCommit, w, cx))),
                )
            })
    }

    pub(super) fn render_commit_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let rows: Vec<_> = self
            .git
            .status
            .iter()
            .enumerate()
            .map(|(ix, file)| {
                let included = !self.git.excluded.contains(&file.path);
                let (dir, name) = file.path.rsplit_once('/').map_or(("", file.path.as_str()), |(d, n)| (d, n));
                let path = file.path.clone();
                let toggle_path = file.path.clone();
                div()
                    .id(("change", ix))
                    .mx_1()
                    .px_2()
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
            })
            .collect();
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
                    .child(self.git.message.clone())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .text_size(px(12.))
                            .child(crate::dialogs::checkbox("amend", "Amend", self.git.amend).on_click(cx.listener(|this, _, _, cx| {
                                this.git.amend ^= true;
                                // Like PhpStorm: amending starts from the last commit message.
                                if this.git.amend && this.git.message.read(cx).text().is_empty()
                                    && let Some(last) = this.intel.repo().and_then(|r| r.last_commit_message())
                                {
                                    this.git.message.update(cx, |m, cx| m.set_text(last, cx));
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
                .child(format!("⎇ {branch}{sync}"))
                .on_click(cx.listener(|this, _, w, cx| this.open_branches(&OpenBranches, w, cx))),
        )
    }

    pub(super) fn render_git_log(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        if !self.git.log_open {
            return None;
        }
        let commits: Vec<_> = self
            .git
            .log
            .iter()
            .enumerate()
            .map(|(ix, commit)| {
                let selected = self.git.log_selected == Some(ix);
                let hash = commit.hash.clone();
                div()
                    .id(("commit", ix))
                    .flex()
                    .gap_3()
                    .px_2()
                    .h(px(22.))
                    .items_center()
                    .cursor_pointer()
                    .when(selected, |d| d.bg(theme::selection()))
                    .hover(|d| d.bg(theme::hover()))
                    .child(div().w(px(64.)).flex_none().font_family(theme::editor_font()).text_color(theme::muted()).child(commit.short.clone()))
                    .child(div().flex_1().overflow_hidden().whitespace_nowrap().text_color(theme::ui_text()).child(commit.subject.clone()))
                    .when(!commit.refs.is_empty(), |d| {
                        d.child(div().px_1().rounded_sm().text_size(px(11.)).text_color(theme::accent()).border_1().border_color(theme::accent()).child(commit.refs.clone()))
                    })
                    .child(div().w(px(120.)).flex_none().overflow_hidden().text_color(theme::muted()).child(commit.author.clone()))
                    .child(div().w(px(80.)).flex_none().text_color(theme::muted()).child(commit.date.clone()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.git.log_selected = Some(ix);
                        this.git.log_files = this.intel.repo().and_then(|r| r.commit_files(&hash).ok()).unwrap_or_default();
                        cx.notify();
                    }))
            })
            .collect();
        let selected_hash = self.git.log_selected.and_then(|i| self.git.log.get(i)).map(|c| c.hash.clone());
        let files: Vec<_> = self
            .git
            .log_files
            .iter()
            .enumerate()
            .map(|(ix, (change, path))| {
                let path = path.clone();
                let hash = selected_hash.clone().unwrap_or_default();
                div()
                    .id(("commit-file", ix))
                    .flex()
                    .gap_2()
                    .px_2()
                    .h(px(22.))
                    .items_center()
                    .cursor_pointer()
                    .hover(|d| d.bg(theme::hover()))
                    .child(div().w(px(12.)).text_color(change_color(*change)).child(change.letter()))
                    .child(div().overflow_hidden().whitespace_nowrap().child(SharedString::from(path.clone())))
                    .on_click(cx.listener(move |this, _, window, cx| this.show_commit_diff(&hash, &path, window, cx)))
            })
            .collect();
        Some(
            div()
                .flex()
                .flex_col()
                .flex_none()
                .h(px(260.))
                .border_t_1()
                .border_color(theme::border_strong())
                .bg(theme::editor_bg())
                .text_size(px(12.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .px_2()
                        .h(px(26.))
                        .bg(theme::panel_bg())
                        .child(div().text_color(theme::ui_text()).child("Git Log"))
                        .child(div().flex_1())
                        .child(div().id("log-refresh").px_1().cursor_pointer().text_color(theme::muted()).child("⟳").on_click(cx.listener(|this, _, _, cx| this.refresh_log(cx))))
                        .child(div().id("log-close").px_1().cursor_pointer().text_color(theme::muted()).child("×").on_click(cx.listener(|this, _, w, cx| this.toggle_git_log(&ToggleGitLog, w, cx)))),
                )
                .child(
                    div()
                        .flex()
                        .flex_1()
                        .min_h(px(0.))
                        .child(div().id("log-commits").flex_1().overflow_y_scroll().children(commits))
                        .child(div().id("log-files").w(px(320.)).flex_none().border_l_1().border_color(theme::border()).overflow_y_scroll().children(files)),
                ),
        )
    }
}
