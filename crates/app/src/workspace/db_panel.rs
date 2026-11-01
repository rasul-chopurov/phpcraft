//! Database tool window (like PhpStorm's): data sources (MySQL, MariaDB, PostgreSQL,
//! SQLite) with their tables and columns, a SQL console, and a result grid whose cells can
//! be edited and submitted. Everything goes through PHP's PDO.

use std::collections::HashMap;

use gpui::{AppContext, Context, Entity, SharedString, Window, actions, div, prelude::*, px};
use laravel::database::DbColumn;
use laravel::datasource::{self, DataSource, Driver};

use super::Workspace;
use crate::dialogs::DataSourceDialog;
use crate::text_field::{FieldEvent, TextField};
use crate::icons::{Icon, icon};
use crate::theme;

actions!(workspace, [RunSql]);

#[derive(Default)]
pub(super) struct DbState {
    sources: Vec<DataSource>,
    /// The source the console runs on.
    selected: Option<String>,
    tables: HashMap<String, Result<Vec<String>, String>>,
    collapsed: std::collections::HashSet<String>,
    columns: HashMap<(String, String), Option<Vec<(DbColumn, bool)>>>,
    filter: Option<Entity<TextField>>,
    loaded: bool,
    add_menu: bool,
    status: Option<String>,
    _subscription: Option<gpui::Subscription>,
}

