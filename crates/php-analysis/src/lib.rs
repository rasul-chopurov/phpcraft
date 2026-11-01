//! Position-based PHP features: go to declaration, completion, imports.
//!
//! Framework support (Laravel) plugs in through [`StringProvider`], which handles
//! string literals such as `route('home')` or `view('users.index')`.

mod complete;
mod docs;
mod generate;
mod hierarchy;
pub mod matcher;
mod goto;
mod imports;
mod inspect;
mod intentions;
mod refactor;
mod types;
mod usages;

use std::ops::Range;
use std::path::Path;

use php_index::{FileSymbols, Index, Location, NameContext, extract};
use tree_sitter::{Node, Tree};

pub use complete::{CompletionItem, CompletionKind, complete};
pub use goto::{goto_definition, reference_range};
pub use docs::{Documentation, documentation, parameter_info};
pub use intentions::{Intention, intentions, remove_unused_variable};
pub use inspect::{Builtins, Problem, Severity, inspect};
pub use refactor::{extract_method, inline_variable, introduce_variable};
pub use hierarchy::{InheritanceMark, inheritance_marks, implementations, related_tests, super_declaration, type_hierarchy};
pub use generate::{generate_accessors, generate_constructor, generate_phpdoc, implement_methods, overridable_methods, override_method};
pub use usages::{Symbol, caller_name, symbol_at, usages_in};
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
    /// Not inside a call (e.g. an array returned from `rules()`).
    None,
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
    /// The string is an element (or element value) of an array literal.
    pub in_array: bool,
    /// Key of the nearest enclosing keyed array element: `'email'` in
    /// `['email' => 'required|email']` or `['email' => ['required']]`.
    pub array_key: Option<String>,
    pub enclosing_method: Option<String>,
    pub enclosing_class: Option<String>,
    /// Class named by the enclosing class's `$model` property (Filament resources, factories).
    pub class_model: Option<String>,
    /// Caret position (byte offset in the document).
    pub offset: usize,
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
    // Step out of (nested) array literals, remembering the nearest element key.
    let mut in_array = false;
    let mut array_key = None;
    let mut child = string;
    let mut parent = string.parent()?;
    while parent.kind() == "array_element_initializer" {
        in_array = true;
        if array_key.is_none()
            && parent.named_child_count() == 2
            && parent.named_child(1).is_some_and(|v| v.id() == child.id())
            && let Some(key) = parent.named_child(0).filter(|k| matches!(k.kind(), "string" | "encapsed_string"))
        {
            array_key = Some(doc.text(key).trim_matches(['\'', '"']).to_string());
        }
        child = parent.parent()?;
        parent = child.parent()?;
    }
    let (callee, arg_index, call) = match parent.kind() {
        "argument" => {
            let arguments = parent.parent()?;
            let mut cursor = arguments.walk();
            let arg_index = arguments.named_children(&mut cursor).position(|a| a.id() == parent.id())?;
            let call = arguments.parent()?;
            let callee = match call.kind() {
                "function_call_expression" => {
                    Callee::Function(doc.text(call.child_by_field_name("function")?).to_ascii_lowercase())
                }
                "member_call_expression" | "nullsafe_member_call_expression" => Callee::Method {
                    class: call.child_by_field_name("object").and_then(|o| types::type_of(doc, index, o, 0)),
                    name: doc.text(call.child_by_field_name("name")?).to_string(),
                },
                "scoped_call_expression" => Callee::Static {
                    class: call.child_by_field_name("scope").and_then(|s| types::scope_class(doc, index, s)),
                    name: doc.text(call.child_by_field_name("name")?).to_string(),
                },
                _ => return None,
            };
            (callee, arg_index, Some(call))
        }
        _ if in_array => (Callee::None, 0, None),
        _ => return None,
    };
    let enclosing_method = types::enclosing_function(string)
        .filter(|f| f.kind() == "method_declaration")
        .and_then(|f| f.child_by_field_name("name"))
        .map(|n| doc.text(n).to_string());
    let enclosing_class = doc.current_class(offset);
    Some(StringArg {
        value: doc.source[range.clone()].to_string(),
        range,
        callee,
        arg_index,
        chain_root: call.and_then(|call| chain_root(doc, index, call)),
        in_array,
        array_key,
        enclosing_method,
        class_model: class_model(doc, offset),
        enclosing_class,
        offset,
    })
}

