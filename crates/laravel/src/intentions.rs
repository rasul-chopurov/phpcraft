//! Laravel intentions (⌥↵): a facade call to an injected dependency, validation rules from a
//! `'a|b'` string to an array, and a `scopeX()` method to a `#[Scope]` method.

use php_analysis::{Document, Intention, TextEdit};
use tree_sitter::Node;

/// Facades and the contract to inject instead, with the property name.
const FACADES: &[(&str, &str, &str)] = &[
    ("Cache", "Illuminate\\Contracts\\Cache\\Repository", "cache"),
    ("Log", "Psr\\Log\\LoggerInterface", "logger"),
    ("Config", "Illuminate\\Contracts\\Config\\Repository", "config"),
    ("DB", "Illuminate\\Database\\DatabaseManager", "db"),
    ("Auth", "Illuminate\\Contracts\\Auth\\Factory", "auth"),
    ("Event", "Illuminate\\Contracts\\Events\\Dispatcher", "events"),
    ("Mail", "Illuminate\\Contracts\\Mail\\Mailer", "mailer"),
    ("Queue", "Illuminate\\Contracts\\Queue\\Factory", "queue"),
    ("Storage", "Illuminate\\Contracts\\Filesystem\\Factory", "storage"),
    ("Http", "Illuminate\\Http\\Client\\Factory", "http"),
    ("Session", "Illuminate\\Session\\SessionManager", "session"),
    ("View", "Illuminate\\Contracts\\View\\Factory", "view"),
    ("Hash", "Illuminate\\Contracts\\Hashing\\Hasher", "hasher"),
    ("Validator", "Illuminate\\Contracts\\Validation\\Factory", "validator"),
    ("Redirect", "Illuminate\\Routing\\Redirector", "redirector"),
    ("Route", "Illuminate\\Routing\\Router", "router"),
    ("Gate", "Illuminate\\Contracts\\Auth\\Access\\Gate", "gate"),
    ("Bus", "Illuminate\\Contracts\\Bus\\Dispatcher", "bus"),
    ("Notification", "Illuminate\\Notifications\\ChannelManager", "notifications"),
    ("Crypt", "Illuminate\\Contracts\\Encryption\\Encrypter", "encrypter"),
    ("URL", "Illuminate\\Contracts\\Routing\\UrlGenerator", "url"),
    ("Cookie", "Illuminate\\Cookie\\CookieJar", "cookies"),
    ("Artisan", "Illuminate\\Contracts\\Console\\Kernel", "artisan"),
    ("Lang", "Illuminate\\Contracts\\Translation\\Translator", "translator"),
    ("Password", "Illuminate\\Auth\\Passwords\\PasswordBrokerManager", "passwords"),
    ("RateLimiter", "Illuminate\\Cache\\RateLimiter", "rateLimiter"),
];

fn ancestors<'t>(doc: &'t Document, offset: usize) -> Vec<Node<'t>> {
    let mut out = Vec::new();
    let mut node = doc.tree.root_node().descendant_for_byte_range(offset, offset);
    while let Some(n) = node {
        out.push(n);
        node = n.parent();
    }
    out
}

/// The facade short name for a scope like `Cache` or `\Illuminate\Support\Facades\Cache`.
fn facade_of(doc: &Document, scope: &str) -> Option<&'static (&'static str, &'static str, &'static str)> {
    let short = scope.trim_start_matches('\\').rsplit('\\').next()?;
    let entry = FACADES.iter().find(|(f, ..)| *f == short)?;
    // Imported as the facade (or the global alias).
    let imported = doc.symbols.uses.iter().any(|u| u.alias == short && (u.fqn.contains("\\Facades\\") || u.fqn == short));
    (imported || scope.contains("Facades") || !doc.symbols.uses.iter().any(|u| u.alias == short)).then_some(entry)
}

