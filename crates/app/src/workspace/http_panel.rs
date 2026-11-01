//! Running requests from `.http` files: ⌘↵ (or the ▶ bar above the editor) sends the request
//! under the caret and shows the response in the output panel.

use gpui::{Context, SharedString, Window, actions, div, prelude::*, px};

use super::Workspace;
use super::tools::Output;
use crate::{http_client, http_convert};
use crate::icons::{Icon, icon};
use crate::theme;

actions!(workspace, [RunHttpRequest, NewHttpRequest, ConvertCurlToHttp, CopyAsCurl, ImportPostman, ImportOpenApi, ShowHttpHistory]);

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
                    Ok(response) => {
                        // History: the request, and the body saved to a file.
                        let saved = http_convert::record(this.project.root(), &request, &response);
                        let mut text = http_client::format_response(&request, &response);
                        if let Some(file) = saved {
                            text.push_str(&format!("\n\nResponse saved to {} (Tools ▸ HTTP Client ▸ History)", file.strip_prefix(this.project.root()).unwrap_or(&file).display()));
                        }
                        Output { title, ok: response.status < 400, text, running: false }
                    }
                    Err(err) => Output { title, text: err, running: false, ok: false },
                });
                cx.notify();
            });
        })
        .detach();
    }

    /// The request at the caret of the active `.http` file, with variables resolved.
    fn http_request_at_caret(&self, cx: &gpui::App) -> Option<http_client::Request> {
        let path = self.active_http_file(cx)?;
        let buffer = &self.tabs.get(self.active)?.read(cx).buffer;
        let text = buffer.text();
        let (line, _) = buffer.char_to_line_col(buffer.selection.head);
        let request = http_client::request_at(&text, line)?;
        let dir = path.parent().unwrap_or(self.project.root()).to_path_buf();
        let (_, _, mut vars) = http_client::environment(&dir, self.http_env.as_deref());
        for (k, v) in http_client::file_variables(&text) {
            vars.entry(k).or_insert(v);
        }
        Some(http_client::resolve(&request, &vars))
    }

    /// Writes generated requests to `http/<name>.http` (a free name) and opens it.
    fn write_http_file(&mut self, name: &str, content: &str, window: &mut Window, cx: &mut Context<Self>) {
        let dir = self.project.root().join("http");
        let _ = std::fs::create_dir_all(&dir);
        let stem: String = name.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '-' }).collect();
        let path = (1..).map(|n| dir.join(if n == 1 { format!("{stem}.http") } else { format!("{stem}-{n}.http") })).find(|p| !p.exists()).unwrap_or_else(|| dir.join("imported.http"));
        if std::fs::write(&path, content).is_ok() {
            self.rows = self.project.rows();
            self.open_file(&path, window, cx);
        }
    }

    /// cURL from the clipboard: inserted at the caret of an open `.http` file, or a new file.
    pub(super) fn convert_curl_to_http(&mut self, _: &ConvertCurlToHttp, window: &mut Window, cx: &mut Context<Self>) {
        let clipboard = cx.read_from_clipboard().and_then(|c| c.text()).unwrap_or_default();
        let Some(request) = http_convert::from_curl(&clipboard) else {
            self.output = Some(Output { title: "HTTP Client".into(), text: "Copy a curl command first (the clipboard has no `curl …`).".into(), running: false, ok: false });
            return cx.notify();
        };
        let http = http_convert::to_http(&request);
        match (self.active_http_file(cx), self.tabs.get(self.active).cloned()) {
            (Some(_), Some(tab)) => tab.update(cx, |editor, cx| {
                editor.buffer.insert(&format!("\n{http}"));
                cx.notify();
            }),
            _ => self.write_http_file("from-curl", &http, window, cx),
        }
    }

    pub(super) fn copy_as_curl(&mut self, _: &CopyAsCurl, _: &mut Window, cx: &mut Context<Self>) {
        let Some(request) = self.http_request_at_caret(cx) else { return };
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(http_convert::to_curl(&request)));
        self.output = Some(Output { title: "HTTP Client".into(), text: "Copied as cURL.".into(), running: false, ok: true });
        cx.notify();
    }

    fn import_requests(&mut self, prompt: &'static str, convert: fn(&str) -> Result<String, String>, cx: &mut Context<Self>, window: &mut Window) {
        let paths = cx.prompt_for_paths(gpui::PathPromptOptions { files: true, directories: false, multiple: false, prompt: Some(prompt.into()) });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else { return };
            let Some(path) = paths.into_iter().next() else { return };
            let result = std::fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|t| convert(&t));
            let name = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "imported".into());
            let _ = this.update_in(cx, |this, window, cx| match result {
                Ok(http) => this.write_http_file(&name, &http, window, cx),
                Err(e) => {
                    this.output = Some(Output { title: "HTTP Client import".into(), text: e, running: false, ok: false });
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(super) fn import_postman(&mut self, _: &ImportPostman, window: &mut Window, cx: &mut Context<Self>) {
        self.import_requests("Import Postman Collection", http_convert::from_postman, cx, window);
    }

    pub(super) fn import_openapi(&mut self, _: &ImportOpenApi, window: &mut Window, cx: &mut Context<Self>) {
        self.import_requests("Import OpenAPI (JSON)", http_convert::from_openapi, cx, window);
    }

    pub(super) fn show_http_history(&mut self, _: &ShowHttpHistory, window: &mut Window, cx: &mut Context<Self>) {
        let path = self.project.root().join(".phpcraft/http/history.http");
        if path.is_file() {
            self.open_file(&path, window, cx);
        } else {
            self.output = Some(Output { title: "HTTP Client".into(), text: "No requests sent yet.".into(), running: false, ok: true });
            cx.notify();
        }
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
