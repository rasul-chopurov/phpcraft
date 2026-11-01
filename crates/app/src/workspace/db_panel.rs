//! Database tool window (like PhpStorm's): data sources (MySQL, MariaDB, PostgreSQL,
//! SQLite) with their tables and columns, a SQL console, and a result grid whose cells can
//! be edited and submitted. Everything goes through PHP's PDO.

use std::collections::HashMap;
use std::sync::Arc;

use gpui::{AppContext, ClipboardItem, Context, Entity, Focusable, SharedString, Window, actions, div, prelude::*, px};
use laravel::database::{DbColumn, QueryResult};
use laravel::datasource::{self, CellEdit, DataSource, Driver};

use super::Workspace;
use crate::dialogs::DataSourceDialog;
use crate::editor_view::EditorView;
use crate::text_field::{FieldEvent, TextField};
use crate::theme;

actions!(workspace, [RunSql]);

/// The table the grid shows, so cells can be edited.
#[derive(Clone)]
struct GridTable {
    source: String,
    table: String,
    /// Primary key column names; editing needs at least one.
    keys: Vec<String>,
}

#[derive(Default)]
pub(super) struct DbState {
    sources: Vec<DataSource>,
    /// The source the console runs on.
    selected: Option<String>,
    tables: HashMap<String, Result<Vec<String>, String>>,
    collapsed: std::collections::HashSet<String>,
    columns: HashMap<(String, String), Option<Vec<(DbColumn, bool)>>>,
    filter: Option<Entity<TextField>>,
    console: Option<Entity<EditorView>>,
    result: Option<Result<Arc<QueryResult>, String>>,
    grid: Option<GridTable>,
    /// Pending cell edits: (row, column) -> new value (`None` = NULL).
    edits: HashMap<(usize, usize), Option<String>>,
    editing: Option<(usize, usize, Entity<TextField>)>,
    add_menu: bool,
    running: bool,
    status: Option<String>,
    _subscription: Option<gpui::Subscription>,
}

fn secrets_path() -> std::path::PathBuf {
    crate::config::dir().join("datasource_secrets.json")
}

fn secret_key(root: &std::path::Path, id: &str) -> String {
    format!("{}|{id}", root.display())
}

fn load_password(root: &std::path::Path, id: &str) -> String {
    std::fs::read_to_string(secrets_path())
        .ok()
        .and_then(|t| serde_json::from_str::<HashMap<String, String>>(&t).ok())
        .and_then(|m| m.get(&secret_key(root, id)).cloned())
        .unwrap_or_default()
}

fn save_password(root: &std::path::Path, id: &str, password: &str) {
    let mut map: HashMap<String, String> =
        std::fs::read_to_string(secrets_path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    map.insert(secret_key(root, id), password.to_string());
    let _ = std::fs::create_dir_all(crate::config::dir());
    let _ = std::fs::write(secrets_path(), serde_json::to_string_pretty(&map).unwrap_or_default());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(secrets_path(), std::fs::Permissions::from_mode(0o600));
    }
}

impl Workspace {
    pub(super) fn db_is_empty(&self) -> bool {
        self.db.sources.is_empty()
    }

    pub(super) fn ensure_db(&mut self, cx: &mut Context<Self>) {
        if self.db.console.is_some() {
            return;
        }
        let intel = self.intel.clone();
        let workspace = cx.entity().downgrade();
        let console = cx.new(|cx| EditorView::plain(intel, workspace, cx));
        console.update(cx, |editor, cx| editor.replace_all_text("SELECT 1", cx));
        self.db.console = Some(console);
        let filter = cx.new(|cx| TextField::new("Filter tables", cx));
        self.db._subscription = Some(cx.subscribe(&filter, |_, _, _: &FieldEvent, cx| cx.notify()));
        self.db.filter = Some(filter);
        self.db.sources = datasource::load(self.project.root());
        self.db.selected = self.db.sources.first().map(|s| s.id.clone());
        for id in self.db.sources.iter().map(|s| s.id.clone()).collect::<Vec<_>>() {
            self.load_tables(&id, cx);
        }
    }

