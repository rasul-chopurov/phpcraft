//! Laravel tool window (like Laravel Idea's): Dashboard with environment facts, databases
//! and issues with fixes; Errors grouped from the log with clickable stack traces;
//! Processes started from the IDE.

use std::collections::HashSet;

use gpui::{AppContext, Context, SharedString, Window, div, prelude::*, px};
use laravel::dashboard::{self, Issue, LogGroup};

use super::Workspace;
use crate::run_configs::{Kind, RunConfig};
use crate::theme;

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum LaravelTab {
    #[default]
    Dashboard,
    Errors,
    Processes,
}

#[derive(Default)]
pub(super) struct LaravelState {
    tab: LaravelTab,
    loaded: bool,
    issues: Vec<Issue>,
    env: Vec<(String, String)>,
    errors: Option<Vec<LogGroup>>,
    expanded: HashSet<usize>,
}

impl Workspace {
    pub(super) fn ensure_laravel_panel(&mut self, cx: &mut Context<Self>) {
        if !self.laravel.loaded {
            self.refresh_laravel(cx);
        }
    }

    fn refresh_laravel(&mut self, cx: &mut Context<Self>) {
        self.laravel.loaded = true;
        let root = self.project.root().to_path_buf();
        cx.spawn(async move |this, cx| {
            let (issues, env, errors) =
                cx.background_spawn(async move { (dashboard::issues(&root), dashboard::env_values(&root), dashboard::log_groups(&root)) }).await;
            let _ = this.update(cx, |this, cx| {
                this.laravel.issues = issues;
                this.laravel.env = env;
                this.laravel.errors = Some(errors);
                cx.notify();
            });
        })
        .detach();
    }

    fn artisan(&mut self, command: &str, cx: &mut Context<Self>) {
        let config = RunConfig { name: format!("artisan {command}"), kind: Kind::Artisan { command: command.to_string() }, env: Default::default() };
        self.start_run(config, false, cx);
    }

    fn open_trace(&mut self, line: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((path, number)) = dashboard::trace_location(self.project.root(), line) {
            self.open_file(&path, window, cx);
            if let Some(tab) = self.tabs.get(self.active).cloned() {
                tab.update(cx, |editor, cx| {
                    let ix = editor.buffer.line_to_char(number.saturating_sub(1));
                    editor.reveal_char(ix, cx);
                });
            }
        }
    }

