//! History rewriting from the Git menu and the Git Log: Interactive Rebase (from the selected
//! commit, or the last 10), Cherry-Pick, Revert, Undo Last Commit, and Continue / Skip / Abort
//! for a rebase, cherry-pick or merge that stopped on conflicts.

use gpui::{AppContext, Context, Pixels, Point, Window, actions};

use super::Workspace;
use super::context_menu::MenuCommand as M;
use crate::dialogs::{GitCommand, RebaseDialog};

actions!(
    workspace,
    [GitInteractiveRebase, GitCherryPick, GitRevertCommit, GitUndoCommit, GitRebaseContinue, GitRebaseSkip, GitRebaseAbort]
);

impl Workspace {
    fn selected_commit(&self) -> Option<String> {
        self.git.log_selected.and_then(|ix| self.git.log.get(ix)).map(|c| c.hash.clone())
    }

    fn git_run(&mut self, title: &str, args: &[&str], cx: &mut Context<Self>) {
        self.run_git(GitCommand::Raw { title: title.into(), args: args.iter().map(|a| a.to_string()).collect(), changes_files: true }, cx);
    }

    /// Interactive Rebase from the commit selected in the Git Log (included), or of the last 10.
    pub(super) fn git_interactive_rebase(&mut self, _: &GitInteractiveRebase, window: &mut Window, cx: &mut Context<Self>) {
        let Some(repo) = self.intel.repo().cloned() else { return };
        let base = match self.selected_commit() {
            Some(hash) => format!("{hash}^"),
            None => {
                let count = repo.run(&["rev-list".into(), "--count".into(), "HEAD".into()]).ok().and_then(|c| c.trim().parse::<usize>().ok()).unwrap_or(0);
                format!("HEAD~{}", count.saturating_sub(1).min(10))
            }
        };
        let base = repo.run(&["rev-parse".into(), base.clone()]).map(|b| b.trim().to_string()).unwrap_or(base);
        let commits = match repo.commits_since(&base) {
            Ok(c) if !c.is_empty() => c,
            Ok(_) => return self.set_output("Interactive Rebase", "No commits to rebase.".into(), true, cx),
            Err(e) => return self.set_output("Interactive Rebase", format!("{e}\n(The first commit of a repository can't be the base.)"), false, cx),
        };
        let dialog = cx.new(|cx| RebaseDialog::new(base, commits, window, cx));
        self.show_modal(dialog, window, cx);
    }

    fn set_output(&mut self, title: &str, text: String, ok: bool, cx: &mut Context<Self>) {
        self.output = Some(super::tools::Output { title: title.into(), text, running: false, ok });
        cx.notify();
    }

    pub(super) fn git_cherry_pick(&mut self, _: &GitCherryPick, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(hash) = self.selected_commit() {
            self.git_run(&format!("Cherry-pick {}", &hash[..8.min(hash.len())]), &["cherry-pick", &hash], cx);
        }
    }

    pub(super) fn git_revert_commit(&mut self, _: &GitRevertCommit, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(hash) = self.selected_commit() {
            self.git_run(&format!("Revert {}", &hash[..8.min(hash.len())]), &["revert", "--no-edit", &hash], cx);
        }
    }

    /// Undo Last Commit: the commit goes away, its changes stay staged.
    pub(super) fn git_undo_commit(&mut self, _: &GitUndoCommit, _: &mut Window, cx: &mut Context<Self>) {
        self.git_run("Undo last commit", &["reset", "--soft", "HEAD~1"], cx);
    }

    /// The operation in progress (rebase, cherry-pick, revert or merge) from `.git`.
    fn operation_in_progress(&self) -> Option<&'static str> {
        let repo = self.intel.repo()?;
        let dir = repo.run(&["rev-parse".into(), "--absolute-git-dir".into()]).ok()?;
        let dir = std::path::Path::new(dir.trim());
        if dir.join("rebase-merge").exists() || dir.join("rebase-apply").exists() {
            Some("rebase")
        } else if dir.join("CHERRY_PICK_HEAD").exists() {
            Some("cherry-pick")
        } else if dir.join("REVERT_HEAD").exists() {
            Some("revert")
        } else if dir.join("MERGE_HEAD").exists() {
            Some("merge")
        } else {
            None
        }
    }

    fn continue_or(&mut self, flag: &str, cx: &mut Context<Self>) {
        let Some(op) = self.operation_in_progress() else {
            return self.set_output("Git", "Nothing to continue: no rebase, cherry-pick, revert or merge in progress.".into(), true, cx);
        };
        let flag = if op == "merge" && flag == "--skip" { "--abort" } else { flag };
        self.git_run(&format!("{op} {flag}"), &["-c", "core.editor=true", op, flag], cx);
    }

    pub(super) fn git_rebase_continue(&mut self, _: &GitRebaseContinue, _: &mut Window, cx: &mut Context<Self>) {
        self.continue_or("--continue", cx);
    }

    pub(super) fn git_rebase_skip(&mut self, _: &GitRebaseSkip, _: &mut Window, cx: &mut Context<Self>) {
        self.continue_or("--skip", cx);
    }

    pub(super) fn git_rebase_abort(&mut self, _: &GitRebaseAbort, _: &mut Window, cx: &mut Context<Self>) {
        self.continue_or("--abort", cx);
    }

    /// Right-click on a commit in the Git Log.
    pub(super) fn open_commit_menu(&mut self, ix: usize, position: Point<Pixels>, cx: &mut Context<Self>) {
        self.git.log_selected = Some(ix);
        let items = vec![
            Some(("Cherry-Pick", "", M::Action("workspace::GitCherryPick"), true)),
            Some(("Revert Commit", "", M::Action("workspace::GitRevertCommit"), true)),
            Some(("Interactively Rebase from Here…", "", M::Action("workspace::GitInteractiveRebase"), true)),
            None,
            Some(("Undo Last Commit", "", M::Action("workspace::GitUndoCommit"), ix == 0)),
        ];
        self.open_menu_at(position, items, cx);
    }
}
