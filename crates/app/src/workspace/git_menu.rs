//! The Git menu's actions beyond commit/push/pull (PhpStorm's Git menu): fetch, merge, rebase,
//! branches and tags, reset, worktrees, stash, patches, remotes, GitHub / GitLab links, clone
//! and the VCS Operations popup (⌃V).

use gpui::{AppContext, Context, Window, actions, point, px};

use super::Workspace;
use super::context_menu::MenuCommand as M;
use crate::dialogs::{GitCommand, GitList, GitListDialog, GitPrompt, GitPromptDialog};

actions!(
    workspace,
    [
        GitFetch,
        GitMerge,
        GitRebase,
        GitNewBranch,
        GitNewTag,
        GitResetHead,
        GitNewWorktree,
        GitWorktrees,
        GitStash,
        GitUnstash,
        GitCreatePatch,
        GitApplyPatch,
        GitManageRemotes,
        GitAddRemote,
        GitOpenOnGitHub,
        GitCreatePullRequest,
        GitPullRequests,
        GitOpenOnGitLab,
        GitClone,
        VcsOperations,
    ]
);

/// `git@github.com:owner/repo.git` / `https://github.com/owner/repo.git` → `https://github.com/owner/repo`.
pub fn web_url(remote: &str) -> Option<String> {
    let remote = remote.trim().trim_end_matches(".git");
    if let Some(rest) = remote.strip_prefix("git@") {
        let (host, path) = rest.split_once(':')?;
        return Some(format!("https://{host}/{path}"));
    }
    if let Some(rest) = remote.strip_prefix("ssh://git@") {
        let (host, path) = rest.split_once('/')?;
        return Some(format!("https://{}/{path}", host.split(':').next().unwrap_or(host)));
    }
    remote.starts_with("http").then(|| remote.to_string())
}

fn base_label(merge_base: &str) -> String {
    format!("base {}", &merge_base[..merge_base.len().min(8)])
}

impl Workspace {
    fn git_raw(&mut self, title: &str, args: &[&str], changes_files: bool, cx: &mut Context<Self>) {
        self.run_git(GitCommand::Raw { title: title.into(), args: args.iter().map(|a| a.to_string()).collect(), changes_files }, cx);
    }

    fn git_prompt(&mut self, kind: GitPrompt, window: &mut Window, cx: &mut Context<Self>) {
        let dialog = cx.new(|cx| GitPromptDialog::new(kind, window, cx));
        self.show_modal(dialog, window, cx);
    }

    /// The web page of `origin` (or the first remote) whose host contains `host`, if any.
    fn remote_page(&self, host: Option<&str>) -> Option<String> {
        let repo = self.intel.repo()?;
        let remotes = repo.run(&["remote".into(), "-v".into()]).ok()?;
        let mut urls = remotes.lines().filter_map(|l| {
            let mut parts = l.split_whitespace();
            Some((parts.next()?.to_string(), parts.next()?.to_string()))
        });
        let pick = |(_, url): &(String, String)| host.is_none_or(|h| url.contains(h));
        let all: Vec<(String, String)> = urls.by_ref().collect();
        all.iter().find(|r| r.0 == "origin" && pick(r)).or_else(|| all.iter().find(|r| pick(r))).and_then(|(_, url)| web_url(url))
    }

    pub(super) fn git_fetch(&mut self, _: &GitFetch, _: &mut Window, cx: &mut Context<Self>) {
        self.run_git(GitCommand::Fetch, cx);
    }

    pub(super) fn git_merge(&mut self, _: &GitMerge, window: &mut Window, cx: &mut Context<Self>) {
        // Branches popup: each branch has Merge into Current.
        self.open_branches_dropdown(window, cx);
    }

    pub(super) fn git_rebase(&mut self, _: &GitRebase, window: &mut Window, cx: &mut Context<Self>) {
        self.git_prompt(GitPrompt::Rebase, window, cx);
    }

    pub(super) fn git_new_branch(&mut self, _: &GitNewBranch, window: &mut Window, cx: &mut Context<Self>) {
        self.git_prompt(GitPrompt::NewBranch, window, cx);
    }

    pub(super) fn git_new_tag(&mut self, _: &GitNewTag, window: &mut Window, cx: &mut Context<Self>) {
        self.git_prompt(GitPrompt::NewTag, window, cx);
    }

    pub(super) fn git_reset_head(&mut self, _: &GitResetHead, window: &mut Window, cx: &mut Context<Self>) {
        self.git_prompt(GitPrompt::ResetHead, window, cx);
    }

