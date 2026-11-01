//! Table editor tab (PhpStorm's data editor): a database table's rows with WHERE / ORDER BY
//! filters, paging, cell editing, added and deleted rows, and Submit in one transaction.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use gpui::{App, AppContext, ClipboardItem, Context, Entity, FocusHandle, Focusable, SharedString, Window, div, prelude::*, px};
use laravel::database::{DbColumn, QueryResult};
use laravel::datasource::{self, DataSource, Driver, Statement};

use crate::text_field::TextField;
use crate::theme;

const PAGE: usize = 200;

pub struct TableView {
    root: PathBuf,
    source: DataSource,
    password: String,
    pub table: String,
    columns: Vec<(DbColumn, bool)>,
    result: Option<Result<Arc<QueryResult>, String>>,
    where_field: Entity<TextField>,
    order_field: Entity<TextField>,
    offset: usize,
    edits: HashMap<(usize, usize), Option<String>>,
    deleted: HashSet<usize>,
    /// New rows (all columns, `None` = default/NULL).
    added: Vec<Vec<Option<String>>>,
    /// (row, column, field); rows past the result are new rows.
    editing: Option<(usize, usize, Entity<TextField>)>,
    selected: Option<usize>,
    loading: bool,
    status: Option<String>,
    focus_handle: FocusHandle,
}

fn laravel_driver(driver: Driver) -> &'static str {
    match driver {
        Driver::Mysql | Driver::Mariadb => "mysql",
        Driver::Pgsql => "pgsql",
        Driver::Sqlite => "sqlite",
    }
}

impl TableView {
    pub fn new(root: PathBuf, source: DataSource, password: String, table: String, cx: &mut Context<Self>) -> Self {
        let where_field = cx.new(|cx| TextField::new("WHERE", cx));
        let order_field = cx.new(|cx| TextField::new("ORDER BY", cx));
        let mut view = Self {
            root,
            source,
            password,
            table,
            columns: Vec::new(),
            result: None,
            where_field,
            order_field,
            offset: 0,
            edits: HashMap::new(),
            deleted: HashSet::new(),
            added: Vec::new(),
            editing: None,
            selected: None,
            loading: false,
            status: None,
            focus_handle: cx.focus_handle(),
        };
        view.load_columns(cx);
        view.reload(cx);
        view
    }

    pub fn title(&self) -> String {
        format!("{} [{}]", self.table, self.source.name)
    }

    fn load_columns(&mut self, cx: &mut Context<Self>) {
        let (root, source, password, table) = (self.root.clone(), self.source.clone(), self.password.clone(), self.table.clone());
        cx.spawn(async move |this, cx| {
            let columns = cx.background_spawn(async move { datasource::columns(&root, &source, &password, &table) }).await;
            let _ = this.update(cx, |view, cx| {
                view.columns = columns.unwrap_or_default();
                cx.notify();
            });
        })
        .detach();
    }

