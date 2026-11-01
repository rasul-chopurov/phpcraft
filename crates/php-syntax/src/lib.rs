//! Parsing and syntax highlighting: PHP via tree-sitter-php, plus Blade, HTML, JS/TS, CSS,
//! JSON, YAML and `.env` for the files a Laravel project contains.

mod blade;
mod simple;
mod ranges;

pub use ranges::{Fold, fold_ranges, node_ranges};

use std::ops::Range;
use std::path::Path;
use std::sync::OnceLock;

use streaming_iterator::StreamingIterator;
use tree_sitter::{Language, Parser, Query, QueryCursor, Tree};

/// Highlight category, roughly matching PhpStorm's Darcula token groups.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Highlight {
    Keyword,
    String,
    Comment,
    Number,
    Function,
    Method,
    Type,
    Variable,
    Property,
    Constant,
    Operator,
    Tag,
    Attribute,
    Namespace,
    /// Log levels.
    Error,
    Warning,
    Info,
}

/// A highlighted byte range in the source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub range: Range<usize>,
    pub highlight: Highlight,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Lang {
    Php,
    Blade,
    Html,
    JavaScript,
    TypeScript,
    Tsx,
    Css,
    Json,
    Yaml,
    Env,
    Xml,
    Log,
    Markdown,
    Sql,
    Ini,
    Shell,
    Dockerfile,
    Ignore,
    Plain,
}

impl Lang {
    pub fn from_path(path: &Path) -> Self {
        let name = path.file_name().map(|n| n.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
        if name.ends_with(".blade.php") {
            return Self::Blade;
        }
        if name == ".env" || name.starts_with(".env.") {
            return Self::Env;
        }
        if name == "dockerfile" || name.starts_with("dockerfile.") || name.ends_with(".dockerfile") {
            return Self::Dockerfile;
        }
        if name.ends_with("ignore") && name.starts_with('.') {
            return Self::Ignore;
        }
        if name == ".editorconfig" || name == ".npmrc" {
            return Self::Ini;
        }
        match path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).as_deref() {
            Some("php" | "phtml" | "phpt") => Self::Php,
            Some("html" | "htm" | "vue" | "svelte") => Self::Html,
            Some("js" | "mjs" | "cjs" | "jsx") => Self::JavaScript,
            Some("ts" | "mts" | "cts") => Self::TypeScript,
            Some("tsx") => Self::Tsx,
            Some("css" | "scss" | "sass" | "less" | "pcss") => Self::Css,
            Some("json" | "lock" | "jsonc" | "webmanifest" | "map") => Self::Json,
            Some("yml" | "yaml" | "neon") => Self::Yaml,
            Some("xml" | "svg" | "xsd" | "dist" | "xliff") => Self::Xml,
            Some("log") => Self::Log,
            Some("md" | "markdown") => Self::Markdown,
            Some("sql") => Self::Sql,
            Some("ini" | "toml" | "conf" | "cfg" | "properties") => Self::Ini,
            Some("sh" | "bash" | "zsh") => Self::Shell,
            _ => Self::Plain,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Php => "PHP",
            Self::Blade => "Blade",
            Self::Html => "HTML",
            Self::JavaScript => "JavaScript",
            Self::TypeScript => "TypeScript",
            Self::Tsx => "TSX",
            Self::Css => "CSS",
            Self::Json => "JSON",
            Self::Yaml => "YAML",
            Self::Env => ".env",
            Self::Xml => "XML",
            Self::Log => "Log",
            Self::Markdown => "Markdown",
            Self::Sql => "SQL",
            Self::Ini => "INI",
            Self::Shell => "Shell",
            Self::Dockerfile => "Dockerfile",
            Self::Ignore => "Ignore",
            Self::Plain => "Text",
        }
    }
}

pub fn language() -> Language {
    tree_sitter_php::LANGUAGE_PHP.into()
}

