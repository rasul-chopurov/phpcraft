//! Search Everywhere (⇧⇧), Go to File / Class / Symbol, Recent Files and Go to Line.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::{
    Action, AnyElement, App, Context, Entity, EventEmitter, FocusHandle, Focusable, SharedString,
    Subscription, Window, actions, div, prelude::*, px,
};
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
    Text,
    Recent,
    UnusedViews,
    Tables,
}

impl NavMode {
    /// Modes showing one list of `sources.recent` without tabs.
    fn is_single(self) -> bool {
        matches!(self, Self::Recent | Self::UnusedViews | Self::Tables)
    }

    const TABS: [NavMode; 6] = [
        Self::All,
        Self::Classes,
        Self::Files,
        Self::Symbols,
        Self::Actions,
        Self::Text,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Classes => "Classes",
            Self::Files => "Files",
            Self::Symbols => "Symbols",
            Self::Actions => "Actions",
            Self::Text => "Text",
            Self::Recent => "Recent Files",
            Self::UnusedViews => "Unused Views",
            Self::Tables => "Database Tables",
        }
    }
}

pub enum NavTarget {
    Location(Location),
    Action(Box<dyn Action>),
    /// A database table to build a model from.
    Table(String),
}

impl Clone for NavTarget {
    fn clone(&self) -> Self {
        match self {
            Self::Location(l) => Self::Location(l.clone()),
            Self::Action(a) => Self::Action(a.boxed_clone()),
            Self::Table(t) => Self::Table(t.clone()),
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
                target: NavTarget::Location(Location {
                    path: path.clone(),
                    range: 0..0,
                }),
            }
        })
        .collect()
}

