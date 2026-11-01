//! Blade beyond file names: sections and stacks (`@yield` ↔ `@section`, `@stack` ↔ `@push`),
//! component props (`@props([...])` or a class component's constructor) for `$variables`
//! inside the component and `type=""` / `:type=""` attributes where it is used.

use std::ops::Range;
use std::path::{Path, PathBuf};

use php_analysis::{CompletionItem, CompletionKind};
use php_index::Location;

use crate::{Laravel, VIEW_EXTENSIONS, studly, walk};

/// Sections and stacks pair up: where content is defined and where it is printed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    Section,
    Stack,
}

impl Slot {
    /// The directive kind for `@name(` (lowercase), and whether it is the "print" side.
    pub fn of(directive: &str) -> Option<(Slot, bool)> {
        match directive {
            "yield" => Some((Slot::Section, true)),
            "section" | "hassection" | "sectionmissing" => Some((Slot::Section, false)),
            "stack" => Some((Slot::Stack, true)),
            "push" | "prepend" | "pushonce" | "prependonce" | "pushif" => Some((Slot::Stack, false)),
            _ => None,
        }
    }

    fn directives(self, printing: bool) -> &'static [&'static str] {
        match (self, printing) {
            (Slot::Section, true) => &["yield"],
            (Slot::Section, false) => &["section"],
            (Slot::Stack, true) => &["stack"],
            (Slot::Stack, false) => &["push", "prepend", "pushOnce", "prependOnce", "pushIf"],
        }
    }
}

/// `@directive('name'` occurrences in `source`: (directive, name, byte range of the name).
pub fn directive_names(source: &str) -> Vec<(String, String, Range<usize>)> {
    let mut out = Vec::new();
    let mut rest = 0;
    while let Some(at) = source[rest..].find('@') {
        let start = rest + at + 1;
        let word_len = source[start..].find(|c: char| !c.is_ascii_alphanumeric()).unwrap_or(source.len() - start);
        let directive = &source[start..start + word_len];
        let after = source[start + word_len..].trim_start();
        rest = start + word_len;
        let Some(args) = after.strip_prefix('(') else { continue };
        let args = args.trim_start();
        let Some(quote) = args.chars().next().filter(|c| *c == '\'' || *c == '"') else { continue };
        let name_start = source.len() - args.len() + 1;
        let Some(len) = source[name_start..].find(quote) else { continue };
        out.push((directive.to_ascii_lowercase(), source[name_start..name_start + len].to_string(), name_start..name_start + len));
    }
    out
}

/// The `@extends('layouts.app')` of a view.
fn extends(source: &str) -> Option<String> {
    directive_names(source).into_iter().find(|(d, ..)| d == "extends").map(|(_, n, _)| n)
}

/// `@props(['type' => 'info', 'message'])`: (name, default, byte range of the name).
pub fn props(source: &str) -> Vec<(String, Option<String>, Range<usize>)> {
    let Some(at) = source.find("@props(") else { return Vec::new() };
    let start = at + "@props(".len();
    let mut depth = 1;
    let mut end = source.len();
    for (i, c) in source[start..].char_indices() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => {
                depth -= 1;
                if depth == 0 && c == ')' {
                    end = start + i;
                    break;
                }
            }
            _ => {}
        }
    }
    let inner = source[start..end].trim().trim_start_matches('[').trim_end_matches(']');
    let inner_start = start + source[start..end].find(inner).unwrap_or(0);
    let mut out = Vec::new();
    let (mut depth, mut item_start) = (0, 0);
    let mut items = Vec::new();
    for (i, c) in inner.char_indices() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => depth -= 1,
            ',' if depth == 0 => {
                items.push(item_start..i);
                item_start = i + 1;
            }
            _ => {}
        }
    }
    items.push(item_start..inner.len());
    for item in items {
        let text = &inner[item.clone()];
        let (key, default) = match text.split_once("=>") {
            Some((k, v)) => (k.trim(), Some(v.trim().to_string())),
            None => (text.trim(), None),
        };
        let name = key.trim_matches(['\'', '"']);
        if name.is_empty() || !name.chars().all(|c| c.is_alphanumeric() || c == '_') {
            continue;
        }
        let offset = inner_start + item.start + text.find(name).unwrap_or(0);
        out.push((name.to_string(), default, offset..offset + name.len()));
    }
    out
}

