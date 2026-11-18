//! A yes/no question about something the workspace noticed on its own (a file changed under an
//! unsaved buffer, a file deleted on disk). The caller's `id` travels back with the answer, so
//! the dialog itself keeps no state beyond the labels.

use gpui::{App, Context, EventEmitter, FocusHandle, Focusable, Window, div, prelude::*, px};

use super::{Cancel, Confirm, DialogEvent, button, card};
use crate::i18n::tr;
use crate::theme;

pub struct ConfirmPrompt {
    id: String,
    title: String,
    message: String,
    confirm_label: &'static str,
    focus_handle: FocusHandle,
}

impl EventEmitter<DialogEvent> for ConfirmPrompt {}

impl ConfirmPrompt {
    pub fn new(
        id: impl Into<String>,
        title: impl Into<String>,
        message: impl Into<String>,
        confirm_label: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();
        window.focus(&focus_handle);
        Self { id: id.into(), title: title.into(), message: message.into(), confirm_label, focus_handle }
    }

    fn confirm(&mut self, _: &Confirm, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(DialogEvent::Confirmed(self.id.clone()));
    }

    fn cancel(&mut self, _: &Cancel, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(DialogEvent::Dismiss);
    }
}

impl Focusable for ConfirmPrompt {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ConfirmPrompt {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        card(440.)
            .key_context("Dialog")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::cancel))
            .p_4()
            .flex()
            .flex_col()
            .gap_3()
            .child(div().text_color(theme::ui_text()).child(tr(&self.title)))
            .child(div().text_size(px(12.)).text_color(theme::muted()).child(tr(&self.message)))
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .child(button("confirm-prompt-cancel", "Cancel", false).on_click(cx.listener(|this, _, window, cx| this.cancel(&Cancel, window, cx))))
                    .child(button("confirm-prompt-ok", self.confirm_label, true).on_click(cx.listener(|this, _, window, cx| this.confirm(&Confirm, window, cx)))),
            )
    }
}
