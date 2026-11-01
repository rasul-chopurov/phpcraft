//! Running requests from `.http` files: ⌘↵ (or the ▶ bar above the editor) sends the request
//! under the caret and shows the response in the output panel.

use gpui::{Context, SharedString, Window, actions, div, prelude::*, px};

use super::Workspace;
use super::tools::Output;
use crate::http_client;
use crate::icons::{Icon, icon};
use crate::theme;

actions!(workspace, [RunHttpRequest, NewHttpRequest]);

impl Workspace {
    /// The active tab's file when it's an `.http` file.
    fn active_http_file(&self, cx: &gpui::App) -> Option<std::path::PathBuf> {
        let path = self.tabs.get(self.active)?.read(cx).buffer.path()?.to_path_buf();
        http_client::is_http_file(&path).then_some(path)
    }

    /// Tools ▸ HTTP Client: a new `.http` file (in `http/`) from the template, opened.
    pub(super) fn new_http_request(&mut self, _: &NewHttpRequest, window: &mut Window, cx: &mut Context<Self>) {
        let dir = self.project.root().join("http");
        let _ = std::fs::create_dir_all(&dir);
        let path = (1..).map(|n| dir.join(if n == 1 { "requests.http".to_string() } else { format!("requests-{n}.http") })).find(|p| !p.exists()).unwrap_or_else(|| dir.join("requests.http"));
        if std::fs::write(&path, http_client::TEMPLATE).is_ok() {
            if !dir.join("http-client.env.json").exists() {
                let _ = std::fs::write(dir.join("http-client.env.json"), "{\n  \"local\": { \"host\": \"http://localhost:8000\" }\n}\n");
            }
            self.rows = self.project.rows();
            self.open_file(&path, window, cx);
        }
    }

    pub(super) fn run_http_request(&mut self, _: &RunHttpRequest, _: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.active_http_file(cx) else { return };
        let Some(tab) = self.tabs.get(self.active) else { return };
        let buffer = &tab.read(cx).buffer;
        let text = buffer.text();
        let (line, _) = buffer.char_to_line_col(buffer.selection.head);
        let Some(request) = http_client::request_at(&text, line) else {
            self.output = Some(Output { title: "HTTP Client".into(), text: "No request at the caret: put it on a `GET https://…` line.".into(), running: false, ok: false });
            cx.notify();
            return;
        };
        let dir = path.parent().unwrap_or(self.project.root()).to_path_buf();
        let (_, env, mut vars) = http_client::environment(&dir, self.http_env.as_deref());
        for (k, v) in http_client::file_variables(&text) {
            vars.entry(k).or_insert(v);
        }
        let request = http_client::resolve(&request, &vars);
        let title = format!("{} {}{}", request.method, request.url, env.map(|e| format!("  ·  {e}")).unwrap_or_default());
        self.output = Some(Output { title: title.clone(), text: "Sending…".into(), running: true, ok: true });
        cx.notify();
        cx.spawn(async move |this, cx| {
            let sent = request.clone();
            let result = cx.background_spawn(async move { http_client::send(&sent) }).await;
            let _ = this.update(cx, |this, cx| {
                this.output = Some(match result {
                    Ok(response) => Output {
                        title,
                        ok: response.status < 400,
                        text: http_client::format_response(&request, &response),
                        running: false,
                    },
                    Err(err) => Output { title, text: err, running: false, ok: false },
                });
                cx.notify();
            });
        })
        .detach();
    }

    /// The bar above an `.http` file: run button, request count and environments.
    pub(super) fn render_http_bar(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        if self.table_active.is_some() || self.diff_active {
            return None;
        }
        let path = self.active_http_file(cx)?;
        let text = self.tabs.get(self.active)?.read(cx).buffer.text();
        let count = http_client::requests(&text).len();
        let dir = path.parent().unwrap_or(self.project.root()).to_path_buf();
        let (envs, current, _) = http_client::environment(&dir, self.http_env.as_deref());
        let chips = envs.into_iter().map(|name| {
            let selected = current.as_deref() == Some(name.as_str());
            let value = name.clone();
            div()
                .id(SharedString::from(format!("http-env-{name}")))
                .px_2()
                .py_0p5()
                .rounded_md()
                .cursor_pointer()
                .border_1()
                .border_color(if selected { theme::accent() } else { theme::border() })
                .text_color(if selected { theme::ui_text() } else { theme::muted() })
                .child(name)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.http_env = Some(value.clone());
                    cx.notify();
                }))
        });
        Some(
            div()
                .flex()
                .items_center()
                .gap_2()
                .px_3()
                .h(px(30.))
                .border_b_1()
                .border_color(theme::border())
                .bg(theme::panel_bg())
                .text_size(px(12.))
                .child(
                    div()
                        .id("http-run")
                        .flex()
                        .items_center()
                        .gap_1()
                        .px_2()
                        .py_0p5()
                        .rounded_md()
                        .cursor_pointer()
                        .bg(theme::accent())
                        .text_color(gpui::white())
                        .child(icon(Icon::play, 11., gpui::white()))
                        .child(format!("Run request  {}", crate::keymap::display("secondary-enter")))
                        .on_click(cx.listener(|this, _, window, cx| this.run_http_request(&RunHttpRequest, window, cx))),
                )
                .child(div().text_color(theme::muted()).child(format!("{count} request{}", if count == 1 { "" } else { "s" })))
                .child(div().flex_1())
                .child(div().text_color(theme::muted()).child("Environment"))
                .children(chips)
                .when(current.is_none(), |d| d.child(div().text_color(theme::muted()).child("none (add http-client.env.json)"))),
        )
    }
}
