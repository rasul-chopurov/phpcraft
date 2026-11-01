//! Find in Files (⇧⌘F) and Replace in Files (⇧⌘R).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::{
    App, Context, Entity, EventEmitter, FocusHandle, Focusable, SharedString, Subscription, Window,
    div, prelude::*, px,
};
use php_index::Location;
use search::{FileSearch, LineMatch, TextQuery};

use super::{
    Cancel, Confirm, DialogEvent, FocusNext, FocusPrev, SelectNext, SelectPrev, button, card,
    checkbox,
};
use crate::text_field::{FieldEvent, TextField};
use crate::theme;

const MAX_RESULTS: usize = 2000;
const VISIBLE_ROWS: usize = 16;

pub struct FindInFilesDialog {
    root: PathBuf,
    query: Entity<TextField>,
    replace: Option<Entity<TextField>>,
    mask: Entity<TextField>,
    regex: bool,
    case_sensitive: bool,
    whole_word: bool,
    include_vendor: bool,
    results: Arc<Vec<LineMatch>>,
    searching: bool,
    error: Option<String>,
    /// Discards results of searches started before the latest change.
    generation: u64,
    selected: usize,
    focus_handle: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<DialogEvent> for FindInFilesDialog {}

impl FindInFilesDialog {
    pub fn new(
        root: &Path,
        replace: bool,
        seed: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let query = cx.new(|cx| TextField::new("Text to find", cx));
        let replace = replace.then(|| cx.new(|cx| TextField::new("Replace with", cx)));
        let mask = cx.new(|cx| TextField::new("File mask, e.g. *.php, *.blade.php", cx));
        let subscriptions = [&query, &mask]
            .into_iter()
            .map(|field| {
                cx.subscribe(field, |this: &mut Self, _, _: &FieldEvent, cx| {
                    this.search(cx)
                })
            })
            .collect();
        window.focus(&query.focus_handle(cx));
        let dialog = Self {
            root: root.to_path_buf(),
            query,
            replace,
            mask,
            regex: false,
            case_sensitive: false,
            whole_word: false,
            include_vendor: false,
            results: Arc::new(Vec::new()),
            searching: false,
            error: None,
            generation: 0,
            selected: 0,
            focus_handle: cx.focus_handle(),
            _subscriptions: subscriptions,
        };
        if let Some(seed) = seed {
            dialog.query.update(cx, |q, cx| q.set_text(seed, cx));
        }
        dialog
    }

    fn text_query(&self, cx: &App) -> TextQuery {
        TextQuery {
            pattern: self.query.read(cx).text().to_string(),
            regex: self.regex,
            case_sensitive: self.case_sensitive,
            whole_word: self.whole_word,
        }
    }

    fn search(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        let generation = self.generation;
        let query = self.text_query(cx);
        let options = FileSearch {
            mask: self.mask.read(cx).text().to_string(),
            include_vendor: self.include_vendor,
            max_results: MAX_RESULTS,
        };
        let root = self.root.clone();
        self.searching = !query.pattern.is_empty();
        self.selected = 0;
        if query.pattern.is_empty() {
            self.results = Arc::new(Vec::new());
            self.error = None;
            cx.notify();
            return;
        }
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { search::search_files(&root, &query, &options) })
                .await;
            this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                this.searching = false;
                match result {
                    Ok(results) => {
                        this.results = Arc::new(results);
                        this.error = None;
                    }
                    Err(err) => this.error = Some(err),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn toggle(&mut self, f: fn(&mut Self), cx: &mut Context<Self>) {
        f(self);
        self.search(cx);
    }

    fn select_prev(&mut self, _: &SelectPrev, _: &mut Window, cx: &mut Context<Self>) {
        self.selected = self.selected.saturating_sub(1);
        cx.notify();
    }

    fn select_next(&mut self, _: &SelectNext, _: &mut Window, cx: &mut Context<Self>) {
        self.selected = (self.selected + 1).min(self.results.len().saturating_sub(1));
        cx.notify();
    }

    fn open(&mut self, ix: usize, cx: &mut Context<Self>) {
        if let Some(found) = self.results.get(ix) {
            let len = found.ranges.first().map_or(0, |r| r.len());
            cx.emit(DialogEvent::OpenLocation(Location {
                path: found.path.clone(),
                range: found.offset..found.offset + len,
            }));
        }
    }

    fn confirm(&mut self, _: &Confirm, _: &mut Window, cx: &mut Context<Self>) {
        self.open(self.selected, cx);
    }

    fn cancel(&mut self, _: &Cancel, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(DialogEvent::Dismiss);
    }

    fn cycle(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let mut fields = vec![self.query.clone()];
        fields.extend(self.replace.clone());
        fields.push(self.mask.clone());
        let current = fields
            .iter()
            .position(|f| f.focus_handle(cx).is_focused(window))
            .unwrap_or(0);
        let next = (current as isize + delta).rem_euclid(fields.len() as isize) as usize;
        window.focus(&fields[next].focus_handle(cx));
    }

    fn focus_next(&mut self, _: &FocusNext, window: &mut Window, cx: &mut Context<Self>) {
        self.cycle(1, window, cx);
    }

    fn focus_prev(&mut self, _: &FocusPrev, window: &mut Window, cx: &mut Context<Self>) {
        self.cycle(-1, window, cx);
    }

    fn replace_all(&mut self, cx: &mut Context<Self>) {
        let Some(replace) = &self.replace else { return };
        let mut paths: Vec<PathBuf> = self.results.iter().map(|r| r.path.clone()).collect();
        paths.dedup();
        if paths.is_empty() {
            return;
        }
        cx.emit(DialogEvent::ReplaceInFiles {
            query: self.text_query(cx),
            replacement: replace.read(cx).text().to_string(),
            paths,
        });
    }

    fn render_row(
        &self,
        ix: usize,
        found: &LineMatch,
        selected: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let rel = found
            .path
            .strip_prefix(&self.root)
            .unwrap_or(&found.path)
            .display()
            .to_string();
        // Trim long leading whitespace and show the line with matches emphasized.
        let indent = found.text.len() - found.text.trim_start().len();
        let mut parts: Vec<gpui::AnyElement> = Vec::new();
        let mut at = indent;
        for range in &found.ranges {
            if range.start < at {
                continue;
            }
            parts.push(
                div()
                    .child(SharedString::from(found.text[at..range.start].to_string()))
                    .into_any_element(),
            );
            parts.push(
                div()
                    .bg(theme::find_match())
                    .text_color(theme::ui_text())
                    .child(SharedString::from(found.text[range.clone()].to_string()))
                    .into_any_element(),
            );
            at = range.end;
        }
        parts.push(
            div()
                .child(SharedString::from(found.text[at..].to_string()))
                .into_any_element(),
        );
        div()
            .id(("found", ix))
            .flex()
            .gap_3()
            .px_3()
            .h(px(22.))
            .items_center()
            .overflow_hidden()
            .cursor_pointer()
            .when(selected, |d| d.bg(theme::selection()))
            .hover(|d| d.bg(theme::hover()))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_color(theme::text())
                    .children(parts),
            )
            .child(
                div()
                    .flex_none()
                    .text_color(theme::muted())
                    .child(format!("{rel}:{}", found.line + 1)),
            )
            .on_click(cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                this.selected = ix;
                if event.click_count() >= 2 {
                    this.open(ix, cx);
                }
                cx.notify();
            }))
    }
}

