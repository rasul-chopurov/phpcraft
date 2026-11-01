//! Quick Documentation (the declaration's signature and doc comment) and Parameter Info (the
//! signature of the call around the caret, with the argument being typed).

use php_index::{Index, Location, MemberKind};
use tree_sitter::Node;

use crate::{Document, goto_definition, symbol_at};

#[derive(Debug, Clone, PartialEq)]
pub struct Documentation {
    /// The declaration as written, up to its body: `public function find(int $id): ?User`.
    pub signature: String,
    /// The doc comment's text without `/** */` and leading `*`.
    pub comment: String,
    pub location: Location,
}

/// Source of a file: `doc` itself when the location is in it, otherwise read from disk.
fn source_of(doc: &Document, path: &std::path::Path) -> Option<String> {
    if path == doc.path { Some(doc.source.to_string()) } else { std::fs::read_to_string(path).ok() }
}

/// The declaration text around `range` (a name) from its line start to `{`, `;` or `=`.
fn signature_at(source: &str, range: &std::ops::Range<usize>) -> String {
    let start = source[..range.start].rfind('\n').map_or(0, |i| i + 1);
    let rest = &source[start..];
    let mut depth = 0i32;
    let mut end = rest.len();
    for (i, c) in rest.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            '{' | ';' if depth <= 0 => {
                end = i;
                break;
            }
            '=' if depth <= 0 && i > range.end - start => {
                end = i;
                break;
            }
            _ => {}
        }
    }
    rest[..end].split_whitespace().collect::<Vec<_>>().join(" ").replace("( ", "(").replace(" )", ")")
}

/// The `/** */` comment right above the line containing `at`, cleaned up.
fn doc_comment_before(source: &str, at: usize) -> String {
    let line_start = source[..at].rfind('\n').map_or(0, |i| i + 1);
    let before = source[..line_start].trim_end();
    // Attributes (`#[Route(...)]`) may sit between the comment and the declaration.
    let mut before = before;
    while before.ends_with(']') {
        match before.rfind("#[") {
            Some(i) => before = before[..i].trim_end(),
            None => break,
        }
    }
    if !before.ends_with("*/") {
        return String::new();
    }
    let Some(start) = before.rfind("/**") else { return String::new() };
    before[start + 3..before.len() - 2]
        .lines()
        .map(|l| l.trim().trim_start_matches('*').trim())
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Quick Documentation for the symbol at `offset`.
pub fn documentation(doc: &Document, index: &Index, offset: usize) -> Option<Documentation> {
    let location = match goto_definition(doc, index, offset, &[]).into_iter().next() {
        Some(location) => location,
        None => symbol_at(doc, index, offset)?.declaration,
    };
    let source = source_of(doc, &location.path)?;
    Some(Documentation {
        signature: signature_at(&source, &location.range),
        comment: doc_comment_before(&source, location.range.start),
        location,
    })
}

/// Parameter Info: the parameter list of the call around `offset` and the index of the
/// argument the caret is in.
pub fn parameter_info(doc: &Document, index: &Index, offset: usize) -> Option<(String, usize)> {
    let mut node = doc.tree.root_node().descendant_for_byte_range(offset, offset)?;
    while node.kind() != "arguments" || offset <= node.start_byte() || offset >= node.end_byte() {
        node = node.parent()?;
    }
    let call = node.parent()?;
    let active = {
        let mut cursor = node.walk();
        node.children(&mut cursor).filter(|c| c.kind() == "," && c.end_byte() <= offset).count()
    };
    let name: Node = match call.kind() {
        "function_call_expression" => call.child_by_field_name("function")?,
        "member_call_expression" | "nullsafe_member_call_expression" | "scoped_call_expression" => call.child_by_field_name("name")?,
        "object_creation_expression" => {
            let mut cursor = call.walk();
            let class = call.named_children(&mut cursor).find(|c| matches!(c.kind(), "name" | "qualified_name"))?;
            let loc = goto_definition(doc, index, class.end_byte().saturating_sub(1), &[]).into_iter().next()?;
            let fqn = index.classes().find(|c| *c.path == *loc.path && c.range == loc.range)?.fqn.clone();
            let (_, ctor) = index.find_member(&fqn, "__construct", &[MemberKind::Method])?;
            return Some((params_of(&ctor.detail), active));
        }
        _ => return None,
    };
    let location = goto_definition(doc, index, name.end_byte().saturating_sub(1), &[]).into_iter().next()?;
    let source = source_of(doc, &location.path)?;
    Some((params_of(&signature_at(&source, &location.range)), active))
}

/// `int $id, array $opts = []` from a signature.
fn params_of(signature: &str) -> String {
    let Some(open) = signature.find('(') else { return String::new() };
    let mut depth = 0;
    for (i, c) in signature[open..].char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return signature[open + 1..open + i].trim().to_string();
                }
            }
            _ => {}
        }
    }
    signature[open + 1..].to_string()
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    const SRC: &str = "<?php\nclass Repo\n{\n    /**\n     * Finds a record.\n     *\n     * @param int $id The key.\n     */\n    public function find(int $id, array $opts = []): ?array\n    {\n        return null;\n    }\n\n    public function __construct(string $table) {}\n}\n\nfunction demo()\n{\n    $r = new Repo('users');\n    $r->find(1, ['a']);\n}\n";

    #[test]
    fn docs_and_parameters() {
        let mut index = Index::new();
        index.update_file(Path::new("/p/Repo.php"), SRC);
        let doc = Document::parse(Path::new("/p/Repo.php"), SRC).unwrap();
        let d = documentation(&doc, &index, SRC.rfind("find(").unwrap() + 1).unwrap();
        assert_eq!(d.signature, "public function find(int $id, array $opts = []): ?array");
        assert_eq!(d.comment, "Finds a record.\n@param int $id The key.");
        let at = SRC.rfind("['a']").unwrap();
        assert_eq!(parameter_info(&doc, &index, at), Some(("int $id, array $opts = []".to_string(), 1)));
        let at = SRC.rfind("'users'").unwrap();
        assert_eq!(parameter_info(&doc, &index, at), Some(("string $table".to_string(), 0)));
    }
}
