//! Extracts declarations (classes, members, functions, imports) from one PHP file.

use std::ops::Range;
use std::path::Path;
use std::sync::Arc;

use tree_sitter::{Node, Tree};

use crate::names::{NameContext, UseItem, UseKind, is_builtin_type, is_relative_class, join};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClassKind {
    Class,
    Interface,
    Trait,
    Enum,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemberKind {
    Method,
    Property,
    Constant,
    Case,
}

#[derive(Debug, Clone)]
pub struct Member {
    /// Name without `$` for properties.
    pub name: String,
    pub kind: MemberKind,
    pub is_static: bool,
    /// Byte range of the member's name in its file.
    pub range: Range<usize>,
    /// Resolved FQN of the declared (or documented) class type, if any.
    pub type_fqn: Option<String>,
    /// Signature shown in completion, e.g. `(int $id): ?User`.
    pub detail: String,
    /// Declared through a phpdoc tag (`@property`, `@method`) rather than code.
    pub virtual_member: bool,
    /// File holding the declaration when it isn't the class's own file
    /// (e.g. a model column declared in a migration).
    pub source: Option<Arc<Path>>,
}

#[derive(Debug, Clone)]
pub struct ClassInfo {
    pub fqn: String,
    pub name: String,
    pub kind: ClassKind,
    pub path: Arc<Path>,
    pub range: Range<usize>,
    /// Parent class(es); interfaces may extend several.
    pub extends: Vec<String>,
    pub implements: Vec<String>,
    pub traits: Vec<String>,
    /// Classes named by `@mixin` whose members are reachable from this class.
    pub mixins: Vec<String>,
    /// Classes named by `@see` (facades point at the class they proxy).
    pub see: Vec<String>,
    pub members: Vec<Member>,
}

#[derive(Debug, Clone)]
pub struct FunctionInfo {
    pub fqn: String,
    pub path: Arc<Path>,
    pub range: Range<usize>,
    pub type_fqn: Option<String>,
    pub detail: String,
}

#[derive(Debug, Clone, Default)]
pub struct FileSymbols {
    pub namespace: String,
    /// Byte range of the `namespace ...;` statement, if any.
    pub namespace_range: Option<Range<usize>>,
    pub uses: Vec<UseItem>,
    pub classes: Vec<ClassInfo>,
    pub functions: Vec<FunctionInfo>,
}

impl FileSymbols {
    pub fn name_context(&self) -> NameContext {
        NameContext::new(&self.namespace, &self.uses)
    }

    /// The class whose body contains `offset`.
    pub fn class_at(&self, tree: &Tree, offset: usize) -> Option<&ClassInfo> {
        let mut node = tree.root_node().descendant_for_byte_range(offset, offset)?;
        loop {
            if is_class_like(node.kind())
                && let Some(name) = node.child_by_field_name("name")
            {
                return self.classes.iter().find(|c| c.range == name.byte_range());
            }
            node = node.parent()?;
        }
    }
}

pub fn is_class_like(kind: &str) -> bool {
    matches!(
        kind,
        "class_declaration" | "interface_declaration" | "trait_declaration" | "enum_declaration"
    )
}

pub fn text<'a>(node: Node, source: &'a str) -> &'a str {
    &source[node.byte_range()]
}

pub fn extract(path: &Path, source: &str, tree: &Tree) -> FileSymbols {
    let path: Arc<Path> = Arc::from(path);
    let mut symbols = FileSymbols::default();
    let root = tree.root_node();
    let mut cursor = root.walk();
    // Imports and the namespace come first so declarations resolve against them.
    for node in root.named_children(&mut cursor) {
        match node.kind() {
            "namespace_definition" => {
                if let Some(name) = node.child_by_field_name("name") {
                    symbols.namespace = text(name, source).to_string();
                    symbols.namespace_range = Some(node.byte_range());
                }
            }
            "namespace_use_declaration" => collect_uses(node, source, &mut symbols.uses),
            _ => {}
        }
    }
    let ctx = symbols.name_context();
    let mut cursor = root.walk();
    for node in root.named_children(&mut cursor) {
        collect_declarations(node, source, &ctx, &path, &mut symbols);
    }
    symbols
}

fn collect_uses(decl: Node, source: &str, out: &mut Vec<UseItem>) {
    let mut kind = UseKind::Class;
    let mut prefix = String::new();
    let mut cursor = decl.walk();
    for child in decl.children(&mut cursor) {
        match child.kind() {
            "function" => kind = UseKind::Function,
            "const" => kind = UseKind::Const,
            "namespace_name" => prefix = text(child, source).trim_start_matches('\\').to_string(),
            "namespace_use_clause" => push_clause(child, source, kind, "", decl, out),
            "namespace_use_group" => {
                let mut group_cursor = child.walk();
                for clause in child.named_children(&mut group_cursor) {
                    if clause.kind() == "namespace_use_clause" {
                        push_clause(clause, source, kind, &prefix, decl, out);
                    }
                }
            }
            _ => {}
        }
    }
}