pub struct NavigateDialog {
    root: std::path::PathBuf,
    /// Also search vendor/ (PhpStorm's "Include non-project items").
    include_vendor: bool,
    /// Full-text matches for the Text tab, for the query they were computed for.
    text_results: (String, Arc<Vec<NavItem>>),
    text_task: Option<gpui::Task<()>>,
    /// Cached file contents for the preview.
    preview_cache: std::cell::RefCell<Option<(std::path::PathBuf, Arc<String>)>>,
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
    pub fn new(
        root: &Path,
        mode: NavMode,
        sources: NavSources,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let query = cx.new(|cx| TextField::new("Type to search", cx));
        let subscription = cx.subscribe(&query, |this: &mut Self, _, _: &FieldEvent, cx| {
            this.selected = 0;
            if this.mode == NavMode::Text {
                this.search_text(cx);
            }
            cx.notify();
        });
        window.focus(&query.focus_handle(cx));
        if sources.files.is_none() && !mode.is_single() {
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
        Self {
            root: root.to_path_buf(),
            include_vendor: false,
            text_results: (String::new(), Arc::new(Vec::new())),
            text_task: None,
            preview_cache: Default::default(),
            mode,
            query,
            sources,
            selected: 0,
            focus_handle: cx.focus_handle(),
            _subscription: subscription,
        }
    }

    /// Full-text search for the Text tab, in the background (newer queries replace it).
    fn search_text(&mut self, cx: &mut Context<Self>) {
        let query = self.query.read(cx).text().to_string();
        if query.trim().len() < 2 {
            self.text_results = (query, Arc::new(Vec::new()));
            self.text_task = None;
            return;
        }
        let root = self.root.clone();
        let include_vendor = self.include_vendor;
        self.text_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(std::time::Duration::from_millis(150)).await;
            let q = query.clone();
            let items = cx
                .background_executor()
                .spawn(async move {
                    let options = search::FileSearch { mask: String::new(), include_vendor, max_results: 200 };
                    search::search_files(&root, &search::TextQuery::new(q), &options)
                        .unwrap_or_default()
                        .into_iter()
                        .map(|m| {
                            let rel = m.path.strip_prefix(&root).unwrap_or(&m.path).display().to_string();
                            NavItem {
                                label: m.text.trim().chars().take(120).collect(),
                                detail: format!("{rel}:{}", m.line + 1),
                                icon: "≡",
                                target: NavTarget::Location(Location { path: m.path.clone(), range: m.offset..m.offset }),
                            }
                        })
                        .collect::<Vec<_>>()
                })
                .await;
            this.update(cx, |this, cx| {
                this.text_results = (query, Arc::new(items));
                cx.notify();
            })
            .ok();
        }));
    }

    fn set_include_vendor(&mut self, include: bool, cx: &mut Context<Self>) {
        self.include_vendor = include;
        // Reload the file list with or without vendor/.
        let root = self.root.clone();
        self.sources.files = None;
        cx.spawn(async move |this, cx| {
            let listed_root = root.clone();
            let files = cx.background_executor().spawn(async move { search::list_files(&listed_root, include) }).await;
            let files = Arc::new(file_items(&root, &files));
            this.update(cx, |this, cx| {
                this.sources.files = Some(files);
                cx.notify();
            })
            .ok();
        })
        .detach();
        if self.mode == NavMode::Text {
            self.search_text(cx);
        }
        cx.notify();
    }

    /// Lines around the selected location, for the preview pane.
    fn preview(&self, item: &NavItem) -> Option<(std::path::PathBuf, usize, Vec<String>)> {
        let NavTarget::Location(location) = &item.target else { return None };
        let cached = self.preview_cache.borrow().as_ref().filter(|(p, _)| *p == location.path).map(|(_, t)| t.clone());
        let text = match cached {
            Some(text) => text,
            None => {
                let text = Arc::new(std::fs::read_to_string(&location.path).ok()?);
                *self.preview_cache.borrow_mut() = Some((location.path.clone(), text.clone()));
                text
            }
        };
        let start = location.range.start.min(text.len());
        let line = text[..start].matches('\n').count();
        let first = line.saturating_sub(4);
        let lines = text.lines().skip(first).take(12).map(|l| l.replace('\t', "    ")).collect();
        Some((location.path.clone(), line - first, lines))
    }

    fn ranked(items: &[NavItem], query: &str, limit: usize) -> Vec<(NavItem, Vec<usize>)> {
        if query.trim().is_empty() {
            return items
                .iter()
                .take(limit)
                .map(|i| (i.clone(), Vec::new()))
                .collect();
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
        let include_vendor = self.include_vendor;
        let in_project = |i: &&NavItem| {
            include_vendor
                || !matches!(&i.target, NavTarget::Location(l) if l.path.components().any(|c| c.as_os_str() == "vendor"))
        };
        let section = |items: &[NavItem], limit| {
            let items: Vec<NavItem> = items.iter().filter(in_project).cloned().collect();
            Self::ranked(&items, &query, limit)
                .into_iter()
                .map(|(i, p)| Row::Item(i, p))
        };
        match self.mode {
            NavMode::Classes => section(&self.sources.classes, LIMIT).collect(),
            NavMode::Files => section(&files, LIMIT).collect(),
            NavMode::Symbols => section(&self.sources.symbols, LIMIT).collect(),
            NavMode::Actions => section(&self.sources.actions, LIMIT).collect(),
            NavMode::Text => self.text_results.1.iter().map(|i| Row::Item(i.clone(), Vec::new())).collect(),
            NavMode::Recent | NavMode::UnusedViews | NavMode::Tables => {
                section(&self.sources.recent, 500).collect()
            }
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
        if mode == NavMode::Text {
            self.search_text(cx);
        }
        cx.notify();
    }

    fn next_mode(&mut self, _: &NextMode, _: &mut Window, cx: &mut Context<Self>) {
        let ix = NavMode::TABS
            .iter()
            .position(|m| *m == self.mode)
            .map_or(0, |i| (i + 1) % NavMode::TABS.len());
        self.set_mode(NavMode::TABS[ix], cx);
    }

    fn prev_mode(&mut self, _: &PrevMode, _: &mut Window, cx: &mut Context<Self>) {
        let len = NavMode::TABS.len();
        let ix = NavMode::TABS
            .iter()
            .position(|m| *m == self.mode)
            .map_or(0, |i| (i + len - 1) % len);
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
            NavTarget::Table(table) => cx.emit(DialogEvent::ModelFromTable(table)),
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
        .when(matched, |d| {
            d.text_color(theme::syntax(php_syntax::Highlight::Function))
        })
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
        let tabs = if self.mode.is_single() {
            vec![self.mode]
        } else {
            NavMode::TABS.to_vec()
        };
        let tabs = tabs.into_iter().map(|mode| {
            div()
                .id(SharedString::from(mode.label()))
                .px_2()
                .py_1()
                .rounded_sm()
                .cursor_pointer()
                .when(mode == self.mode, |d| {
                    d.bg(theme::selection()).text_color(theme::ui_text())
                })
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
                        elements.push(
                            div()
                                .px_3()
                                .pt_2()
                                .pb_1()
                                .text_size(px(11.))
                                .text_color(theme::muted())
                                .child(title)
                                .into_any_element(),
                        );
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
                            .child(
                                div()
                                    .w(px(16.))
                                    .flex_none()
                                    .text_color(theme::syntax(php_syntax::Highlight::Keyword))
                                    .child(item.icon),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_none()
                                    .text_color(theme::text())
                                    .children(highlighted(&item.label, &positions)),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_right()
                                    .text_color(theme::muted())
                                    .child(item.detail.clone()),
                            )
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.open(open_item.clone(), cx)),
                            )
                            .into_any_element(),
                    );
                }
            }
        }
        let loading =
            self.sources.files.is_none() && matches!(self.mode, NavMode::Files | NavMode::All);
        let selected_item = self.items(cx).into_iter().nth(selected);
        let preview = selected_item.as_ref().and_then(|item| self.preview(item));
        card(900.)
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
                    .when(!self.mode.is_single(), |d| {
                        let include = self.include_vendor;
                        d.child(super::checkbox("nav-vendor", "Include non-project items", include).on_click(cx.listener(move |this, _, _, cx| {
                            this.set_include_vendor(!include, cx);
                        })))
                    }),
            )
            .child(div().p_2().child(self.query.clone()))
            .when(loading, |d| {
                d.child(
                    div()
                        .px_3()
                        .text_size(px(12.))
                        .text_color(theme::muted())
                        .child("Indexing files…"),
                )
            })
            .child(
                div()
                    .pb_2()
                    .min_h(px(200.))
                    .font_family(theme::editor_font())
                    .text_size(px(12.))
                    .children(elements),
            )
            .when_some(preview, |d, (path, current, lines)| {
                // Code around the selected result, like PhpStorm's preview.
                d.child(
                    div()
                        .border_t_1()
                        .border_color(theme::border())
                        .bg(theme::editor_bg())
                        .py_1()
                        .font_family(theme::editor_font())
                        .text_size(px(12.))
                        .children(lines.into_iter().enumerate().map(|(i, line)| {
                            div()
                                .px_3()
                                .h(px(18.))
                                .whitespace_nowrap()
                                .overflow_hidden()
                                .text_color(theme::text())
                                .when(i == current, |d| d.bg(theme::active_row()))
                                .child(SharedString::from(if line.is_empty() { " ".to_string() } else { line }))
                        })),
                )
                .child(
                    div()
                        .px_3()
                        .py_1()
                        .border_t_1()
                        .border_color(theme::border())
                        .text_size(px(11.))
                        .text_color(theme::muted())
                        .child(path.display().to_string()),
                )
            })
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
        Self {
            field,
            focus_handle: cx.focus_handle(),
        }
    }

    fn confirm(&mut self, _: &Confirm, _: &mut Window, cx: &mut Context<Self>) {
        let text = self.field.read(cx).text().trim().to_string();
        let mut parts = text.split(':').map(|p| p.trim().parse::<usize>().ok());
        if let Some(Some(line)) = parts.next() {
            let column = parts.next().flatten().unwrap_or(1);
            cx.emit(DialogEvent::GoToLine(
                line.saturating_sub(1),
                column.saturating_sub(1),
            ));
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
