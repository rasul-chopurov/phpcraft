//! Data Source dialog (PhpStorm's "Data Sources and Drivers"): name, driver, connection
//! type, host/port, user/password, database or SQLite file, DSN preview, Test Connection.

use std::path::PathBuf;

use gpui::{App, AppContext, Context, Entity, EventEmitter, FocusHandle, Focusable, SharedString, Window, div, prelude::*, px};
use laravel::datasource::{ConnectionType, DataSource, Driver};

use super::{Cancel, Confirm, DialogEvent, button, card};
use crate::text_field::{FieldEvent, TextField};
use crate::theme;

pub struct DataSourceDialog {
    root: PathBuf,
    source: DataSource,
    name: Entity<TextField>,
    host: Entity<TextField>,
    port: Entity<TextField>,
    user: Entity<TextField>,
    password: Entity<TextField>,
    database: Entity<TextField>,
    path: Entity<TextField>,
    url: Entity<TextField>,
    status: Option<Result<String, String>>,
    testing: bool,
    focus_handle: FocusHandle,
    _subscriptions: Vec<gpui::Subscription>,
}

impl EventEmitter<DialogEvent> for DataSourceDialog {}

fn field(placeholder: &'static str, text: String, cx: &mut Context<DataSourceDialog>) -> Entity<TextField> {
    cx.new(|cx| {
        let mut f = TextField::new(placeholder, cx);
        f.set_text(text, cx);
        f
    })
}

impl DataSourceDialog {
    pub fn new(root: PathBuf, source: DataSource, password: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let name = field("Name", source.name.clone(), cx);
        let host = field("127.0.0.1", source.host.clone(), cx);
        let port = field("3306", source.port.to_string(), cx);
        let user = field("root", source.user.clone(), cx);
        let password = field("Password", password, cx);
        let database = field("Database", source.database.clone(), cx);
        let path = field(if source.driver == Driver::Sqlite { "database/database.sqlite" } else { "/tmp/mysql.sock" }, source.path.clone(), cx);
        let url = field("mysql:host=127.0.0.1;port=3306;dbname=app", source.url.clone(), cx);
        let subscriptions = [&host, &port, &user, &database, &path, &url]
            .into_iter()
            .map(|f| cx.subscribe(f, |this: &mut Self, _, _: &FieldEvent, cx| {
                this.status = None;
                cx.notify();
            }))
            .collect();
        window.focus(&name.focus_handle(cx));
        Self { root, source, name, host, port, user, password, database, path, url, status: None, testing: false, focus_handle: cx.focus_handle(), _subscriptions: subscriptions }
    }

    /// The source with the fields applied, and the password.
    fn current(&self, cx: &App) -> (DataSource, String) {
        let mut s = self.source.clone();
        s.name = self.name.read(cx).text().trim().to_string();
        s.host = self.host.read(cx).text().trim().to_string();
        s.port = self.port.read(cx).text().trim().parse().unwrap_or(s.driver.default_port());
        s.user = self.user.read(cx).text().trim().to_string();
        s.database = self.database.read(cx).text().trim().to_string();
        s.path = self.path.read(cx).text().trim().to_string();
        s.url = self.url.read(cx).text().trim().to_string();
        if s.name.is_empty() {
            s.name = if s.driver == Driver::Sqlite { s.path.rsplit('/').next().unwrap_or("SQLite").to_string() } else { format!("@{}", s.host) };
        }
        (s, self.password.read(cx).text().to_string())
    }

    fn set_driver(&mut self, driver: Driver, cx: &mut Context<Self>) {
        if self.source.driver.default_port().to_string() == self.port.read(cx).text() {
            self.port.update(cx, |f, cx| f.set_text(driver.default_port().to_string(), cx));
        }
        self.source.driver = driver;
        self.status = None;
        cx.notify();
    }

