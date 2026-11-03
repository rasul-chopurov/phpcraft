//! Agent Client Protocol client: runs an external agent (`opencode acp`, Claude Code,
//! Codex…) as a child process speaking JSON-RPC 2.0 over stdio (one message per line),
//! and turns its session updates into [`AgentEvent`]s.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};

use serde_json::{Value, json};

use crate::AgentEvent;

/// An agent the IDE can start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcpAgent {
    pub name: &'static str,
    pub program: &'static str,
    pub args: &'static [&'static str],
    /// Command that signs in to the agent's own account (run in a terminal).
    pub login: &'static [&'static str],
    /// Where the agent keeps its sign-in, relative to the home folder (empty when it uses the
    /// system keychain and there's nothing to look at).
    pub credentials: &'static [&'static str],
}

impl AcpAgent {
    /// Whether one of the agent's credential files has something in it (OpenCode leaves `{}`
    /// behind after logging out).
    pub fn signed_in(&self) -> bool {
        let Some(home) = dirs_home() else { return false };
        let data = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".local/share"));
        self.credentials.iter().any(|file| {
            let path = match file.strip_prefix("$data/") {
                Some(rest) => data.join(rest),
                None => home.join(file),
            };
            std::fs::read_to_string(path).is_ok_and(|text| text.trim().len() > 2)
        })
    }
}

pub const KNOWN_AGENTS: &[AcpAgent] = &[
    AcpAgent { name: "OpenCode", program: "opencode", args: &["acp"], login: &["opencode", "auth", "login"], credentials: &["$data/opencode/auth.json"] },
    AcpAgent { name: "Claude Code", program: "claude-code-acp", args: &[], login: &["claude", "/login"], credentials: &[".claude/.credentials.json"] },
    AcpAgent { name: "Codex", program: "codex-acp", args: &[], login: &["codex", "login"], credentials: &[".codex/auth.json"] },
    // Google AI subscription (Gemini CLI signed in with a Google account).
    AcpAgent { name: "Gemini", program: "gemini", args: &["--experimental-acp"], login: &["gemini"], credentials: &[".gemini/oauth_creds.json"] },
];

/// Finds `program` on PATH (and common install folders GUI apps don't inherit).
pub fn which(program: &str) -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    if let Some(home) = dirs_home() {
        for extra in [".opencode/bin", ".local/bin", ".bun/bin", ".npm-global/bin", ".cargo/bin"] {
            dirs.push(home.join(extra));
        }
    }
    dirs.extend(["/opt/homebrew/bin", "/usr/local/bin"].map(PathBuf::from));
    let names: Vec<String> = if cfg!(windows) {
        vec![format!("{program}.exe"), format!("{program}.cmd"), program.to_string()]
    } else {
        vec![program.to_string()]
    };
    dirs.iter().flat_map(|d| names.iter().map(move |n| d.join(n))).find(|p| p.is_file())
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from)
}

type Pending = Arc<Mutex<HashMap<u64, mpsc::Sender<Result<Value, String>>>>>;

struct Connection {
    stdin: Mutex<ChildStdin>,
    next_id: AtomicU64,
    pending: Pending,
    child: Mutex<Child>,
}

impl Connection {
    fn write(&self, message: &Value) -> Result<(), String> {
        let mut stdin = self.stdin.lock().unwrap();
        writeln!(stdin, "{message}").and_then(|_| stdin.flush()).map_err(|e| format!("agent pipe: {e}"))
    }

    /// Sends a request and waits for its response.
    fn request(&self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = mpsc::channel();
        self.pending.lock().unwrap().insert(id, tx);
        self.write(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))?;
        rx.recv().map_err(|_| "the agent exited".to_string())?
    }

    fn notify(&self, method: &str, params: Value) {
        let _ = self.write(&json!({"jsonrpc": "2.0", "method": method, "params": params}));
    }

    fn respond(&self, id: &Value, result: Result<Value, String>) {
        let message = match result {
            Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
            Err(message) => json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32603, "message": message}}),
        };
        let _ = self.write(&message);
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        let _ = self.child.lock().unwrap().kill();
    }
}

/// A running ACP agent with one session in the project.
#[derive(Clone)]
pub struct AcpSession {
    connection: Arc<Connection>,
    session_id: String,
    /// Where session updates go during the current prompt.
    events: Arc<Mutex<Option<mpsc::Sender<AgentEvent>>>>,
}

fn resolve_inside(root: &Path, path: &str) -> Result<PathBuf, String> {
    let path = Path::new(path);
    let full = if path.is_absolute() { path.to_path_buf() } else { root.join(path) };
    if full.components().any(|c| matches!(c, std::path::Component::ParentDir)) || !full.starts_with(root) {
        return Err(format!("{}: outside the project", full.display()));
    }
    Ok(full)
}

