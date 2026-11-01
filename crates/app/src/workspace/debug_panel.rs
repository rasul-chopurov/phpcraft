//! The Debug tool window (⌘5): listen for Xdebug, step through PHP, read the stack and
//! variables, evaluate expressions. Run ▸ Debug (⌃D) runs the current configuration with
//! Xdebug turned on.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use dbgp::{Frame, Property};
use gpui::{AppContext, Context, Entity, FocusHandle, Task, Window, actions, div, prelude::*, px};

use super::Workspace;
use super::layout::ToolWindow;
use crate::debugger::{Command, Controller, Event};
use crate::text_field::TextField;
use crate::theme;

actions!(
    workspace,
    [ToggleDebugListen, DebugResume, DebugStepOver, DebugStepInto, DebugStepOut, DebugStop, DebugEvaluate, DebugContext]
);

struct Paused {
    file: PathBuf,
    line: usize,
    frames: Vec<(Frame, PathBuf)>,
    variables: Vec<Property>,
    frame: usize,
}

pub(super) struct DebugState {
    controller: Controller,
    listening: Option<u16>,
    connected: bool,
    paused: Option<Paused>,
    /// Children fetched for expanded arrays and objects, by full name.
    loaded: HashMap<String, Vec<Property>>,
    open: HashSet<String>,
    eval: Entity<TextField>,
    eval_focus: FocusHandle,
    results: Vec<(String, Result<Property, String>)>,
    message: Option<String>,
    _pump: Task<()>,
}

impl Workspace {
    /// Starts listening for Xdebug connections (the controller's events drive the panel).
    fn start_debugger(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.debug.is_some() {
            return;
        }
        let controller = Controller::start(self.project.root(), crate::breakpoints::all());
        let events = controller.events.clone();
        let pump = cx.spawn_in(window, async move |this, cx| {
            while let Ok(event) = events.recv().await {
                if this.update_in(cx, |this, window, cx| this.on_debug_event(event, window, cx)).is_err() {
                    break;
                }
            }
        });
        let eval = cx.new(|cx| TextField::new("Evaluate expression (Enter)", cx));
        self.debug = Some(DebugState {
            controller,
            listening: None,
            connected: false,
            paused: None,
            loaded: HashMap::new(),
            open: HashSet::new(),
            eval,
            eval_focus: cx.focus_handle(),
            results: Vec::new(),
            message: None,
            _pump: pump,
        });
        self.layout.show(ToolWindow::Debug);
        cx.notify();
    }

    fn stop_debugger(&mut self, cx: &mut Context<Self>) {
        self.debug = None;
        self.clear_execution_line(cx);
        cx.notify();
    }

    fn clear_execution_line(&mut self, cx: &mut Context<Self>) {
        for editor in self.tabs.clone().into_iter().chain(self.panes.iter().flatten().flat_map(|p| p.tabs.clone())) {
            editor.update(cx, |e, cx| e.set_execution_line(None, cx));
        }
    }

    fn on_debug_event(&mut self, event: Event, window: &mut Window, cx: &mut Context<Self>) {
        let Some(debug) = self.debug.as_mut() else { return };
        match event {
            Event::Listening(port) => {
                debug.listening = Some(port);
                debug.message = Some(format!("Listening for Xdebug on port {port}"));
            }
            Event::Connected(file) => {
                debug.connected = true;
                debug.message = Some(format!("Connected: {}", file.display()));
            }
            Event::Paused { file, line, frames, variables } => {
                debug.loaded.clear();
                debug.open.clear();
                debug.paused = Some(Paused { file: file.clone(), line, frames, variables, frame: 0 });
                debug.message = None;
                self.show_execution_point(&file, line, window, cx);
                self.layout.show(ToolWindow::Debug);
            }
            Event::Variables { frame, variables } => {
                if let Some(paused) = debug.paused.as_mut() {
                    paused.frame = frame;
                    paused.variables = variables;
                }
                debug.loaded.clear();
            }
            Event::Expanded(property) => {
                debug.loaded.insert(property.fullname.clone(), property.children);
            }
            Event::Evaluated { expression, result } => debug.results.insert(0, (expression, result)),
            Event::Finished => {
                debug.connected = false;
                debug.paused = None;
                debug.message = Some("PHP finished. Still listening.".into());
                self.clear_execution_line(cx);
            }
            Event::Error(message) => debug.message = Some(message),
            Event::Closed => {
                self.debug = None;
                self.clear_execution_line(cx);
            }
        }
        cx.notify();
    }

    fn show_execution_point(&mut self, file: &std::path::Path, line: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.clear_execution_line(cx);
        if !file.is_file() {
            return;
        }
        self.open_file(file, window, cx);
        if let Some(editor) = self.tabs.get(self.active).cloned() {
            editor.update(cx, |e, cx| e.set_execution_line(Some(line.saturating_sub(1)), cx));
        }
    }

