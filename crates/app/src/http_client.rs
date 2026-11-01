//! HTTP Client (PhpStorm's `.http` files): requests separated by `###`, each a request line
//! (`GET https://…`), headers and an optional body. `{{name}}` is replaced from `@name = value`
//! lines in the file and from `http-client.env.json` / `http-client.private.env.json` next to
//! it (the first environment, or the one chosen).

use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, Instant};

const METHODS: &[&str] = &["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS", "TRACE", "CONNECT"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub name: Option<String>,
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

pub struct Response {
    pub status: u16,
    pub reason: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
    pub elapsed: Duration,
}

pub fn is_http_file(path: &Path) -> bool {
    path.extension().is_some_and(|e| e == "http" || e == "rest")
}

/// `@name = value` variables declared in the file.
pub fn file_variables(text: &str) -> HashMap<String, String> {
    text.lines()
        .filter_map(|line| {
            let rest = line.trim().strip_prefix('@')?;
            let (name, value) = rest.split_once('=')?;
            Some((name.trim().to_string(), value.trim().to_string()))
        })
        .collect()
}

/// Environment names and the chosen environment's variables from the env files in `dir`
/// (private values override public ones).
pub fn environment(dir: &Path, chosen: Option<&str>) -> (Vec<String>, Option<String>, HashMap<String, String>) {
    let read = |name: &str| -> serde_json::Map<String, serde_json::Value> {
        std::fs::read_to_string(dir.join(name))
            .ok()
            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
            .and_then(|v| v.as_object().cloned())
            .unwrap_or_default()
    };
    let public = read("http-client.env.json");
    let private = read("http-client.private.env.json");
    let mut names: Vec<String> = public.keys().chain(private.keys()).cloned().collect();
    names.dedup();
    let mut seen = std::collections::HashSet::new();
    names.retain(|n| seen.insert(n.clone()));
    let env = chosen.filter(|c| names.iter().any(|n| n == c)).map(str::to_string).or_else(|| names.first().cloned());
    let mut vars = HashMap::new();
    if let Some(env) = &env {
        for source in [&public, &private] {
            if let Some(object) = source.get(env).and_then(|v| v.as_object()) {
                for (k, v) in object {
                    let value = v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string());
                    vars.insert(k.clone(), value);
                }
            }
        }
    }
    (names, env, vars)
}

/// Replaces `{{name}}`; `{{$uuid}}`, `{{$timestamp}}` and `{{$randomInt}}` are generated.
pub fn substitute(text: &str, vars: &HashMap<String, String>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else {
            out.push_str(&rest[start..]);
            return out;
        };
        let name = after[..end].trim();
        let value = match name {
            "$timestamp" => Some(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0).to_string()),
            "$uuid" | "$random.uuid" => Some(pseudo_uuid()),
            "$randomInt" => Some((nanos() % 1000).to_string()),
            _ => vars.get(name).cloned(),
        };
        match value {
            Some(value) => out.push_str(&value),
            None => out.push_str(&rest[start..start + 2 + end + 2]),
        }
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    out
}

fn nanos() -> u128 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0)
}

fn pseudo_uuid() -> String {
    let n = nanos() ^ (std::process::id() as u128) << 64;
    let hex = format!("{n:032x}");
    format!("{}-{}-4{}-a{}-{}", &hex[0..8], &hex[8..12], &hex[13..16], &hex[17..20], &hex[20..32])
}

/// The request block containing `line` (0-based), parsed. Blocks are separated by `###`.
pub fn request_at(text: &str, line: usize) -> Option<Request> {
    let lines: Vec<&str> = text.lines().collect();
    let mut start = 0;
    let mut name = None;
    for (ix, l) in lines.iter().enumerate().take(line.min(lines.len().saturating_sub(1)) + 1) {
        if l.trim_start().starts_with("###") {
            start = ix + 1;
            let title = l.trim_start().trim_start_matches('#').trim();
            name = (!title.is_empty()).then(|| title.to_string());
        }
    }
    let end = lines[start.min(lines.len())..].iter().position(|l| l.trim_start().starts_with("###")).map_or(lines.len(), |p| start + p);
    let mut request = parse_block(&lines[start.min(end)..end])?;
    request.name = request.name.or(name);
    Some(request)
}