impl AcpSession {
    /// Starts the agent, initializes the protocol and opens a session in `root`.
    pub fn start(agent: &AcpAgent, root: &Path) -> Result<Self, String> {
        let program = which(agent.program).ok_or_else(|| format!("{} is not installed ({} not found on PATH)", agent.name, agent.program))?;
        let mut child = process::command(&program)
            .args(agent.args)
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("cannot start {}: {e}", program.display()))?;
        let stdin = child.stdin.take().ok_or("no stdin")?;
        let stdout = child.stdout.take().ok_or("no stdout")?;
        let pending: Pending = Default::default();
        let connection = Arc::new(Connection { stdin: Mutex::new(stdin), next_id: AtomicU64::new(1), pending: pending.clone(), child: Mutex::new(child) });
        let events: Arc<Mutex<Option<mpsc::Sender<AgentEvent>>>> = Default::default();
        Self::spawn_reader(stdout, connection.clone(), pending, events.clone(), root.to_path_buf());

        connection.request(
            "initialize",
            json!({
                "protocolVersion": 1,
                "clientCapabilities": {"fs": {"readTextFile": true, "writeTextFile": true}, "terminal": false},
                "clientInfo": {"name": "PhpCraft", "version": env!("CARGO_PKG_VERSION")}
            }),
        )?;
        let session = connection.request("session/new", json!({"cwd": root, "mcpServers": []}))?;
        let session_id = session["sessionId"].as_str().ok_or("the agent returned no session id")?.to_string();
        Ok(Self { connection, session_id, events })
    }

    fn spawn_reader(stdout: std::process::ChildStdout, connection: Arc<Connection>, pending: Pending, events: Arc<Mutex<Option<mpsc::Sender<AgentEvent>>>>, root: PathBuf) {
        let weak = Arc::downgrade(&connection);
        drop(connection);
        std::thread::Builder::new()
            .name("acp-reader".into())
            .spawn(move || {
                for line in BufReader::new(stdout).lines() {
                    let Ok(line) = line else { break };
                    let Ok(message) = serde_json::from_str::<Value>(&line) else { continue };
                    let Some(connection) = weak.upgrade() else { break };
                    let send = |event: AgentEvent| {
                        if let Some(tx) = events.lock().unwrap().as_ref() {
                            let _ = tx.send(event);
                        }
                    };
                    match (message.get("id"), message.get("method").and_then(Value::as_str)) {
                        // Response to one of our requests.
                        (Some(id), None) => {
                            if let Some(tx) = id.as_u64().and_then(|id| pending.lock().unwrap().remove(&id)) {
                                let result = match message.get("error") {
                                    Some(error) => Err(error["message"].as_str().unwrap_or("agent error").to_string()),
                                    None => Ok(message.get("result").cloned().unwrap_or(Value::Null)),
                                };
                                let _ = tx.send(result);
                            }
                        }
                        // Request from the agent.
                        (Some(id), Some(method)) => {
                            let params = &message["params"];
                            let id = id.clone();
                            match method {
                                "fs/read_text_file" => {
                                    let result = resolve_inside(&root, params["path"].as_str().unwrap_or("")).and_then(|path| {
                                        let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
                                        let start = params["line"].as_u64().map(|l| l.saturating_sub(1) as usize).unwrap_or(0);
                                        let limit = params["limit"].as_u64().map(|l| l as usize).unwrap_or(usize::MAX);
                                        let content = if start == 0 && limit == usize::MAX {
                                            text
                                        } else {
                                            text.lines().skip(start).take(limit).collect::<Vec<_>>().join("\n")
                                        };
                                        Ok(json!({"content": content}))
                                    });
                                    connection.respond(&id, result);
                                }
                                "fs/write_text_file" => {
                                    let result = resolve_inside(&root, params["path"].as_str().unwrap_or("")).and_then(|path| {
                                        if let Some(parent) = path.parent() {
                                            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                                        }
                                        std::fs::write(&path, params["content"].as_str().unwrap_or("")).map_err(|e| e.to_string())?;
                                        send(AgentEvent::FilesChanged(vec![path]));
                                        Ok(Value::Null)
                                    });
                                    connection.respond(&id, result);
                                }
                                "session/request_permission" => {
                                    // Ask the user on another thread so responses keep flowing.
                                    let options = params["options"].as_array().cloned().unwrap_or_default();
                                    let tool = params["toolCall"].clone();
                                    let tool_id = tool["toolCallId"].as_str().unwrap_or("").to_string();
                                    let (reply, answer) = mpsc::channel();
                                    send(AgentEvent::Approve { id: tool_id, name: tool["title"].as_str().unwrap_or("Tool").to_string(), input: tool, reply });
                                    let connection = connection.clone();
                                    std::thread::spawn(move || {
                                        let allow = answer.recv().unwrap_or(false);
                                        let wanted = if allow { ["allow_once", "allow_always"] } else { ["reject_once", "reject_always"] };
                                        let option = options.iter().find(|o| wanted.contains(&o["kind"].as_str().unwrap_or(""))).and_then(|o| o["optionId"].as_str());
                                        let outcome = match option {
                                            Some(option) => json!({"outcome": {"outcome": "selected", "optionId": option}}),
                                            None => json!({"outcome": {"outcome": "cancelled"}}),
                                        };
                                        connection.respond(&id, Ok(outcome));
                                    });
                                }
                                _ => connection.respond(&id, Err(format!("{method} is not supported"))),
                            }
                        }
                        // Notification.
                        (None, Some("session/update")) => {
                            for event in update_events(&message["params"]["update"]) {
                                send(event);
                            }
                        }
                        _ => {}
                    }
                }
                // The agent exited: fail every waiting request.
                for (_, tx) in pending.lock().unwrap().drain() {
                    let _ = tx.send(Err("the agent exited".into()));
                }
            })
            .ok();
    }

    /// Sends a prompt; events arrive on `events` until [`AgentEvent::Done`].
    pub fn send(&self, text: String, events: mpsc::Sender<AgentEvent>) {
        let session = self.clone();
        *self.events.lock().unwrap() = Some(events.clone());
        std::thread::spawn(move || {
            let result = session.connection.request("session/prompt", json!({"sessionId": session.session_id, "prompt": [{"type": "text", "text": text}]}));
            match result {
                Ok(result) => match result["stopReason"].as_str() {
                    Some("max_tokens") => drop(events.send(AgentEvent::Notice("The reply hit the output limit.".into()))),
                    Some("refusal") => drop(events.send(AgentEvent::Notice("The agent declined this request.".into()))),
                    _ => {}
                },
                Err(err) => drop(events.send(AgentEvent::Error(err))),
            }
            let _ = events.send(AgentEvent::Done);
        });
    }

    pub fn cancel(&self) {
        self.connection.notify("session/cancel", json!({"sessionId": self.session_id}));
    }
}

