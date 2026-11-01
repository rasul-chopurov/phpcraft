//! The Xdebug session controller: a background thread listens for PHP (port 9003 by default),
//! drives the DBGp session and reports pauses; the UI sends it commands. Paths are mapped
//! between the server (Docker, Sail, a VM) and the project with `.phpcraft/debug.json`:
//! `{ "port": 9003, "mappings": { "/var/www/html": "." } }`.

use std::collections::BTreeMap;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use dbgp::{Frame, Property, Session};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DebugConfig {
    pub port: u16,
    /// Server path → project path (relative to the project root, or absolute).
    pub mappings: BTreeMap<String, String>,
}

impl DebugConfig {
    pub fn load(root: &Path) -> Self {
        let mut config: Self = std::fs::read_to_string(root.join(".phpcraft/debug.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        if config.port == 0 {
            config.port = 9003;
        }
        config
    }
}

/// Maps file paths between the project and the debugged server.
#[derive(Debug, Clone)]
pub struct PathMap {
    /// (server prefix, local prefix), longest server prefix first.
    pairs: Vec<(String, PathBuf)>,
}

impl PathMap {
    pub fn new(root: &Path, config: &DebugConfig) -> Self {
        let mut pairs: Vec<(String, PathBuf)> = config
            .mappings
            .iter()
            .map(|(remote, local)| {
                let local = Path::new(local);
                (remote.trim_end_matches('/').to_string(), if local.is_absolute() { local.to_path_buf() } else { root.join(local) })
            })
            .collect();
        pairs.sort_by_key(|(r, _)| std::cmp::Reverse(r.len()));
        Self { pairs }
    }

    /// A server `file://` URI as a project path.
    pub fn to_local(&self, uri: &str) -> PathBuf {
        let remote = dbgp::uri_to_path(uri);
        let text = remote.to_string_lossy().replace('\\', "/");
        for (prefix, local) in &self.pairs {
            if let Some(rest) = text.strip_prefix(prefix.as_str()) {
                return local.join(rest.trim_start_matches('/'));
            }
        }
        remote
    }

    /// A project path as the server's `file://` URI.
    pub fn to_remote(&self, path: &Path) -> String {
        for (prefix, local) in &self.pairs {
            if let Ok(rest) = path.strip_prefix(local) {
                return dbgp::path_to_uri(&Path::new(prefix).join(rest));
            }
        }
        dbgp::path_to_uri(path)
    }
}

#[derive(Debug, Clone)]
pub enum Command {
    Resume,
    StepOver,
    StepInto,
    StepOut,
    /// Ends the PHP process.
    Stop,
    /// Stops listening (and ends any session).
    Close,
    SelectFrame(usize),
    Expand { fullname: String, frame: usize },
    Evaluate(String),
    Breakpoints(Vec<crate::breakpoints::Breakpoint>),
}

#[derive(Debug, Clone)]
pub enum Event {
    Listening(u16),
    Connected(PathBuf),
    Paused { file: PathBuf, line: usize, frames: Vec<(Frame, PathBuf)>, variables: Vec<Property> },
    Variables { frame: usize, variables: Vec<Property> },
    Expanded(Property),
    Evaluated { expression: String, result: Result<Property, String> },
    /// The PHP process finished; listening goes on.
    Finished,
    Error(String),
    Closed,
}

pub struct Controller {
    commands: mpsc::Sender<Command>,
    pub events: async_channel::Receiver<Event>,
}

impl Controller {
    pub fn start(root: &Path, breakpoints: Vec<crate::breakpoints::Breakpoint>) -> Self {
        let (commands, receiver) = mpsc::channel();
        let (events, events_rx) = async_channel::unbounded();
        let config = DebugConfig::load(root);
        let map = PathMap::new(root, &config);
        std::thread::spawn(move || listen(config.port, map, breakpoints, receiver, events));
        Self { commands, events: events_rx }
    }

    pub fn send(&self, command: Command) {
        let _ = self.commands.send(command);
    }
}

impl Drop for Controller {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Close);
    }
}

