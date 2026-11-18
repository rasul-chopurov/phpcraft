//! JavaScript / TypeScript intelligence: declarations, imports and their resolution across
//! modules, for `.js` / `.ts` / `.jsx` / `.tsx` files and the `<script>` blocks of Vue and
//! Svelte components. Enough for go to declaration, find usages, rename and completion in
//! Inertia and Vue projects without a language server.

use std::collections::HashMap;
use std::ops::Range;
use std::path::{Component, Path, PathBuf};

use tree_sitter::{Language, Node, Parser, Tree};

/// What a declaration is, for completion icons and detail text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Function,
    Class,
    Const,
    Interface,
    Type,
    Enum,
    /// `const Card = defineComponent({…})` and friends.
    Component,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Function => "function",
            Self::Class => "class",
            Self::Const => "const",
            Self::Interface => "interface",
            Self::Type => "type",
            Self::Enum => "enum",
            Self::Component => "component",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Decl {
    pub name: String,
    pub kind: Kind,
    /// Byte range of the name.
    pub range: Range<usize>,
    pub exported: bool,
}

#[derive(Debug, Clone)]
pub struct Import {
    /// Name used in this file (`import UserCard from './UserCard.vue'` -> `UserCard`).
    pub local: String,
    /// Name in the source module: `"default"`, `"*"` or the exported name.
    pub imported: String,
    pub source: String,
    pub local_range: Range<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Location {
    pub path: PathBuf,
    pub range: Range<usize>,
}

/// A declaration found from a reference, in the same file or through an import.
#[derive(Debug, Clone)]
pub struct Symbol {
    pub name: String,
    pub declaration: Location,
}

#[derive(Debug)]
pub struct Document {
    pub path: PathBuf,
    /// The source that was parsed: for Vue and Svelte components everything outside
    /// `<script>` is blanked out, byte for byte, so ranges still point into the original text.
    pub text: String,
    tree: Tree,
    decls: Vec<Decl>,
    imports: Vec<Import>,
    /// `export { a as b }` (exported, local).
    aliases: Vec<(String, String)>,
    /// `export { a as b } from './m'` (exported, name in the module, module).
    reexports: Vec<(String, String, String)>,
    /// `export * from './m'`.
    star_exports: Vec<String>,
    default_export: Option<String>,
}

pub fn is_js(path: &Path) -> bool {
    matches!(
        extension(path).as_deref(),
        Some("js" | "mjs" | "cjs" | "jsx" | "ts" | "tsx" | "vue" | "svelte")
    )
}

pub fn parse(path: &Path, source: &str) -> Option<Document> {
    let language = language_for(path, source)?;
    let text = masked(source);
    let mut parser = Parser::new();
    parser.set_language(&language).ok()?;
    let tree = parser.parse(&text, None)?;
    let mut collected = Collected::default();
    {
        let mut cursor = tree.root_node().walk();
        for child in tree.root_node().children(&mut cursor) {
            collect(&child, &text, false, &mut collected);
        }
    }
    Some(Document {
        path: path.to_path_buf(),
        text,
        tree,
        decls: collected.decls,
        imports: collected.imports,
        aliases: collected.aliases,
        reexports: collected.reexports,
        star_exports: collected.star_exports,
        default_export: collected.default_export,
    })
}

impl Document {
    pub fn decls(&self) -> &[Decl] {
        &self.decls
    }

    pub fn imports(&self) -> &[Import] {
        &self.imports
    }

    /// Identifier under `offset` (byte offset), e.g. for a caret. A caret sits between characters,
    /// so both the byte at the caret and the one before it are probed.
    pub fn identifier_at(&self, offset: usize) -> Option<(String, Range<usize>)> {
        for probe in [offset, offset.saturating_sub(1)] {
            let probe = probe.min(self.text.len());
            let Some(mut node) = self.tree.root_node().descendant_for_byte_range(probe, probe) else {
                continue;
            };
            for _ in 0..4 {
                if is_identifier(node.kind()) {
                    let range = node.byte_range();
                    let name = self.text.get(range.clone())?;
                    return Some((name.to_string(), range));
                }
                let Some(parent) = node.parent() else { break };
                node = parent;
            }
        }
        None
    }

