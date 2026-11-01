//! PHP name resolution: namespaces, `use` imports and aliases.

use std::collections::HashMap;
use std::ops::Range;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UseKind {
    Class,
    Function,
    Const,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UseItem {
    pub kind: UseKind,
    /// Fully qualified name without leading backslash.
    pub fqn: String,
    /// Name the import is visible under (alias or last segment).
    pub alias: String,
    pub has_explicit_alias: bool,
    /// Byte range of the whole `use ...;` declaration containing this item.
    pub decl_range: Range<usize>,
}

/// Names that never refer to a class.
const BUILTIN_TYPES: &[&str] = &[
    "int", "integer", "float", "double", "string", "bool", "boolean", "array", "object", "mixed",
    "void", "null", "callable", "iterable", "never", "false", "true", "resource", "scalar",
    "numeric", "list", "non-empty-string", "positive-int", "class-string", "array-key",
];

/// Classes and interfaces built into PHP itself (not present in vendor/).
/// TODO: replace with bundled JetBrains phpstorm-stubs for full coverage.
const BUILTIN_CLASSES: &[&str] = &[
    "stdclass", "closure", "generator", "throwable", "exception", "error", "errorexception",
    "typeerror", "valueerror", "argumentcounterror", "arithmeticerror", "divisionbyzeroerror",
    "jsonexception", "logicexception", "badfunctioncallexception", "badmethodcallexception",
    "domainexception", "invalidargumentexception", "lengthexception", "outofrangeexception",
    "runtimeexception", "outofboundsexception", "overflowexception", "rangeexception",
    "underflowexception", "unexpectedvalueexception", "traversable", "iterator",
    "iteratoraggregate", "arrayaccess", "countable", "serializable", "stringable",
    "jsonserializable", "unitenum", "backedenum", "attribute", "weakmap", "weakreference",
    "arrayobject", "arrayiterator", "splobjectstorage", "splqueue", "splstack", "splfileinfo",
    "splfixedarray", "splpriorityqueue", "spldoublylinkedlist", "datetime", "datetimeimmutable",
    "datetimeinterface", "datetimezone", "dateinterval", "dateperiod", "pdo", "pdoexception",
    "pdostatement", "reflectionclass", "reflectionmethod", "reflectionproperty",
    "reflectionfunction", "reflectionnamedtype", "reflectionexception", "simplexmlelement",
    "domdocument", "domelement", "domnode", "domxpath", "intldateformatter", "numberformatter",
    "collator", "normalizer", "sensitiveparameter", "returntypewillchange", "allowdynamicproperties",
    "override", "fiber", "curlhandle", "finfo", "ziparchive", "directoryiterator",
    "recursivedirectoryiterator", "recursiveiteratoriterator", "filesystemiterator",
    "globiterator", "splsubject", "splobserver", "emptyiterator", "iteratoriterator",
    "limititerator", "callbackfilteriterator", "appenditerator", "infiniteiterator",
    "cachingiterator", "regexiterator", "splheap", "splminheap", "splmaxheap", "splfileobject",
    "spltempfileobject", "php_user_filter", "random\\randomizer", "sodiumexception",
];

pub fn is_builtin_class(fqn: &str) -> bool {
    BUILTIN_CLASSES.contains(&fqn.trim_start_matches('\\').to_ascii_lowercase().as_str())
}

pub fn is_builtin_type(name: &str) -> bool {
    BUILTIN_TYPES.contains(&name.to_ascii_lowercase().as_str())
}

pub fn is_relative_class(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "self" | "static" | "parent" | "$this"
    )
}

/// The namespace and imports in effect for a file.
#[derive(Debug, Clone, Default)]
pub struct NameContext {
    pub namespace: String,
    classes: HashMap<String, String>,
    functions: HashMap<String, String>,
}

impl NameContext {
    pub fn new(namespace: &str, uses: &[UseItem]) -> Self {
        let mut ctx = Self {
            namespace: namespace.to_string(),
            ..Default::default()
        };
        for item in uses {
            let key = item.alias.to_ascii_lowercase();
            match item.kind {
                UseKind::Class => ctx.classes.insert(key, item.fqn.clone()),
                UseKind::Function => ctx.functions.insert(key, item.fqn.clone()),
                UseKind::Const => None,
            };
        }
        ctx
    }

    /// Resolves a class reference as written in source to its FQN.
    /// Returns `None` for builtin types and relative names (`self`, `static`, `parent`).
    pub fn resolve_class(&self, name: &str) -> Option<String> {
        let name = name.trim();
        if name.is_empty() || is_builtin_type(name) || is_relative_class(name) {
            return None;
        }
        if let Some(fqn) = name.strip_prefix('\\') {
            return Some(fqn.to_string());
        }
        if let Some(rest) = name.strip_prefix("namespace\\") {
            return Some(join(&self.namespace, rest));
        }
        let (first, rest) = match name.split_once('\\') {
            Some((first, rest)) => (first, Some(rest)),
            None => (name, None),
        };
        if let Some(imported) = self.classes.get(&first.to_ascii_lowercase()) {
            return Some(match rest {
                Some(rest) => format!("{imported}\\{rest}"),
                None => imported.clone(),
            });
        }
        Some(join(&self.namespace, name))
    }

    /// Candidate FQNs for a function call, in lookup order (namespaced, then global).
    pub fn resolve_function(&self, name: &str) -> Vec<String> {
        if let Some(fqn) = name.strip_prefix('\\') {
            return vec![fqn.to_string()];
        }
        if let Some(imported) = self.functions.get(&name.to_ascii_lowercase()) {
            return vec![imported.clone()];
        }
        if name.contains('\\') {
            return vec![self.resolve_class(name).unwrap_or_else(|| name.to_string())];
        }
        let mut candidates = Vec::new();
        if !self.namespace.is_empty() {
            candidates.push(join(&self.namespace, name));
        }
        candidates.push(name.to_string());
        candidates
    }

    /// Whether `alias` is already taken by an import of a different class.
    pub fn imported_class(&self, alias: &str) -> Option<&str> {
        self.classes.get(&alias.to_ascii_lowercase()).map(String::as_str)
    }
}

pub fn join(namespace: &str, name: &str) -> String {
    if namespace.is_empty() {
        name.to_string()
    } else {
        format!("{namespace}\\{name}")
    }
}

pub fn short_name(fqn: &str) -> &str {
    fqn.rsplit('\\').next().unwrap_or(fqn)
}

pub fn namespace_of(fqn: &str) -> &str {
    fqn.rsplit_once('\\').map(|(ns, _)| ns).unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn class_use(fqn: &str, alias: &str) -> UseItem {
        UseItem {
            kind: UseKind::Class,
            fqn: fqn.into(),
            alias: alias.into(),
            has_explicit_alias: false,
            decl_range: 0..0,
        }
    }

    #[test]
    fn resolves_imports_namespace_and_fqn() {
        let ctx = NameContext::new("App\\Http", &[class_use("App\\Models\\User", "User")]);
        assert_eq!(ctx.resolve_class("User").unwrap(), "App\\Models\\User");
        assert_eq!(ctx.resolve_class("Controller").unwrap(), "App\\Http\\Controller");
        assert_eq!(ctx.resolve_class("\\Carbon\\Carbon").unwrap(), "Carbon\\Carbon");
        assert_eq!(ctx.resolve_class("user\\Profile").unwrap(), "App\\Models\\User\\Profile");
        assert_eq!(ctx.resolve_class("string"), None);
        assert_eq!(ctx.resolve_class("self"), None);
        assert_eq!(ctx.resolve_function("view"), ["App\\Http\\view", "view"]);
    }
}