/// `20261101-1530` for dump file names.
fn chrono_like_stamp() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()) as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm), UTC.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}{month:02}{day:02}-{:02}{:02}", rem / 3600, rem % 3600 / 60)
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
        if self.db.loaded {
            return;
        }
        self.db.loaded = true;
        let filter = cx.new(|cx| TextField::new("Filter tables", cx));
        self.db._subscription = Some(cx.subscribe(&filter, |_, _, _: &FieldEvent, cx| cx.notify()));
        self.db.filter = Some(filter);
        self.db.sources = datasource::load(self.project.root());
        self.db.selected = self.db.sources.first().map(|s| s.id.clone());
        for id in self.db.sources.iter().map(|s| s.id.clone()).collect::<Vec<_>>() {
            self.load_tables(&id, cx);
        }
    }

    /// Data sources and the selected one (for the `.sql` file bar).
    pub(super) fn db_sources(&self) -> (&[DataSource], Option<&str>) {
        (&self.db.sources, self.db.selected.as_deref())
    }

    pub(super) fn select_source(&mut self, id: String, cx: &mut Context<Self>) {
        self.db.selected = Some(id);
        cx.notify();
    }

    /// Runs `sql` on the selected data source in a new console tab.
    pub(super) fn run_in_console(&mut self, sql: String, window: &mut Window, cx: &mut Context<Self>) {
        self.ensure_db(cx);
        let Some(source) = self.db.selected.clone().and_then(|id| self.source(&id)) else {
            self.output = Some(super::tools::Output { title: "SQL".into(), text: "Add a data source in the Database window first.".into(), running: false, ok: false });
            cx.notify();
            return;
        };
        let password = load_password(self.project.root(), &source.id);
        self.open_console_tab(source, password, window, cx);
        if let Some(view) = self.tables.last().cloned() {
            view.update(cx, |view, cx| {
                view.set_console_text(&sql, cx);
                view.run(cx);
            });
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


    /// Export to SQL: asks where to save, then runs mysqldump / pg_dump / sqlite3 in the background.
    fn export_selected(&mut self, cx: &mut Context<Self>) {
        let Some(source) = self.db.selected.clone().and_then(|id| self.source(&id)) else {
            self.db.status = Some("Select a data source to export.".into());
            cx.notify();
            return;
        };
        let root = self.project.root().to_path_buf();
        let password = load_password(&root, &source.id);
        let stamp = chrono_like_stamp();
        let base = if source.database.is_empty() { source.name.trim_start_matches('@').to_string() } else { source.database.clone() };
        let suggested = format!("{base}-{stamp}.sql");
        let target = cx.prompt_for_new_path(&root.join("database"), Some(&suggested));
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(out))) = target.await else { return };
            let _ = this.update(cx, |this, cx| {
                this.db.status = Some(format!("Exporting {} …", source.name));
                cx.notify();
            });
            let shown = out.clone();
            let result = cx.background_spawn(async move { laravel::dump::export(&root, &source, &password, &out) }).await;
            let _ = this.update(cx, |this, cx| {
                this.db.status = Some(match result {
                    Ok(bytes) => format!("Exported {:.1} MB to {}", bytes as f64 / 1_048_576., shown.display()),
                    Err(err) => format!("Export failed: {err}"),
                });
                cx.notify();
            });
        })
        .detach();
    }

    fn open_console_in_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(source) = self.db.selected.clone().and_then(|id| self.source(&id)) else {
            self.db.status = Some("Add a data source first (+).".into());
            cx.notify();
            return;
        };
        let password = load_password(self.project.root(), &source.id);
        let starter = if source.driver == Driver::Redis { "KEYS *" } else { "SELECT 1" };
        self.open_console_tab(source, password, window, cx);
        if let Some(view) = self.tables.last().cloned() {
            view.update(cx, |view, cx| view.set_console_text(starter, cx));
        }
    }

    /// ⌘↵ outside a console: open one in the editor.
    pub(super) fn run_sql(&mut self, _: &RunSql, window: &mut Window, cx: &mut Context<Self>) {
        self.ensure_db(cx);
        self.open_console_in_editor(window, cx);
    }

    pub(super) fn render_database(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let icon_button = |id: &'static str, glyph: Icon, tip: &'static str| {
            div()
                .id(id)
                .size(px(24.))
                .flex()
                .items_center()
                .justify_center()
                .rounded_md()
                .cursor_pointer()
                .hover(|d| d.bg(theme::hover()))
                .child(icon(glyph, 15., theme::muted()))
                .tooltip(super::layout::tooltip(tip))
        };
        let toolbar = div()
            .flex()
            .items_center()
            .gap_0p5()
            .px_2()
            .py_1()
            .child(icon_button("db-add", Icon::plus, "New data source").on_click(cx.listener(|this, _, _, cx| {
                this.db.add_menu = !this.db.add_menu;
                cx.notify();
            })))
            .child(icon_button("db-edit", Icon::settings, "Data source properties").on_click(cx.listener(|this, _, window, cx| {
                if let Some(source) = this.db.selected.clone().and_then(|id| this.source(&id)) {
                    let password = load_password(this.project.root(), &source.id);
                    this.open_source_dialog(source, password, window, cx);
                }
            })))
            .child(icon_button("db-remove", Icon::minus, "Remove data source").on_click(cx.listener(|this, _, _, cx| this.remove_selected_source(cx))))
            .child(icon_button("db-refresh", Icon::refresh, "Refresh").on_click(cx.listener(|this, _, _, cx| this.refresh_db(cx))))
            .child(icon_button("db-export", Icon::download, "Export to SQL (mysqldump / pg_dump / sqlite3)").on_click(cx.listener(|this, _, _, cx| this.export_selected(cx))))
            .child(div().flex_1())
            .child(icon_button("db-console", Icon::play, "Open a console in the editor (⌘↵ runs)").on_click(cx.listener(|this, _, window, cx| this.open_console_in_editor(window, cx))));
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
                    // PhpStorm's levels: data source › schema › "tables N" › tables.
                    let schema = match source.driver {
                        Driver::Sqlite => "main".to_string(),
                        Driver::Redis => format!("db{}", if source.database.is_empty() { "0" } else { &source.database }),
                        _ if source.database.is_empty() => "default".to_string(),
                        _ => source.database.clone(),
                    };
                    let folder_row = |key: String, depth: f32, icon: &'static str, label: String, count: Option<usize>, cx: &mut Context<Self>| {
                        let collapsed = self.db.collapsed.contains(&key);
                        div()
                            .id(SharedString::from(format!("db-node-{key}")))
                            .mx_1()
                            .pl(px(depth))
                            .pr_2()
                            .h(px(22.))
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .rounded_md()
                            .cursor_pointer()
                            .hover(|d| d.bg(theme::hover()))
                            .child(div().w(px(10.)).text_size(px(9.)).text_color(theme::muted()).child(if collapsed { "▶" } else { "▼" }))
                            .child(div().text_color(theme::muted()).child(icon))
                            .child(div().text_color(theme::ui_text()).child(SharedString::from(label)))
                            .when_some(count, |d, n| d.child(div().text_size(px(11.)).text_color(theme::muted()).child(n.to_string())))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if !this.db.collapsed.remove(&key) {
                                    this.db.collapsed.insert(key.clone());
                                }
                                cx.notify();
                            }))
                            .into_any_element()
                    };
                    let schema_key = format!("{id}/schema");
                    let tables_key = format!("{id}/tables");
                    tree.push(folder_row(schema_key.clone(), 26., "⊞", schema, None, cx));
                    if self.db.collapsed.contains(&schema_key) {
                        continue;
                    }
                    tree.push(folder_row(tables_key.clone(), 42., "▭", if source.driver == Driver::Redis { "keys".into() } else { "tables".into() }, Some(tables.len()), cx));
                    if self.db.collapsed.contains(&tables_key) {
                        continue;
                    }
                    for (ix, table) in tables.iter().filter(|t| query.is_empty() || t.to_lowercase().contains(&query)).enumerate() {
                        let key = (id.clone(), table.clone());
                        let expanded = self.db.columns.get(&key);
                        let (open_id, open_table) = key.clone();
                        tree.push(
                            div()
                                .id(SharedString::from(format!("db-t-{id}-{ix}")))
                                .mx_1()
                                .pl(px(58.))
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
                                .on_click(cx.listener(move |this, event: &gpui::ClickEvent, window, cx| {
                                    if event.click_count() >= 2 {
                                        // PhpStorm opens the table's data in an editor tab.
                                        if let Some(source) = this.source(&open_id) {
                                            let password = load_password(this.project.root(), &source.id);
                                            this.open_table_tab(source, password, open_table.clone(), window, cx);
                                        }
                                    } else {
                                        this.toggle_columns(open_id.clone(), open_table.clone(), cx);
                                    }
                                }))
                                .into_any_element(),
                        );
                        match expanded {
                            Some(None) => tree.push(div().pl(px(88.)).text_size(px(12.)).text_color(theme::muted()).child("Loading…").into_any_element()),
                            Some(Some(columns)) => {
                                for (column, pk) in columns {
                                    tree.push(
                                        div()
                                            .pl(px(88.))
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
        div()
            .flex_1()
            .min_h(px(0.))
            .flex()
            .flex_col()
            .text_size(px(13.))
            .child(toolbar)
            .children(add_menu)
            .children(self.db.filter.clone().map(|f| div().px_2().pb_1().flex().child(f)))
            .child(div().id("db-tree").flex_1().min_h(px(0.)).overflow_y_scroll().children(tree))
            .when_some(self.db.status.clone(), |d, status| {
                d.child(
                    div()
                        .id("db-status")
                        .px_2()
                        .py_1()
                        .border_t_1()
                        .border_color(theme::border())
                        .text_size(px(12.))
                        .text_color(theme::muted())
                        .child(status)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.db.status = None;
                            cx.notify();
                        })),
                )
            })
            .child(
                div()
                    .px_2()
                    .py_1()
                    .border_t_1()
                    .border_color(theme::border())
                    .text_size(px(11.))
                    .text_color(theme::muted())
                    .child("Double-click a table to open it · ▶ opens a console"),
            )
    }
}
