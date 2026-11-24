//! Choosing one of the project's SFTP servers. A project can deploy to several hosts, so
//! "Upload to Server…" asks which one instead of assuming the default; the same list picks
//! the download source and the server automatic upload uses.

use gpui::{App, Context, EventEmitter, FocusHandle, Focusable, SharedString, Window, div, prelude::*, px};

use super::{Cancel, Confirm, DialogEvent, button, card};
use crate::deployment::Server;
use crate::i18n::tr;
use crate::theme;

pub struct ServersDialog {
    /// `(server, what to do with it)`: upload when true, download when false.
    servers: Vec<Server>,
    upload: bool,
    default_id: Option<String>,
    selected: usize,
    focus_handle: FocusHandle,
}

impl EventEmitter<DialogEvent> for ServersDialog {}

impl ServersDialog {
    pub fn new(servers: Vec<Server>, upload: bool, default: Option<String>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        window.focus(&focus_handle);
        let selected = default.as_ref().and_then(|id| servers.iter().position(|s| &s.id == id)).unwrap_or(0);
        Self { servers, upload, default_id: default, selected, focus_handle }
    }

    fn title(&self) -> SharedString {
        if self.upload { tr("Upload to Server") } else { tr("Download from Server") }
    }

    fn confirm(&mut self, _: &Confirm, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(server) = self.servers.get(self.selected) {
            cx.emit(DialogEvent::ServerPicked { id: server.id.clone(), upload: self.upload });
        }
    }

    fn make_default(&mut self, _: &Confirm, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(server) = self.servers.get(self.selected) {
            cx.emit(DialogEvent::DefaultServer(server.id.clone()));
        }
    }

    fn cancel(&mut self, _: &Cancel, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(DialogEvent::Dismiss);
    }

    fn step(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.servers.is_empty() {
            return;
        }
        let len = self.servers.len() as isize;
        self.selected = (((self.selected as isize + delta) % len + len) % len) as usize;
        cx.notify();
    }
}

impl Focusable for ServersDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ServersDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rows: Vec<gpui::AnyElement> = self
            .servers
            .iter()
            .enumerate()
            .map(|(ix, server)| {
                let is_default = self.default_id.as_deref() == Some(server.id.as_str());
                let detail = format!("{}@{}:{}  ·  {}", server.user, server.host, server.port, server.remote_root);
                div()
                    .id(("server", ix))
                    .mx_2()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .cursor_pointer()
                    .when(ix == self.selected, |d| d.bg(theme::selection()))
                    .hover(|d| d.bg(theme::hover()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.selected = ix;
                        cx.notify();
                    }))
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .items_center()
                            .gap_3()
                            .child(div().child(server.name.clone()))
                            .when(is_default, |d| {
                                d.child(div().text_size(px(11.)).text_color(theme::accent()).child(tr("default")))
                            }),
                    )
                    .child(div().text_size(px(11.5)).text_color(theme::muted()).child(detail))
                    .into_any_element()
            })
            .collect();
        let empty = rows.is_empty();
        let go = if self.upload { button("servers-go", "Upload", true) } else { button("servers-go", "Download", true) };
        let go = go.on_click(cx.listener(|this, _, window, cx| this.confirm(&Confirm, window, cx)));
        let hint = if self.upload {
            tr("↵ uploads the current file to the selected server.")
        } else {
            tr("↵ downloads the current file from the selected server.")
        };
        card(520.)
            .key_context("Dialog ServersDialog")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::make_default))
            .on_action(cx.listener(Self::step_up))
            .on_action(cx.listener(Self::step_down))
            .on_action(cx.listener(Self::cancel))
            .child(div().px_3().py_2().border_b_1().border_color(theme::border()).child(self.title()))
            .child(
                div()
                    .id("servers-list")
                    .max_h(px(300.))
                    .overflow_y_scroll()
                    .py_1()
                    .when(empty, |d| d.child(div().px_3().py_2().text_color(theme::muted()).child(tr("No SFTP servers yet."))))
                    .children(rows),
            )
            .child(div().px_3().py_2().text_size(px(11.5)).text_color(theme::muted()).child(hint))
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .px_3()
                    .pb_3()
                    .child(button("servers-cancel", "Cancel", false).on_click(cx.listener(|this, _, window, cx| this.cancel(&Cancel, window, cx))))
                    .child(button("servers-default", "Make Default", false).on_click(cx.listener(|this, _, window, cx| this.make_default(&Confirm, window, cx))))
                    .child(go),
            )
    }
}

impl ServersDialog {
    fn step_up(&mut self, _: &super::SelectPrev, _: &mut Window, cx: &mut Context<Self>) {
        self.step(-1, cx)
    }

    fn step_down(&mut self, _: &super::SelectNext, _: &mut Window, cx: &mut Context<Self>) {
        self.step(1, cx)
    }
}
