//! Twig templates: ⌘B on a template name (`extends`, `include`, `embed`, `import`, `from`,
//! `include()` / `source()`) and from `{% block name %}` to the same block in the parent
//! template; completion of tags after `{%`, filters after `|` and template names in quotes.

use std::path::{Path, PathBuf};

use php_analysis::{CompletionItem, CompletionKind};
use php_index::Location;
use php_syntax::twig::{FILTERS, KEYWORDS};

/// Where templates live: Symfony's `templates/`, then other common folders.
fn template_roots(root: &Path) -> Vec<PathBuf> {
    ["templates", "resources/views", "views", "app/Resources/views", "src/Resources/views"].iter().map(|d| root.join(d)).filter(|d| d.is_dir()).collect()
}

fn resolve(root: &Path, name: &str) -> Option<PathBuf> {
    let name = name.trim_start_matches('/');
    template_roots(root).into_iter().map(|dir| dir.join(name)).find(|p| p.is_file())
}

/// The quoted template name around `offset` and whether it follows a tag or function that
/// takes a template.
fn template_name_at(source: &str, offset: usize) -> Option<(String, std::ops::Range<usize>)> {
    let line_start = source[..offset].rfind('\n').map_or(0, |i| i + 1);
    let before = &source[line_start..offset];
    let quote_at = before.rfind(['\'', '"'])?;
    let quote = before.as_bytes()[quote_at] as char;
    let start = line_start + quote_at + 1;
    let end = source[start..].find(quote).map_or(offset, |i| start + i);
    let lead = before[..quote_at].trim_end();
    let lead = lead.trim_end_matches(['(', '[', ',', ' ']);
    let word: String = lead.chars().rev().take_while(|c| c.is_alphanumeric() || *c == '_').collect::<Vec<_>>().into_iter().rev().collect();
    matches!(word.as_str(), "extends" | "include" | "embed" | "import" | "from" | "use" | "source" | "template_from_string")
        .then(|| (source[start..end].to_string(), start..end))
}

/// `{% block name %}` with `offset` on the name.
fn block_name_at(source: &str, offset: usize) -> Option<String> {
    let line_start = source[..offset].rfind('\n').map_or(0, |i| i + 1);
    let tag_start = source[line_start..offset].rfind("{%")? + line_start;
    let tag_end = source[tag_start..].find("%}").map_or(source.len(), |i| tag_start + i);
    let mut words = source[tag_start + 2..tag_end].split_whitespace();
    (words.next()? == "block").then(|| words.next().map(str::to_string)).flatten()
}

fn extends_of(source: &str) -> Option<String> {
    let at = source.find("{% extends")?.checked_add("{% extends".len())?;
    let rest = source[at..].trim_start();
    let quote = rest.chars().next().filter(|c| *c == '\'' || *c == '"')?;
    Some(rest[1..].split(quote).next()?.to_string())
}

pub fn goto(root: &Path, source: &str, offset: usize) -> Vec<Location> {
    if let Some((name, _)) = template_name_at(source, offset) {
        return resolve(root, &name).map(|path| vec![Location { path, range: 0..0 }]).unwrap_or_default();
    }
    // A block: the nearest parent template that defines it too.
    if let Some(block) = block_name_at(source, offset) {
        let mut parent = extends_of(source);
        let mut seen = 0;
        while let Some(name) = parent.take() {
            let Some(path) = resolve(root, &name) else { break };
            let Ok(text) = std::fs::read_to_string(&path) else { break };
            for pattern in [format!("{{% block {block} %}}"), format!("{{% block {block}")] {
                if let Some(at) = text.find(&pattern) {
                    let start = at + pattern.find(&block).unwrap_or(0);
                    return vec![Location { path, range: start..start + block.len() }];
                }
            }
            parent = extends_of(&text);
            seen += 1;
            if seen > 16 {
                break;
            }
        }
    }
    Vec::new()
}

pub fn complete(root: &Path, source: &str, offset: usize) -> Vec<CompletionItem> {
    // Template names in quotes.
    if let Some((typed, range)) = template_name_at(source, offset) {
        let typed = &typed[..offset - range.start];
        let mut names = Vec::new();
        for dir in template_roots(root) {
            let mut files = Vec::new();
            collect(&dir, &mut files);
            names.extend(files.into_iter().filter_map(|f| f.strip_prefix(&dir).ok().map(|p| p.to_string_lossy().replace('\\', "/"))));
        }
        names.sort();
        names.dedup();
        return names.into_iter().filter(|n| n.starts_with(typed)).map(|n| CompletionItem::simple(n, "template", CompletionKind::Keyword, range.start..offset)).collect();
    }
    let word_start = source[..offset].char_indices().rev().take_while(|(_, c)| c.is_alphanumeric() || *c == '_').last().map_or(offset, |(i, _)| i);
    let typed = &source[word_start..offset];
    let before = source[..word_start].trim_end_matches([' ', '\t']);
    let replace = word_start..offset;
    let list: &[&str] = if before.ends_with('|') {
        FILTERS
    } else if before.ends_with("{%") || before.ends_with("{%-") {
        KEYWORDS
    } else {
        return Vec::new();
    };
    let kind = if list == FILTERS { CompletionKind::Function } else { CompletionKind::Keyword };
    list.iter().filter(|w| w.starts_with(typed)).map(|w| CompletionItem::simple(*w, "", kind, replace.clone())).collect()
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, out);
        } else if path.to_string_lossy().ends_with(".twig") {
            out.push(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn navigates_and_completes() {
        let root = std::env::temp_dir().join(format!("phpcraft-twig-{}", std::process::id()));
        std::fs::create_dir_all(root.join("templates/blog")).unwrap();
        std::fs::write(root.join("templates/base.html.twig"), "<html>{% block body %}{% endblock %}</html>").unwrap();
        std::fs::write(root.join("templates/blog/_card.html.twig"), "card").unwrap();
        let page = "{% extends 'base.html.twig' %}\n{% block body %}{% include 'blog/_card.html.twig' %}{% endblock %}";
        let at = page.find("base.html").unwrap() + 2;
        assert_eq!(goto(&root, page, at)[0].path, root.join("templates/base.html.twig"));
        let at = page.find("_card").unwrap();
        assert_eq!(goto(&root, page, at)[0].path, root.join("templates/blog/_card.html.twig"));
        let at = page.find("body %}").unwrap() + 1;
        let block = goto(&root, page, at);
        assert_eq!(block[0].path, root.join("templates/base.html.twig"));

        let src = "{% include 'blog/";
        assert_eq!(complete(&root, src, src.len())[0].label, "blog/_card.html.twig");
        let src = "{{ name|up";
        assert_eq!(complete(&root, src, src.len())[0].label, "upper");
        let src = "{% en";
        assert!(complete(&root, src, src.len()).iter().any(|i| i.label == "endblock"));
        std::fs::remove_dir_all(root).ok();
    }
}
