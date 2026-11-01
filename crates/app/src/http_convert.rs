//! HTTP Client conversions: cURL ↔ `.http` requests, Postman collections and OpenAPI / Swagger
//! (JSON) to `.http` files, and the request history kept in `.phpcraft/http/`.

use std::path::{Path, PathBuf};

use crate::http_client::{Request, Response};

/// Splits a shell command line into words (quotes and backslash-newlines handled).
fn shell_words(text: &str) -> Vec<String> {
    let text = text.replace("\\\r\n", " ").replace("\\\n", " ");
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quote: Option<char> = None;
    let mut in_word = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some('"'), '\\') => {
                if let Some(next) = chars.next() {
                    word.push(next);
                }
            }
            (Some(_), c) => word.push(c),
            (None, '\'' | '"') => {
                quote = Some(c);
                in_word = true;
            }
            (None, '\\') => {
                if let Some(next) = chars.next() {
                    word.push(next);
                    in_word = true;
                }
            }
            (None, c) if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut word));
                    in_word = false;
                }
            }
            (None, c) => {
                word.push(c);
                in_word = true;
            }
        }
    }
    if in_word {
        words.push(word);
    }
    words
}

/// A `curl …` command as a request.
pub fn from_curl(command: &str) -> Option<Request> {
    let words = shell_words(command.trim());
    if words.first().map(String::as_str) != Some("curl") {
        return None;
    }
    let mut request = Request { name: None, method: String::new(), url: String::new(), headers: Vec::new(), body: String::new() };
    let mut i = 1;
    while i < words.len() {
        let w = words[i].as_str();
        let next = words.get(i + 1).cloned().unwrap_or_default();
        match w {
            "-X" | "--request" => {
                request.method = next.to_uppercase();
                i += 1;
            }
            "-H" | "--header" => {
                if let Some((k, v)) = next.split_once(':') {
                    request.headers.push((k.trim().to_string(), v.trim().to_string()));
                }
                i += 1;
            }
            "-d" | "--data" | "--data-raw" | "--data-binary" | "--data-ascii" => {
                if !request.body.is_empty() {
                    request.body.push('&');
                }
                request.body.push_str(&next);
                i += 1;
            }
            "--json" => {
                request.body = next;
                request.headers.push(("Content-Type".into(), "application/json".into()));
                request.headers.push(("Accept".into(), "application/json".into()));
                i += 1;
            }
            "-u" | "--user" => {
                request.headers.push(("Authorization".into(), format!("Basic {}", base64(&next))));
                i += 1;
            }
            "-A" | "--user-agent" => {
                request.headers.push(("User-Agent".into(), next));
                i += 1;
            }
            "-b" | "--cookie" => {
                request.headers.push(("Cookie".into(), next));
                i += 1;
            }
            "--url" => {
                request.url = next;
                i += 1;
            }
            w if w.starts_with('-') => {
                // Flags with a value we don't use.
                if matches!(w, "-o" | "--output" | "-w" | "--write-out" | "-m" | "--max-time" | "--connect-timeout" | "-e" | "--referer") {
                    i += 1;
                }
            }
            url => request.url = url.to_string(),
        }
        i += 1;
    }
    if request.method.is_empty() {
        request.method = if request.body.is_empty() { "GET".into() } else { "POST".into() };
    }
    (!request.url.is_empty()).then_some(request)
}

