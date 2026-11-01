//! Terminal / Run tool window at the bottom (⌥F12), and running configurations.

use std::collections::HashMap;

use gpui::{Context, Entity, Focusable, SharedString, Subscription, Window, actions, div, prelude::*, px};

use super::Workspace;
use super::tools::Output;
use crate::dialogs::{DialogEvent, RunDialog};
use crate::run_configs::{self, RunConfig};
use crate::terminal_view::{TerminalView, TerminalViewEvent};
use crate::theme;

actions!(workspace, [ToggleTerminal, NewTerminal, CloseTerminal, RunPopup, RunLast, RunContext, StopRun, EditRunConfigurations]);

pub(super) struct TerminalTab {
    view: Entity<TerminalView>,
    /// Set for a run configuration's tab.
    run: Option<RunConfig>,
    _subscription: Subscription,
}

#[derive(Default)]
pub(super) struct TerminalState {
    tabs: Vec<TerminalTab>,
    active: usize,
    pub open: bool,
    /// What ⌃R reruns.
    pub last_run: Option<RunConfig>,
}

impl Workspace {
    fn add_terminal(&mut self, run: Option<RunConfig>, cx: &mut Context<Self>) -> Option<Entity<TerminalView>> {
        let root = self.project.root().to_path_buf();
        let (command, title, env) = match &run {
            Some(config) => (
                Some(run_configs::command(config, &root)),
                config.name.clone(),
                config.env.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
            ),
            None => {
                let shell = crate::config::Settings::load_cached().terminal_shell;
                let command = (!shell.trim().is_empty()).then(|| {
                    let mut parts = shell.split_whitespace().map(str::to_string);
                    terminal::Command { program: parts.next().unwrap_or_default(), args: parts.collect() }
                });
                (command, "Local".to_string(), HashMap::new())
            }
        };
        let view = match TerminalView::spawn(&root, command, title.clone(), env, cx) {
            Ok(view) => view,
            Err(err) => {
                self.output = Some(Output { title: "Terminal".into(), text: format!("Could not start {title}: {err}"), running: false, ok: false });
                cx.notify();
                return None;
            }
        };
        let is_run = run.is_some();
        let subscription = cx.subscribe(&view, move |this, _, event: &TerminalViewEvent, cx| {
            if matches!(event, TerminalViewEvent::Exited) && is_run {
                // Generators, migrations and composer change files on disk.
                this.rescan(cx);
            }
            cx.notify();
        });
        // Replace a finished tab of the same configuration instead of piling up tabs.
        let reuse = run.as_ref().and_then(|config| {
            self.terminals
                .tabs
                .iter()
                .position(|t| t.run.as_ref().is_some_and(|r| r.name == config.name) && !t.view.read(cx).is_running())
        });
        let tab = TerminalTab { view: view.clone(), run, _subscription: subscription };
        match reuse {
            Some(ix) => {
                self.terminals.tabs[ix] = tab;
                self.terminals.active = ix;
            }
            None => {
                self.terminals.tabs.push(tab);
                self.terminals.active = self.terminals.tabs.len() - 1;
            }
        }
        self.terminals.open = true;
        cx.notify();
        Some(view)
    }

    fn active_terminal(&self) -> Option<&Entity<TerminalView>> {
        self.terminals.tabs.get(self.terminals.active).map(|t| &t.view)
    }

    pub(super) fn toggle_terminal(&mut self, _: &ToggleTerminal, window: &mut Window, cx: &mut Context<Self>) {
        let focused = self.active_terminal().is_some_and(|t| t.focus_handle(cx).is_focused(window));
        if self.terminals.open && focused {
            self.terminals.open = false;
            self.focus_active(window, cx);
        } else {
            self.terminals.open = true;
            if self.terminals.tabs.is_empty() {
                self.add_terminal(None, cx);
            }
            if let Some(view) = self.active_terminal() {
                window.focus(&view.focus_handle(cx));
            }
        }
        cx.notify();
    }

    pub(super) fn new_terminal(&mut self, _: &NewTerminal, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(view) = self.add_terminal(None, cx) {
            window.focus(&view.focus_handle(cx));
        }
    }

