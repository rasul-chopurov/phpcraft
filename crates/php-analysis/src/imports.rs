//! `use` statement management: auto-import, "Import class" quick-fix, Optimize Imports.

use std::collections::HashSet;
use std::ops::Range;

use php_index::{Index, UseKind, namespace_of, short_name};
use tree_sitter::Node;

use crate::Document;
use crate::types::visit;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextEdit {
    /// Byte range in the document to replace.
    pub range: Range<usize>,
    pub text: String,
}

/// A class reference that resolves to nothing in the index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unresolved {
    pub range: Range<usize>,
    /// The name as written (first segment for qualified names).
    pub name: String,
    /// Indexed classes with that short name: candidates for "Import class".
    pub candidates: Vec<String>,
}

/// How to reference `fqn` from this document: the text to write, plus an import if needed.
/// Falls back to a fully qualified `\Name` when the short name is taken by another import.
pub fn class_insertion(doc: &Document, fqn: &str) -> (String, Option<TextEdit>) {
    let short = short_name(fqn).to_string();
    if let Some(imported) = doc.ctx.imported_class(&short) {
        return if imported.eq_ignore_ascii_case(fqn) {
            (short, None)
        } else {
            (format!("\\{fqn}"), None)
        };
    }
    if namespace_of(fqn).eq_ignore_ascii_case(&doc.ctx.namespace) {
        return (short, None);
    }
    (short, import_edit(doc, fqn))
}

/// Edits for "Import class": adds `use <fqn>;`, or qualifies the reference at `reference`
/// when its short name is already imported from another namespace.
pub fn import_class_at(doc: &Document, fqn: &str, reference: Range<usize>) -> Vec<TextEdit> {
    let (insert, edit) = class_insertion(doc, fqn);
    if insert.starts_with('\\') {
        return vec![TextEdit { range: reference, text: insert }];
    }
    edit.into_iter().collect()
}

/// The edit that adds `use <fqn>;` in sorted position, or `None` if nothing is needed.
pub fn import_edit(doc: &Document, fqn: &str) -> Option<TextEdit> {
    let fqn = fqn.trim_start_matches('\\');
    if doc.symbols.uses.iter().any(|u| u.kind == UseKind::Class && u.fqn.eq_ignore_ascii_case(fqn)) {
        return None;
    }
    if namespace_of(fqn).eq_ignore_ascii_case(&doc.ctx.namespace) {
        return None;
    }
    insert_imports(doc, &[fqn])
}

/// Whether a `name`/`qualified_name` node is a class reference (not a member, function or declaration name).
/// FQNs of the classes used by the code in `ranges`, for copy/paste: what the destination
/// file needs to import. Fully qualified references are skipped — they work as written.
pub fn used_imports(doc: &Document, index: &Index, ranges: &[Range<usize>]) -> Vec<String> {
    let mut out = Vec::new();
    for node in class_references(doc) {
        let range = node.byte_range();
        if !ranges.iter().any(|sel| sel.start <= range.start && range.end <= sel.end) {
            continue;
        }
        let text = doc.text(node);
        if text.starts_with('\\') {
            continue;
        }
        let Some(fqn) = doc.ctx.resolve_class(text) else { continue };
        if index.class(&fqn).is_none() || php_index::is_builtin_class(&fqn) {
            continue;
        }
        out.push(fqn);
    }
    out.sort_by_key(|fqn| fqn.to_ascii_lowercase());
    out.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    out
}

/// Edits that make the pasted `imports` resolve in `doc` (the text after the paste): the
/// `use` lines in sorted position, plus fully qualified references where the short name is
/// already taken by another import.
pub fn paste_edits(doc: &Document, imports: &[String], pasted: Range<usize>) -> Vec<TextEdit> {
    let mut additions: Vec<&str> = Vec::new();
    let mut edits = Vec::new();
    for fqn in imports {
        let fqn = fqn.trim_start_matches('\\');
        if namespace_of(fqn).eq_ignore_ascii_case(&doc.ctx.namespace) {
            continue;
        }
        match doc.ctx.imported_class(short_name(fqn)) {
            Some(imported) if imported.eq_ignore_ascii_case(fqn) => {}
            // The short name belongs to another class: keep the pasted code qualified.
            Some(_) => {
                for node in class_references(doc) {
                    let range = node.byte_range();
                    if pasted.start <= range.start
                        && range.end <= pasted.end
                        && doc.text(node).eq_ignore_ascii_case(short_name(fqn))
                    {
                        edits.push(TextEdit { range, text: format!("\\{fqn}") });
                    }
                }
            }
            None => additions.push(fqn),
        }
    }
    if !additions.is_empty() {
        additions.sort_by_key(|fqn| fqn.to_ascii_lowercase());
        additions.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
        if let Some(edit) = insert_imports(doc, &additions) {
            edits.push(edit);
        }
    }
    edits
}

