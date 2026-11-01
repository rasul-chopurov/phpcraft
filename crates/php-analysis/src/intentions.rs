//! Intention actions (⌥↵): small rewrites offered for the code at the caret.

use tree_sitter::Node;

use crate::types::visit;
use crate::{Document, TextEdit};

#[derive(Debug, Clone, PartialEq)]
pub struct Intention {
    pub label: String,
    pub edits: Vec<TextEdit>,
}

fn edit(node: Node, text: String) -> TextEdit {
    TextEdit { range: node.byte_range(), text }
}

/// The nodes from the one at `offset` up to the root.
fn ancestors_at<'t>(doc: &'t Document, offset: usize) -> Vec<Node<'t>> {
    let mut out = Vec::new();
    let mut node = doc.tree.root_node().descendant_for_byte_range(offset, offset);
    while let Some(n) = node {
        out.push(n);
        node = n.parent();
    }
    out
}

/// Intentions available at `offset`.
pub fn intentions(doc: &Document, offset: usize) -> Vec<Intention> {
    let mut out = Vec::new();
    let mut seen_kinds = Vec::new();
    for node in ancestors_at(doc, offset) {
        let kind = node.kind();
        if seen_kinds.contains(&kind) {
            continue;
        }
        let before = out.len();
        match kind {
            "string" => quotes(doc, node, &mut out),
            "encapsed_string" => quotes(doc, node, &mut out),
            "binary_expression" => {
                concatenation(doc, node, &mut out);
                flip(doc, node, &mut out);
            }
            "array_creation_expression" => array_syntax(doc, node, &mut out),
            "arrow_function" => arrow_to_closure(doc, node, &mut out),
            "anonymous_function" => closure_to_arrow(doc, node, &mut out),
            "method_declaration" | "property_declaration" | "const_declaration" => visibility(doc, node, offset, &mut out),
            "optional_type" | "union_type" => nullable(doc, node, &mut out),
            "if_statement" => invert_if(doc, node, &mut out),
            "qualified_name" => import_fqn(doc, node, &mut out),
            _ => {}
        }
        if out.len() > before {
            seen_kinds.push(kind);
        }
    }
    out
}

fn quotes(doc: &Document, node: Node, out: &mut Vec<Intention>) {
    let text = doc.text(node);
    if let Some(inner) = text.strip_prefix('\'').and_then(|t| t.strip_suffix('\'')) {
        if !inner.contains(['"', '$', '\\']) {
            out.push(Intention { label: "Replace quotes".into(), edits: vec![edit(node, format!("\"{inner}\""))] });
        }
    } else if let Some(inner) = text.strip_prefix('"').and_then(|t| t.strip_suffix('"'))
        && !inner.contains(['\'', '$', '\\', '{'])
    {
        out.push(Intention { label: "Replace quotes".into(), edits: vec![edit(node, format!("'{inner}'"))] });
    }
}

/// Operands of a `.` chain, left to right.
fn concat_parts<'t>(doc: &Document, node: Node<'t>, parts: &mut Vec<Node<'t>>) -> bool {
    if node.kind() == "binary_expression" && node.child_by_field_name("operator").is_some_and(|o| doc.text(o) == ".") {
        let (Some(l), Some(r)) = (node.child_by_field_name("left"), node.child_by_field_name("right")) else { return false };
        return concat_parts(doc, l, parts) && concat_parts(doc, r, parts);
    }
    parts.push(node);
    true
}

/// `'Hi ' . $name . '!'` → `"Hi {$name}!"`.
fn concatenation(doc: &Document, node: Node, out: &mut Vec<Intention>) {
    // Only for the outermost `.` of a chain.
    if node.parent().is_some_and(|p| p.kind() == "binary_expression" && p.child_by_field_name("operator").is_some_and(|o| doc.text(o) == ".")) {
        return;
    }
    let mut parts = Vec::new();
    if !concat_parts(doc, node, &mut parts) || parts.len() < 2 || !parts.iter().any(|p| matches!(p.kind(), "string" | "encapsed_string")) {
        return;
    }
    let mut text = String::from("\"");
    for part in parts {
        let t = doc.text(part);
        match part.kind() {
            "string" if t.starts_with('\'') => text.push_str(&t[1..t.len() - 1].replace("\\'", "'").replace('"', "\\\"").replace('$', "\\$")),
            "string" | "encapsed_string" => text.push_str(&t[1..t.len() - 1]),
            "variable_name" | "member_access_expression" | "member_call_expression" | "subscript_expression" => text.push_str(&format!("{{{t}}}")),
            _ => return,
        }
    }
    text.push('"');
    out.push(Intention { label: "Replace concatenation with interpolation".into(), edits: vec![edit(node, text)] });
}

