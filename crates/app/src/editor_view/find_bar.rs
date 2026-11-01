//! In-file Find / Replace bar (⌘F / ⌘R).

use std::ops::Range;

use editor::Selection;
use gpui::{
    Context, Entity, Focusable, SharedString, Subscription, Window, actions, div, prelude::*, px,
};
use search::TextQuery;

use super::EditorView;
use crate::text_field::{FieldEvent, TextField};
use crate::theme;

actions!(
    editor,
    [
        ShowFind,
        ShowReplace,
        FindNext,
        FindPrev,
        CloseFind,
        ReplaceNext,
        ReplaceAll,
        ToggleCase,
        ToggleWord,
        ToggleRegex
    ]
);

pub struct FindBar {
    pub(super) query: Entity<TextField>,
    replace: Entity<TextField>,
    show_replace: bool,
    regex: bool,
    case_sensitive: bool,
    whole_word: bool,
    /// Byte ranges of matches, valid for `computed_for`.
    pub(super) matches: Vec<Range<usize>>,
    computed_for: Option<(u64, TextQuery)>,
    pub(super) current: Option<usize>,
    error: Option<String>,
    _subscriptions: Vec<Subscription>,
}

impl FindBar {
    fn text_query(&self, cx: &gpui::App) -> TextQuery {
        TextQuery {
            pattern: self.query.read(cx).text().to_string(),
            regex: self.regex,
            case_sensitive: self.case_sensitive,
            whole_word: self.whole_word,
        }
    }
}

impl EditorView {
    fn open_find(&mut self, replace: bool, window: &mut Window, cx: &mut Context<Self>) {
        // Seed with a single-line selection, as PhpStorm does.
        let selected = self.buffer.selected_text();
        let seed = (!selected.is_empty() && !selected.contains('\n')).then_some(selected);
        if self.find.is_none() {
            let query = cx.new(|cx| TextField::new("Search", cx));
            let replace_field = cx.new(|cx| TextField::new("Replace", cx));
            let subscriptions =
                vec![
                    cx.subscribe(&query, |this: &mut Self, _, _: &FieldEvent, cx| {
                        this.refresh_find(cx);
                        this.select_nearest_match(cx);
                    }),
                ];
            self.find = Some(FindBar {
                query,
                replace: replace_field,
                show_replace: replace,
                regex: false,
                case_sensitive: false,
                whole_word: false,
                matches: Vec::new(),
                computed_for: None,
                current: None,
                error: None,
                _subscriptions: subscriptions,
            });
        }
        let find = self.find.as_mut().unwrap();
        find.show_replace = replace;
        let query = find.query.clone();
        if let Some(seed) = seed {
            query.update(cx, |q, cx| q.set_text(seed, cx));
        }
        window.focus(&query.focus_handle(cx));
        self.refresh_find(cx);
        cx.notify();
    }

    pub(super) fn show_find(&mut self, _: &ShowFind, window: &mut Window, cx: &mut Context<Self>) {
        self.open_find(false, window, cx);
    }

    pub(super) fn show_replace(
        &mut self,
        _: &ShowReplace,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_find(true, window, cx);
    }

    pub(super) fn close_find(
        &mut self,
        _: &CloseFind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.find = None;
        window.focus(&self.focus_handle);
        cx.notify();
    }

    /// Recomputes matches when the query, options or buffer changed.
    pub(super) fn refresh_find(&mut self, cx: &mut Context<Self>) {
        let version = self.buffer.version();
        let Some(find) = &mut self.find else { return };
        let query = find.text_query(cx);
        let key = Some((version, query.clone()));
        if find.computed_for == key {
            return;
        }
        match query.find_all(&self.buffer.text()) {
            Ok(matches) => {
                find.matches = matches;
                find.error = None;
            }
            Err(err) => {
                find.matches.clear();
                find.error = Some(err);
            }
        }
        find.computed_for = key;
        find.current = None;
        cx.notify();
    }

    /// While typing, jump to the first match at or after the caret.
    fn select_nearest_match(&mut self, cx: &mut Context<Self>) {
        let caret = self
            .buffer
            .char_to_byte(self.buffer.selection.range().start);
        let Some(find) = &self.find else { return };
        let ix = find
            .matches
            .iter()
            .position(|m| m.start >= caret)
            .or((!find.matches.is_empty()).then_some(0));
        if let Some(ix) = ix {
            self.select_match(ix, cx);
        }
    }

