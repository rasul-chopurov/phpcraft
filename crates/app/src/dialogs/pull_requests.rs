//! Git ▸ Pull Requests…: open pull / merge requests on the left, the selected one's description,
//! comments and files on the right, with Checkout / Show Diff / Open in Browser / Add Comment.

use std::collections::HashMap;
use std::path::PathBuf;

use gpui::{App, Context, Entity, EventEmitter, FocusHandle, Focusable, Window, div, prelude::*, px};

use super::{Cancel, DialogEvent, GitCommand, button, card};
use crate::pull_requests::{self as prs, Details, Host, PullRequest};
use crate::text_field::TextField;
use crate::theme;

pub struct PullRequestsDialog {
    root: PathBuf,
    host: Host,
    /// `None` while loading.
    list: Option<Result<Vec<PullRequest>, String>>,
    selected: usize,
    details: HashMap<u64, Result<Details, String>>,
    comment: Entity<TextField>,
    status: Option<String>,
    focus_handle: FocusHandle,
}

impl EventEmitter<DialogEvent> for PullRequestsDialog {}

impl PullRequestsDialog {
    pub fn new(root: PathBuf, host: Host, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        window.focus(&focus_handle);
        let comment = cx.new(|cx| TextField::new("Add a comment…", cx));
        let mut this = Self { root, host, list: None, selected: 0, details: HashMap::new(), comment, status: None, focus_handle };
        this.reload(cx);
        this
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        self.list = None;
        self.details.clear();
        let (root, host) = (self.root.clone(), self.host);
        cx.spawn(async move |this, cx| {
            let list = cx.background_spawn(async move { prs::list(&root, host) }).await;
            this.update(cx, |this, cx| {
                this.list = Some(list);
                this.select(0, cx);
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn current(&self) -> Option<&PullRequest> {
        self.list.as_ref()?.as_ref().ok()?.get(self.selected)
    }

    fn select(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.selected = ix;
        self.status = None;
        if let Some(number) = self.current().map(|p| p.number)
            && !self.details.contains_key(&number)
        {
            self.load_details(number, cx);
        }
        cx.notify();
    }

    fn load_details(&mut self, number: u64, cx: &mut Context<Self>) {
        let (root, host) = (self.root.clone(), self.host);
        cx.spawn(async move |this, cx| {
            let details = cx.background_spawn(async move { prs::details(&root, host, number) }).await;
            this.update(cx, |this, cx| {
                this.details.insert(number, details);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn add_comment(&mut self, cx: &mut Context<Self>) {
        let body = self.comment.read(cx).text().trim().to_string();
        let Some(number) = self.current().map(|p| p.number) else { return };
        if body.is_empty() {
            return;
        }
        self.status = Some("Posting the comment…".into());
        let (root, host) = (self.root.clone(), self.host);
        cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move { prs::comment(&root, host, number, &body) }).await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(_) => {
                        this.status = None;
                        this.comment.update(cx, |f, cx| f.set_text("", cx));
                        this.load_details(number, cx);
                    }
                    Err(err) => this.status = Some(err),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn show_diff(&mut self, file: Option<String>, cx: &mut Context<Self>) {
        let Some(pr) = self.current() else { return };
        cx.emit(DialogEvent::PullRequestDiff { host: self.host, number: pr.number, base: pr.base.clone(), file });
    }

    fn cancel(&mut self, _: &Cancel, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(DialogEvent::Dismiss);
    }

    fn render_details(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let Some(pr) = self.current() else {
            return div().p_3().text_color(theme::muted()).child("Nothing selected.").into_any_element();
        };
        let heading = |text: &'static str| div().mt_3().mb_1().text_size(px(11.5)).text_color(theme::muted()).child(text);
        let mut body = div()
            .id("pr-details")
            .flex_1()
            .overflow_y_scroll()
            .px_3()
            .py_2()
            .child(div().text_size(px(14.)).child(format!("#{} {}", pr.number, pr.title)))
            .child(div().text_size(px(11.5)).text_color(theme::muted()).child(format!("{} wants to merge {} into {}{}", pr.author, pr.head, pr.base, if pr.draft { " · draft" } else { "" })));
        match self.details.get(&pr.number) {
            None => body = body.child(div().mt_3().text_color(theme::muted()).child("Loading…")),
            Some(Err(err)) => body = body.child(div().mt_3().text_color(theme::error()).child(err.clone())),
            Some(Ok(details)) => {
                let text = details.body.trim();
                body = body.child(heading("Description")).child(div().text_color(if text.is_empty() { theme::muted() } else { theme::ui_text() }).child(if text.is_empty() { "No description.".to_string() } else { text.to_string() }));
                if !details.files.is_empty() {
                    body = body.child(heading("Files")).children(details.files.iter().enumerate().map(|(ix, file)| {
                        let file2 = file.clone();
                        div()
                            .id(("pr-file", ix))
                            .px_1()
                            .rounded_sm()
                            .text_color(theme::accent())
                            .cursor_pointer()
                            .hover(|d| d.bg(theme::hover()))
                            .child(file.clone())
                            .on_click(cx.listener(move |this, _, _, cx| this.show_diff(Some(file2.clone()), cx)))
                    }));
                }
                if !details.comments.is_empty() {
                    body = body.child(heading("Comments")).children(details.comments.iter().map(|(author, text)| {
                        div().mb_2().child(div().text_size(px(11.5)).text_color(theme::muted()).child(author.clone())).child(text.trim().to_string())
                    }));
                }
            }
        }
        let url = pr.url.clone();
        let (host, number) = (self.host, pr.number);
        div()
            .flex_1()
            .flex()
            .flex_col()
            .min_w(px(0.))
            .child(body)
            .child(
                div()
                    .flex()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .border_t_1()
                    .border_color(theme::border())
                    .child(button("pr-checkout", "Checkout", false).on_click(cx.listener(move |_, _, _, cx| cx.emit(DialogEvent::Git(GitCommand::CheckoutPullRequest { host, number })))))
                    .child(button("pr-diff", "Show Diff", false).on_click(cx.listener(|this, _, _, cx| this.show_diff(None, cx))))
                    .child(button("pr-browser", "Open in Browser", false).on_click(move |_, _, cx| cx.open_url(&url))),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .pb_2()
                    .child(div().flex_1().child(self.comment.clone()))
                    .child(button("pr-comment", "Add Comment", false).on_click(cx.listener(|this, _, _, cx| this.add_comment(cx)))),
            )
            .into_any_element()
    }
}

impl Focusable for PullRequestsDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for PullRequestsDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let title = match self.host {
            Host::GitHub => "Pull Requests",
            Host::GitLab => "Merge Requests",
        };
        let list: gpui::AnyElement = match &self.list {
            None => div().p_3().text_color(theme::muted()).child("Loading…").into_any_element(),
            Some(Err(err)) => div().p_3().text_color(theme::error()).child(err.clone()).into_any_element(),
            Some(Ok(items)) if items.is_empty() => div().p_3().text_color(theme::muted()).child("No open requests.").into_any_element(),
            Some(Ok(items)) => div()
                .id("pr-list")
                .size_full()
                .overflow_y_scroll()
                .py_1()
                .children(items.iter().enumerate().map(|(ix, pr)| {
                    div()
                        .id(("pr", ix))
                        .px_3()
                        .py_1()
                        .cursor_pointer()
                        .when(ix == self.selected, |d| d.bg(theme::selection()))
                        .hover(|d| d.bg(theme::hover()))
                        .child(div().overflow_hidden().whitespace_nowrap().text_ellipsis().child(format!("#{} {}", pr.number, pr.title)))
                        .child(div().text_size(px(11.5)).text_color(theme::muted()).child(format!("{} · {}{}", pr.author, pr.head, if pr.draft { " · draft" } else { "" })))
                        .on_click(cx.listener(move |this, _, _, cx| this.select(ix, cx)))
                }))
                .into_any_element(),
        };
        let details = self.render_details(cx);
        card(900.)
            .key_context("Dialog")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::cancel))
            .child(div().px_3().py_2().border_b_1().border_color(theme::border()).child(title))
            .child(div().flex().h(px(460.)).child(div().w(px(320.)).flex_none().border_r_1().border_color(theme::border()).child(list)).child(details))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .p_3()
                    .border_t_1()
                    .border_color(theme::border())
                    .child(button("pr-refresh", "Refresh", false).on_click(cx.listener(|this, _, _, cx| this.reload(cx))))
                    .child(div().flex_1().overflow_hidden().text_color(theme::muted()).children(self.status.clone()))
                    .child(button("pr-close", "Close", true).on_click(cx.listener(|this, _, window, cx| this.cancel(&Cancel, window, cx)))),
            )
    }
}
