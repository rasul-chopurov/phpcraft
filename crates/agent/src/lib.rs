//! AI assistant: a Claude agent loop over the Messages API (raw HTTP + SSE streaming) with
//! project tools. UI-agnostic: the app sends prompts and receives [`AgentEvent`]s.
//!
//! Reading tools run on their own; tools that change files or run commands wait for the
//! user's approval through [`AgentEvent::Approve`].

pub mod acp;
pub mod complete;
mod sse;
mod tools;

use std::io::BufReader;
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub use tools::tool_definitions;

pub const API_URL: &str = "https://api.anthropic.com/v1/messages";
pub const DEFAULT_MODEL: &str = "claude-opus-5-5";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentConfig {
    /// Empty: read `ANTHROPIC_API_KEY` from the environment.
    pub api_key: String,
    pub model: String,
    /// `low` | `medium` | `high` | `xhigh` | `max`.
    pub effort: String,
    /// `"anthropic"` (API key) or the name of an ACP agent such as `"OpenCode"`.
    pub provider: String,
    /// Model for inline (ghost text) completions in the editor.
    pub completion_model: String,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self { api_key: String::new(), model: DEFAULT_MODEL.into(), effort: "high".into(), provider: "anthropic".into(), completion_model: DEFAULT_MODEL.into() }
    }
}

impl AgentConfig {
    pub fn resolved_key(&self) -> Option<String> {
        Some(self.api_key.trim().to_string())
            .filter(|k| !k.is_empty())
            .or_else(|| std::env::var("ANTHROPIC_API_KEY").ok().filter(|k| !k.is_empty()))
    }
}

#[derive(Debug)]
pub enum AgentEvent {
    Text(String),
    /// Summarized reasoning or progress notes.
    Thinking(String),
    ToolCall { id: String, name: String, input: Value },
    ToolResult { id: String, output: String, is_error: bool },
    /// A tool that changes something wants to run; answer on `reply`.
    Approve { id: String, name: String, input: Value, reply: mpsc::Sender<bool> },
    /// Files the agent changed (the IDE reloads them).
    FilesChanged(Vec<PathBuf>),
    Notice(String),
    Error(String),
    Done,
}

/// One conversation. Cheap to clone; turns run on a background thread.
#[derive(Clone)]
pub struct Session {
    root: PathBuf,
    config: AgentConfig,
    /// Messages exactly as the API expects them (assistant content blocks kept verbatim,
    /// including thinking blocks, so they can be replayed).
    history: Arc<Mutex<Vec<Value>>>,
    cancel: Arc<std::sync::atomic::AtomicBool>,
}

const SYSTEM: &str = "You are the AI assistant inside PhpCraft, a PHP and Laravel IDE. You work in the user's open project through the provided tools: read and search files before changing them, make focused edits with edit_file, and keep the project's existing style. Prefer Laravel conventions (artisan, Eloquent, Form Requests) where they fit. Paths are relative to the project root. When you run commands, explain briefly why. Answer in the language the user writes in. Use Markdown in replies.";

impl Session {
    pub fn new(root: PathBuf, config: AgentConfig) -> Self {
        Self { root, config, history: Default::default(), cancel: Default::default() }
    }

    pub fn reset(&self) {
        self.history.lock().unwrap().clear();
    }

    pub fn cancel(&self) {
        self.cancel.store(true, std::sync::atomic::Ordering::SeqCst);
    }

    /// Sends a user message and runs the agent until it finishes its turn.
    pub fn send(&self, text: String, events: mpsc::Sender<AgentEvent>) {
        let session = self.clone();
        session.cancel.store(false, std::sync::atomic::Ordering::SeqCst);
        std::thread::Builder::new()
            .name("agent".into())
            .spawn(move || {
                session.history.lock().unwrap().push(json!({"role": "user", "content": text}));
                if let Err(err) = session.run(&events) {
                    let _ = events.send(AgentEvent::Error(err));
                }
                let _ = events.send(AgentEvent::Done);
            })
            .ok();
    }