/// Every request in the file with the 0-based line of its request line (for gutter markers).
pub fn requests(text: &str) -> Vec<(usize, Request)> {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = Vec::new();
    let mut start = 0;
    for ix in 0..=lines.len() {
        if ix == lines.len() || lines[ix].trim_start().starts_with("###") {
            if let Some(request) = parse_block(&lines[start..ix]) {
                let at = (start..ix).find(|i| request_line(lines[*i]).is_some()).unwrap_or(start);
                out.push((at, request));
            }
            start = ix + 1;
        }
    }
    out
}

fn is_comment(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with('#') || t.starts_with("//")
}

fn request_line(line: &str) -> Option<(String, String)> {
    let t = line.trim();
    if t.is_empty() || is_comment(t) || t.starts_with('@') {
        return None;
    }
    let mut parts = t.split_whitespace();
    let first = parts.next()?;
    if METHODS.contains(&first.to_ascii_uppercase().as_str()) {
        let url = parts.next()?.to_string();
        return Some((first.to_ascii_uppercase(), url));
    }
    // A bare URL means GET.
    (first.starts_with("http://") || first.starts_with("https://") || first.starts_with("{{")).then(|| ("GET".to_string(), first.to_string()))
}

fn parse_block(lines: &[&str]) -> Option<Request> {
    let mut name = None;
    let mut ix = 0;
    let (method, url) = loop {
        let line = lines.get(ix)?;
        if let Some(found) = request_line(line) {
            break found;
        }
        if let Some(title) = line.trim().strip_prefix("# @name").or_else(|| line.trim().strip_prefix("// @name")) {
            name = Some(title.trim().trim_start_matches('=').trim().to_string());
        }
        ix += 1;
    };
    ix += 1;
    let mut headers = Vec::new();
    while let Some(line) = lines.get(ix) {
        if line.trim().is_empty() {
            ix += 1;
            break;
        }
        if !is_comment(line)
            && let Some((key, value)) = line.split_once(':')
        {
            headers.push((key.trim().to_string(), value.trim().to_string()));
        }
        ix += 1;
    }
    let body_lines: Vec<&str> = lines.get(ix..).unwrap_or_default().iter().copied().filter(|l| !l.trim_start().starts_with("> ") && !l.trim_start().starts_with("<> ")).collect();
    let body = body_lines.join("\n").trim().to_string();
    Some(Request { name, method, url, headers, body })
}

/// Applies variables to the URL, headers and body.
pub fn resolve(request: &Request, vars: &HashMap<String, String>) -> Request {
    Request {
        name: request.name.clone(),
        method: request.method.clone(),
        url: substitute(&request.url, vars),
        headers: request.headers.iter().map(|(k, v)| (k.clone(), substitute(v, vars))).collect(),
        body: substitute(&request.body, vars),
    }
}

/// Sends the request (blocking). HTTP error statuses are responses, not errors.
pub fn send(request: &Request) -> Result<Response, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(60)))
        .build()
        .into();
    let mut builder = ureq::http::Request::builder().method(request.method.as_str()).uri(request.url.as_str());
    for (key, value) in &request.headers {
        builder = builder.header(key.as_str(), value.as_str());
    }
    let started = Instant::now();
    let result = if request.body.is_empty() {
        agent.run(builder.body(()).map_err(|e| e.to_string())?)
    } else {
        agent.run(builder.body(request.body.clone()).map_err(|e| e.to_string())?)
    };
    let mut response = result.map_err(|e| e.to_string())?;
    let status = response.status();
    let headers = response.headers().iter().map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string())).collect();
    let body = response.body_mut().with_config().limit(20 * 1024 * 1024).read_to_string().unwrap_or_else(|e| format!("<could not read body: {e}>"));
    Ok(Response { status: status.as_u16(), reason: status.canonical_reason().unwrap_or("").to_string(), headers, body, elapsed: started.elapsed() })
}

