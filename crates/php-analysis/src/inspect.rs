//! Code inspections: problems found by reading one file against the index. Each one stays
//! quiet when it can't be sure (unknown types, magic `__call` / `__get`, `compact`/`extract`).

use std::collections::{HashMap, HashSet};
use std::ops::Range;

use php_index::{Index, MemberKind};
use tree_sitter::Node;

use crate::Document;
use crate::types::{is_function_like, scope_class, type_of, visit};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Error,
    Warning,
    /// Unused code: drawn greyed rather than underlined in red.
    Weak,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Problem {
    pub range: Range<usize>,
    pub severity: Severity,
    pub message: String,
    /// Stable name of the inspection (`undefined-method`, `unused-variable`…).
    pub id: &'static str,
}

/// Built-in functions of the PHP runtime (lowercase). Empty when unknown: then undefined
/// functions aren't reported.
#[derive(Debug, Default, Clone)]
pub struct Builtins {
    pub functions: HashSet<String>,
}

/// Runs every inspection on `doc`.
pub fn inspect(doc: &Document, index: &Index, builtins: &Builtins) -> Vec<Problem> {
    let mut out = Vec::new();
    let root = doc.tree.root_node();
    visit(root, &mut |node| {
        match node.kind() {
            "member_call_expression" | "nullsafe_member_call_expression" => undefined_member(doc, index, node, true, &mut out),
            "member_access_expression" | "nullsafe_member_access_expression" => undefined_member(doc, index, node, false, &mut out),
            "scoped_call_expression" => undefined_static(doc, index, node, &mut out),
            "function_call_expression" => function_call(doc, index, builtins, node, &mut out),
            "compound_statement" => unreachable(doc, node, &mut out),
            "array_creation_expression" => duplicate_keys(doc, node, &mut out),
            kind if is_function_like(kind) => {
                function_body(doc, node, &mut out);
                if matches!(kind, "method_declaration" | "function_definition") {
                    missing_return(doc, node, &mut out);
                }
            }
            "class_declaration" | "trait_declaration" | "enum_declaration" => unused_private(doc, node, &mut out),
            _ => {}
        }
        true
    });
    unused_imports(doc, &mut out);
    out.sort_by_key(|p| p.range.start);
    out
}

fn problem(range: Range<usize>, severity: Severity, id: &'static str, message: String) -> Problem {
    Problem { range, severity, message, id }
}

/// Whether members of `class` can't be checked: magic method `magic`, mixins, facades
/// (`@see`), traits and interfaces (where inference usually stops short of the real class),
/// or an ancestor missing from the index.
fn has_magic(index: &Index, class: &str, magic: &str) -> bool {
    if index.find_member(class, magic, &[MemberKind::Method]).is_some() {
        return true;
    }
    // Laravel facades forward every static call.
    if class.contains("\\Facades\\") {
        return true;
    }
    let ancestors = index.ancestors(class);
    let Some(own) = index.class(class) else { return true };
    if matches!(own.kind, php_index::ClassKind::Trait | php_index::ClassKind::Interface) {
        return true;
    }
    let known: HashSet<String> = ancestors.iter().map(|a| a.fqn.to_lowercase()).collect();
    let unresolved = ancestors.iter().flat_map(|a| a.extends.iter().chain(&a.implements).chain(&a.traits)).any(|p| !known.contains(&p.to_lowercase()));
    unresolved || ancestors.iter().any(|a| !a.mixins.is_empty() || !a.see.is_empty() || a.fqn.eq_ignore_ascii_case("stdClass"))
}

fn undefined_member(doc: &Document, index: &Index, node: Node, call: bool, out: &mut Vec<Problem>) {
    let (Some(object), Some(name)) = (node.child_by_field_name("object"), node.child_by_field_name("name")) else { return };
    if name.kind() != "name" {
        return;
    }
    // A property access that is really the callee of `$a->b()` is checked as a call.
    if !call && node.parent().is_some_and(|p| p.kind().contains("call_expression") && p.child_by_field_name("function").is_some_and(|f| f.id() == node.id())) {
        return;
    }
    let Some(class) = type_of(doc, index, object, 0) else { return };
    if index.class(&class).is_none() {
        return;
    }
    let text = doc.text(name);
    let (kinds, magic) = if call { (&[MemberKind::Method][..], "__call") } else { (&[MemberKind::Property][..], "__get") };
    match index.find_member(&class, text, kinds) {
        Some((_, member)) if call && !member.virtual_member => check_arguments(doc, node, &member.detail, text, out),
        Some(_) => {}
        None if !has_magic(index, &class, magic) => {
            let short = class.rsplit('\\').next().unwrap_or(&class);
            out.push(problem(name.byte_range(), Severity::Warning, if call { "undefined-method" } else { "undefined-property" }, format!("{} '{text}' not found in {short}", if call { "Method" } else { "Property" })));
        }
        None => {}
    }
}

