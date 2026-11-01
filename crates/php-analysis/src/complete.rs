//! Code completion.

use std::collections::HashSet;
use std::ops::Range;

use php_index::{Index, MemberKind, short_name};
use tree_sitter::Node;

use crate::imports::{TextEdit, class_insertion};
use crate::types::{enclosing_function, is_function_like, scope_class, type_of, visit};
use crate::{Document, StringProvider, string_arg_at};

const MAX_ITEMS: usize = 80;

const KEYWORDS: &[&str] = &[
    "abstract", "array", "as", "break", "case", "catch", "class", "clone", "const", "continue",
    "declare", "default", "do", "echo", "else", "elseif", "enum", "extends", "final", "finally",
    "fn", "for", "foreach", "function", "global", "if", "implements", "include", "instanceof",
    "interface", "match", "namespace", "new", "null", "private", "protected", "public",
    "readonly", "require", "require_once", "return", "static", "switch", "throw", "trait", "try",
    "use", "while", "yield", "true", "false",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionKind {
    Class,
    Method,
    Property,
    Constant,
    Function,
    Variable,
    Keyword,
    Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionItem {
    pub label: String,
    pub detail: String,
    pub kind: CompletionKind,
    /// Text that replaces `replace`.
    pub insert: String,
    /// Byte range of the typed prefix.
    pub replace: Range<usize>,
    /// Extra edit applied with the completion (an auto-import).
    pub additional_edit: Option<TextEdit>,
}

impl CompletionItem {
    pub fn simple(label: impl Into<String>, detail: impl Into<String>, kind: CompletionKind, replace: Range<usize>) -> Self {
        let label = label.into();
        Self {
            insert: label.clone(),
            label,
            detail: detail.into(),
            kind,
            replace,
            additional_edit: None,
        }
    }
}

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '\\'
}

pub fn complete(
    doc: &Document,
    index: &Index,
    offset: usize,
    providers: &[&dyn StringProvider],
) -> Vec<CompletionItem> {
    let source = doc.source;
    let before = &source[..offset];
    let start = before
        .char_indices()
        .rev()
        .take_while(|(_, c)| is_ident(*c))
        .last()
        .map_or(offset, |(i, _)| i);
    let prefix = &source[start..offset];
    let replace = start..offset;

    if let Some(arg) = string_arg_at(doc, index, offset) {
        // Providers may replace only part of the string (e.g. one `|`-separated rule):
        // the typed prefix starts where their replacement starts.
        let typed_from = |item: &CompletionItem| item.replace.start.clamp(arg.range.start, offset);
        let mut items: Vec<_> = providers
            .iter()
            .flat_map(|p| p.complete(&arg))
            .filter(|item| matches(&item.label, &source[typed_from(item)..offset]))
            .collect();
        for item in &mut items {
            item.replace = typed_from(item)..offset;
        }
        let typed = items.first().map_or(&source[arg.range.start..offset], |i| &source[i.replace.clone()]);
        return rank(items, typed);
    }

    let lead = &before[..start];
    let items = if lead.ends_with('$') {
        variables(doc, offset, start - 1..offset)
    } else if let Some(object_end) = lead.strip_suffix("?->").or(lead.strip_suffix("->")).map(str::len) {
        expression_ending_at(doc, object_end)
            .and_then(|node| type_of(doc, index, node, 0))
            .map(|class| members(index, &class, false, replace.clone()))
            .unwrap_or_default()
    } else if let Some(scope_end) = lead.strip_suffix("::").map(str::len) {
        let scope = expression_ending_at(doc, scope_end);
        let resolved = scope.and_then(|node| scope_class(doc, index, node));
        match resolved.filter(|class| index.class(class).is_some()) {
            Some(class) => members(index, &class, true, replace.clone()),
            // `TextInput::` before the class is imported: find it by short name and
            // import it when a member is chosen.
            None => scope
                .map(|node| doc.text(node))
                .filter(|name| !name.contains('\\') && !name.starts_with('$'))
                .and_then(|name| index.classes_by_short_name(name).into_iter().next().cloned())
                .map(|class| {
                    let (_, import) = class_insertion(doc, &class.fqn);
                    let mut items = members(index, &class.fqn, true, replace.clone());
                    for item in &mut items {
                        item.additional_edit = import.clone();
                    }
                    items
                })
                .unwrap_or_default(),
        }
    } else if prefix.is_empty() || prefix.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        Vec::new()
    } else {
        global_items(doc, index, prefix, replace.clone())
    };
    let items = items.into_iter().filter(|i| matches(&i.label, prefix.trim_start_matches('$'))).collect();
    rank(items, prefix.trim_start_matches('$'))
}

/// Case-insensitive prefix or subsequence ("CamelHumps"-like) match.
fn matches(label: &str, typed: &str) -> bool {
    if typed.is_empty() {
        return true;
    }
    let label = label.trim_start_matches('$').to_ascii_lowercase();
    let typed = typed.to_ascii_lowercase();
    if label.starts_with(&typed) {
        return true;
    }
    let mut chars = label.chars();
    typed.chars().all(|t| chars.any(|c| c == t)) && label.starts_with(typed.chars().next().unwrap())
}

