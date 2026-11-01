//! Git ▸ Interactive Rebase: the commits after a base, oldest first. Click an action to cycle
//! pick → reword → squash → fixup → drop, ↑ / ↓ reorder, a reword gets a new message.

use git::{RebaseAction, RebaseStep};
use gpui::{App, Context, Entity, EventEmitter, FocusHandle, Focusable, Window, div, prelude::*, px};

use super::{Cancel, Confirm, DialogEvent, GitCommand, button, card};
use crate::text_field::TextField;
use crate::theme;

pub struct RebaseDialog {
    base: String,
    steps: Vec<RebaseStep>,
    selected: usize,
    message: Entity<TextField>,
    focus_handle: FocusHandle,
    error: Option<String>,
}

impl EventEmitter<DialogEvent> for RebaseDialog {}

impl RebaseDialog {
    pub fn new(base: String, commits: Vec<(String, String)>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let steps = commits
            .into_iter()
            .map(|(hash, subject)| RebaseStep { action: RebaseAction::Pick, message: subject.clone(), hash, subject })
            .collect();
        let message = cx.new(|cx| TextField::new("New commit message", cx));
        let focus_handle = cx.focus_handle();
        window.focus(&focus_handle);
        Self { base, steps, selected: 0, message, focus_handle, error: None }
    }

    /// Saves the message field into the selected step (when it is a reword).
    fn store_message(&mut self, cx: &mut Context<Self>) {
        let text = self.message.read(cx).text().trim().to_string();
        if let Some(step) = self.steps.get_mut(self.selected)
            && step.action == RebaseAction::Reword
            && !text.is_empty()
        {
            step.message = text;
        }
    }

    fn select(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.store_message(cx);
        self.selected = ix;
        let message = self.steps.get(ix).map(|s| s.message.clone()).unwrap_or_default();
        self.message.update(cx, |f, cx| f.set_text(message, cx));
        cx.notify();
    }

    fn cycle(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.select(ix, cx);
        if let Some(step) = self.steps.get_mut(ix) {
            let pos = RebaseAction::ALL.iter().position(|a| *a == step.action).unwrap_or(0);
            step.action = RebaseAction::ALL[(pos + 1) % RebaseAction::ALL.len()];
        }
        cx.notify();
    }

    fn shift(&mut self, ix: usize, up: bool, cx: &mut Context<Self>) {
        let target = if up { ix.checked_sub(1) } else { (ix + 1 < self.steps.len()).then_some(ix + 1) };
        if let Some(target) = target {
            self.store_message(cx);
            self.steps.swap(ix, target);
            self.selected = target;
            cx.notify();
        }
    }

    fn confirm(&mut self, _: &Confirm, _: &mut Window, cx: &mut Context<Self>) {
        self.store_message(cx);
        // squash / fixup need a commit before them to fold into.
        let first_kept = self.steps.iter().find(|s| s.action != RebaseAction::Drop);
        if first_kept.is_some_and(|s| matches!(s.action, RebaseAction::Squash | RebaseAction::Fixup)) {
            self.error = Some("The first commit can't be squashed or fixed up into a previous one".into());
            cx.notify();
            return;
        }
        cx.emit(DialogEvent::Git(GitCommand::Rebase { base: self.base.clone(), steps: self.steps.clone() }));
    }

    fn cancel(&mut self, _: &Cancel, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(DialogEvent::Dismiss);
    }
}

impl Focusable for RebaseDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for RebaseDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = self.steps.iter().enumerate().map(|(ix, step)| {
            let color = match step.action {
                RebaseAction::Pick => theme::ui_text(),
                RebaseAction::Reword => theme::accent(),
                RebaseAction::Squash | RebaseAction::Fixup => theme::syntax(php_syntax::Highlight::Warning),
                RebaseAction::Drop => theme::error(),
            };
            let small = |id: (&'static str, usize), label: &'static str| div().id(id).px_1().rounded_sm().cursor_pointer().hover(|d| d.bg(theme::hover())).child(label);
            div()
                .id(("rebase-row", ix))
                .flex()
                .items_center()
                .gap_2()
                .px_2()
                .h(px(26.))
                .when(ix == self.selected, |d| d.bg(theme::selection()))
                .on_click(cx.listener(move |this, _, _, cx| this.select(ix, cx)))
                .child(
                    div()
                        .id(("rebase-action", ix))
                        .w(px(64.))
                        .px_1()
                        .rounded_sm()
                        .border_1()
                        .border_color(theme::border())
                        .cursor_pointer()
                        .text_color(color)
                        .child(step.action.name())
                        .on_click(cx.listener(move |this, _, _, cx| this.cycle(ix, cx))),
                )
                .child(div().w(px(64.)).text_color(theme::muted()).font_family(theme::editor_font()).child(step.hash[..step.hash.len().min(8)].to_string()))
                .child(
                    div()
                        .flex_1()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .when(step.action == RebaseAction::Drop, |d| d.line_through().text_color(theme::muted()))
                        .child(if step.action == RebaseAction::Reword { step.message.clone() } else { step.subject.clone() }),
                )
                .child(small(("rebase-up", ix), "↑").on_click(cx.listener(move |this, _, _, cx| this.shift(ix, true, cx))))
                .child(small(("rebase-down", ix), "↓").on_click(cx.listener(move |this, _, _, cx| this.shift(ix, false, cx))))
        });
        let reword = self.steps.get(self.selected).is_some_and(|s| s.action == RebaseAction::Reword);
        card(720.)
            .key_context("Dialog")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::cancel))
            .child(div().px_3().py_2().border_b_1().border_color(theme::border()).child(format!("Interactive Rebase onto {}", &self.base[..self.base.len().min(12)])))
            .child(div().px_3().pt_2().text_size(px(12.)).text_color(theme::muted()).child("Oldest first. Click an action to change it; ↑ / ↓ move a commit."))
            .child(div().id("rebase-list").max_h(px(420.)).overflow_y_scroll().py_1().children(rows))
            .when(reword, |d| d.child(div().px_3().py_1().child(self.message.clone())))
            .children(self.error.clone().map(|e| div().px_3().text_color(theme::error()).child(e)))
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .p_3()
                    .child(button("rebase-cancel", "Cancel", false).on_click(cx.listener(|this, _, window, cx| this.cancel(&Cancel, window, cx))))
                    .child(button("rebase-start", "Start Rebasing", true).on_click(cx.listener(|this, _, window, cx| this.confirm(&Confirm, window, cx)))),
            )
    }
}