fn undefined_static(doc: &Document, index: &Index, node: Node, out: &mut Vec<Problem>) {
    let (Some(scope), Some(name)) = (node.child_by_field_name("scope"), node.child_by_field_name("name")) else { return };
    if name.kind() != "name" {
        return;
    }
    let Some(class) = scope_class(doc, index, scope) else { return };
    if index.class(&class).is_none() {
        return;
    }
    let text = doc.text(name);
    match index.find_member(&class, text, &[MemberKind::Method]) {
        Some((_, member)) if !member.virtual_member => check_arguments(doc, node, &member.detail, text, out),
        Some(_) => {}
        None if !has_magic(index, &class, "__callStatic") && !has_magic(index, &class, "__call") => {
            let short = class.rsplit('\\').next().unwrap_or(&class);
            out.push(problem(name.byte_range(), Severity::Warning, "undefined-method", format!("Method '{text}' not found in {short}")));
        }
        None => {}
    }
}

/// (required, maximum) parameter counts from a signature like `(int $a, $b = 1, ...$c)`.
fn arity(signature: &str) -> Option<(usize, Option<usize>)> {
    let open = signature.find('(')?;
    let mut depth = 0;
    let mut close = None;
    for (i, c) in signature[open..].char_indices() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => {
                depth -= 1;
                if depth == 0 {
                    close = Some(open + i);
                    break;
                }
            }
            _ => {}
        }
    }
    let params = signature[open + 1..close?].trim();
    if params.is_empty() {
        return Some((0, Some(0)));
    }
    let mut parts = Vec::new();
    let (mut depth, mut start) = (0, 0);
    for (i, c) in params.char_indices() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => depth -= 1,
            ',' if depth == 0 => {
                parts.push(&params[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&params[start..]);
    let parts: Vec<&str> = parts.into_iter().map(str::trim).filter(|p| !p.is_empty()).collect();
    let variadic = parts.iter().any(|p| p.contains("..."));
    let required = parts.iter().take_while(|p| !p.contains('=') && !p.contains("...")).count();
    Some((required, (!variadic).then_some(parts.len())))
}

fn check_arguments(doc: &Document, call: Node, signature: &str, name: &str, out: &mut Vec<Problem>) {
    let Some(args) = call.child_by_field_name("arguments") else { return };
    let mut cursor = args.walk();
    let list: Vec<Node> = args.named_children(&mut cursor).collect();
    // Spread (`...$args`) and named arguments can't be counted.
    if list.iter().any(|a| doc.text(*a).starts_with("...") || a.child_by_field_name("name").is_some()) {
        return;
    }
    let Some((required, max)) = arity(signature) else { return };
    let given = list.len();
    if given < required {
        out.push(problem(args.byte_range(), Severity::Error, "argument-count", format!("'{name}' expects at least {required} argument(s), {given} given")));
    } else if max.is_some_and(|m| given > m) {
        out.push(problem(args.byte_range(), Severity::Error, "argument-count", format!("'{name}' expects at most {} argument(s), {given} given", max.unwrap_or(0))));
    }
}

fn function_call(doc: &Document, index: &Index, builtins: &Builtins, node: Node, out: &mut Vec<Problem>) {
    let Some(function) = node.child_by_field_name("function") else { return };
    if !matches!(function.kind(), "name" | "qualified_name") {
        return;
    }
    let name = doc.text(function);
    let found = doc.ctx.resolve_function(name).into_iter().find_map(|fqn| index.function(&fqn).cloned());
    match found {
        Some(info) => check_arguments(doc, node, &info.detail, name, out),
        None => {
            let short = name.rsplit('\\').next().unwrap_or(name).to_lowercase();
            if !builtins.functions.is_empty() && !builtins.functions.contains(&short) {
                out.push(problem(function.byte_range(), Severity::Error, "undefined-function", format!("Undefined function '{name}'")));
            }
        }
    }
}

/// Statements after `return`, `throw`, `exit`, `break` or `continue` in the same block.
fn unreachable(doc: &Document, block: Node, out: &mut Vec<Problem>) {
    let mut cursor = block.walk();
    let statements: Vec<Node> = block.named_children(&mut cursor).filter(|n| n.kind() != "comment").collect();
    for (i, statement) in statements.iter().enumerate() {
        let text = doc.text(*statement);
        let terminal = matches!(statement.kind(), "return_statement" | "break_statement" | "continue_statement")
            || (statement.kind() == "expression_statement" && (text.starts_with("throw ") || text.starts_with("exit") || text.starts_with("die")));
        if terminal && let Some(next) = statements.get(i + 1) {
            let last = statements.last().unwrap();
            out.push(problem(next.start_byte()..last.end_byte(), Severity::Warning, "unreachable-code", "Unreachable statement".into()));
            return;
        }
    }
}

fn duplicate_keys(doc: &Document, array: Node, out: &mut Vec<Problem>) {
    let mut seen = HashSet::new();
    let mut cursor = array.walk();
    for element in array.named_children(&mut cursor) {
        let mut c = element.walk();
        let parts: Vec<Node> = element.named_children(&mut c).collect();
        if parts.len() == 2 && matches!(parts[0].kind(), "string" | "encapsed_string" | "integer") {
            let key = doc.text(parts[0]).trim_matches(['\'', '"']).to_string();
            if !seen.insert(key.clone()) {
                out.push(problem(parts[0].byte_range(), Severity::Warning, "duplicate-key", format!("Duplicate array key '{key}'")));
            }
        }
    }
}

/// Nodes of `body` outside nested functions and closures.
fn own_nodes<'t>(body: Node<'t>, f: &mut impl FnMut(Node<'t>)) {
    visit(body, &mut |n| {
        if n.id() != body.id() && is_function_like(n.kind()) {
            // A closure's `use ($x)` reads the outer variable.
            if let Some(uses) = (0..n.named_child_count()).filter_map(|i| n.named_child(i)).find(|c| c.kind() == "anonymous_function_use_clause") {
                visit(uses, &mut |u| {
                    f(u);
                    true
                });
            }
            return false;
        }
        f(n);
        true
    });
}

/// Unused local variables and variables read without ever being assigned.
fn function_body(doc: &Document, function: Node, out: &mut Vec<Problem>) {
    let Some(body) = function.child_by_field_name("body") else { return };
    let body_text = doc.text(body);
    if ["compact(", "extract(", "$$", "get_defined_vars", "include", "require", "global ", "eval("].iter().any(|s| body_text.contains(s)) {
        return;
    }
    let mut params = HashSet::new();
    if let Some(list) = function.child_by_field_name("parameters") {
        visit(list, &mut |n| {
            if n.kind() == "variable_name" {
                params.insert(doc.text(n).to_string());
            }
            true
        });
    }
    // Arrow functions read the enclosing scope freely; closures get `use ($x)` variables.
    if function.kind() == "arrow_function" {
        return;
    }
    if let Some(uses) = (0..function.named_child_count()).filter_map(|i| function.named_child(i)).find(|c| c.kind() == "anonymous_function_use_clause") {
        visit(uses, &mut |n| {
            if n.kind() == "variable_name" {
                params.insert(doc.text(n).to_string());
            }
            true
        });
    }
    let mut writes: HashMap<String, Vec<Range<usize>>> = HashMap::new();
    let mut reads: HashMap<String, Vec<Range<usize>>> = HashMap::new();
    own_nodes(body, &mut |n| {
        if n.kind() != "variable_name" {
            return;
        }
        let name = doc.text(n).to_string();
        let parent = n.parent();
        // `self::$cache` is a static property, not a local variable.
        if name == "$this" || parent.is_some_and(|p| p.kind() == "scoped_property_access_expression") {
            return;
        }
        let is_write = parent.is_some_and(|p| {
            (p.kind() == "assignment_expression" && p.child_by_field_name("left").is_some_and(|l| l.id() == n.id()))
                || (p.kind() == "foreach_statement" && p.named_child(0).is_some_and(|first| first.id() != n.id()))
                || p.kind() == "pair" && p.parent().is_some_and(|g| g.kind() == "foreach_statement")
                || p.kind() == "static_variable_declaration"
                || p.kind() == "by_ref"
                || p.kind() == "list_literal"
                || (p.kind() == "array_element_initializer" && p.parent().is_some_and(|a| a.parent().is_some_and(|x| x.kind() == "assignment_expression" && x.child_by_field_name("left").is_some_and(|l| l.id() == a.id()))))
        });
        // Passed to a function: may be filled by reference (`preg_match($re, $s, $matches)`).
        let by_ref_arg = parent.is_some_and(|p| p.kind() == "argument");
        // `isset($x)` / `empty($x)` may name variables that don't exist.
        let in_isset = {
            let mut p = n.parent();
            let mut found = false;
            while let Some(node) = p {
                if matches!(node.kind(), "function_call_expression" | "expression_statement") {
                    break;
                }
                let t = doc.text(node);
                if t.starts_with("isset(") || t.starts_with("empty(") {
                    found = true;
                    break;
                }
                p = node.parent();
            }
            found
        };
        // `catch (E $e)` names the exception even when it isn't used.
        if parent.is_some_and(|p| p.kind() == "catch_clause") {
            params.insert(name.clone());
        }
        if is_write || by_ref_arg {
            writes.entry(name.clone()).or_default().push(n.byte_range());
        }
        if !is_write && !in_isset {
            reads.entry(name).or_default().push(n.byte_range());
        }
    });
    for (name, ranges) in &writes {
        if !reads.contains_key(name) && !params.contains(name) {
            for r in ranges {
                out.push(problem(r.clone(), Severity::Weak, "unused-variable", format!("Unused local variable '{name}'")));
            }
        }
    }
    for (name, ranges) in &reads {
        if !writes.contains_key(name) && !params.contains(name) && !matches!(name.as_str(), "$_GET" | "$_POST" | "$_SERVER" | "$_SESSION" | "$_COOKIE" | "$_FILES" | "$_REQUEST" | "$_ENV" | "$GLOBALS" | "$argv" | "$argc" | "$http_response_header") {
            for r in ranges {
                out.push(problem(r.clone(), Severity::Warning, "undefined-variable", format!("Undefined variable '{name}'")));
            }
        }
    }
}

/// A function declaring a return type that never returns a value.
fn missing_return(doc: &Document, function: Node, out: &mut Vec<Problem>) {
    let (Some(ret), Some(body)) = (function.child_by_field_name("return_type"), function.child_by_field_name("body")) else { return };
    let ret_text = doc.text(ret).trim_start_matches('?').to_lowercase();
    if matches!(ret_text.as_str(), "void" | "never" | "null") || ret_text.contains("generator") || ret_text.contains("iterable") {
        return;
    }
    let mut returns = false;
    own_nodes(body, &mut |n| {
        let text = doc.text(n);
        if (n.kind() == "return_statement" && n.named_child_count() > 0) || n.kind() == "throw_expression" || n.kind() == "yield_expression" || text.starts_with("throw ") || text.starts_with("exit") || text.starts_with("die") {
            returns = true;
        }
    });
    if !returns && let Some(name) = function.child_by_field_name("name") {
        out.push(problem(name.byte_range(), Severity::Warning, "missing-return", "Missing return statement".into()));
    }
}

/// Private methods, properties and constants never referenced in their class.
fn unused_private(doc: &Document, class: Node, out: &mut Vec<Problem>) {
    let Some(body) = class.child_by_field_name("body") else { return };
    let body_text = doc.text(body);
    let mut cursor = body.walk();
    for member in body.named_children(&mut cursor) {
        let text = doc.text(member);
        let is_private = text.split_whitespace().take(4).any(|w| w == "private");
        if !is_private {
            continue;
        }
        let names: Vec<(Range<usize>, String)> = match member.kind() {
            "method_declaration" => member.child_by_field_name("name").map(|n| vec![(n.byte_range(), doc.text(n).to_string())]).unwrap_or_default(),
            "property_declaration" => {
                let mut c = member.walk();
                member.named_children(&mut c).filter(|e| e.kind() == "property_element").filter_map(|e| e.child_by_field_name("name")).map(|n| (n.byte_range(), doc.text(n).trim_start_matches('$').to_string())).collect()
            }
            _ => Vec::new(),
        };
        for (range, name) in names {
            if name.starts_with("__") {
                continue;
            }
            let uses = body_text.match_indices(name.as_str()).filter(|(i, _)| {
                let before = body_text[..*i].chars().last();
                let after = body_text[i + name.len()..].chars().next();
                !before.is_some_and(|c| c.is_alphanumeric() || c == '_') && !after.is_some_and(|c| c.is_alphanumeric() || c == '_')
            }).count();
            if uses <= 1 {
                let what = if member.kind() == "method_declaration" { "method" } else { "property" };
                out.push(problem(range, Severity::Weak, "unused-private", format!("Unused private {what} '{name}'")));
            }
        }
    }
}

fn unused_imports(doc: &Document, out: &mut Vec<Problem>) {
    let used = crate::imports::used_names(doc);
    for item in &doc.symbols.uses {
        if !used.contains(&item.alias.to_ascii_lowercase()) {
            out.push(problem(item.decl_range.clone(), Severity::Weak, "unused-import", format!("Unused import '{}'", item.fqn)));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    const LIB: &str = "<?php\nnamespace App;\n\nclass Repo\n{\n    public int $count = 0;\n    public function find(int $id, bool $fresh = false): ?array { return null; }\n}\n\nfunction helper(string $a): string { return $a; }\n";

    fn run(src: &str) -> Vec<(&'static str, String)> {
        let mut index = Index::new();
        index.update_file(Path::new("/p/Repo.php"), LIB);
        let doc = Document::parse(Path::new("/p/a.php"), src).unwrap();
        let builtins = Builtins { functions: ["strlen", "count"].into_iter().map(String::from).collect() };
        inspect(&doc, &index, &builtins).into_iter().map(|p| (p.id, src[p.range].to_string())).collect()
    }

    #[test]
    fn finds_problems() {
        let src = "<?php\nnamespace App;\n\nuse Foo\\Unused;\n\nclass Svc\n{\n    private function never(): void {}\n\n    public function run(Repo $repo): int\n    {\n        $repo->missing();\n        $repo->find();\n        $repo->find(1, true, 3);\n        echo $repo->nope;\n        $tmp = 5;\n        helper('x');\n        nothere();\n        echo $undefinedVar;\n        $x = ['a' => 1, 'a' => 2];\n        return $x['a'];\n        echo 'dead';\n    }\n\n    public function total(): int\n    {\n        $n = 1;\n        echo $n;\n    }\n}\n";
        let found = run(src);
        let has = |id: &str, text: &str| found.iter().any(|(i, t)| *i == id && t == text);
        assert!(has("undefined-method", "missing"), "{found:?}");
        assert!(has("argument-count", "()"), "{found:?}");
        assert!(has("argument-count", "(1, true, 3)"), "{found:?}");
        assert!(has("undefined-property", "nope"), "{found:?}");
        assert!(has("unused-variable", "$tmp"), "{found:?}");
        assert!(has("undefined-function", "nothere"), "{found:?}");
        assert!(has("undefined-variable", "$undefinedVar"), "{found:?}");
        assert!(has("duplicate-key", "'a'"), "{found:?}");
        assert!(has("unreachable-code", "echo 'dead';"), "{found:?}");
        assert!(has("missing-return", "total"), "{found:?}");
        assert!(has("unused-private", "never"), "{found:?}");
        assert!(found.iter().any(|(i, _)| *i == "unused-import"), "{found:?}");
        // Correct code stays quiet.
        assert!(!has("undefined-method", "find"));
        assert!(!found.iter().any(|(i, t)| *i == "undefined-function" && (t == "helper" || t == "strlen")));
    }

    /// Survey on a real project: `INSPECT_ROOT=/path cargo test -p php-analysis survey -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn survey() {
        let Some(root) = std::env::var_os("INSPECT_ROOT") else { return };
        let root = std::path::PathBuf::from(root);
        let index = Index::scan(&root);
        let builtins = Builtins::default();
        let mut counts: HashMap<&str, usize> = HashMap::new();
        let mut samples: Vec<String> = Vec::new();
        let mut stack = vec![root.join("app"), root.join("routes"), root.join("database")];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else { continue };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().is_some_and(|e| e == "php") {
                    let src = std::fs::read_to_string(&path).unwrap_or_default();
                    let Some(doc) = Document::parse(&path, &src) else { continue };
                    for p in inspect(&doc, &index, &builtins) {
                        *counts.entry(p.id).or_default() += 1;
                        if p.id != "unused-import" && samples.iter().filter(|s| s.starts_with(p.id)).count() < 6 {
                            let line = src[..p.range.start].matches('\n').count() + 1;
                            samples.push(format!("{} {}:{} {}", p.id, path.strip_prefix(&root).unwrap().display(), line, p.message));
                        }
                    }
                }
            }
        }
        println!("{counts:#?}");

        for s in samples {
            println!("{s}");
        }
    }
}