    /// Sends the current breakpoints to a running session.
    pub(crate) fn breakpoints_changed(&mut self) {
        if let Some(debug) = &self.debug {
            debug.controller.send(Command::Breakpoints(crate::breakpoints::all()));
        }
    }

    fn debug_command(&mut self, command: Command, cx: &mut Context<Self>) {
        if let Some(debug) = self.debug.as_mut()
            && debug.paused.is_some()
        {
            debug.controller.send(command);
            debug.paused = None;
            self.clear_execution_line(cx);
            cx.notify();
        }
    }

    pub(super) fn toggle_debug_listen(&mut self, _: &ToggleDebugListen, window: &mut Window, cx: &mut Context<Self>) {
        if self.debug.is_some() {
            self.stop_debugger(cx);
        } else {
            self.start_debugger(window, cx);
        }
    }

    pub(super) fn debug_resume(&mut self, _: &DebugResume, _: &mut Window, cx: &mut Context<Self>) {
        self.debug_command(Command::Resume, cx);
    }

    pub(super) fn debug_step_over(&mut self, _: &DebugStepOver, _: &mut Window, cx: &mut Context<Self>) {
        self.debug_command(Command::StepOver, cx);
    }

    pub(super) fn debug_step_into(&mut self, _: &DebugStepInto, _: &mut Window, cx: &mut Context<Self>) {
        self.debug_command(Command::StepInto, cx);
    }

    pub(super) fn debug_step_out(&mut self, _: &DebugStepOut, _: &mut Window, cx: &mut Context<Self>) {
        self.debug_command(Command::StepOut, cx);
    }

    pub(super) fn debug_stop(&mut self, _: &DebugStop, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(debug) = self.debug.as_mut() {
            if debug.paused.is_some() {
                debug.controller.send(Command::Stop);
                debug.paused = None;
                self.clear_execution_line(cx);
            } else {
                self.stop_debugger(cx);
            }
            cx.notify();
        }
    }

    pub(super) fn debug_evaluate(&mut self, _: &DebugEvaluate, _: &mut Window, cx: &mut Context<Self>) {
        let Some(debug) = self.debug.as_ref() else { return };
        let expression = debug.eval.read(cx).text().trim().to_string();
        if expression.is_empty() || debug.paused.is_none() {
            return;
        }
        debug.controller.send(Command::Evaluate(expression));
        let field = debug.eval.clone();
        field.update(cx, |f, cx| f.set_text("", cx));
    }

    /// Run ▸ Debug: listens, then runs the context configuration with Xdebug on.
    pub(super) fn debug_context(&mut self, _: &DebugContext, window: &mut Window, cx: &mut Context<Self>) {
        self.start_debugger(window, cx);
        let Some(mut config) = self.context_run_config(cx) else { return };
        let port = crate::debugger::DebugConfig::load(self.project.root()).port;
        config.env.insert("XDEBUG_MODE".into(), "debug".into());
        config.env.insert("XDEBUG_SESSION".into(), "PHPCRAFT".into());
        config.env.insert("XDEBUG_CONFIG".into(), format!("client_host=127.0.0.1 client_port={port} idekey=PHPCRAFT"));
        config.name = format!("Debug {}", config.name);
        self.start_run(config, false, cx);
    }

    fn select_frame(&mut self, frame: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(debug) = self.debug.as_ref() else { return };
        let Some(paused) = debug.paused.as_ref() else { return };
        let Some((f, path)) = paused.frames.get(frame).cloned() else { return };
        debug.controller.send(Command::SelectFrame(frame));
        self.show_execution_point(&path, f.line, window, cx);
    }

    fn toggle_variable(&mut self, property: &Property, cx: &mut Context<Self>) {
        let Some(debug) = self.debug.as_mut() else { return };
        let name = property.fullname.clone();
        if !debug.open.remove(&name) {
            debug.open.insert(name.clone());
            if property.children.is_empty() && !debug.loaded.contains_key(&name) {
                let frame = debug.paused.as_ref().map_or(0, |p| p.frame);
                debug.controller.send(Command::Expand { fullname: name, frame });
            }
        }
        cx.notify();
    }