/// `a == b` → `b == a` (and `<` ↔ `>`).
fn flip(doc: &Document, node: Node, out: &mut Vec<Intention>) {
    let (Some(l), Some(op), Some(r)) = (node.child_by_field_name("left"), node.child_by_field_name("operator"), node.child_by_field_name("right")) else { return };
    let flipped = match doc.text(op) {
        o @ ("==" | "===" | "!=" | "!==" | "<>" | "&&" | "||" | "and" | "or" | "+" | "*" | "&" | "|") => o.to_string(),
        "<" => ">".into(),
        ">" => "<".into(),
        "<=" => ">=".into(),
        ">=" => "<=".into(),
        _ => return,
    };
    out.push(Intention { label: format!("Flip '{}'", doc.text(op)), edits: vec![edit(node, format!("{} {flipped} {}", doc.text(r), doc.text(l)))] });
}

fn array_syntax(doc: &Document, node: Node, out: &mut Vec<Intention>) {
    let text = doc.text(node);
    if let Some(rest) = text.strip_prefix("array(").or_else(|| text.strip_prefix("array (")).and_then(|t| t.strip_suffix(')')) {
        out.push(Intention { label: "Convert to short array syntax".into(), edits: vec![edit(node, format!("[{rest}]"))] });
    } else if let Some(rest) = text.strip_prefix('[').and_then(|t| t.strip_suffix(']')) {
        out.push(Intention { label: "Convert to long array syntax".into(), edits: vec![edit(node, format!("array({rest})"))] });
    }
}

/// Variables an expression reads that aren't its own parameters (for `use (...)`).
fn free_variables(doc: &Document, body: Node, params: &[String]) -> Vec<String> {
    let mut found = Vec::new();
    visit(body, &mut |n| {
        if n.kind() == "variable_name" {
            let name = doc.text(n).to_string();
            if name != "$this" && !params.contains(&name) && !found.contains(&name) {
                found.push(name);
            }
        }
        true
    });
    found
}

fn param_names(doc: &Document, function: Node) -> Vec<String> {
    let mut names = Vec::new();
    if let Some(params) = function.child_by_field_name("parameters") {
        visit(params, &mut |n| {
            if n.kind() == "variable_name" {
                names.push(doc.text(n).to_string());
            }
            true
        });
    }
    names
}

/// `fn($x) => $x * $k` → `function ($x) use ($k) { return $x * $k; }`.
fn arrow_to_closure(doc: &Document, node: Node, out: &mut Vec<Intention>) {
    let (Some(params), Some(body)) = (node.child_by_field_name("parameters"), node.child_by_field_name("body")) else { return };
    let ret = node.child_by_field_name("return_type").map(|r| format!(": {}", doc.text(r))).unwrap_or_default();
    let free = free_variables(doc, body, &param_names(doc, node));
    let uses = if free.is_empty() { String::new() } else { format!(" use ({})", free.join(", ")) };
    let is_static = doc.text(node).starts_with("static");
    let text = format!("{}function {}{uses}{ret} {{ return {}; }}", if is_static { "static " } else { "" }, doc.text(params), doc.text(body));
    out.push(Intention { label: "Convert to closure".into(), edits: vec![edit(node, text)] });
}

/// A closure that only returns an expression → `fn(...) => expr`.
fn closure_to_arrow(doc: &Document, node: Node, out: &mut Vec<Intention>) {
    let (Some(params), Some(body)) = (node.child_by_field_name("parameters"), node.child_by_field_name("body")) else { return };
    let mut cursor = body.walk();
    let statements: Vec<Node> = body.named_children(&mut cursor).filter(|n| n.kind() != "comment").collect();
    let [statement] = statements[..] else { return };
    if statement.kind() != "return_statement" || doc.text(node).contains("use (&") {
        return;
    }
    let Some(value) = statement.named_child(0) else { return };
    let ret = node.child_by_field_name("return_type").map(|r| format!(": {}", doc.text(r))).unwrap_or_default();
    let text = format!("fn{}{ret} => {}", doc.text(params), doc.text(value));
    out.push(Intention { label: "Convert to arrow function".into(), edits: vec![edit(node, text)] });
}

