//! The HTTP response panel: the status, the headers and a highlighted body, shown instead of the
//! plain output log so a JSON, XML or HTML answer reads the way it does in an editor.

use std::ops::Range;
use std::path::Path;

use gpui::{Context, SharedString, StyledText, div, prelude::*, px};
use php_syntax::Span;

use super::Workspace;
use crate::http_client::{Request, Response};
use crate::i18n::{tr, trf};
use crate::theme;

/// A body beyond this is neither pretty-printed nor highlighted: the panel is a preview, and a
/// 20 MB answer would turn every frame into a parse.
const MAX_BODY: usize = 256 * 1024;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum HttpTab {
    Body,
    Headers,
}

/// One answered request, kept until the next one or until the panel is closed.
pub(super) struct HttpView {
    pub method: String,
    pub url: String,
    pub status: u16,
    pub reason: String,
    pub elapsed: u128,
    pub headers: Vec<(String, String)>,
    pub tab: HttpTab,
    /// The body as the panel shows it: pretty-printed JSON and shortened when it is huge.
    pub body: String,
    /// What the server actually sent, kept whole for saving and copying.
    pub raw: String,
    pub note: Option<String>,
    spans: Vec<Span>,
}

impl HttpView {
    pub fn new(request: &Request, response: &Response) -> Self {
        let content_type = response
            .headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))
            .map(|(_, v)| v.to_ascii_lowercase())
            .unwrap_or_default();
        let pretty = content_type.contains("json").then(|| serde_json::from_str::<serde_json::Value>(&response.body).ok()).flatten();
        let shown = pretty.map(|v| serde_json::to_string_pretty(&v).unwrap_or_else(|_| response.body.clone())).unwrap_or_else(|| response.body.clone());
        let (body, note) = if shown.len() > MAX_BODY {
            let mut cut = MAX_BODY;
            while cut > 0 && !shown.is_char_boundary(cut) {
                cut -= 1;
            }
            (shown[..cut].to_string(), Some(trf("Preview shortened to {} KB · Save keeps the whole body", &[&(MAX_BODY / 1024).to_string()])))
        } else {
            (shown, None)
        };
        let spans = php_syntax::highlight_path(Path::new(language_path(&content_type)), &body);
        Self {
            method: request.method.clone(),
            url: request.url.clone(),
            status: response.status,
            reason: response.reason.clone(),
            elapsed: response.elapsed.as_millis(),
            headers: response.headers.clone(),
            tab: HttpTab::Body,
            body,
            raw: response.body.clone(),
            note,
            spans,
        }
    }

    fn highlights(&self) -> Vec<(Range<usize>, gpui::HighlightStyle)> {
        self.spans
            .iter()
            .map(|span| (span.range.clone(), gpui::HighlightStyle { color: Some(theme::syntax(span.highlight)), ..Default::default() }))
            .collect()
    }

    fn success(&self) -> bool {
        self.status < 400
    }

    /// The name a saved body gets: the last path segment, or a plain `response` with the
    /// extension the content type asks for.
    fn file_name(&self) -> String {
        // The host is not a file name: only the segments after it count.
        let after_host = self.url.split("://").nth(1).unwrap_or(&self.url);
        let last = after_host
            .split(['?', '#'])
            .next()
            .unwrap_or_default()
            .split('/')
            .skip(1)
            .filter(|s| !s.is_empty())
            .last()
            .map(|s| s.to_string())
            .unwrap_or_else(|| "response".to_string());
        let last: String = last.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' || c == '.' { c } else { '-' }).collect();
        if last.contains('.') { last } else { format!("{last}.{}", language_path(&self.content_type()).rsplit('.').next().unwrap_or("txt")) }
    }

    fn content_type(&self) -> String {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))
            .map(|(_, v)| v.to_ascii_lowercase())
            .unwrap_or_default()
    }

    fn header_lines(&self) -> Vec<String> {
        let mut out = vec![format!("HTTP {} {}", self.status, self.reason)];
        out.extend(self.headers.iter().map(|(k, v)| format!("{k}: {v}")));
        out
    }
}

/// A stand-in path, so the syntax highlighter picks the grammar the body was written in.
fn language_path(content_type: &str) -> &'static str {
    if content_type.contains("json") {
        "response.json"
    } else if content_type.contains("html") {
        "response.html"
    } else if content_type.contains("xml") {
        "response.xml"
    } else if content_type.contains("yaml") {
        "response.yaml"
    } else if content_type.contains("css") {
        "response.css"
    } else if content_type.contains("javascript") {
        "response.js"
    } else {
        "response.txt"
    }
}

