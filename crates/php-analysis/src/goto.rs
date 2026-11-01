//! Go to declaration (⌘B / ⌘+click).

use php_index::{Index, Location, MemberKind, UseKind};
use tree_sitter::Node;

use crate::types::{enclosing_function, is_function_like, scope_class, type_of, visit};
use crate::{Document, StringProvider, string_arg_at};

/// Declarations for the symbol at `offset`. Tries `offset - 1` too, so a caret placed
/// right after an identifier still resolves.
pub fn goto_definition(
    doc: &Document,
    index: &Index,
    offset: usize,
    providers: &[&dyn StringProvider],
) -> Vec<Location> {
    for at in [offset, offset.saturating_sub(1)] {
        let found = goto_at(doc, index, at, providers);
        if !found.is_empty() {
            return found;
        }
    }
    Vec::new()
}

/// Byte range of the identifier or string under `offset` that ⌘+hover should underline.
pub fn reference_range(doc: &Document, offset: usize) -> Option<std::ops::Range<usize>> {
    let node = doc.tree.root_node().descendant_for_byte_range(offset, offset)?;
    match node.kind() {
        "name" | "variable_name" | "string_content" => Some(node.byte_range()),
        _ => None,
    }
}

fn goto_at(doc: &Document, index: &Index, offset: usize, providers: &[&dyn StringProvider]) -> Vec<Location> {
    let Some(node) = doc.tree.root_node().descendant_for_byte_range(offset, offset) else {
        return Vec::new();
    };
    if matches!(node.kind(), "string_content" | "string" | "encapsed_string" | "'" | "\"") {
        return goto_string(doc, index, offset, providers);
    }
    if node.kind() == "variable_name" || node.parent().is_some_and(|p| p.kind() == "variable_name") {
        let var = if node.kind() == "variable_name" { node } else { node.parent().unwrap() };
        return goto_variable(doc, index, var).into_iter().collect();
    }
    if !matches!(node.kind(), "name" | "relative_scope") {
        return Vec::new();
    }
    goto_name(doc, index, node).into_iter().collect()
}

fn class_loc(index: &Index, fqn: Option<String>) -> Option<Location> {
    index.class_location(&fqn?)
}

fn member_loc(index: &Index, class: Option<String>, name: &str, kinds: &[MemberKind]) -> Option<Location> {
    let (owner, member) = index.find_member(&class?, name, kinds)?;
    Some(Location {
        path: owner.path.to_path_buf(),
        range: member.range,
    })
}

fn goto_name(doc: &Document, index: &Index, node: Node) -> Option<Location> {
    // Widen `Foo` inside `\App\Foo` to the whole qualified name.
    let mut target = node;
    while let Some(parent) = target.parent() {
        if matches!(parent.kind(), "qualified_name" | "namespace_name") {
            target = parent;
        } else {
            break;
        }
    }
    let parent = target.parent()?;
    let name = doc.text(target);
    let is_field = |field: &str| parent.child_by_field_name(field).is_some_and(|n| n.id() == target.id());

    match parent.kind() {
        "namespace_use_clause" => {
            let item = doc.symbols.uses.iter().find(|u| {
                u.decl_range.contains(&target.start_byte())
                    && (u.fqn.ends_with(name.trim_start_matches('\\')) || u.alias == name)
            })?;
            match item.kind {
                UseKind::Function => index.function(&item.fqn).map(|f| Location {
                    path: f.path.to_path_buf(),
                    range: f.range.clone(),
                }),
                _ => index.class_location(&item.fqn),
            }
        }
        "member_call_expression" | "nullsafe_member_call_expression" if is_field("name") => {
            let class = type_of(doc, index, parent.child_by_field_name("object")?, 0);
            member_loc(index, class, name, &[MemberKind::Method])
        }
        "member_access_expression" | "nullsafe_member_access_expression" if is_field("name") => {
            let class = type_of(doc, index, parent.child_by_field_name("object")?, 0);
            member_loc(index, class, name, &[MemberKind::Property])
        }
        "scoped_call_expression" if is_field("name") => {
            let class = scope_class(doc, index, parent.child_by_field_name("scope")?);
            member_loc(index, class, name, &[MemberKind::Method])
        }
        "class_constant_access_expression" if parent.named_child(1).is_some_and(|n| n.id() == target.id()) => {
            let class = scope_class(doc, index, parent.named_child(0)?);
            if name == "class" {
                return class_loc(index, class);
            }
            member_loc(index, class, name, &[MemberKind::Constant, MemberKind::Case])
        }
        "function_call_expression" if is_field("function") => doc
            .ctx
            .resolve_function(name)
            .iter()
            .find_map(|fqn| index.function(fqn))
            .map(|f| Location {
                path: f.path.to_path_buf(),
                range: f.range.clone(),
            }),
        "class_declaration" | "interface_declaration" | "trait_declaration" | "enum_declaration"
        | "method_declaration" | "function_definition" => None,
        // Any other position is a class reference: types, `new`, `extends`, scopes...
        _ => class_loc(index, scope_class(doc, index, target)),
    }
}

