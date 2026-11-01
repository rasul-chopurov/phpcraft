//! Database tool window (like PhpStorm's): the project's Laravel connection, its tables and
//! columns, and a SQL console with a result grid. Queries go through the app's own
//! connection (PDO), so every driver Laravel supports works.

use std::collections::HashMap;
use std::sync::Arc;

use gpui::{AppContext, ClipboardItem, Context, Entity, SharedString, Window, actions, div, prelude::*, px};
use laravel::database::{DbColumn, QueryResult};

use super::Workspace;
use crate::editor_view::EditorView;
use crate::text_field::{FieldEvent, TextField};
use crate::theme;

actions!(workspace, [RunSql]);

#[derive(Default)]
pub(super) struct DbState {
    loaded: bool,
    info: Option<Result<(String, String), String>>,
    tables: Option<Result<Vec<String>, String>>,
    expanded: HashMap<String, Option<Vec<DbColumn>>>,
    filter: Option<Entity<TextField>>,
    console: Option<Entity<EditorView>>,
    result: Option<Result<Arc<QueryResult>, String>>,
    running: bool,
    _subscription: Option<gpui::Subscription>,
}

impl Workspace {
    /// Loads the connection and table list the first time the window shows.
    pub(super) fn ensure_db(&mut self, cx: &mut Context<Self>) {
        if self.db.console.is_none() {
            let intel = self.intel.clone();
            let workspace = cx.entity().downgrade();
            let console = cx.new(|cx| EditorView::plain(intel, workspace, cx));
            console.update(cx, |editor, cx| editor.replace_all_text("SELECT 1", cx));
            self.db.console = Some(console);
            let filter = cx.new(|cx| TextField::new("Filter tables", cx));
            self.db._subscription = Some(cx.subscribe(&filter, |_, _, _: &FieldEvent, cx| cx.notify()));
            self.db.filter = Some(filter);
        }
        if self.db.loaded {
            return;
        }
        self.db.loaded = true;
        let root = self.project.root().to_path_buf();
        cx.spawn(async move |this, cx| {
            let (info, tables) = cx
                .background_spawn(async move { (laravel::database::connection_info(&root), laravel::database::list_tables(&root)) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.db.info = Some(info);
                this.db.tables = Some(tables);
                cx.notify();
            });
        })
        .detach();
    }

    fn refresh_db(&mut self, cx: &mut Context<Self>) {
        self.db.loaded = false;
        self.db.info = None;
        self.db.tables = None;
        self.db.expanded.clear();
        self.ensure_db(cx);
        cx.notify();
    }

    fn driver(&self) -> String {
        self.db.info.as_ref().and_then(|i| i.as_ref().ok()).map(|(d, _)| d.clone()).unwrap_or_default()
    }