fn visibility(doc: &Document, node: Node, offset: usize, out: &mut Vec<Intention>) {
    // Only on the declaration's first line (not anywhere in a method body).
    let header_end = doc.source[node.start_byte()..].find(['{', ';', '\n']).map_or(node.end_byte(), |i| node.start_byte() + i);
    if offset > header_end {
        return;
    }
    let mut cursor = node.walk();
    let modifier = node.children(&mut cursor).find(|c| c.kind() == "visibility_modifier");
    let current = modifier.map(|m| doc.text(m)).unwrap_or("public");
    for target in ["public", "protected", "private"] {
        if target == current {
            continue;
        }
        let edits = match modifier {
            Some(m) => vec![edit(m, target.to_string())],
            None => vec![TextEdit { range: node.start_byte()..node.start_byte(), text: format!("{target} ") }],
        };
        out.push(Intention { label: format!("Make {target}"), edits });
    }
}

fn nullable(doc: &Document, node: Node, out: &mut Vec<Intention>) {
    let text = doc.text(node);
    if let Some(inner) = text.strip_prefix('?') {
        out.push(Intention { label: format!("Replace with '{inner}|null'"), edits: vec![edit(node, format!("{inner}|null"))] });
    } else {
        let parts: Vec<&str> = text.split('|').map(str::trim).collect();
        if parts.len() == 2 && parts.iter().any(|p| p.eq_ignore_ascii_case("null")) {
            let other = parts.iter().find(|p| !p.eq_ignore_ascii_case("null")).unwrap();
            out.push(Intention { label: format!("Replace with '?{other}'"), edits: vec![edit(node, format!("?{other}"))] });
        }
    }
}

/// `if (c) {A} else {B}` → `if (!c) {B} else {A}`; without `else`, `if (!c) {} else {A}`.
fn invert_if(doc: &Document, node: Node, out: &mut Vec<Intention>) {
    let (Some(cond), Some(body)) = (node.child_by_field_name("condition"), node.child_by_field_name("body")) else { return };
    let mut cursor = node.walk();
    let clauses: Vec<Node> = node.children(&mut cursor).filter(|c| matches!(c.kind(), "else_clause" | "else_if_clause")).collect();
    if clauses.iter().any(|c| c.kind() == "else_if_clause") || clauses.len() > 1 || body.kind() != "compound_statement" {
        return;
    }
    let inner = doc.text(cond).trim_start_matches('(').trim_end_matches(')');
    let negated = match inner.strip_prefix('!') {
        Some(rest) if !rest.contains(['&', '|', '=', '<', '>']) => rest.trim_start_matches('(').trim_end_matches(')').to_string(),
        _ if inner.contains(' ') => format!("!({inner})"),
        _ => format!("!{inner}"),
    };
    let else_body = clauses.first().and_then(|c| c.child_by_field_name("body")).map(|b| doc.text(b).to_string()).unwrap_or_else(|| "{\n}".into());
    let text = format!("if ({negated}) {else_body} else {}", doc.text(body));
    out.push(Intention { label: "Invert 'if' condition".into(), edits: vec![edit(node, text)] });
}

/// `\App\Models\User` in code → `User` plus `use App\Models\User;`.
fn import_fqn(doc: &Document, node: Node, out: &mut Vec<Intention>) {
    let text = doc.text(node);
    if !text.starts_with('\\') || node.parent().is_some_and(|p| p.kind().contains("namespace")) {
        return;
    }
    let fqn = text.trim_start_matches('\\');
    let Some(short) = fqn.rsplit('\\').next() else { return };
    let mut edits = vec![edit(node, short.to_string())];
    if !doc.symbols.uses.iter().any(|u| u.fqn.eq_ignore_ascii_case(fqn)) {
        let Some(import) = crate::import_edit(doc, fqn) else { return };
        edits.push(import);
    }
    out.push(Intention { label: format!("Import class '{short}'"), edits });
}