fn push_clause(clause: Node, source: &str, kind: UseKind, prefix: &str, decl: Node, out: &mut Vec<UseItem>) {
    let kind = match clause.child_by_field_name("type").map(|t| t.kind()) {
        Some("function") => UseKind::Function,
        Some("const") => UseKind::Const,
        _ => kind,
    };
    let alias = clause.child_by_field_name("alias");
    let mut cursor = clause.walk();
    let Some(name) = clause
        .named_children(&mut cursor)
        .find(|n| matches!(n.kind(), "name" | "qualified_name") && Some(*n) != alias)
    else {
        return;
    };
    let fqn = join(prefix, text(name, source).trim_start_matches('\\'));
    let alias_text = alias
        .map(|a| text(a, source).to_string())
        .unwrap_or_else(|| crate::names::short_name(&fqn).to_string());
    out.push(UseItem {
        kind,
        fqn,
        alias: alias_text,
        has_explicit_alias: alias.is_some(),
        decl_range: decl.byte_range(),
    });
}

fn collect_declarations(node: Node, source: &str, ctx: &NameContext, path: &Arc<Path>, out: &mut FileSymbols) {
    match node.kind() {
        kind if is_class_like(kind) => {
            if let Some(class) = class_info(node, source, ctx, path) {
                out.classes.push(class);
            }
        }
        "function_definition" => {
            if let Some(name) = node.child_by_field_name("name") {
                let doc = doc_comment(node, source);
                out.functions.push(FunctionInfo {
                    fqn: join(&ctx.namespace, text(name, source)),
                    path: path.clone(),
                    range: name.byte_range(),
                    type_fqn: return_type(node, source, ctx, "", doc.as_deref()),
                    detail: signature(node, source),
                });
            }
        }
        // Declarations wrapped in `if (!function_exists(...)) { ... }` (common in helpers).
        "if_statement" | "compound_statement" | "namespace_definition" => {
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                collect_declarations(child, source, ctx, path, out);
            }
        }
        _ => {}
    }
}

fn class_info(node: Node, source: &str, ctx: &NameContext, path: &Arc<Path>) -> Option<ClassInfo> {
    let name_node = node.child_by_field_name("name")?;
    let name = text(name_node, source).to_string();
    let fqn = join(&ctx.namespace, &name);
    let kind = match node.kind() {
        "interface_declaration" => ClassKind::Interface,
        "trait_declaration" => ClassKind::Trait,
        "enum_declaration" => ClassKind::Enum,
        _ => ClassKind::Class,
    };
    let mut class = ClassInfo {
        fqn: fqn.clone(),
        name,
        kind,
        path: path.clone(),
        range: name_node.byte_range(),
        extends: Vec::new(),
        implements: Vec::new(),
        traits: Vec::new(),
        mixins: Vec::new(),
        see: Vec::new(),
        members: Vec::new(),
    };
    let resolve_names = |clause: Node| -> Vec<String> {
        let mut cursor = clause.walk();
        clause
            .named_children(&mut cursor)
            .filter(|n| matches!(n.kind(), "name" | "qualified_name"))
            .filter_map(|n| ctx.resolve_class(text(n, source)))
            .collect()
    };
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        match child.kind() {
            "base_clause" => class.extends = resolve_names(child),
            "class_interface_clause" => class.implements = resolve_names(child),
            _ => {}
        }
    }
    if let Some(doc) = doc_comment(node, source) {
        let doc_start = doc_comment_start(node).unwrap_or(0);
        doc_class_tags(&doc, doc_start, ctx, &fqn, &mut class);
    }
    if let Some(body) = node.child_by_field_name("body") {
        let mut cursor = body.walk();
        for item in body.named_children(&mut cursor) {
            collect_member(item, source, ctx, &fqn, &mut class);
        }
    }
    Some(class)
}

fn has_static(node: Node) -> bool {
    let mut cursor = node.walk();
    node.children(&mut cursor).any(|c| c.kind() == "static_modifier")
}

