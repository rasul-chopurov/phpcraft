//! Navigation through the class hierarchy: Go to Implementation(s), Super Method, Go to Test,
//! and the Type Hierarchy of a class.

use std::sync::Arc;

use php_index::{ClassInfo, ClassKind, Index, Location, Member, MemberKind};

use crate::{Document, symbol_at};

/// The class (and member) a declaration location belongs to.
fn owner_of(index: &Index, location: &Location) -> Option<(Arc<ClassInfo>, Option<Member>)> {
    index.classes().find_map(|class| {
        let source = |m: &Member| m.source.as_deref().unwrap_or(&class.path).to_path_buf();
        if *class.path == *location.path && class.range == location.range {
            return Some((class.clone(), None));
        }
        class
            .members
            .iter()
            .find(|m| m.range == location.range && source(m) == location.path)
            .map(|m| (class.clone(), Some(m.clone())))
    })
}

/// The class and member at the caret (on a declaration or a reference).
fn target(doc: &Document, index: &Index, offset: usize) -> Option<(Arc<ClassInfo>, Option<Member>)> {
    if let Some(symbol) = symbol_at(doc, index, offset)
        && let Some(found) = owner_of(index, &symbol.declaration)
    {
        return Some(found);
    }
    // Anywhere inside a class body: the class itself.
    let fqn = doc.current_class(offset)?;
    Some((index.class(&fqn)?.clone(), None))
}

fn member_location(class: &ClassInfo, member: &Member) -> Location {
    Location { path: member.source.as_deref().unwrap_or(&class.path).to_path_buf(), range: member.range.clone() }
}

fn class_location(class: &ClassInfo) -> Location {
    Location { path: class.path.to_path_buf(), range: class.range.clone() }
}

/// Classes extending or implementing `fqn`, directly or not.
fn subclasses(index: &Index, fqn: &str) -> Vec<Arc<ClassInfo>> {
    let key = fqn.to_lowercase();
    index
        .classes()
        .filter(|c| c.fqn.to_lowercase() != key && index.ancestors(&c.fqn).iter().any(|a| a.fqn.to_lowercase() == key))
        .cloned()
        .collect()
}

/// Go to Implementation(s) (⌘⌥B): subclasses of the class, or their versions of the method.
pub fn implementations(doc: &Document, index: &Index, offset: usize) -> Vec<(String, Location)> {
    let Some((class, member)) = target(doc, index, offset) else { return Vec::new() };
    let subs = subclasses(index, &class.fqn);
    match member {
        None => subs.iter().map(|c| (c.fqn.clone(), class_location(c))).collect(),
        Some(member) => subs
            .iter()
            .filter_map(|c| {
                let m = c.members.iter().find(|m| m.kind == member.kind && m.name.eq_ignore_ascii_case(&member.name))?;
                Some((format!("{}::{}", c.fqn, m.name), member_location(c, m)))
            })
            .collect(),
    }
}

/// Go to Super Method (⌘U): the method this one overrides or implements; on a class, its
/// parent class.
pub fn super_declaration(doc: &Document, index: &Index, offset: usize) -> Option<Location> {
    let (class, member) = target(doc, index, offset)?;
    let ancestors = index.ancestors(&class.fqn).into_iter().filter(|a| a.fqn != class.fqn);
    match member {
        None => {
            let parent = class.extends.first()?;
            index.class_location(parent)
        }
        Some(member) => ancestors
            .into_iter()
            .find_map(|a| a.members.iter().find(|m| m.kind == MemberKind::Method && m.name.eq_ignore_ascii_case(&member.name)).map(|m| member_location(&a, m))),
    }
}

/// Go to Test (⌘⇧T): `UserTest` for `User` and back.
pub fn related_tests(doc: &Document, index: &Index, offset: usize) -> Vec<(String, Location)> {
    let Some((class, _)) = target(doc, index, offset) else { return Vec::new() };
    let names: Vec<String> = match class.name.strip_suffix("Test") {
        Some(subject) => vec![subject.to_string()],
        None => vec![format!("{}Test", class.name), format!("{}Tests", class.name)],
    };
    names
        .iter()
        .flat_map(|n| index.classes_by_short_name(n))
        .map(|c| (c.fqn.clone(), class_location(c)))
        .collect()
}

/// Type Hierarchy (⌃H): ancestors from the top down to the class, then its subclasses, each
/// with its depth for indentation.
pub fn type_hierarchy(doc: &Document, index: &Index, offset: usize) -> Vec<(usize, String, Location)> {
    let Some((class, _)) = target(doc, index, offset) else { return Vec::new() };
    let mut chain = Vec::new();
    let mut current = Some(class.clone());
    while let Some(c) = current {
        current = c.extends.first().and_then(|p| index.class(p).cloned());
        chain.push(c);
        if chain.len() > 32 {
            break;
        }
    }
    chain.reverse();
    let depth = chain.len();
    let mut rows: Vec<(usize, String, Location)> = chain.iter().enumerate().map(|(i, c)| (i, c.fqn.clone(), class_location(c))).collect();
    let mut subs = subclasses(index, &class.fqn);
    subs.sort_by(|a, b| a.fqn.cmp(&b.fqn));
    for sub in subs {
        let kind = if sub.kind == ClassKind::Interface { " (interface)" } else { "" };
        rows.push((depth, format!("{}{kind}", sub.fqn), class_location(&sub)));
    }
    rows
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    const SHAPE: &str = "<?php\nnamespace App;\n\nabstract class Shape\n{\n    abstract public function area(): float;\n}\n";
    const CIRCLE: &str = "<?php\nnamespace App;\n\nclass Circle extends Shape\n{\n    public function area(): float { return 3.14; }\n}\n";
    const TEST: &str = "<?php\nnamespace Tests;\n\nclass CircleTest\n{\n}\n";

    fn index() -> Index {
        let mut index = Index::new();
        index.update_file(Path::new("/p/Shape.php"), SHAPE);
        index.update_file(Path::new("/p/Circle.php"), CIRCLE);
        index.update_file(Path::new("/p/CircleTest.php"), TEST);
        index
    }

    #[test]
    fn navigates_the_hierarchy() {
        let index = index();
        let shape = Document::parse(Path::new("/p/Shape.php"), SHAPE).unwrap();
        let at = SHAPE.find("area").unwrap() + 1;
        let impls = implementations(&shape, &index, at);
        assert_eq!(impls.len(), 1);
        assert_eq!(impls[0].0, "App\\Circle::area");
        let at = SHAPE.find("Shape").unwrap() + 1;
        assert_eq!(implementations(&shape, &index, at)[0].0, "App\\Circle");

        let circle = Document::parse(Path::new("/p/Circle.php"), CIRCLE).unwrap();
        let sup = super_declaration(&circle, &index, CIRCLE.find("area").unwrap() + 1).unwrap();
        assert_eq!(&SHAPE[sup.range], "area");
        assert_eq!(related_tests(&circle, &index, CIRCLE.find("Circle").unwrap() + 1)[0].0, "Tests\\CircleTest");
        let tree = type_hierarchy(&circle, &index, CIRCLE.find("Circle").unwrap() + 1);
        assert_eq!(tree.iter().map(|(d, n, _)| (*d, n.as_str())).collect::<Vec<_>>(), [(0, "App\\Shape"), (1, "App\\Circle")]);
    }
}