    fn sql(&self, cx: &App) -> String {
        let quote = |n: &str| laravel::database::quote_ident(laravel_driver(self.source.driver), n);
        let filter = self.where_field.read(cx).text().trim().to_string();
        let order = self.order_field.read(cx).text().trim().to_string();
        let mut sql = format!("SELECT * FROM {}", quote(&self.table));
        if !filter.is_empty() {
            sql.push_str(&format!(" WHERE {filter}"));
        }
        if !order.is_empty() {
            sql.push_str(&format!(" ORDER BY {order}"));
        }
        sql.push_str(&format!(" LIMIT {PAGE} OFFSET {}", self.offset));
        sql
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        let sql = self.sql(cx);
        let (root, source, password) = (self.root.clone(), self.source.clone(), self.password.clone());
        self.loading = true;
        self.edits.clear();
        self.deleted.clear();
        self.added.clear();
        self.editing = None;
        self.selected = None;
        cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move { datasource::query(&root, &source, &password, &sql) }).await;
            let _ = this.update(cx, |view, cx| {
                view.loading = false;
                view.result = Some(result.map(Arc::new));
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn keys(&self) -> Vec<String> {
        self.columns.iter().filter(|(_, pk)| *pk).map(|(c, _)| c.name.clone()).collect()
    }

    fn rows(&self) -> usize {
        self.result.as_ref().and_then(|r| r.as_ref().ok()).map_or(0, |r| r.rows.len())
    }

    fn pending(&self) -> usize {
        self.edits.len() + self.deleted.len() + self.added.len()
    }

    fn add_row(&mut self, cx: &mut Context<Self>) {
        let Some(Ok(result)) = &self.result else { return };
        self.added.push(vec![None; result.columns.len()]);
        cx.notify();
    }

    fn delete_selected(&mut self, cx: &mut Context<Self>) {
        let Some(row) = self.selected else { return };
        let rows = self.rows();
        if row >= rows {
            if row - rows < self.added.len() {
                self.added.remove(row - rows);
            }
        } else if !self.deleted.remove(&row) {
            self.deleted.insert(row);
        }
        cx.notify();
    }

    fn start_edit(&mut self, row: usize, col: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Ok(result)) = &self.result else { return };
        let rows = result.rows.len();
        if row < rows && self.keys().is_empty() {
            self.status = Some("Rows can't be edited: the table has no primary key.".into());
            cx.notify();
            return;
        }
        let current = if row < rows {
            self.edits.get(&(row, col)).cloned().unwrap_or_else(|| result.rows[row][col].clone())
        } else {
            self.added.get(row - rows).and_then(|r| r.get(col).cloned()).flatten()
        };
        let field = cx.new(|cx| {
            let mut f = TextField::new("NULL", cx);
            f.set_text(current.unwrap_or_default(), cx);
            f
        });
        window.focus(&field.focus_handle(cx));
        self.editing = Some((row, col, field));
        cx.notify();
    }

    fn finish_edit(&mut self, null: bool, cx: &mut Context<Self>) {
        let Some((row, col, field)) = self.editing.take() else { return };
        let value = if null { None } else { Some(field.read(cx).text().to_string()) };
        let rows = self.rows();
        if row >= rows {
            if let Some(new_row) = self.added.get_mut(row - rows) {
                new_row[col] = value;
            }
        } else {
            let original = self.result.as_ref().and_then(|r| r.as_ref().ok()).map(|r| r.rows[row][col].clone());
            if original.as_ref() == Some(&value) {
                self.edits.remove(&(row, col));
            } else {
                self.edits.insert((row, col), value);
            }
        }
        cx.notify();
    }

    /// Builds UPDATE / DELETE / INSERT statements for the pending changes.
    fn statements(&self) -> Result<Vec<Statement>, String> {
        let Some(Ok(result)) = &self.result else { return Ok(Vec::new()) };
        let keys = self.keys();
        let key_of = |row: usize| -> Result<Vec<(String, Option<String>)>, String> {
            keys.iter()
                .map(|k| {
                    let i = result.columns.iter().position(|c| c == k).ok_or("The primary key must be part of the rows")?;
                    Ok((k.clone(), result.rows[row][i].clone()))
                })
                .collect()
        };
        let driver = self.source.driver;
        let mut statements = Vec::new();
        for ((row, col), value) in &self.edits {
            if self.deleted.contains(row) {
                continue;
            }
            let edit = datasource::CellEdit { column: result.columns[*col].clone(), value: value.clone(), key: key_of(*row)? };
            let (sql, params) = datasource::update_statement(driver, &self.table, &edit);
            statements.push(Statement { sql, params });
        }
        for row in &self.deleted {
            statements.push(datasource::delete_statement(driver, &self.table, &key_of(*row)?));
        }
        for new_row in &self.added {
            let values: Vec<(String, Option<String>)> =
                result.columns.iter().cloned().zip(new_row.iter().cloned()).filter(|(_, v)| v.is_some()).collect();
            statements.push(datasource::insert_statement(driver, &self.table, &values));
        }
        Ok(statements)
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        let statements = match self.statements() {
            Ok(s) if !s.is_empty() => s,
            Ok(_) => return,
            Err(err) => {
                self.status = Some(err);
                cx.notify();
                return;
            }
        };
        let (root, source, password) = (self.root.clone(), self.source.clone(), self.password.clone());
        cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move { datasource::execute(&root, &source, &password, &statements) }).await;
            let _ = this.update(cx, |view, cx| {
                match result {
                    Ok(n) => {
                        view.status = Some(format!("Submitted · {n} row(s) affected"));
                        view.reload(cx);
                    }
                    Err(err) => view.status = Some(err),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn copy_csv(&mut self, cx: &mut Context<Self>) {
        let Some(Ok(result)) = &self.result else { return };
        let escape = |v: &str| if v.contains([',', '"', '\n']) { format!("\"{}\"", v.replace('"', "\"\"")) } else { v.to_string() };
        let mut csv = result.columns.iter().map(|c| escape(c)).collect::<Vec<_>>().join(",");
        for row in &result.rows {
            csv.push('\n');
            csv.push_str(&row.iter().map(|c| c.as_deref().map(escape).unwrap_or_default()).collect::<Vec<_>>().join(","));
        }
        cx.write_to_clipboard(ClipboardItem::new_string(csv));
        self.status = Some("Copied as CSV".into());
        cx.notify();
    }
}

impl Focusable for TableView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for TableView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tool = |id: &'static str, label: &'static str, tip: &'static str, enabled: bool| {
            div()
                .id(id)
                .px_1p5()
                .h(px(24.))
                .flex()
                .items_center()
                .rounded_md()
                .text_color(if enabled { theme::ui_text() } else { theme::muted() })
                .when(enabled, |d| d.cursor_pointer().hover(|d| d.bg(theme::hover())))
                .child(label)
                .tooltip(move |_, cx| cx.new(|_| crate::dialogs::TextTooltip(tip.into())).into())
        };
        let pending = self.pending();
        let has_result = matches!(self.result, Some(Ok(_)));
        let toolbar = div()
            .flex()
            .items_center()
            .gap_1()
            .px_2()
            .h(px(32.))
            .border_b_1()
            .border_color(theme::border())
            .child(tool("tv-reload", "⟳", "Reload", true).on_click(cx.listener(|this, _, _, cx| this.reload(cx))))
            .child(tool("tv-add", "+", "Add row", has_result).on_click(cx.listener(|this, _, _, cx| this.add_row(cx))))
            .child(tool("tv-del", "−", "Delete selected row", self.selected.is_some()).on_click(cx.listener(|this, _, _, cx| this.delete_selected(cx))))
            .child(tool("tv-revert", "↶", "Revert changes", pending > 0).on_click(cx.listener(|this, _, _, cx| {
                this.edits.clear();
                this.deleted.clear();
                this.added.clear();
                cx.notify();
            })))
            .child(
                div()
                    .id("tv-submit")
                    .px_2()
                    .h(px(22.))
                    .flex()
                    .items_center()
                    .rounded_md()
                    .when(pending > 0, |d| d.bg(theme::accent()).text_color(gpui::white()).cursor_pointer())
                    .when(pending == 0, |d| d.text_color(theme::muted()))
                    .child(if pending > 0 { format!("⇧ Submit ({pending})") } else { "⇧ Submit".into() })
                    .on_click(cx.listener(|this, _, _, cx| this.submit(cx))),
            )
            .child(div().w(px(12.)))
            .child(tool("tv-prev", "‹", "Previous page", self.offset > 0).on_click(cx.listener(|this, _, _, cx| {
                if this.offset > 0 {
                    this.offset = this.offset.saturating_sub(PAGE);
                    this.reload(cx);
                }
            })))
            .child(div().text_size(px(12.)).text_color(theme::muted()).child(format!("{}–{}", self.offset + 1, self.offset + self.rows())))
            .child(tool("tv-next", "›", "Next page", self.rows() == PAGE).on_click(cx.listener(|this, _, _, cx| {
                if this.rows() == PAGE {
                    this.offset += PAGE;
                    this.reload(cx);
                }
            })))
            .child(div().flex_1())
            .when_some(self.status.clone(), |d, s| d.child(div().text_size(px(12.)).text_color(theme::muted()).child(s)))
            .child(tool("tv-csv", "CSV", "Copy all rows as CSV", has_result).on_click(cx.listener(|this, _, _, cx| this.copy_csv(cx))));
        let filters = div()
            .flex()
            .items_center()
            .border_b_1()
            .border_color(theme::border())
            .child(
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .gap_1()
                    .px_2()
                    .border_r_1()
                    .border_color(theme::border())
                    .key_context("TableFilter")
                    .on_action(cx.listener(|this, _: &crate::dialogs::Confirm, _, cx| {
                        this.offset = 0;
                        this.reload(cx);
                    }))
                    .child(div().text_color(theme::muted()).text_size(px(12.)).child("WHERE"))
                    .child(div().flex_1().child(self.where_field.clone())),
            )
            .child(
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .gap_1()
                    .px_2()
                    .key_context("TableFilter")
                    .on_action(cx.listener(|this, _: &crate::dialogs::Confirm, _, cx| {
                        this.offset = 0;
                        this.reload(cx);
                    }))
                    .child(div().text_color(theme::muted()).text_size(px(12.)).child("ORDER BY"))
                    .child(div().flex_1().child(self.order_field.clone())),
            );
        let grid: gpui::AnyElement = match &self.result {
            None => div().p_3().text_color(theme::muted()).child("Loading…").into_any_element(),
            Some(Err(err)) => div().p_3().text_color(theme::error()).child(SharedString::from(err.clone())).into_any_element(),
            Some(Ok(result)) => {
                let width = px(170.);
                let number_width = px(56.);
                let keys = self.keys();
                let types: HashMap<&str, &str> = self.columns.iter().map(|(c, _)| (c.name.as_str(), c.ty.as_str())).collect();
                let header = div()
                    .flex()
                    .bg(theme::panel_bg())
                    .border_b_1()
                    .border_color(theme::border())
                    .child(div().w(number_width).flex_none())
                    .children(result.columns.iter().map(|c| {
                        let key = keys.contains(c);
                        div()
                            .w(width)
                            .flex_none()
                            .px_2()
                            .py_1()
                            .flex()
                            .gap_1()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .child(div().text_color(theme::ui_text()).child(SharedString::from(format!("{}{c}", if key { "🔑 " } else { "" }))))
                            .child(div().text_size(px(10.)).text_color(theme::muted()).child(SharedString::from(types.get(c.as_str()).copied().unwrap_or("").to_string())))
                    }));
                let total = result.rows.len() + self.added.len();
                let rows = (0..total).map(|r| {
                    let is_new = r >= result.rows.len();
                    let values: Vec<Option<String>> = if is_new { self.added[r - result.rows.len()].clone() } else { result.rows[r].clone() };
                    let deleted = self.deleted.contains(&r);
                    let selected = self.selected == Some(r);
                    div()
                        .flex()
                        .border_b_1()
                        .border_color(theme::border())
                        .when(deleted, |d| d.bg(theme::error().opacity(0.15)))
                        .when(is_new, |d| d.bg(theme::success().opacity(0.12)))
                        .when(selected, |d| d.bg(theme::selection()))
                        .child(
                            div()
                                .id(("tv-num", r))
                                .w(number_width)
                                .flex_none()
                                .pr_2()
                                .text_right()
                                .cursor_pointer()
                                .text_color(theme::line_number())
                                .child(if is_new { "+".to_string() } else { (self.offset + r + 1).to_string() })
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.selected = if this.selected == Some(r) { None } else { Some(r) };
                                    cx.notify();
                                })),
                        )
                        .children(values.into_iter().enumerate().map(|(c, cell)| {
                            if let Some((er, ec, field)) = &self.editing
                                && (*er, *ec) == (r, c)
                            {
                                return div()
                                    .w(width * 1.6)
                                    .flex_none()
                                    .flex()
                                    .gap_1()
                                    .key_context("DbCellEdit")
                                    .on_action(cx.listener(|this, _: &crate::dialogs::Confirm, _, cx| this.finish_edit(false, cx)))
                                    .on_action(cx.listener(|this, _: &crate::dialogs::Cancel, _, cx| {
                                        this.editing = None;
                                        cx.notify();
                                    }))
                                    .child(div().flex_1().child(field.clone()))
                                    .child(div().id("tv-null").px_1().cursor_pointer().text_color(theme::muted()).child("NULL").on_click(cx.listener(|this, _, _, cx| this.finish_edit(true, cx))))
                                    .into_any_element();
                            }
                            let pending = self.edits.get(&(r, c));
                            let value = pending.cloned().unwrap_or(cell);
                            let (text, null) = match &value {
                                Some(text) => (text.chars().take(80).collect::<String>().replace('\n', " "), false),
                                None => ("<null>".to_string(), true),
                            };
                            div()
                                .id(("tv-cell", r * 10_000 + c))
                                .w(width)
                                .flex_none()
                                .px_2()
                                .py(px(3.))
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .cursor_pointer()
                                .when(pending.is_some(), |d| d.bg(theme::accent().opacity(0.25)))
                                .when(deleted, |d| d.line_through())
                                .text_color(if null { theme::muted() } else { theme::text() })
                                .hover(|d| d.bg(theme::hover()))
                                .child(SharedString::from(text))
                                .on_click(cx.listener(move |this, event: &gpui::ClickEvent, window, cx| {
                                    this.selected = Some(r);
                                    if event.click_count() >= 2 {
                                        this.start_edit(r, c, window, cx);
                                    }
                                    cx.notify();
                                }))
                                .into_any_element()
                        }))
                });
                div()
                    .id("tv-grid")
                    .size_full()
                    .overflow_scroll()
                    .font_family(theme::editor_font())
                    .text_size(px(12.))
                    .child(div().flex().flex_col().min_w(number_width + width * result.columns.len().max(1) as f32).child(header).children(rows))
                    .into_any_element()
            }
        };
        div()
            .track_focus(&self.focus_handle)
            .size_full()
            .flex()
            .flex_col()
            .bg(theme::editor_bg())
            .text_size(px(13.))
            .child(toolbar)
            .child(filters)
            .child(div().flex_1().min_h(px(0.)).child(grid))
            .when(self.loading, |d| d.child(div().px_2().py_1().text_size(px(11.)).text_color(theme::muted()).child("Loading…")))
    }
}