fn collect_member(item: Node, source: &str, ctx: &NameContext, class_fqn: &str, class: &mut ClassInfo) {
    let doc = doc_comment(item, source);
    match item.kind() {
        "method_declaration" => {
            let Some(name) = item.child_by_field_name("name") else { return };
            class.members.push(Member {
                name: text(name, source).to_string(),
                kind: MemberKind::Method,
                is_static: has_static(item),
                range: name.byte_range(),
                type_fqn: return_type(item, source, ctx, class_fqn, doc.as_deref()),
                detail: signature(item, source),
                virtual_member: false,
                    source: None,
            });
            // Constructor-promoted properties.
            if let Some(params) = item.child_by_field_name("parameters") {
                let mut cursor = params.walk();
                for param in params.named_children(&mut cursor) {
                    if param.kind() != "property_promotion_parameter" {
                        continue;
                    }
                    let Some(var) = param.child_by_field_name("name") else { continue };
                    let type_text = param.child_by_field_name("type").map(|t| text(t, source));
                    class.members.push(Member {
                        name: text(var, source).trim_start_matches('$').to_string(),
                        kind: MemberKind::Property,
                        is_static: false,
                        range: var.byte_range(),
                        type_fqn: type_text.and_then(|t| class_type(t, ctx, class_fqn)),
                        detail: type_text.unwrap_or_default().to_string(),
                        virtual_member: false,
                    source: None,
                    });
                }
            }
        }
        "property_declaration" => {
            let type_text = item.child_by_field_name("type").map(|t| text(t, source).to_string());
            let doc_type = doc.as_deref().and_then(|d| doc_tag_type(d, "@var"));
            let type_text = doc_type.or(type_text);
            let is_static = has_static(item);
            let mut cursor = item.walk();
            for element in item.named_children(&mut cursor) {
                if element.kind() != "property_element" {
                    continue;
                }
                let Some(var) = element.child_by_field_name("name") else { continue };
                class.members.push(Member {
                    name: text(var, source).trim_start_matches('$').to_string(),
                    kind: MemberKind::Property,
                    is_static,
                    range: var.byte_range(),
                    type_fqn: type_text.as_deref().and_then(|t| class_type(t, ctx, class_fqn)),
                    detail: type_text.clone().unwrap_or_default(),
                    virtual_member: false,
                    source: None,
                });
            }
        }
        "const_declaration" => {
            let mut cursor = item.walk();
            for element in item.named_children(&mut cursor) {
                if element.kind() != "const_element" {
                    continue;
                }
                let mut inner = element.walk();
                if let Some(name) = element.named_children(&mut inner).find(|n| n.kind() == "name") {
                    class.members.push(Member {
                        name: text(name, source).to_string(),
                        kind: MemberKind::Constant,
                        is_static: true,
                        range: name.byte_range(),
                        type_fqn: None,
                        detail: String::new(),
                        virtual_member: false,
                    source: None,
                    });
                }
            }
        }
        "enum_case" => {
            if let Some(name) = item.child_by_field_name("name") {
                class.members.push(Member {
                    name: text(name, source).to_string(),
                    kind: MemberKind::Case,
                    is_static: true,
                    range: name.byte_range(),
                    type_fqn: Some(class_fqn.to_string()),
                    detail: String::new(),
                    virtual_member: false,
                    source: None,
                });
            }
        }
        "use_declaration" => {
            let mut cursor = item.walk();
            for name in item.named_children(&mut cursor) {
                if matches!(name.kind(), "name" | "qualified_name")
                    && let Some(fqn) = ctx.resolve_class(text(name, source))
                {
                    class.traits.push(fqn);
                }
            }
        }
        _ => {}
    }
}

fn signature(node: Node, source: &str) -> String {
    let params = node
        .child_by_field_name("parameters")
        .map(|p| text(p, source).split_whitespace().collect::<Vec<_>>().join(" "))
        .unwrap_or_else(|| "()".into());
    match node.child_by_field_name("return_type") {
        Some(ret) => format!("{params}: {}", text(ret, source)),
        None => params,
    }
}

fn return_type(node: Node, source: &str, ctx: &NameContext, class_fqn: &str, doc: Option<&str>) -> Option<String> {
    let declared = node.child_by_field_name("return_type").map(|t| text(t, source));
    let documented = doc.and_then(|d| doc_tag_type(d, "@return"));
    // Docblocks are usually more precise (`@return static`, `@return HasMany<Post>`).
    documented
        .as_deref()
        .and_then(|t| class_type(t, ctx, class_fqn))
        .or_else(|| declared.and_then(|t| class_type(t, ctx, class_fqn)))
}

/// Picks the first class type from a type expression like `?User`, `User|null`, `Collection<int, User>`.
pub fn class_type(type_text: &str, ctx: &NameContext, class_fqn: &str) -> Option<String> {
    for part in type_text.split(['|', '&']) {
        let part = part.trim().trim_start_matches('?').trim_start_matches('(');
        let part = part.split('<').next().unwrap_or(part).trim_end_matches("[]").trim();
        if part.is_empty() || is_builtin_type(part) {
            continue;
        }
        if is_relative_class(part) {
            if !class_fqn.is_empty() {
                return Some(class_fqn.to_string());
            }
            continue;
        }
        if let Some(fqn) = ctx.resolve_class(part) {
            return Some(fqn);
        }
    }
    None
}