    pub(super) fn render_debug_panel(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let button = |id: &'static str, label: &'static str, enabled: bool, action: fn(&mut Workspace, &mut Window, &mut Context<Workspace>)| {
            div()
                .id(id)
                .px_2()
                .py_0p5()
                .rounded_sm()
                .text_color(if enabled { theme::ui_text() } else { theme::muted() })
                .when(enabled, |d| d.hover(|d| d.bg(theme::hover())).cursor_pointer())
                .child(label)
                .when(enabled, |d| d.on_click(cx.listener(move |this, _, window, cx| action(this, window, cx))))
        };
        let Some(debug) = self.debug.as_ref() else {
            return div()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_2()
                .text_color(theme::muted())
                .child("Not listening for Xdebug.")
                .child(button("debug-listen", "Start Listening (Run ▸ Start Listening for PHP Debug Connections)", true, |this, window, cx| this.start_debugger(window, cx)))
                .child(div().text_size(px(12.)).child("Run ▸ Debug (⌃D) runs the current file or test with Xdebug. Path mappings: .phpcraft/debug.json"))
                .into_any_element();
        };
        let paused = debug.paused.is_some();
        let toolbar = div()
            .flex()
            .items_center()
            .gap_1()
            .px_2()
            .h(px(30.))
            .border_b_1()
            .border_color(theme::border())
            .child(button("debug-resume", "▶ Resume (F9)", paused, |this, w, cx| this.debug_resume(&DebugResume, w, cx)))
            .child(button("debug-over", "Step Over (F8)", paused, |this, w, cx| this.debug_step_over(&DebugStepOver, w, cx)))
            .child(button("debug-into", "Step Into (F7)", paused, |this, w, cx| this.debug_step_into(&DebugStepInto, w, cx)))
            .child(button("debug-out", "Step Out (⇧F8)", paused, |this, w, cx| this.debug_step_out(&DebugStepOut, w, cx)))
            .child(button("debug-stop", "■ Stop", true, |this, w, cx| this.debug_stop(&DebugStop, w, cx)))
            .child(div().flex_1())
            .child(div().text_size(px(12.)).text_color(theme::muted()).child(match (&debug.paused, &debug.message, debug.listening) {
                (Some(p), ..) => format!("Paused at {}:{}", p.file.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default(), p.line),
                (None, Some(m), _) => m.clone(),
                (None, None, Some(port)) => format!("Listening on {port}"),
                _ => String::new(),
            }));

        let frames = debug.paused.as_ref().map(|p| {
            let current = p.frame;
            div().flex().flex_col().children(p.frames.iter().enumerate().map(|(ix, (frame, path))| {
                let file = path.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
                div()
                    .id(("debug-frame", ix))
                    .px_2()
                    .py_0p5()
                    .cursor_pointer()
                    .when(ix == current, |d| d.bg(theme::selection()))
                    .hover(|d| d.bg(theme::hover()))
                    .child(format!("{}  {file}:{}", frame.function, frame.line))
                    .on_click(cx.listener(move |this, _, window, cx| this.select_frame(ix, window, cx)))
            }))
        });

        let mut rows = Vec::new();
        if let Some(p) = &debug.paused {
            self.variable_rows(&p.variables, 0, &mut rows, cx);
        }
        let results = debug.results.iter().take(20).map(|(expr, result)| {
            let value = match result {
                Ok(p) => p.display(),
                Err(e) => e.clone(),
            };
            div().px_2().child(format!("{expr} = {value}"))
        });
        div()
            .size_full()
            .flex()
            .flex_col()
            .text_size(px(12.5))
            .child(toolbar)
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.))
                    .flex()
                    .child(div().id("debug-frames").w(px(280.)).h_full().overflow_y_scroll().border_r_1().border_color(theme::border()).children(frames))
                    .child(
                        div()
                            .flex_1()
                            .h_full()
                            .flex()
                            .flex_col()
                            .child(div().id("debug-vars").flex_1().min_h(px(0.)).overflow_y_scroll().font_family(theme::editor_font()).children(rows))
                            .child(
                                div()
                                    .key_context("DebugEval")
                                    .track_focus(&debug.eval_focus)
                                    .border_t_1()
                                    .border_color(theme::border())
                                    .p_1()
                                    .child(debug.eval.clone())
                                    .child(div().font_family(theme::editor_font()).children(results)),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn variable_rows(&self, properties: &[Property], depth: usize, out: &mut Vec<gpui::AnyElement>, cx: &mut Context<Self>) {
        let Some(debug) = self.debug.as_ref() else { return };
        for property in properties {
            let open = debug.open.contains(&property.fullname);
            let arrow = if property.has_children { if open { "▾ " } else { "▸ " } } else { "  " };
            let p = property.clone();
            out.push(
                div()
                    .id(gpui::SharedString::from(format!("var-{}-{depth}", property.fullname)))
                    .pl(px(8. + depth as f32 * 14.))
                    .flex()
                    .gap_2()
                    .when(property.has_children, |d| d.cursor_pointer().hover(|d| d.bg(theme::hover())))
                    .child(div().text_color(theme::syntax(php_syntax::Highlight::Variable)).child(format!("{arrow}{}", property.name)))
                    .child(div().text_color(theme::muted()).child(property.kind.clone()))
                    .child(div().text_color(theme::text()).child(property.display()))
                    .when(property.has_children, |d| d.on_click(cx.listener(move |this, _, _, cx| this.toggle_variable(&p, cx))))
                    .into_any_element(),
            );
            if open {
                let children = if property.children.is_empty() { debug.loaded.get(&property.fullname).cloned().unwrap_or_default() } else { property.children.clone() };
                self.variable_rows(&children, depth + 1, out, cx);
            }
        }
    }
}
