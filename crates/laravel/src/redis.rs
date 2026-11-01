//! A small Redis client (RESP over TCP) for the Database window: keys are listed like tables,
//! a key opens as a grid of its value by type, and the console runs raw commands.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

use crate::database::QueryResult;

/// How many keys the tree lists (SCAN stops after this many).
const MAX_KEYS: usize = 2000;

#[derive(Debug, Clone, PartialEq)]
pub enum Reply {
    Simple(String),
    Error(String),
    Int(i64),
    Bulk(Option<String>),
    Array(Option<Vec<Reply>>),
}

impl Reply {
    fn text(&self) -> Option<String> {
        match self {
            Reply::Simple(s) | Reply::Error(s) => Some(s.clone()),
            Reply::Int(n) => Some(n.to_string()),
            Reply::Bulk(b) => b.clone(),
            Reply::Array(a) => a.as_ref().map(|items| items.iter().map(|i| i.text().unwrap_or_else(|| "(nil)".into())).collect::<Vec<_>>().join(", ")),
        }
    }

    fn items(self) -> Vec<Reply> {
        match self {
            Reply::Array(Some(items)) => items,
            _ => Vec::new(),
        }
    }
}

pub struct Connection {
    reader: BufReader<TcpStream>,
}

/// Splits a console line into arguments, honouring "double" and 'single' quotes.
pub fn split_args(line: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut has = false;
    let mut chars = line.chars().peekable();
    while let Some(ch) = chars.next() {
        match (quote, ch) {
            (Some(q), c) if c == q => quote = None,
            (Some('"'), '\\') => {
                if let Some(next) = chars.next() {
                    current.push(match next {
                        'n' => '\n',
                        't' => '\t',
                        other => other,
                    });
                }
            }
            (Some(_), c) => current.push(c),
            (None, '"' | '\'') => {
                quote = Some(ch);
                has = true;
            }
            (None, c) if c.is_whitespace() => {
                if has || !current.is_empty() {
                    args.push(std::mem::take(&mut current));
                    has = false;
                }
            }
            (None, c) => current.push(c),
        }
    }
    if has || !current.is_empty() {
        args.push(current);
    }
    args
}

impl Connection {
    pub fn open(host: &str, port: u16, password: &str, user: &str, db: &str) -> Result<Self, String> {
        let host = if host.is_empty() || host == "localhost" { "127.0.0.1" } else { host };
        let addr = std::net::ToSocketAddrs::to_socket_addrs(&(host, port)).map_err(|e| e.to_string())?.next().ok_or("unknown host")?;
        let stream = TcpStream::connect_timeout(&addr, Duration::from_secs(5)).map_err(|e| format!("redis {host}:{port}: {e}"))?;
        stream.set_read_timeout(Some(Duration::from_secs(15))).ok();
        let mut connection = Self { reader: BufReader::new(stream) };
        if !password.is_empty() {
            let auth: Vec<&str> = if user.is_empty() || user == "default" { vec!["AUTH", password] } else { vec!["AUTH", user, password] };
            connection.ok(&auth)?;
        }
        if let Ok(n) = db.trim().parse::<u32>()
            && n > 0
        {
            connection.ok(&["SELECT", &n.to_string()])?;
        }
        Ok(connection)
    }

    pub fn command(&mut self, args: &[&str]) -> Result<Reply, String> {
        let mut out = format!("*{}\r\n", args.len()).into_bytes();
        for arg in args {
            out.extend_from_slice(format!("${}\r\n", arg.len()).as_bytes());
            out.extend_from_slice(arg.as_bytes());
            out.extend_from_slice(b"\r\n");
        }
        self.reader.get_mut().write_all(&out).map_err(|e| e.to_string())?;
        read_reply(&mut self.reader)
    }

    fn ok(&mut self, args: &[&str]) -> Result<Reply, String> {
        match self.command(args)? {
            Reply::Error(e) => Err(e),
            reply => Ok(reply),
        }
    }

    /// Up to MAX_KEYS keys, sorted.
    pub fn keys(&mut self) -> Result<Vec<String>, String> {
        let mut cursor = "0".to_string();
        let mut keys = Vec::new();
        loop {
            let reply = self.ok(&["SCAN", &cursor, "COUNT", "500"])?.items();
            let mut parts = reply.into_iter();
            cursor = parts.next().and_then(|c| c.text()).unwrap_or_else(|| "0".into());
            keys.extend(parts.next().map(Reply::items).unwrap_or_default().into_iter().filter_map(|k| k.text()));
            if cursor == "0" || keys.len() >= MAX_KEYS {
                break;
            }
        }
        keys.sort();
        keys.truncate(MAX_KEYS);
        Ok(keys)
    }