fn listen(port: u16, map: PathMap, mut breakpoints: Vec<crate::breakpoints::Breakpoint>, commands: mpsc::Receiver<Command>, events: async_channel::Sender<Event>) {
    let listener = match TcpListener::bind(("0.0.0.0", port)) {
        Ok(l) => l,
        Err(e) => {
            let _ = events.send_blocking(Event::Error(format!("Cannot listen on port {port}: {e}")));
            let _ = events.send_blocking(Event::Closed);
            return;
        }
    };
    let _ = listener.set_nonblocking(true);
    let _ = events.send_blocking(Event::Listening(port));
    loop {
        // Commands while waiting: breakpoint changes, or closing.
        while let Ok(command) = commands.try_recv() {
            match command {
                Command::Close => {
                    let _ = events.send_blocking(Event::Closed);
                    return;
                }
                Command::Breakpoints(list) => breakpoints = list,
                _ => {}
            }
        }
        match listener.accept() {
            Ok((stream, _)) => {
                let _ = stream.set_nonblocking(false);
                match Session::from_stream(stream) {
                    Ok(session) => {
                        let closed = run_session(session, &map, &mut breakpoints, &commands, &events);
                        let _ = events.send_blocking(Event::Finished);
                        if closed {
                            let _ = events.send_blocking(Event::Closed);
                            return;
                        }
                    }
                    Err(e) => {
                        let _ = events.send_blocking(Event::Error(e.to_string()));
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(100)),
            Err(e) => {
                let _ = events.send_blocking(Event::Error(e.to_string()));
                std::thread::sleep(Duration::from_millis(500));
            }
        }
    }
}

/// Drives one PHP process until it ends. Returns true when the UI closed the debugger.
fn run_session(mut session: Session, map: &PathMap, breakpoints: &mut Vec<crate::breakpoints::Breakpoint>, commands: &mpsc::Receiver<Command>, events: &async_channel::Sender<Event>) -> bool {
    let _ = events.send_blocking(Event::Connected(map.to_local(&session.init.file_uri)));
    let _ = session.feature_set("max_children", "100");
    let _ = session.feature_set("max_depth", "1");
    let _ = session.feature_set("show_hidden", "1");
    let mut ids = set_breakpoints(&mut session, map, breakpoints);
    let mut status = session.run();
    loop {
        let paused = match &status {
            Ok(s) if s.is_break() => s.clone(),
            Ok(_) => return false,
            Err(e) => {
                let _ = events.send_blocking(Event::Error(e.to_string()));
                return false;
            }
        };
        let frames: Vec<(Frame, PathBuf)> = session.stack().unwrap_or_default().into_iter().map(|f| {
            let local = map.to_local(&f.file_uri);
            (f, local)
        }).collect();
        let (file, line) = match (&paused.file_uri, paused.line) {
            (Some(uri), Some(line)) => (map.to_local(uri), line),
            _ => frames.first().map(|(f, p)| (p.clone(), f.line)).unwrap_or_default(),
        };
        let variables = session.variables(0, 0).unwrap_or_default();
        let _ = events.send_blocking(Event::Paused { file, line, frames, variables });
        // Wait for the UI.
        loop {
            let Ok(command) = commands.recv() else { return true };
            match command {
                Command::Resume => status = session.run(),
                Command::StepOver => status = session.step_over(),
                Command::StepInto => status = session.step_into(),
                Command::StepOut => status = session.step_out(),
                Command::Stop => {
                    let _ = session.stop();
                    return false;
                }
                Command::Close => {
                    let _ = session.detach();
                    return true;
                }
                Command::SelectFrame(frame) => {
                    let variables = session.variables(frame, 0).unwrap_or_default();
                    let _ = events.send_blocking(Event::Variables { frame, variables });
                    continue;
                }
                Command::Expand { fullname, frame } => {
                    if let Ok(property) = session.property(&fullname, frame) {
                        let _ = events.send_blocking(Event::Expanded(property));
                    }
                    continue;
                }
                Command::Evaluate(expression) => {
                    let result = session.eval(&expression).map_err(|e| e.to_string());
                    let _ = events.send_blocking(Event::Evaluated { expression, result });
                    continue;
                }
                Command::Breakpoints(list) => {
                    for id in ids.drain(..) {
                        let _ = session.remove_breakpoint(&id);
                    }
                    *breakpoints = list;
                    ids = set_breakpoints(&mut session, map, breakpoints);
                    continue;
                }
            }
            break;
        }
    }
}

fn set_breakpoints(session: &mut Session, map: &PathMap, breakpoints: &[crate::breakpoints::Breakpoint]) -> Vec<String> {
    breakpoints
        .iter()
        .filter_map(|b| session.set_breakpoint(&map.to_remote(&b.path), b.line + 1, b.condition.as_deref()).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    fn packet(xml: &str) -> Vec<u8> {
        format!("{}\0{xml}\0", xml.len()).into_bytes()
    }

    /// A fake Xdebug: stops once at `/srv/app/index.php:3`, then finishes.
    fn fake_xdebug(port: u16) {
        let mut stream = loop {
            match std::net::TcpStream::connect(("127.0.0.1", port)) {
                Ok(s) => break s,
                Err(_) => std::thread::sleep(Duration::from_millis(50)),
            }
        };
        stream.write_all(&packet(r#"<init fileuri="file:///srv/app/index.php" idekey="PHPCRAFT" language="PHP"/>"#)).unwrap();
        let mut runs = 0;
        loop {
            let mut buf = Vec::new();
            let mut byte = [0u8; 1];
            loop {
                if stream.read(&mut byte).unwrap_or(0) == 0 {
                    return;
                }
                if byte[0] == 0 {
                    break;
                }
                buf.push(byte[0]);
            }
            let line = String::from_utf8_lossy(&buf).to_string();
            let name = line.split_whitespace().next().unwrap().to_string();
            let id = line.split(" -i ").nth(1).unwrap().split_whitespace().next().unwrap().to_string();
            let body = match name.as_str() {
                "breakpoint_set" => {
                    assert!(line.contains("-f file:///srv/app/index.php -n 3"), "{line}");
                    format!(r#"<response transaction_id="{id}" id="1"/>"#)
                }
                "run" if runs == 0 => {
                    runs += 1;
                    format!(r#"<response transaction_id="{id}" status="break"><message filename="file:///srv/app/index.php" lineno="3"/></response>"#)
                }
                "run" => format!(r#"<response transaction_id="{id}" status="stopping"/>"#),
                "stack_get" => format!(r#"<response transaction_id="{id}"><stack level="0" where="{{main}}" filename="file:///srv/app/index.php" lineno="3"/></response>"#),
                "context_get" => format!(r#"<response transaction_id="{id}"><property name="$n" fullname="$n" type="int"><![CDATA[7]]></property></response>"#),
                _ => format!(r#"<response transaction_id="{id}"/>"#),
            };
            stream.write_all(&packet(&body)).unwrap();
        }
    }

    #[test]
    fn controller_runs_a_session() {
        let root = std::env::temp_dir().join(format!("phpcraft-debug-{}", std::process::id()));
        std::fs::create_dir_all(root.join(".phpcraft")).unwrap();
        let port = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap().local_addr().unwrap().port();
        std::fs::write(root.join(".phpcraft/debug.json"), format!(r#"{{"port": {port}, "mappings": {{"/srv/app": "."}}}}"#)).unwrap();
        let breakpoint = crate::breakpoints::Breakpoint { path: root.join("index.php"), line: 2, condition: None };
        let controller = Controller::start(&root, vec![breakpoint]);
        let engine = std::thread::spawn(move || fake_xdebug(port));
        let next = || controller.events.recv_blocking().unwrap();
        assert!(matches!(next(), Event::Listening(p) if p == port));
        assert!(matches!(next(), Event::Connected(f) if f == root.join("index.php")));
        match next() {
            Event::Paused { file, line, frames, variables } => {
                assert_eq!((file, line), (root.join("index.php"), 3));
                assert_eq!(frames[0].1, root.join("index.php"));
                assert_eq!(variables[0].display(), "7");
            }
            other => panic!("{other:?}"),
        }
        controller.send(Command::Resume);
        assert!(matches!(next(), Event::Finished));
        engine.join().unwrap();
        drop(controller);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn maps_paths_both_ways() {
        let root = Path::new("/Users/me/shop");
        let config = DebugConfig { port: 9003, mappings: BTreeMap::from([("/var/www/html".to_string(), ".".to_string())]) };
        let map = PathMap::new(root, &config);
        assert_eq!(map.to_local("file:///var/www/html/app/User.php"), Path::new("/Users/me/shop/app/User.php"));
        assert_eq!(map.to_remote(Path::new("/Users/me/shop/app/User.php")), "file:///var/www/html/app/User.php");
        // Outside every mapping: unchanged.
        assert_eq!(map.to_local("file:///tmp/x.php"), Path::new("/tmp/x.php"));
    }
}
