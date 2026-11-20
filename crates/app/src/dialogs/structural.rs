//! Structural Search (⌥⌘F): a token pattern with `$Name$` holes, optionally replaced
//! across the project.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::{
    App, Context, Entity, EventEmitter, FocusHandle, Focusable, SharedString, Subscription, Window,
    div, prelude::*, px,
};
use php_index::Location;
use search::FileSearch;
use search::structural::Pattern;

use super::{Cancel, Confirm, DialogEvent, SelectNext, SelectPrev, button, card, checkbox};
use crate::i18n::{tr, trf};
use crate::text_field::{FieldEvent, TextField};
use crate::theme;

const MAX_RESULTS: usize = 2000;
const VISIBLE_ROWS: usize = 12;

pub struct StructuralSearchDialog {
    root: PathBuf,
    pattern: Entity<TextField>,
    replacement: Entity<TextField>,
    mask: Entity<TextField>,
    include_vendor: bool,
    hits: Arc<Vec<search::LineMatch>>,
    searching: bool,
    error: Option<String>,
    /// Discards results of searches started before the latest change.
    generation: u64,
    selected: usize,
    focus_handle: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<DialogEvent> for StructuralSearchDialog {}

impl StructuralSearchDialog {
    pub fn new(root: &Path, seed: Option<String>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let pattern = cx.new(|cx| TextField::new(tr("Structure, e.g. $A$->where($B$)"), cx));
        let replacement = cx.new(|cx| TextField::new(tr("Replace with, e.g. $A$->filter($B$)"), cx));
        let mask = cx.new(|cx| TextField::new(tr("File mask, e.g. *.php"), cx));
        mask.update(cx, |m, cx| m.set_text("*.php".to_string(), cx));
        let subscriptions = [&pattern, &replacement, &mask]
            .into_iter()
            .map(|field| {
                cx.subscribe(field, |this: &mut Self, _, _: &FieldEvent, cx| this.search(cx))
            })
            .collect();
        window.focus(&pattern.focus_handle(cx));
        let dialog = Self {
            root: root.to_path_buf(),
            pattern,
            replacement,
            mask,
            include_vendor: false,
            hits: Arc::new(Vec::new()),
            searching: false,
            error: None,
            generation: 0,
            selected: 0,
            focus_handle: cx.focus_handle(),
            _subscriptions: subscriptions,
        };
        if let Some(seed) = seed {
            dialog.pattern.update(cx, |p, cx| p.set_text(seed, cx));
        }
        dialog
    }

    fn search(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        let generation = self.generation;
        let text = self.pattern.read(cx).text().to_string();
        let options = FileSearch {
            mask: self.mask.read(cx).text().to_string(),
            include_vendor: self.include_vendor,
            max_results: MAX_RESULTS,
        };
        let root = self.root.clone();
        self.selected = 0;
        if text.trim().is_empty() {
            self.searching = false;
            self.hits = Arc::new(Vec::new());
            self.error = None;
            cx.notify();
            return;
        }
        self.searching = true;
        cx.spawn(async move |this, cx| {
            let found = cx
                .background_executor()
                .spawn(async move {
                    let pattern = Pattern::compile(&text)?;
                    Ok::<_, String>(search::structural::search_files(
                        &root,
                        &pattern,
                        &options,
                        MAX_RESULTS,
                    ))
                })
                .await;
            this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                this.searching = false;
                match found {
                    Ok(hits) => {
                        this.hits = Arc::new(hits);
                        this.error = None;
                    }
                    Err(err) => {
                        this.hits = Arc::new(Vec::new());
                        this.error = Some(err);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
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
        self.selected = (self.selected + 1).min(self.hits.len().saturating_sub(1));
        cx.notify();
    }

    fn open(&mut self, ix: usize, cx: &mut Context<Self>) {
        if let Some(found) = self.hits.get(ix) {
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

    fn replace_all(&mut self, cx: &mut Context<Self>) {
        let mut paths: Vec<PathBuf> = self.hits.iter().map(|h| h.path.clone()).collect();
        paths.dedup();
        if paths.is_empty() {
            return;
        }
        cx.emit(DialogEvent::StructuralReplace {
            pattern: self.pattern.read(cx).text().to_string(),
            replacement: self.replacement.read(cx).text().to_string(),
            paths,
        });
    }

    fn render_row(
        &self,
        ix: usize,
        found: &search::LineMatch,
        selected: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let rel = found
            .path
            .strip_prefix(&self.root)
            .unwrap_or(&found.path)
            .display()
            .to_string();
        let indent = found.text.len() - found.text.trim_start().len();
        let mut parts: Vec<gpui::AnyElement> = Vec::new();
        let mut at = indent;
        for range in &found.ranges {
            if range.start < at || range.end > found.text.len() {
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
            .id(("structural-hit", ix))
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

impl Focusable for StructuralSearchDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for StructuralSearchDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let files = {
            let mut paths: Vec<&PathBuf> = self.hits.iter().map(|h| &h.path).collect();
            paths.dedup();
            paths.len()
        };
        let count = self.hits.len();
        let status: SharedString = match (&self.error, self.searching, count) {
            (Some(err), _, _) => tr(err),
            (None, true, _) => tr("Searching…"),
            (None, false, 0) => tr("No matches."),
            (None, false, _) => {
                let (n, files) = (count.to_string(), files.to_string());
                if count >= MAX_RESULTS {
                    trf("first {} matches in {} files", &[&n, &files]).into()
                } else {
                    trf("{} matches in {} files", &[&n, &files]).into()
                }
            }
        };
        let selected = self.selected.min(self.hits.len().saturating_sub(1));
        let first = selected.saturating_sub(VISIBLE_ROWS - 1);
        let hits = self.hits.clone();
        let rows: Vec<_> = hits
            .iter()
            .enumerate()
            .skip(first)
            .take(VISIBLE_ROWS)
            .map(|(ix, found)| self.render_row(ix, found, ix == selected, cx))
            .collect();
        card(900.)
            .key_context("Dialog StructuralSearch")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::cancel))
            .on_action(cx.listener(Self::select_prev))
            .on_action(cx.listener(Self::select_next))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(theme::border())
                    .child(tr("Structural Search"))
                    .child(div().flex_1())
                    .child(
                        checkbox("ssr-vendor", tr("Include vendor"), self.include_vendor)
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
                            .child(div().flex_1().child(self.pattern.clone()))
                            .child(div().w(px(220.)).child(self.mask.clone())),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(div().flex_1().child(self.replacement.clone()))
                            .child(
                                button("structural-replace-all", "Replace All", false)
                                    .on_click(cx.listener(|this, _, _, cx| this.replace_all(cx))),
                            ),
                    ),
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
            .child(
                div()
                    .px_3()
                    .py_1()
                    .border_t_1()
                    .border_color(theme::border())
                    .text_size(px(11.))
                    .text_color(theme::muted())
                    .child(tr(
                        "$A$ stands for any expression · ↵ opens a match · Replace All writes the files",
                    )),
            )
    }
}
