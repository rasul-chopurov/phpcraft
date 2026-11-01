//! Laravel support: navigation and completion for routes, views, config, translations,
//! env keys, Inertia pages and Blade components — the Laravel Idea feature set.

mod array_keys;

use std::fs;
use std::ops::Range;
use std::path::{Path, PathBuf};

use php_analysis::{CompletionItem, CompletionKind, StringArg, StringProvider};
use php_index::{Index, Location};

pub use array_keys::array_keys;

const VIEW_EXTENSIONS: &[&str] = &[".blade.php", ".php"];
const INERTIA_EXTENSIONS: &[&str] = &["vue", "tsx", "jsx", "svelte", "ts", "js"];

/// What a Laravel string literal refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    View,
    Route,
    Config,
    Lang,
    Env,
    Inertia,
}

pub fn is_laravel_project(root: &Path) -> bool {
    root.join("artisan").is_file()
}

/// Teaches the index Laravel's magic forwarding (`User::where(...)` lives on the query builder).
pub fn register_mixins(index: &mut Index) {
    index.add_mixin("Illuminate\\Database\\Eloquent\\Model", "Illuminate\\Database\\Eloquent\\Builder");
    index.add_mixin("Illuminate\\Database\\Eloquent\\Builder", "Illuminate\\Database\\Query\\Builder");
}

pub struct Laravel {
    root: PathBuf,
}

impl Laravel {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Classifies a string argument by the call it is passed to.
    pub fn classify(arg: &StringArg) -> Option<Kind> {
        let first = arg.arg_index == 0;
        let kind = if arg.is_static(&["Inertia"], &["render"]) || arg.is_function(&["inertia"]) {
            Kind::Inertia
        } else if (first && (arg.is_function(&["view"]) || arg.is_static(&["View"], &["make", "exists", "first"])))
            || (arg.is_method(&["view", "markdown", "text"]) && !arg.value.starts_with('/'))
        {
            Kind::View
        } else if first && (arg.is_function(&["route", "to_route"]) || arg.is_method(&["route", "routeIs", "toRoute"])) {
            Kind::Route
        } else if first && (arg.is_function(&["config"]) || arg.is_static(&["Config"], &["get", "has", "set", "string", "integer", "boolean", "array"])) {
            Kind::Config
        } else if first && (arg.is_function(&["__", "trans", "trans_choice"]) || arg.is_static(&["Lang"], &["get", "has", "choice"])) {
            Kind::Lang
        } else if first && arg.is_function(&["env"]) {
            Kind::Env
        } else {
            return None;
        };
        Some(kind)
    }

    pub fn targets(&self, kind: Kind, value: &str) -> Vec<Location> {
        match kind {
            Kind::View => self.view_file(value).map(file_location).into_iter().collect(),
            Kind::Inertia => self.inertia_page(value).map(file_location).into_iter().collect(),
            Kind::Route => self.route_definitions(value),
            Kind::Config => self.config_key(value).into_iter().collect(),
            Kind::Lang => self.lang_key(value),
            Kind::Env => self.env_key(value),
        }
    }

    pub fn candidates(&self, kind: Kind) -> Vec<(String, String)> {
        match kind {
            Kind::View => self.view_names().into_iter().map(|v| (v, "view".into())).collect(),
            Kind::Inertia => self.inertia_pages().into_iter().map(|v| (v, "page".into())).collect(),
            Kind::Route => self.route_names().into_iter().map(|(n, _)| (n, "route".into())).collect(),
            Kind::Config => self.config_keys(),
            Kind::Lang => self.lang_keys(),
            Kind::Env => self.env_keys(),
        }
    }

    // --- views ---------------------------------------------------------

    fn views_dir(&self) -> PathBuf {
        self.root.join("resources/views")
    }

    pub fn view_file(&self, name: &str) -> Option<PathBuf> {
        // Package views (`mail::message`) live in vendor; not resolved here.
        if name.contains("::") || name.is_empty() {
            return None;
        }
        let relative = name.replace('.', "/");
        VIEW_EXTENSIONS
            .iter()
            .map(|ext| self.views_dir().join(format!("{relative}{ext}")))
            .find(|p| p.is_file())
    }