    fn source(&self, id: &str) -> Option<DataSource> {
        self.db.sources.iter().find(|s| s.id == id).cloned()
    }

    fn load_tables(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(source) = self.source(id) else { return };
        let root = self.project.root().to_path_buf();
        let id = id.to_string();
        self.db.tables.remove(&id);
        cx.spawn(async move |this, cx| {
            let password = load_password(&root, &source.id);
            let result = cx.background_spawn(async move { datasource::tables(&root, &source, &password) }).await;
            let _ = this.update(cx, |this, cx| {
                this.db.tables.insert(id, result);
                cx.notify();
            });
        })
        .detach();
    }

    fn refresh_db(&mut self, cx: &mut Context<Self>) {
        self.db.columns.clear();
        for id in self.db.sources.iter().map(|s| s.id.clone()).collect::<Vec<_>>() {
            self.load_tables(&id, cx);
        }
        cx.notify();
    }

    fn open_source_dialog(&mut self, source: DataSource, password: String, window: &mut Window, cx: &mut Context<Self>) {
        self.db.add_menu = false;
        let root = self.project.root().to_path_buf();
        let dialog = cx.new(|cx| DataSourceDialog::new(root, source, password, window, cx));
        self.show_modal(dialog, window, cx);
    }

    /// From the dialog's OK.
    pub(super) fn save_data_source(&mut self, source: DataSource, password: String, cx: &mut Context<Self>) {
        let root = self.project.root().to_path_buf();
        save_password(&root, &source.id, &password);
        match self.db.sources.iter_mut().find(|s| s.id == source.id) {
            Some(existing) => *existing = source.clone(),
            None => self.db.sources.push(source.clone()),
        }
        let _ = datasource::save(&root, &self.db.sources);
        self.db.selected = Some(source.id.clone());
        self.load_tables(&source.id, cx);
        cx.notify();
    }

    fn remove_selected_source(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.db.selected.clone() else { return };
        self.db.sources.retain(|s| s.id != id);
        let _ = datasource::save(self.project.root(), &self.db.sources);
        self.db.tables.remove(&id);
        self.db.selected = self.db.sources.first().map(|s| s.id.clone());
        cx.notify();
    }