/// The response as text: status, headers, then the body (JSON pretty-printed).
pub fn format_response(request: &Request, response: &Response) -> String {
    let mut out = format!("{} {}\n\nHTTP {} {} · {} ms · {} bytes\n", request.method, request.url, response.status, response.reason, response.elapsed.as_millis(), response.body.len());
    for (k, v) in &response.headers {
        out.push_str(&format!("{k}: {v}\n"));
    }
    out.push('\n');
    let is_json = response.headers.iter().any(|(k, v)| k.eq_ignore_ascii_case("content-type") && v.contains("json"));
    let pretty = is_json.then(|| serde_json::from_str::<serde_json::Value>(&response.body).ok().and_then(|v| serde_json::to_string_pretty(&v).ok())).flatten();
    out.push_str(pretty.as_deref().unwrap_or(&response.body));
    out
}

/// Starter content for a new `.http` file.
pub const TEMPLATE: &str = "### Get users\nGET {{host}}/api/users\nAccept: application/json\n\n### Create a user\nPOST {{host}}/api/users\nContent-Type: application/json\n\n{\n  \"name\": \"Ann\"\n}\n";

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = "@token = abc\n\n### List\nGET {{host}}/users?page=1\nAuthorization: Bearer {{token}}\n\n### Create\n# @name createUser\nPOST https://api.test/users\nContent-Type: application/json\n\n{\"name\": \"{{who}}\"}\n\n> {% client.global.set('id', response.body.id) %}\n###\nhttps://example.com\n";

    #[test]
    fn parses_blocks_and_variables() {
        let vars = HashMap::from([("host".to_string(), "https://api.test".to_string()), ("who".to_string(), "Ann".to_string())]);
        let mut all = file_variables(FILE);
        all.extend(vars);
        let list = request_at(FILE, 3).unwrap();
        assert_eq!(list.name.as_deref(), Some("List"));
        let list = resolve(&list, &all);
        assert_eq!(list.url, "https://api.test/users?page=1");
        assert_eq!(list.headers, vec![("Authorization".to_string(), "Bearer abc".to_string())]);
        let create = request_at(FILE, 9).unwrap();
        assert_eq!(create.name.as_deref(), Some("createUser"));
        assert_eq!(create.method, "POST");
        assert_eq!(resolve(&create, &all).body, "{\"name\": \"Ann\"}");
        let bare = request_at(FILE, 15).unwrap();
        assert_eq!((bare.method.as_str(), bare.url.as_str()), ("GET", "https://example.com"));
        assert_eq!(requests(FILE).iter().map(|(l, _)| *l).collect::<Vec<_>>(), [3, 8, 15]);
        assert_eq!(substitute("{{missing}}", &HashMap::new()), "{{missing}}");
    }

    #[test]
    fn environments() {
        let dir = std::env::temp_dir().join(format!("phpcraft-http-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("http-client.env.json"), r#"{"dev": {"host": "http://localhost"}, "prod": {"host": "https://x.az"}}"#).unwrap();
        std::fs::write(dir.join("http-client.private.env.json"), r#"{"dev": {"token": "secret"}}"#).unwrap();
        let (names, env, vars) = environment(&dir, None);
        assert_eq!(names, ["dev", "prod"]);
        assert_eq!(env.as_deref(), Some("dev"));
        assert_eq!(vars.get("token").map(String::as_str), Some("secret"));
        let (_, env, vars) = environment(&dir, Some("prod"));
        assert_eq!((env.as_deref(), vars.get("host").map(String::as_str)), (Some("prod"), Some("https://x.az")));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
