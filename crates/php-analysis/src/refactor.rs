//! Refactorings that rewrite one file: Introduce Variable, Extract Method and Inline Variable.

use std::collections::BTreeSet;
use std::ops::Range;

use tree_sitter::Node;

use crate::types::{enclosing_function, visit};
use crate::{Document, TextEdit};

/// The statement containing `node` that sits directly in a block (where code can be inserted).
fn statement_of(node: Node) -> Option<Node> {
    let mut node = node;
    loop {
        let parent = node.parent()?;
        if matches!(parent.kind(), "compound_statement" | "program" | "declaration_list" | "switch_block" | "case_statement" | "default_statement") {
            return Some(node);
        }
        node = parent;
    }
}

fn indent_of(doc: &Document, at: usize) -> String {
    let start = doc.source[..at].rfind('\n').map_or(0, |i| i + 1);
    doc.source[start..].chars().take_while(|c| *c == ' ' || *c == '\t').collect()
}

fn line_start(doc: &Document, at: usize) -> usize {
    doc.source[..at].rfind('\n').map_or(0, |i| i + 1)
}

/// A variable name for an expression: `$user` for `getUser()` / `new User()` / `->user`.
fn suggest_name(doc: &Document, expr: Node) -> String {
    let base = match expr.kind() {
        "member_call_expression" | "scoped_call_expression" | "nullsafe_member_call_expression" => expr.child_by_field_name("name").map(|n| doc.text(n).to_string()),
        "function_call_expression" => expr.child_by_field_name("function").map(|n| doc.text(n).rsplit('\\').next().unwrap_or("").to_string()),
        "member_access_expression" | "nullsafe_member_access_expression" => expr.child_by_field_name("name").map(|n| doc.text(n).to_string()),
        "object_creation_expression" => {
            let mut cursor = expr.walk();
            let name = expr.named_children(&mut cursor).find(|c| matches!(c.kind(), "name" | "qualified_name")).map(|n| doc.text(n).rsplit('\\').next().unwrap_or("").to_string());
            name
        }
        "string" | "encapsed_string" => Some("text".into()),
        "integer" | "float" => Some("number".into()),
        "array_creation_expression" => Some("items".into()),
        _ => None,
    };
    let base = base.unwrap_or_else(|| "value".into());
    let base = ["get", "is", "has", "find", "to"].iter().find_map(|p| base.strip_prefix(p).filter(|r| r.starts_with(char::is_uppercase))).unwrap_or(&base);
    let mut chars = base.chars();
    let name: String = chars.next().map(|c| c.to_lowercase().chain(chars).collect()).unwrap_or_else(|| "value".into());
    format!("${name}")
}

/// Introduce Variable (⌘⌥V): the selected expression (or the one at the caret) becomes a
/// variable assigned on the line before its statement. Returns the edits and the name.
pub fn introduce_variable(doc: &Document, selection: Range<usize>) -> Option<(Vec<TextEdit>, String)> {
    let root = doc.tree.root_node();
    let expr = if selection.is_empty() {
        let mut node = root.descendant_for_byte_range(selection.start, selection.start)?;
        // The largest expression ending at the caret's statement level that is not a whole statement.
        while let Some(parent) = node.parent() {
            if parent.kind().ends_with("_statement") || parent.kind() == "compound_statement" || parent.kind() == "assignment_expression" {
                break;
            }
            node = parent;
        }
        node
    } else {
        let mut node = root.descendant_for_byte_range(selection.start, selection.end)?;
        while node.byte_range() != selection {
            node = node.parent()?;
            if node.start_byte() < selection.start {
                return None;
            }
        }
        node
    };
    if !expr.is_named() || expr.kind().ends_with("statement") || expr.kind() == "variable_name" {
        return None;
    }
    let statement = statement_of(expr)?;
    let name = suggest_name(doc, expr);
    let at = line_start(doc, statement.start_byte());
    let indent = indent_of(doc, statement.start_byte());
    Some((
        vec![
            TextEdit { range: at..at, text: format!("{indent}{name} = {};\n", doc.text(expr)) },
            TextEdit { range: expr.byte_range(), text: name.clone() },
        ],
        name,
    ))
}

