//! Move Class (F6): moves a class declaration to another namespace and file, rewrites the
//! `use` statements that point at it, and imports what the moved body now needs.

use std::ops::Range;

use php_index::{Index, UseKind, namespace_of, short_name};

use crate::imports::{class_references, insert_imports, used_imports};
use crate::{Document, TextEdit};

/// The class declaration found under the caret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Moving {
    pub name: String,
    pub fqn: String,
    pub namespace: String,
    /// The whole declaration with its docblock and attributes.
    pub range: Range<usize>,
    /// The class name itself (the rename target of find-usages).
    pub name_range: Range<usize>,
    pub path: std::path::PathBuf,
}

/// The class-like declaration at `offset`, when it sits in the global scope of the file.
pub fn moving_class(doc: &Document, offset: usize) -> Option<Moving> {
    let mut node = doc.tree.root_node().descendant_for_byte_range(offset, offset)?;
    loop {
        if php_index::is_class_like(node.kind()) {
            break;
        }
        node = node.parent()?;
    }
    if node.parent().is_some_and(|p| p.kind() != "program") {
        return None;
    }
    let name = node.child_by_field_name("name")?;
    let name_range = name.byte_range();
    let fqn = doc
        .symbols
        .classes
        .iter()
        .find(|c| c.range == name_range)
        .map(|c| c.fqn.clone())
        .or_else(|| doc.ctx.resolve_class(doc.text(name)))?;
    let start = declared_range(doc, &node).start;
    Some(Moving {
        name: doc.text(name).to_string(),
        namespace: doc.ctx.namespace.clone(),
        fqn,
        range: start..node.end_byte(),
        name_range,
        path: doc.path.to_path_buf(),
    })
}

/// The range of a declaration with the docblock above it (attributes are part of the node).
fn declared_range(doc: &Document, node: &tree_sitter::Node) -> Range<usize> {
    let mut start = node.start_byte();
    let mut sibling = node.prev_named_sibling();
    while let Some(prev) = sibling {
        if prev.kind() == "comment" && doc.text(prev).starts_with("/**") {
            start = prev.start_byte();
            sibling = prev.prev_named_sibling();
            continue;
        }
        break;
    }
    start..node.end_byte()
}

/// Whether `name` appears in `text` as a whole word (PHP names are case-insensitive).
fn mentions(text: &str, name: &str) -> bool {
    let text = text.to_ascii_lowercase();
    let name = name.to_ascii_lowercase();
    let word = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '\\');
    let mut from = 0;
    while let Some(at) = text[from..].find(name.as_str()) {
        let start = from + at;
        let end = start + name.len();
        if !word(text[..start].chars().next_back()) && !word(text[end..].chars().next()) {
            return true;
        }
        from = end;
    }
    false
}

/// Declarations in the same file that the class needs and that nothing else there refers to: the
/// trait it uses, the interface it implements, the enum its properties are typed with. They move
/// together with the class instead of staying behind in the old namespace.
pub fn companions(doc: &Document, class: &Moving) -> Vec<Moving> {
    let mut cursor = doc.tree.root_node().walk();
    let siblings: Vec<(Range<usize>, String, Range<usize>)> = doc
        .tree
        .root_node()
        .children(&mut cursor)
        .into_iter()
        .filter(|node| php_index::is_class_like(node.kind()))
        .filter_map(|node| {
            let name = node.child_by_field_name("name")?;
            let name_range = name.byte_range();
            Some((declared_range(doc, &node), doc.text(name).to_string(), name_range))
        })
        .filter(|(_, _, name_range)| *name_range != class.name_range)
        .collect();
    let class_text = &doc.source[class.range.clone()];
    let mut out = Vec::new();
    for (range, name, name_range) in &siblings {
        if !mentions(class_text, name) {
            continue;
        }
        // Another declaration of the file uses it too, or the class does not: leave it alone.
        let used_elsewhere = siblings
            .iter()
            .any(|(other, _, _)| other != range && mentions(&doc.source[other.clone()], name));
        if used_elsewhere {
            continue;
        }
        let Some(fqn) = doc
            .symbols
            .classes
            .iter()
            .find(|c| c.range == *name_range)
            .map(|c| c.fqn.clone())
        else {
            continue;
        };
        out.push(Moving {
            name: name.clone(),
            fqn,
            namespace: doc.ctx.namespace.clone(),
            range: range.clone(),
            name_range: name_range.clone(),
            path: doc.path.to_path_buf(),
        });
    }
    out
}