    /// A key's value as a grid, by type.
    pub fn value(&mut self, key: &str) -> Result<QueryResult, String> {
        let started = Instant::now();
        let kind = self.ok(&["TYPE", key])?.text().unwrap_or_default();
        let pairs = |items: Vec<Reply>| -> Vec<Vec<Option<String>>> { items.chunks(2).map(|c| c.iter().map(Reply::text).collect()).collect() };
        let (columns, rows): (Vec<&str>, Vec<Vec<Option<String>>>) = match kind.as_str() {
            "string" => (vec!["value"], vec![vec![self.ok(&["GET", key])?.text()]]),
            "hash" => (vec!["field", "value"], pairs(self.ok(&["HGETALL", key])?.items())),
            "list" => (vec!["index", "value"], self.ok(&["LRANGE", key, "0", "999"])?.items().iter().enumerate().map(|(i, v)| vec![Some(i.to_string()), v.text()]).collect()),
            "set" => (vec!["member"], self.ok(&["SMEMBERS", key])?.items().iter().map(|v| vec![v.text()]).collect()),
            "zset" => (vec!["member", "score"], pairs(self.ok(&["ZRANGE", key, "0", "999", "WITHSCORES"])?.items())),
            "none" => return Err(format!("Key {key} doesn't exist")),
            other => (vec!["value"], vec![vec![Some(format!("<{other}>"))]]),
        };
        let total = rows.len();
        Ok(QueryResult { columns: columns.into_iter().map(str::to_string).collect(), rows, total, affected: 0, millis: started.elapsed().as_secs_f64() * 1000. })
    }

    /// Runs a console line (`HGETALL user:1`) and shows the reply as rows.
    pub fn run_line(&mut self, line: &str) -> Result<QueryResult, String> {
        let started = Instant::now();
        let args = split_args(line);
        if args.is_empty() {
            return Err("Type a Redis command, e.g. KEYS *".into());
        }
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let reply = self.command(&refs)?;
        let rows: Vec<Vec<Option<String>>> = match reply {
            Reply::Error(e) => return Err(e),
            Reply::Array(Some(items)) => items.iter().map(|i| vec![i.text()]).collect(),
            other => vec![vec![other.text()]],
        };
        let total = rows.len();
        Ok(QueryResult { columns: vec!["reply".into()], rows, total, affected: 0, millis: started.elapsed().as_secs_f64() * 1000. })
    }
}

fn read_line(reader: &mut impl BufRead) -> Result<String, String> {
    let mut line = String::new();
    reader.read_line(&mut line).map_err(|e| e.to_string())?;
    if line.is_empty() {
        return Err("connection closed".into());
    }
    Ok(line.trim_end_matches(['\r', '\n']).to_string())
}

fn read_reply(reader: &mut impl BufRead) -> Result<Reply, String> {
    let line = read_line(reader)?;
    let (kind, rest) = line.split_at(1.min(line.len()));
    Ok(match kind {
        "+" => Reply::Simple(rest.into()),
        "-" => Reply::Error(rest.into()),
        ":" => Reply::Int(rest.parse().unwrap_or(0)),
        "$" => {
            let len: i64 = rest.parse().map_err(|_| "bad bulk length")?;
            if len < 0 {
                Reply::Bulk(None)
            } else {
                let mut buf = vec![0; len as usize + 2];
                reader.read_exact(&mut buf).map_err(|e| e.to_string())?;
                buf.truncate(len as usize);
                Reply::Bulk(Some(String::from_utf8_lossy(&buf).into_owned()))
            }
        }
        "*" => {
            let n: i64 = rest.parse().map_err(|_| "bad array length")?;
            if n < 0 {
                Reply::Array(None)
            } else {
                Reply::Array(Some((0..n).map(|_| read_reply(reader)).collect::<Result<_, _>>()?))
            }
        }
        // RESP3 types we don't ask for: treat as text.
        _ => Reply::Simple(line),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_replies_and_args() {
        let data = b"*3\r\n$5\r\nhello\r\n:42\r\n$-1\r\n-ERR nope\r\n";
        let mut reader = BufReader::new(&data[..]);
        let reply = read_reply(&mut reader).unwrap();
        assert_eq!(reply, Reply::Array(Some(vec![Reply::Bulk(Some("hello".into())), Reply::Int(42), Reply::Bulk(None)])));
        assert_eq!(read_reply(&mut reader).unwrap(), Reply::Error("ERR nope".into()));
        assert_eq!(split_args(r#"SET "user name" 'a b' x"#), ["SET", "user name", "a b", "x"]);
        assert_eq!(split_args("GET \"\""), ["GET", ""]);
    }

    #[test]
    fn talks_to_a_server() {
        // A fake server answering SCAN, TYPE and HGETALL.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut writer = stream;
            let replies: [&[u8]; 3] = [b"*2\r\n$1\r\n0\r\n*2\r\n$6\r\nuser:1\r\n$4\r\nhits\r\n", b"+hash\r\n", b"*4\r\n$4\r\nname\r\n$3\r\nAnn\r\n$3\r\nage\r\n$2\r\n30\r\n"];
            for reply in replies {
                // Consume one request (an array of bulk strings).
                let header = read_line(&mut reader).unwrap();
                let n: usize = header[1..].parse().unwrap();
                for _ in 0..n {
                    read_line(&mut reader).unwrap();
                    read_line(&mut reader).unwrap();
                }
                writer.write_all(reply).unwrap();
            }
        });
        let mut redis = Connection::open("127.0.0.1", port, "", "", "0").unwrap();
        assert_eq!(redis.keys().unwrap(), ["hits", "user:1"]);
        let value = redis.value("user:1").unwrap();
        assert_eq!(value.columns, ["field", "value"]);
        assert_eq!(value.rows, vec![vec![Some("name".into()), Some("Ann".into())], vec![Some("age".into()), Some("30".into())]]);
    }
}