    fn close_terminal_tab(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if ix < self.terminals.tabs.len() {
            self.terminals.tabs.remove(ix);
        }
        self.terminals.active = self.terminals.active.min(self.terminals.tabs.len().saturating_sub(1));
        if self.terminals.tabs.is_empty() {
            self.terminals.open = false;
            self.focus_active(window, cx);
        }
        cx.notify();
    }

    pub(super) fn close_terminal(&mut self, _: &CloseTerminal, window: &mut Window, cx: &mut Context<Self>) {
        self.close_terminal_tab(self.terminals.active, window, cx);
    }

    /// Runs `config` in a Run tab (saving it first when asked).
    pub(super) fn start_run(&mut self, config: RunConfig, save: bool, cx: &mut Context<Self>) {
        if save {
            let _ = run_configs::save(self.project.root(), &config);
        }
        self.terminals.last_run = Some(config.clone());
        self.add_terminal(Some(config), cx);
    }

    pub(super) fn run_popup(&mut self, _: &RunPopup, window: &mut Window, cx: &mut Context<Self>) {
        let root = self.project.root().to_path_buf();
        let context = self.context_run_config(cx);
        let saved = run_configs::load(&root);
        let detected = run_configs::detect(&root);
        let dialog = cx.new(|cx| RunDialog::new(context, saved, detected, window, cx));
        self.show_modal(dialog, window, cx);
    }

    pub(super) fn run_last(&mut self, _: &RunLast, window: &mut Window, cx: &mut Context<Self>) {
        match self.terminals.last_run.clone().or_else(|| run_configs::load(self.project.root()).into_iter().next()) {
            Some(config) => self.start_run(config, false, cx),
            None => self.run_popup(&RunPopup, window, cx),
        }
    }

    pub(super) fn run_context(&mut self, _: &RunContext, window: &mut Window, cx: &mut Context<Self>) {
        match self.context_run_config(cx) {
            Some(config) => self.start_run(config, false, cx),
            None => self.run_popup(&RunPopup, window, cx),
        }
    }

    /// Stops the active Run tab's process, or the newest running configuration.
    pub(super) fn stop_run(&mut self, _: &StopRun, _: &mut Window, cx: &mut Context<Self>) {
        let tab = self
            .terminals
            .tabs
            .get(self.terminals.active)
            .filter(|t| t.run.is_some() && t.view.read(cx).is_running())
            .or_else(|| self.terminals.tabs.iter().rev().find(|t| t.run.is_some() && t.view.read(cx).is_running()));
        if let Some(tab) = tab {
            tab.view.read(cx).interrupt();
        }
    }

    pub(super) fn edit_run_configurations(&mut self, _: &EditRunConfigurations, window: &mut Window, cx: &mut Context<Self>) {
        if let Ok(path) = run_configs::ensure_file(self.project.root()) {
            self.open_file(&path, window, cx);
        }
    }

    /// The test or script under the caret of the active editor.
    fn context_run_config(&self, cx: &Context<Self>) -> Option<RunConfig> {
        let editor = self.tabs.get(self.active)?.read(cx);
        let path = editor.buffer.path()?.to_path_buf();
        let text = editor.buffer.text();
        let offset = editor.buffer.char_to_byte(editor.buffer.selection.head);
        run_configs::for_context(self.project.root(), &path, &text, offset)
    }

    pub(super) fn on_run_dialog_event(&mut self, event: &DialogEvent, window: &mut Window, cx: &mut Context<Self>) -> bool {
        match event {
            DialogEvent::Run { config, save } => {
                let (config, save) = (config.clone(), *save);
                self.dismiss_modal(window, cx);
                self.start_run(config, save, cx);
                true
            }
            DialogEvent::EditRunConfigurations => {
                self.dismiss_modal(window, cx);
                self.edit_run_configurations(&EditRunConfigurations, window, cx);
                true
            }
            _ => false,
        }
    }

