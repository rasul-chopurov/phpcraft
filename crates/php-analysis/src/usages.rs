//! Find Usages: the symbol under the caret and its declaration, and every reference to that
//! declaration in a file (a name that resolves to it through Go to Declaration).

use std::ops::Range;

use php_index::{Index, Location};
use tree_sitter::Node;

use crate::types::{enclosing_function, visit};
use crate::{Document, goto_definition};

/// A symbol to find: the name to look for in files, and where it is declared.
#[derive(Debug, Clone, PartialEq)]
pub struct Symbol {
    /// Text to search for (`title` for `$title` properties and `->title` accesses).
    pub name: String,
    pub declaration: Location,
    /// Local variables and parameters are only used inside one function of one file.
    pub local: bool,
}

const DECLARATIONS: &[&str] = &[
    "class_declaration",
    "interface_declaration",
    "trait_declaration",
    "enum_declaration",
    "method_declaration",
    "function_definition",
    "const_element",
    "enum_case",
];

/// The symbol at `offset`: its declaration, when the caret is on one, or what it resolves to.
pub fn symbol_at(doc: &Document, index: &Index, offset: usize) -> Option<Symbol> {
    for at in [offset, offset.saturating_sub(1)] {
        let Some(node) = doc.tree.root_node().descendant_for_byte_range(at, at) else { continue };
        let node = if node.kind() == "name" && node.parent().is_some_and(|p| p.kind() == "variable_name") { node.parent()? } else { node };
        match node.kind() {
            "name" if node.parent().is_some_and(|p| DECLARATIONS.contains(&p.kind())) => {
                return Some(Symbol { name: doc.text(node).to_string(), declaration: doc.location(node.byte_range()), local: false });
            }
            "variable_name" => {
                let in_property = node.parent().is_some_and(|p| p.kind() == "property_element");
                let promoted = node.parent().is_some_and(|p| p.kind() == "property_promotion_parameter");
                let text = doc.text(node);
                if in_property || promoted {
                    return Some(Symbol { name: text.trim_start_matches('$').to_string(), declaration: doc.location(node.byte_range()), local: false });
                }
                if text == "$this" {
                    continue;
                }
                // A local variable: its first occurrence is the declaration.
                let declaration = goto_definition(doc, index, node.start_byte(), &[]).into_iter().next().unwrap_or_else(|| doc.location(node.byte_range()));
                let is_property = declaration.path != doc.path || doc.source.get(declaration.range.clone()) != Some(text);
                return Some(Symbol { name: text.trim_start_matches('$').to_string(), declaration, local: !is_property });
            }
            "name" | "relative_scope" => {
                if let Some(declaration) = goto_definition(doc, index, node.start_byte(), &[]).into_iter().next() {
                    let name = doc.text(node).to_string();
                    return Some(Symbol { name, declaration, local: false });
                }
            }
            _ => {}
        }
    }
    None
}

/// Byte ranges in `doc` that refer to `symbol` (including its declaration when it is here).
pub fn usages_in(doc: &Document, index: &Index, symbol: &Symbol) -> Vec<Range<usize>> {
    let same = |loc: &Location| loc.path == symbol.declaration.path && loc.range == symbol.declaration.range;
    let is_declaration = |range: &Range<usize>| doc.path == symbol.declaration.path && *range == symbol.declaration.range;
    let root = if symbol.local {
        if doc.path != symbol.declaration.path {
            return Vec::new();
        }
        let decl = doc.tree.root_node().descendant_for_byte_range(symbol.declaration.range.start, symbol.declaration.range.start);
        decl.and_then(enclosing_function).unwrap_or(doc.tree.root_node())
    } else {
        doc.tree.root_node()
    };
    let mut found = Vec::new();
    visit(root, &mut |node: Node| {
        let kind = node.kind();
        let text = doc.text(node);
        let candidate = match kind {
            "name" => text == symbol.name && node.parent().is_none_or(|p| p.kind() != "variable_name"),
            "variable_name" => text.trim_start_matches('$') == symbol.name,
            _ => false,
        };
        if candidate {
            let range = node.byte_range();
            if is_declaration(&range) || goto_definition(doc, index, node.start_byte(), &[]).iter().any(same) {
                found.push(range);
            }
            return false;
        }
        true
    });
    found.sort_by_key(|r| r.start);
    found.dedup();
    found
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    const A: &str = "<?php\nnamespace App;\n\nclass Cart\n{\n    public int $total = 0;\n\n    public function add(int $amount): void\n    {\n        $this->total += $amount;\n        $amount = 0;\n    }\n}\n";
    const B: &str = "<?php\nnamespace App;\n\nfunction checkout(Cart $cart)\n{\n    $cart->add(5);\n    return $cart->total;\n}\n";

    fn index() -> Index {
        let mut index = Index::new();
        index.update_file(Path::new("/p/Cart.php"), A);
        index.update_file(Path::new("/p/checkout.php"), B);
        index
    }

    fn texts(src: &str, ranges: &[Range<usize>]) -> Vec<String> {
        ranges.iter().map(|r| src[r.clone()].to_string()).collect()
    }

    #[test]
    fn finds_method_property_and_class_usages() {
        let index = index();
        let a = Document::parse(Path::new("/p/Cart.php"), A).unwrap();
        let b = Document::parse(Path::new("/p/checkout.php"), B).unwrap();

        // From the declaration of `add`.
        let symbol = symbol_at(&a, &index, A.find("add(").unwrap() + 1).unwrap();
        assert_eq!(symbol.name, "add");
        assert_eq!(texts(A, &usages_in(&a, &index, &symbol)), ["add"]);
        assert_eq!(texts(B, &usages_in(&b, &index, &symbol)), ["add"]);

        // From a use of the property.
        let symbol = symbol_at(&b, &index, B.find("->total").unwrap() + 3).unwrap();
        assert_eq!(symbol.name, "total");
        assert_eq!(texts(A, &usages_in(&a, &index, &symbol)), ["$total", "total"]);

        // The class, from a type hint.
        let symbol = symbol_at(&b, &index, B.find("Cart $").unwrap() + 1).unwrap();
        assert_eq!(texts(A, &usages_in(&a, &index, &symbol)), ["Cart"]);
        assert_eq!(texts(B, &usages_in(&b, &index, &symbol)), ["Cart"]);
    }

    #[test]
    fn local_variables_stay_in_their_function() {
        let index = index();
        let a = Document::parse(Path::new("/p/Cart.php"), A).unwrap();
        let symbol = symbol_at(&a, &index, A.rfind("$amount = 0").unwrap() + 2).unwrap();
        assert!(symbol.local);
        assert_eq!(usages_in(&a, &index, &symbol).len(), 3);
        let b = Document::parse(Path::new("/p/checkout.php"), B).unwrap();
        assert!(usages_in(&b, &index, &symbol).is_empty());
    }
}