pub fn parse(source: &str) -> Option<Tree> {
    // One parser per thread: creating a parser per file shows up when indexing vendor/.
    thread_local! {
        static PARSER: std::cell::RefCell<Option<Parser>> = const { std::cell::RefCell::new(None) };
    }
    PARSER.with(|cell| {
        let mut slot = cell.borrow_mut();
        if slot.is_none() {
            let mut parser = Parser::new();
            parser.set_language(&language()).ok()?;
            *slot = Some(parser);
        }
        slot.as_mut()?.parse(source, None)
    })
}

fn parse_with(language: &Language, source: &str) -> Option<Tree> {
    let mut parser = Parser::new();
    parser.set_language(language).ok()?;
    parser.parse(source, None)
}

/// Grammars with highlight queries; `PhpOnly` parses PHP without `<?php` (Blade expressions).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Grammar {
    Php,
    PhpOnly,
    Html,
    JavaScript,
    TypeScript,
    Tsx,
    Css,
    Json,
    Yaml,
}

const GRAMMARS: usize = 9;

struct Compiled {
    language: Language,
    query: Query,
    kinds: Vec<Option<Highlight>>,
    /// Neovim-style queries (YAML) let later patterns override earlier ones.
    last_wins: bool,
}

fn capture_kind(name: &str, grammar: Grammar) -> Option<Highlight> {
    let is_php = matches!(grammar, Grammar::Php | Grammar::PhpOnly);
    let base = name.split('.').next().unwrap_or(name);
    Some(match (base, name) {
        (_, "function.method") => Highlight::Method,
        (_, "string.special.key") => Highlight::Property,
        (_, "variable.builtin") if !is_php => Highlight::Keyword,
        ("variable", _) if !is_php => return None,
        ("keyword" | "boolean" | "charset" | "import" | "keyframes" | "media" | "namespace" | "supports", _)
            if !(is_php && base == "namespace") =>
        {
            Highlight::Keyword
        }
        ("string" | "escape" | "embedded", _) => Highlight::String,
        ("comment", _) => Highlight::Comment,
        ("number" | "float", _) => Highlight::Number,
        ("function" | "constructor", _) => Highlight::Function,
        ("type" | "label", _) => Highlight::Type,
        ("variable", _) => Highlight::Variable,
        ("property", _) => Highlight::Property,
        ("constant", _) => Highlight::Constant,
        ("operator", _) => Highlight::Operator,
        ("tag", _) => Highlight::Tag,
        ("attribute", _) => Highlight::Attribute,
        ("module", _) => Highlight::Namespace,
        _ => return None,
    })
}

fn compiled(grammar: Grammar) -> Option<&'static Compiled> {
    static CACHE: [OnceLock<Option<Compiled>>; GRAMMARS] = [const { OnceLock::new() }; GRAMMARS];
    CACHE[grammar as usize]
        .get_or_init(|| {
            let (language, source): (Language, String) = match grammar {
                Grammar::Php => (tree_sitter_php::LANGUAGE_PHP.into(), tree_sitter_php::HIGHLIGHTS_QUERY.into()),
                Grammar::PhpOnly => (tree_sitter_php::LANGUAGE_PHP_ONLY.into(), tree_sitter_php::HIGHLIGHTS_QUERY.into()),
                Grammar::Html => (tree_sitter_html::LANGUAGE.into(), tree_sitter_html::HIGHLIGHTS_QUERY.into()),
                Grammar::JavaScript => (tree_sitter_javascript::LANGUAGE.into(), tree_sitter_javascript::HIGHLIGHT_QUERY.into()),
                // TypeScript's query only adds to the JavaScript one.
                Grammar::TypeScript => (
                    tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
                    format!("{}\n{}", tree_sitter_typescript::HIGHLIGHTS_QUERY, tree_sitter_javascript::HIGHLIGHT_QUERY),
                ),
                Grammar::Tsx => (
                    tree_sitter_typescript::LANGUAGE_TSX.into(),
                    format!(
                        "{}\n{}\n{}",
                        tree_sitter_typescript::HIGHLIGHTS_QUERY,
                        tree_sitter_javascript::HIGHLIGHT_QUERY,
                        tree_sitter_javascript::JSX_HIGHLIGHT_QUERY
                    ),
                ),
                Grammar::Css => (tree_sitter_css::LANGUAGE.into(), tree_sitter_css::HIGHLIGHTS_QUERY.into()),
                Grammar::Json => (tree_sitter_json::LANGUAGE.into(), tree_sitter_json::HIGHLIGHTS_QUERY.into()),
                Grammar::Yaml => (tree_sitter_yaml::LANGUAGE.into(), tree_sitter_yaml::HIGHLIGHTS_QUERY.into()),
            };
            let query = Query::new(&language, &source).ok()?;
            let kinds = query.capture_names().iter().map(|n| capture_kind(n, grammar)).collect();
            Some(Compiled { language, query, kinds, last_wins: grammar == Grammar::Yaml })
        })
        .as_ref()
}