impl Focusable for FindInFilesDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for FindInFilesDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let option = |id: &'static str, label: &'static str, on: bool| {
            div()
                .id(id)
                .px_1()
                .h(px(22.))
                .flex()
                .items_center()
                .rounded_sm()
                .cursor_pointer()
                .font_family(theme::editor_font())
                .when(on, |d| {
                    d.bg(theme::selection()).text_color(theme::ui_text())
                })
                .when(!on, |d| d.text_color(theme::muted()))
                .hover(|d| d.bg(theme::hover()))
                .child(label)
        };
        let files = {
            let mut paths: Vec<&PathBuf> = self.results.iter().map(|r| &r.path).collect();
            paths.dedup();
            paths.len()
        };
        let status: SharedString = match (&self.error, self.searching) {
            (Some(err), _) => err.clone().into(),
            (None, true) => "Searching…".into(),
            (None, false) if self.query.read(cx).text().is_empty() => "".into(),
            (None, false) => format!(
                "{}{} matches in {files} files",
                if self.results.len() >= MAX_RESULTS {
                    "first "
                } else {
                    ""
                },
                self.results.len()
            )
            .into(),
        };
        let selected = self.selected.min(self.results.len().saturating_sub(1));
        let first = selected.saturating_sub(VISIBLE_ROWS - 1);
        let results = self.results.clone();
        let rows: Vec<_> = results
            .iter()
            .enumerate()
            .skip(first)
            .take(VISIBLE_ROWS)
            .map(|(ix, found)| self.render_row(ix, found, ix == selected, cx))
            .collect();
        let title = if self.replace.is_some() {
            "Replace in Files"
        } else {
            "Find in Files"
        };
        card(900.)
            .key_context("Dialog ArtisanDialog")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::cancel))
            .on_action(cx.listener(Self::select_prev))
            .on_action(cx.listener(Self::select_next))
            .on_action(cx.listener(Self::focus_next))
            .on_action(cx.listener(Self::focus_prev))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(theme::border())
                    .child(title)
                    .child(div().flex_1())
                    .child(
                        checkbox("include-vendor", "Include vendor", self.include_vendor)
                            .text_size(px(12.))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.toggle(|d| d.include_vendor ^= true, cx)
                            })),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .p_2()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .text_size(px(12.))
                            .child(div().flex_1().child(self.query.clone()))
                            .child(option("ff-case", "Cc", self.case_sensitive).on_click(
                                cx.listener(|this, _, _, cx| {
                                    this.toggle(|d| d.case_sensitive ^= true, cx)
                                }),
                            ))
                            .child(
                                option("ff-word", "W", self.whole_word).on_click(cx.listener(
                                    |this, _, _, cx| this.toggle(|d| d.whole_word ^= true, cx),
                                )),
                            )
                            .child(option("ff-regex", ".*", self.regex).on_click(
                                cx.listener(|this, _, _, cx| this.toggle(|d| d.regex ^= true, cx)),
                            ))
                            .child(div().w(px(220.)).child(self.mask.clone())),
                    )
                    .children(self.replace.clone().map(|replace| {
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(div().flex_1().child(replace))
                            .child(
                                button("replace-in-files", "Replace All", true)
                                    .on_click(cx.listener(|this, _, _, cx| this.replace_all(cx))),
                            )
                    })),
            )
            .child(
                div()
                    .px_3()
                    .pb_1()
                    .text_size(px(12.))
                    .text_color(if self.error.is_some() {
                        theme::error()
                    } else {
                        theme::muted()
                    })
                    .child(status),
            )
            .child(
                div()
                    .min_h(px(VISIBLE_ROWS as f32 * 22.))
                    .pb_2()
                    .font_family(theme::editor_font())
                    .text_size(px(12.))
                    .children(rows),
            )
    }
}