/// Variables (`$name`) used in `range` of `scope`, in order of first use (`$this` excluded).
fn variables_in(doc: &Document, scope: Node, range: &Range<usize>) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    visit(scope, &mut |n| {
        if n.kind() == "variable_name" && n.start_byte() >= range.start && n.end_byte() <= range.end {
            let name = doc.text(n).to_string();
            if name != "$this" && !found.contains(&name) {
                found.push(name);
            }
        }
        !(n.start_byte() > range.end)
    });
    found
}

/// Whether `$var` is written in `range` (assignment target or foreach variable).
fn assigned_in(doc: &Document, scope: Node, range: &Range<usize>, var: &str) -> bool {
    let mut assigned = false;
    visit(scope, &mut |n| {
        if n.start_byte() >= range.start && n.end_byte() <= range.end && matches!(n.kind(), "assignment_expression" | "augmented_assignment_expression") {
            if let Some(left) = n.child_by_field_name("left")
                && doc.text(left) == var
            {
                assigned = true;
            }
        }
        !assigned
    });
    assigned
}

/// Extract Method (⌘⌥M): the selected statements move into a new private method after the
/// current one; variables they read become parameters, a variable they set and that is used
/// afterwards is returned.
pub fn extract_method(doc: &Document, selection: Range<usize>, name: &str) -> Option<Vec<TextEdit>> {
    let root = doc.tree.root_node();
    let first = statement_of(root.descendant_for_byte_range(selection.start, selection.start)?)?;
    let last = statement_of(root.descendant_for_byte_range(selection.end.saturating_sub(1), selection.end.saturating_sub(1))?)?;
    if first.parent()?.id() != last.parent()?.id() {
        return None;
    }
    let range = first.start_byte()..last.end_byte();
    let function = enclosing_function(first)?;
    let is_method = function.kind() == "method_declaration";
    let body = function.child_by_field_name("body")?;
    let used = variables_in(doc, body, &range);
    let params_node = function.child_by_field_name("parameters");
    let before: BTreeSet<String> = {
        let mut set = BTreeSet::new();
        if let Some(params) = params_node {
            for v in variables_in(doc, params, &params.byte_range()) {
                set.insert(v);
            }
        }
        for v in variables_in(doc, body, &(body.start_byte()..range.start)) {
            set.insert(v);
        }
        set
    };
    let after: BTreeSet<String> = variables_in(doc, body, &(range.end..body.end_byte())).into_iter().collect();
    let params: Vec<String> = used.iter().filter(|v| before.contains(*v)).cloned().collect();
    let outputs: Vec<String> = used.iter().filter(|v| after.contains(*v) && assigned_in(doc, body, &range, v)).cloned().collect();
    if outputs.len() > 1 {
        return None;
    }
    let output = outputs.first();

    let indent = indent_of(doc, first.start_byte());
    let member_indent = indent_of(doc, function.start_byte());
    let unit = indent.strip_prefix(&member_indent).filter(|u| !u.is_empty()).unwrap_or("    ").to_string();
    let mut body_lines: Vec<String> = doc.source[line_start(doc, range.start)..range.end]
        .lines()
        .map(|l| format!("{member_indent}{unit}{}", l.strip_prefix(indent.as_str()).unwrap_or(l.trim_start())))
        .collect();
    if let Some(out) = output {
        body_lines.push(format!("{member_indent}{unit}return {out};"));
    }
    let (visibility, call_prefix) = if is_method { ("private function", "$this->") } else { ("function", "") };
    let has_static = is_method && {
        let mut cursor = function.walk();
        let is_static = function.children(&mut cursor).any(|c| c.kind() == "static_modifier");
        is_static
    };
    let (visibility, call_prefix) = if has_static { ("private static function", "self::") } else { (visibility, call_prefix) };
    let method = format!("\n\n{member_indent}{visibility} {name}({})\n{member_indent}{{\n{}\n{member_indent}}}", params.join(", "), body_lines.join("\n"));
    let call = format!("{call_prefix}{name}({})", params.join(", "));
    let call = match output {
        Some(out) => format!("{out} = {call};"),
        None => format!("{call};"),
    };
    Some(vec![
        TextEdit { range: function.end_byte()..function.end_byte(), text: method },
        TextEdit { range: range.clone(), text: call },
    ])
}