fn rank(mut items: Vec<CompletionItem>, typed: &str) -> Vec<CompletionItem> {
    let typed_lower = typed.to_ascii_lowercase();
    items.sort_by_key(|item| {
        let label = item.label.trim_start_matches('$');
        (
            !label.starts_with(typed),
            !label.to_ascii_lowercase().starts_with(&typed_lower),
            item.kind == CompletionKind::Keyword,
            label.len(),
            item.label.clone(),
        )
    });
    let mut seen = HashSet::new();
    items.retain(|i| seen.insert((i.label.clone(), i.detail.clone())));
    items.truncate(MAX_ITEMS);
    items
}

/// The largest expression ending exactly at `end` (the object before `->` or `::`).
fn expression_ending_at<'d>(doc: &'d Document, end: usize) -> Option<Node<'d>> {
    let mut node = doc.tree.root_node().descendant_for_byte_range(end.saturating_sub(1), end.saturating_sub(1))?;
    while !node.is_named() || node.end_byte() != end {
        node = node.parent()?;
    }
    while let Some(parent) = node.parent() {
        let expression = parent.kind().ends_with("_expression") && parent.kind() != "expression_statement";
        if parent.end_byte() == end && expression {
            node = parent;
        } else {
            break;
        }
    }
    // `$x->` where `$x` is the variable itself: climb from its inner `name` node.
    if node.kind() == "name" && node.parent().is_some_and(|p| p.kind() == "variable_name") {
        node = node.parent()?;
    }
    Some(node)
}

fn members(index: &Index, class: &str, is_static: bool, replace: Range<usize>) -> Vec<CompletionItem> {
    let mut items = Vec::new();
    if is_static {
        items.push(CompletionItem::simple("class", short_name(class), CompletionKind::Keyword, replace.clone()));
    }
    for (owner, member) in index.members(class) {
        let (kind, label, insert) = match member.kind {
            MemberKind::Method => {
                if member.name.starts_with("__") || (is_static && !member.is_static && !member.virtual_member) {
                    continue;
                }
                (CompletionKind::Method, member.name.clone(), format!("{}()", member.name))
            }
            MemberKind::Property => {
                if member.is_static != is_static {
                    continue;
                }
                let name = if is_static { format!("${}", member.name) } else { member.name.clone() };
                (CompletionKind::Property, name.clone(), name)
            }
            MemberKind::Constant | MemberKind::Case => {
                if !is_static {
                    continue;
                }
                (CompletionKind::Constant, member.name.clone(), member.name.clone())
            }
        };
        items.push(CompletionItem {
            label,
            detail: format!("{} {}", member.detail, owner.name).trim().to_string(),
            kind,
            insert,
            replace: replace.clone(),
            additional_edit: None,
        });
    }
    items
}

fn variables(doc: &Document, offset: usize, replace: Range<usize>) -> Vec<CompletionItem> {
    let Some(node) = doc.tree.root_node().descendant_for_byte_range(offset, offset) else {
        return Vec::new();
    };
    let scope = enclosing_function(node).unwrap_or(doc.tree.root_node());
    let mut names = Vec::new();
    if doc.current_class(offset).is_some() && scope.kind() != "function_definition" {
        names.push("$this".to_string());
    }
    visit(scope, &mut |n| {
        if n.kind() == "variable_name" && n.end_byte() < replace.start {
            names.push(doc.text(n).to_string());
        }
        n.id() == scope.id() || !is_function_like(n.kind()) || n.kind() == "arrow_function"
    });
    names
        .into_iter()
        .map(|n| CompletionItem::simple(n, "", CompletionKind::Variable, replace.clone()))
        .collect()
}

fn global_items(doc: &Document, index: &Index, prefix: &str, replace: Range<usize>) -> Vec<CompletionItem> {
    let mut items = Vec::new();
    let typed = prefix.trim_start_matches('\\');
    if !prefix.contains('\\') {
        let first = typed.chars().next().unwrap_or(' ').to_ascii_lowercase();
        for class in index.classes() {
            if !class.name.to_ascii_lowercase().starts_with(first) || !matches(&class.name, typed) {
                continue;
            }
            let (insert, additional_edit) = class_insertion(doc, &class.fqn);
            items.push(CompletionItem {
                label: class.name.clone(),
                detail: class.fqn.clone(),
                kind: CompletionKind::Class,
                insert,
                replace: replace.clone(),
                additional_edit,
            });
        }
        for function in index.functions() {
            let name = short_name(&function.fqn);
            if matches(name, typed) {
                items.push(CompletionItem {
                    label: name.to_string(),
                    detail: function.detail.clone(),
                    kind: CompletionKind::Function,
                    insert: format!("{name}()"),
                    replace: replace.clone(),
                    additional_edit: None,
                });
            }
        }
        for keyword in KEYWORDS {
            items.push(CompletionItem::simple(*keyword, "", CompletionKind::Keyword, replace.clone()));
        }
    } else {
        // Fully qualified typing: `\App\Mo` -> `\App\Models\User`.
        for class in index.classes() {
            if class.fqn.to_ascii_lowercase().starts_with(&typed.to_ascii_lowercase()) {
                let insert = format!("{}{}", if prefix.starts_with('\\') { "\\" } else { "" }, class.fqn);
                items.push(CompletionItem {
                    label: insert.clone(),
                    detail: String::new(),
                    kind: CompletionKind::Class,
                    insert,
                    replace: replace.clone(),
                    additional_edit: None,
                });
            }
        }
    }
    items
}
