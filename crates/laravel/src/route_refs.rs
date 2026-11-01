//! Routes read straight from `routes/*.php` (no `artisan`): which controller method, invokable
//! or Livewire class each `Route::get(...)` / `Route::resource(...)` dispatches to. Used for
//! the gutter mark on controllers.

use std::collections::HashMap;
use std::path::Path;

use php_index::Location;

#[derive(Debug, Clone, PartialEq)]
pub struct RouteRef {
    /// `GET`, `POST`… (`RESOURCE` actions get their own verbs).
    pub verb: String,
    pub uri: String,
    pub name: Option<String>,
    /// Fully-qualified controller class.
    pub controller: String,
    /// The method, or `None` for an invokable / Livewire class.
    pub method: Option<String>,
    /// The `Route::…` call in the routes file.
    pub location: Location,
}

const VERBS: &[&str] = &["get", "post", "put", "patch", "delete", "options", "any", "match", "resource", "apiResource", "view", "redirect"];

/// `/* */` comments and `//` / `#` line comments blanked out (offsets kept).
fn blank_comments(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let bytes = source.as_bytes();
    let mut i = 0;
    let mut quote: Option<u8> = None;
    while i < bytes.len() {
        let c = bytes[i];
        if let Some(q) = quote {
            if c == b'\\' && i + 1 < bytes.len() {
                out.push_str(&source[i..i + 2]);
                i += 2;
                continue;
            }
            if c == q {
                quote = None;
            }
            out.push(c as char);
            i += 1;
            continue;
        }
        if c == b'\'' || c == b'"' {
            quote = Some(c);
        } else if source[i..].starts_with("/*") {
            let end = source[i + 2..].find("*/").map_or(bytes.len(), |e| i + 2 + e + 2);
            out.extend(source[i..end].chars().map(|ch| if ch == '\n' { '\n' } else { ' ' }));
            i = end;
            continue;
        } else if source[i..].starts_with("//") || (c == b'#' && !source[i..].starts_with("#[")) {
            let end = source[i..].find('\n').map_or(bytes.len(), |e| i + e);
            out.extend(std::iter::repeat_n(' ', source[i..end].chars().count()));
            i = end;
            continue;
        }
        let len = source[i..].chars().next().map_or(1, char::len_utf8);
        out.push_str(&source[i..i + len]);
        i += len;
    }
    out
}

/// `use A\B;` / `use A\B as C;` → alias → FQN.
fn imports(source: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for line in source.lines() {
        let Some(rest) = line.trim().strip_prefix("use ") else { continue };
        let rest = rest.trim_end_matches(';').trim();
        let (fqn, alias) = match rest.split_once(" as ") {
            Some((f, a)) => (f.trim(), a.trim().to_string()),
            None => (rest, rest.rsplit('\\').next().unwrap_or(rest).to_string()),
        };
        map.insert(alias, fqn.trim_start_matches('\\').to_string());
    }
    map
}

fn resolve_class(name: &str, imports: &HashMap<String, String>) -> String {
    let name = name.trim().trim_end_matches("::class").trim();
    if let Some(fqn) = name.strip_prefix('\\') {
        return fqn.to_string();
    }
    let (first, rest) = name.split_once('\\').map_or((name, None), |(a, b)| (a, Some(b)));
    match (imports.get(first), rest) {
        (Some(fqn), Some(rest)) => format!("{fqn}\\{rest}"),
        (Some(fqn), None) => fqn.clone(),
        _ if name.contains('\\') => name.to_string(),
        _ => format!("App\\Http\\Controllers\\{name}"),
    }
}

/// Top-level comma-separated arguments of the call starting at `open` (`(`).
fn arguments(source: &str, open: usize) -> Option<(Vec<&str>, usize)> {
    let mut depth = 0;
    let mut quote: Option<char> = None;
    let mut start = open + 1;
    let mut args = Vec::new();
    for (i, c) in source[open..].char_indices() {
        let at = open + i;
        if let Some(q) = quote {
            if c == q {
                quote = None;
            }
            continue;
        }
        match c {
            '\'' | '"' => quote = Some(c),
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => {
                depth -= 1;
                if depth == 0 {
                    args.push(&source[start..at]);
                    return Some((args, at));
                }
            }
            ',' if depth == 1 => {
                args.push(&source[start..at]);
                start = at + 1;
            }
            _ => {}
        }
    }
    None
}