/// Non-overlapping spans sorted by start. Earlier query patterns win where captures overlap,
/// following the precedence order of the highlights files.
fn highlight_grammar(grammar: Grammar, source: &str) -> Vec<Span> {
    let Some(compiled) = compiled(grammar) else { return Vec::new() };
    let Some(tree) = parse_with(&compiled.language, source) else { return Vec::new() };
    spans_from_tree(compiled, &tree, source)
}

fn spans_from_tree(compiled: &Compiled, tree: &Tree, source: &str) -> Vec<Span> {
    let mut cursor = QueryCursor::new();
    let mut raw: Vec<(Range<usize>, usize, Highlight)> = Vec::new();
    let mut captures = cursor.captures(&compiled.query, tree.root_node(), source.as_bytes());
    while let Some((m, index)) = captures.next() {
        let capture = m.captures[*index];
        if let Some(kind) = compiled.kinds[capture.index as usize] {
            let priority = if compiled.last_wins { usize::MAX - m.pattern_index } else { m.pattern_index };
            raw.push((capture.node.byte_range(), priority, kind));
        }
    }
    raw.sort_by_key(|(range, priority, _)| (range.start, *priority));
    let mut spans: Vec<Span> = Vec::with_capacity(raw.len());
    let mut covered_until = 0;
    for (range, _, highlight) in raw {
        let start = range.start.max(covered_until);
        if start >= range.end {
            continue;
        }
        spans.push(Span { range: start..range.end, highlight });
        covered_until = range.end;
    }
    spans
}

/// Shifts spans computed on a fragment to its position in the document.
fn offset(spans: Vec<Span>, by: usize) -> Vec<Span> {
    spans
        .into_iter()
        .map(|s| Span { range: s.range.start + by..s.range.end + by, highlight: s.highlight })
        .collect()
}

/// Lays `top` over `base`: base spans are cut where top spans are, then both are merged.
fn overlay(base: Vec<Span>, top: Vec<Span>) -> Vec<Span> {
    if top.is_empty() {
        return base;
    }
    let mut out = Vec::with_capacity(base.len() + top.len());
    let mut t = 0;
    for span in base {
        let mut start = span.range.start;
        while start < span.range.end {
            while t < top.len() && top[t].range.end <= start {
                t += 1;
            }
            match top.get(t) {
                Some(cover) if cover.range.start < span.range.end => {
                    if cover.range.start > start {
                        out.push(Span { range: start..cover.range.start, highlight: span.highlight });
                    }
                    start = cover.range.end;
                }
                _ => {
                    out.push(Span { range: start..span.range.end, highlight: span.highlight });
                    break;
                }
            }
        }
    }
    out.extend(top);
    out.sort_by_key(|s| s.range.start);
    out
}

/// PHP highlighting for a code fragment without `<?php` (Blade expressions, directive args).
pub(crate) fn highlight_php_fragment(source: &str) -> Vec<Span> {
    highlight_grammar(Grammar::PhpOnly, source)
}

