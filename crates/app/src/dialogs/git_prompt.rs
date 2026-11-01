//! One-field dialogs for the Git menu: New Branch, New Tag, Rebase, Reset HEAD, New Worktree,
//! Add Remote, Create Patch and Apply Patch.

use gpui::{App, AppContext, Context, Entity, EventEmitter, FocusHandle, Focusable, Window, div, prelude::*, px};

use super::{Cancel, Confirm, DialogEvent, GitCommand, button, card};
use crate::text_field::TextField;
use crate::theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitPrompt {
    NewBranch,
    NewTag,
    Rebase,
    ResetHead,
    NewWorktree,
    AddRemote,
    CreatePatch,
    ApplyPatch,
}

impl GitPrompt {
    fn title(self) -> &'static str {
        match self {
            GitPrompt::NewBranch => "New Branch",
            GitPrompt::NewTag => "New Tag",
            GitPrompt::Rebase => "Rebase Current Branch onto…",
            GitPrompt::ResetHead => "Reset HEAD (mixed; add --hard or --soft before the target)",
            GitPrompt::NewWorktree => "New Worktree: folder, then an optional new branch",
            GitPrompt::AddRemote => "Add Remote: name and URL",
            GitPrompt::CreatePatch => "Create Patch from Uncommitted Changes",
            GitPrompt::ApplyPatch => "Apply Patch",
        }
    }

    fn placeholder(self) -> &'static str {
        match self {
            GitPrompt::NewBranch => "feature/login",
            GitPrompt::NewTag => "v1.0.0",
            GitPrompt::Rebase => "main",
            GitPrompt::ResetHead => "HEAD~1",
            GitPrompt::NewWorktree => "../app-hotfix hotfix",
            GitPrompt::AddRemote => "upstream https://github.com/owner/repo.git",
            GitPrompt::CreatePatch | GitPrompt::ApplyPatch => "changes.patch",
        }
    }

    fn initial(self) -> &'static str {
        match self {
            GitPrompt::ResetHead => "HEAD",
            GitPrompt::CreatePatch | GitPrompt::ApplyPatch => "changes.patch",
            _ => "",
        }
    }

    /// The git command for what was typed.
    pub fn command(self, text: &str) -> Result<GitCommand, String> {
        let text = text.trim();
        let words: Vec<String> = text.split_whitespace().map(str::to_string).collect();
        if words.is_empty() {
            return Err("Type a value".into());
        }
        let raw = |title: String, args: Vec<String>, changes_files: bool| GitCommand::Raw { title, args, changes_files };
        let arg = |s: &str| s.to_string();
        Ok(match self {
            GitPrompt::NewBranch => GitCommand::CreateBranch { name: words[0].clone(), from: None },
            GitPrompt::NewTag => raw(format!("tag {}", words[0]), vec![arg("tag"), words[0].clone()], false),
            GitPrompt::Rebase => raw(format!("rebase {}", words[0]), vec![arg("rebase"), words[0].clone()], true),
            GitPrompt::ResetHead => raw(format!("reset {text}"), [vec![arg("reset")], words].concat(), true),
            GitPrompt::NewWorktree => match words.as_slice() {
                [path] => raw(format!("worktree add {path}"), vec![arg("worktree"), arg("add"), path.clone()], false),
                [path, branch, ..] => raw(format!("worktree add {path} ({branch})"), vec![arg("worktree"), arg("add"), arg("-b"), branch.clone(), path.clone()], false),
                [] => unreachable!(),
            },
            GitPrompt::AddRemote => match words.as_slice() {
                [name, url, ..] => raw(format!("remote add {name}"), vec![arg("remote"), arg("add"), name.clone(), url.clone()], false),
                _ => return Err("Type a name and a URL".into()),
            },
            GitPrompt::CreatePatch => raw(format!("diff → {text}"), vec![arg("diff"), arg("HEAD"), format!("--output={text}")], false),
            GitPrompt::ApplyPatch => raw(format!("apply {text}"), vec![arg("apply"), text.to_string()], true),
        })
    }
}

pub struct GitPromptDialog {
    kind: GitPrompt,
    field: Entity<TextField>,
    error: Option<String>,
    focus_handle: FocusHandle,
}

impl EventEmitter<DialogEvent> for GitPromptDialog {}

impl GitPromptDialog {
    pub fn new(kind: GitPrompt, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let field = cx.new(|cx| {
            let mut f = TextField::new(kind.placeholder(), cx);
            f.set_text(kind.initial(), cx);
            f
        });
        window.focus(&field.focus_handle(cx));
        Self { kind, field, error: None, focus_handle: cx.focus_handle() }
    }

    fn confirm(&mut self, _: &Confirm, _: &mut Window, cx: &mut Context<Self>) {
        match self.kind.command(self.field.read(cx).text()) {
            Ok(command) => cx.emit(DialogEvent::Git(command)),
            Err(err) => {
                self.error = Some(err);
                cx.notify();
            }
        }
    }

    fn cancel(&mut self, _: &Cancel, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(DialogEvent::Dismiss);
    }
}

impl Focusable for GitPromptDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for GitPromptDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        card(520.)
            .key_context("Dialog")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::cancel))
            .p_4()
            .flex()
            .flex_col()
            .gap_3()
            .child(div().text_color(theme::ui_text()).child(self.kind.title()))
            .child(div().flex().child(self.field.clone()))
            .children(self.error.clone().map(|e| div().text_size(px(12.)).text_color(theme::error()).child(e)))
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .child(button("git-prompt-cancel", "Cancel", false).on_click(cx.listener(|this, _, window, cx| this.cancel(&Cancel, window, cx))))
                    .child(button("git-prompt-ok", "OK", true).on_click(cx.listener(|this, _, window, cx| this.confirm(&Confirm, window, cx)))),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands() {
        let args = |c: GitCommand| match c {
            GitCommand::Raw { args, .. } => args,
            _ => vec![],
        };
        assert_eq!(args(GitPrompt::ResetHead.command("--hard HEAD~1").unwrap()), ["reset", "--hard", "HEAD~1"]);
        assert_eq!(args(GitPrompt::NewWorktree.command("../hotfix fix").unwrap()), ["worktree", "add", "-b", "fix", "../hotfix"]);
        assert!(GitPrompt::AddRemote.command("upstream").is_err());
        assert!(matches!(GitPrompt::NewBranch.command("feature/x").unwrap(), GitCommand::CreateBranch { .. }));
    }
}