/// `protected static ?string $model = Post::class;` in the class around `offset`.
fn class_model(doc: &Document, offset: usize) -> Option<String> {
    let mut node = doc.tree.root_node().descendant_for_byte_range(offset, offset)?;
    while !php_index::is_class_like(node.kind()) {
        node = node.parent()?;
    }
    let body = node.child_by_field_name("body")?;
    let mut cursor = body.walk();
    for item in body.named_children(&mut cursor) {
        if item.kind() != "property_declaration" {
            continue;
        }
        let mut inner = item.walk();
        for element in item.named_children(&mut inner) {
            let is_model = element.child_by_field_name("name").is_some_and(|n| doc.text(n) == "$model");
            if let (true, Some(value)) = (is_model, element.child_by_field_name("default_value")) {
                let class = doc.text(value).strip_suffix("::class")?;
                return doc.ctx.resolve_class(class);
            }
        }
    }
    None
}

/// The file a string literal under `offset` points to: `__DIR__ . '/../x.php'`,
/// `base_path('routes/web.php')` (helpers from `helper_dirs`), or a path relative to the
/// file's directory or one of `roots`. Only existing files are returned.
pub fn file_path_at(
    doc: &Document,
    offset: usize,
    helper_dirs: &[(&str, std::path::PathBuf)],
    roots: &[std::path::PathBuf],
) -> Option<std::path::PathBuf> {
    let mut node = doc.tree.root_node().descendant_for_byte_range(offset, offset)?;
    while !matches!(node.kind(), "string" | "encapsed_string") {
        node = node.parent()?;
    }
    let raw = doc.text(node);
    let value = raw.trim_matches(['\'', '"']);
    if value.is_empty() || value.contains('\n') || value.len() > 300 {
        return None;
    }
    let dir = doc.path.parent().map(std::path::Path::to_path_buf).unwrap_or_default();
    let mut bases: Vec<std::path::PathBuf> = Vec::new();
    let parent = node.parent();
    // `__DIR__ . '/x'` / `dirname(__FILE__) . '/x'`
    if let Some(p) = parent.filter(|p| p.kind() == "binary_expression")
        && let Some(left) = p.child_by_field_name("left")
        && left.id() != node.id()
    {
        let left = doc.text(left).replace(' ', "");
        if left == "__DIR__" || left == "dirname(__FILE__)" {
            bases.push(dir.clone());
        } else if let Some(rest) = left.strip_prefix("dirname(__DIR__").and_then(|r| r.strip_suffix(')')) {
            // dirname(__DIR__) or dirname(__DIR__, 2)
            let levels: usize = rest.trim_start_matches(',').parse().unwrap_or(1);
            let mut base = dir.clone();
            for _ in 0..levels {
                base.pop();
            }
            bases.push(base);
        }
    }
    // `base_path('routes/web.php')`
    if let Some(call) = parent.and_then(|p| p.parent()).and_then(|args| args.parent())
        && call.kind() == "function_call_expression"
        && let Some(function) = call.child_by_field_name("function")
    {
        let name = doc.text(function).trim_start_matches('\\').to_ascii_lowercase();
        if let Some((_, base)) = helper_dirs.iter().find(|(f, _)| *f == name) {
            bases.push(base.clone());
        }
    }
    if bases.is_empty() {
        if !(value.contains('/') || value.contains('.')) {
            return None;
        }
        bases.push(dir);
        bases.extend(roots.iter().cloned());
    }
    bases.into_iter().find_map(|base| {
        let candidate = base.join(value.trim_start_matches('/'));
        let candidate = std::fs::canonicalize(&candidate).ok()?;
        candidate.is_file().then_some(candidate)
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