pub(crate) fn highlight_html(source: &str) -> Vec<Span> {
    let Some(compiled) = compiled(Grammar::Html) else { return Vec::new() };
    let Some(tree) = parse_with(&compiled.language, source) else { return Vec::new() };
    let mut spans = spans_from_tree(compiled, &tree, source);
    // <script> and <style> contents.
    let mut injected = Vec::new();
    let mut stack = vec![tree.root_node()];
    while let Some(node) = stack.pop() {
        let grammar = match node.kind() {
            "script_element" => Some(Grammar::JavaScript),
            "style_element" => Some(Grammar::Css),
            _ => None,
        };
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            match grammar {
                Some(grammar) if child.kind() == "raw_text" => {
                    let range = child.byte_range();
                    injected.extend(offset(highlight_grammar(grammar, &source[range.clone()]), range.start));
                }
                _ => stack.push(child),
            }
        }
    }
    injected.sort_by_key(|s| s.range.start);
    spans = overlay(spans, injected);
    spans
}

fn highlight_env(source: &str) -> Vec<Span> {
    let mut spans = Vec::new();
    let mut at = 0;
    for line in source.split_inclusive('\n') {
        let content = line.trim_end_matches(['\n', '\r']);
        let indent = content.len() - content.trim_start().len();
        if content.trim_start().starts_with('#') {
            spans.push(Span { range: at + indent..at + content.len(), highlight: Highlight::Comment });
        } else if let Some(eq) = content.find('=') {
            spans.push(Span { range: at + indent..at + eq, highlight: Highlight::Property });
            if eq + 1 < content.len() {
                spans.push(Span { range: at + eq + 1..at + content.len(), highlight: Highlight::String });
            }
        }
        at += line.len();
    }
    spans
}

/// Highlights `source` as `lang`.
pub fn highlight_lang(lang: Lang, source: &str) -> Vec<Span> {
    match lang {
        Lang::Php => highlight(source),
        Lang::Blade => blade::highlight(source),
        Lang::Html => highlight_html(source),
        Lang::JavaScript => highlight_grammar(Grammar::JavaScript, source),
        Lang::TypeScript => highlight_grammar(Grammar::TypeScript, source),
        Lang::Tsx => highlight_grammar(Grammar::Tsx, source),
        Lang::Css => highlight_grammar(Grammar::Css, source),
        Lang::Json => highlight_grammar(Grammar::Json, source),
        Lang::Yaml => highlight_grammar(Grammar::Yaml, source),
        Lang::Env => highlight_env(source),
        Lang::Xml => highlight_html(source),
        Lang::Log => simple::log(source),
        Lang::Markdown => simple::markdown(source),
        Lang::Sql => simple::sql(source),
        Lang::Ini => simple::ini(source),
        Lang::Shell => simple::shell(source),
        Lang::Dockerfile => simple::dockerfile(source),
        Lang::Ignore => simple::ignore_file(source),
        Lang::Plain => Vec::new(),
    }
}

/// Highlights a file, choosing the language from its path.
pub fn highlight_path(path: &Path, source: &str) -> Vec<Span> {
    highlight_lang(Lang::from_path(path), source)
}

/// PHP file highlighting, including inline HTML outside `<?php ?>`.
pub fn highlight(source: &str) -> Vec<Span> {
    let Some(tree) = parse(source) else { return Vec::new() };
    highlight_tree(&tree, source)
}