/// `kebab-case` attribute → `camelCase` prop (Blade converts them).
fn camel(attr: &str) -> String {
    let mut out = String::new();
    let mut upper = false;
    for c in attr.chars() {
        if c == '-' {
            upper = true;
        } else if upper {
            out.extend(c.to_uppercase());
            upper = false;
        } else {
            out.push(c);
        }
    }
    out
}

/// The `<x-name` tag around `offset` (inside its attributes), and the attribute word there.
fn component_tag_at(source: &str, offset: usize) -> Option<(String, Range<usize>)> {
    let open = source[..offset].rfind('<')?;
    if source[open..offset].contains('>') {
        return None;
    }
    let tag = source[open + 1..].split(|c: char| c.is_whitespace() || c == '>' || c == '/').next()?;
    let component = tag.strip_prefix("x-")?.to_string();
    let is_attr = |c: char| c.is_alphanumeric() || matches!(c, '-' | '_' | ':');
    let start = source[..offset].char_indices().rev().take_while(|(_, c)| is_attr(*c)).last().map_or(offset, |(i, _)| i);
    let end = source[offset..].find(|c: char| !is_attr(c)).map_or(source.len(), |i| offset + i);
    // Not on the tag name itself.
    (start > open + 1 + tag.len()).then_some((component, start..end))
}

impl Laravel {
    fn view_files(&self) -> Vec<PathBuf> {
        let mut files = Vec::new();
        walk(&self.views_dir(), &mut files);
        files.into_iter().filter(|p| VIEW_EXTENSIONS.iter().any(|e| p.to_string_lossy().ends_with(e))).collect()
    }

    /// Every `@directive('name')` of `slot` on one side (printing or defining) in all views.
    fn slot_locations(&self, slot: Slot, printing: bool, name: Option<&str>) -> Vec<(String, Location)> {
        let wanted = slot.directives(printing);
        let mut out = Vec::new();
        for path in self.view_files() {
            let Ok(source) = std::fs::read_to_string(&path) else { continue };
            for (directive, found, range) in directive_names(&source) {
                if wanted.iter().any(|w| w.eq_ignore_ascii_case(&directive)) && name.is_none_or(|n| n == found) {
                    out.push((found, Location { path: path.clone(), range }));
                }
            }
        }
        out
    }

    /// ⌘B on a section or stack name: from `@yield` / `@stack` to where content is given, and
    /// back (preferring the layout this view extends).
    pub fn slot_targets(&self, source: &str, directive: &str, name: &str) -> Vec<Location> {
        let Some((slot, printing)) = Slot::of(directive) else { return Vec::new() };
        if !printing && let Some(layout) = extends(source).and_then(|l| self.view_file(&l)) {
            let text = std::fs::read_to_string(&layout).unwrap_or_default();
            let found: Vec<Location> = directive_names(&text)
                .into_iter()
                .filter(|(d, n, _)| slot.directives(true).contains(&d.as_str()) && n == name)
                .map(|(_, _, range)| Location { path: layout.clone(), range })
                .collect();
            if !found.is_empty() {
                return found;
            }
        }
        self.slot_locations(slot, !printing, Some(name)).into_iter().map(|(_, l)| l).collect()
    }