/// Inline Variable (⌘⌥N): a variable assigned once is replaced by its value everywhere and
/// the assignment removed.
pub fn inline_variable(doc: &Document, offset: usize) -> Option<Vec<TextEdit>> {
    let mut node = doc.tree.root_node().descendant_for_byte_range(offset, offset)?;
    if node.kind() != "variable_name" {
        node = node.parent().filter(|p| p.kind() == "variable_name")?;
    }
    let var = doc.text(node).to_string();
    let scope = enclosing_function(node).and_then(|f| f.child_by_field_name("body")).unwrap_or(doc.tree.root_node());
    let mut assignments = Vec::new();
    let mut uses = Vec::new();
    visit(scope, &mut |n| {
        if n.kind() == "assignment_expression" && n.child_by_field_name("left").is_some_and(|l| doc.text(l) == var) {
            assignments.push(n);
        }
        if n.kind() == "variable_name" && doc.text(n) == var {
            uses.push(n);
        }
        true
    });
    let [assignment] = assignments[..] else { return None };
    let value = assignment.child_by_field_name("right")?;
    let left = assignment.child_by_field_name("left")?;
    let statement = statement_of(assignment).filter(|s| s.kind() == "expression_statement")?;
    let simple = matches!(value.kind(), "variable_name" | "name" | "integer" | "float" | "string" | "encapsed_string" | "boolean" | "null" | "member_call_expression" | "function_call_expression" | "member_access_expression" | "scoped_call_expression" | "class_constant_access_expression" | "array_creation_expression" | "parenthesized_expression");
    let text = if simple { doc.text(value).to_string() } else { format!("({})", doc.text(value)) };
    let mut edits: Vec<TextEdit> = uses
        .iter()
        .filter(|u| u.id() != left.id() && u.start_byte() > statement.end_byte())
        .map(|u| TextEdit { range: u.byte_range(), text: text.clone() })
        .collect();
    // Remove the assignment line.
    let start = line_start(doc, statement.start_byte());
    let end = doc.source[statement.end_byte()..].find('\n').map_or(doc.source.len(), |i| statement.end_byte() + i + 1);
    edits.push(TextEdit { range: start..end, text: String::new() });
    Some(edits)
}

/// The expression at `selection` (exactly selected, or the largest one at the caret).
fn expression_at<'t>(doc: &'t Document, selection: &Range<usize>) -> Option<Node<'t>> {
    let root = doc.tree.root_node();
    let expr = if selection.is_empty() {
        let mut node = root.descendant_for_byte_range(selection.start, selection.start)?;
        while let Some(parent) = node.parent() {
            if parent.kind().ends_with("_statement") || parent.kind() == "compound_statement" || parent.kind() == "assignment_expression" {
                break;
            }
            node = parent;
        }
        node
    } else {
        let mut node = root.descendant_for_byte_range(selection.start, selection.end)?;
        while node.byte_range() != *selection {
            node = node.parent()?;
            if node.start_byte() < selection.start {
                return None;
            }
        }
        node
    };
    (expr.is_named() && !expr.kind().ends_with("statement")).then_some(expr)
}

