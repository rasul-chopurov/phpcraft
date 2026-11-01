//! Git ▸ Worktrees… and Git ▸ Manage Remotes…: lists with Open / Remove and Add buttons.

use gpui::{App, Context, EventEmitter, FocusHandle, Focusable, Window, div, prelude::*, px};

use super::{Cancel, DialogEvent, GitCommand, OpenMode, button, card};
use crate::theme;

pub enum GitList {
    Worktrees(Vec<git::Worktree>),
    Remotes(Vec<(String, String)>),
}

pub struct GitListDialog {
    list: GitList,
    focus_handle: FocusHandle,
}

impl EventEmitter<DialogEvent> for GitListDialog {}

impl GitListDialog {
    pub fn new(list: GitList, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        window.focus(&focus_handle);
        Self { list, focus_handle }
    }

    fn cancel(&mut self, _: &Cancel, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(DialogEvent::Dismiss);
    }
}

impl Focusable for GitListDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

fn raw(title: String, args: &[&str]) -> DialogEvent {
    DialogEvent::Git(GitCommand::Raw { title, args: args.iter().map(|a| a.to_string()).collect(), changes_files: false })
}

impl Render for GitListDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let link = |id: gpui::SharedString, label: &'static str| div().id(id).px_1p5().rounded_sm().text_color(theme::accent()).cursor_pointer().hover(|d| d.bg(theme::hover())).child(label);
        let (title, add_label, add_action, rows): (&str, &'static str, &'static str, Vec<gpui::AnyElement>) = match &self.list {
            GitList::Worktrees(trees) => (
                "Worktrees",
                "New Worktree…",
                "workspace::GitNewWorktree",
                trees
                    .iter()
                    .enumerate()
                    .map(|(ix, t)| {
                        let (open, remove) = (t.path.clone(), t.path.to_string_lossy().to_string());
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .px_3()
                            .py_1()
                            .child(
                                div()
                                    .flex_1()
                                    .overflow_hidden()
                                    .child(div().text_color(theme::ui_text()).child(if t.branch.is_empty() { format!("(detached {})", t.head) } else { t.branch.clone() }))
                                    .child(div().text_size(px(11.5)).text_color(theme::muted()).child(t.path.display().to_string())),
                            )
                            .child(link(format!("wt-open-{ix}").into(), "Open").on_click(cx.listener(move |_, _, _, cx| cx.emit(DialogEvent::OpenProject(open.clone(), OpenMode::NewWindow)))))
                            .when(ix > 0, |d| {
                                d.child(link(format!("wt-remove-{ix}").into(), "Remove").on_click(cx.listener(move |_, _, _, cx| cx.emit(raw(format!("worktree remove {remove}"), &["worktree", "remove", &remove])))))
                            })
                            .into_any_element()
                    })
                    .collect(),
            ),
            GitList::Remotes(remotes) => (
                "Remotes",
                "Add Remote…",
                "workspace::GitAddRemote",
                remotes
                    .iter()
                    .enumerate()
                    .map(|(ix, (name, url))| {
                        let name2 = name.clone();
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .px_3()
                            .py_1()
                            .child(div().w(px(110.)).text_color(theme::ui_text()).child(name.clone()))
                            .child(div().flex_1().overflow_hidden().text_color(theme::muted()).child(url.clone()))
                            .child(link(format!("rm-remove-{ix}").into(), "Remove").on_click(cx.listener(move |_, _, _, cx| cx.emit(raw(format!("remote remove {name2}"), &["remote", "remove", &name2])))))
                            .into_any_element()
                    })
                    .collect(),
            ),
        };
        let empty = rows.is_empty();
        card(620.)
            .key_context("Dialog")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::cancel))
            .child(div().px_3().py_2().border_b_1().border_color(theme::border()).child(title))
            .child(div().id("git-list").max_h(px(380.)).overflow_y_scroll().py_1().children(rows))
            .when(empty, |d| d.child(div().px_3().py_2().text_color(theme::muted()).child("None yet.")))
            .child(
                div()
                    .flex()
                    .justify_between()
                    .p_3()
                    .child(button("git-list-add", add_label, false).on_click(cx.listener(move |_, _, _, cx| cx.emit(DialogEvent::Dispatch(add_action)))))
                    .child(button("git-list-close", "Close", true).on_click(cx.listener(|this, _, window, cx| this.cancel(&Cancel, window, cx)))),
            )
    }
}
