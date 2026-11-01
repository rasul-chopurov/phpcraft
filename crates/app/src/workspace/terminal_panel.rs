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
    /// What ⌃R reruns.
    pub last_run: Option<RunConfig>,
    /// The tab being renamed and its name field.
    renaming: Option<(usize, Entity<crate::text_field::TextField>)>,
}

impl Workspace {
    pub(super) fn add_terminal(&mut self, run: Option<RunConfig>, cx: &mut Context<Self>) -> Option<Entity<TerminalView>> {
        let root = self.project.root().to_path_buf();
        self.add_terminal_at(run, root, cx)
    }

    /// A shell (or run configuration) started in `root`.
    pub(super) fn add_terminal_at(&mut self, run: Option<RunConfig>, root: std::path::PathBuf, cx: &mut Context<Self>) -> Option<Entity<TerminalView>> {
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
        let is_test = run.as_ref().is_some_and(|r| matches!(r.kind, run_configs::Kind::Test { .. }));
        if is_test {
            let report = root.join(run_configs::JUNIT_REPORT);
            let _ = std::fs::create_dir_all(report.parent().unwrap_or(&root));
            let _ = std::fs::remove_file(&report);
        }
        let subscription = cx.subscribe(&view, move |this, _, event: &TerminalViewEvent, cx| {
            if matches!(event, TerminalViewEvent::Exited) && is_run {
                // Generators, migrations and composer change files on disk.
                this.rescan(cx);
                if is_test {
                    this.read_test_results(cx);
                }
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
        self.layout.show(super::layout::ToolWindow::Terminal);
        cx.notify();
        Some(view)
    }

    /// Run configurations started from the IDE: (tab, name, exit code if finished).
    pub(super) fn run_processes(&self, cx: &gpui::App) -> Vec<(usize, String, Option<i32>)> {
        self.terminals
            .tabs
            .iter()
            .enumerate()
            .filter_map(|(ix, t)| t.run.as_ref().map(|r| (ix, r.name.clone(), t.view.read(cx).exit_code)))
            .collect()
    }

    pub(super) fn show_terminal_tab(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.terminals.active = ix.min(self.terminals.tabs.len().saturating_sub(1));
        self.layout.show(super::layout::ToolWindow::Terminal);
        if let Some(view) = self.active_terminal() {
            window.focus(&view.focus_handle(cx));
        }
        cx.notify();
    }

    pub(super) fn stop_terminal_tab(&mut self, ix: usize, cx: &mut Context<Self>) {
        if let Some(tab) = self.terminals.tabs.get(ix) {
            tab.view.read(cx).interrupt();
        }
    }

    fn active_terminal(&self) -> Option<&Entity<TerminalView>> {
        self.terminals.tabs.get(self.terminals.active).map(|t| &t.view)
    }

    pub(super) fn toggle_terminal(&mut self, _: &ToggleTerminal, window: &mut Window, cx: &mut Context<Self>) {
        let focused = self.active_terminal().is_some_and(|t| t.focus_handle(cx).is_focused(window));
        if self.layout.is_active(super::layout::ToolWindow::Terminal) && focused {
            self.layout.hide(super::layout::ToolWindow::Terminal);
            self.focus_active(window, cx);
        } else {
            self.layout.show(super::layout::ToolWindow::Terminal);
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

    pub(super) fn terminal_count(&self) -> usize {
        self.terminals.tabs.len()
    }

    pub(super) fn select_terminal(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if ix < self.terminals.tabs.len() {
            self.terminals.active = ix;
            if let Some(view) = self.active_terminal() {
                window.focus(&view.focus_handle(cx));
            }
        }
        cx.notify();
    }

    pub(super) fn close_other_terminals(&mut self, keep: usize, cx: &mut Context<Self>) {
        if keep < self.terminals.tabs.len() {
            let tab = self.terminals.tabs.remove(keep);
            self.terminals.tabs = vec![tab];
            self.terminals.active = 0;
        }
        cx.notify();
    }

    pub(super) fn start_terminal_rename(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.terminals.tabs.get(ix) else { return };
        let title = tab.view.read(cx).title.clone();
        let field = cx.new(|cx| {
            let mut f = crate::text_field::TextField::new("Session name", cx);
            f.set_text(title, cx);
            f
        });
        window.focus(&field.focus_handle(cx));
        self.terminals.renaming = Some((ix, field));
        cx.notify();
    }

    fn finish_terminal_rename(&mut self, apply: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some((ix, field)) = self.terminals.renaming.take() else { return };
        let name = field.read(cx).text().trim().to_string();
        if apply
            && !name.is_empty()
            && let Some(tab) = self.terminals.tabs.get(ix)
        {
            tab.view.update(cx, |view, cx| {
                view.title = name;
                cx.notify();
            });
        }
        self.select_terminal(ix, window, cx);
    }

    /// Right-click on a terminal tab (PhpStorm's menu).
    pub(super) fn open_terminal_menu(&mut self, ix: usize, position: gpui::Point<gpui::Pixels>, cx: &mut Context<Self>) {
        use super::context_menu::MenuCommand as M;
        use super::layout::{Side, ToolWindow};
        let count = self.terminals.tabs.len();
        let dock = |side: Side, label: &'static str| Some((label, "", M::Dock(ToolWindow::Terminal, side), true));
        let items = vec![
            Some(("Rename Session", "", M::TerminalRename(ix), true)),
            None,
            Some(("Close Tab", "", M::TerminalClose(ix), true)),
            Some(("Close All Tabs", "", M::TerminalCloseAll, true)),
            Some(("Close Other Tabs", "", M::TerminalCloseOthers(ix), count > 1)),
            None,
            Some(("New Session", "⌘T", M::Action("workspace::NewTerminal"), true)),
            Some(("Select Next Tab", "", M::TerminalSelect((ix + 1) % count.max(1)), count > 1)),
            Some(("Select Previous Tab", "", M::TerminalSelect((ix + count.max(1) - 1) % count.max(1)), count > 1)),
            None,
            Some(("Settings", "⌘,", M::Action("workspace::OpenSettings"), true)),
            None,
            Some(("Move to", "›", M::Submenu(11, vec![dock(Side::Bottom, "Bottom"), dock(Side::BottomRight, "Bottom Right"), dock(Side::Left, "Left"), dock(Side::Right, "Right")]), true)),
            None,
            Some(("Hide", "⇧Esc", M::HideTool(ToolWindow::Terminal), true)),
        ];
        self.open_menu_at(position, items, cx);
    }

    pub(super) fn close_terminal_tab(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if ix < self.terminals.tabs.len() {
            self.terminals.tabs.remove(ix);
        }
        self.terminals.active = self.terminals.active.min(self.terminals.tabs.len().saturating_sub(1));
        if self.terminals.tabs.is_empty() {
            self.layout.hide(super::layout::ToolWindow::Terminal);
            self.focus_active(window, cx);
        }
        cx.notify();
    }

    pub(super) fn close_terminal(&mut self, _: &CloseTerminal, window: &mut Window, cx: &mut Context<Self>) {
        self.close_terminal_tab(self.terminals.active, window, cx);
    }

    /// Runs `config` in a Run tab (from the editor's ▶ marks).
    pub(crate) fn run_config(&mut self, config: RunConfig, cx: &mut Context<Self>) {
        self.start_run(config, false, cx);
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
    pub(super) fn context_run_config(&self, cx: &Context<Self>) -> Option<RunConfig> {
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
                    .child(crate::icons::icon(crate::icons::Icon::chevron_down, 11., theme::muted()))
                    .on_click(cx.listener(|this, _, window, cx| this.run_popup(&RunPopup, window, cx))),
            )
            .child(
                button("run-start")
                    .child(crate::icons::icon(crate::icons::Icon::play, 13., theme::success()))
                    .on_click(cx.listener(|this, _, window, cx| this.run_last(&RunLast, window, cx))),
            )
            .child(
                button("run-stop")
                    .child(crate::icons::icon(crate::icons::Icon::stop, 13., if running { theme::error() } else { theme::muted() }))
                    .on_click(cx.listener(|this, _, window, cx| this.stop_run(&StopRun, window, cx))),
            )
    }

    pub(super) fn render_terminal_panel(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        if self.terminals.tabs.is_empty() {
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
                .map(|d| match self.terminals.renaming.as_ref().filter(|(r, _)| *r == ix) {
                    Some((_, field)) => d.child(
                        div()
                            .w(px(140.))
                            .flex()
                            .key_context("TerminalRename")
                            .on_action(cx.listener(|this, _: &crate::dialogs::Confirm, window, cx| this.finish_terminal_rename(true, window, cx)))
                            .on_action(cx.listener(|this, _: &crate::dialogs::Cancel, window, cx| this.finish_terminal_rename(false, window, cx)))
                            .child(field.clone()),
                    ),
                    None => d.child(SharedString::from(label)),
                })
                .on_mouse_down(gpui::MouseButton::Right, cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| this.open_terminal_menu(ix, event.position, cx)))
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
                .size_full()
                .flex()
                .flex_col()
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
                            this.layout.hide(super::layout::ToolWindow::Terminal);
                            this.focus_active(window, cx);
                            cx.notify();
                        }))),
                )
                .child(div().flex_1().min_h(px(0.)).child(view)),
        )
    }
}