    fn cancelled(&self) -> bool {
        self.cancel.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn run(&self, events: &mpsc::Sender<AgentEvent>) -> Result<(), String> {
        let key = self.config.resolved_key().ok_or("No API key: set one in the assistant panel or ANTHROPIC_API_KEY")?;
        for _ in 0..50 {
            if self.cancelled() {
                return Ok(());
            }
            let body = json!({
                "model": self.config.model,
                "max_tokens": 64000,
                "stream": true,
                "system": SYSTEM,
                "thinking": {"type": "adaptive", "display": "summarized"},
                "output_config": {"effort": self.config.effort},
                "fallbacks": "default",
                "tools": tool_definitions(),
                "messages": *self.history.lock().unwrap(),
            });
            let response = ureq::post(API_URL)
                .header("x-api-key", &key)
                .header("anthropic-version", "2026-10-21")
                .header("anthropic-beta", "server-side-fallback-2026-07-01")
                .header("content-type", "application/json")
                .config()
                .http_status_as_error(false)
                .build()
                .send(body.to_string())
                .map_err(|e| format!("Network error: {e}"))?;
            let status = response.status().as_u16();
            if status != 200 {
                let text = response.into_body().read_to_string().unwrap_or_default();
                let message = serde_json::from_str::<Value>(&text)
                    .ok()
                    .and_then(|v| v["error"]["message"].as_str().map(str::to_string))
                    .unwrap_or(text);
                return Err(format!("API error {status}: {message}"));
            }
            let reader = BufReader::new(response.into_body().into_reader());
            let message = sse::read_message(reader, events, &|| self.cancelled())?;
            let stop = message.stop_reason.clone().unwrap_or_default();
            self.history.lock().unwrap().push(json!({"role": "assistant", "content": message.content}));
            match stop.as_str() {
                "tool_use" => {
                    let results = self.run_tools(&message.content, events);
                    self.history.lock().unwrap().push(json!({"role": "user", "content": results}));
                }
                // The server paused a long turn; continue it.
                "pause_turn" => {}
                "max_tokens" => {
                    let _ = events.send(AgentEvent::Notice("The reply hit the output limit.".into()));
                    return Ok(());
                }
                "refusal" => {
                    let _ = events.send(AgentEvent::Notice("Claude declined this request.".into()));
                    return Ok(());
                }
                _ => return Ok(()),
            }
        }
        Err("Stopped after 50 steps.".into())
    }

    /// Runs every `tool_use` block and returns all results for one user message.
    fn run_tools(&self, content: &[Value], events: &mpsc::Sender<AgentEvent>) -> Vec<Value> {
        let mut results = Vec::new();
        for block in content.iter().filter(|b| b["type"] == "tool_use") {
            let id = block["id"].as_str().unwrap_or_default().to_string();
            let name = block["name"].as_str().unwrap_or_default().to_string();
            let input = block["input"].clone();
            let _ = events.send(AgentEvent::ToolCall { id: id.clone(), name: name.clone(), input: input.clone() });
            let (output, is_error) = if self.cancelled() {
                ("Cancelled by the user.".to_string(), true)
            } else if !input.is_object() {
                // Streamed input that didn't parse (cut off or invalid JSON).
                ("INVALID_JSON: the tool input could not be parsed; send the call again.".to_string(), true)
            } else if tools::needs_approval(&name) {
                let (tx, rx) = mpsc::channel();
                let _ = events.send(AgentEvent::Approve { id: id.clone(), name: name.clone(), input: input.clone(), reply: tx });
                if rx.recv().unwrap_or(false) {
                    tools::run(&self.root, &name, &input, events)
                } else {
                    ("The user declined this action.".to_string(), true)
                }
            } else {
                tools::run(&self.root, &name, &input, events)
            };
            let _ = events.send(AgentEvent::ToolResult { id: id.clone(), output: output.clone(), is_error });
            results.push(json!({"type": "tool_result", "tool_use_id": id, "content": output, "is_error": is_error}));
        }
        results
    }
}