    pub(super) fn git_new_worktree(&mut self, _: &GitNewWorktree, window: &mut Window, cx: &mut Context<Self>) {
        self.git_prompt(GitPrompt::NewWorktree, window, cx);
    }

    pub(super) fn git_worktrees(&mut self, _: &GitWorktrees, window: &mut Window, cx: &mut Context<Self>) {
        let Some(repo) = self.intel.repo().cloned() else { return };
        let dialog = cx.new(|cx| GitListDialog::new(GitList::Worktrees(repo.worktrees()), window, cx));
        self.show_modal(dialog, window, cx);
    }

    pub(super) fn git_stash(&mut self, _: &GitStash, _: &mut Window, cx: &mut Context<Self>) {
        self.git_raw("stash (incl. untracked)", &["stash", "push", "--include-untracked", "-m", "PhpCraft stash"], true, cx);
    }

    pub(super) fn git_unstash(&mut self, _: &GitUnstash, _: &mut Window, cx: &mut Context<Self>) {
        self.git_raw("stash pop", &["stash", "pop"], true, cx);
    }

    pub(super) fn git_create_patch(&mut self, _: &GitCreatePatch, window: &mut Window, cx: &mut Context<Self>) {
        self.git_prompt(GitPrompt::CreatePatch, window, cx);
    }

    pub(super) fn git_apply_patch(&mut self, _: &GitApplyPatch, window: &mut Window, cx: &mut Context<Self>) {
        self.git_prompt(GitPrompt::ApplyPatch, window, cx);
    }

    pub(super) fn git_manage_remotes(&mut self, _: &GitManageRemotes, window: &mut Window, cx: &mut Context<Self>) {
        let Some(repo) = self.intel.repo().cloned() else { return };
        let dialog = cx.new(|cx| GitListDialog::new(GitList::Remotes(repo.remotes()), window, cx));
        self.show_modal(dialog, window, cx);
    }

    pub(super) fn git_add_remote(&mut self, _: &GitAddRemote, window: &mut Window, cx: &mut Context<Self>) {
        self.git_prompt(GitPrompt::AddRemote, window, cx);
    }

    fn open_remote(&mut self, host: Option<&str>, suffix: &str, cx: &mut Context<Self>) {
        match self.remote_page(host) {
            Some(url) => cx.open_url(&format!("{url}{suffix}")),
            None => {
                self.output = Some(super::tools::Output { title: "Git".into(), text: format!("No {} remote in this repository.", host.unwrap_or("web")), running: false, ok: false });
                cx.notify();
            }
        }
    }

    pub(super) fn git_open_on_github(&mut self, _: &GitOpenOnGitHub, _: &mut Window, cx: &mut Context<Self>) {
        let branch = self.git.branch.clone().map(|b| format!("/tree/{b}")).unwrap_or_default();
        self.open_remote(Some("github"), &branch, cx);
    }

    pub(super) fn git_create_pull_request(&mut self, _: &GitCreatePullRequest, _: &mut Window, cx: &mut Context<Self>) {
        // GitHub's compare page starts a pull request for the current branch.
        let branch = self.git.branch.clone().map(|b| format!("/compare/{b}?expand=1")).unwrap_or_default();
        self.open_remote(Some("github"), &branch, cx);
    }

    pub(super) fn git_pull_requests(&mut self, _: &GitPullRequests, window: &mut Window, cx: &mut Context<Self>) {
        let Some(repo) = self.intel.repo().cloned() else { return };
        let remotes = repo.remotes();
        let origin = remotes.iter().find(|(name, _)| name == "origin").or(remotes.first());
        let Some(host) = origin.and_then(|(_, url)| crate::pull_requests::host(url)) else {
            self.output = Some(super::tools::Output { title: "Pull Requests".into(), text: "The origin remote is neither GitHub nor GitLab.".into(), running: false, ok: false });
            cx.notify();
            return;
        };
        let root = repo.root().to_path_buf();
        let dialog = cx.new(|cx| crate::dialogs::PullRequestsDialog::new(root, host, window, cx));
        self.show_modal(dialog, window, cx);
    }

