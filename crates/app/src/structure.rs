//! File structure (PhpStorm's Structure tool window) and the breadcrumb under the caret.

use std::ops::Range;

use tree_sitter::Node;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructItem {
    pub depth: usize,
    pub icon: &'static str,
    pub name: String,
    pub detail: String,
    /// Byte range of the declaration's name (where navigation lands).
    pub range: Range<usize>,
}

fn name_of<'a>(node: Node, source: &'a str) -> Option<(&'a str, Range<usize>)> {
    let name = node.child_by_field_name("name")?;
    Some((&source[name.byte_range()], name.byte_range()))
}

fn is_class_like(kind: &str) -> bool {
    matches!(kind, "class_declaration" | "interface_declaration" | "trait_declaration" | "enum_declaration")
}

fn class_icon(kind: &str) -> &'static str {
    match kind {
        "interface_declaration" => "Ⓘ",
        "trait_declaration" => "Ⓣ",
        "enum_declaration" => "Ⓔ",
        _ => "Ⓒ",
    }
}

/// Visibility and `static` of a member declaration, e.g. `public static`.
fn modifiers(node: Node, source: &str) -> String {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .filter(|c| matches!(c.kind(), "visibility_modifier" | "static_modifier" | "abstract_modifier" | "final_modifier" | "readonly_modifier"))
        .map(|c| &source[c.byte_range()])
        .collect::<Vec<_>>()
        .join(" ")
}

fn walk(node: Node, source: &str, depth: usize, out: &mut Vec<StructItem>) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        let kind = child.kind();
        if is_class_like(kind) {
            if let Some((name, range)) = name_of(child, source) {
                out.push(StructItem { depth, icon: class_icon(kind), name: name.to_string(), detail: String::new(), range });
            }
            if let Some(body) = child.child_by_field_name("body") {
                walk(body, source, depth + 1, out);
            }
            continue;
        }
        match kind {
            "method_declaration" | "function_definition" => {
                if let Some((name, range)) = name_of(child, source) {
                    let params = child.child_by_field_name("parameters").map(|p| &source[p.byte_range()]).unwrap_or("()");
                    let returns = child.child_by_field_name("return_type").map(|r| format!(": {}", &source[r.byte_range()])).unwrap_or_default();
                    let params = if params.len() > 60 { "(…)".to_string() } else { params.split_whitespace().collect::<Vec<_>>().join(" ") };
                    out.push(StructItem {
                        depth,
                        icon: if kind == "method_declaration" { "ⓜ" } else { "ƒ" },
                        name: format!("{name}{params}{returns}"),
                        detail: modifiers(child, source),
                        range,
                    });
                }
            }
            "property_declaration" => {
                let mut c = child.walk();
                for element in child.named_children(&mut c).filter(|n| n.kind() == "property_element") {
                    if let Some(var) = element.named_child(0) {
                        out.push(StructItem { depth, icon: "ⓟ", name: source[var.byte_range()].to_string(), detail: modifiers(child, source), range: var.byte_range() });
                    }
                }
            }
            "const_declaration" => {
                let mut c = child.walk();
                for element in child.named_children(&mut c).filter(|n| n.kind() == "const_element") {
                    if let Some(name) = element.named_child(0) {
                        out.push(StructItem { depth, icon: "ⓒ", name: source[name.byte_range()].to_string(), detail: "const".into(), range: name.byte_range() });
                    }
                }
            }
            "enum_case" => {
                if let Some((name, range)) = name_of(child, source) {
                    out.push(StructItem { depth, icon: "ⓒ", name: name.to_string(), detail: "case".into(), range });
                }
            }
            "namespace_definition" | "compound_statement" | "declaration_list" | "enum_declaration_list" => walk(child, source, depth, out),
            _ => {}
        }
    }
}

/// Classes, members and functions of a PHP file in source order.
pub fn outline(source: &str) -> Vec<StructItem> {
    let Some(tree) = php_syntax::parse(source) else { return Vec::new() };
    let mut out = Vec::new();
    walk(tree.root_node(), source, 0, &mut out);
    out
}

/// `["User", "posts()"]`: the class and member around `offset`.
pub fn scope_at(source: &str, offset: usize) -> Vec<String> {
    let Some(tree) = php_syntax::parse(source) else { return Vec::new() };
    let Some(mut node) = tree.root_node().descendant_for_byte_range(offset, offset) else { return Vec::new() };
    let mut parts = Vec::new();
    loop {
        let kind = node.kind();
        if (is_class_like(kind) || matches!(kind, "method_declaration" | "function_definition"))
            && let Some((name, _)) = name_of(node, source)
        {
            parts.push(if is_class_like(kind) { name.to_string() } else { format!("{name}()") });
        }
        match node.parent() {
            Some(parent) => node = parent,
            None => break,
        }
    }
    parts.reverse();
    parts
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = "<?php\nnamespace App;\n\nclass User extends Model\n{\n    const ADMIN = 'a';\n    protected $fillable = [];\n    public static function find(int $id): ?self\n    {\n        return null; // here\n    }\n}\n\nfunction helper() {}\n";

    #[test]
    fn outlines_and_scopes() {
        let items: Vec<(usize, String)> = outline(SRC).into_iter().map(|i| (i.depth, i.name)).collect();
        assert_eq!(
            items,
            [
                (0, "User".to_string()),
                (1, "ADMIN".to_string()),
                (1, "$fillable".to_string()),
                (1, "find(int $id): ?self".to_string()),
                (0, "helper()".to_string()),
            ]
        );
        assert_eq!(scope_at(SRC, SRC.find("// here").unwrap()), ["User", "find()"]);
        assert!(scope_at(SRC, 3).is_empty());
    }
}
