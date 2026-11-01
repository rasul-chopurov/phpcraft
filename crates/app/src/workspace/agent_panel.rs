//! AI Assistant tool window (right side): a chat with a Claude agent that works in the
//! project through tools. Changes and commands wait for approval in the chat.

use std::sync::mpsc;
use std::time::Duration;

use agent::acp::{AcpSession, KNOWN_AGENTS};
use agent::{AgentConfig, AgentEvent, Session};
use gpui::{AppContext, Context, Entity, Focusable, SharedString, Task, Window, actions, div, prelude::*, px};
use serde_json::Value;

use super::Workspace;
use crate::editor_view::EditorView;
use crate::text_field::TextField;
use crate::theme;

actions!(workspace, [ToggleAgent, SendAgentMessage, NewAgentChat]);

enum ChatItem {
    User(String),
    Assistant(String),
    Thinking(String),
    Tool { id: String, name: String, input: Value, output: Option<(String, bool)>, approval: Option<mpsc::Sender<bool>> },
    Notice(String),
    Error(String),
}

/// The conversation's backend: Claude over the API, or an external agent over ACP.
#[derive(Clone)]
enum Backend {
    Api(Session),
    Acp(AcpSession),
}

impl Backend {
    fn send(&self, text: String, events: mpsc::Sender<AgentEvent>) {
        match self {
            Backend::Api(s) => s.send(text, events),
            Backend::Acp(s) => s.send(text, events),
        }
    }

    fn cancel(&self) {
        match self {
            Backend::Api(s) => s.cancel(),
            Backend::Acp(s) => s.cancel(),
        }
    }
}

pub(super) struct AgentState {
    pub open: bool,
    session: Option<Backend>,
    items: Vec<ChatItem>,
    input: Option<Entity<EditorView>>,
    key_field: Option<Entity<TextField>>,
    running: bool,
    _pump: Option<Task<()>>,
}

impl Default for AgentState {
    fn default() -> Self {
        Self { open: false, session: None, items: Vec::new(), input: None, key_field: None, running: false, _pump: None }
    }
}

fn config_path() -> std::path::PathBuf {
    crate::config::dir().join("agent.json")
}

