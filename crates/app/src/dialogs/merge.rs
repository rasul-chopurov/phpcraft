//! Git ▸ Merge Conflicts: the three-pane merge window. Local on the left, the remote side on the
//! right and the result in the middle, with the common ancestor (BASE) in between when the file
//! carries `|||||||` sections. A choice applies to one conflict and jumps to the next unresolved
//! one, so Apply & Close sends the whole file back to the editor.

use std::ops::Range;
use std::path::{Path, PathBuf};

use gpui::{AnyElement, App, Context, EventEmitter, FocusHandle, Focusable, Hsla, KeyDownEvent, SharedString, Window, StyledText, div, prelude::*, px};
use php_syntax::Highlight;

use super::{Cancel, Confirm, DialogEvent, button, card};
use crate::i18n::{tr, trf};
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

/// What replaces one whole conflict block, and which button produced it (`None` when the two
/// sides were merged line by line from the common ancestor).
#[derive(Clone)]
struct Decision {
    side: Option<Side>,
    text: String,
}

pub struct MergeDialog {
    path: PathBuf,
    text: String,
    /// Whole-file highlighting: a conflict's text is only valid in the context of the file.
    spans: Vec<php_syntax::Span>,
    conflicts: Vec<git::Conflict>,
    choices: Vec<Option<Decision>>,
    selected: usize,
    focus_handle: FocusHandle,
}

impl EventEmitter<DialogEvent> for MergeDialog {}

impl MergeDialog {
    pub fn new(path: &Path, text: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let conflicts = git::conflicts(&text);
        // Non-overlapping edits inside one conflict block need no decision from anybody.
        let choices = conflicts.iter().map(|conflict| auto(&text, conflict)).collect();
        let spans = php_syntax::highlight_path(path, &text);
        let focus_handle = cx.focus_handle();
        window.focus(&focus_handle);
        Self { path: path.to_path_buf(), text, spans, conflicts, choices, selected: 0, focus_handle }
    }

    /// Number of conflicts still holding `<<<<<<<` markers.
    fn unresolved(&self) -> usize {
        self.choices.iter().filter(|c| c.is_none()).count()
    }

    fn choose(&mut self, side: Side, cx: &mut Context<Self>) {
        let Some(conflict) = self.conflicts.get(self.selected) else { return };
        let (ours, theirs, theirs_first) = side.flags();
        let text = git::resolve(&self.text, conflict, ours, theirs, theirs_first);
        self.choices[self.selected] = Some(Decision { side: Some(side), text });
        self.next_unresolved(cx);
    }

    /// Decides every conflict whose two sides touched different lines of the ancestor.
    fn auto_merge_rest(&mut self, cx: &mut Context<Self>) {
        for ix in 0..self.conflicts.len() {
            if self.choices[ix].is_none() {
                self.choices[ix] = auto(&self.text, &self.conflicts[ix]);
            }
        }
        self.next_unresolved(cx);
    }