    fn toggle_table(&mut self, table: String, cx: &mut Context<Self>) {
        if self.db.expanded.remove(&table).is_some() {
            cx.notify();
            return;
        }
        self.db.expanded.insert(table.clone(), None);
        let root = self.project.root().to_path_buf();
        cx.spawn(async move |this, cx| {
            let name = table.clone();
            let columns = cx.background_spawn(async move { laravel::database::table_columns(&root, &name) }).await;
            let _ = this.update(cx, |this, cx| {
                if let Some(slot) = this.db.expanded.get_mut(&table) {
                    *slot = Some(columns.unwrap_or_default());
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Shows a table's first rows (PhpStorm's double-click on a table).
    fn open_table_data(&mut self, table: &str, cx: &mut Context<Self>) {
        let sql = laravel::database::select_sql(&self.driver(), table, "", 100);
        if let Some(console) = &self.db.console {
            console.update(cx, |editor, cx| editor.replace_all_text(&sql, cx));
        }
        self.execute_sql(cx);
    }

    pub(super) fn run_sql(&mut self, _: &RunSql, _: &mut Window, cx: &mut Context<Self>) {
        self.execute_sql(cx);
    }

    fn execute_sql(&mut self, cx: &mut Context<Self>) {
        let Some(console) = &self.db.console else { return };
        let editor = console.read(cx);
        // The selection runs on its own, like "Execute Selection".
        let selected = editor.buffer.selected_text();
        let sql = if selected.trim().is_empty() { editor.buffer.text() } else { selected };
        let sql = sql.trim().trim_end_matches(';').to_string();
        if sql.is_empty() || self.db.running {
            return;
        }
        self.db.running = true;
        let root = self.project.root().to_path_buf();
        cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move { laravel::database::run_query(&root, &sql) }).await;
            let _ = this.update(cx, |this, cx| {
                this.db.running = false;
                this.db.result = Some(result.map(Arc::new));
                cx.notify();
            });
        })
        .detach();
        cx.notify();
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
        let info = match &self.db.info {
            None => "Connecting…".to_string(),
            Some(Ok((driver, db))) => format!("{driver} · {}", db.rsplit('/').next().unwrap_or(db)),
            Some(Err(err)) => err.lines().next().unwrap_or("").chars().take(120).collect(),
        };
        let query = self.db.filter.as_ref().map(|f| f.read(cx).text().to_lowercase()).unwrap_or_default();
        let mut tree: Vec<gpui::AnyElement> = Vec::new();
        if let Some(Ok(tables)) = &self.db.tables {
            for (ix, table) in tables.iter().filter(|t| query.is_empty() || t.to_lowercase().contains(&query)).enumerate() {
                let expanded = self.db.expanded.get(table);
                let name = table.clone();
                let open = table.clone();
                tree.push(
                    div()
                        .id(("db-table", ix))
                        .mx_1()
                        .px_2()
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
                                this.open_table_data(&open, cx);
                            } else {
                                this.toggle_table(name.clone(), cx);
                            }
                        }))
                        .into_any_element(),
                );
                match expanded {
                    Some(None) => tree.push(div().pl(px(40.)).text_size(px(12.)).text_color(theme::muted()).child("Loading…").into_any_element()),
                    Some(Some(columns)) => {
                        for column in columns {
                            tree.push(
                                div()
                                    .pl(px(40.))
                                    .pr_2()
                                    .h(px(20.))
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .text_size(px(12.))
                                    .child(div().text_color(theme::text()).child(SharedString::from(column.name.clone())))
                                    .child(
                                        div().text_color(theme::muted()).child(SharedString::from(format!("{}{}", column.ty, if column.nullable { "?" } else { "" }))),
                                    )
                                    .into_any_element(),
                            );
                        }
                    }
                    None => {}
                }
            }
        } else if let Some(Err(err)) = &self.db.tables {
            tree.push(div().p_2().text_size(px(12.)).text_color(theme::error()).child(SharedString::from(err.chars().take(400).collect::<String>())).into_any_element());
        }
        let result = match &self.db.result {
            None => div().p_2().text_size(px(12.)).text_color(theme::muted()).child("Double-click a table or run a query.").into_any_element(),
            Some(Err(err)) => div().p_2().text_size(px(12.)).text_color(theme::error()).child(SharedString::from(err.clone())).into_any_element(),
            Some(Ok(result)) => {
                let width = px(150.);
                let header = div().flex().bg(theme::panel_bg()).border_b_1().border_color(theme::border()).children(
                    result.columns.iter().map(|c| {
                        div().w(width).flex_none().px_2().py_1().overflow_hidden().whitespace_nowrap().text_color(theme::ui_text()).child(SharedString::from(c.clone()))
                    }),
                );
                let rows = result.rows.iter().map(|row| {
                    div().flex().border_b_1().border_color(theme::border()).children(row.iter().map(|cell| {
                        let (text, null) = match cell {
                            Some(text) => (text.chars().take(60).collect::<String>().replace('\n', " "), false),
                            None => ("NULL".to_string(), true),
                        };
                        div()
                            .w(width)
                            .flex_none()
                            .px_2()
                            .py(px(2.))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_color(if null { theme::muted() } else { theme::text() })
                            .child(SharedString::from(text))
                    }))
                });
                div()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .px_2()
                            .py_1()
                            .text_size(px(11.))
                            .text_color(theme::muted())
                            .child(if result.columns.is_empty() {
                                format!("{} row(s) affected · {:.0} ms", result.affected, result.millis)
                            } else if result.total > result.rows.len() {
                                format!("{} rows (showing {}) · {:.0} ms", result.total, result.rows.len(), result.millis)
                            } else {
                                format!("{} rows · {:.0} ms", result.total, result.millis)
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
        div()
            .flex_1()
            .min_h(px(0.))
            .flex()
            .flex_col()
            .text_size(px(13.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .child(div().text_color(theme::syntax(php_syntax::Highlight::Type)).child("⛁"))
                    .child(div().flex_1().text_size(px(12.)).text_color(theme::muted()).overflow_hidden().whitespace_nowrap().child(info))
                    .child(small("db-refresh", "⟳").tooltip(super::layout::tooltip("Refresh")).on_click(cx.listener(|this, _, _, cx| this.refresh_db(cx)))),
            )
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
            .child(div().id("db-result").flex_1().min_h(px(0.)).overflow_y_scroll().child(result))
    }
}
