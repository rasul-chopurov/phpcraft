//! Position-based PHP features: go to declaration, completion, imports.
//!
//! Framework support (Laravel) plugs in through [`StringProvider`], which handles
//! string literals such as `route('home')` or `view('users.index')`.

mod complete;
mod goto;
mod imports;
mod types;

use std::ops::Range;
use std::path::Path;

use php_index::{FileSymbols, Index, Location, NameContext, extract};
use tree_sitter::{Node, Tree};

pub use complete::{CompletionItem, CompletionKind, complete};
pub use goto::{goto_definition, reference_range};
pub use imports::{
    TextEdit, Unresolved, class_insertion, import_edit, optimize_imports, unresolved_classes,
};

/// A parsed file plus its declarations and name context.
pub struct Document<'a> {
    pub path: &'a Path,
    pub source: &'a str,
    pub tree: Tree,
    pub symbols: FileSymbols,
    pub ctx: NameContext,
}

impl<'a> Document<'a> {
    pub fn parse(path: &'a Path, source: &'a str) -> Option<Self> {
        let tree = php_syntax::parse(source)?;
        let symbols = extract(path, source, &tree);
        let ctx = symbols.name_context();
        Some(Self {
            path,
            source,
            tree,
            symbols,
            ctx,
        })
    }

    pub fn text(&self, node: Node) -> &'a str {
        &self.source[node.byte_range()]
    }

    /// FQN of the class enclosing `offset`.
    pub fn current_class(&self, offset: usize) -> Option<String> {
        self.symbols.class_at(&self.tree, offset).map(|c| c.fqn.clone())
    }

    pub fn location(&self, range: Range<usize>) -> Location {
        Location {
            path: self.path.to_path_buf(),
            range,
        }
    }
}

/// What a string literal is an argument of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Callee {
    /// `view('x')`; name is lowercased as written.
    Function(String),
    /// `$x->route('x')`; class is the inferred receiver type.
    Method { class: Option<String>, name: String },
    /// `Route::view('x')`.
    Static { class: Option<String>, name: String },
}

/// A string literal argument under the cursor.
#[derive(Debug, Clone)]
pub struct StringArg {
    pub value: String,
    /// Byte range of the string contents (without quotes).
    pub range: Range<usize>,
    pub callee: Callee,
    pub arg_index: usize,
    /// Class at the start of a call chain (`User` in `User::where(..)->orderBy('x')`),
    /// for query builders that don't carry the model type.
    pub chain_root: Option<String>,
}

/// Framework-specific handling of string literals (routes, views, config keys...).
pub trait StringProvider: Send + Sync {
    fn goto(&self, arg: &StringArg) -> Vec<Location>;
    fn complete(&self, arg: &StringArg) -> Vec<CompletionItem>;
}

/// Finds the string literal argument containing `offset`, if any.
pub fn string_arg_at(doc: &Document, index: &Index, offset: usize) -> Option<StringArg> {
    let mut node = doc.tree.root_node().descendant_for_byte_range(offset, offset)?;
    while !matches!(node.kind(), "string" | "encapsed_string") {
        node = node.parent()?;
    }
    let string = node;
    let full = string.byte_range();
    let quote = doc.source[full.clone()].find(['\'', '"'])?;
    let range = full.start + quote + 1..full.end.saturating_sub(1).max(full.start + quote + 1);
    if offset < range.start || offset > range.end {
        return None;
    }
    let argument = string.parent().filter(|p| p.kind() == "argument")?;
    let arguments = argument.parent()?;
    let mut cursor = arguments.walk();
    let arg_index = arguments
        .named_children(&mut cursor)
        .position(|a| a.id() == argument.id())?;
    let call = arguments.parent()?;
    let callee = match call.kind() {
        "function_call_expression" => {
            Callee::Function(doc.text(call.child_by_field_name("function")?).to_ascii_lowercase())
        }
        "member_call_expression" | "nullsafe_member_call_expression" => Callee::Method {
            class: call
                .child_by_field_name("object")
                .and_then(|o| types::type_of(doc, index, o, 0)),
            name: doc.text(call.child_by_field_name("name")?).to_string(),
        },
        "scoped_call_expression" => Callee::Static {
            class: call
                .child_by_field_name("scope")
                .and_then(|s| types::scope_class(doc, index, s)),
            name: doc.text(call.child_by_field_name("name")?).to_string(),
        },
        _ => return None,
    };
    Some(StringArg {
        value: doc.source[range.clone()].to_string(),
        range,
        callee,
        arg_index,
        chain_root: chain_root(doc, index, call),
    })
}

/// Walks `a()->b()->c()` back to its first receiver and resolves its class.
fn chain_root(doc: &Document, index: &Index, call: Node) -> Option<String> {
    let mut node = call;
    loop {
        match node.kind() {
            "member_call_expression" | "nullsafe_member_call_expression" | "member_access_expression" => {
                node = node.child_by_field_name("object")?;
            }
            "scoped_call_expression" => return types::scope_class(doc, index, node.child_by_field_name("scope")?),
            "variable_name" | "object_creation_expression" => return types::type_of(doc, index, node, 0),
            _ => return None,
        }
    }
}

impl StringArg {
    /// Matches a global helper call like `view(...)` (with or without leading `\`).
    pub fn is_function(&self, names: &[&str]) -> bool {
        matches!(&self.callee, Callee::Function(f) if names.contains(&f.trim_start_matches('\\')))
    }

    /// Matches a static call whose class short name is one of `classes`, e.g. `Route::view`.
    pub fn is_static(&self, classes: &[&str], methods: &[&str]) -> bool {
        matches!(&self.callee, Callee::Static { class: Some(c), name }
            if classes.iter().any(|k| php_index::short_name(c).eq_ignore_ascii_case(k))
                && methods.iter().any(|m| m.eq_ignore_ascii_case(name)))
    }

    pub fn is_method(&self, methods: &[&str]) -> bool {
        matches!(&self.callee, Callee::Method { name, .. } | Callee::Static { name, .. }
            if methods.iter().any(|m| m.eq_ignore_ascii_case(name)))
    }
}

#[cfg(test)]
mod tests;
