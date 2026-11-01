//! Search Everywhere (⇧⇧), Go to File / Class / Symbol, Recent Files and Go to Line.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::{Action, AnyElement, App, Context, Entity, EventEmitter, FocusHandle, Focusable, SharedString, Subscription, Window, actions, div, prelude::*, px};
use php_index::Location;

use super::{Cancel, Confirm, DialogEvent, SelectNext, SelectPrev, card};
use crate::text_field::{FieldEvent, TextField};
use crate::theme;

actions!(navigate, [NextMode, PrevMode]);

const PER_SECTION: usize = 8;
const LIMIT: usize = 60;
const VISIBLE_ROWS: usize = 18;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavMode {
    All,
    Classes,
    Files,
    Symbols,
    Actions,
    Recent,
}

impl NavMode {
    const TABS: [NavMode; 5] = [Self::All, Self::Classes, Self::Files, Self::Symbols, Self::Actions];

    fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Classes => "Classes",
            Self::Files => "Files",
            Self::Symbols => "Symbols",
            Self::Actions => "Actions",
            Self::Recent => "Recent Files",
        }
    }
}

pub enum NavTarget {
    Location(Location),
    Action(Box<dyn Action>),
}

impl Clone for NavTarget {
    fn clone(&self) -> Self {
        match self {
            Self::Location(l) => Self::Location(l.clone()),
            Self::Action(a) => Self::Action(a.boxed_clone()),
        }
    }
}

#[derive(Clone)]
pub struct NavItem {
    /// Matched against the query.
    pub label: String,
    pub detail: String,
    pub icon: &'static str,
    pub target: NavTarget,
}

/// Everything the dialog can search; built by the workspace.
pub struct NavSources {
    pub classes: Arc<Vec<NavItem>>,
    pub symbols: Arc<Vec<NavItem>>,
    pub actions: Arc<Vec<NavItem>>,
    pub recent: Vec<NavItem>,
    /// Loaded in the background when not cached yet.
    pub files: Option<Arc<Vec<NavItem>>>,
}

pub fn file_items(root: &Path, files: &[PathBuf]) -> Vec<NavItem> {
    files
        .iter()
        .map(|path| {
            let rel = path.strip_prefix(root).unwrap_or(path);
            NavItem {
                label: rel.display().to_string(),
                detail: String::new(),
                icon: "▤",
                target: NavTarget::Location(Location { path: path.clone(), range: 0..0 }),
            }
        })
        .collect()
}

pub struct NavigateDialog {
    mode: NavMode,
    query: Entity<TextField>,
    sources: NavSources,
    selected: usize,
    focus_handle: FocusHandle,
    _subscription: Subscription,
}

impl EventEmitter<DialogEvent> for NavigateDialog {}