    fn select_match(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(find) = &mut self.find else { return };
        let Some(range) = find.matches.get(ix).cloned() else {
            return;
        };
        find.current = Some(ix);
        let anchor = self.buffer.byte_to_char(range.start);
        let head = self.buffer.byte_to_char(range.end);
        self.buffer.selection = Selection { anchor, head };
        self.autoscroll = true;
        cx.notify();
    }

    fn step(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.find.is_none() {
            self.open_find(false, window, cx);
            return;
        }
        self.refresh_find(cx);
        let caret = self.buffer.char_to_byte(self.buffer.selection.head);
        let start = self
            .buffer
            .char_to_byte(self.buffer.selection.range().start);
        let Some(find) = &self.find else { return };
        let count = find.matches.len();
        if count == 0 {
            return;
        }
        let ix = if forward {
            find.matches
                .iter()
                .position(|m| m.start >= caret && m.start != start)
                .unwrap_or(0)
        } else {
            find.matches
                .iter()
                .rposition(|m| m.start < start)
                .unwrap_or(count - 1)
        };
        self.select_match(ix, cx);
    }

    pub(super) fn find_next(&mut self, _: &FindNext, window: &mut Window, cx: &mut Context<Self>) {
        self.step(true, window, cx);
    }

    pub(super) fn find_prev(&mut self, _: &FindPrev, window: &mut Window, cx: &mut Context<Self>) {
        self.step(false, window, cx);
    }

    /// Replaces the current match (if the selection is one) and moves to the next.
    pub(super) fn replace_next(
        &mut self,
        _: &ReplaceNext,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.refresh_find(cx);
        let Some(find) = &self.find else { return };
        let selection = self.buffer.selection.range();
        let selected =
            self.buffer.char_to_byte(selection.start)..self.buffer.char_to_byte(selection.end);
        if find.matches.contains(&selected) {
            let query = find.text_query(cx);
            let replacement = query.expand(
                &self.buffer.slice(selection.clone()),
                find.replace.read(cx).text(),
            );
            self.buffer.replace(selection, &replacement);
            self.refresh_find(cx);
        }
        self.step(true, window, cx);
    }

    pub(super) fn replace_all(&mut self, _: &ReplaceAll, _: &mut Window, cx: &mut Context<Self>) {
        let Some(find) = &self.find else { return };
        let query = find.text_query(cx);
        let replacement = find.replace.read(cx).text().to_string();
        let text = self.buffer.text();
        if let Ok((replaced, count)) = query.replace_all(&text, &replacement)
            && count > 0
        {
            let caret = self
                .buffer
                .char_to_byte(self.buffer.selection.head)
                .min(replaced.len());
            self.buffer.apply_edits(vec![(0..text.len(), replaced)], 0);
            let caret = self.buffer.byte_to_char(caret);
            self.buffer.move_to(caret, false);
            self.refresh_find(cx);
            self.changed(cx);
        }
    }

    fn toggle(&mut self, f: fn(&mut FindBar), cx: &mut Context<Self>) {
        if let Some(find) = &mut self.find {
            f(find);
            self.refresh_find(cx);
            self.select_nearest_match(cx);
        }
    }

    pub(super) fn toggle_case(&mut self, _: &ToggleCase, _: &mut Window, cx: &mut Context<Self>) {
        self.toggle(|f| f.case_sensitive ^= true, cx);
    }

    pub(super) fn toggle_word(&mut self, _: &ToggleWord, _: &mut Window, cx: &mut Context<Self>) {
        self.toggle(|f| f.whole_word ^= true, cx);
    }

    pub(super) fn toggle_regex(&mut self, _: &ToggleRegex, _: &mut Window, cx: &mut Context<Self>) {
        self.toggle(|f| f.regex ^= true, cx);
    }

