//! Code ▸ Generate for PHP classes: constructor, getters and setters, implementing and
//! overriding methods, and PHPDoc for the function at the caret. Each returns the text to
//! insert as edits.

use php_index::{ClassKind, Index, Member, MemberKind};
use tree_sitter::Node;

use crate::{Document, TextEdit};

/// The class body enclosing `offset`: (its `{`..`}` node, the class FQN).
fn class_body<'t>(doc: &'t Document, offset: usize) -> Option<(Node<'t>, String)> {
    let mut node = doc.tree.root_node().descendant_for_byte_range(offset, offset)?;
    loop {
        if matches!(node.kind(), "class_declaration" | "trait_declaration" | "enum_declaration")
            && let Some(body) = node.child_by_field_name("body")
        {
            return Some((body, doc.current_class(body.start_byte() + 1)?));
        }
        node = node.parent()?;
    }
}

/// Indentation of the class members (the first member's, or 4 spaces).
fn member_indent(doc: &Document, body: Node) -> String {
    let mut cursor = body.walk();
    let first = body.named_children(&mut cursor).next();
    first
        .map(|m| {
            let line_start = doc.source[..m.start_byte()].rfind('\n').map_or(0, |i| i + 1);
            doc.source[line_start..m.start_byte()].to_string()
        })
        .filter(|i| i.trim().is_empty() && !i.is_empty())
        .unwrap_or_else(|| "    ".into())
}

/// Inserts `code` (members separated by blank lines) before the class's closing brace.
fn insert_members(doc: &Document, body: Node, members: Vec<String>) -> Option<TextEdit> {
    if members.is_empty() {
        return None;
    }
    let close = body.end_byte() - 1;
    let before = doc.source[body.start_byte() + 1..close].trim_end();
    let indent = member_indent(doc, body);
    let code: Vec<String> = members
        .iter()
        .map(|m| m.lines().map(|l| if l.is_empty() { String::new() } else { format!("{indent}{l}") }).collect::<Vec<_>>().join("\n"))
        .collect();
    let lead = if before.is_empty() { "" } else { "\n" };
    let start = body.start_byte() + 1 + before.len();
    Some(TextEdit { range: start..close, text: format!("\n{lead}{}\n", code.join("\n\n")) })
}

fn own_members<'a>(index: &'a Index, fqn: &str) -> Vec<&'a Member> {
    index.class(fqn).map(|c| c.members.iter().collect()).unwrap_or_default()
}

fn has_method(index: &Index, fqn: &str, name: &str) -> bool {
    own_members(index, fqn).iter().any(|m| m.kind == MemberKind::Method && m.name.eq_ignore_ascii_case(name))
}

/// Instance properties declared in the class, with their type text.
fn properties(index: &Index, fqn: &str) -> Vec<(String, String)> {
    own_members(index, fqn)
        .into_iter()
        .filter(|m| m.kind == MemberKind::Property && !m.is_static && !m.virtual_member)
        .map(|m| (m.name.clone(), m.detail.clone()))
        .collect()
}