    /// `▶ name ▾  ▶ ■` in the title bar.
    pub(super) fn render_run_widget(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let selected = self.terminals.last_run.clone().or_else(|| run_configs::load(self.project.root()).into_iter().next());
        let running = self.terminals.tabs.iter().any(|t| t.run.is_some() && t.view.read(cx).is_running());
        let button = |id: &'static str| {
            div().id(id).flex().items_center().gap_1().px_1p5().h(px(22.)).rounded_sm().cursor_pointer().hover(|d| d.bg(theme::hover()))
        };
        div()
            .flex()
            .items_center()
            .gap_0p5()
            .child(
                button("run-config")
                    .child(SharedString::from(selected.map(|c| c.name).unwrap_or_else(|| "Current File".into())))
                    .child(div().text_color(theme::muted()).child("▾"))
                    .on_click(cx.listener(|this, _, window, cx| this.run_popup(&RunPopup, window, cx))),
            )
            .child(
                button("run-start")
                    .text_color(theme::success())
                    .child("▶")
                    .on_click(cx.listener(|this, _, window, cx| this.run_last(&RunLast, window, cx))),
            )
            .child(
                button("run-stop")
                    .text_color(if running { theme::error() } else { theme::muted() })
                    .child("■")
                    .on_click(cx.listener(|this, _, window, cx| this.stop_run(&StopRun, window, cx))),
            )
    }

    pub(super) fn render_terminal_panel(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        if !self.terminals.open || self.terminals.tabs.is_empty() {
            return None;
        }
        let active = self.terminals.active;
        let tabs = self.terminals.tabs.iter().enumerate().map(|(ix, tab)| {
            let view = tab.view.read(cx);
            let (dot, color) = match view.exit_code {
                None if tab.run.is_some() => ("●", theme::success()),
                None => ("", theme::muted()),
                Some(0) => ("✓", theme::success()),
                Some(_) => ("✕", theme::error()),
            };
            let label = if tab.run.is_some() { format!("▶ {}", view.title) } else { view.title.clone() };
            div()
                .id(("term-tab", ix))
                .flex()
                .items_center()
                .gap_1()
                .pl_2()
                .pr_1()
                .h(px(22.))
                .rounded_md()
                .cursor_pointer()
                .when(ix == active, |d| d.bg(theme::active_row()).text_color(theme::ui_text()))
                .when(ix != active, |d| d.text_color(theme::muted()).hover(|d| d.bg(theme::hover())))
                .when(!dot.is_empty(), |d| d.child(div().text_color(color).text_size(px(10.)).child(dot)))
                .child(SharedString::from(label))
                .child(
                    div()
                        .id(("term-close", ix))
                        .px_1()
                        .rounded_sm()
                        .text_color(theme::muted())
                        .hover(|d| d.bg(theme::hover()))
                        .child("×")
                        .on_click(cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.close_terminal_tab(ix, window, cx);
                        })),
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.terminals.active = ix;
                    if let Some(view) = this.active_terminal() {
                        window.focus(&view.focus_handle(cx));
                    }
                    cx.notify();
                }))
        });
        let tool = |id: &'static str, label: &'static str| {
            div().id(id).px_1p5().h(px(22.)).flex().items_center().rounded_sm().cursor_pointer().text_color(theme::muted()).hover(|d| d.bg(theme::hover())).child(label)
        };
        let active_run = self.terminals.tabs.get(active).and_then(|t| t.run.clone());
        let view = self.active_terminal()?.clone();
        Some(
            div()
                .flex()
                .flex_col()
                .flex_none()
                .h(px(300.))
                .border_t_1()
                .border_color(theme::border_strong())
                .text_size(px(12.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .px_2()
                        .h(px(30.))
                        .bg(theme::panel_bg())
                        .child(div().pr_2().text_color(theme::ui_text()).child(if active_run.is_some() { "Run" } else { "Terminal" }))
                        .children(tabs)
                        .child(tool("term-new", "+").on_click(cx.listener(|this, _, window, cx| this.new_terminal(&NewTerminal, window, cx))))
                        .child(div().flex_1())
                        .when_some(active_run, |d, config| {
                            d.child(tool("term-rerun", "⟳ Rerun").on_click(cx.listener(move |this, _, _, cx| this.start_run(config.clone(), false, cx))))
                                .child(tool("term-stop", "■ Stop").on_click(cx.listener(|this, _, window, cx| this.stop_run(&StopRun, window, cx))))
                        })
                        .child(tool("term-hide", "—").on_click(cx.listener(|this, _, window, cx| {
                            this.terminals.open = false;
                            this.focus_active(window, cx);
                            cx.notify();
                        }))),
                )
                .child(div().flex_1().min_h(px(0.)).child(view)),
        )
    }
}