/// A row in the result list: an item with matched char positions, or a section header.
enum Row {
    Header(&'static str),
    Item(NavItem, Vec<usize>),
}

impl NavigateDialog {
    pub fn new(root: &Path, mode: NavMode, sources: NavSources, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let query = cx.new(|cx| TextField::new("Type to search", cx));
        let subscription = cx.subscribe(&query, |this: &mut Self, _, _: &FieldEvent, cx| {
            this.selected = 0;
            cx.notify();
        });
        window.focus(&query.focus_handle(cx));
        if sources.files.is_none() {
            let root = root.to_path_buf();
            cx.spawn(async move |this, cx| {
                let listed_root = root.clone();
                let files = cx
                    .background_executor()
                    .spawn(async move { search::list_files(&listed_root, false) })
                    .await;
                let files = Arc::new(file_items(&root, &files));
                this.update(cx, |this, cx| {
                    this.sources.files = Some(files.clone());
                    cx.emit(DialogEvent::FilesListed(files));
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
        Self { mode, query, sources, selected: 0, focus_handle: cx.focus_handle(), _subscription: subscription }
    }

    fn ranked(items: &[NavItem], query: &str, limit: usize) -> Vec<(NavItem, Vec<usize>)> {
        if query.trim().is_empty() {
            return items.iter().take(limit).map(|i| (i.clone(), Vec::new())).collect();
        }
        search::fuzzy_rank(items.iter(), query, |i| i.label.as_str(), limit)
            .into_iter()
            .map(|(i, p)| (i.clone(), p))
            .collect()
    }

    fn rows(&self, cx: &App) -> Vec<Row> {
        let query = self.query.read(cx).text().to_string();
        let empty = Arc::new(Vec::new());
        let files = self.sources.files.clone().unwrap_or(empty);
        let section = |items: &[NavItem], limit| Self::ranked(items, &query, limit).into_iter().map(|(i, p)| Row::Item(i, p));
        match self.mode {
            NavMode::Classes => section(&self.sources.classes, LIMIT).collect(),
            NavMode::Files => section(&files, LIMIT).collect(),
            NavMode::Symbols => section(&self.sources.symbols, LIMIT).collect(),
            NavMode::Actions => section(&self.sources.actions, LIMIT).collect(),
            NavMode::Recent => section(&self.sources.recent, LIMIT).collect(),
            NavMode::All => {
                if query.trim().is_empty() {
                    let mut rows = vec![Row::Header("Recent Files")];
                    rows.extend(section(&self.sources.recent, PER_SECTION));
                    return rows;
                }
                let mut rows = Vec::new();
                for (title, items) in [
                    ("Classes", &self.sources.classes),
                    ("Files", &files),
                    ("Symbols", &self.sources.symbols),
                    ("Actions", &self.sources.actions),
                ] {
                    let found: Vec<Row> = section(items, PER_SECTION).collect();
                    if !found.is_empty() {
                        rows.push(Row::Header(title));
                        rows.extend(found);
                    }
                }
                rows
            }
        }
    }

    fn items(&self, cx: &App) -> Vec<NavItem> {
        self.rows(cx)
            .into_iter()
            .filter_map(|r| match r {
                Row::Item(item, _) => Some(item),
                Row::Header(_) => None,
            })
            .collect()
    }

    fn set_mode(&mut self, mode: NavMode, cx: &mut Context<Self>) {
        self.mode = mode;
        self.selected = 0;
        cx.notify();
    }

    fn next_mode(&mut self, _: &NextMode, _: &mut Window, cx: &mut Context<Self>) {
        let ix = NavMode::TABS.iter().position(|m| *m == self.mode).map_or(0, |i| (i + 1) % NavMode::TABS.len());
        self.set_mode(NavMode::TABS[ix], cx);
    }

    fn prev_mode(&mut self, _: &PrevMode, _: &mut Window, cx: &mut Context<Self>) {
        let len = NavMode::TABS.len();
        let ix = NavMode::TABS.iter().position(|m| *m == self.mode).map_or(0, |i| (i + len - 1) % len);
        self.set_mode(NavMode::TABS[ix], cx);
    }

    fn select_prev(&mut self, _: &SelectPrev, _: &mut Window, cx: &mut Context<Self>) {
        self.selected = self.selected.saturating_sub(1);
        cx.notify();
    }

    fn select_next(&mut self, _: &SelectNext, _: &mut Window, cx: &mut Context<Self>) {
        let count = self.items(cx).len();
        self.selected = (self.selected + 1).min(count.saturating_sub(1));
        cx.notify();
    }

    fn open(&mut self, item: NavItem, cx: &mut Context<Self>) {
        match item.target {
            NavTarget::Location(location) => cx.emit(DialogEvent::OpenLocation(location)),
            NavTarget::Action(action) => cx.emit(DialogEvent::RunAction(action)),
        }
    }

    fn confirm(&mut self, _: &Confirm, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(item) = self.items(cx).into_iter().nth(self.selected) {
            self.open(item, cx);
        }
    }

    fn cancel(&mut self, _: &Cancel, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(DialogEvent::Dismiss);
    }
}

/// The label with fuzzy-matched characters emphasized.
fn highlighted(label: &str, positions: &[usize]) -> Vec<AnyElement> {
    let mut out = Vec::new();
    let mut run = String::new();
    let mut run_matched = false;
    for (i, c) in label.chars().enumerate() {
        let matched = positions.contains(&i);
        if matched != run_matched && !run.is_empty() {
            out.push(styled(std::mem::take(&mut run), run_matched));
        }
        run_matched = matched;
        run.push(c);
    }
    if !run.is_empty() {
        out.push(styled(run, run_matched));
    }
    out
}

fn styled(text: String, matched: bool) -> AnyElement {
    div()
        .when(matched, |d| d.text_color(theme::syntax(php_syntax::Highlight::Function)))
        .child(SharedString::from(text))
        .into_any_element()
}

impl Focusable for NavigateDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for NavigateDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = self.rows(cx);
        let item_count = rows.iter().filter(|r| matches!(r, Row::Item(..))).count();
        let selected = self.selected.min(item_count.saturating_sub(1));
        let tabs = if self.mode == NavMode::Recent { vec![NavMode::Recent] } else { NavMode::TABS.to_vec() };
        let tabs = tabs.into_iter().map(|mode| {
            div()
                .id(SharedString::from(mode.label()))
                .px_2()
                .py_1()
                .rounded_sm()
                .cursor_pointer()
                .when(mode == self.mode, |d| d.bg(theme::selection()).text_color(theme::ui_text()))
                .when(mode != self.mode, |d| d.text_color(theme::muted()))
                .hover(|d| d.bg(theme::hover()))
                .child(mode.label())
                .on_click(cx.listener(move |this, _, _, cx| this.set_mode(mode, cx)))
        });
        let mut elements: Vec<AnyElement> = Vec::new();
        let mut item_ix = 0;
        let first = selected.saturating_sub(VISIBLE_ROWS - 4);
        for row in rows {
            match row {
                Row::Header(title) => {
                    if item_ix >= first {
                        elements.push(div().px_3().pt_2().pb_1().text_size(px(11.)).text_color(theme::muted()).child(title).into_any_element());
                    }
                }
                Row::Item(item, positions) => {
                    let ix = item_ix;
                    item_ix += 1;
                    if ix < first || ix >= first + VISIBLE_ROWS {
                        continue;
                    }
                    let open_item = item.clone();
                    elements.push(
                        div()
                            .id(("nav", ix))
                            .flex()
                            .gap_2()
                            .px_3()
                            .h(px(24.))
                            .items_center()
                            .cursor_pointer()
                            .when(ix == selected, |d| d.bg(theme::selection()))
                            .hover(|d| d.bg(theme::hover()))
                            .child(div().w(px(16.)).flex_none().text_color(theme::syntax(php_syntax::Highlight::Keyword)).child(item.icon))
                            .child(div().flex().flex_none().text_color(theme::text()).children(highlighted(&item.label, &positions)))
                            .child(div().flex_1().overflow_hidden().whitespace_nowrap().text_right().text_color(theme::muted()).child(item.detail.clone()))
                            .on_click(cx.listener(move |this, _, _, cx| this.open(open_item.clone(), cx)))
                            .into_any_element(),
                    );
                }
            }
        }
        let loading = self.sources.files.is_none() && matches!(self.mode, NavMode::Files | NavMode::All);
        card(760.)
            .key_context("ListDialog NavigateDialog")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::cancel))
            .on_action(cx.listener(Self::select_prev))
            .on_action(cx.listener(Self::select_next))
            .on_action(cx.listener(Self::next_mode))
            .on_action(cx.listener(Self::prev_mode))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .px_2()
                    .pt_2()
                    .text_size(px(12.))
                    .children(tabs)
                    .child(div().flex_1())
                    .when(self.mode != NavMode::Recent, |d| d.child(div().text_color(theme::muted()).child("Tab to switch"))),
            )
            .child(div().p_2().child(self.query.clone()))
            .when(loading, |d| d.child(div().px_3().text_size(px(12.)).text_color(theme::muted()).child("Indexing files…")))
            .child(div().pb_2().min_h(px(200.)).font_family(theme::EDITOR_FONT).text_size(px(12.)).children(elements))
    }
}

// --- Go to Line -----------------------------------------------------------------

pub struct GoToLineDialog {
    field: Entity<TextField>,
    focus_handle: FocusHandle,
}

impl EventEmitter<DialogEvent> for GoToLineDialog {}

impl GoToLineDialog {
    pub fn new(current: (usize, usize), window: &mut Window, cx: &mut Context<Self>) -> Self {
        let placeholder = format!("{}:{}", current.0 + 1, current.1 + 1);
        let field = cx.new(|cx| TextField::new(placeholder, cx));
        window.focus(&field.focus_handle(cx));
        Self { field, focus_handle: cx.focus_handle() }
    }

    fn confirm(&mut self, _: &Confirm, _: &mut Window, cx: &mut Context<Self>) {
        let text = self.field.read(cx).text().trim().to_string();
        let mut parts = text.split(':').map(|p| p.trim().parse::<usize>().ok());
        if let Some(Some(line)) = parts.next() {
            let column = parts.next().flatten().unwrap_or(1);
            cx.emit(DialogEvent::GoToLine(line.saturating_sub(1), column.saturating_sub(1)));
        }
    }

    fn cancel(&mut self, _: &Cancel, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(DialogEvent::Dismiss);
    }
}

impl Focusable for GoToLineDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for GoToLineDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        card(320.)
            .key_context("Dialog")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::cancel))
            .child(div().px_3().pt_2().child("Go to Line:Column"))
            .child(div().p_3().child(self.field.clone()))
    }
}
