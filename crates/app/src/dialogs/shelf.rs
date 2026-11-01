//! Git ▸ Shelve Changes… (pick files and a name) and Git ▸ Shelf (unshelve or delete).

use std::collections::BTreeSet;

use gpui::{App, Context, Entity, EventEmitter, FocusHandle, Focusable, Window, div, prelude::*, px};

use super::{Cancel, Confirm, DialogEvent, button, card, checkbox};
use crate::text_field::TextField;
use crate::theme;

pub enum ShelfMode {
    /// Changed files to choose from.
    Shelve(Vec<String>),
    /// Existing shelves.
    Browse(Vec<git::Shelf>),
}

pub struct ShelfDialog {
    mode: ShelfMode,
    chosen: BTreeSet<usize>,
    selected: usize,
    name: Entity<TextField>,
    focus_handle: FocusHandle,
}

impl EventEmitter<DialogEvent> for ShelfDialog {}

impl ShelfDialog {
    pub fn new(mode: ShelfMode, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let name = cx.new(|cx| {
            let mut f = TextField::new("Shelf name", cx);
            f.set_text("Work in progress", cx);
            f
        });
        let chosen = match &mode {
            ShelfMode::Shelve(files) => (0..files.len()).collect(),
            ShelfMode::Browse(_) => BTreeSet::new(),
        };
        let focus_handle = cx.focus_handle();
        match &mode {
            ShelfMode::Shelve(_) => window.focus(&name.focus_handle(cx)),
            ShelfMode::Browse(_) => window.focus(&focus_handle),
        }
        Self { mode, chosen, selected: 0, name, focus_handle }
    }

    fn confirm(&mut self, _: &Confirm, _: &mut Window, cx: &mut Context<Self>) {
        match &self.mode {
            ShelfMode::Shelve(files) => {
                let files: Vec<String> = self.chosen.iter().filter_map(|i| files.get(*i).cloned()).collect();
                let name = self.name.read(cx).text().trim().to_string();
                if !files.is_empty() {
                    cx.emit(DialogEvent::Shelve { name: if name.is_empty() { "Shelved changes".into() } else { name }, files });
                }
            }
            ShelfMode::Browse(shelves) => {
                if let Some(shelf) = shelves.get(self.selected) {
                    cx.emit(DialogEvent::Unshelve { id: shelf.id.clone(), delete_only: false });
                }
            }
        }
    }

    fn cancel(&mut self, _: &Cancel, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(DialogEvent::Dismiss);
    }
}

impl Focusable for ShelfDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ShelfDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (title, ok, body): (&str, &'static str, Vec<gpui::AnyElement>) = match &self.mode {
            ShelfMode::Shelve(files) => (
                "Shelve Changes",
                "Shelve",
                files
                    .iter()
                    .enumerate()
                    .map(|(ix, file)| {
                        checkbox(format!("shelf-file-{ix}"), file.clone(), self.chosen.contains(&ix))
                            .px_3()
                            .py_0p5()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if !this.chosen.remove(&ix) {
                                    this.chosen.insert(ix);
                                }
                                cx.notify();
                            }))
                            .into_any_element()
                    })
                    .collect(),
            ),
            ShelfMode::Browse(shelves) => (
                "Shelf",
                "Unshelve",
                shelves
                    .iter()
                    .enumerate()
                    .map(|(ix, shelf)| {
                        let id = shelf.id.clone();
                        div()
                            .id(("shelf", ix))
                            .px_3()
                            .py_1()
                            .cursor_pointer()
                            .when(ix == self.selected, |d| d.bg(theme::selection()))
                            .hover(|d| d.bg(theme::hover()))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.selected = ix;
                                cx.notify();
                            }))
                            .child(div().flex().justify_between().child(shelf.name.clone()).child(
                                div()
                                    .id(("shelf-delete", ix))
                                    .text_color(theme::muted())
                                    .hover(|d| d.text_color(theme::error()))
                                    .child("Delete")
                                    .on_click(cx.listener(move |_, _, _, cx| cx.emit(DialogEvent::Unshelve { id: id.clone(), delete_only: true }))),
                            ))
                            .child(div().text_size(px(11.5)).text_color(theme::muted()).child(shelf.files.join(", ")))
                            .into_any_element()
                    })
                    .collect(),
            ),
        };
        let empty = body.is_empty();
        card(560.)
            .key_context("Dialog")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::cancel))
            .child(div().px_3().py_2().border_b_1().border_color(theme::border()).child(title))
            .when(matches!(self.mode, ShelfMode::Shelve(_)), |d| d.child(div().p_3().child(self.name.clone())))
            .child(div().id("shelf-list").max_h(px(360.)).overflow_y_scroll().py_1().children(body))
            .when(empty, |d| d.child(div().px_3().py_2().text_color(theme::muted()).child("Nothing here.")))
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .p_3()
                    .child(button("shelf-cancel", "Cancel", false).on_click(cx.listener(|this, _, window, cx| this.cancel(&Cancel, window, cx))))
                    .child(button("shelf-ok", ok, true).on_click(cx.listener(|this, _, window, cx| this.confirm(&Confirm, window, cx)))),
            )
    }
}
