//! The DBGp protocol that Xdebug speaks: PhpCraft listens (port 9003), PHP connects when a
//! script or request starts with debugging on, and the session sets breakpoints, steps and
//! reads the stack and variables.
//!
//! Engine → IDE packets are `<length>\0<xml>\0`; IDE → engine commands are
//! `name -i <transaction> [args] [-- base64(data)]\0`.

pub mod xml;

use std::io::{self, BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
pub use xml::Element;

/// What the engine said when it connected.
#[derive(Debug, Clone, PartialEq)]
pub struct Init {
    /// The script that started, as a `file://` URI.
    pub file_uri: String,
    pub idekey: String,
    pub language: String,
}

/// Where execution stands after a continuation command.
#[derive(Debug, Clone, PartialEq)]
pub struct Status {
    /// `break`, `stopping`, `stopped`, `running`…
    pub status: String,
    /// The breakpoint or step location, when stopped at one.
    pub file_uri: Option<String>,
    pub line: Option<usize>,
}

impl Status {
    pub fn is_break(&self) -> bool {
        self.status == "break"
    }

    pub fn is_finished(&self) -> bool {
        matches!(self.status.as_str(), "stopping" | "stopped")
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    pub level: usize,
    /// Function or method name (`{main}` at the top).
    pub function: String,
    pub file_uri: String,
    pub line: usize,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Property {
    pub name: String,
    /// The expression to fetch it again (`$user->address`).
    pub fullname: String,
    /// `int`, `string`, `array`, `object`, `null`…
    pub kind: String,
    pub classname: Option<String>,
    /// The value, for scalars.
    pub value: Option<String>,
    pub has_children: bool,
    pub num_children: usize,
    pub children: Vec<Property>,
}

impl Property {
    fn from_xml(e: &Element) -> Self {
        let text = e.text.trim();
        let value = match e.attr("encoding") {
            Some("base64") => BASE64.decode(text).ok().map(|b| String::from_utf8_lossy(&b).into_owned()),
            _ if text.is_empty() => None,
            _ => Some(text.to_string()),
        };
        Property {
            name: e.attr("name").unwrap_or("").to_string(),
            fullname: e.attr("fullname").unwrap_or("").to_string(),
            kind: e.attr("type").unwrap_or("").to_string(),
            classname: e.attr("classname").map(str::to_string),
            value,
            has_children: e.attr("children") == Some("1"),
            num_children: e.attr("numchildren").and_then(|n| n.parse().ok()).unwrap_or(0),
            children: e.children_named("property").map(Property::from_xml).collect(),
        }
    }

    /// The value as shown in the Variables tree: `"text"`, `42`, `array(3)`, `App\User`.
    pub fn display(&self) -> String {
        match self.kind.as_str() {
            "string" => format!("\"{}\"", self.value.as_deref().unwrap_or("")),
            "array" => format!("array({})", self.num_children),
            "object" => self.classname.clone().unwrap_or_else(|| "object".into()),
            "null" | "uninitialized" => "null".into(),
            _ => self.value.clone().unwrap_or_default(),
        }
    }
}

#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    /// The engine answered a command with an `<error>`.
    Engine { code: String, message: String },
    Protocol(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Io(e) => write!(f, "{e}"),
            Error::Engine { code, message } => write!(f, "error {code}: {message}"),
            Error::Protocol(m) => write!(f, "{m}"),
        }
    }
}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error::Io(e)
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// Reads one `<length>\0<xml>\0` packet.
fn read_packet(reader: &mut impl BufRead) -> Result<Element> {
    let mut len = Vec::new();
    if reader.read_until(0, &mut len)? == 0 {
        return Err(Error::Io(io::Error::new(io::ErrorKind::UnexpectedEof, "debugger disconnected")));
    }
    len.pop();
    let len: usize = std::str::from_utf8(&len).ok().and_then(|s| s.trim().parse().ok()).ok_or_else(|| Error::Protocol("bad packet length".into()))?;
    let mut body = vec![0; len + 1];
    reader.read_exact(&mut body)?;
    body.pop();
    let text = String::from_utf8_lossy(&body);
    xml::parse(&text).ok_or_else(|| Error::Protocol("bad packet".into()))
}

/// A connected debugging session.
pub struct Session {
    writer: TcpStream,
    reader: BufReader<TcpStream>,
    next_id: u32,
    pub init: Init,
}

impl Session {
    /// Waits for the next PHP process on `listener` and reads its `init` packet.
    pub fn accept(listener: &TcpListener) -> Result<Session> {
        let (stream, _) = listener.accept()?;
        Self::from_stream(stream)
    }

    pub fn from_stream(stream: TcpStream) -> Result<Session> {
        stream.set_nodelay(true).ok();
        let mut reader = BufReader::new(stream.try_clone()?);
        let init = read_packet(&mut reader)?;
        let init = Init {
            file_uri: init.attr("fileuri").unwrap_or("").to_string(),
            idekey: init.attr("idekey").unwrap_or("").to_string(),
            language: init.attr("language").unwrap_or("PHP").to_string(),
        };
        Ok(Session { writer: stream, reader, next_id: 1, init })
    }

    /// Sends a command and waits for its response (skipping stream/notify packets).
    pub fn command(&mut self, name: &str, args: &[(&str, &str)], data: Option<&str>) -> Result<Element> {
        let id = self.next_id;
        self.next_id += 1;
        let mut line = format!("{name} -i {id}");
        for (flag, value) in args {
            let value = if value.contains([' ', '"']) { format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\"")) } else { value.to_string() };
            line.push_str(&format!(" -{flag} {value}"));
        }
        if let Some(data) = data {
            line.push_str(&format!(" -- {}", BASE64.encode(data)));
        }
        line.push('\0');
        self.writer.write_all(line.as_bytes())?;
        loop {
            let packet = read_packet(&mut self.reader)?;
            if packet.name != "response" || packet.attr("transaction_id") != Some(&id.to_string()) {
                continue;
            }
            if let Some(error) = packet.child("error") {
                return Err(Error::Engine {
                    code: error.attr("code").unwrap_or("").to_string(),
                    message: error.child("message").map(|m| m.text.clone()).unwrap_or_default(),
                });
            }
            return Ok(packet);
        }
    }

    pub fn feature_set(&mut self, name: &str, value: &str) -> Result<()> {
        self.command("feature_set", &[("n", name), ("v", value)], None).map(|_| ())
    }

    /// A line breakpoint, optionally only when `condition` (PHP) is true; returns its id.
    pub fn set_breakpoint(&mut self, file_uri: &str, line: usize, condition: Option<&str>) -> Result<String> {
        let line = line.to_string();
        let response = self.command("breakpoint_set", &[("t", if condition.is_some() { "conditional" } else { "line" }), ("f", file_uri), ("n", &line)], condition)?;
        Ok(response.attr("id").unwrap_or("").to_string())
    }

    pub fn remove_breakpoint(&mut self, id: &str) -> Result<()> {
        self.command("breakpoint_remove", &[("d", id)], None).map(|_| ())
    }

    fn continuation(&mut self, name: &str) -> Result<Status> {
        let response = self.command(name, &[], None)?;
        let message = response.child("message");
        Ok(Status {
            status: response.attr("status").unwrap_or("").to_string(),
            file_uri: message.and_then(|m| m.attr("filename")).map(str::to_string),
            line: message.and_then(|m| m.attr("lineno")).and_then(|l| l.parse().ok()),
        })
    }

    pub fn run(&mut self) -> Result<Status> {
        self.continuation("run")
    }

    pub fn step_into(&mut self) -> Result<Status> {
        self.continuation("step_into")
    }

    pub fn step_over(&mut self) -> Result<Status> {
        self.continuation("step_over")
    }

    pub fn step_out(&mut self) -> Result<Status> {
        self.continuation("step_out")
    }

    pub fn stop(&mut self) -> Result<()> {
        self.command("stop", &[], None).map(|_| ())
    }

    pub fn detach(&mut self) -> Result<()> {
        self.command("detach", &[], None).map(|_| ())
    }

    pub fn stack(&mut self) -> Result<Vec<Frame>> {
        let response = self.command("stack_get", &[], None)?;
        Ok(response
            .children_named("stack")
            .map(|s| Frame {
                level: s.attr("level").and_then(|l| l.parse().ok()).unwrap_or(0),
                function: s.attr("where").unwrap_or("").to_string(),
                file_uri: s.attr("filename").unwrap_or("").to_string(),
                line: s.attr("lineno").and_then(|l| l.parse().ok()).unwrap_or(0),
            })
            .collect())
    }

    /// Variables of a stack frame; context 0 is locals, 1 superglobals, 2 constants.
    pub fn variables(&mut self, depth: usize, context: usize) -> Result<Vec<Property>> {
        let (d, c) = (depth.to_string(), context.to_string());
        let response = self.command("context_get", &[("d", &d), ("c", &c)], None)?;
        Ok(response.children_named("property").map(Property::from_xml).collect())
    }

    /// One variable with its children (for expanding arrays and objects).
    pub fn property(&mut self, fullname: &str, depth: usize) -> Result<Property> {
        let d = depth.to_string();
        let response = self.command("property_get", &[("n", fullname), ("d", &d)], None)?;
        response.child("property").map(Property::from_xml).ok_or_else(|| Error::Protocol("no property".into()))
    }

    /// Evaluates PHP in the current frame (Evaluate Expression, watches).
    pub fn eval(&mut self, expression: &str) -> Result<Property> {
        let response = self.command("eval", &[], Some(expression))?;
        Ok(response.child("property").map(Property::from_xml).unwrap_or_default())
    }
}

