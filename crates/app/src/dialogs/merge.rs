//! Git ▸ Merge Conflicts: the three-pane merge window. Local on the left, the remote side on the
//! right and the result in the middle; a choice applies to one conflict and jumps to the next
//! unresolved one, so Apply & Close sends the whole file back to the editor.

use std::path::{Path, PathBuf};

use gpui::{AnyElement, App, Context, EventEmitter, FocusHandle, Focusable, Hsla, KeyDownEvent, Window, div, prelude::*, px};
use php_syntax::Highlight;

use super::{Cancel, Confirm, DialogEvent, button, card};
use crate::theme;

/// How one conflict is resolved: a side alone, both (local first or remote first).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Side {
    Local,
    Remote,
    Both,
    BothReversed,
}

impl Side {
    fn flags(self) -> (bool, bool, bool) {
        match self {
            Side::Local => (true, false, false),
            Side::Remote => (false, true, false),
            Side::Both => (true, true, false),
            Side::BothReversed => (true, true, true),
        }
    }
}

pub struct MergeDialog {
    path: PathBuf,
    text: String,
    conflicts: Vec<git::Conflict>,
    choices: Vec<Option<Side>>,
    selected: usize,
    focus_handle: FocusHandle,
}

impl EventEmitter<DialogEvent> for MergeDialog {}

impl MergeDialog {
    pub fn new(path: &Path, text: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let conflicts = git::conflicts(&text);
        let choices = vec![None; conflicts.len()];
        let focus_handle = cx.focus_handle();
        window.focus(&focus_handle);
        Self { path: path.to_path_buf(), text, conflicts, choices, selected: 0, focus_handle }
    }

    /// Number of conflicts still holding `<<<<<<<` markers.
    fn unresolved(&self) -> usize {
        self.choices.iter().filter(|c| c.is_none()).count()
    }

    fn choose(&mut self, side: Side, cx: &mut Context<Self>) {
        if self.selected >= self.conflicts.len() {
            return;
        }
        self.choices[self.selected] = Some(side);
        // Straight on to the next conflict that still needs a decision.
        let next = (self.selected + 1..self.conflicts.len())
            .chain(0..self.selected)
            .find(|ix| self.choices[*ix].is_none());
        if let Some(next) = next {
            self.selected = next;
        }
        cx.notify();
    }

    fn step(&mut self, delta: isize, cx: &mut Context<Self>) {
        let len = self.conflicts.len() as isize;
        if len == 0 {
            return;
        }
        self.selected = (self.selected as isize + delta).rem_euclid(len) as usize;
        cx.notify();
    }

    fn confirm(&mut self, _: &Confirm, _: &mut Window, cx: &mut Context<Self>) {
        let choices: Vec<Option<(bool, bool, bool)>> = self.choices.iter().map(|c| c.map(Side::flags)).collect();
        let (text, remaining) = git::resolve_all(&self.text, &choices);
        cx.emit(DialogEvent::MergeFile { path: self.path.clone(), text, remaining });
    }

    fn cancel(&mut self, _: &Cancel, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(DialogEvent::Dismiss);
    }

    fn on_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            "up" => self.step(-1, cx),
            "down" => self.step(1, cx),
            "left" => self.choose(Side::Local, cx),
            "right" => self.choose(Side::Remote, cx),
            "b" => self.choose(Side::Both, cx),
            "escape" => self.cancel(&Cancel, window, cx),
            "enter" => self.confirm(&Confirm, window, cx),
            _ => {}
        }
    }

    /// The text of the three panes for the selected conflict.
    fn panes(&self) -> (String, String, String) {
        let Some(conflict) = self.conflicts.get(self.selected) else { return (String::new(), String::new(), String::new()) };
        let ours = self.text[conflict.ours.clone()].to_string();
        let theirs = self.text[conflict.theirs.clone()].to_string();
        let result = match self.choices.get(self.selected).copied().flatten() {
            Some(side) => {
                let (o, t, tf) = side.flags();
                git::resolve(&self.text, conflict, o, t, tf)
            }
            None => String::new(),
        };
        (ours, result, theirs)
    }

    /// File line number of the first line of `offset`.
    fn line_number(&self, offset: usize) -> usize {
        1 + self.text.as_bytes()[..offset].iter().filter(|b| **b == b'\n').count()
    }

    fn pane_body(&self, text: &str, empty: &'static str, first: usize, color: Hsla) -> Vec<AnyElement> {
        if text.trim().is_empty() {
            return vec![div().p_2().text_color(theme::muted()).child(empty).into_any_element()];
        }
        text.lines()
            .enumerate()
            .map(|(ix, line)| {
                div()
                    .flex()
                    .gap_2()
                    .child(div().w(px(34.)).flex_none().text_color(theme::muted()).child(format!("{}", first + ix)))
                    .child(div().flex_1().whitespace_nowrap().text_color(color).child(line.to_string()))
                    .into_any_element()
            })
            .collect()
    }

    fn pane(title: String, color: Hsla) -> gpui::Div {
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .border_1()
            .border_color(theme::border())
            .rounded_sm()
            .overflow_hidden()
            .child(div().px_2().py_1().text_size(px(11.)).text_color(color).child(title))
    }
}