/// The file the class moves to: header, the imports the moved declarations need, then them in the
/// order they had in the old file.
pub fn moved_source(doc: &Document, index: &Index, class: &Moving, companions: &[Moving], new_namespace: &str) -> String {
    let mut moved: Vec<&Moving> = std::iter::once(class).chain(companions.iter()).collect();
    moved.sort_by_key(|m| m.range.start);
    let mut bodies = Vec::new();
    for item in &moved {
        let mut body = doc.source[item.range.clone()].to_string();
        // `Foo\Bar` was resolved from the old namespace: qualify it, an import would not help.
        let mut rewrites: Vec<(Range<usize>, String)> = Vec::new();
        for node in class_references(doc) {
            let range = node.byte_range();
            if range.start < item.range.start || range.end > item.range.end {
                continue;
            }
            let text = doc.text(node);
            if text.contains('\\')
                && !text.starts_with('\\')
                && let Some(fqn) = doc.ctx.resolve_class(text)
                && !moved.iter().any(|m| fqn.eq_ignore_ascii_case(&m.fqn))
            {
                rewrites.push((range.start - item.range.start..range.end - item.range.start, format!("\\{fqn}")));
            }
        }
        rewrites.sort_by_key(|(r, _)| std::cmp::Reverse(r.start));
        for (range, text) in rewrites {
            body.replace_range(range, &text);
        }
        bodies.push(body.trim_end().to_string());
    }
    let ranges: Vec<Range<usize>> = moved.iter().map(|m| m.range.clone()).collect();
    let mut imports: Vec<String> = used_imports(doc, index, &ranges)
        .into_iter()
        .filter(|fqn| {
            !moved.iter().any(|m| fqn.eq_ignore_ascii_case(&m.fqn))
                && !namespace_of(fqn).eq_ignore_ascii_case(new_namespace)
        })
        .collect();
    imports.sort_by_key(|fqn| fqn.to_ascii_lowercase());
    imports.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    let mut out = String::from("<?php\n");
    if !new_namespace.is_empty() {
        out.push_str(&format!("\nnamespace {new_namespace};\n"));
    }
    if !imports.is_empty() {
        out.push('\n');
        for fqn in &imports {
            out.push_str(&format!("use {fqn};\n"));
        }
    }
    out.push('\n');
    out.push_str(&bodies.join("\n\n"));
    out.push('\n');
    out
}

/// The edit that deletes the declaration (with its docblock) from the old file.
pub fn removal_edit(doc: &Document, class: &Moving) -> TextEdit {
    let start = doc.source[..class.range.start].rfind('\n').map_or(0, |i| i + 1);
    let mut end = class.range.end;
    while end < doc.source.len() && matches!(doc.source.as_bytes()[end], b' ' | b'\t' | b'\r') {
        end += 1;
    }
    if doc.source[end..].starts_with('\n') {
        end += 1;
    }
    TextEdit {
        range: start..end,
        text: String::new(),
    }
}

/// Edits that keep the references to `class` in `doc` working after it moves to `new_fqn`:
/// rewritten `use` statements, an import for unqualified uses, and fully qualified names
/// where the short name belongs to another class.
pub fn reference_edits(doc: &Document, class: &Moving, new_fqn: &str) -> Vec<TextEdit> {
    let mut edits = Vec::new();
    for item in doc
        .symbols
        .uses
        .iter()
        .filter(|u| u.kind == UseKind::Class && u.fqn.eq_ignore_ascii_case(&class.fqn))
    {
        let decl = doc.source.get(item.decl_range.clone()).unwrap_or_default();
        let lower = decl.to_ascii_lowercase();
        // Group imports (`use App\{A, B};`) name only the tail: they are left alone.
        if let Some(at) = lower.find(&class.fqn.to_ascii_lowercase()) {
            let start = item.decl_range.start + at;
            edits.push(TextEdit {
                range: start..start + class.fqn.len(),
                text: new_fqn.to_string(),
            });
        }
    }
    let short = short_name(new_fqn).to_string();
    let new_ns = namespace_of(new_fqn).to_string();
    let decl_here = doc
        .symbols
        .classes
        .iter()
        .any(|c| c.name.eq_ignore_ascii_case(&short) && !c.fqn.eq_ignore_ascii_case(&class.fqn));
    let mut additions: Vec<String> = Vec::new();
    for node in class_references(doc) {
        let range = node.byte_range();
        if doc.path == class.path && range.start >= class.range.start && range.end <= class.range.end {
            continue;
        }
        let text = doc.text(node);
        if !doc.ctx.resolve_class(text).is_some_and(|fqn| fqn.eq_ignore_ascii_case(&class.fqn)) {
            continue;
        }
        if text.starts_with('\\') || text.contains('\\') {
            edits.push(TextEdit {
                range,
                text: format!("\\{new_fqn}"),
            });
        } else {
            match doc.ctx.imported_class(text) {
                // The `use` rewrite above keeps this name valid.
                Some(imported) if imported.eq_ignore_ascii_case(&class.fqn) => {}
                Some(_) => edits.push(TextEdit {
                    range,
                    text: format!("\\{new_fqn}"),
                }),
                None if new_ns.eq_ignore_ascii_case(&doc.ctx.namespace) => {}
                None if decl_here || doc.ctx.imported_class(&short).is_some() => edits.push(TextEdit {
                    range,
                    text: format!("\\{new_fqn}"),
                }),
                None => additions.push(new_fqn.to_string()),
            }
        }
    }
    if !additions.is_empty() {
        additions.sort_by_key(|fqn| fqn.to_ascii_lowercase());
        additions.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
        let refs: Vec<&str> = additions.iter().map(String::as_str).collect();
        if let Some(edit) = insert_imports(doc, &refs) {
            edits.push(edit);
        }
    }
    edits
}