fn unquote(s: &str) -> Option<String> {
    let s = s.trim();
    let q = s.chars().next().filter(|c| *c == '\'' || *c == '"')?;
    s.strip_prefix(q)?.strip_suffix(q).map(str::to_string)
}

/// Routes of one routes file.
pub fn parse_routes(path: &Path, source: &str) -> Vec<RouteRef> {
    let code = blank_comments(source);
    let imports = imports(&code);
    let mut out = Vec::new();
    let mut search = 0;
    while let Some(at) = code[search..].find("Route::") {
        let start = search + at;
        search = start + 7;
        let verb_len = code[search..].find(|c: char| !c.is_alphanumeric()).unwrap_or(0);
        let verb = &code[search..search + verb_len];
        if !VERBS.contains(&verb) || !code[search + verb_len..].starts_with('(') {
            continue;
        }
        let Some((args, close)) = arguments(&code, search + verb_len) else { continue };
        let statement_end = code[close..].find(';').map_or(code.len(), |i| close + i);
        let name = code[close..statement_end].find("->name(").and_then(|i| unquote(code[close + i + 7..statement_end].split(')').next()?));
        let uri = args.first().and_then(|a| unquote(a)).unwrap_or_default();
        let location = Location { path: path.to_path_buf(), range: start..start + 7 + verb_len };
        let action = args.get(1).map(|a| a.trim()).unwrap_or("");
        let push = |verb: &str, controller: String, method: Option<String>, out: &mut Vec<RouteRef>| {
            out.push(RouteRef { verb: verb.to_uppercase(), uri: uri.clone(), name: name.clone(), controller, method, location: location.clone() });
        };
        match verb {
            "resource" | "apiResource" => {
                let controller = resolve_class(action, &imports);
                let methods: &[&str] = if verb == "resource" { &["index", "create", "store", "show", "edit", "update", "destroy"] } else { &["index", "store", "show", "update", "destroy"] };
                for m in methods {
                    push(verb, controller.clone(), Some(m.to_string()), &mut out);
                }
            }
            _ if action.starts_with('[') => {
                let inner = action.trim_start_matches('[').trim_end_matches(']');
                let mut parts = inner.splitn(2, ',');
                let class = parts.next().unwrap_or("");
                let method = parts.next().and_then(unquote);
                push(verb, resolve_class(class, &imports), method, &mut out);
            }
            _ if action.ends_with("::class") => push(verb, resolve_class(action, &imports), None, &mut out),
            _ => {
                if let Some((class, method)) = unquote(action).as_deref().and_then(|a| a.split_once('@')) {
                    push(verb, resolve_class(class, &imports), Some(method.to_string()), &mut out);
                }
            }
        }
    }
    out
}

/// Every route under `routes/`.
pub fn route_refs(root: &Path) -> Vec<RouteRef> {
    let mut files = Vec::new();
    crate::walk_dir(&root.join("routes"), &mut files);
    files
        .into_iter()
        .filter(|f| f.extension().is_some_and(|e| e == "php"))
        .flat_map(|f| std::fs::read_to_string(&f).map(|s| parse_routes(&f, &s)).unwrap_or_default())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_routes() {
        let src = r#"<?php
use App\Http\Controllers\UserController;
use App\Livewire\Pages\Home;

Route::get('/', Home::class)->name('home');
Route::get('/users/{user}', [UserController::class, 'show'])->name('users.show');
// Route::get('/old', [UserController::class, 'old']);
Route::post('/legacy', 'App\Http\Controllers\LegacyController@store');
Route::apiResource('photos', \App\Http\Controllers\PhotoController::class);
Route::get('/closure', function () { return 1; });
"#;
        let routes = parse_routes(Path::new("/p/routes/web.php"), src);
        let find = |controller: &str, method: Option<&str>| routes.iter().find(|r| r.controller == controller && r.method.as_deref() == method);
        assert_eq!(find("App\\Livewire\\Pages\\Home", None).unwrap().name.as_deref(), Some("home"));
        let show = find("App\\Http\\Controllers\\UserController", Some("show")).unwrap();
        assert_eq!((show.verb.as_str(), show.uri.as_str()), ("GET", "/users/{user}"));
        assert!(find("App\\Http\\Controllers\\UserController", Some("old")).is_none(), "commented out");
        assert!(find("App\\Http\\Controllers\\LegacyController", Some("store")).is_some());
        assert_eq!(routes.iter().filter(|r| r.controller == "App\\Http\\Controllers\\PhotoController").count(), 5);
        assert_eq!(routes.len(), 8);
    }
}
