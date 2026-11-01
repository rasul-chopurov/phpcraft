//! Side-by-side diff shown as an editor tab (like PhpStorm's "Commit: File.php" tab), with
//! navigation between changes and between the files of a commit or change list.

use std::path::PathBuf;

use gpui::{
    App, Context, FocusHandle, Focusable, ScrollHandle, SharedString, Window, actions, div, prelude::*, px,
};

use std::sync::Arc;

use crate::theme;

actions!(diff, [NextChange, PrevChange, NextFile, PrevFile]);

/// Reads `(old, new)` contents; runs on a background thread when the file is shown.
pub type Loader = Arc<dyn Fn() -> (String, String) + Send + Sync>;

/// One file to compare.
#[derive(Clone)]
pub struct DiffSource {
    /// Shown in the tab and header, e.g. `Dashboard.php`.
    pub title: String,
    /// `2d2e7232 app/Filament/Pages/Dashboard.php` or `HEAD · …`.
    pub subtitle: String,
    /// Fetches the two versions (git show / disk) lazily, so opening a commit with many
    /// files doesn't block.
    pub load: Loader,
    pub left_label: String,
    pub right_label: String,
    /// File to open from the diff (working tree version).
    pub open_path: Option<PathBuf>,
}

pub enum DiffEvent {
    Open(PathBuf),
}

impl gpui::EventEmitter<DiffEvent> for DiffView {}

pub struct DiffView {
    /// "Commit" or "Changes": the tab prefix.
    pub kind: String,
    files: Vec<DiffSource>,
    index: usize,
    rows: Vec<git::DiffRow>,
    /// Row indices where a change block starts.
    changes: Vec<usize>,
    current_change: Option<usize>,
    loading: bool,
    _load: Option<gpui::Task<()>>,
    scroll: ScrollHandle,
    focus_handle: FocusHandle,
}

impl DiffView {
    pub fn new(kind: impl Into<String>, files: Vec<DiffSource>, index: usize, cx: &mut Context<Self>) -> Self {
        let mut view = Self {
            kind: kind.into(),
            files,
            index: 0,
            rows: Vec::new(),
            changes: Vec::new(),
            current_change: None,
            loading: false,
            _load: None,
            scroll: ScrollHandle::new(),
            focus_handle: cx.focus_handle(),
        };
        view.show(index, cx);
        view
    }

    pub fn tab_title(&self) -> String {
        let name = self.files.get(self.index).map(|f| f.title.as_str()).unwrap_or("");
        format!("{}: {name}", self.kind)
    }

    /// Shows file `index`: its versions and diff are computed in the background.
    fn show(&mut self, index: usize, cx: &mut Context<Self>) {
        self.index = index.min(self.files.len().saturating_sub(1));
        let Some(file) = self.files.get(self.index) else { return };
        let load = file.load.clone();
        self.loading = true;
        self.rows.clear();
        self.changes.clear();
        self.current_change = None;
        let shown = self.index;
        self._load = Some(cx.spawn(async move |this, cx| {
            let rows = cx
                .background_spawn(async move {
                    let (old, new) = load();
                    git::side_by_side(&old, &new)
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                if view.index != shown {
                    return;
                }
                view.changes = rows
                    .iter()
                    .enumerate()
                    .filter(|(i, r)| r.tag != git::RowTag::Equal && (*i == 0 || rows[*i - 1].tag == git::RowTag::Equal))
                    .map(|(i, _)| i)
                    .collect();
                view.rows = rows;
                view.loading = false;
                view.scroll.scroll_to_item(0);
                cx.notify();
            });
        }));
        cx.notify();
    }


    fn jump(&mut self, forward: bool, cx: &mut Context<Self>) {
        if self.changes.is_empty() {
            return;
        }
        let next = match (self.current_change, forward) {
            (None, true) => 0,
            (None, false) => self.changes.len() - 1,
            (Some(c), true) => (c + 1).min(self.changes.len() - 1),
            (Some(c), false) => c.saturating_sub(1),
        };
        self.current_change = Some(next);
        // Show a little context above the change.
        self.scroll.scroll_to_item(self.changes[next].saturating_sub(3));
        cx.notify();
    }

    fn next_change(&mut self, _: &NextChange, _: &mut Window, cx: &mut Context<Self>) {
        self.jump(true, cx);
    }

    fn prev_change(&mut self, _: &PrevChange, _: &mut Window, cx: &mut Context<Self>) {
        self.jump(false, cx);
    }

    fn next_file(&mut self, _: &NextFile, _: &mut Window, cx: &mut Context<Self>) {
        if self.index + 1 < self.files.len() {
            self.show(self.index + 1, cx);
        }
    }