impl Workspace {
    /// The response of the last request: a header row with the status, tabs for the body and the
    /// headers, and the body itself with syntax colours.
    pub(super) fn render_http_response(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let view = self.http_view.as_ref()?;
        let color = if view.success() { theme::success() } else { theme::error() };
        let url = if view.url.chars().count() > 90 {
            format!("{}…", view.url.chars().take(90).collect::<String>())
        } else {
            view.url.clone()
        };
        let tab = |id: &'static str, label: &str, wanted: HttpTab| {
            let active = view.tab == wanted;
            div()
                .id(id)
                .px_2()
                .py_0p5()
                .rounded_md()
                .cursor_pointer()
                .border_1()
                .border_color(if active { theme::accent() } else { theme::border() })
                .text_color(if active { theme::ui_text() } else { theme::muted() })
                .child(tr(label))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(view) = this.http_view.as_mut() {
                        view.tab = wanted;
                    }
                    cx.notify();
                }))
        };
        let body = match view.tab {
            HttpTab::Body => div()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .text_color(theme::text())
                        .child(StyledText::new(SharedString::from(view.body.clone())).with_highlights(view.highlights())),
                )
                .when_some(view.note.clone(), |d, note| d.child(div().text_size(px(11.5)).text_color(theme::muted()).child(SharedString::from(note)))),
            HttpTab::Headers => div().flex().flex_col().children(view.header_lines().into_iter().map(|line| div().child(SharedString::from(line)))),
        };
        Some(
            div()
                .flex()
                .flex_col()
                .flex_none()
                .h(px(320.))
                .border_t_1()
                .border_color(theme::border_strong())
                .bg(theme::editor_bg())
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .px_2()
                        .h(px(28.))
                        .flex_none()
                        .bg(theme::panel_bg())
                        .text_size(px(12.))
                        .child(div().text_color(theme::ui_text()).child(SharedString::from(format!("{} {}", view.method, url))))
                        .child(div().text_color(color).child(SharedString::from(format!("{} {}", view.status, view.reason))))
                        .child(div().text_color(theme::muted()).child(SharedString::from(format!("{} ms · {} bytes", view.elapsed, view.raw.len()))))
                        .child(div().flex_1())
                        .child(tab("http-tab-body", "Body", HttpTab::Body))
                        .child(tab("http-tab-headers", "Headers", HttpTab::Headers))
                        .child(
                            div()
                                .id("http-response-copy")
                                .px_1()
                                .cursor_pointer()
                                .text_color(theme::muted())
                                .hover(|d| d.text_color(theme::ui_text()))
                                .child(tr("Copy"))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    if let Some(view) = this.http_view.as_ref() {
                                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(view.raw.clone()));
                                    }
                                })),
                        )
                        .child(
                            div()
                                .id("http-response-save")
                                .px_1()
                                .cursor_pointer()
                                .text_color(theme::muted())
                                .hover(|d| d.text_color(theme::ui_text()))
                                .child(tr("Save…"))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    let Some(view) = this.http_view.as_ref() else { return };
                                    let dir = this.project.root().join("http");
                                    let raw = view.raw.clone();
                                    let suggested = view.file_name();
                                    let save = cx.prompt_for_new_path(&dir, Some(&suggested));
                                    cx.spawn(async move |this, cx| {
                                        let Ok(Ok(Some(path))) = save.await else { return };
                                        let _ = std::fs::write(&path, raw);
                                        let _ = this.update(cx, |this, cx| {
                                            this.output = Some(super::tools::Output {
                                                title: tr("HTTP Client").to_string(),
                                                text: trf("Response saved to {}", &[&path.display().to_string()]),
                                                running: false,
                                                ok: true,
                                            });
                                            cx.notify();
                                        });
                                    })
                                    .detach();
                                })),
                        )
                        .child(
                            div()
                                .id("http-response-close")
                                .px_1()
                                .cursor_pointer()
                                .text_color(theme::muted())
                                .hover(|d| d.text_color(theme::ui_text()))
                                .child("×")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.http_view = None;
                                    cx.notify();
                                })),
                        ),
                )
                .child(
                    div()
                        .id("http-response-body")
                        .flex_1()
                        .min_h(px(0.))
                        .overflow_y_scroll()
                        .overflow_x_scroll()
                        .p_2()
                        .font_family(theme::editor_font())
                        .text_size(px(12.))
                        .whitespace_nowrap()
                        .child(body),
                ),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(url: &str) -> Request {
        Request { name: None, method: "GET".into(), url: url.into(), headers: Vec::new(), body: String::new() }
    }

    fn response(content_type: &str, body: &str) -> Response {
        Response {
            status: 200,
            reason: "OK".into(),
            headers: vec![("content-type".into(), content_type.into())],
            body: body.into(),
            elapsed: std::time::Duration::from_millis(12),
        }
    }

    #[test]
    fn json_bodies_are_pretty_printed_and_saved_under_their_extension() {
        let view = HttpView::new(&request("https://api.test/users?page=1"), &response("application/json; charset=utf-8", r#"{"a":1,"b":[2]}"#));
        assert!(view.body.contains("\n  \"a\": 1"), "{}", view.body);
        assert_eq!(view.file_name(), "users.json");
        assert_eq!(view.raw, r#"{"a":1,"b":[2]}"#);
    }

    #[test]
    fn html_bodies_keep_their_grammar_and_a_bare_host_is_not_a_file_name() {
        let view = HttpView::new(&request("https://api.test/"), &response("text/html", "<h1>Hi</h1>"));
        assert_eq!(view.file_name(), "response.html");
        assert!(!view.spans.is_empty());
    }
}