/// The class body `{` containing `node` and the indentation of its members.
fn class_body_of<'t>(doc: &Document, node: Node<'t>) -> Option<(Node<'t>, String)> {
    let mut n = node;
    loop {
        if matches!(n.kind(), "class_declaration" | "trait_declaration" | "enum_declaration") {
            let body = n.child_by_field_name("body")?;
            let mut cursor = body.walk();
            let first = body.named_children(&mut cursor).next();
            let indent = first.map(|m| indent_of(doc, m.start_byte())).filter(|i| !i.is_empty()).unwrap_or_else(|| "    ".into());
            return Some((body, indent));
        }
        n = n.parent()?;
    }
}

fn constant_name(doc: &Document, expr: Node) -> String {
    let text = doc.text(expr).trim_matches(['\'', '"']);
    let name: String = text.chars().map(|c| if c.is_alphanumeric() { c.to_ascii_uppercase() } else { '_' }).collect();
    let name = name.trim_matches('_').to_string();
    if name.is_empty() || name.starts_with(|c: char| c.is_ascii_digit()) || !matches!(expr.kind(), "string" | "encapsed_string") { "VALUE".into() } else { name.chars().take(40).collect() }
}

/// Introduce Constant (⌘⌥C): the expression becomes `private const NAME` of the class.
pub fn introduce_constant(doc: &Document, selection: Range<usize>) -> Option<(Vec<TextEdit>, String)> {
    let expr = expression_at(doc, &selection)?;
    let (body, indent) = class_body_of(doc, expr)?;
    let name = constant_name(doc, expr);
    let at = body.start_byte() + 1;
    Some((
        vec![
            TextEdit { range: at..at, text: format!("\n{indent}private const {name} = {};\n", doc.text(expr)) },
            TextEdit { range: expr.byte_range(), text: format!("self::{name}") },
        ],
        name,
    ))
}

/// Introduce Field (⌘⌥F): the expression is assigned to a new property before its statement.
pub fn introduce_field(doc: &Document, selection: Range<usize>) -> Option<(Vec<TextEdit>, String)> {
    let expr = expression_at(doc, &selection)?;
    let (body, indent) = class_body_of(doc, expr)?;
    let statement = statement_of(expr)?;
    let name = suggest_name(doc, expr).trim_start_matches('$').to_string();
    let at = body.start_byte() + 1;
    let line = line_start(doc, statement.start_byte());
    let statement_indent = indent_of(doc, statement.start_byte());
    Some((
        vec![
            TextEdit { range: at..at, text: format!("\n{indent}private ${name};\n") },
            TextEdit { range: line..line, text: format!("{statement_indent}$this->{name} = {};\n", doc.text(expr)) },
            TextEdit { range: expr.byte_range(), text: format!("$this->{name}") },
        ],
        name,
    ))
}