    fn test(&mut self, cx: &mut Context<Self>) {
        let (source, password) = self.current(cx);
        let root = self.root.clone();
        self.testing = true;
        cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move { laravel::datasource::test(&root, &source, &password) }).await;
            let _ = this.update(cx, |this, cx| {
                this.testing = false;
                this.status = Some(result);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn confirm(&mut self, _: &Confirm, _: &mut Window, cx: &mut Context<Self>) {
        let (source, password) = self.current(cx);
        cx.emit(DialogEvent::DataSourceSaved { source, password });
    }

    fn cancel(&mut self, _: &Cancel, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(DialogEvent::Dismiss);
    }
}

impl Focusable for DataSourceDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for DataSourceDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let s = &self.source;
        let row = |label: &'static str| div().flex().items_center().gap_3().child(div().w(px(130.)).flex_none().text_color(theme::muted()).child(label));
        let chip = |id: SharedString, label: &'static str, active: bool| {
            div()
                .id(id)
                .px_2()
                .py(px(3.))
                .rounded_md()
                .border_1()
                .cursor_pointer()
                .border_color(if active { theme::accent() } else { theme::border_strong() })
                .when(active, |d| d.bg(theme::active_row()))
                .hover(|d| d.bg(theme::hover()))
                .child(label)
        };
        let drivers = Driver::ALL.into_iter().map(|driver| {
            chip(SharedString::from(format!("ds-driver-{driver:?}")), driver.label(), s.driver == driver)
                .on_click(cx.listener(move |this, _, _, cx| this.set_driver(driver, cx)))
        });
        let sqlite = s.driver == Driver::Sqlite;
        let kinds = [(ConnectionType::Default, "default"), (ConnectionType::UnixSocket, "Unix Socket"), (ConnectionType::Url, "URL only")].into_iter().map(|(kind, label)| {
            let active = s.connection == kind;
            chip(SharedString::from(format!("ds-kind-{label}")), label, active).on_click(cx.listener(move |this, _, _, cx| {
                this.source.connection = kind.clone();
                cx.notify();
            }))
        });
        let (current, _) = self.current(cx);
        let dsn = current.dsn(&self.root);
        let status = match &self.status {
            _ if self.testing => div().text_color(theme::muted()).child("Testing…"),
            None => div(),
            Some(Ok(version)) => div().text_color(theme::success()).child(format!("✓ Succeeded · {version}")),
            Some(Err(err)) => div().text_color(theme::error()).child(SharedString::from(err.chars().take(300).collect::<String>())),
        };
        card(640.)
            .key_context("Dialog")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::cancel))
            .child(div().px_3().py_2().border_b_1().border_color(theme::border()).child("Data Source"))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .p_3()
                    .text_size(px(13.))
                    .child(row("Name").child(div().flex_1().child(self.name.clone())))
                    .child(row("Driver").child(div().flex().gap_1().children(drivers)))
                    .when(sqlite, |d| d.child(row("File").child(div().flex_1().child(self.path.clone()))))
                    .when(!sqlite, |d| {
                        d.child(row("Connection type").child(div().flex().gap_1().children(kinds)))
                            .when(s.connection == ConnectionType::Default, |d| {
                                d.child(row("Host").child(div().flex_1().child(self.host.clone())).child(div().text_color(theme::muted()).child("Port")).child(div().w(px(90.)).child(self.port.clone())))
                            })
                            .when(s.connection == ConnectionType::UnixSocket, |d| d.child(row("Socket").child(div().flex_1().child(self.path.clone()))))
                            .when(s.connection == ConnectionType::Url, |d| d.child(row("PDO DSN").child(div().flex_1().child(self.url.clone()))))
                            .child(row("User").child(div().flex_1().child(self.user.clone())))
                            .child(row("Password").child(div().flex_1().child(self.password.clone())))
                            .when(s.connection != ConnectionType::Url, |d| d.child(row("Database").child(div().flex_1().child(self.database.clone()))))
                    })
                    .child(row("DSN").child(div().flex_1().text_size(px(12.)).font_family(theme::editor_font()).text_color(theme::muted()).child(dsn)))
                    .child(div().text_size(px(11.)).text_color(theme::muted()).child("Connects through PHP's PDO. The password is stored in your PhpCraft config folder, not in the project.")),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .p_3()
                    .border_t_1()
                    .border_color(theme::border())
                    .child(div().id("ds-test").cursor_pointer().text_color(theme::accent()).child("Test Connection").on_click(cx.listener(|this, _, _, cx| this.test(cx))))
                    .child(div().flex_1().text_size(px(12.)).child(status))
                    .child(button("ds-cancel", "Cancel", false).on_click(cx.listener(|this, _, window, cx| this.cancel(&Cancel, window, cx))))
                    .child(button("ds-ok", "OK", true).on_click(cx.listener(|this, _, window, cx| this.confirm(&Confirm, window, cx)))),
            )
    }
}