    /// Completion inside `@yield('…')`, `@section('…')`, `@stack('…')`, `@push('…')`.
    pub fn slot_completions(&self, source: &str, offset: usize) -> Option<Vec<CompletionItem>> {
        let line_start = source[..offset].rfind('\n').map_or(0, |i| i + 1);
        let before = &source[line_start..offset];
        let quote = before.rfind(['\'', '"'])?;
        let call = before[..quote].trim_end().strip_suffix('(')?;
        let directive: String = call.chars().rev().take_while(|c| c.is_ascii_alphanumeric()).collect::<Vec<_>>().into_iter().rev().collect();
        if !call[..call.len() - directive.len()].ends_with('@') {
            return None;
        }
        let (slot, printing) = Slot::of(&directive.to_ascii_lowercase())?;
        let typed = &before[quote + 1..];
        let replace = offset - typed.len()..offset;
        // Names from the other side first, then this side.
        let mut names: Vec<String> = self.slot_locations(slot, !printing, None).into_iter().chain(self.slot_locations(slot, printing, None)).map(|(n, _)| n).collect();
        names.sort();
        names.dedup();
        let detail = if slot == Slot::Section { "section" } else { "stack" };
        Some(names.into_iter().filter(|n| n.starts_with(typed)).map(|n| CompletionItem::simple(n, detail, CompletionKind::Keyword, replace.clone())).collect())
    }

    /// The props of `<x-name>`: (prop, default, where it is declared).
    pub fn component_props(&self, name: &str) -> Vec<(String, Option<String>, Location)> {
        let mut out = Vec::new();
        let view = format!("components.{name}");
        if let Some(path) = self.view_file(&view).or_else(|| self.view_file(&format!("{view}.index")))
            && let Ok(source) = std::fs::read_to_string(&path)
        {
            out.extend(props(&source).into_iter().map(|(n, d, range)| (n, d, Location { path: path.clone(), range })));
        }
        // A class component: its constructor's parameters.
        let class: Vec<String> = name.split('.').map(studly).collect();
        let file = self.root.join("app/View/Components").join(format!("{}.php", class.join("/")));
        if let Ok(source) = std::fs::read_to_string(&file)
            && let Some(at) = source.find("function __construct(")
        {
            let params_start = at + "function __construct(".len();
            let params_end = source[params_start..].find(")\n").or_else(|| source[params_start..].find(')')).map_or(source.len(), |i| params_start + i);
            let params = &source[params_start..params_end];
            let mut search = 0;
            while let Some(dollar) = params[search..].find('$') {
                let start = search + dollar + 1;
                let len = params[start..].find(|c: char| !(c.is_alphanumeric() || c == '_')).unwrap_or(params.len() - start);
                let name = &params[start..start + len];
                let rest = params[start + len..].trim_start();
                let default = rest.strip_prefix('=').map(|d| d.split(',').next().unwrap_or("").trim().to_string());
                let offset = params_start + start;
                out.push((name.to_string(), default, Location { path: file.clone(), range: offset..offset + len }));
                search = start + len;
            }
        }
        out
    }

    /// ⌘B on an attribute of `<x-name …>`: where that prop is declared.
    pub fn component_attribute_target(&self, source: &str, offset: usize) -> Vec<Location> {
        let Some((component, range)) = component_tag_at(source, offset) else { return Vec::new() };
        let attr = source[range].trim_start_matches(':');
        let prop = camel(attr);
        self.component_props(&component).into_iter().filter(|(n, ..)| *n == prop).map(|(.., l)| l).collect()
    }

    /// Attribute completion inside `<x-name …>` from the component's props.
    pub fn component_attribute_completions(&self, source: &str, offset: usize) -> Option<Vec<CompletionItem>> {
        let (component, range) = component_tag_at(source, offset)?;
        let replace = range.start..offset;
        let typed = &source[replace.clone()];
        let props = self.component_props(&component);
        if props.is_empty() {
            return None;
        }
        let mut items = Vec::new();
        for (name, default, _) in props {
            let kebab: String = name.chars().flat_map(|c| if c.is_uppercase() { vec!['-', c.to_ascii_lowercase()] } else { vec![c] }).collect();
            let detail = default.map(|d| format!("= {d}")).unwrap_or_else(|| "required".into());
            for label in [kebab.clone(), format!(":{kebab}")] {
                if label.starts_with(typed) {
                    let mut item = CompletionItem::simple(label.clone(), detail.clone(), CompletionKind::Property, replace.clone());
                    item.insert = format!("{label}=\"\"");
                    items.push(item);
                }
            }
        }
        Some(items)
    }
}