fn facade_to_injection(doc: &Document, offset: usize, out: &mut Vec<Intention>) {
    let nodes = ancestors(doc, offset);
    let Some(call) = nodes.iter().find(|n| n.kind() == "scoped_call_expression") else { return };
    let Some(scope) = call.child_by_field_name("scope") else { return };
    let Some(&(facade, contract, prop)) = facade_of(doc, doc.text(scope)) else { return };
    let Some(method) = nodes.iter().find(|n| n.kind() == "method_declaration") else { return };
    if doc.text(*method).split('(').next().is_some_and(|h| h.contains("static")) {
        return;
    }
    let Some(body) = nodes.iter().find(|n| n.kind() == "declaration_list") else { return };
    let contract_short = contract.rsplit('\\').next().unwrap_or(contract);
    let mut edits = Vec::new();
    // Every call through the facade in this class.
    let mut stack = vec![*body];
    while let Some(n) = stack.pop() {
        if n.kind() == "scoped_call_expression"
            && let Some(s) = n.child_by_field_name("scope")
            && facade_of(doc, doc.text(s)).is_some_and(|f| f.0 == facade)
        {
            edits.push(TextEdit { range: s.start_byte()..s.end_byte() + 2, text: format!("$this->{prop}->") });
            continue;
        }
        let mut cursor = n.walk();
        stack.extend(n.named_children(&mut cursor));
    }
    // The constructor parameter.
    let mut cursor = body.walk();
    let constructor = body.named_children(&mut cursor).find(|m| m.kind() == "method_declaration" && m.child_by_field_name("name").is_some_and(|n| doc.text(n) == "__construct"));
    let param = format!("private {contract_short} ${prop}");
    match constructor.and_then(|c| c.child_by_field_name("parameters")) {
        Some(params) => {
            let inner = doc.text(params).trim_start_matches('(').trim_end_matches(')').trim();
            let at = params.end_byte() - 1;
            let text = if inner.is_empty() { param } else if inner.ends_with(',') { format!(" {param},") } else { format!(", {param}") };
            edits.push(TextEdit { range: at..at, text });
        }
        None => {
            let at = body.start_byte() + 1;
            edits.push(TextEdit { range: at..at, text: format!("\n    public function __construct({param})\n    {{\n    }}\n") });
        }
    }
    if let Some(import) = php_analysis::import_edit(doc, contract) {
        edits.push(import);
    }
    out.push(Intention { label: format!("Inject {contract_short} instead of the {facade} facade"), edits });
}

fn rules_to_array(doc: &Document, offset: usize, out: &mut Vec<Intention>) {
    let nodes = ancestors(doc, offset);
    let Some(string) = nodes.iter().find(|n| matches!(n.kind(), "string" | "encapsed_string")) else { return };
    let text = doc.text(*string);
    let inner = &text[1..text.len().saturating_sub(1)];
    if !inner.contains('|') || inner.contains("regex:") || inner.contains('$') {
        return;
    }
    // A value in a `'field' => '…'` pair.
    let in_pair = string.parent().is_some_and(|p| p.kind() == "array_element_initializer" && p.named_child_count() == 2 && p.named_child(1).is_some_and(|v| v.id() == string.id()));
    if !in_pair {
        return;
    }
    let quote = &text[..1];
    let items: Vec<String> = inner.split('|').map(|r| format!("{quote}{}{quote}", r.trim())).collect();
    out.push(Intention { label: "Convert validation rules to an array".into(), edits: vec![TextEdit { range: string.byte_range(), text: format!("[{}]", items.join(", ")) }] });
}