/// Session update -> chat events.
pub fn update_events(update: &Value) -> Vec<AgentEvent> {
    let text = |v: &Value| v["content"]["text"].as_str().unwrap_or("").to_string();
    let id = || update["toolCallId"].as_str().unwrap_or("").to_string();
    match update["sessionUpdate"].as_str().unwrap_or("") {
        "agent_message_chunk" => vec![AgentEvent::Text(text(update))],
        "agent_thought_chunk" => vec![AgentEvent::Thinking(text(update))],
        "tool_call" => vec![AgentEvent::ToolCall { id: id(), name: update["title"].as_str().unwrap_or("Tool").to_string(), input: update.clone() }],
        "tool_call_update" => match update["status"].as_str() {
            Some(status @ ("completed" | "failed")) => {
                let output = update["content"]
                    .as_array()
                    .map(|items| items.iter().filter_map(|c| c["content"]["text"].as_str()).collect::<Vec<_>>().join("\n"))
                    .unwrap_or_default();
                vec![AgentEvent::ToolResult { id: id(), output, is_error: status == "failed" }]
            }
            _ => Vec::new(),
        },
        "plan" => {
            let steps = update["entries"]
                .as_array()
                .map(|e| e.iter().map(|s| format!("{} {}", if s["status"] == "completed" { "✓" } else { "•" }, s["content"].as_str().unwrap_or(""))).collect::<Vec<_>>().join("\n"))
                .unwrap_or_default();
            vec![AgentEvent::Notice(format!("Plan:\n{steps}"))]
        }
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_updates() {
        let events = update_events(&json!({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "Hi"}}));
        assert!(matches!(&events[..], [AgentEvent::Text(t)] if t == "Hi"));
        let events = update_events(&json!({"sessionUpdate": "tool_call_update", "toolCallId": "c1", "status": "completed", "content": [{"type": "content", "content": {"type": "text", "text": "ok"}}]}));
        assert!(matches!(&events[..], [AgentEvent::ToolResult { id, output, is_error: false }] if id == "c1" && output == "ok"));
        assert!(update_events(&json!({"sessionUpdate": "available_commands_update"})).is_empty());
    }

    #[test]
    fn paths_stay_inside() {
        let root = Path::new("/project");
        assert!(resolve_inside(root, "/project/app/a.php").is_ok());
        assert!(resolve_inside(root, "app/a.php").is_ok());
        assert!(resolve_inside(root, "/etc/passwd").is_err());
        assert!(resolve_inside(root, "/project/../etc").is_err());
    }
}

#[cfg(test)]
mod live {
    /// `cargo test -p agent -- --ignored opencode_handshake`: needs opencode installed.
    #[test]
    #[ignore]
    fn opencode_handshake() {
        let root = std::env::current_dir().unwrap();
        let session = super::AcpSession::start(&super::KNOWN_AGENTS[0], &root).expect("handshake");
        assert!(!session.session_id.is_empty());
    }
}