    /// The request's head (fetched into `refs/phpcraft/pr-N`) against its merge base with
    /// `origin/<base>`, file by file.
    pub(super) fn show_pull_request_diff(&mut self, host: crate::pull_requests::Host, number: u64, base: String, file: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(repo) = self.intel.repo().cloned() else { return };
        let task = self.begin_task(format!("Fetching #{number}…"), cx);
        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let head = crate::pull_requests::fetch_head(repo.root(), host, number)?;
                    let _ = repo.run(&["fetch".into(), "--quiet".into(), "origin".into(), base.clone()]);
                    let merge_base = repo.run(&["merge-base".into(), format!("origin/{base}"), head.clone()])?.trim().to_string();
                    let files: Vec<String> = repo.run(&["diff".into(), "--name-only".into(), merge_base.clone(), head.clone()])?.lines().map(str::to_string).collect();
                    Ok::<_, String>((repo, head, merge_base, files))
                })
                .await;
            this.update_in(cx, |this, window, cx| {
                this.end_task(task, cx);
                match result {
                    Ok((_, _, _, files)) if files.is_empty() => {
                        this.output = Some(super::tools::Output { title: format!("Pull request #{number}"), text: "No changes against the base branch.".into(), running: false, ok: true })
                    }
                    Ok((repo, head, merge_base, files)) => {
                        let index = file.and_then(|f| files.iter().position(|x| *x == f)).unwrap_or(0);
                        let sources = files
                            .iter()
                            .map(|path| crate::diff_view::DiffSource {
                                title: path.rsplit('/').next().unwrap_or(path).to_string(),
                                subtitle: path.clone(),
                                load: {
                                    let (repo, head, merge_base, path) = (repo.clone(), head.clone(), merge_base.clone(), path.clone());
                                    std::sync::Arc::new(move || (repo.show(&merge_base, &path).unwrap_or_default(), repo.show(&head, &path).unwrap_or_default()))
                                },
                                left_label: base_label(&merge_base),
                                right_label: format!("#{number}"),
                                open_path: Some(repo.root().join(path)),
                            })
                            .collect();
                        this.open_diff_tab(&format!("#{number}"), sources, index, window, cx);
                    }
                    Err(err) => this.output = Some(super::tools::Output { title: format!("Pull request #{number}"), text: err, running: false, ok: false }),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn git_open_on_gitlab(&mut self, _: &GitOpenOnGitLab, _: &mut Window, cx: &mut Context<Self>) {
        let branch = self.git.branch.clone().map(|b| format!("/-/tree/{b}")).unwrap_or_default();
        self.open_remote(Some("gitlab"), &branch, cx);
    }

    pub(super) fn git_clone(&mut self, _: &GitClone, window: &mut Window, cx: &mut Context<Self>) {
        let dialog = cx.new(|cx| crate::dialogs::ProjectDialog::clone_repository(window, cx));
        self.show_modal(dialog, window, cx);
    }

    /// ⌃V: the VCS operations as a popup near the top of the window.
    pub(super) fn vcs_operations(&mut self, _: &VcsOperations, window: &mut Window, cx: &mut Context<Self>) {
        let a = |label: &'static str, key: &'static str, action: &'static str| Some((label, key, M::Action(action), true));
        let items = vec![
            a("Commit…", "⌘0", "workspace::ShowCommit"),
            a("Push…", "⇧⌘K", "workspace::GitPush"),
            a("Update Project…", "", "workspace::GitPull"),
            a("Fetch", "", "workspace::GitFetch"),
            None,
            a("Branches…", "", "workspace::OpenBranches"),
            a("New Branch…", "⌥⌘N", "workspace::GitNewBranch"),
            a("New Tag…", "", "workspace::GitNewTag"),
            a("Merge…", "", "workspace::GitMerge"),
            a("Rebase…", "", "workspace::GitRebase"),
            None,
            a("Stash Changes", "", "workspace::GitStash"),
            a("Unstash Changes", "", "workspace::GitUnstash"),
            a("Show Git Log", "⌘9", "workspace::ToggleGitLog"),
        ];
        let width = window.viewport_size().width;
        self.open_menu_at(point(width / 2. - px(140.), px(60.)), items, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_urls() {
        assert_eq!(web_url("git@github.com:owner/repo.git").as_deref(), Some("https://github.com/owner/repo"));
        assert_eq!(web_url("https://gitlab.com/g/p.git").as_deref(), Some("https://gitlab.com/g/p"));
        assert_eq!(web_url("ssh://git@github.com:22/owner/repo.git").as_deref(), Some("https://github.com/owner/repo"));
        assert_eq!(web_url("/local/path"), None);
    }
}