    pub fn view_names(&self) -> Vec<String> {
        let dir = self.views_dir();
        let mut files = Vec::new();
        walk(&dir, &mut files);
        let mut names: Vec<String> = files
            .iter()
            .filter_map(|p| {
                let rel = p.strip_prefix(&dir).ok()?.to_string_lossy().replace('\\', "/");
                let stem = VIEW_EXTENSIONS.iter().find_map(|ext| rel.strip_suffix(ext))?;
                Some(stem.replace('/', "."))
            })
            .collect();
        names.sort();
        names
    }

    /// `<x-forms.input>` -> anonymous component view or class component.
    pub fn component(&self, name: &str) -> Vec<Location> {
        let mut out = Vec::new();
        let view = format!("components.{name}");
        if let Some(path) = self.view_file(&view).or_else(|| self.view_file(&format!("{view}.index"))) {
            out.push(file_location(path));
        }
        let class_path: Vec<String> = name.split('.').map(studly).collect();
        let class_file = self.root.join("app/View/Components").join(format!("{}.php", class_path.join("/")));
        if class_file.is_file() {
            out.push(file_location(class_file));
        }
        out
    }

    /// `<livewire:counter />` -> app/Livewire/Counter.php (v3) or app/Http/Livewire (v2).
    pub fn livewire(&self, name: &str) -> Vec<Location> {
        let class: Vec<String> = name.split('.').map(studly).collect();
        let class = format!("{}.php", class.join("/"));
        ["app/Livewire", "app/Http/Livewire"]
            .iter()
            .map(|dir| self.root.join(dir).join(&class))
            .filter(|p| p.is_file())
            .map(file_location)
            .collect()
    }

    // --- Inertia -------------------------------------------------------

    fn pages_dir(&self) -> PathBuf {
        self.root.join("resources/js/Pages")
    }

    pub fn inertia_page(&self, name: &str) -> Option<PathBuf> {
        INERTIA_EXTENSIONS
            .iter()
            .map(|ext| self.pages_dir().join(format!("{name}.{ext}")))
            .find(|p| p.is_file())
    }

    fn inertia_pages(&self) -> Vec<String> {
        let dir = self.pages_dir();
        let mut files = Vec::new();
        walk(&dir, &mut files);
        files
            .iter()
            .filter(|p| p.extension().is_some_and(|e| INERTIA_EXTENSIONS.contains(&e.to_string_lossy().as_ref())))
            .filter_map(|p| {
                let rel = p.strip_prefix(&dir).ok()?.with_extension("");
                Some(rel.to_string_lossy().replace('\\', "/"))
            })
            .collect()
    }

    // --- routes --------------------------------------------------------

    /// Every `->name('x')` / `'as' => 'x'` in routes/, with the string's location.
    pub fn route_names(&self) -> Vec<(String, Location)> {
        let mut files = Vec::new();
        walk(&self.root.join("routes"), &mut files);
        let mut out = Vec::new();
        for path in files.into_iter().filter(|p| p.extension().is_some_and(|e| e == "php")) {
            let Ok(source) = fs::read_to_string(&path) else { continue };
            for marker in ["->name(", "'as' =>", "\"as\" =>"] {
                for (at, _) in source.match_indices(marker) {
                    if let Some((value, range)) = next_string(&source, at + marker.len()) {
                        // Group prefixes like `->name('admin.')` aren't routes themselves.
                        if !value.ends_with('.') {
                            out.push((value, Location { path: path.clone(), range }));
                        }
                    }
                }
            }
        }
        out
    }

    pub fn route_definitions(&self, name: &str) -> Vec<Location> {
        self.route_names()
            .into_iter()
            .filter(|(n, _)| n == name)
            .map(|(_, loc)| loc)
            .collect()
    }

    // --- config & translations -----------------------------------------

    pub fn config_key(&self, key: &str) -> Option<Location> {
        let (file, rest) = key.split_once('.').unwrap_or((key, ""));
        let path = self.root.join("config").join(format!("{file}.php"));
        if !path.is_file() {
            return None;
        }
        if rest.is_empty() {
            return Some(file_location(path));
        }
        let source = fs::read_to_string(&path).ok()?;
        let range = array_keys(&source).into_iter().find(|(k, _)| k == rest)?.1;
        Some(Location { path, range })
    }