    fn next_unresolved(&mut self, cx: &mut Context<Self>) {
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

    fn clear(&mut self, cx: &mut Context<Self>) {
        if self.selected < self.choices.len() {
            self.choices[self.selected] = None;
            cx.notify();
        }
    }

    fn confirm(&mut self, _: &Confirm, _: &mut Window, cx: &mut Context<Self>) {
        let texts: Vec<Option<String>> = self.choices.iter().map(|c| c.as_ref().map(|d| d.text.clone())).collect();
        let (text, remaining) = git::resolve_all_texts(&self.text, &texts);
        cx.emit(DialogEvent::MergeFile { path: self.path.clone(), text, remaining });
    }

    fn cancel(&mut self, _: &Cancel, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(DialogEvent::Dismiss);
    }

    fn on_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            "up" | "p" | "k" => self.step(-1, cx),
            "down" | "n" | "j" => self.step(1, cx),
            "left" => self.choose(Side::Local, cx),
            "right" => self.choose(Side::Remote, cx),
            "b" => self.choose(Side::Both, cx),
            "a" => self.auto_merge_rest(cx),
            "u" => self.clear(cx),
            "escape" => self.cancel(&Cancel, window, cx),
            "enter" => self.confirm(&Confirm, window, cx),
            _ => {}
        }
    }

    /// File line number of the first line of `offset`.
    fn line_number(&self, offset: usize) -> usize {
        1 + self.text.as_bytes()[..offset].iter().filter(|b| **b == b'\n').count()
    }

    /// Highlights of one line of `len` bytes, taken from the whole-file spans and made relative to
    /// the line, so panes show the same colours as the editor.
    fn line_highlights(&self, start: usize, len: usize) -> Vec<(Range<usize>, gpui::HighlightStyle)> {
        let end = start + len;
        self.spans
            .iter()
            .filter_map(|span| {
                let from = span.range.start.max(start);
                let to = span.range.end.min(end);
                (from < to).then(|| (from - start..to - start, gpui::HighlightStyle { color: Some(theme::syntax(span.highlight)), ..Default::default() }))
            })
            .collect()
    }

    /// Rows of `text`: `origin` is its first byte in the file, so highlights stay in place. A
    /// synthesized result (both sides, or merged lines) has no single origin and stays plain.
    fn pane_body(&self, text: &str, origin: Option<usize>, empty: &str, color: Hsla) -> Vec<AnyElement> {
        if text.trim().is_empty() {
            return vec![div().p_2().text_color(theme::muted()).child(tr(empty)).into_any_element()];
        }
        let first = origin.map_or(1, |o| self.line_number(o));
        let mut offset = 0;
        text.split_inclusive('\n')
            .enumerate()
            .map(|(ix, raw)| {
                let shown = raw.trim_end_matches(['\r', '\n']);
                let highlights = origin.map_or_else(Vec::new, |o| self.line_highlights(o + offset, shown.len()));
                offset += raw.len();
                div()
                    .flex()
                    .gap_2()
                    .child(div().w(px(34.)).flex_none().text_color(theme::muted()).child(format!("{}", first + ix)))
                    .child(
                        div()
                            .flex_1()
                            .whitespace_nowrap()
                            .text_color(color)
                            .child(StyledText::new(SharedString::from(shown.replace('\t', " "))).with_highlights(highlights)),
                    )
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

    fn scroller(&self, id: &'static str, body: Vec<AnyElement>) -> gpui::Stateful<gpui::Div> {
        div()
            .id(id)
            .flex_1()
            .overflow_y_scroll()
            .p_1()
            .font_family(theme::editor_font())
            .text_size(px(12.))
            .children(body)
    }
}

/// The line-level merge of one conflict straight from its common ancestor.
fn auto(text: &str, conflict: &git::Conflict) -> Option<Decision> {
    let base = conflict.base.as_ref()?;
    let merged = git::auto_merge(&text[base.clone()], &text[conflict.ours.clone()], &text[conflict.theirs.clone()])?;
    Some(Decision { side: None, text: merged })
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
        let conflict = self.conflicts.get(self.selected).cloned();
        let decision = self.choices.get(self.selected).and_then(|c| c.clone());
        let chosen = decision.as_ref().and_then(|d| d.side);
        let ours_label = conflict.as_ref().map(|c| c.ours_label.clone()).unwrap_or_default();
        let theirs_label = conflict.as_ref().map(|c| c.theirs_label.clone()).unwrap_or_default();
        let unresolved = self.unresolved();
        let auto = self.choices.iter().filter(|c| c.as_ref().is_some_and(|d| d.side.is_none())).count();

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

        let local = Self::pane(trf("LOCAL · {}", &[&ours_label]), theme::success())
            .h(px(400.))
            .child(match &conflict {
                Some(c) => self.scroller("merge-local", self.pane_body(&self.text[c.ours.clone()], Some(c.ours.start), "(deleted on this side)", theme::text())),
                None => self.scroller("merge-local", Vec::new()),
            })
            .child(
                div().p_1().child(
                    button("merge-local-apply", "Use Local ◀", chosen == Some(Side::Local))
                        .on_click(cx.listener(|this, _, _, cx| this.choose(Side::Local, cx))),
                ),
            );

        // The common ancestor only exists in diff3-style markers; without it there is no BASE pane.
        let base = conflict.as_ref().and_then(|c| c.base.clone()).map(|range| {
            Self::pane(tr("BASE").to_string(), theme::muted())
                .h(px(400.))
                .child(self.scroller("merge-base", self.pane_body(&self.text[range.clone()], Some(range.start), "(removed)", theme::text())))
        });

        let result = Self::pane(tr("RESULT").to_string(), theme::accent())
            .h(px(400.))
            .child(match &decision {
                Some(decision) => self.scroller(
                    "merge-result",
                    self.pane_body(&decision.text, result_origin(&conflict, decision), "(removed)", theme::ui_text()),
                ),
                None => self.scroller(
                    "merge-result",
                    vec![div().p_2().text_color(theme::muted()).child(tr("Choose Local, Both or Remote")).into_any_element()],
                ),
            })
            .child(
                div()
                    .flex()
                    .gap_1()
                    .p_1()
                    .child(
                        button("merge-both", "Both", chosen == Some(Side::Both))
                            .text_size(px(12.))
                            .on_click(cx.listener(|this, _, _, cx| this.choose(Side::Both, cx))),
                    )
                    .child(
                        button("merge-both-reversed", "Both, remote first", chosen == Some(Side::BothReversed))
                            .text_size(px(12.))
                            .on_click(cx.listener(|this, _, _, cx| this.choose(Side::BothReversed, cx))),
                    ),
            );

        let remote = Self::pane(trf("REMOTE · {}", &[&theirs_label]), theme::syntax(Highlight::Warning))
            .h(px(400.))
            .child(match &conflict {
                Some(c) => self.scroller("merge-remote", self.pane_body(&self.text[c.theirs.clone()], Some(c.theirs.start), "(deleted on that side)", theme::text())),
                None => self.scroller("merge-remote", Vec::new()),
            })
            .child(
                div().p_1().child(
                    button("merge-remote-apply", "Use Remote ▶", chosen == Some(Side::Remote))
                        .on_click(cx.listener(|this, _, _, cx| this.choose(Side::Remote, cx))),
                ),
            );

        let width = if base.is_some() { 1440. } else { 1180. };
        card(width)
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
                    .child(div().flex_1().child(trf("Merge {}", &[&name])))
                    .when(auto > 0, |d| {
                        d.child(div().text_size(px(12.)).text_color(theme::muted()).child(trf("{} auto-merged", &[&auto.to_string()])))
                    })
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(theme::muted())
                            .child(if total == 0 { tr("No conflicts").to_string() } else { trf("Conflict {} of {}", &[&(self.selected + 1).to_string(), &total.to_string()]) }),
                    )
                    .child(
                        div()
                            .id("merge-auto")
                            .px_2()
                            .rounded_sm()
                            .cursor_pointer()
                            .text_size(px(12.))
                            .hover(|d| d.bg(theme::hover()))
                            .child(tr("Auto-merge"))
                            .on_click(cx.listener(|this, _, _, cx| this.auto_merge_rest(cx))),
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
                    .child(local)
                    .children(base)
                    .child(result)
                    .child(remote),
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
                    .child(tr("← / → use a side · ↑ / ↓ walk the conflicts · B both · A auto-merge · U clear · Esc cancel"))
                    .child(
                        div()
                            .flex_1()
                            .text_right()
                            .when(unresolved > 0, |d| d.text_color(theme::syntax(Highlight::Warning)))
                            .child(if unresolved == 0 {
                                tr("Every conflict has a decision").to_string()
                            } else {
                                trf("{} conflict(s) keep their markers", &[&unresolved.to_string()])
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

/// Where the result text starts in the file, when it is one side's block verbatim.
fn result_origin(conflict: &Option<git::Conflict>, decision: &Decision) -> Option<usize> {
    let conflict = conflict.as_ref()?;
    match decision.side {
        Some(Side::Local) => Some(conflict.ours.start),
        Some(Side::Remote) => Some(conflict.theirs.start),
        _ => None,
    }
}