fn ucfirst(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

/// Constructor assigning every instance property.
pub fn generate_constructor(doc: &Document, index: &Index, offset: usize) -> Option<TextEdit> {
    let (body, fqn) = class_body(doc, offset)?;
    if has_method(index, &fqn, "__construct") {
        return None;
    }
    let props = properties(index, &fqn);
    let params: Vec<String> = props.iter().map(|(n, t)| if t.is_empty() { format!("${n}") } else { format!("{t} ${n}") }).collect();
    let assigns: Vec<String> = props.iter().map(|(n, _)| format!("    $this->{n} = ${n};")).collect();
    let code = format!("public function __construct({})\n{{\n{}\n}}", params.join(", "), assigns.join("\n"));
    insert_members(doc, body, vec![code])
}

/// Getters (`getName()`, `isActive()` for bools) and/or setters for properties without one.
pub fn generate_accessors(doc: &Document, index: &Index, offset: usize, getters: bool, setters: bool) -> Option<TextEdit> {
    let (body, fqn) = class_body(doc, offset)?;
    let mut code = Vec::new();
    for (name, ty) in properties(index, &fqn) {
        let is_bool = matches!(ty.trim_start_matches('?'), "bool" | "boolean");
        let getter = format!("{}{}", if is_bool { "is" } else { "get" }, ucfirst(&name));
        if getters && !has_method(index, &fqn, &getter) {
            let ret = if ty.is_empty() { String::new() } else { format!(": {ty}") };
            code.push(format!("public function {getter}(){ret}\n{{\n    return $this->{name};\n}}"));
        }
        let setter = format!("set{}", ucfirst(&name));
        if setters && !has_method(index, &fqn, &setter) {
            let param = if ty.is_empty() { format!("${name}") } else { format!("{ty} ${name}") };
            code.push(format!("public function {setter}({param}): static\n{{\n    $this->{name} = ${name};\n\n    return $this;\n}}"));
        }
    }
    insert_members(doc, body, code)
}

/// A method declaration's visibility and whether it is abstract, read from its source.
fn declaration_flags(member: &Member, path: &std::path::Path) -> (String, bool) {
    let source = member.source.as_deref().unwrap_or(path);
    let text = std::fs::read_to_string(source).unwrap_or_default();
    let head = text.get(..member.range.start).unwrap_or("");
    let line_start = head.rfind(['\n', ';', '}', '{']).map_or(0, |i| i + 1);
    let modifiers = &head[line_start..];
    let visibility = ["private", "protected", "public"].into_iter().find(|v| modifiers.contains(v)).unwrap_or("public");
    (visibility.to_string(), modifiers.contains("abstract"))
}

/// Methods a class may implement or override: (owner FQN, member, visibility, abstract).
fn inherited_methods(index: &Index, fqn: &str) -> Vec<(String, Member, String, bool)> {
    let mut seen: Vec<String> = own_members(index, fqn).iter().filter(|m| m.kind == MemberKind::Method).map(|m| m.name.to_lowercase()).collect();
    let mut found = Vec::new();
    for ancestor in index.ancestors(fqn).into_iter().filter(|a| a.fqn != fqn) {
        for member in ancestor.members.iter().filter(|m| m.kind == MemberKind::Method && !m.virtual_member) {
            if seen.contains(&member.name.to_lowercase()) {
                continue;
            }
            let (visibility, is_abstract) = declaration_flags(member, &ancestor.path);
            if visibility == "private" {
                continue;
            }
            seen.push(member.name.to_lowercase());
            let is_abstract = is_abstract || ancestor.kind == ClassKind::Interface;
            found.push((ancestor.fqn.clone(), member.clone(), visibility, is_abstract));
        }
    }
    found
}

/// Names of the methods Override Methods can offer (non-abstract inherited ones).
pub fn overridable_methods(doc: &Document, index: &Index, offset: usize) -> Vec<String> {
    let Some((_, fqn)) = class_body(doc, offset) else { return Vec::new() };
    inherited_methods(index, &fqn).into_iter().filter(|(.., a)| !*a).map(|(_, m, ..)| m.name).filter(|n| n != "__construct").collect()
}

fn method_code(member: &Member, visibility: &str, call_parent: bool) -> String {
    let stat = if member.is_static { "static " } else { "" };
    let signature = &member.detail;
    let returns_void = signature.rsplit_once(')').is_some_and(|(_, ret)| {
        let ret = ret.trim().trim_start_matches(':').trim();
        ret.is_empty() || ret == "void" || ret == "never"
    });
    let body = if call_parent {
        let params = signature.split_once('(').map(|(_, rest)| rest).unwrap_or("");
        let params = params.rsplit_once(')').map(|(p, _)| p).unwrap_or(params);
        let args: Vec<&str> = params
            .split(',')
            .filter_map(|p| p.split_whitespace().find(|w| w.starts_with('$') || w.starts_with("...$") || w.starts_with("&$")))
            .map(|w| w.trim_start_matches('&').split('=').next().unwrap_or(w))
            .collect();
        let call = format!("parent::{}({})", member.name, args.join(", "));
        if returns_void { format!("    {call};") } else { format!("    return {call};") }
    } else {
        "    // TODO: Implement".to_string() + &format!(" {}() method.", member.name)
    };
    format!("{visibility} {stat}function {}{signature}\n{{\n{body}\n}}", member.name)
}

/// Implement Methods: every abstract or interface method the class doesn't define yet.
pub fn implement_methods(doc: &Document, index: &Index, offset: usize) -> Option<TextEdit> {
    let (body, fqn) = class_body(doc, offset)?;
    let code = inherited_methods(index, &fqn)
        .into_iter()
        .filter(|(.., is_abstract)| *is_abstract)
        .map(|(_, m, vis, _)| method_code(&m, &vis, false))
        .collect();
    insert_members(doc, body, code)
}

/// Override Method: `name` calling the parent implementation.
pub fn override_method(doc: &Document, index: &Index, offset: usize, name: &str) -> Option<TextEdit> {
    let (body, fqn) = class_body(doc, offset)?;
    let (_, member, visibility, _) = inherited_methods(index, &fqn).into_iter().find(|(_, m, ..)| m.name == name)?;
    insert_members(doc, body, vec![method_code(&member, &visibility, true)])
}

/// PHPDoc for the function or method at the caret: `@param` per parameter, `@return`.
pub fn generate_phpdoc(doc: &Document, offset: usize) -> Option<TextEdit> {
    let mut node = doc.tree.root_node().descendant_for_byte_range(offset, offset)?;
    while !matches!(node.kind(), "method_declaration" | "function_definition") {
        node = node.parent()?;
    }
    // Already documented?
    if node.prev_sibling().is_some_and(|p| p.kind() == "comment" && doc.text(p).starts_with("/**")) {
        return None;
    }
    let line_start = doc.source[..node.start_byte()].rfind('\n').map_or(0, |i| i + 1);
    let indent: String = doc.source[line_start..].chars().take_while(|c| *c == ' ' || *c == '\t').collect();
    let mut lines = vec!["/**".to_string()];
    if let Some(params) = node.child_by_field_name("parameters") {
        let mut cursor = params.walk();
        for param in params.named_children(&mut cursor) {
            let Some(name) = param.child_by_field_name("name") else { continue };
            let ty = param.child_by_field_name("type").map(|t| doc.text(t)).unwrap_or("mixed");
            lines.push(format!(" * @param {ty} {}", doc.text(name)));
        }
    }
    if let Some(ret) = node.child_by_field_name("return_type") {
        let ret = doc.text(ret);
        if ret != "void" {
            lines.push(format!(" * @return {ret}"));
        }
    }
    lines.push(" */".to_string());
    let text: String = lines.iter().map(|l| format!("{l}\n{indent}")).collect();
    let at = line_start + indent.len();
    Some(TextEdit { range: at..at, text })
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    fn apply(src: &str, edit: TextEdit) -> String {
        let mut s = src.to_string();
        s.replace_range(edit.range, &edit.text);
        s
    }

    const USER: &str = "<?php\nnamespace App;\n\nclass User extends Model implements HasName\n{\n    private string $name;\n    private bool $active;\n}\n";
    const BASE: &str = "<?php\nnamespace App;\n\nabstract class Model\n{\n    public function save(array $options = []): bool { return true; }\n    abstract protected function table(): string;\n}\n";
    const IFACE: &str = "<?php\nnamespace App;\n\ninterface HasName\n{\n    public function displayName(): string;\n}\n";

    fn setup(dir: &Path) -> Index {
        let mut index = Index::new();
        for (file, src) in [("User.php", USER), ("Model.php", BASE), ("HasName.php", IFACE)] {
            let path = dir.join(file);
            std::fs::write(&path, src).unwrap();
            index.update_file(&path, src);
        }
        index
    }

    #[test]
    fn generates_members() {
        let dir = std::env::temp_dir().join(format!("phpcraft-generate-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let index = setup(&dir);
        let path = dir.join("User.php");
        let doc = Document::parse(&path, USER).unwrap();
        let at = USER.find("$name").unwrap();

        let ctor = apply(USER, generate_constructor(&doc, &index, at).unwrap());
        assert!(ctor.contains("    public function __construct(string $name, bool $active)\n    {\n        $this->name = $name;"), "{ctor}");

        let acc = apply(USER, generate_accessors(&doc, &index, at, true, true).unwrap());
        assert!(acc.contains("public function getName(): string"));
        assert!(acc.contains("public function isActive(): bool"));
        assert!(acc.contains("public function setName(string $name): static"));

        let imp = apply(USER, implement_methods(&doc, &index, at).unwrap());
        assert!(imp.contains("protected function table(): string"), "{imp}");
        assert!(imp.contains("public function displayName(): string"));
        assert!(!imp.contains("function save"));

        assert_eq!(overridable_methods(&doc, &index, at), ["save"]);
        let ov = apply(USER, override_method(&doc, &index, at, "save").unwrap());
        assert!(ov.contains("public function save(array $options = []): bool\n    {\n        return parent::save($options);"), "{ov}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn generates_phpdoc() {
        let src = "<?php\nclass A\n{\n    public function find(int $id, $opts): ?User {}\n}\n";
        let doc = Document::parse(Path::new("/p/A.php"), src).unwrap();
        let out = apply(src, generate_phpdoc(&doc, src.find("find").unwrap()).unwrap());
        assert!(out.contains("    /**\n     * @param int $id\n     * @param mixed $opts\n     * @return ?User\n     */\n    public function find"), "{out}");
    }
}