    pub(super) fn render_laravel_panel(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let tab = |id: &'static str, label: &'static str, which: LaravelTab, current: LaravelTab| {
            div()
                .id(id)
                .px_2()
                .h(px(22.))
                .flex()
                .items_center()
                .rounded_md()
                .cursor_pointer()
                .when(which == current, |d| d.bg(theme::selection()).text_color(theme::ui_text()))
                .when(which != current, |d| d.text_color(theme::muted()).hover(|d| d.bg(theme::hover())))
                .child(label)
        };
        let action = |id: &'static str, label: &'static str| {
            div().id(id).px_2().h(px(22.)).flex().items_center().gap_1().rounded_md().cursor_pointer().text_color(theme::ui_text()).hover(|d| d.bg(theme::hover())).child(label)
        };
        let current = self.laravel.tab;
        let errors_count = self.laravel.errors.as_ref().map_or(0, |e| e.len());
        let header = div()
            .flex()
            .items_center()
            .gap_1()
            .px_2()
            .h(px(30.))
            .bg(theme::panel_bg())
            .child(div().pr_2().text_color(theme::ui_text()).child("Laravel"))
            .child(tab("lv-dash", "Dashboard", LaravelTab::Dashboard, current).on_click(cx.listener(|this, _, _, cx| {
                this.laravel.tab = LaravelTab::Dashboard;
                cx.notify();
            })))
            .child(tab("lv-errors", if errors_count > 0 { "Errors •" } else { "Errors" }, LaravelTab::Errors, current).on_click(cx.listener(|this, _, _, cx| {
                this.laravel.tab = LaravelTab::Errors;
                cx.notify();
            })))
            .child(tab("lv-proc", "Processes", LaravelTab::Processes, current).on_click(cx.listener(|this, _, _, cx| {
                this.laravel.tab = LaravelTab::Processes;
                cx.notify();
            })));
        let toolbar = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_1()
            .px_2()
            .py_1()
            .border_b_1()
            .border_color(theme::border())
            .child(action("lv-refresh", "⟳ Refresh Data").on_click(cx.listener(|this, _, _, cx| this.refresh_laravel(cx))))
            .child(action("lv-serve", "▷ Start Server").on_click(cx.listener(|this, _, _, cx| this.artisan("serve", cx))))
            .child(action("lv-migrate", "⛁ Migrate Database").on_click(cx.listener(|this, _, _, cx| this.artisan("migrate", cx))))
            .child(action("lv-artisan", "▶ Run Artisan Command").on_click(cx.listener(|this, _, window, cx| this.open_artisan(&super::tools::OpenArtisan, window, cx))))
            .child(action("lv-routes", "⌕ Route Search").on_click(cx.listener(|this, _, window, cx| this.open_route_search(&super::tools::OpenRouteSearch, window, cx))))
            .child(action("lv-clear", "✧ Clear Caches").on_click(cx.listener(|this, _, _, cx| this.artisan("optimize:clear", cx))));
        let body: gpui::AnyElement = match current {
            LaravelTab::Dashboard => {
                let env = &self.laravel.env;
                let value = |k: &str| dashboard::env_value(env, k).unwrap_or_default();
                let fact = |label: &'static str, v: String| {
                    div().flex().gap_3().child(div().w(px(130.)).text_color(theme::muted()).child(label)).child(div().text_color(theme::ui_text()).child(SharedString::from(v)))
                };
                let connection = value("DB_CONNECTION");
                let databases = [
                    if connection == "sqlite" || value("DB_HOST").is_empty() {
                        format!("{connection} ({})", if value("DB_DATABASE").is_empty() { "database/database.sqlite".to_string() } else { value("DB_DATABASE") })
                    } else {
                        format!("{connection} {}{}:{}", if value("DB_USERNAME").is_empty() { String::new() } else { format!("{}@", value("DB_USERNAME")) }, value("DB_HOST"), value("DB_PORT"))
                    },
                    if value("REDIS_HOST").is_empty() { String::new() } else { format!("Redis ({})", value("REDIS_HOST")) },
                ];
                let issues = self.laravel.issues.clone();
                div()
                    .flex()
                    .gap_6()
                    .p_3()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1p5()
                            .min_w(px(360.))
                            .child(fact("Laravel version", crate::project_info::laravel_version(self.project.root()).unwrap_or_else(|| "—".into())))
                            .child(fact("Environment", value("APP_ENV")))
                            .child(fact("Application URL", value("APP_URL")))
                            .child(fact("PHP", crate::project_info::php_version().unwrap_or_else(|| "—".into())))
                            .child(div().pt_2().text_color(theme::muted()).child("Databases"))
                            .children(databases.into_iter().filter(|d| !d.trim().is_empty() && d.trim() != ":").enumerate().map(|(ix, db)| {
                                div()
                                    .flex()
                                    .gap_3()
                                    .child(div().text_color(theme::ui_text()).child(SharedString::from(db)))
                                    .child(
                                        div()
                                            .id(("lv-open-db", ix))
                                            .cursor_pointer()
                                            .text_color(theme::accent())
                                            .child("Open in the Database tab")
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.layout.show(super::layout::ToolWindow::Database);
                                                this.ensure_db(cx);
                                                if this.db_is_empty() {
                                                    this.add_application_database(window, cx);
                                                }
                                                cx.notify();
                                            })),
                                    )
                            })),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .when(issues.is_empty(), |d| d.child(div().text_color(theme::success()).child("✓ No issues detected")))
                            .children(issues.into_iter().enumerate().map(|(ix, issue)| {
                                let fix = issue.fix;
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(div().text_color(theme::error()).child("⚠"))
                                    .child(
                                        div()
                                            .flex()
                                            .flex_col()
                                            .child(div().text_color(theme::ui_text()).child(issue.title))
                                            .child(div().text_size(px(12.)).text_color(theme::muted()).child(issue.description)),
                                    )
                                    .when(!fix.is_empty(), |d| {
                                        d.child(
                                            div()
                                                .id(("lv-fix", ix))
                                                .px_2()
                                                .rounded_md()
                                                .border_1()
                                                .border_color(theme::border_strong())
                                                .cursor_pointer()
                                                .hover(|d| d.bg(theme::hover()))
                                                .child(format!("Fix: artisan {}", fix.join(" ")))
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.artisan(&fix.join(" "), cx);
                                                    this.laravel.loaded = false;
                                                })),
                                        )
                                    })
                            })),
                    )
                    .into_any_element()
            }
            LaravelTab::Errors => match &self.laravel.errors {
                None => div().p_3().text_color(theme::muted()).child("Reading storage/logs…").into_any_element(),
                Some(groups) if groups.is_empty() => div().p_3().text_color(theme::success()).child("✓ No errors in storage/logs").into_any_element(),
                Some(groups) => div()
                    .flex()
                    .flex_col()
                    .children(groups.iter().take(200).enumerate().map(|(ix, group)| {
                        let expanded = self.laravel.expanded.contains(&ix);
                        let color = match group.level.as_str() {
                            "ERROR" | "CRITICAL" | "ALERT" | "EMERGENCY" => theme::error(),
                            "WARNING" => theme::syntax(php_syntax::Highlight::Number),
                            _ => theme::muted(),
                        };
                        div()
                            .flex()
                            .flex_col()
                            .border_b_1()
                            .border_color(theme::border())
                            .child(
                                div()
                                    .id(("lv-err", ix))
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .px_2()
                                    .py_1()
                                    .cursor_pointer()
                                    .hover(|d| d.bg(theme::hover()))
                                    .child(div().w(px(70.)).flex_none().text_size(px(11.)).text_color(color).child(SharedString::from(group.level.clone())))
                                    .child(div().flex_1().overflow_hidden().whitespace_nowrap().text_color(theme::ui_text()).child(SharedString::from(group.signature.clone())))
                                    .child(div().text_size(px(11.)).text_color(theme::muted()).child(format!("×{} · {}", group.count, group.last)))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if !this.laravel.expanded.remove(&ix) {
                                            this.laravel.expanded.insert(ix);
                                        }
                                        cx.notify();
                                    })),
                            )
                            .when(expanded, |d| {
                                d.child(
                                    div()
                                        .px_3()
                                        .pb_2()
                                        .font_family(theme::editor_font())
                                        .text_size(px(11.))
                                        .child(div().text_color(theme::text()).child(SharedString::from(group.latest.message.chars().take(600).collect::<String>())))
                                        .children(group.latest.details.iter().take(60).enumerate().map(|(line_ix, line)| {
                                            let link = dashboard::trace_location(self.project.root(), line).is_some();
                                            let text = line.clone();
                                            div()
                                                .id(("lv-trace", ix * 1000 + line_ix))
                                                .whitespace_nowrap()
                                                .overflow_hidden()
                                                .text_color(if link { theme::accent() } else { theme::muted() })
                                                .when(link, |d| d.cursor_pointer().hover(|d| d.underline()))
                                                .child(SharedString::from(line.clone()))
                                                .on_click(cx.listener(move |this, _, window, cx| this.open_trace(&text, window, cx)))
                                        })),
                                )
                            })
                    }))
                    .into_any_element(),
            },
            LaravelTab::Processes => {
                let processes = self.run_processes(cx);
                div()
                    .flex()
                    .flex_col()
                    .p_2()
                    .gap_1()
                    .when(processes.is_empty(), |d| d.child(div().text_color(theme::muted()).child("Nothing is running. Start Server or Run Artisan Command starts one.")))
                    .children(processes.into_iter().map(|(ix, name, exit)| {
                        let (status, color) = match exit {
                            None => ("● running", theme::success()),
                            Some(0) => ("✓ finished", theme::muted()),
                            Some(_) => ("✕ failed", theme::error()),
                        };
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .child(div().w(px(90.)).text_size(px(12.)).text_color(color).child(status))
                            .child(div().flex_1().text_color(theme::ui_text()).child(SharedString::from(name)))
                            .child(div().id(("lv-show", ix)).px_2().cursor_pointer().text_color(theme::accent()).child("Show").on_click(cx.listener(move |this, _, window, cx| this.show_terminal_tab(ix, window, cx))))
                            .when(exit.is_none(), |d| {
                                d.child(div().id(("lv-stop", ix)).px_2().cursor_pointer().text_color(theme::error()).child("Stop").on_click(cx.listener(move |this, _, _, cx| this.stop_terminal_tab(ix, cx))))
                            })
                    }))
                    .into_any_element()
            }
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme::editor_bg())
            .text_size(px(13.))
            .child(header)
            .when(current == LaravelTab::Dashboard, |d| d.child(toolbar))
            .child(div().id("lv-body").flex_1().min_h(px(0.)).overflow_y_scroll().child(body))
    }
}