impl Focusable for MergeDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for MergeDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let name = self.path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let total = self.conflicts.len();
        let (ours, result, theirs) = self.panes();
        let current = self.conflicts.get(self.selected);
        let ours_line = current.map_or(1, |c| self.line_number(c.ours.start));
        let theirs_line = current.map_or(1, |c| self.line_number(c.theirs.start));
        let ours_label = current.map(|c| c.ours_label.clone()).unwrap_or_default();
        let theirs_label = current.map(|c| c.theirs_label.clone()).unwrap_or_default();
        let solved = current.is_some_and(|_| self.choices[self.selected].is_some());
        let unresolved = self.unresolved();

        let rail: Vec<AnyElement> = self
            .conflicts
            .iter()
            .enumerate()
            .map(|(ix, conflict)| {
                let done = self.choices[ix].is_some();
                div()
                    .id(("merge-conflict", ix))
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .h(px(22.))
                    .cursor_pointer()
                    .text_size(px(12.))
                    .when(ix == self.selected, |d| d.bg(theme::selection()))
                    .hover(|d| d.bg(theme::hover()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.selected = ix;
                        cx.notify();
                    }))
                    .child(div().w(px(12.)).text_color(if done { theme::success() } else { theme::accent() }).child(if done { "✓" } else { "●" }))
                    .child(div().flex_1().overflow_hidden().whitespace_nowrap().child(format!("{}. {}", ix + 1, conflict.ours_label)))
                    .into_any_element()
            })
            .collect();

        card(1180.)
            .key_context("Dialog")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::cancel))
            .on_key_down(cx.listener(Self::on_key))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(theme::border())
                    .child(div().flex_1().child(format!("Merge {name}")))
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(theme::muted())
                            .child(if total == 0 { "No conflicts".to_string() } else { format!("Conflict {} of {total}", self.selected + 1) }),
                    )
                    .child(
                        div()
                            .id("merge-prev")
                            .px_2()
                            .rounded_sm()
                            .cursor_pointer()
                            .hover(|d| d.bg(theme::hover()))
                            .child("‹")
                            .on_click(cx.listener(|this, _, _, cx| this.step(-1, cx))),
                    )
                    .child(
                        div()
                            .id("merge-next")
                            .px_2()
                            .rounded_sm()
                            .cursor_pointer()
                            .hover(|d| d.bg(theme::hover()))
                            .child("›")
                            .on_click(cx.listener(|this, _, _, cx| this.step(1, cx))),
                    ),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .p_3()
                    .child(div().id("merge-rail").w(px(150.)).flex_none().overflow_y_scroll().children(rail))
                    .child(
                        Self::pane(format!("LOCAL · {ours_label}"), theme::success())
                            .h(px(400.))
                            .child(
                                div()
                                    .id("merge-local")
                                    .flex_1()
                                    .overflow_y_scroll()
                                    .p_1()
                                    .font_family(theme::editor_font())
                                    .text_size(px(12.))
                                    .children(self.pane_body(&ours, "(deleted on this side)", ours_line, theme::text())),
                            )
                            .child(
                                div().p_1().child(
                                    button("merge-local-apply", "Use Local ◀", !solved)
                                        .on_click(cx.listener(|this, _, _, cx| this.choose(Side::Local, cx))),
                                ),
                            ),
                    )
                    .child(
                        Self::pane("RESULT".into(), theme::accent())
                            .h(px(400.))
                            .child(
                                div()
                                    .id("merge-result")
                                    .flex_1()
                                    .overflow_y_scroll()
                                    .p_1()
                                    .font_family(theme::editor_font())
                                    .text_size(px(12.))
                                    .children(if solved {
                                        self.pane_body(&result, "(removed)", ours_line, theme::ui_text())
                                    } else {
                                        vec![div().p_2().text_color(theme::muted()).child("Choose Local, Both or Remote").into_any_element()]
                                    }),
                            )
                            .child(
                                div()
                                    .flex()
                                    .gap_1()
                                    .p_1()
                                    .child(
                                        button("merge-both", "Both", false)
                                            .text_size(px(12.))
                                            .on_click(cx.listener(|this, _, _, cx| this.choose(Side::Both, cx))),
                                    )
                                    .child(
                                        button("merge-both-reversed", "Both, remote first", false)
                                            .text_size(px(12.))
                                            .on_click(cx.listener(|this, _, _, cx| this.choose(Side::BothReversed, cx))),
                                    ),
                            ),
                    )
                    .child(
                        Self::pane(format!("REMOTE · {theirs_label}"), theme::syntax(Highlight::Warning))
                            .h(px(400.))
                            .child(
                                div()
                                    .id("merge-remote")
                                    .flex_1()
                                    .overflow_y_scroll()
                                    .p_1()
                                    .font_family(theme::editor_font())
                                    .text_size(px(12.))
                                    .children(self.pane_body(&theirs, "(deleted on that side)", theirs_line, theme::text())),
                            )
                            .child(
                                div().p_1().child(
                                    button("merge-remote-apply", "Use Remote ▶", !solved)
                                        .on_click(cx.listener(|this, _, _, cx| this.choose(Side::Remote, cx))),
                                ),
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .pb_2()
                    .text_size(px(12.))
                    .text_color(theme::muted())
                    .child("← / → use a side · ↑ / ↓ walk the conflicts · B both · Esc cancel")
                    .child(
                        div()
                            .flex_1()
                            .text_right()
                            .when(unresolved > 0, |d| d.text_color(theme::syntax(Highlight::Warning)))
                            .child(if unresolved == 0 {
                                "Every conflict has a decision".to_string()
                            } else {
                                format!("{unresolved} conflict(s) keep their markers")
                            }),
                    ),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .p_3()
                    .child(button("merge-cancel", "Cancel", false).on_click(cx.listener(|this, _, window, cx| this.cancel(&Cancel, window, cx))))
                    .child(button("merge-apply", "Apply & Close", true).on_click(cx.listener(|this, _, window, cx| this.confirm(&Confirm, window, cx)))),
            )
    }
}