    pub(super) fn render_find_bar(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        self.refresh_find(cx);
        let find = self.find.as_ref()?;
        let option = |id: &'static str, label: &'static str, on: bool, tip: &'static str| {
            div()
                .id(id)
                .px_1()
                .h(px(22.))
                .flex()
                .items_center()
                .rounded_sm()
                .cursor_pointer()
                .font_family(theme::editor_font())
                .text_size(px(12.))
                .when(on, |d| {
                    d.bg(theme::selection()).text_color(theme::ui_text())
                })
                .when(!on, |d| d.text_color(theme::muted()))
                .hover(|d| d.bg(theme::hover()))
                .child(label)
                .tooltip(move |_, cx| cx.new(|_| Tip(tip)).into())
        };
        let status: SharedString = match (&find.error, find.matches.len(), find.current) {
            (Some(_), _, _) => "Bad regex".into(),
            (None, 0, _) if find.query.read(cx).text().is_empty() => "".into(),
            (None, 0, _) => "0 results".into(),
            (None, n, Some(i)) => format!("{}/{n}", i + 1).into(),
            (None, n, None) => format!("{n} results").into(),
        };
        let no_results = find.matches.is_empty() && !find.query.read(cx).text().is_empty();
        let nav = |id: &'static str, label: &'static str| {
            div()
                .id(id)
                .px_1()
                .cursor_pointer()
                .text_color(theme::ui_text())
                .hover(|d| d.bg(theme::hover()))
                .child(label)
        };
        let text_button = |id: &'static str, label: &'static str| {
            div()
                .id(id)
                .px_2()
                .h(px(22.))
                .flex()
                .items_center()
                .rounded_sm()
                .border_1()
                .border_color(theme::border_strong())
                .cursor_pointer()
                .hover(|d| d.bg(theme::hover()))
                .child(label)
        };
        Some(
            div()
                .key_context("FindBar")
                .flex()
                .flex_col()
                .gap_1()
                .px_2()
                .py_1()
                .bg(theme::panel_bg())
                .border_b_1()
                .border_color(theme::border_strong())
                .text_size(px(12.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .w(px(320.))
                                .when(no_results, |d| d.bg(gpui::rgb(0x5a2828)))
                                .child(find.query.clone()),
                        )
                        .child(
                            option("find-case", "Cc", find.case_sensitive, "Match case (⌥C)")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.toggle_case(&ToggleCase, window, cx)
                                })),
                        )
                        .child(
                            option("find-word", "W", find.whole_word, "Words (⌥W)").on_click(
                                cx.listener(|this, _, window, cx| {
                                    this.toggle_word(&ToggleWord, window, cx)
                                }),
                            ),
                        )
                        .child(
                            option("find-regex", ".*", find.regex, "Regex (⌥X)").on_click(
                                cx.listener(|this, _, window, cx| {
                                    this.toggle_regex(&ToggleRegex, window, cx)
                                }),
                            ),
                        )
                        .child(
                            div()
                                .w(px(80.))
                                .text_color(if no_results {
                                    theme::error()
                                } else {
                                    theme::muted()
                                })
                                .child(status),
                        )
                        .child(nav("find-prev", "↑").on_click(
                            cx.listener(|this, _, window, cx| this.step(false, window, cx)),
                        ))
                        .child(nav("find-next", "↓").on_click(
                            cx.listener(|this, _, window, cx| this.step(true, window, cx)),
                        ))
                        .child(div().flex_1())
                        .child(nav("find-close", "×").on_click(cx.listener(
                            |this, _, window, cx| this.close_find(&CloseFind, window, cx),
                        ))),
                )
                .when(find.show_replace, |d| {
                    d.child(
                        div()
                            .key_context("ReplaceField")
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(div().w(px(320.)).child(find.replace.clone()))
                            .child(text_button("replace-one", "Replace").on_click(cx.listener(
                                |this, _, window, cx| this.replace_next(&ReplaceNext, window, cx),
                            )))
                            .child(text_button("replace-all", "Replace All").on_click(
                                cx.listener(|this, _, window, cx| {
                                    this.replace_all(&ReplaceAll, window, cx)
                                }),
                            )),
                    )
                })
                .on_action(cx.listener(Self::find_next))
                .on_action(cx.listener(Self::find_prev))
                .on_action(cx.listener(Self::close_find))
                .on_action(cx.listener(Self::replace_next))
                .on_action(cx.listener(Self::replace_all))
                .on_action(cx.listener(Self::toggle_case))
                .on_action(cx.listener(Self::toggle_word))
                .on_action(cx.listener(Self::toggle_regex)),
        )
    }
}

struct Tip(&'static str);

impl Render for Tip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_2()
            .py_1()
            .bg(theme::panel_bg())
            .border_1()
            .border_color(theme::border_strong())
            .rounded_sm()
            .text_size(px(12.))
            .text_color(theme::ui_text())
            .child(self.0)
    }
}