fn scope_to_attribute(doc: &Document, offset: usize, out: &mut Vec<Intention>) {
    let nodes = ancestors(doc, offset);
    let Some(method) = nodes.iter().find(|n| n.kind() == "method_declaration") else { return };
    let Some(name) = method.child_by_field_name("name") else { return };
    let text = doc.text(name);
    let Some(rest) = text.strip_prefix("scope").filter(|r| r.starts_with(char::is_uppercase)) else { return };
    // Only from the declaration line.
    if offset > method.child_by_field_name("parameters").map_or(method.end_byte(), |p| p.end_byte()) {
        return;
    }
    let new_name: String = rest.chars().next().map(|c| c.to_lowercase().chain(rest.chars().skip(1)).collect()).unwrap_or_default();
    let line_start = doc.source[..method.start_byte()].rfind('\n').map_or(0, |i| i + 1);
    let indent = &doc.source[line_start..method.start_byte()];
    // The attribute goes in front; a public scope becomes protected (one edit: same start).
    let mut cursor = method.walk();
    let visibility = method.children(&mut cursor).find(|c| c.kind() == "visibility_modifier" && c.start_byte() == method.start_byte());
    let mut edits = vec![TextEdit { range: name.byte_range(), text: new_name }];
    match visibility {
        Some(v) => {
            let keep = if doc.text(v) == "public" { "protected" } else { doc.text(v) };
            edits.push(TextEdit { range: v.byte_range(), text: format!("#[Scope]\n{indent}{keep}") });
        }
        None => edits.push(TextEdit { range: method.start_byte()..method.start_byte(), text: format!("#[Scope]\n{indent}") }),
    }
    if let Some(import) = php_analysis::import_edit(doc, "Illuminate\\Database\\Eloquent\\Attributes\\Scope") {
        edits.push(import);
    }
    out.push(Intention { label: "Convert to a #[Scope] method".into(), edits });
}

/// Laravel intentions at `offset`.
pub fn intentions(doc: &Document, offset: usize) -> Vec<Intention> {
    let mut out = Vec::new();
    facade_to_injection(doc, offset, &mut out);
    rules_to_array(doc, offset, &mut out);
    scope_to_attribute(doc, offset, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    fn apply(src: &str, i: &Intention) -> String {
        let mut edits = i.edits.clone();
        edits.sort_by_key(|e| std::cmp::Reverse(e.range.start));
        let mut s = src.to_string();
        for e in edits {
            s.replace_range(e.range, &e.text);
        }
        s
    }

    fn run(src: &str, at: &str, label: &str) -> String {
        let doc = Document::parse(Path::new("/p/A.php"), src).unwrap();
        let found = intentions(&doc, src.find(at).unwrap() + 1);
        let i = found.iter().find(|i| i.label.starts_with(label)).unwrap_or_else(|| panic!("{label}: {found:?}"));
        apply(src, i)
    }

    #[test]
    fn facade_rules_and_scope() {
        let src = "<?php\nnamespace App\\Services;\n\nuse Illuminate\\Support\\Facades\\Cache;\n\nclass Stats\n{\n    public function get()\n    {\n        return Cache::get('x') ?? Cache::put('x', 1);\n    }\n}\n";
        let out = run(src, "Cache::get", "Inject Repository");
        assert!(out.contains("return $this->cache->get('x') ?? $this->cache->put('x', 1);"), "{out}");
        assert!(out.contains("public function __construct(private Repository $cache)"), "{out}");
        assert!(out.contains("use Illuminate\\Contracts\\Cache\\Repository;"), "{out}");

        let src = "<?php\nclass R\n{\n    public function rules(): array\n    {\n        return ['name' => 'required|string|max:255'];\n    }\n}\n";
        let out = run(src, "'required", "Convert validation");
        assert!(out.contains("'name' => ['required', 'string', 'max:255']"), "{out}");

        let src = "<?php\nnamespace App\\Models;\n\nclass Post extends Model\n{\n    public function scopeActive($query)\n    {\n        return $query->where('active', 1);\n    }\n}\n";
        let out = run(src, "scopeActive", "Convert to a #[Scope]");
        assert!(out.contains("    #[Scope]\n    protected function active($query)"), "{out}");
        assert!(out.contains("use Illuminate\\Database\\Eloquent\\Attributes\\Scope;"), "{out}");
    }
}