/// `$variable` completion inside a component view: its props plus `$attributes` and `$slot`.
pub fn prop_variable_completions(path: &Path, source: &str, offset: usize) -> Option<Vec<CompletionItem>> {
    let word_start = source[..offset].char_indices().rev().take_while(|(_, c)| c.is_alphanumeric() || *c == '_').last().map_or(offset, |(i, _)| i);
    let dollar = word_start.checked_sub(1).filter(|&i| source.as_bytes()[i] == b'$')?;
    let in_component = path.components().any(|c| c.as_os_str() == "components");
    let declared = props(source);
    if declared.is_empty() && !in_component {
        return None;
    }
    let typed = &source[word_start..offset];
    let mut names: Vec<(String, String)> = declared.into_iter().map(|(n, d, _)| (n, d.map(|d| format!("= {d}")).unwrap_or_else(|| "prop".into()))).collect();
    if in_component {
        names.push(("attributes".into(), "ComponentAttributeBag".into()));
        names.push(("slot".into(), "ComponentSlot".into()));
    }
    Some(
        names
            .into_iter()
            .filter(|(n, _)| n.starts_with(typed))
            .map(|(n, detail)| CompletionItem::simple(format!("${n}"), detail, CompletionKind::Variable, dollar..offset))
            .collect(),
    )
}

/// Variables a Blade fragment uses (`$user`, not `$loop`, `$errors`, `$slot`…), in order.
fn fragment_variables(text: &str) -> Vec<String> {
    const IMPLICIT: &[&str] = &["this", "loop", "errors", "slot", "attributes", "component", "app", "message", "__env", "__data"];
    let mut found: Vec<String> = Vec::new();
    let bytes = text.as_bytes();
    for (i, _) in text.match_indices('$') {
        let len = text[i + 1..].find(|c: char| !(c.is_alphanumeric() || c == '_')).unwrap_or(text.len() - i - 1);
        let name = &text[i + 1..i + 1 + len];
        // `@foreach($users as $user)` defines `$user` inside the fragment.
        let defined = text[..i].trim_end().ends_with(" as") || (i > 0 && bytes[i - 1] == b'>' && text[..i].trim_end().ends_with("=>"));
        if !name.is_empty() && !name.starts_with(|c: char| c.is_ascii_digit()) && !IMPLICIT.contains(&name) && !defined && !found.iter().any(|f| f == name) {
            found.push(name.to_string());
        }
    }
    // Loop variables are defined by the fragment itself.
    let defined: Vec<String> = text.match_indices(" as $").filter_map(|(i, _)| {
        let rest = &text[i + 5..];
        Some(rest[..rest.find(|c: char| !(c.is_alphanumeric() || c == '_'))?].to_string())
    }).collect();
    found.retain(|v| !defined.contains(v));
    found
}

/// Removes the common indentation of `text`'s lines.
fn dedent(text: &str) -> String {
    let indent = text.lines().filter(|l| !l.trim().is_empty()).map(|l| l.len() - l.trim_start().len()).min().unwrap_or(0);
    text.lines().map(|l| l.get(indent..).unwrap_or(l.trim_start())).collect::<Vec<_>>().join("\n").trim_end().to_string() + "\n"
}

/// Refactor ▸ Extract Blade Component / Extract View: the new file's content and what replaces
/// the selection. `name` is dotted (`cards.user`).
pub fn extract(selection: &str, name: &str, component: bool) -> (String, String) {
    let body = dedent(selection);
    if !component {
        return (body, format!("@include('{name}')"));
    }
    let vars = fragment_variables(selection);
    let tag: String = name.to_string();
    if vars.is_empty() {
        return (body, format!("<x-{tag} />"));
    }
    let props = vars.iter().map(|v| format!("'{v}'")).collect::<Vec<_>>().join(", ");
    let attrs = vars.iter().map(|v| {
        let kebab: String = v.chars().flat_map(|c| if c.is_uppercase() { vec!['-', c.to_ascii_lowercase()] } else { vec![c] }).collect();
        format!(":{kebab}=\"${v}\"")
    }).collect::<Vec<_>>().join(" ");
    (format!("@props([{props}])\n\n{body}"), format!("<x-{tag} {attrs} />"))
}