/// Safe Delete: removes the declaration named at `name_range` with its doc comment and
/// attributes (the caller checks there are no usages first).
pub fn delete_declaration(doc: &Document, name_range: Range<usize>) -> Option<TextEdit> {
    let mut node = doc.tree.root_node().descendant_for_byte_range(name_range.start, name_range.end)?;
    loop {
        if matches!(node.kind(), "method_declaration" | "function_definition" | "class_declaration" | "interface_declaration" | "trait_declaration" | "enum_declaration" | "property_declaration" | "const_declaration") {
            break;
        }
        node = node.parent()?;
    }
    let mut start = node.start_byte();
    // A doc comment right above goes too.
    if let Some(prev) = node.prev_sibling().filter(|p| p.kind() == "comment" && doc.text(*p).starts_with("/**")) {
        start = prev.start_byte();
    }
    let mut start = line_start(doc, start);
    let mut end = doc.source[node.end_byte()..].find('\n').map_or(doc.source.len(), |i| node.end_byte() + i + 1);
    // Keep one blank line between the remaining members: drop the one after the declaration,
    // or the one before it when it was the last member.
    if doc.source[end..].starts_with('\n') {
        end += 1;
    } else if doc.source[end..].trim_start_matches([' ', '\t']).starts_with('}') && doc.source[..start].ends_with("\n\n") {
        start -= 1;
    }
    Some(TextEdit { range: start..end, text: String::new() })
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    fn apply(src: &str, mut edits: Vec<TextEdit>) -> String {
        edits.sort_by_key(|e| std::cmp::Reverse(e.range.start));
        let mut s = src.to_string();
        for e in edits {
            s.replace_range(e.range, &e.text);
        }
        s
    }

    #[test]
    fn introduces_a_variable() {
        let src = "<?php\nfunction f($repo)\n{\n    return $repo->getUser()->name;\n}\n";
        let doc = Document::parse(Path::new("/p/a.php"), src).unwrap();
        let start = src.find("$repo->getUser()").unwrap();
        let (edits, name) = introduce_variable(&doc, start..start + "$repo->getUser()".len()).unwrap();
        assert_eq!(name, "$user");
        assert_eq!(apply(src, edits), "<?php\nfunction f($repo)\n{\n    $user = $repo->getUser();\n    return $user->name;\n}\n");
    }

    #[test]
    fn extracts_a_method() {
        let src = "<?php\nclass A\n{\n    public function run(int $a)\n    {\n        $b = $a * 2;\n        $c = $b + 1;\n        return $c;\n    }\n}\n";
        let doc = Document::parse(Path::new("/p/A.php"), src).unwrap();
        let start = src.find("$b = ").unwrap();
        let end = src.find("$c = $b + 1;").unwrap() + "$c = $b + 1;".len();
        let out = apply(src, extract_method(&doc, start..end, "compute").unwrap());
        assert!(out.contains("        $c = $this->compute($a);\n        return $c;"), "{out}");
        assert!(out.contains("    private function compute($a)\n    {\n        $b = $a * 2;\n        $c = $b + 1;\n        return $c;\n    }"), "{out}");
    }

    #[test]
    fn inlines_a_variable() {
        let src = "<?php\nfunction f($u)\n{\n    $name = $u->name;\n    echo $name;\n    return $name . '!';\n}\n";
        let doc = Document::parse(Path::new("/p/a.php"), src).unwrap();
        let out = apply(src, inline_variable(&doc, src.find("echo $name").unwrap() + 6).unwrap());
        assert_eq!(out, "<?php\nfunction f($u)\n{\n    echo $u->name;\n    return $u->name . '!';\n}\n");
    }

    #[test]
    fn introduces_constants_and_fields_and_deletes() {
        let src = "<?php\nclass A\n{\n    public function f()\n    {\n        return 'admin' . $this->repo->load();\n    }\n\n    /** Old. */\n    public function unused(): void {}\n}\n";
        let doc = Document::parse(Path::new("/p/A.php"), src).unwrap();
        let at = src.find("'admin'").unwrap();
        let (edits, name) = introduce_constant(&doc, at..at + 7).unwrap();
        assert_eq!(name, "ADMIN");
        let out = apply(src, edits);
        assert!(out.contains("{\n    private const ADMIN = 'admin';\n") && out.contains("return self::ADMIN . "), "{out}");

        let at = src.find("$this->repo->load()").unwrap();
        let (edits, name) = introduce_field(&doc, at..at + "$this->repo->load()".len()).unwrap();
        assert_eq!(name, "load");
        let out = apply(src, edits);
        assert!(out.contains("    private $load;") && out.contains("        $this->load = $this->repo->load();\n        return 'admin' . $this->load;"), "{out}");

        let at = src.find("unused").unwrap();
        let out = apply(src, vec![delete_declaration(&doc, at..at + 6).unwrap()]);
        assert_eq!(out, "<?php\nclass A\n{\n    public function f()\n    {\n        return 'admin' . $this->repo->load();\n    }\n}\n");
    }
}