    fn config_keys(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        for path in php_files_in(&self.root.join("config")) {
            let Some(file) = path.file_stem().map(|s| s.to_string_lossy().into_owned()) else { continue };
            out.push((file.clone(), "config".into()));
            let Ok(source) = fs::read_to_string(&path) else { continue };
            for (key, _) in array_keys(&source) {
                out.push((format!("{file}.{key}"), "config".into()));
            }
        }
        out
    }

    fn lang_dirs(&self) -> Vec<PathBuf> {
        // Unpublished framework translations live in vendor (Laravel 11+ has no lang/ by default).
        ["lang", "resources/lang", "vendor/laravel/framework/src/Illuminate/Translation/lang"]
            .iter()
            .map(|d| self.root.join(d))
            .filter(|d| d.is_dir())
            .collect()
    }

    pub fn lang_key(&self, key: &str) -> Vec<Location> {
        let mut out = Vec::new();
        for dir in self.lang_dirs() {
            let Ok(entries) = fs::read_dir(&dir) else { continue };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    // lang/en/messages.php => 'messages.welcome'
                    let Some((file, rest)) = key.split_once('.') else { continue };
                    let file_path = path.join(format!("{file}.php"));
                    let Ok(source) = fs::read_to_string(&file_path) else { continue };
                    if let Some((_, range)) = array_keys(&source).into_iter().find(|(k, _)| k == rest) {
                        out.push(Location { path: file_path, range });
                    }
                } else if path.extension().is_some_and(|e| e == "json") {
                    // lang/en.json => 'Welcome back'
                    let Ok(source) = fs::read_to_string(&path) else { continue };
                    let needle = format!("\"{}\"", key.replace('"', "\\\""));
                    if let Some(at) = source.find(&needle) {
                        out.push(Location { path, range: at + 1..at + needle.len() - 1 });
                    }
                }
            }
        }
        out
    }

    fn lang_keys(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        for dir in self.lang_dirs() {
            let Ok(entries) = fs::read_dir(&dir) else { continue };
            for entry in entries.flatten() {
                let locale = entry.file_name().to_string_lossy().into_owned();
                for path in php_files_in(&entry.path()) {
                    let Some(file) = path.file_stem().map(|s| s.to_string_lossy().into_owned()) else { continue };
                    let Ok(source) = fs::read_to_string(&path) else { continue };
                    for (key, _) in array_keys(&source) {
                        out.push((format!("{file}.{key}"), locale.clone()));
                    }
                }
            }
        }
        out
    }

    // --- env -----------------------------------------------------------

    fn env_files(&self) -> Vec<PathBuf> {
        [".env", ".env.example"]
            .iter()
            .map(|f| self.root.join(f))
            .filter(|p| p.is_file())
            .collect()
    }

    pub fn env_key(&self, key: &str) -> Vec<Location> {
        let mut out = Vec::new();
        for path in self.env_files() {
            let Ok(source) = fs::read_to_string(&path) else { continue };
            let mut offset = 0;
            for line in source.split_inclusive('\n') {
                if line.trim_start().strip_prefix(key).is_some_and(|rest| rest.trim_start().starts_with('=')) {
                    let start = offset + line.find(key).unwrap_or(0);
                    out.push(Location { path: path.clone(), range: start..start + key.len() });
                }
                offset += line.len();
            }
        }
        out
    }

    fn env_keys(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        for path in self.env_files() {
            let Ok(source) = fs::read_to_string(&path) else { continue };
            for line in source.lines() {
                if let Some((key, value)) = line.split_once('=')
                    && !key.trim_start().starts_with('#')
                {
                    out.push((key.trim().to_string(), value.trim().to_string()));
                }
            }
        }
        out
    }

    // --- Blade ---------------------------------------------------------

    /// Navigation inside Blade templates (and a text fallback for PHP): directives like
    /// `@include('x')`, helpers like `{{ route('x') }}`, and `<x-component>` tags.
    pub fn text_goto(&self, source: &str, offset: usize) -> Vec<Location> {
        if let Some((tag, name)) = tag_at(source, offset) {
            return match tag {
                Tag::Component => self.component(&name),
                Tag::Livewire => self.livewire(&name),
            };
        }
        let Some((callee, value)) = quoted_call_at(source, offset) else {
            return Vec::new();
        };
        let kind = match callee.as_str() {
            "include" | "includeif" | "includewhen" | "includefirst" | "extends" | "component"
            | "each" | "view" => Kind::View,
            "route" | "to_route" | "routeis" => Kind::Route,
            "config" => Kind::Config,
            "lang" | "__" | "trans" | "choice" | "trans_choice" => Kind::Lang,
            "env" => Kind::Env,
            _ => return Vec::new(),
        };
        self.targets(kind, &value)
    }
}

