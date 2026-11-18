//! View ▸ Window Layouts…: saved arrangements of the tool windows — apply one, delete one, or
//! name the current arrangement so it can be brought back later.

use gpui::{App, Context, Entity, EventEmitter, FocusHandle, Focusable, Window, div, prelude::*, px};

use super::{Cancel, Confirm, DialogEvent, button, card};
use crate::i18n::tr;
use crate::text_field::TextField;
use crate::theme;

pub struct LayoutsDialog {
    /// `(name, one-line description of the arrangement)`.
    rows: Vec<(String, String)>,
    selected: usize,
    name: Entity<TextField>,
    focus_handle: FocusHandle,
}

impl EventEmitter<DialogEvent> for LayoutsDialog {}

impl LayoutsDialog {
    pub fn new(rows: Vec<(String, String)>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let name = cx.new(|cx| TextField::new(tr("Name for this layout"), cx));
        let focus_handle = cx.focus_handle();
        window.focus(&name.focus_handle(cx));
        Self { rows, selected: 0, name, focus_handle }
    }

    fn typed_name(&self, cx: &App) -> String {
        self.name.read(cx).text().trim().to_string()
    }

    /// Enter applies the selected layout; with no layouts saved yet it saves the current one.
    fn confirm(&mut self, _: &Confirm, _: &mut Window, cx: &mut Context<Self>) {
        match self.rows.get(self.selected) {
            Some((name, _)) => cx.emit(DialogEvent::ApplyWindowLayout(name.clone())),
            None => {
                let name = self.typed_name(cx);
                if !name.is_empty() {
                    cx.emit(DialogEvent::SaveWindowLayout(name));
                }
            }
        }
    }

    fn save(&mut self, _: &Confirm, _: &mut Window, cx: &mut Context<Self>) {
        // An empty field updates the selected layout; otherwise a new name is created.
        let name = match self.typed_name(cx) {
            typed if typed.is_empty() => self.rows.get(self.selected).map(|(name, _)| name.clone()).unwrap_or_default(),
            typed => typed,
        };
        if !name.is_empty() {
            cx.emit(DialogEvent::SaveWindowLayout(name));
        }
    }

    fn cancel(&mut self, _: &Cancel, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(DialogEvent::Dismiss);
    }
}

impl Focusable for LayoutsDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for LayoutsDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rows: Vec<gpui::AnyElement> = self
            .rows
            .iter()
            .enumerate()
            .map(|(ix, (name, detail))| {
                let delete_name = name.clone();
                div()
                    .id(("layout", ix))
                    .mx_2()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .cursor_pointer()
                    .when(ix == self.selected, |d| d.bg(theme::selection()))
                    .hover(|d| d.bg(theme::hover()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.selected = ix;
                        this.name.update(cx, |field, cx| field.set_text(String::new(), cx));
                        cx.notify();
                    }))
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .items_center()
                            .gap_3()
                            .child(div().child(name.clone()))
                            .child(
                                div()
                                    .id(("layout-delete", ix))
                                    .text_size(px(11.5))
                                    .text_color(theme::muted())
                                    .hover(|d| d.text_color(theme::error()))
                                    .child(tr("Delete"))
                                    .on_click(cx.listener(move |_, _, _, cx| {
                                        cx.emit(DialogEvent::DeleteWindowLayout(delete_name.clone()));
                                    })),
                            ),
                    )
                    .child(div().text_size(px(11.5)).text_color(theme::muted()).child(detail.clone()))
                    .into_any_element()
            })
            .collect();
        let empty = rows.is_empty();
        card(560.)
            .key_context("Dialog")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::cancel))
            .child(div().px_3().py_2().border_b_1().border_color(theme::border()).child(tr("Window Layouts")))
            .child(
                div()
                    .id("layouts-list")
                    .max_h(px(300.))
                    .overflow_y_scroll()
                    .py_1()
                    .when(empty, |d| d.child(div().px_3().py_2().text_color(theme::muted()).child(tr("No saved layouts yet."))))
                    .children(rows),
            )
            .child(
                div()
                    .p_3()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .border_t_1()
                    .border_color(theme::border())
                    .child(self.name.clone())
                    .child(div().text_size(px(11.5)).text_color(theme::muted()).child(tr(
                        "Leave the name empty to update the selected layout. ↵ applies the selected one.",
                    ))),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .px_3()
                    .pb_3()
                    .child(button("layouts-cancel", "Cancel", false).on_click(cx.listener(|this, _, window, cx| this.cancel(&Cancel, window, cx))))
                    .child(button("layouts-save", "Save Current", false).on_click(cx.listener(|this, _, window, cx| this.save(&Confirm, window, cx))))
                    .child(button("layouts-apply", "Apply", true).on_click(cx.listener(|this, _, window, cx| this.confirm(&Confirm, window, cx)))),
            )
    }
}