/// Where Extract View / Extract Blade Component writes `name`.
pub fn extracted_path(views: &Path, name: &str, component: bool) -> PathBuf {
    let rel = name.replace('.', "/");
    if component { views.join("components").join(format!("{rel}.blade.php")) } else { views.join(format!("{rel}.blade.php")) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> PathBuf {
        let root = std::env::temp_dir().join(format!("phpcraft-blade-{}-{}", std::process::id(), line!()));
        let views = root.join("resources/views");
        std::fs::create_dir_all(views.join("layouts")).unwrap();
        std::fs::create_dir_all(views.join("components")).unwrap();
        std::fs::write(views.join("layouts/app.blade.php"), "<title>@yield('title')</title>\n@yield('content')\n@stack('scripts')\n").unwrap();
        std::fs::write(views.join("home.blade.php"), "@extends('layouts.app')\n@section('content')\n<x-alert type=\"x\" />\n@endsection\n@push('scripts')<script></script>@endpush\n").unwrap();
        std::fs::write(views.join("components/alert.blade.php"), "@props(['type' => 'info', 'message', 'dismissible' => false])\n<div>{{ $message }}</div>\n").unwrap();
        root
    }

    #[test]
    fn sections_stacks_and_props() {
        let root = project();
        let laravel = Laravel::new(&root);
        let home = std::fs::read_to_string(root.join("resources/views/home.blade.php")).unwrap();
        let to_yield = laravel.slot_targets(&home, "section", "content");
        assert_eq!(to_yield.len(), 1);
        assert!(to_yield[0].path.ends_with("layouts/app.blade.php"));
        let to_sections = laravel.slot_targets("", "yield", "content");
        assert!(to_sections[0].path.ends_with("home.blade.php"));
        assert_eq!(laravel.slot_targets("", "stack", "scripts").len(), 1);

        let src = "@section('";
        let items = laravel.slot_completions(src, src.len()).unwrap();
        assert!(items.iter().any(|i| i.label == "title") && items.iter().any(|i| i.label == "content"));

        let props = props("@props(['type' => 'info', 'message'])");
        assert_eq!(props.iter().map(|(n, d, _)| (n.as_str(), d.clone())).collect::<Vec<_>>(), [("type", Some("'info'".into())), ("message", None)]);

        let at = home.find("type=").unwrap() + 1;
        let target = laravel.component_attribute_target(&home, at);
        assert!(target[0].path.ends_with("components/alert.blade.php"));
        let tag = "<x-alert ";
        let attrs = laravel.component_attribute_completions(tag, tag.len()).unwrap();
        assert!(attrs.iter().any(|i| i.label == ":dismissible" && i.insert == ":dismissible=\"\""));

        let view = root.join("resources/views/components/alert.blade.php");
        let source = "@props(['type'])\n{{ $ty";
        let vars = prop_variable_completions(&view, source, source.len()).unwrap();
        assert_eq!(vars.iter().map(|i| i.label.as_str()).collect::<Vec<_>>(), ["$type"]);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn extracts_components_and_views() {
        let selection = "    <div class=\"card\">\n        <h2>{{ $user->name }}</h2>\n        @foreach($posts as $post) {{ $post->title }} @endforeach\n        @error('x') {{ $message }} @enderror\n    </div>";
        let (file, replacement) = extract(selection, "user-card", true);
        assert_eq!(replacement, "<x-user-card :user=\"$user\" :posts=\"$posts\" />");
        assert!(file.starts_with("@props(['user', 'posts'])\n\n<div class=\"card\">\n    <h2>"), "{file}");
        let (file, replacement) = extract(selection, "partials.card", false);
        assert_eq!(replacement, "@include('partials.card')");
        assert!(file.starts_with("<div class=\"card\">"));
        assert!(extracted_path(Path::new("/v"), "cards.user", true).ends_with("components/cards/user.blade.php"));
    }
}