    /// "Add Application Database": the Laravel default connection.
    pub(super) fn add_application_database(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.db.add_menu = false;
        self.db.status = Some("Reading the Laravel database config…".into());
        let root = self.project.root().to_path_buf();
        cx.spawn_in(window, async move |this, cx| {
            let result = cx.background_spawn(async move { datasource::from_laravel(&root) }).await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.db.status = None;
                match result {
                    Ok((source, password)) => this.open_source_dialog(source, password, window, cx),
                    Err(err) => this.db.status = Some(err),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn toggle_columns(&mut self, id: String, table: String, cx: &mut Context<Self>) {
        let key = (id.clone(), table.clone());
        if self.db.columns.remove(&key).is_some() {
            cx.notify();
            return;
        }
        self.db.columns.insert(key.clone(), None);
        let Some(source) = self.source(&id) else { return };
        let root = self.project.root().to_path_buf();
        cx.spawn(async move |this, cx| {
            let password = load_password(&root, &source.id);
            let columns = cx.background_spawn(async move { datasource::columns(&root, &source, &password, &table) }).await;
            let _ = this.update(cx, |this, cx| {
                if let Some(slot) = this.db.columns.get_mut(&key) {
                    *slot = Some(columns.unwrap_or_default());
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Double-click on a table: its first rows, editable when it has a primary key.
    fn open_table_data(&mut self, id: String, table: String, cx: &mut Context<Self>) {
        let Some(source) = self.source(&id) else { return };
        self.db.selected = Some(id.clone());
        let driver = match source.driver {
            Driver::Mysql | Driver::Mariadb => "mysql",
            Driver::Pgsql => "pgsql",
            Driver::Sqlite => "sqlite",
        };
        let sql = laravel::database::select_sql(driver, &table, "", 200);
        if let Some(console) = &self.db.console {
            console.update(cx, |editor, cx| editor.replace_all_text(&sql, cx));
        }
        let root = self.project.root().to_path_buf();
        let password = load_password(&root, &id);
        let keys_table = table.clone();
        let keys_source = source.clone();
        let keys_root = root.clone();
        let keys_password = password.clone();
        self.run_query_on(source, password, sql, Some((id, table)), cx);
        // Primary keys decide whether the grid can be edited.
        cx.spawn(async move |this, cx| {
            let columns = cx.background_spawn(async move { datasource::columns(&keys_root, &keys_source, &keys_password, &keys_table) }).await;
            let _ = this.update(cx, |this, cx| {
                if let (Ok(columns), Some(grid)) = (columns, this.db.grid.as_mut()) {
                    grid.keys = columns.into_iter().filter(|(_, pk)| *pk).map(|(c, _)| c.name).collect();
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn run_sql(&mut self, _: &RunSql, _: &mut Window, cx: &mut Context<Self>) {
        self.execute_sql(cx);
    }

    fn execute_sql(&mut self, cx: &mut Context<Self>) {
        let Some(console) = &self.db.console else { return };
        let Some(source) = self.db.selected.clone().and_then(|id| self.source(&id)) else {
            self.db.result = Some(Err("Add a data source first (+).".into()));
            cx.notify();
            return;
        };
        let editor = console.read(cx);
        let selected = editor.buffer.selected_text();
        let sql = if selected.trim().is_empty() { editor.buffer.text() } else { selected };
        let sql = sql.trim().trim_end_matches(';').to_string();
        if sql.is_empty() {
            return;
        }
        let password = load_password(self.project.root(), &source.id);
        self.run_query_on(source, password, sql, None, cx);
    }

    fn run_query_on(&mut self, source: DataSource, password: String, sql: String, table: Option<(String, String)>, cx: &mut Context<Self>) {
        if self.db.running {
            return;
        }
        self.db.running = true;
        self.db.edits.clear();
        self.db.editing = None;
        self.db.grid = table.map(|(source, table)| GridTable { source, table, keys: Vec::new() });
        let root = self.project.root().to_path_buf();
        cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move { datasource::query(&root, &source, &password, &sql) }).await;
            let _ = this.update(cx, |this, cx| {
                this.db.running = false;
                this.db.result = Some(result.map(Arc::new));
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn start_edit(&mut self, row: usize, col: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Ok(result)) = &self.db.result else { return };
        if self.db.grid.as_ref().is_none_or(|g| g.keys.is_empty()) {
            self.db.status = Some("This result can't be edited: open a table that has a primary key.".into());
            cx.notify();
            return;
        }
        let current = self.db.edits.get(&(row, col)).cloned().unwrap_or_else(|| result.rows[row][col].clone());
        let field = cx.new(|cx| {
            let mut f = TextField::new("NULL", cx);
            f.set_text(current.unwrap_or_default(), cx);
            f
        });
        window.focus(&field.focus_handle(cx));
        self.db.editing = Some((row, col, field));
        cx.notify();
    }

    fn finish_edit(&mut self, null: bool, cx: &mut Context<Self>) {
        let Some((row, col, field)) = self.db.editing.take() else { return };
        let value = if null { None } else { Some(field.read(cx).text().to_string()) };
        let original = self.db.result.as_ref().and_then(|r| r.as_ref().ok()).map(|r| r.rows[row][col].clone());
        if original.as_ref() == Some(&value) {
            self.db.edits.remove(&(row, col));
        } else {
            self.db.edits.insert((row, col), value);
        }
        cx.notify();
    }

    /// Saves pending edits in one transaction (PhpStorm's Submit).
    fn submit_edits(&mut self, cx: &mut Context<Self>) {
        let (Some(grid), Some(Ok(result))) = (self.db.grid.clone(), self.db.result.clone()) else { return };
        let Some(source) = self.source(&grid.source) else { return };
        let key_columns: Vec<(String, usize)> =
            grid.keys.iter().filter_map(|k| result.columns.iter().position(|c| c == k).map(|i| (k.clone(), i))).collect();
        if key_columns.len() != grid.keys.len() {
            self.db.status = Some("The primary key columns must be part of the result.".into());
            cx.notify();
            return;
        }
        let edits: Vec<CellEdit> = self
            .db
            .edits
            .iter()
            .map(|((row, col), value)| CellEdit {
                column: result.columns[*col].clone(),
                value: value.clone(),
                key: key_columns.iter().map(|(k, i)| (k.clone(), result.rows[*row][*i].clone())).collect(),
            })
            .collect();
        let root = self.project.root().to_path_buf();
        let password = load_password(&root, &source.id);
        let table = grid.table.clone();
        let reload_source = source.clone();
        cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move { datasource::apply_edits(&root, &source, &password, &table, &edits) }).await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(n) => {
                        this.db.status = Some(format!("{n} row(s) updated"));
                        this.open_table_data(reload_source.id.clone(), grid.table.clone(), cx);
                    }
                    Err(err) => this.db.status = Some(err),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn copy_result(&self, cx: &mut Context<Self>) {
        let Some(Ok(result)) = &self.db.result else { return };
        let mut tsv = result.columns.join("\t");
        for row in &result.rows {
            tsv.push('\n');
            tsv.push_str(&row.iter().map(|c| c.clone().unwrap_or_else(|| "NULL".into())).collect::<Vec<_>>().join("\t"));
        }
        cx.write_to_clipboard(ClipboardItem::new_string(tsv));
    }

    pub(super) fn render_database(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let icon_button = |id: &'static str, label: &'static str, tip: &'static str| {
            div()
                .id(id)
                .px_1p5()
                .h(px(22.))
                .flex()
                .items_center()
                .rounded_md()
                .cursor_pointer()
                .text_color(theme::ui_text())
                .hover(|d| d.bg(theme::hover()))
                .child(label)
                .tooltip(super::layout::tooltip(tip))
        };
        let small = |id: &'static str, label: &'static str| {
            div()
                .id(id)
                .px_2()
                .h(px(22.))
                .flex()
                .items_center()
                .rounded_md()
                .cursor_pointer()
                .border_1()
                .border_color(theme::border_strong())
                .text_color(theme::ui_text())
                .hover(|d| d.bg(theme::hover()))
                .child(label)
        };
        let toolbar = div()
            .flex()
            .items_center()
            .gap_1()
            .px_2()
            .py_1()
            .child(icon_button("db-add", "+", "New data source").on_click(cx.listener(|this, _, _, cx| {
                this.db.add_menu = !this.db.add_menu;
                cx.notify();
            })))
            .child(icon_button("db-edit", "✎", "Data source properties").on_click(cx.listener(|this, _, window, cx| {
                if let Some(source) = this.db.selected.clone().and_then(|id| this.source(&id)) {
                    let password = load_password(this.project.root(), &source.id);
                    this.open_source_dialog(source, password, window, cx);
                }
            })))
            .child(icon_button("db-remove", "−", "Remove data source").on_click(cx.listener(|this, _, _, cx| this.remove_selected_source(cx))))
            .child(icon_button("db-refresh", "⟳", "Refresh").on_click(cx.listener(|this, _, _, cx| this.refresh_db(cx))))
            .child(div().flex_1())
            .child(icon_button("db-console", "▶", "Run console (⌘↵)").on_click(cx.listener(|this, _, _, cx| this.execute_sql(cx))));
        let add_menu = self.db.add_menu.then(|| {
            let item = |id: &'static str, label: &'static str| {
                div().id(id).px_3().h(px(24.)).flex().items_center().rounded_md().cursor_pointer().text_color(theme::ui_text()).hover(|d| d.bg(theme::selection())).child(label)
            };
            let mut menu = div().mx_2().mb_1().p_1().rounded_md().border_1().border_color(theme::border_strong()).bg(theme::panel_bg()).text_size(px(13.));
            for driver in Driver::ALL {
                menu = menu.child(item(driver.label(), driver.label()).on_click(cx.listener(move |this, _, window, cx| {
                    this.open_source_dialog(DataSource::new(driver), String::new(), window, cx)
                })));
            }
            if self.intel.is_laravel() {
                menu = menu
                    .child(div().my_1().h(px(1.)).bg(theme::border()))
                    .child(item("db-app", "Add Application Database…").on_click(cx.listener(|this, _, window, cx| this.add_application_database(window, cx))))
                    .child(item("db-env", "Import from .env").on_click(cx.listener(|this, _, window, cx| {
                        this.db.add_menu = false;
                        match datasource::from_env(this.project.root()) {
                            Ok((source, password)) => this.open_source_dialog(source, password, window, cx),
                            Err(err) => this.db.status = Some(err),
                        }
                        cx.notify();
                    })));
            }
            menu
        });
        let query = self.db.filter.as_ref().map(|f| f.read(cx).text().to_lowercase()).unwrap_or_default();
        let mut tree: Vec<gpui::AnyElement> = Vec::new();
        for source in &self.db.sources {
            let id = source.id.clone();
            let selected = self.db.selected.as_deref() == Some(id.as_str());
            let collapsed = self.db.collapsed.contains(&id);
            let toggle_id = id.clone();
            tree.push(
                div()
                    .id(SharedString::from(format!("db-source-{id}")))
                    .mx_1()
                    .px_2()
                    .h(px(24.))
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .rounded_md()
                    .cursor_pointer()
                    .when(selected, |d| d.bg(theme::active_row()))
                    .hover(|d| d.bg(theme::hover()))
                    .child(div().w(px(10.)).text_size(px(9.)).text_color(theme::muted()).child(if collapsed { "▶" } else { "▼" }))
                    .child(div().text_color(theme::syntax(php_syntax::Highlight::Type)).child("⛁"))
                    .child(div().text_color(theme::ui_text()).child(SharedString::from(source.name.clone())))
                    .child(div().text_size(px(11.)).text_color(theme::muted()).child(source.driver.label()))
                    .on_click(cx.listener(move |this, event: &gpui::ClickEvent, window, cx| {
                        this.db.selected = Some(toggle_id.clone());
                        if event.click_count() >= 2 {
                            if let Some(source) = this.source(&toggle_id) {
                                let password = load_password(this.project.root(), &source.id);
                                this.open_source_dialog(source, password, window, cx);
                            }
                        } else if !this.db.collapsed.remove(&toggle_id) {
                            this.db.collapsed.insert(toggle_id.clone());
                        }
                        cx.notify();
                    }))
                    .into_any_element(),
            );
            if collapsed {
                continue;
            }
            match self.db.tables.get(&id) {
                None => tree.push(div().pl(px(32.)).text_size(px(12.)).text_color(theme::muted()).child("Connecting…").into_any_element()),
                Some(Err(err)) => tree.push(
                    div().pl(px(32.)).pr_2().text_size(px(12.)).text_color(theme::error()).child(SharedString::from(err.chars().take(300).collect::<String>())).into_any_element(),
                ),
                Some(Ok(tables)) => {
                    for (ix, table) in tables.iter().filter(|t| query.is_empty() || t.to_lowercase().contains(&query)).enumerate() {
                        let key = (id.clone(), table.clone());
                        let expanded = self.db.columns.get(&key);
                        let (open_id, open_table) = key.clone();
                        tree.push(
                            div()
                                .id(SharedString::from(format!("db-t-{id}-{ix}")))
                                .mx_1()
                                .pl(px(26.))
                                .pr_2()
                                .h(px(22.))
                                .flex()
                                .items_center()
                                .gap_1p5()
                                .rounded_md()
                                .cursor_pointer()
                                .hover(|d| d.bg(theme::hover()))
                                .child(div().w(px(10.)).text_size(px(9.)).text_color(theme::muted()).child(if expanded.is_some() { "▼" } else { "▶" }))
                                .child(div().text_color(theme::syntax(php_syntax::Highlight::Type)).child("▦"))
                                .child(div().flex_1().text_color(theme::ui_text()).child(SharedString::from(table.clone())))
                                .on_click(cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                                    if event.click_count() >= 2 {
                                        this.open_table_data(open_id.clone(), open_table.clone(), cx);
                                    } else {
                                        this.toggle_columns(open_id.clone(), open_table.clone(), cx);
                                    }
                                }))
                                .into_any_element(),
                        );
                        match expanded {
                            Some(None) => tree.push(div().pl(px(56.)).text_size(px(12.)).text_color(theme::muted()).child("Loading…").into_any_element()),
                            Some(Some(columns)) => {
                                for (column, pk) in columns {
                                    tree.push(
                                        div()
                                            .pl(px(56.))
                                            .pr_2()
                                            .h(px(20.))
                                            .flex()
                                            .items_center()
                                            .gap_2()
                                            .text_size(px(12.))
                                            .child(div().w(px(10.)).text_color(theme::accent()).child(if *pk { "🔑" } else { "" }))
                                            .child(div().text_color(theme::text()).child(SharedString::from(column.name.clone())))
                                            .child(div().text_color(theme::muted()).child(SharedString::from(format!("{}{}", column.ty, if column.nullable { "?" } else { "" }))))
                                            .into_any_element(),
                                    );
                                }
                            }
                            None => {}
                        }
                    }
                }
            }
        }
        if self.db.sources.is_empty() {
            tree.push(
                div()
                    .p_3()
                    .text_size(px(12.))
                    .text_color(theme::muted())
                    .child(if self.intel.is_laravel() { "No data sources. Click + → Add Application Database, or add MySQL, MariaDB, PostgreSQL or SQLite." } else { "No data sources. Click + to add MySQL, MariaDB, PostgreSQL or SQLite." })
                    .into_any_element(),
            );
        }
        let editable = self.db.grid.as_ref().is_some_and(|g| !g.keys.is_empty());
        let result = match &self.db.result {
            None => div().p_2().text_size(px(12.)).text_color(theme::muted()).child("Double-click a table, or run a query.").into_any_element(),
            Some(Err(err)) => div().p_2().text_size(px(12.)).text_color(theme::error()).child(SharedString::from(err.clone())).into_any_element(),
            Some(Ok(result)) => {
                let width = px(150.);
                let header = div().flex().bg(theme::panel_bg()).border_b_1().border_color(theme::border()).children(result.columns.iter().map(|c| {
                    let key = self.db.grid.as_ref().is_some_and(|g| g.keys.contains(c));
                    div().w(width).flex_none().px_2().py_1().overflow_hidden().whitespace_nowrap().text_color(theme::ui_text()).child(SharedString::from(format!("{}{c}", if key { "🔑 " } else { "" })))
                }));
                let rows = result.rows.iter().enumerate().map(|(r, row)| {
                    div().flex().border_b_1().border_color(theme::border()).children(row.iter().enumerate().map(|(c, cell)| {
                        if let Some((er, ec, field)) = &self.db.editing
                            && (*er, *ec) == (r, c)
                        {
                            return div()
                                .w(width * 2.)
                                .flex_none()
                                .flex()
                                .gap_1()
                                .p(px(1.))
                                .key_context("DbCellEdit")
                                .on_action(cx.listener(|this, _: &crate::dialogs::Confirm, _, cx| this.finish_edit(false, cx)))
                                .on_action(cx.listener(|this, _: &crate::dialogs::Cancel, _, cx| {
                                    this.db.editing = None;
                                    cx.notify();
                                }))
                                .child(div().flex_1().child(field.clone()))
                                .child(div().id("db-null").px_1().cursor_pointer().text_color(theme::muted()).child("NULL").on_click(cx.listener(|this, _, _, cx| this.finish_edit(true, cx))))
                                .child(div().id("db-ok").px_1().cursor_pointer().text_color(theme::accent()).child("✓").on_click(cx.listener(|this, _, _, cx| this.finish_edit(false, cx))))
                                .into_any_element();
                        }
                        let pending = self.db.edits.get(&(r, c));
                        let value = pending.cloned().unwrap_or_else(|| cell.clone());
                        let (text, null) = match &value {
                            Some(text) => (text.chars().take(60).collect::<String>().replace('\n', " "), false),
                            None => ("NULL".to_string(), true),
                        };
                        div()
                            .id(("db-cell", r * 10_000 + c))
                            .w(width)
                            .flex_none()
                            .px_2()
                            .py(px(2.))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .when(pending.is_some(), |d| d.bg(theme::accent().opacity(0.25)))
                            .text_color(if null { theme::muted() } else { theme::text() })
                            .when(editable, |d| d.cursor_pointer().hover(|d| d.bg(theme::hover())))
                            .child(SharedString::from(text))
                            .on_click(cx.listener(move |this, event: &gpui::ClickEvent, window, cx| {
                                if event.click_count() >= 2 {
                                    this.start_edit(r, c, window, cx);
                                }
                            }))
                            .into_any_element()
                    }))
                });
                let pending = self.db.edits.len();
                div()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .px_2()
                            .py_1()
                            .text_size(px(11.))
                            .text_color(theme::muted())
                            .child(div().flex_1().child(if result.columns.is_empty() {
                                format!("{} row(s) affected · {:.0} ms", result.affected, result.millis)
                            } else if result.total > result.rows.len() {
                                format!("{} rows (showing {}) · {:.0} ms", result.total, result.rows.len(), result.millis)
                            } else {
                                format!("{} rows · {:.0} ms{}", result.total, result.millis, if editable { " · double-click a cell to edit" } else { "" })
                            }))
                            .when(pending > 0, |d| {
                                d.child(small("db-revert", "Revert").on_click(cx.listener(|this, _, _, cx| {
                                    this.db.edits.clear();
                                    cx.notify();
                                })))
                                .child(small("db-submit", "Submit").bg(theme::accent()).text_color(gpui::white()).on_click(cx.listener(|this, _, _, cx| this.submit_edits(cx))))
                            }),
                    )
                    .child(
                        div()
                            .id("db-grid")
                            .overflow_x_scroll()
                            .font_family(theme::editor_font())
                            .text_size(px(12.))
                            .child(div().flex().flex_col().min_w(width * result.columns.len().max(1) as f32).child(header).children(rows)),
                    )
                    .into_any_element()
            }
        };
        let running = self.db.running;
        let console_target = self.db.selected.clone().and_then(|id| self.source(&id)).map(|s| s.name).unwrap_or_else(|| "no data source".into());
        div()
            .flex_1()
            .min_h(px(0.))
            .flex()
            .flex_col()
            .text_size(px(13.))
            .child(toolbar)
            .children(add_menu)
            .children(self.db.filter.clone().map(|f| div().px_2().pb_1().child(f)))
            .child(div().id("db-tree").h(px(220.)).flex_none().overflow_y_scroll().border_b_1().border_color(theme::border()).children(tree))
            .child(
                div()
                    .key_context("DbConsole")
                    .on_action(cx.listener(Self::run_sql))
                    .p_2()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(div().text_size(px(11.)).text_color(theme::muted()).child(format!("Console · {console_target}")))
                    .child(div().h(px(90.)).rounded_md().border_1().border_color(theme::border_strong()).overflow_hidden().children(self.db.console.clone()))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(div().flex_1().text_size(px(11.)).text_color(theme::muted()).child(crate::keymap::display("secondary-enter") + " runs the query (or the selection)"))
                            .child(small("db-copy", "Copy TSV").on_click(cx.listener(|this, _, _, cx| this.copy_result(cx))))
                            .child(small("db-run", if running { "Running…" } else { "▶ Run" }).on_click(cx.listener(|this, _, _, cx| this.execute_sql(cx)))),
                    ),
            )
            .when_some(self.db.status.clone(), |d, status| {
                d.child(
                    div()
                        .id("db-status")
                        .px_2()
                        .py_1()
                        .text_size(px(12.))
                        .text_color(theme::muted())
                        .child(status)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.db.status = None;
                            cx.notify();
                        })),
                )
            })
            .child(div().id("db-result").flex_1().min_h(px(0.)).overflow_y_scroll().child(result))
    }
}