/// Remove an unused variable's assignment, keeping a call on the right side for its effects.
pub fn remove_unused_variable(doc: &Document, range: std::ops::Range<usize>) -> Option<Intention> {
    let var = doc.tree.root_node().descendant_for_byte_range(range.start, range.end)?;
    let assignment = var.parent().filter(|p| p.kind() == "assignment_expression")?;
    let statement = assignment.parent().filter(|p| p.kind() == "expression_statement")?;
    let value = assignment.child_by_field_name("right")?;
    let pure = matches!(value.kind(), "string" | "encapsed_string" | "integer" | "float" | "boolean" | "null" | "variable_name" | "array_creation_expression");
    let edits = if pure {
        let start = doc.source[..statement.start_byte()].rfind('\n').map_or(0, |i| i + 1);
        let end = doc.source[statement.end_byte()..].find('\n').map_or(doc.source.len(), |i| statement.end_byte() + i + 1);
        vec![TextEdit { range: start..end, text: String::new() }]
    } else {
        vec![TextEdit { range: assignment.byte_range(), text: doc.text(value).to_string() }]
    };
    Some(Intention { label: format!("Remove unused variable '{}'", doc.text(var)), edits })
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    fn apply(src: &str, i: &Intention) -> String {
        let mut edits = i.edits.clone();
        edits.sort_by_key(|e| std::cmp::Reverse(e.range.start));
        let mut s = src.to_string();
        for e in edits {
            s.replace_range(e.range, &e.text);
        }
        s
    }

    fn run(src: &str, at: &str, label: &str) -> String {
        let doc = Document::parse(Path::new("/p/a.php"), src).unwrap();
        let offset = src.find(at).unwrap_or_else(|| panic!("{at}")) + 1;
        let all = intentions(&doc, offset);
        let i = all.iter().find(|i| i.label.starts_with(label)).unwrap_or_else(|| panic!("{label} not in {:?}", all.iter().map(|i| &i.label).collect::<Vec<_>>()));
        apply(src, i)
    }

    #[test]
    fn rewrites() {
        assert_eq!(run("<?php $a = 'x';", "'x'", "Replace quotes"), "<?php $a = \"x\";");
        assert_eq!(run("<?php $a = 'Hi ' . $name . '!';", "' . $", "Replace concatenation"), "<?php $a = \"Hi {$name}!\";");
        assert_eq!(run("<?php $a = array(1, 2);", "array(", "Convert to short"), "<?php $a = [1, 2];");
        assert_eq!(run("<?php $f = fn($x) => $x * $k;", "fn(", "Convert to closure"), "<?php $f = function ($x) use ($k) { return $x * $k; };");
        assert_eq!(run("<?php $f = function ($x) { return $x + 1; };", "function", "Convert to arrow"), "<?php $f = fn($x) => $x + 1;");
        assert_eq!(run("<?php class A { public function f() {} }", "public", "Make private"), "<?php class A { private function f() {} }");
        assert_eq!(run("<?php function f(?int $a) {}", "?int", "Replace with 'int|null'"), "<?php function f(int|null $a) {}");
        assert_eq!(run("<?php if ($a) { x(); } else { y(); }", "if (", "Invert"), "<?php if (!$a) { y(); } else { x(); }");
        assert_eq!(run("<?php if ($a < $b) {}", " < $b", "Flip"), "<?php if ($b > $a) {}");
        assert_eq!(run("<?php\nnamespace App;\n\n$u = new \\App\\Models\\User();", "\\App\\Models", "Import class"), "<?php\nnamespace App;\n\nuse App\\Models\\User;\n\n$u = new User();");
    }

    #[test]
    fn removes_unused_variables() {
        let src = "<?php\nfunction f() {\n    $a = 5;\n    $b = save();\n}\n";
        let doc = Document::parse(Path::new("/p/a.php"), src).unwrap();
        let a = src.find("$a").unwrap();
        assert_eq!(apply(src, &remove_unused_variable(&doc, a..a + 2).unwrap()), "<?php\nfunction f() {\n    $b = save();\n}\n");
        let b = src.find("$b").unwrap();
        assert_eq!(apply(src, &remove_unused_variable(&doc, b..b + 2).unwrap()), "<?php\nfunction f() {\n    $a = 5;\n    save();\n}\n");
    }
}