impl StringProvider for Laravel {
    fn goto(&self, arg: &StringArg) -> Vec<Location> {
        Self::classify(arg)
            .map(|kind| self.targets(kind, &arg.value))
            .unwrap_or_default()
    }

    fn complete(&self, arg: &StringArg) -> Vec<CompletionItem> {
        let Some(kind) = Self::classify(arg) else {
            return Vec::new();
        };
        self.candidates(kind)
            .into_iter()
            .map(|(label, detail)| CompletionItem::simple(label, detail, CompletionKind::Value, arg.range.clone()))
            .collect()
    }
}

fn file_location(path: PathBuf) -> Location {
    Location { path, range: 0..0 }
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, out);
        } else {
            out.push(path);
        }
    }
}

fn php_files_in(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else { return Vec::new() };
    let mut files: Vec<_> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "php"))
        .collect();
    files.sort();
    files
}

fn studly(segment: &str) -> String {
    segment
        .split(['-', '_'])
        .map(|part| {
            let mut chars = part.chars();
            chars.next().map(|c| c.to_uppercase().chain(chars).collect::<String>()).unwrap_or_default()
        })
        .collect()
}

/// The first quoted string at or after `from`, skipping whitespace: (value, contents range).
fn next_string(source: &str, from: usize) -> Option<(String, Range<usize>)> {
    let rest = &source[from..];
    let skipped = rest.len() - rest.trim_start().len();
    let start = from + skipped;
    let quote = source[start..].chars().next().filter(|c| *c == '\'' || *c == '"')?;
    let end = source[start + 1..].find(quote)? + start + 1;
    Some((source[start + 1..end].to_string(), start + 1..end))
}

/// For `name('value')` or `@name('value')` with `offset` inside the quotes: (lowercased name, value).
fn quoted_call_at(source: &str, offset: usize) -> Option<(String, String)> {
    let line_start = source[..offset].rfind('\n').map_or(0, |i| i + 1);
    let line_end = source[offset..].find('\n').map_or(source.len(), |i| offset + i);
    let line = &source[line_start..line_end];
    let col = offset - line_start;
    let open = line[..col].rfind(['\'', '"'])?;
    let quote = line.as_bytes()[open] as char;
    let close = line[col..].find(quote)? + col;
    let before = line[..open].trim_end().strip_suffix('(')?.trim_end();
    let name: String = before
        .chars()
        .rev()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    (!name.is_empty()).then(|| (name.to_ascii_lowercase(), line[open + 1..close].to_string()))
}

enum Tag {
    Component,
    Livewire,
}

/// `<x-alert` / `<livewire:counter` with `offset` on the tag name.
fn tag_at(source: &str, offset: usize) -> Option<(Tag, String)> {
    let is_name = |c: char| c.is_alphanumeric() || matches!(c, '-' | '.' | ':' | '_');
    let start = source[..offset]
        .char_indices()
        .rev()
        .take_while(|(_, c)| is_name(*c))
        .last()
        .map_or(offset, |(i, _)| i);
    if !source[..start].ends_with('<') && !source[..start].ends_with("</") {
        return None;
    }
    let end = source[offset..].find(|c| !is_name(c)).map_or(source.len(), |i| offset + i);
    let name = &source[start..end];
    if let Some(component) = name.strip_prefix("x-") {
        Some((Tag::Component, component.to_string()))
    } else {
        name.strip_prefix("livewire:").map(|n| (Tag::Livewire, n.to_string()))
    }
}

#[cfg(test)]
mod tests;
