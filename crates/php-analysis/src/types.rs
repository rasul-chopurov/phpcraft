//! Lightweight type inference for expressions, enough for member navigation and completion.

use php_index::{Index, MemberKind, class_type, doc_tag_type};
use tree_sitter::Node;

use crate::Document;

const MAX_DEPTH: usize = 8;

pub fn is_function_like(kind: &str) -> bool {
    matches!(
        kind,
        "method_declaration" | "function_definition" | "anonymous_function" | "arrow_function"
    )
}

pub fn enclosing_function(node: Node) -> Option<Node> {
    let mut current = node.parent();
    while let Some(n) = current {
        if is_function_like(n.kind()) {
            return Some(n);
        }
        current = n.parent();
    }
    None
}

/// Resolves the class named by a scope expression (`User`, `self`, `static`, `parent`, `$x`).
pub fn scope_class(doc: &Document, index: &Index, scope: Node) -> Option<String> {
    let text = doc.text(scope);
    let current = || doc.current_class(scope.start_byte());
    match text.to_ascii_lowercase().as_str() {
        "self" | "static" => current(),
        "parent" => {
            let class = current()?;
            index.class(&class)?.extends.first().cloned()
        }
        _ if scope.kind() == "variable_name" => type_of(doc, index, scope, 0),
        _ => doc.ctx.resolve_class(text),
    }
}

/// Infers the class of an expression node.
pub fn type_of(doc: &Document, index: &Index, node: Node, depth: usize) -> Option<String> {
    if depth > MAX_DEPTH {
        return None;
    }
    let member_type = |object: Option<Node>, name: Option<Node>, kinds: &[MemberKind]| {
        let class = type_of(doc, index, object?, depth + 1)?;
        let (_, member) = index.find_member(&class, doc.text(name?), kinds)?;
        member.type_fqn
    };
    match node.kind() {
        "variable_name" => variable_type(doc, index, node, depth),
        "object_creation_expression" => {
            let mut cursor = node.walk();
            let class = node
                .named_children(&mut cursor)
                .find(|n| matches!(n.kind(), "name" | "qualified_name" | "relative_scope"))?;
            scope_class(doc, index, class)
        }
        "member_call_expression" | "nullsafe_member_call_expression" => member_type(
            node.child_by_field_name("object"),
            node.child_by_field_name("name"),
            &[MemberKind::Method],
        ),
        "member_access_expression" | "nullsafe_member_access_expression" => member_type(
            node.child_by_field_name("object"),
            node.child_by_field_name("name"),
            &[MemberKind::Property],
        ),
        "scoped_call_expression" => {
            let class = scope_class(doc, index, node.child_by_field_name("scope")?)?;
            let name = doc.text(node.child_by_field_name("name")?);
            let (_, member) = index.find_member(&class, name, &[MemberKind::Method])?;
            member.type_fqn
        }
        "class_constant_access_expression" => {
            let scope = node.named_child(0)?;
            let class = scope_class(doc, index, scope)?;
            let name = doc.text(node.named_child(1)?);
            index
                .find_member(&class, name, &[MemberKind::Case])
                .and_then(|(_, m)| m.type_fqn)
        }
        "parenthesized_expression" => type_of(doc, index, node.named_child(0)?, depth + 1),
        "function_call_expression" => {
            let name = doc.text(node.child_by_field_name("function")?);
            doc.ctx
                .resolve_function(name)
                .iter()
                .find_map(|fqn| index.function(fqn))
                .and_then(|f| f.type_fqn.clone())
        }
        "name" | "qualified_name" | "relative_scope" => scope_class(doc, index, node),
        _ => None,
    }
}

fn variable_type(doc: &Document, index: &Index, var: Node, depth: usize) -> Option<String> {
    let name = doc.text(var);
    if name == "$this" {
        return doc.current_class(var.start_byte());
    }
    let function = enclosing_function(var)?;
    // Typed parameter.
    if let Some(params) = function.child_by_field_name("parameters") {
        let mut cursor = params.walk();
        for param in params.named_children(&mut cursor) {
            let Some(param_name) = param.child_by_field_name("name") else { continue };
            if doc.text(param_name) == name {
                let ty = param.child_by_field_name("type").map(|t| doc.text(t));
                let class = doc.current_class(var.start_byte()).unwrap_or_default();
                return ty.and_then(|t| class_type(t, &doc.ctx, &class));
            }
        }
    }
    // `/** @var Foo $x */` or the last assignment before the usage.
    let mut found = None;
    let mut doc_type = None;
    let body = function.child_by_field_name("body")?;
    visit(body, &mut |node| {
        if node.start_byte() >= var.start_byte() {
            return false;
        }
        match node.kind() {
            "comment" => {
                let text = doc.text(node);
                if text.contains(name)
                    && let Some(ty) = doc_tag_type(text, "@var")
                {
                    doc_type = Some(ty);
                }
            }
            "assignment_expression" => {
                if let (Some(left), Some(right)) =
                    (node.child_by_field_name("left"), node.child_by_field_name("right"))
                    && doc.text(left) == name
                {
                    found = Some(right);
                }
            }
            _ => {}
        }
        // Don't descend into nested closures with their own scope.
        !is_function_like(node.kind())
    });
    if let Some(ty) = doc_type {
        let class = doc.current_class(var.start_byte()).unwrap_or_default();
        if let Some(fqn) = class_type(&ty, &doc.ctx, &class) {
            return Some(fqn);
        }
    }
    type_of(doc, index, found?, depth + 1)
}

/// Pre-order traversal; the callback returns whether to descend into the node.
pub fn visit<'t>(node: Node<'t>, f: &mut impl FnMut(Node<'t>) -> bool) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if f(child) {
            visit(child, f);
        }
    }
}