pub fn highlight_tree(tree: &Tree, source: &str) -> Vec<Span> {
    let Some(compiled) = compiled(Grammar::Php) else { return Vec::new() };
    let spans = spans_from_tree(compiled, tree, source);
    let mut html = Vec::new();
    let mut cursor = tree.root_node().walk();
    for node in tree.root_node().children(&mut cursor) {
        if node.kind() == "text" {
            let range = node.byte_range();
            html.extend(offset(highlight_html(&source[range.clone()]), range.start));
        }
    }
    overlay(spans, html)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kind_in(spans: &[Span], source: &str, needle: &str) -> Option<Highlight> {
        let start = source.find(needle).unwrap();
        spans.iter().find(|s| s.range.start <= start && start < s.range.end).map(|s| s.highlight)
    }

    fn kind_of(source: &str, needle: &str) -> Option<Highlight> {
        kind_in(&highlight(source), source, needle)
    }

    fn assert_sorted(spans: &[Span]) {
        for pair in spans.windows(2) {
            assert!(pair[0].range.end <= pair[1].range.start, "{pair:?}");
        }
    }

    #[test]
    fn highlights_basic_php() {
        let src = "<?php\n// hi\nfunction foo($bar) { return \"x\" . 42; }\n";
        assert_eq!(kind_of(src, "// hi"), Some(Highlight::Comment));
        assert_eq!(kind_of(src, "function"), Some(Highlight::Keyword));
        assert_eq!(kind_of(src, "foo"), Some(Highlight::Function));
        assert_eq!(kind_of(src, "$bar"), Some(Highlight::Variable));
        assert_eq!(kind_of(src, "\"x\""), Some(Highlight::String));
        assert_eq!(kind_of(src, "42"), Some(Highlight::Number));
    }

    #[test]
    fn spans_do_not_overlap() {
        let src = "<?php\nnamespace App;\nuse Foo\\Bar;\nclass A extends Bar { public int $x = 1; }\n";
        assert_sorted(&highlight(src));
    }

    #[test]
    fn inline_html_in_php() {
        let src = "<div class=\"x\"><?php echo $a; ?></div>";
        let spans = highlight(src);
        assert_sorted(&spans);
        assert_eq!(kind_in(&spans, src, "div"), Some(Highlight::Tag));
        assert_eq!(kind_in(&spans, src, "class"), Some(Highlight::Attribute));
        assert_eq!(kind_in(&spans, src, "$a"), Some(Highlight::Variable));
    }

    #[test]
    fn other_languages() {
        let html = "<script>const a = 1;</script><style>.x { color: red; }</style>";
        let spans = highlight_lang(Lang::Html, html);
        assert_sorted(&spans);
        assert_eq!(kind_in(&spans, html, "script"), Some(Highlight::Tag));
        assert_eq!(kind_in(&spans, html, "const"), Some(Highlight::Keyword));
        assert_eq!(kind_in(&spans, html, "color"), Some(Highlight::Property));

        let json = "{\"name\": \"x\", \"n\": 1, \"ok\": true}";
        let spans = highlight_lang(Lang::Json, json);
        assert_eq!(kind_in(&spans, json, "\"name\""), Some(Highlight::Property));
        assert_eq!(kind_in(&spans, json, "1"), Some(Highlight::Number));

        let ts = "interface A { x: number }\nconst f = (a: string) => a;";
        assert!(!highlight_lang(Lang::TypeScript, ts).is_empty());
        let yaml = "name: app\nport: 80\n";
        assert_eq!(kind_in(&highlight_lang(Lang::Yaml, yaml), yaml, "name"), Some(Highlight::Property));

        let env = "# c\nAPP_NAME=Laravel\n";
        let spans = highlight_lang(Lang::Env, env);
        assert_eq!(kind_in(&spans, env, "# c"), Some(Highlight::Comment));
        assert_eq!(kind_in(&spans, env, "APP_NAME"), Some(Highlight::Property));
        assert_eq!(kind_in(&spans, env, "Laravel"), Some(Highlight::String));
        assert_eq!(Lang::from_path(Path::new("resources/views/a.blade.php")), Lang::Blade);
        assert_eq!(Lang::from_path(Path::new(".env.example")), Lang::Env);
        assert_eq!(Lang::from_path(Path::new("storage/logs/laravel.log")), Lang::Log);
        assert_eq!(Lang::from_path(Path::new("phpunit.xml.dist")), Lang::Xml);
        assert_eq!(Lang::from_path(Path::new("Dockerfile")), Lang::Dockerfile);
        assert_eq!(Lang::from_path(Path::new(".gitignore")), Lang::Ignore);
    }
}