    fn prev_file(&mut self, _: &PrevFile, _: &mut Window, cx: &mut Context<Self>) {
        if self.index > 0 {
            self.show(self.index - 1, cx);
        }
    }
}

impl Focusable for DiffView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

fn side(line: &Option<(usize, String)>, bg: Option<gpui::Hsla>) -> gpui::Div {
    let (number, text) = match line {
        Some((n, text)) => ((n + 1).to_string(), text.replace('\t', "    ")),
        None => (String::new(), String::new()),
    };
    div()
        .flex_1()
        .flex()
        .min_w(px(0.))
        .overflow_hidden()
        .when_some(bg, |d, bg| d.bg(bg))
        .child(div().w(px(46.)).flex_none().pr_2().text_right().text_color(theme::line_number()).child(number))
        .child(div().flex_1().whitespace_nowrap().overflow_hidden().text_color(theme::text()).child(SharedString::from(text)))
}

impl Render for DiffView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let added = theme::success().opacity(0.16);
        let removed = theme::error().opacity(0.16);
        let changed = theme::accent().opacity(0.14);
        let current_row = self.current_change.and_then(|c| self.changes.get(c)).copied();
        let rows = self.rows.iter().enumerate().map(|(ix, row)| {
            let (left_bg, right_bg) = match row.tag {
                git::RowTag::Equal => (None, None),
                git::RowTag::Removed => (Some(removed), None),
                git::RowTag::Added => (None, Some(added)),
                git::RowTag::Changed => (Some(changed), Some(changed)),
            };
            div()
                .id(("diff-row", ix))
                .flex()
                .h(px(20.))
                .when(current_row == Some(ix), |d| d.border_t_1().border_color(theme::accent()))
                .child(side(&row.left, left_bg))
                .child(div().w(px(1.)).h_full().bg(theme::border()))
                .child(side(&row.right, right_bg))
        });
        let file = self.files.get(self.index).cloned();
        let nav = |id: &'static str, label: &'static str, enabled: bool| {
            div()
                .id(id)
                .px_1p5()
                .h(px(22.))
                .flex()
                .items_center()
                .rounded_md()
                .text_color(if enabled { theme::ui_text() } else { theme::muted() })
                .when(enabled, |d| d.cursor_pointer().hover(|d| d.bg(theme::hover())))
                .child(label)
        };
        let has_prev = self.index > 0;
        let has_next = self.index + 1 < self.files.len();
        div()
            .key_context("DiffView")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::next_change))
            .on_action(cx.listener(Self::prev_change))
            .on_action(cx.listener(Self::next_file))
            .on_action(cx.listener(Self::prev_file))
            .size_full()
            .flex()
            .flex_col()
            .bg(theme::editor_bg())
            .text_size(px(12.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .px_2()
                    .h(px(30.))
                    .border_b_1()
                    .border_color(theme::border())
                    .child(nav("prev-change", "↑", !self.changes.is_empty()).on_click(cx.listener(|this, _, w, cx| this.prev_change(&PrevChange, w, cx))))
                    .child(nav("next-change", "↓", !self.changes.is_empty()).on_click(cx.listener(|this, _, w, cx| this.next_change(&NextChange, w, cx))))
                    .child(div().w(px(8.)))
                    .child(nav("prev-file", "←", has_prev).on_click(cx.listener(|this, _, w, cx| this.prev_file(&PrevFile, w, cx))))
                    .child(div().text_color(theme::accent()).child(format!("{}/{} files", self.index + 1, self.files.len().max(1))))
                    .child(nav("next-file", "→", has_next).on_click(cx.listener(|this, _, w, cx| this.next_file(&NextFile, w, cx))))
                    .child(div().flex_1())
                    .child(div().text_color(theme::muted()).child(if self.loading { "Loading…".to_string() } else { format!("{} differences", self.changes.len()) }))
                    .when_some(file.as_ref().and_then(|f| f.open_path.clone()), |d, path| {
                        d.child(nav("diff-open", "Open in Editor", true).on_click(cx.listener(move |_, _, _, cx| cx.emit(DiffEvent::Open(path.clone())))))
                    }),
            )
            .when_some(file, |d, file| {
                d.child(
                    div()
                        .flex()
                        .h(px(24.))
                        .items_center()
                        .border_b_1()
                        .border_color(theme::border())
                        .text_color(theme::muted())
                        .child(div().flex_1().px_3().child(format!("{}  {}", file.left_label, file.subtitle)))
                        .child(div().flex_1().px_3().child(file.right_label)),
                )
            })
            .child(
                div()
                    .id("diff-rows")
                    .flex_1()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .font_family(theme::editor_font())
                    .children(rows),
            )
    }
}