fn base64(text: &str) -> String {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes = text.as_bytes();
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(TABLE[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// A request in `.http` syntax.
pub fn to_http(request: &Request) -> String {
    let mut out = String::new();
    if let Some(name) = &request.name {
        out.push_str(&format!("### {name}\n"));
    } else {
        out.push_str("###\n");
    }
    out.push_str(&format!("{} {}\n", request.method, request.url));
    for (k, v) in &request.headers {
        out.push_str(&format!("{k}: {v}\n"));
    }
    if !request.body.is_empty() {
        out.push('\n');
        out.push_str(request.body.trim_end());
        out.push('\n');
    }
    out
}

/// A request as a `curl` command.
pub fn to_curl(request: &Request) -> String {
    let quote = |s: &str| format!("'{}'", s.replace('\'', "'\\''"));
    let mut parts = vec!["curl".to_string()];
    if request.method != "GET" {
        parts.push(format!("-X {}", request.method));
    }
    parts.push(quote(&request.url));
    for (k, v) in &request.headers {
        parts.push(format!("-H {}", quote(&format!("{k}: {v}"))));
    }
    if !request.body.is_empty() {
        parts.push(format!("--data-raw {}", quote(&request.body)));
    }
    parts.join(" \\\n  ")
}

/// A Postman collection (v2 / v2.1) as an `.http` file: folders become `#` comments.
pub fn from_postman(json: &str) -> Result<String, String> {
    let value: serde_json::Value = serde_json::from_str(json).map_err(|e| format!("Not a Postman collection: {e}"))?;
    let mut out = String::new();
    if let Some(name) = value.pointer("/info/name").and_then(|n| n.as_str()) {
        out.push_str(&format!("# {name} (imported from Postman)\n\n"));
    }
    for var in value.get("variable").and_then(|v| v.as_array()).into_iter().flatten() {
        if let (Some(k), Some(v)) = (var.get("key").and_then(|k| k.as_str()), var.get("value").and_then(|v| v.as_str())) {
            out.push_str(&format!("@{k} = {v}\n"));
        }
    }
    fn walk(items: &[serde_json::Value], folder: &str, out: &mut String) {
        for item in items {
            let name = item.get("name").and_then(|n| n.as_str()).unwrap_or("Request");
            if let Some(children) = item.get("item").and_then(|i| i.as_array()) {
                let path = if folder.is_empty() { name.to_string() } else { format!("{folder} / {name}") };
                walk(children, &path, out);
                continue;
            }
            let Some(req) = item.get("request") else { continue };
            let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("GET");
            let url = match req.get("url") {
                Some(serde_json::Value::String(s)) => s.clone(),
                Some(u) => u.get("raw").and_then(|r| r.as_str()).unwrap_or("").to_string(),
                None => String::new(),
            };
            let headers = req
                .get("header")
                .and_then(|h| h.as_array())
                .into_iter()
                .flatten()
                .filter(|h| !h.get("disabled").and_then(|d| d.as_bool()).unwrap_or(false))
                .filter_map(|h| Some((h.get("key")?.as_str()?.to_string(), h.get("value")?.as_str()?.to_string())))
                .collect();
            let body = req.pointer("/body/raw").and_then(|b| b.as_str()).unwrap_or("").to_string();
            let title = if folder.is_empty() { name.to_string() } else { format!("{folder} / {name}") };
            out.push('\n');
            out.push_str(&to_http(&Request { name: Some(title), method: method.to_uppercase(), url, headers, body }));
        }
    }
    let items = value.get("item").and_then(|i| i.as_array()).ok_or("The collection has no requests")?;
    walk(items, "", &mut out);
    Ok(out)
}

/// An OpenAPI 3 / Swagger 2 document (JSON) as `.http` requests against `{{baseUrl}}`.
pub fn from_openapi(json: &str) -> Result<String, String> {
    let value: serde_json::Value = serde_json::from_str(json).map_err(|e| format!("Not an OpenAPI JSON document: {e}"))?;
    let paths = value.get("paths").and_then(|p| p.as_object()).ok_or("No paths in the document")?;
    let base = value
        .pointer("/servers/0/url")
        .and_then(|u| u.as_str())
        .map(str::to_string)
        .or_else(|| {
            let host = value.get("host")?.as_str()?;
            let base_path = value.get("basePath").and_then(|b| b.as_str()).unwrap_or("");
            Some(format!("https://{host}{base_path}"))
        })
        .unwrap_or_else(|| "http://localhost".into());
    let title = value.pointer("/info/title").and_then(|t| t.as_str()).unwrap_or("API");
    let mut out = format!("# {title} (generated from OpenAPI)\n@baseUrl = {base}\n");
    for (path, item) in paths {
        let Some(item) = item.as_object() else { continue };
        for (method, op) in item {
            if !matches!(method.as_str(), "get" | "post" | "put" | "patch" | "delete" | "head" | "options") {
                continue;
            }
            let name = op.get("summary").or_else(|| op.get("operationId")).and_then(|s| s.as_str()).unwrap_or(path);
            // `{id}` path parameters become `{{id}}` variables.
            let url_path = path.replace('{', "{{").replace('}', "}}");
            let query: Vec<String> = op
                .get("parameters")
                .and_then(|p| p.as_array())
                .into_iter()
                .flatten()
                .filter(|p| p.get("in").and_then(|i| i.as_str()) == Some("query"))
                .filter_map(|p| p.get("name")?.as_str().map(|n| format!("{n}=")))
                .collect();
            let url = if query.is_empty() { format!("{{{{baseUrl}}}}{url_path}") } else { format!("{{{{baseUrl}}}}{url_path}?{}", query.join("&")) };
            let has_json_body = op.pointer("/requestBody/content/application~1json").is_some()
                || op.get("parameters").and_then(|p| p.as_array()).is_some_and(|p| p.iter().any(|x| x.get("in").and_then(|i| i.as_str()) == Some("body")));
            let mut headers = vec![("Accept".to_string(), "application/json".to_string())];
            let body = if has_json_body {
                headers.push(("Content-Type".into(), "application/json".into()));
                "{\n}".to_string()
            } else {
                String::new()
            };
            out.push('\n');
            out.push_str(&to_http(&Request { name: Some(name.to_string()), method: method.to_uppercase(), url, headers, body }));
        }
    }
    Ok(out)
}

/// Appends a request and its response to `.phpcraft/http/history.http` (newest last) and saves
/// the body under `responses/`. Returns the response file.
pub fn record(root: &Path, request: &Request, response: &Response) -> Option<PathBuf> {
    let dir = root.join(".phpcraft/http");
    let responses = dir.join("responses");
    std::fs::create_dir_all(&responses).ok()?;
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).ok()?.as_secs();
    let content_type = response.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case("content-type")).map(|(_, v)| v.to_lowercase()).unwrap_or_default();
    let ext = if content_type.contains("json") { "json" } else if content_type.contains("html") { "html" } else if content_type.contains("xml") { "xml" } else { "txt" };
    let file = responses.join(format!("{stamp}-{}.{ext}", response.status));
    std::fs::write(&file, &response.body).ok()?;
    let mut entry = to_http(request).replacen("###", &format!("### {} — {} {}", stamp, response.status, response.reason), 1);
    entry.push_str(&format!("\n<> responses/{}\n\n", file.file_name()?.to_string_lossy()));
    let history = dir.join("history.http");
    let mut text = std::fs::read_to_string(&history).unwrap_or_default();
    text.push_str(&entry);
    // Keep the last 200 requests.
    let blocks: Vec<&str> = text.split("\n### ").collect();
    if blocks.len() > 200 {
        text = blocks[blocks.len() - 200..].join("\n### ");
    }
    std::fs::write(&history, text).ok()?;
    Some(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn curl_round_trip() {
        let curl = "curl -X POST 'https://api.test/users?x=1' \\\n  -H 'Content-Type: application/json' -H \"Authorization: Bearer abc\" \\\n  --data-raw '{\"name\": \"Ann\"}'";
        let request = from_curl(curl).unwrap();
        assert_eq!((request.method.as_str(), request.url.as_str()), ("POST", "https://api.test/users?x=1"));
        assert_eq!(request.headers[1], ("Authorization".to_string(), "Bearer abc".to_string()));
        assert_eq!(request.body, "{\"name\": \"Ann\"}");
        let back = to_curl(&request);
        assert!(back.contains("-X POST") && back.contains("--data-raw '{\"name\": \"Ann\"}'"), "{back}");
        assert_eq!(from_curl(&back).unwrap(), request);
        let basic = from_curl("curl -u ann:secret https://x.test").unwrap();
        assert_eq!(basic.headers[0].1, "Basic YW5uOnNlY3JldA==");
        assert!(to_http(&request).starts_with("###\nPOST https://api.test/users?x=1\nContent-Type: application/json\n"));
    }

    #[test]
    fn imports_postman_and_openapi() {
        let postman = r#"{"info":{"name":"Shop"},"variable":[{"key":"host","value":"https://shop.test"}],"item":[{"name":"Users","item":[{"name":"List","request":{"method":"GET","url":{"raw":"{{host}}/users"},"header":[{"key":"Accept","value":"application/json"}]}}]}]}"#;
        let http = from_postman(postman).unwrap();
        assert!(http.contains("@host = https://shop.test") && http.contains("### Users / List\nGET {{host}}/users\nAccept: application/json"), "{http}");
        let openapi = r#"{"openapi":"3.0.0","info":{"title":"Pets"},"servers":[{"url":"https://pets.test/v1"}],"paths":{"/pets/{id}":{"get":{"summary":"Show pet","parameters":[{"name":"id","in":"path"},{"name":"fields","in":"query"}]}},"/pets":{"post":{"operationId":"addPet","requestBody":{"content":{"application/json":{}}}}}}}"#;
        let http = from_openapi(openapi).unwrap();
        assert!(http.contains("@baseUrl = https://pets.test/v1"));
        assert!(http.contains("### Show pet\nGET {{baseUrl}}/pets/{{id}}?fields="), "{http}");
        assert!(http.contains("### addPet\nPOST {{baseUrl}}/pets\nAccept: application/json\nContent-Type: application/json\n\n{\n}"), "{http}");
    }
}