fn goto_variable(doc: &Document, index: &Index, var: Node) -> Option<Location> {
    let name = doc.text(var);
    if name == "$this" {
        return class_loc(index, doc.current_class(var.start_byte()));
    }
    // `User::$instance`
    if let Some(parent) = var.parent()
        && parent.kind() == "scoped_property_access_expression"
    {
        let class = scope_class(doc, index, parent.child_by_field_name("scope")?);
        return member_loc(index, class, name.trim_start_matches('$'), &[MemberKind::Property]);
    }
    // First occurrence in the enclosing function: a parameter or the first assignment.
    let scope = enclosing_function(var).unwrap_or(doc.tree.root_node());
    let mut first = None;
    if let Some(params) = scope.child_by_field_name("parameters") {
        visit(params, &mut |n| {
            if first.is_none() && n.kind() == "variable_name" && doc.text(n) == name {
                first = Some(n);
            }
            first.is_none()
        });
    }
    if first.is_none() {
        visit(scope, &mut |n| {
            if first.is_none() && n.kind() == "variable_name" && doc.text(n) == name {
                first = Some(n);
            }
            first.is_none() && (n.id() == scope.id() || !is_function_like(n.kind()))
        });
    }
    let first = first?;
    (first.id() != var.id()).then(|| doc.location(first.byte_range()))
}

fn goto_string(doc: &Document, index: &Index, offset: usize, providers: &[&dyn StringProvider]) -> Vec<Location> {
    if let Some(found) = callable_array_target(doc, index, offset) {
        return vec![found];
    }
    let Some(arg) = string_arg_at(doc, index, offset) else {
        return Vec::new();
    };
    providers.iter().flat_map(|p| p.goto(&arg)).collect()
}

/// `[UserController::class, 'index']` -> `UserController::index`.
fn callable_array_target(doc: &Document, index: &Index, offset: usize) -> Option<Location> {
    let mut node = doc.tree.root_node().descendant_for_byte_range(offset, offset)?;
    while !matches!(node.kind(), "string" | "encapsed_string") {
        node = node.parent()?;
    }
    let element = node.parent().filter(|p| p.kind() == "array_element_initializer")?;
    let array = element.parent().filter(|p| p.kind() == "array_creation_expression")?;
    if array.named_child_count() != 2 || array.named_child(1)?.id() != element.id() {
        return None;
    }
    let class_expr = array.named_child(0)?.named_child(0)?;
    if class_expr.kind() != "class_constant_access_expression" || doc.text(class_expr.named_child(1)?) != "class" {
        return None;
    }
    let class = scope_class(doc, index, class_expr.named_child(0)?);
    let method = doc.text(node).trim_matches(['\'', '"']);
    member_loc(index, class, method, &[MemberKind::Method])
}