/// The `/** ... */` comment directly preceding a declaration.
pub fn doc_comment(node: Node, source: &str) -> Option<String> {
    let mut prev = node.prev_sibling()?;
    // Attributes sit between the docblock and the declaration.
    while prev.kind() == "attribute_list" {
        prev = prev.prev_sibling()?;
    }
    let text = text(prev, source);
    (prev.kind() == "comment" && text.starts_with("/**")).then(|| text.to_string())
}

fn doc_comment_start(node: Node) -> Option<usize> {
    let mut prev = node.prev_sibling()?;
    while prev.kind() == "attribute_list" {
        prev = prev.prev_sibling()?;
    }
    Some(prev.start_byte())
}

fn doc_lines(doc: &str) -> impl Iterator<Item = &str> {
    doc.lines().map(|l| {
        l.trim()
            .trim_start_matches("/**")
            .trim_end_matches("*/")
            .trim_start_matches('*')
            .trim()
    })
}

/// The type following a tag, e.g. `@return static` -> `static`.
pub fn doc_tag_type(doc: &str, tag: &str) -> Option<String> {
    doc_lines(doc).find_map(|line| {
        let rest = line.strip_prefix(tag)?;
        if !rest.starts_with(char::is_whitespace) {
            return None;
        }
        Some(read_type(rest.trim_start()).to_string())
    })
}

/// Reads a type expression up to the first whitespace outside `<...>`.
fn read_type(s: &str) -> &str {
    let mut depth = 0i32;
    for (i, c) in s.char_indices() {
        match c {
            '<' | '(' | '{' => depth += 1,
            '>' | ')' | '}' => depth -= 1,
            c if c.is_whitespace() && depth <= 0 => return &s[..i],
            _ => {}
        }
    }
    s
}

fn doc_class_tags(doc: &str, doc_start: usize, ctx: &NameContext, class_fqn: &str, class: &mut ClassInfo) {
    let mut line_start = doc_start;
    for raw in doc.split_inclusive('\n') {
        let line_offset = line_start;
        line_start += raw.len();
        let line = doc_lines(raw).next().unwrap_or("");
        // Range of `word` within this docblock line, for navigation to the tag.
        let range_of = |word: &str| {
            let at = raw.find(word).map_or(0, |i| i);
            line_offset + at..line_offset + at + word.len()
        };
        let Some((tag, rest)) = line.split_once(char::is_whitespace) else { continue };
        let rest = rest.trim();
        match tag {
            "@property" | "@property-read" | "@property-write" => {
                let ty = read_type(rest);
                let Some(var) = rest[ty.len()..].split_whitespace().next() else { continue };
                if !var.starts_with('$') {
                    continue;
                }
                class.members.push(Member {
                    name: var.trim_start_matches('$').to_string(),
                    kind: MemberKind::Property,
                    is_static: false,
                    range: range_of(var),
                    type_fqn: class_type(ty, ctx, class_fqn),
                    detail: ty.to_string(),
                    virtual_member: true,
                    source: None,
                });
            }
            "@method" => {
                let (is_static, rest) = match rest.strip_prefix("static ") {
                    Some(r) => (true, r.trim()),
                    None => (false, rest),
                };
                // `@method ReturnType name(args)` or `@method name(args)`.
                let Some(paren) = rest.find('(') else { continue };
                let head = rest[..paren].trim();
                let (ty, name) = match head.rsplit_once(char::is_whitespace) {
                    Some((ty, name)) => (Some(ty.trim()), name),
                    None => (None, head),
                };
                class.members.push(Member {
                    name: name.to_string(),
                    kind: MemberKind::Method,
                    is_static,
                    range: range_of(&format!("{name}(")).start..range_of(name).start + name.len(),
                    type_fqn: ty.and_then(|t| class_type(t, ctx, class_fqn)),
                    detail: rest[paren..].to_string(),
                    virtual_member: true,
                    source: None,
                });
            }
            "@see" => {
                let target = read_type(rest);
                if target.starts_with(|c: char| c.is_uppercase() || c == '\\') && !target.contains("::")
                    && let Some(fqn) = ctx.resolve_class(target)
                {
                    class.see.push(fqn);
                }
            }
            "@mixin" => {
                if let Some(fqn) = ctx.resolve_class(read_type(rest)) {
                    class.mixins.push(fqn);
                }
            }
            _ => {}
        }
    }
}