pub(crate) fn load_config() -> AgentConfig {
    std::fs::read_to_string(config_path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn save_config(config: &AgentConfig) {
    let _ = std::fs::create_dir_all(crate::config::dir());
    let path = config_path();
    let _ = std::fs::write(&path, serde_json::to_string_pretty(config).unwrap_or_default());
    // The file holds an API key: keep it private to the user.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
}

/// One-line summary of a tool call for the chat.
fn describe(name: &str, input: &Value) -> String {
    let s = |k: &str| input[k].as_str().unwrap_or("").to_string();
    match name {
        "read_file" => format!("Read {}", s("path")),
        "list_files" => format!("List files {}", s("filter")),
        "search" => format!("Search “{}”", s("query")),
        "edit_file" => format!("Edit {}", s("path")),
        "write_file" => format!("Write {}", s("path")),
        "run_command" => format!("Run `{}`", s("command")),
        other => other.to_string(),
    }
}

impl Workspace {
    pub(super) fn toggle_agent(&mut self, _: &ToggleAgent, window: &mut Window, cx: &mut Context<Self>) {
        self.agent.open = !self.agent.open;
        if self.agent.open {
            self.ensure_agent_input(cx);
            if let Some(input) = &self.agent.input {
                window.focus(&input.focus_handle(cx));
            }
        } else {
            self.focus_active(window, cx);
        }
        cx.notify();
    }

    fn ensure_agent_input(&mut self, cx: &mut Context<Self>) {
        if self.agent.input.is_none() {
            let intel = self.intel.clone();
            let workspace = cx.entity().downgrade();
            self.agent.input = Some(cx.new(|cx| EditorView::plain(intel, workspace, cx)));
        }
        let config = load_config();
        if config.provider == "anthropic" && config.resolved_key().is_none() && self.agent.key_field.is_none() {
            self.agent.key_field = Some(cx.new(|cx| TextField::new("sk-ant-…", cx)));
        }
    }

    pub(super) fn new_agent_chat(&mut self, _: &NewAgentChat, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(session) = &self.agent.session {
            session.cancel();
        }
        self.agent.session = None;
        self.agent.items.clear();
        self.agent.running = false;
        self.agent._pump = None;
        cx.notify();
    }

    fn save_agent_key(&mut self, cx: &mut Context<Self>) {
        let Some(field) = &self.agent.key_field else { return };
        let key = field.read(cx).text().trim().to_string();
        if key.is_empty() {
            return;
        }
        let mut config = load_config();
        config.api_key = key;
        save_config(&config);
        self.agent.key_field = None;
        self.agent.session = None;
        cx.notify();
    }

    /// Where the user is: the active file, caret line and selection.
    fn agent_context(&self, cx: &Context<Self>) -> String {
        let Some(editor) = self.tabs.get(self.active) else { return String::new() };
        let buffer = &editor.read(cx).buffer;
        let Some(path) = buffer.path() else { return String::new() };
        let relative = path.strip_prefix(self.project.root()).unwrap_or(path).display().to_string();
        let (line, _) = buffer.char_to_line_col(buffer.selection.head);
        let selected = buffer.selected_text();
        let mut context = format!("\n\n<ide_context>Open file: {relative} (caret on line {})", line + 1);
        if !selected.trim().is_empty() {
            context.push_str(&format!("\nSelected text:\n```\n{}\n```", selected.chars().take(8000).collect::<String>()));
        }
        context.push_str("</ide_context>");
        context
    }

    pub(super) fn send_agent_message(&mut self, _: &SendAgentMessage, _: &mut Window, cx: &mut Context<Self>) {
        let Some(input) = self.agent.input.clone() else { return };
        let text = input.read(cx).buffer.text().trim().to_string();
        if text.is_empty() || self.agent.running {
            return;
        }
        let config = load_config();
        let is_api = config.provider == "anthropic";
        if is_api && config.resolved_key().is_none() {
            self.agent.items.push(ChatItem::Error("Add an Anthropic API key first.".into()));
            cx.notify();
            return;
        }
        input.update(cx, |editor, cx| editor.replace_all_text("", cx));
        self.agent.items.push(ChatItem::User(text.clone()));
        self.agent.running = true;
        let prompt = format!("{text}{}", self.agent_context(cx));
        let root = self.project.root().to_path_buf();
        let existing = self.agent.session.clone();
        let (tx, rx) = mpsc::channel();
        self.agent._pump = Some(cx.spawn(async move |this, cx| {
            // Start the backend on first use (an ACP agent takes a few seconds to boot).
            let backend = match existing {
                Some(backend) => Ok(backend),
                None if is_api => Ok(Backend::Api(Session::new(root, config))),
                None => {
                    let agent = KNOWN_AGENTS.iter().find(|a| a.name == config.provider).cloned();
                    cx.background_spawn(async move {
                        let agent = agent.ok_or("Unknown agent")?;
                        AcpSession::start(&agent, &root).map(Backend::Acp)
                    })
                    .await
                }
            };
            let backend = match backend {
                Ok(backend) => backend,
                Err(err) => {
                    let _ = this.update(cx, |this, cx| {
                        this.agent.items.push(ChatItem::Error(err.to_string()));
                        this.agent.running = false;
                        cx.notify();
                    });
                    return;
                }
            };
            let _ = this.update(cx, |this, _| this.agent.session = Some(backend.clone()));
            backend.send(prompt, tx);
            // Poll the agent's events while it works.
            loop {
                cx.background_executor().timer(Duration::from_millis(40)).await;
                let events: Vec<AgentEvent> = rx.try_iter().collect();
                let done = events.iter().any(|e| matches!(e, AgentEvent::Done));
                let alive = this
                    .update(cx, |this, cx| {
                        for event in events {
                            this.on_agent_event(event, cx);
                        }
                        cx.notify();
                    })
                    .is_ok();
                if done || !alive {
                    break;
                }
            }
        }));
        cx.notify();
    }

    fn on_agent_event(&mut self, event: AgentEvent, cx: &mut Context<Self>) {
        let items = &mut self.agent.items;
        match event {
            AgentEvent::Text(text) => match items.last_mut() {
                Some(ChatItem::Assistant(current)) => current.push_str(&text),
                _ => items.push(ChatItem::Assistant(text)),
            },
            AgentEvent::Thinking(text) => match items.last_mut() {
                Some(ChatItem::Thinking(current)) => current.push_str(&text),
                _ => items.push(ChatItem::Thinking(text)),
            },
            AgentEvent::ToolCall { id, name, input } => items.push(ChatItem::Tool { id, name, input, output: None, approval: None }),
            AgentEvent::Approve { id, name, input, reply } => {
                match items.iter_mut().rev().find(|i| matches!(i, ChatItem::Tool { id: t, .. } if *t == id)) {
                    Some(ChatItem::Tool { approval, .. }) => *approval = Some(reply),
                    // An ACP agent may ask before announcing the tool call.
                    _ => items.push(ChatItem::Tool { id, name, input, output: None, approval: Some(reply) }),
                }
            }
            AgentEvent::ToolResult { id, output, is_error } => {
                if let Some(ChatItem::Tool { output: slot, approval, .. }) =
                    items.iter_mut().rev().find(|i| matches!(i, ChatItem::Tool { id: t, .. } if *t == id))
                {
                    *slot = Some((output, is_error));
                    *approval = None;
                }
            }
            AgentEvent::FilesChanged(paths) => {
                for path in paths {
                    if let Some(tab) = self.active_tab_for(&path, cx) {
                        tab.update(cx, |editor, cx| editor.reload_if_clean(&path, cx));
                    }
                }
                self.rescan(cx);
            }
            AgentEvent::Notice(text) => items.push(ChatItem::Notice(text)),
            AgentEvent::Error(text) => items.push(ChatItem::Error(text)),
            AgentEvent::Done => self.agent.running = false,
        }
    }

    fn answer_approval(&mut self, ix: usize, allow: bool, cx: &mut Context<Self>) {
        if let Some(ChatItem::Tool { approval, .. }) = self.agent.items.get_mut(ix)
            && let Some(reply) = approval.take()
        {
            let _ = reply.send(allow);
        }
        cx.notify();
    }

    fn set_provider(&mut self, provider: &str, cx: &mut Context<Self>) {
        let mut config = load_config();
        config.provider = provider.to_string();
        save_config(&config);
        if let Some(session) = self.agent.session.take() {
            session.cancel();
        }
        self.agent.items.clear();
        self.agent.key_field = None;
        self.ensure_agent_input(cx);
        cx.notify();
    }

    fn stop_agent(&mut self, cx: &mut Context<Self>) {
        if let Some(session) = &self.agent.session {
            session.cancel();
        }
        // Unblock a pending approval so the loop can end.
        for item in &mut self.agent.items {
            if let ChatItem::Tool { approval, .. } = item
                && let Some(reply) = approval.take()
            {
                let _ = reply.send(false);
            }
        }
        cx.notify();
    }

    pub(super) fn render_agent_panel(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        if !self.agent.open {
            return None;
        }
        let small_button = |id: SharedString, label: &'static str| {
            div().id(id).px_2().h(px(22.)).flex().items_center().rounded_md().cursor_pointer().border_1().border_color(theme::border_strong()).hover(|d| d.bg(theme::hover())).child(label)
        };
        let items = self.agent.items.iter().enumerate().map(|(ix, item)| -> gpui::AnyElement {
            match item {
                ChatItem::User(text) => div()
                    .flex()
                    .justify_end()
                    .child(
                        div()
                    .max_w(px(300.))
                    .px_3()
                    .py_2()
                    .rounded_lg()
                    .bg(theme::selection())
                    .text_color(theme::ui_text())
                    .child(SharedString::from(text.clone())),
                    )
                    .into_any_element(),
                ChatItem::Assistant(text) => {
                    let blocks = crate::markdown_preview::parse(text);
                    div().text_size(px(13.)).child(crate::markdown_preview::render_inline(&blocks)).into_any_element()
                }
                ChatItem::Thinking(text) => div()
                    .pl_2()
                    .border_l_2()
                    .border_color(theme::border_strong())
                    .text_size(px(12.))
                    .text_color(theme::muted())
                    .child(SharedString::from(text.chars().rev().take(400).collect::<Vec<_>>().into_iter().rev().collect::<String>()))
                    .into_any_element(),
                ChatItem::Tool { name, input, output, approval, .. } => {
                    let (status, color) = match (output, approval) {
                        (_, Some(_)) => ("needs approval", theme::accent()),
                        (None, None) => ("running…", theme::muted()),
                        (Some((_, false)), _) => ("✓", theme::success()),
                        (Some((_, true)), _) => ("✕", theme::error()),
                    };
                    let detail = match name.as_str() {
                        "edit_file" => Some(format!("- {}\n+ {}", input["old_text"].as_str().unwrap_or(""), input["new_text"].as_str().unwrap_or(""))),
                        "write_file" => Some(input["content"].as_str().unwrap_or("").chars().take(1500).collect()),
                        _ => None,
                    };
                    let result = output.as_ref().filter(|(_, err)| *err || name == "run_command").map(|(o, _)| o.chars().take(1500).collect::<String>());
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .p_2()
                        .rounded_md()
                        .border_1()
                        .border_color(theme::border())
                        .bg(theme::panel_bg())
                        .text_size(px(12.))
                        .child(
                            div()
                                .flex()
                                .gap_2()
                                .child(div().flex_1().text_color(theme::ui_text()).child(describe(name, input)))
                                .child(div().text_color(color).child(status)),
                        )
                        .when_some(detail.filter(|_| approval.is_some()), |d, detail| {
                            d.child(div().max_h(px(220.)).overflow_hidden().font_family(theme::editor_font()).text_color(theme::muted()).child(detail))
                        })
                        .when_some(result, |d, result| d.child(div().max_h(px(160.)).overflow_hidden().font_family(theme::editor_font()).text_color(theme::muted()).child(result)))
                        .when(approval.is_some(), |d| {
                            d.child(
                                div()
                                    .flex()
                                    .gap_2()
                                    .child(small_button(SharedString::from(format!("allow-{ix}")), "Allow").on_click(cx.listener(move |this, _, _, cx| this.answer_approval(ix, true, cx))))
                                    .child(small_button(SharedString::from(format!("deny-{ix}")), "Deny").on_click(cx.listener(move |this, _, _, cx| this.answer_approval(ix, false, cx)))),
                            )
                        })
                        .into_any_element()
                }
                ChatItem::Notice(text) => div().text_size(px(12.)).text_color(theme::muted()).child(SharedString::from(text.clone())).into_any_element(),
                ChatItem::Error(text) => div().text_size(px(12.)).text_color(theme::error()).child(SharedString::from(text.clone())).into_any_element(),
            }
        });
        let config = load_config();
        let header = div()
            .flex()
            .items_center()
            .gap_2()
            .px_3()
            .h(px(32.))
            .border_b_1()
            .border_color(theme::border())
            .bg(theme::panel_bg())
            .child(div().flex_1().text_color(theme::ui_text()).child("AI Assistant"))
            .child(small_button("agent-new".into(), "New").on_click(cx.listener(|this, _, window, cx| this.new_agent_chat(&NewAgentChat, window, cx))));
        // Anthropic plus the ACP agents installed on this machine.
        let providers = std::iter::once(("anthropic", "Claude API"))
            .chain(KNOWN_AGENTS.iter().filter(|a| agent::acp::which(a.program).is_some()).map(|a| (a.name, a.name)))
            .map(|(id, label)| {
                let active = config.provider == id;
                div()
                    .id(SharedString::from(format!("agent-provider-{id}")))
                    .px_2()
                    .h(px(22.))
                    .flex()
                    .items_center()
                    .rounded_md()
                    .cursor_pointer()
                    .text_size(px(12.))
                    .when(active, |d| d.bg(theme::selection()).text_color(theme::ui_text()))
                    .when(!active, |d| d.text_color(theme::muted()).hover(|d| d.bg(theme::hover())))
                    .child(label)
                    .on_click(cx.listener(move |this, _, _, cx| this.set_provider(id, cx)))
            });
        let provider_bar = div().flex().flex_wrap().gap_1().px_2().py_1().border_b_1().border_color(theme::border()).children(providers);
        let key_setup = self.agent.key_field.clone().map(|field| {
            div()
                .flex()
                .flex_col()
                .gap_2()
                .p_3()
                .m_3()
                .rounded_md()
                .border_1()
                .border_color(theme::border_strong())
                .child(div().text_color(theme::ui_text()).child("Anthropic API key"))
                .child(div().text_size(px(12.)).text_color(theme::muted()).child("Stored in your PhpCraft config folder (readable only by you). ANTHROPIC_API_KEY also works."))
                .child(field)
                .child(div().flex().child(small_button("agent-key-save".into(), "Save").on_click(cx.listener(|this, _, _, cx| this.save_agent_key(cx)))))
        });
        let empty = self.agent.items.is_empty();
        let running = self.agent.running;
        Some(
            div()
                .w(px(400.))
                .flex_none()
                .flex()
                .flex_col()
                .border_l_1()
                .border_color(theme::border_strong())
                .bg(theme::editor_bg())
                .text_size(px(13.))
                .child(header)
                .child(provider_bar)
                .children(key_setup)
                .child(
                    div()
                        .id("agent-chat")
                        .flex_1()
                        .min_h(px(0.))
                        .overflow_y_scroll()
                        .p_3()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .when(empty, |d| {
                            d.child(div().text_color(theme::muted()).child(
                                "Ask about the project, or ask for a change: “add a slug column to posts”, “why does this test fail?”. The open file and selection are sent as context.",
                            ))
                        })
                        .children(items)
                        .when(running, |d| d.child(div().text_size(px(12.)).text_color(theme::muted()).child("Working…"))),
                )
                .child(
                    div()
                        .key_context("AgentInput")
                        .on_action(cx.listener(Self::send_agent_message))
                        .flex()
                        .flex_col()
                        .gap_2()
                        .p_2()
                        .border_t_1()
                        .border_color(theme::border())
                        .child(div().h(px(90.)).rounded_md().border_1().border_color(theme::border_strong()).overflow_hidden().children(self.agent.input.clone()))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(div().flex_1().text_size(px(11.)).text_color(theme::muted()).child(crate::keymap::display("secondary-enter") + " to send"))
                                .when(running, |d| d.child(small_button("agent-stop".into(), "Stop").on_click(cx.listener(|this, _, _, cx| this.stop_agent(cx)))))
                                .when(!running, |d| {
                                    d.child(small_button("agent-send".into(), "Send").on_click(cx.listener(|this, _, window, cx| this.send_agent_message(&SendAgentMessage, window, cx))))
                                }),
                        ),
                ),
        )
    }
}