    fn decl(&self, name: &str) -> Option<&Decl> {
        self.decls.iter().find(|d| d.name == name)
    }
}

/// Goes to the declaration of the identifier at `offset`: a local declaration, a default or
/// named import (following `@/` aliases and re-exports) or a namespace import.
pub fn definition(
    doc: &Document,
    offset: usize,
    root: &Path,
    load: &dyn Fn(&Path) -> Option<String>,
) -> Vec<Location> {
    resolve(doc, offset, root, load)
        .map(|symbol| vec![symbol.declaration])
        .unwrap_or_default()
}

/// The declaration a reference points at, including through import chains.
pub fn resolve(
    doc: &Document,
    offset: usize,
    root: &Path,
    load: &dyn Fn(&Path) -> Option<String>,
) -> Option<Symbol> {
    let (name, _) = doc.identifier_at(offset)?;
    if let Some(decl) = doc.decl(&name) {
        return Some(Symbol {
            name: decl.name.clone(),
            declaration: Location { path: doc.path.clone(), range: decl.range.clone() },
        });
    }
    let import = doc.imports.iter().find(|i| i.local == name)?;
    let mut seen = Vec::new();
    follow_import(&doc.path, &import.source, &import.imported, root, load, &mut seen)
}

fn follow_import(
    from: &Path,
    source: &str,
    imported: &str,
    root: &Path,
    load: &dyn Fn(&Path) -> Option<String>,
    seen: &mut Vec<PathBuf>,
) -> Option<Symbol> {
    if seen.len() > 4 {
        return None;
    }
    let target = resolve_import(from, source, root, load)?;
    if seen.contains(&target) {
        return None;
    }
    seen.push(target.clone());
    let text = load(&target)?;
    let module = parse(&target, &text)?;
    find_export(&module, imported, root, load, seen)
}

/// Declaration of `name` exported by `module`, following re-exports and `export *`.
fn find_export(
    module: &Document,
    name: &str,
    root: &Path,
    load: &dyn Fn(&Path) -> Option<String>,
    seen: &mut Vec<PathBuf>,
) -> Option<Symbol> {
    let here = |decl_name: &str, range: Range<usize>| {
        Some(Symbol {
            name: decl_name.to_string(),
            declaration: Location { path: module.path.clone(), range },
        })
    };
    if name == "*" {
        return here("*", 0..0);
    }
    if name == "default" {
        return match &module.default_export {
            Some(local) => module
                .decl(local)
                .and_then(|decl| here(&decl.name, decl.range.clone())),
            // An anonymous default (`export default {…}`): opening the module is the best we can do.
            None => here("*", 0..0),
        };
    }
    if let Some(decl) = module.decl(name) {
        return here(&decl.name, decl.range.clone());
    }
    if let Some((_, local)) = module.aliases.iter().find(|(exported, _)| exported == name)
        && let Some(decl) = module.decl(local)
    {
        return here(&decl.name, decl.range.clone());
    }
    if let Some((_, target, source)) = module.reexports.iter().find(|(exported, _, _)| exported == name)
        && let Some(found) = follow_import(&module.path, source, target, root, load, seen)
    {
        return Some(found);
    }
    for source in &module.star_exports {
        if let Some(found) = follow_import(&module.path, source, name, root, load, seen) {
            return Some(found);
        }
    }
    None
}

/// Usages of `symbol` in one file, for find usages and rename.
pub fn usages_in(
    doc: &Document,
    symbol: &Symbol,
    root: &Path,
    load: &dyn Fn(&Path) -> Option<String>,
) -> Vec<Range<usize>> {
    let names: Vec<&str> = if doc.path == symbol.declaration.path {
        vec![symbol.name.as_str()]
    } else {
        doc.imports
            .iter()
            .filter(|import| import_names_symbol(doc, import, symbol, root, load))
            .map(|import| import.local.as_str())
            .collect()
    };
    if names.is_empty() {
        return Vec::new();
    }
    identifiers(&doc.text, &doc.tree)
        .into_iter()
        .filter(|(name, _)| names.contains(&name.as_str()))
        .map(|(_, range)| range)
        .collect()
}

/// Whether `import` brings `symbol` into `doc` (through `@/` aliases and extensions).
fn import_names_symbol(
    doc: &Document,
    import: &Import,
    symbol: &Symbol,
    root: &Path,
    load: &dyn Fn(&Path) -> Option<String>,
) -> bool {
    let Some(target) = resolve_import(&doc.path, &import.source, root, load) else { return false };
    if target != symbol.declaration.path {
        return false;
    }
    match import.imported.as_str() {
        "*" => false,
        // A default import brings the module's default export, not any symbol it declares.
        "default" => load(&target)
            .and_then(|text| parse(&target, &text))
            .and_then(|module| module.default_export)
            .is_some_and(|name| name == symbol.name),
        name => name == symbol.name,
    }
}

/// Usages across the project: `files` is the candidate list, `open` supplies unsaved editor text.
pub fn usages(
    doc: &Document,
    offset: usize,
    root: &Path,
    files: &[PathBuf],
    open: &HashMap<PathBuf, String>,
) -> Vec<Location> {
    let load = |path: &Path| -> Option<String> {
        open.get(path)
            .cloned()
            .or_else(|| std::fs::read_to_string(path).ok())
    };
    let Some(symbol) = resolve(doc, offset, root, &load) else { return Vec::new() };
    let mut found = Vec::new();
    for path in files {
        let parsed = if *path == doc.path {
            parse(path, &doc.text)
        } else {
            load(path).and_then(|text| parse(path, &text))
        };
        let Some(parsed) = parsed else { continue };
        for range in usages_in(&parsed, &symbol, root, &load) {
            let location = Location { path: path.clone(), range };
            if !found.contains(&location) {
                found.push(location);
            }
        }
    }
    found.sort_by(|a, b| a.path.cmp(&b.path).then(a.range.start.cmp(&b.range.start)));
    found
}

/// Completion items as `(label, detail, kind)`.
pub fn complete(
    doc: &Document,
    offset: usize,
    root: &Path,
    load: &dyn Fn(&Path) -> Option<String>,
) -> Vec<(String, String, Kind)> {
    let (prefix, _) = word_at(&doc.text, offset);
    let mut out: Vec<(String, String, Kind)> = Vec::new();
    let mut add = |label: &str, detail: String, kind: Kind| {
        if label.starts_with(prefix.as_str())
            && !out.iter().any(|(l, _, k)| l == label && *k == kind)
        {
            out.push((label.to_string(), detail, kind));
        }
    };
    for decl in &doc.decls {
        add(&decl.name, kind_detail(decl.kind, None), decl.kind);
    }
    for import in &doc.imports {
        add(&import.local, format!("from '{}'", import.source), Kind::Const);
        let Some(target) = resolve_import(&doc.path, &import.source, root, load) else { continue };
        let Some(text) = load(&target) else { continue };
        let Some(module) = parse(&target, &text) else { continue };
        let detail = format!("from '{}'", import.source);
        for decl in &module.decls {
            add(&decl.name, detail.clone(), decl.kind);
        }
        for exported in exported_names(&module) {
            add(&exported, detail.clone(), Kind::Const);
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

fn exported_names(module: &Document) -> Vec<String> {
    let mut names: Vec<String> = module
        .decls
        .iter()
        .filter(|decl| decl.exported)
        .map(|decl| decl.name.clone())
        .collect();
    names.extend(module.aliases.iter().map(|(exported, _)| exported.clone()));
    names.extend(module.reexports.iter().map(|(exported, _, _)| exported.clone()));
    names.sort();
    names.dedup();
    names
}

fn kind_detail(kind: Kind, source: Option<&str>) -> String {
    match source {
        Some(source) => format!("{} from '{}'", kind.label(), source),
        None => kind.label().to_string(),
    }
}

/// Byte range of the partial word before `offset` and its text.
pub fn word_at(text: &str, offset: usize) -> (String, Range<usize>) {
    let offset = offset.min(text.len());
    let start = text[..offset]
        .char_indices()
        .rev()
        .take_while(|(_, c)| is_word_char(*c))
        .last()
        .map_or(offset, |(i, _)| i);
    (text[start..offset].to_string(), start..offset)
}

/// Whether `offset` is inside a script block: always true for plain modules, and only inside
/// `<script>` for Vue and Svelte components (templates are markup, not JS).
pub fn in_script(source: &str, offset: usize) -> bool {
    let ranges = script_ranges(source);
    ranges.is_empty() || ranges.iter().any(|range| range.contains(&offset))
}

/// Paths a module specifier may point at, in probe order.
pub fn module_candidates(from: &Path, source: &str, root: &Path) -> Vec<PathBuf> {
    let base = if let Some(rest) = strip_alias(source) {
        root.join("resources").join("js").join(rest)
    } else if source.starts_with('.') {
        from.parent().unwrap_or(Path::new("")).join(source)
    } else {
        return Vec::new();
    };
    let base = normalize(&base);
    const EXTENSIONS: [&str; 8] = ["ts", "tsx", "js", "jsx", "mjs", "cjs", "vue", "svelte"];
    let mut out = Vec::new();
    if base.extension().is_some() {
        out.push(base.clone());
    }
    for extension in EXTENSIONS {
        out.push(base.with_extension(extension));
    }
    for extension in EXTENSIONS {
        out.push(base.join(format!("index.{extension}")));
    }
    out
}

/// The file a module specifier points at, when it exists in `load`.
pub fn resolve_import(
    from: &Path,
    source: &str,
    root: &Path,
    load: &dyn Fn(&Path) -> Option<String>,
) -> Option<PathBuf> {
    module_candidates(from, source, root)
        .into_iter()
        .find(|candidate| load(candidate).is_some())
}

fn strip_alias(source: &str) -> Option<&str> {
    // Laravel / Inertia convention: `@/Pages/User` lives in `resources/js`.
    source.strip_prefix("@/").or_else(|| source.strip_prefix("~/"))
}

fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn extension(path: &Path) -> Option<String> {
    Some(path.extension()?.to_string_lossy().to_ascii_lowercase())
}

fn language_for(path: &Path, source: &str) -> Option<Language> {
    let extension = extension(path)?;
    let typescript = source.contains("lang=\"ts\"") || source.contains("lang='ts'");
    Some(match extension.as_str() {
        "ts" => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        "tsx" => tree_sitter_typescript::LANGUAGE_TSX.into(),
        "vue" | "svelte" if typescript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        "js" | "mjs" | "cjs" | "jsx" | "vue" | "svelte" => {
            tree_sitter_javascript::LANGUAGE.into()
        }
        _ => return None,
    })
}

/// Blanks everything outside `<script>` blocks, keeping byte offsets and line breaks, so a
/// component parses as the module its script block contains.
fn masked(source: &str) -> String {
    let ranges = script_ranges(source);
    if ranges.is_empty() {
        return source.to_string();
    }
    let mut out = String::with_capacity(source.len());
    let mut index = 0;
    for (offset, ch) in source.char_indices() {
        while index < ranges.len() && offset >= ranges[index].end {
            index += 1;
        }
        let keep = index < ranges.len() && ranges[index].contains(&offset);
        if keep {
            out.push(ch);
        } else if ch == '\n' {
            out.push('\n');
        } else {
            for _ in 0..ch.len_utf8() {
                out.push(' ');
            }
        }
    }
    out
}

#[derive(Default)]
struct Collected {
    decls: Vec<Decl>,
    imports: Vec<Import>,
    aliases: Vec<(String, String)>,
    reexports: Vec<(String, String, String)>,
    star_exports: Vec<String>,
    default_export: Option<String>,
}

fn collect(node: &Node, text: &str, exported: bool, out: &mut Collected) {
    match node.kind() {
        "export_statement" => collect_export(node, text, out),
        "import_statement" => collect_import(node, text, out),
        "function_declaration" | "generator_function_declaration" | "function_signature" => {
            if let Some(name) = node.child_by_field_name("name") {
                push(out, text, &name, Kind::Function, exported);
            }
        }
        "class_declaration" | "abstract_class_declaration" => {
            if let Some(name) = node.child_by_field_name("name") {
                push(out, text, &name, Kind::Class, exported);
            }
        }
        "interface_declaration" => {
            if let Some(name) = node.child_by_field_name("name") {
                push(out, text, &name, Kind::Interface, exported);
            }
        }
        "type_alias_declaration" => {
            if let Some(name) = node.child_by_field_name("name") {
                push(out, text, &name, Kind::Type, exported);
            }
        }
        "enum_declaration" => {
            if let Some(name) = node.child_by_field_name("name") {
                push(out, text, &name, Kind::Enum, exported);
            }
        }
        "lexical_declaration" | "variable_declaration" => collect_variables(node, text, exported, out),
        _ => {}
    }
}

fn collect_export(node: &Node, text: &str, out: &mut Collected) {
    let mut default = false;
    let mut source = None;
    let mut clause = None;
    let mut star = false;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "default" => default = true,
            "string" => source = Some(unquote(text, &child)),
            "export_clause" => clause = Some(child),
            "*" => star = true,
            _ => {}
        }
    }
    if let Some(clause) = clause {
        let mut cursor = clause.walk();
        for specifier in clause.children(&mut cursor) {
            if specifier.kind() != "export_specifier" {
                continue;
            }
            let Some(name) = specifier.child_by_field_name("name") else { continue };
            let local = text[name.byte_range()].to_string();
            let alias = specifier.child_by_field_name("alias");
            let exported = alias
                .as_ref()
                .map(|alias| text[alias.byte_range()].to_string())
                .unwrap_or_else(|| local.clone());
            match &source {
                Some(source) => out.reexports.push((exported, local, source.clone())),
                None => out.aliases.push((exported, local)),
            }
        }
        return;
    }
    if star {
        if let Some(source) = source {
            out.star_exports.push(source);
        }
        return;
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if is_identifier(child.kind()) && default {
            // `export default UserCard`
            out.default_export = Some(text[child.byte_range()].to_string());
        } else if matches!(
            child.kind(),
            "function_declaration"
                | "generator_function_declaration"
                | "class_declaration"
                | "abstract_class_declaration"
                | "interface_declaration"
                | "type_alias_declaration"
                | "enum_declaration"
                | "lexical_declaration"
                | "variable_declaration"
        ) {
            let before = out.decls.len();
            collect(&child, text, true, out);
            if default && let Some(decl) = out.decls.get(before) {
                out.default_export = Some(decl.name.clone());
            }
        }
    }
}

fn collect_import(node: &Node, text: &str, out: &mut Collected) {
    let mut cursor = node.walk();
    let Some(source) = node
        .children(&mut cursor)
        .find(|child| child.kind() == "string")
        .map(|child| unquote(text, &child))
    else {
        return;
    };
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() != "import_clause" {
            continue;
        }
        let mut cursor = child.walk();
        for part in child.children(&mut cursor) {
            match part.kind() {
                "identifier" => {
                    out.imports.push(Import {
                        local: text[part.byte_range()].to_string(),
                        imported: "default".into(),
                        source: source.clone(),
                        local_range: part.byte_range(),
                    });
                }
                "namespace_import" => {
                    let mut inner = part.walk();
                    if let Some(name) = part
                        .children(&mut inner)
                        .filter(|child| is_identifier(child.kind()))
                        .last()
                    {
                        out.imports.push(Import {
                            local: text[name.byte_range()].to_string(),
                            imported: "*".into(),
                            source: source.clone(),
                            local_range: name.byte_range(),
                        });
                    }
                }
                "named_imports" => {
                    let mut inner = part.walk();
                    for specifier in part.children(&mut inner) {
                        if specifier.kind() != "import_specifier" {
                            continue;
                        }
                        let Some(name) = specifier.child_by_field_name("name") else { continue };
                        let alias = specifier.child_by_field_name("alias");
                        let local = alias.unwrap_or(name);
                        out.imports.push(Import {
                            local: text[local.byte_range()].to_string(),
                            imported: text[name.byte_range()].to_string(),
                            source: source.clone(),
                            local_range: local.byte_range(),
                        });
                    }
                }
                _ => {}
            }
        }
    }
}

fn collect_variables(node: &Node, text: &str, exported: bool, out: &mut Collected) {
    let mut cursor = node.walk();
    for declarator in node.children(&mut cursor) {
        if declarator.kind() != "variable_declarator" {
            continue;
        }
        let Some(name) = declarator.child_by_field_name("name") else { continue };
        if !is_identifier(name.kind()) {
            continue;
        }
        let value = declarator.child_by_field_name("value");
        let value_kind = value.map(|value| value.kind());
        if let Some(value) = value
            && is_require(&value, text)
        {
            let mut inner = value.walk();
            let source = value
                .children(&mut inner)
                .find(|child| child.kind() == "arguments")
                .and_then(|arguments| arguments.children(&mut arguments.walk()).find(|c| c.kind() == "string"))
                .map(|string| unquote(text, &string));
            if let Some(source) = source {
                out.imports.push(Import {
                    local: text[name.byte_range()].to_string(),
                    imported: "*".into(),
                    source,
                    local_range: name.byte_range(),
                });
                continue;
            }
        }
        let kind = match value_kind {
            Some("arrow_function" | "function_expression" | "function") => Kind::Function,
            Some("call_expression") if value.is_some_and(|v| is_component_factory(&v, text)) => {
                Kind::Component
            }
            _ => Kind::Const,
        };
        push(out, text, &name, kind, exported);
    }
}

fn is_require(node: &Node, text: &str) -> bool {
    node.child_by_field_name("function")
        .is_some_and(|function| &text[function.byte_range()] == "require")
}

fn is_component_factory(node: &Node, text: &str) -> bool {
    let Some(function) = node.child_by_field_name("function") else { return false };
    let name = &text[function.byte_range()];
    matches!(
        name,
        "defineComponent" | "defineAsyncComponent" | "React.forwardRef" | "React.memo" | "observer"
    )
}

fn push(out: &mut Collected, text: &str, name: &Node, kind: Kind, exported: bool) {
    out.decls.push(Decl {
        name: text[name.byte_range()].to_string(),
        kind,
        range: name.byte_range(),
        exported,
    });
}

fn is_identifier(kind: &str) -> bool {
    matches!(
        kind,
        "identifier"
            | "type_identifier"
            | "property_identifier"
            | "shorthand_property_identifier"
            | "shorthand_property_identifier_pattern"
            | "jsx_identifier"
    )
}

fn identifiers(text: &str, tree: &Tree) -> Vec<(String, Range<usize>)> {
    let mut out = Vec::new();
    let mut stack = vec![tree.root_node()];
    while let Some(node) = stack.pop() {
        if node.kind() == "identifier" || node.kind() == "type_identifier" || node.kind() == "jsx_identifier" {
            out.push((text[node.byte_range()].to_string(), node.byte_range()));
        }
        let mut cursor = node.walk();
        let children: Vec<Node> = node.children(&mut cursor).collect();
        stack.extend(children);
    }
    out
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}

fn unquote(text: &str, node: &Node) -> String {
    let raw = &text[node.byte_range()];
    raw.trim_matches(|c| c == '\'' || c == '"' || c == '`').to_string()
}

/// Ranges of `<script>` bodies in a Vue or Svelte component.
pub fn script_ranges(source: &str) -> Vec<Range<usize>> {
    let lower = source.to_ascii_lowercase();
    let mut ranges = Vec::new();
    let mut at = 0;
    while let Some(found) = lower[at..].find("<script") {
        let open = at + found;
        let Some(gt) = lower[open..].find('>') else { break };
        let body = open + gt + 1;
        let Some(close) = lower[body..].find("</script") else { break };
        ranges.push(body..body + close);
        at = body + close + 8;
    }
    ranges
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROOT: &str = "/project";

    const FORMAT: &str = "export function format(value: number): string {\n  return String(value)\n}\n\nexport const currency = '$'\n";
    const CARD: &str = "export default class Card {}\n\nexport class Panel {}\n";
    const INDEX: &str = "export { format } from './format'\n";
    const HOME: &str = "import Card, { Panel as P } from '../Card'\nimport { format } from '@/lib/format'\n\nconst card = new Card()\nconst panel = new P()\nformat(1)\n";
    const BUTTON: &str = "<template>\n  <button>{{ label }}</button>\n</template>\n\n<script setup lang=\"ts\">\nimport { computed } from 'vue'\n\nexport interface Props {\n  label: string\n}\n\nconst label = computed(() => 'x')\nconst shown = computed(() => label)\n</script>\n";

    fn project() -> (PathBuf, HashMap<PathBuf, String>) {
        let root = PathBuf::from(ROOT);
        let mut files = HashMap::new();
        files.insert(root.join("resources/js/lib/format.ts"), FORMAT.to_string());
        files.insert(root.join("resources/js/lib/index.ts"), INDEX.to_string());
        files.insert(root.join("resources/js/Card.ts"), CARD.to_string());
        files.insert(root.join("resources/js/Pages/Home.ts"), HOME.to_string());
        files.insert(root.join("resources/js/Components/Button.vue"), BUTTON.to_string());
        (root, files)
    }

    fn at(source: &str, needle: &str, delta: usize) -> usize {
        source.find(needle).unwrap() + delta
    }

    fn name_offset(source: &str, decl: &str) -> usize {
        source.find(decl).unwrap()
    }

    #[test]
    fn default_import_resolves_to_the_class() {
        let (root, files) = project();
        let doc = parse(&PathBuf::from(ROOT).join("resources/js/Pages/Home.ts"), HOME).unwrap();
        let load = |path: &Path| files.get(path).cloned();
        let found = definition(&doc, at(HOME, "new Card", 5), &root, &load);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].path, root.join("resources/js/Card.ts"));
        assert_eq!(found[0].range.start, name_offset(CARD, "Card {}"));
    }

    #[test]
    fn aliased_named_import_resolves_to_the_declaration() {
        let (root, files) = project();
        let doc = parse(&PathBuf::from(ROOT).join("resources/js/Pages/Home.ts"), HOME).unwrap();
        let load = |path: &Path| files.get(path).cloned();
        let found = definition(&doc, at(HOME, "new P", 5), &root, &load);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].range.start, name_offset(CARD, "Panel"));
    }

    #[test]
    fn at_alias_resolves_into_resources_js() {
        let (root, files) = project();
        let doc = parse(&PathBuf::from(ROOT).join("resources/js/Pages/Home.ts"), HOME).unwrap();
        let load = |path: &Path| files.get(path).cloned();
        let found = definition(&doc, at(HOME, "format(1)", 0), &root, &load);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].path, root.join("resources/js/lib/format.ts"));
        assert_eq!(found[0].range.start, name_offset(FORMAT, "format"));
    }

    #[test]
    fn reexport_chains_resolve() {
        let (root, files) = project();
        let home = "import { format } from '@/lib'\nformat(1)\n";
        let doc = parse(&PathBuf::from(ROOT).join("resources/js/Pages/Reexport.ts"), home).unwrap();
        let load = |path: &Path| files.get(path).cloned();
        let found = definition(&doc, at(home, "format(1)", 0), &root, &load);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].path, root.join("resources/js/lib/format.ts"));
    }

    #[test]
    fn vue_script_block_is_parsed_with_original_offsets() {
        let (root, files) = project();
        let path = PathBuf::from(ROOT).join("resources/js/Components/Button.vue");
        let doc = parse(&path, BUTTON).unwrap();
        assert!(doc.decls().iter().any(|decl| decl.name == "Props"));
        assert!(doc.decls().iter().any(|decl| decl.name == "label"));
        let load = |path: &Path| files.get(path).cloned();
        let usage = usages_in(
            &doc,
            &Symbol { name: "label".into(), declaration: Location { path: path.clone(), range: at(BUTTON, "const label", 6)..at(BUTTON, "const label", 11) } },
            &root,
            &load,
        );
        assert_eq!(usage.len(), 2, "the template must be blanked out: {usage:?}");
        assert!(usage.iter().all(|range| &BUTTON[range.clone()] == "label"));
    }

    #[test]
    fn usages_follow_the_local_alias() {
        let (root, files) = project();
        let card_path = root.join("resources/js/Card.ts");
        let card = parse(&card_path, CARD).unwrap();
        let decl = card.decls().iter().find(|decl| decl.name == "Panel").unwrap().clone();
        let symbol = Symbol {
            name: decl.name.clone(),
            declaration: Location { path: card_path.clone(), range: decl.range.clone() },
        };
        let home = parse(&root.join("resources/js/Pages/Home.ts"), HOME).unwrap();
        let load = |path: &Path| files.get(path).cloned();
        let found = usages_in(&home, &symbol, &root, &load);
        assert_eq!(found.len(), 2, "the import alias and its use: {found:?}");
        assert!(found.iter().all(|range| &HOME[range.clone()] == "P"));
    }

    #[test]
    fn usages_skip_the_imported_name_side() {
        let (root, files) = project();
        let format_path = root.join("resources/js/lib/format.ts");
        let format = parse(&format_path, FORMAT).unwrap();
        let decl = format.decls().iter().find(|decl| decl.name == "format").unwrap().clone();
        let symbol = Symbol {
            name: decl.name.clone(),
            declaration: Location { path: format_path.clone(), range: decl.range.clone() },
        };
        let home = "import { format as fmt } from '@/lib/format'\nfmt(1)\n";
        let doc = parse(&root.join("resources/js/Pages/Typed.ts"), home).unwrap();
        let load = |path: &Path| files.get(path).cloned();
        let found = usages_in(&doc, &symbol, &root, &load);
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(found.iter().all(|range| &home[range.clone()] == "fmt"));
    }

    #[test]
    fn completion_offers_exports_of_imported_modules() {
        let (root, files) = project();
        let doc = parse(&PathBuf::from(ROOT).join("resources/js/Pages/Home.ts"), HOME).unwrap();
        let offset = HOME.len();
        let load = |path: &Path| files.get(path).cloned();
        let items = complete(&doc, offset, &root, &load);
        let labels: Vec<&str> = items.iter().map(|(label, _, _)| label.as_str()).collect();
        assert!(labels.contains(&"currency"), "{labels:?}");
        assert!(labels.contains(&"format"), "{labels:?}");
        assert!(labels.contains(&"Card"), "{labels:?}");
        assert!(labels.contains(&"P"), "{labels:?}");
    }

    #[test]
    fn require_is_an_import() {
        let (root, files) = project();
        let source = "const Card = require('../Card')\nnew Card()\n";
        let doc = parse(&root.join("resources/js/Pages/Cjs.js"), source).unwrap();
        let load = |path: &Path| files.get(path).cloned();
        let found = definition(&doc, at(source, "new Card", 5), &root, &load);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].path, root.join("resources/js/Card.ts"));
    }

    #[test]
    fn script_ranges_cover_only_the_script_block() {
        let ranges = script_ranges(BUTTON);
        assert_eq!(ranges.len(), 1);
        assert!(BUTTON[ranges[0].clone()].starts_with("\nimport { computed }"));
        assert!(ranges[0].end <= BUTTON.len());
    }
}