/// `file:///var/www/a%20b.php` → `/var/www/a b.php` (`file:///C:/x` → `C:/x` on Windows).
pub fn uri_to_path(uri: &str) -> PathBuf {
    let path = uri.strip_prefix("file://").unwrap_or(uri);
    let mut decoded = Vec::new();
    let bytes = path.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len()
            && let Ok(b) = u8::from_str_radix(&path[i + 1..i + 3], 16)
        {
            decoded.push(b);
            i += 3;
            continue;
        }
        decoded.push(bytes[i]);
        i += 1;
    }
    let path = String::from_utf8_lossy(&decoded).into_owned();
    // `/C:/x` on Windows.
    let path = if path.len() > 2 && path.as_bytes()[0] == b'/' && path.as_bytes()[2] == b':' { path[1..].to_string() } else { path };
    PathBuf::from(path)
}

/// The `file://` URI Xdebug uses for `path`.
pub fn path_to_uri(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    let text = text.strip_prefix("//?/").unwrap_or(&text);
    let encoded: String = text
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || "/-_.~:".contains(c) { c.to_string() } else { c.to_string().bytes().map(|b| format!("%{b:02X}")).collect() })
        .collect();
    if encoded.starts_with('/') { format!("file://{encoded}") } else { format!("file:///{encoded}") }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read as _;

    fn packet(xml: &str) -> Vec<u8> {
        let mut out = format!("{}\0{xml}\0", xml.len()).into_bytes();
        out.shrink_to_fit();
        out
    }

    /// A fake engine: answers each command from a script keyed by the command name.
    fn engine(port: u16) {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream.write_all(&packet(r#"<init xmlns="urn:debugger_protocol_v1" fileuri="file:///app/index.php" language="PHP" idekey="PHPCRAFT"></init>"#)).unwrap();
        let mut buf = Vec::new();
        let mut byte = [0u8; 1];
        loop {
            buf.clear();
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
            let mut parts = line.split_whitespace();
            let name = parts.next().unwrap().to_string();
            let id = line.split(" -i ").nth(1).unwrap().split_whitespace().next().unwrap().to_string();
            let body = match name.as_str() {
                "breakpoint_set" => {
                    assert!(line.contains("-f file:///app/index.php") && line.contains("-n 5"), "{line}");
                    format!(r#"<response command="breakpoint_set" transaction_id="{id}" id="1001"/>"#)
                }
                "run" => format!(r#"<response command="run" transaction_id="{id}" status="break" reason="ok"><xdebug:message filename="file:///app/index.php" lineno="5"></xdebug:message></response>"#),
                "stack_get" => format!(r#"<response command="stack_get" transaction_id="{id}"><stack where="{{main}}" level="0" type="file" filename="file:///app/index.php" lineno="5"></stack></response>"#),
                "context_get" => format!(r#"<response command="context_get" transaction_id="{id}"><property name="$name" fullname="$name" type="string" encoding="base64"><![CDATA[QWRh]]></property><property name="$items" fullname="$items" type="array" children="1" numchildren="2"></property></response>"#),
                "eval" => {
                    let data = line.split(" -- ").nth(1).unwrap();
                    assert_eq!(BASE64.decode(data).unwrap(), b"1 + 1");
                    format!(r#"<response command="eval" transaction_id="{id}"><property type="int"><![CDATA[2]]></property></response>"#)
                }
                "property_get" => format!(r#"<response command="property_get" transaction_id="{id}"><error code="300"><message>can not get property</message></error></response>"#),
                "stop" => {
                    stream.write_all(&packet(&format!(r#"<response command="stop" transaction_id="{id}" status="stopped"/>"#))).unwrap();
                    return;
                }
                _ => format!(r#"<response command="{name}" transaction_id="{id}"/>"#),
            };
            // A stream packet before the response must be skipped.
            stream.write_all(&packet(r#"<stream type="stdout" encoding="base64">aGk=</stream>"#)).unwrap();
            stream.write_all(&packet(&body)).unwrap();
        }
    }

    #[test]
    fn talks_to_an_engine() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let engine = std::thread::spawn(move || engine(port));
        let mut session = Session::accept(&listener).unwrap();
        assert_eq!(session.init.file_uri, "file:///app/index.php");
        session.feature_set("max_depth", "1").unwrap();
        assert_eq!(session.set_breakpoint("file:///app/index.php", 5, None).unwrap(), "1001");
        let status = session.run().unwrap();
        assert!(status.is_break());
        assert_eq!((status.file_uri.as_deref(), status.line), (Some("file:///app/index.php"), Some(5)));
        assert_eq!(session.stack().unwrap()[0].function, "{main}");
        let vars = session.variables(0, 0).unwrap();
        assert_eq!(vars[0].display(), "\"Ada\"");
        assert_eq!(vars[1].display(), "array(2)");
        assert!(vars[1].has_children);
        assert_eq!(session.eval("1 + 1").unwrap().display(), "2");
        assert!(matches!(session.property("$nope", 0), Err(Error::Engine { .. })));
        session.stop().unwrap();
        engine.join().unwrap();
    }

    #[test]
    fn converts_paths() {
        assert_eq!(uri_to_path("file:///var/www/a%20b.php"), PathBuf::from("/var/www/a b.php"));
        assert_eq!(uri_to_path("file:///C:/www/a.php"), PathBuf::from("C:/www/a.php"));
        assert_eq!(path_to_uri(Path::new("/var/www/a b.php")), "file:///var/www/a%20b.php");
        assert_eq!(path_to_uri(Path::new("C:\\www\\a.php")), "file:///C:/www/a.php");
    }
}