/// Adds several `use` lines as one block, keeping classes before `use function` / `use const`.
pub(crate) fn insert_imports(doc: &Document, fqns: &[&str]) -> Option<TextEdit> {
    let uses = &doc.symbols.uses;
    let block = fqns.iter().map(|fqn| format!("use {fqn};")).collect::<Vec<_>>().join("\n");
    let largest = fqns.last()?.to_ascii_lowercase();
    let class_uses: Vec<_> = uses.iter().filter(|u| u.kind == UseKind::Class).collect();
    if let Some(next) = class_uses
        .iter()
        .find(|u| u.fqn.to_ascii_lowercase() > largest)
    {
        let at = next.decl_range.start;
        return Some(TextEdit { range: at..at, text: format!("{block}\n") });
    }
    if let Some(last) = class_uses.last() {
        let at = last.decl_range.end;
        return Some(TextEdit { range: at..at, text: format!("\n{block}") });
    }
    if let Some(first) = uses.iter().min_by_key(|u| u.decl_range.start) {
        let at = first.decl_range.start;
        return Some(TextEdit { range: at..at, text: format!("{block}\n") });
    }
    if let Some(ns) = &doc.symbols.namespace_range {
        let at = ns.end;
        return Some(TextEdit { range: at..at, text: format!("\n\n{block}") });
    }
    let at = doc.source.find("<?php").map(|i| i + 5)?;
    Some(TextEdit { range: at..at, text: format!("\n\n{block}") })
}

/// Whether a `name`/`qualified_name` node is a class reference (not a member, function or declaration name).
fn is_class_reference(doc: &Document, node: Node) -> bool {
    let Some(parent) = node.parent() else { return false };
    let is_field = |field: &str| parent.child_by_field_name(field).is_some_and(|n| n.id() == node.id());
    match parent.kind() {
        "named_type" | "base_clause" | "class_interface_clause" | "object_creation_expression"
        | "use_declaration" | "attribute" | "type_list" | "binary_expression" => {
            !(parent.kind() == "binary_expression" && !doc.text(parent).contains("instanceof"))
        }
        "scoped_call_expression" | "scoped_property_access_expression" => is_field("scope"),
        "class_constant_access_expression" => parent.named_child(0).is_some_and(|n| n.id() == node.id()),
        _ => false,
    }
}

pub(crate) fn class_references<'d>(doc: &'d Document) -> Vec<Node<'d>> {
    let mut out = Vec::new();
    visit(doc.tree.root_node(), &mut |node| {
        if matches!(node.kind(), "name" | "qualified_name") && is_class_reference(doc, node) {
            out.push(node);
            return false;
        }
        node.kind() != "namespace_use_declaration"
    });
    out
}

pub fn unresolved_classes(doc: &Document, index: &Index) -> Vec<Unresolved> {
    class_references(doc)
        .into_iter()
        .filter_map(|node| {
            let text = doc.text(node);
            let fqn = doc.ctx.resolve_class(text)?;
            if index.class(&fqn).is_some() || php_index::is_builtin_class(&fqn) {
                return None;
            }
            let name = text.split('\\').find(|s| !s.is_empty()).unwrap_or(text).to_string();
            let candidates = if text.starts_with('\\') {
                Vec::new()
            } else {
                index
                    .classes_by_short_name(&name)
                    .into_iter()
                    .map(|c| c.fqn.clone())
                    .collect()
            };
            Some(Unresolved {
                range: node.byte_range(),
                name,
                candidates,
            })
        })
        .collect()
}

/// Names (lowercased first segments) referenced in code or docblocks.
pub(crate) fn used_names(doc: &Document) -> HashSet<String> {
    let mut used = HashSet::new();
    visit(doc.tree.root_node(), &mut |node| {
        match node.kind() {
            "namespace_use_declaration" => return false,
            "name" | "qualified_name" => {
                let text = doc.text(node).trim_start_matches('\\');
                if !doc.text(node).starts_with('\\')
                    && let Some(first) = text.split('\\').next()
                {
                    used.insert(first.to_ascii_lowercase());
                }
                return false;
            }
            "comment" => {
                for word in doc.text(node).split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '\\')) {
                    if let Some(first) = word.split('\\').find(|s| !s.is_empty()) {
                        used.insert(first.to_ascii_lowercase());
                    }
                }
            }
            _ => {}
        }
        true
    });
    used
}

/// Removes unused imports and sorts the rest (classes, then functions, then constants).
pub fn optimize_imports(doc: &Document) -> Option<TextEdit> {
    let uses = &doc.symbols.uses;
    let start = uses.iter().map(|u| u.decl_range.start).min()?;
    let end = uses.iter().map(|u| u.decl_range.end).max()?;
    let used = used_names(doc);
    let mut kept: Vec<_> = uses
        .iter()
        .filter(|u| used.contains(&u.alias.to_ascii_lowercase()))
        .collect();
    kept.sort_by_key(|u| (u.kind as u8, u.fqn.to_ascii_lowercase()));
    kept.dedup_by(|a, b| a.kind == b.kind && a.fqn.eq_ignore_ascii_case(&b.fqn) && a.alias == b.alias);
    let text = kept
        .iter()
        .map(|u| {
            let keyword = match u.kind {
                UseKind::Class => "use",
                UseKind::Function => "use function",
                UseKind::Const => "use const",
            };
            if u.has_explicit_alias {
                format!("{keyword} {} as {};", u.fqn, u.alias)
            } else {
                format!("{keyword} {};", u.fqn)
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    // Removing every import also swallows the blank line after the block.
    let end = if text.is_empty() {
        end + doc.source[end..].len() - doc.source[end..].trim_start_matches('\n').len()
    } else {
        end
    };
    (doc.source[start..end] != text).then(|| TextEdit { range: start..end, text })
}
