//! Markup completion in Blade, HTML and Vue files: Alpine.js, Livewire and Vue attributes,
//! Tailwind CSS classes and `<livewire:…>` component tags.

use std::ops::Range;
use std::path::Path;

use php_analysis::{CompletionItem, CompletionKind};

const ALPINE: &[(&str, &str)] = &[
    ("x-data", "Alpine component state"),
    ("x-init", "Run on init"),
    ("x-show", "Toggle display"),
    ("x-bind:", "Bind an attribute"),
    ("x-on:", "Listen to an event"),
    ("x-text", "Set text content"),
    ("x-html", "Set inner HTML"),
    ("x-model", "Two-way binding"),
    ("x-modelable", "Expose a property to x-model"),
    ("x-for", "Loop (on <template>)"),
    ("x-if", "Conditional (on <template>)"),
    ("x-transition", "Transition"),
    ("x-effect", "Re-run on change"),
    ("x-ignore", "Skip initialisation"),
    ("x-ref", "Reference as $refs"),
    ("x-cloak", "Hide until initialised"),
    ("x-teleport", "Move to another element"),
    ("x-id", "Scoped ids"),
    ("@click", "x-on:click"),
    ("@click.prevent", "x-on:click.prevent"),
    ("@click.outside", "Click outside"),
    ("@submit.prevent", "x-on:submit.prevent"),
    ("@keydown.enter", "Enter key"),
    ("@keydown.escape", "Escape key"),
    ("@input", "x-on:input"),
    ("@change", "x-on:change"),
    (":class", "x-bind:class"),
    (":style", "x-bind:style"),
    (":disabled", "x-bind:disabled"),
];

const LIVEWIRE: &[(&str, &str)] = &[
    ("wire:model", "Bind to a property (deferred)"),
    ("wire:model.live", "Bind and update live"),
    ("wire:model.live.debounce.300ms", "Live with debounce"),
    ("wire:model.blur", "Update on blur"),
    ("wire:click", "Call a method on click"),
    ("wire:submit", "Call a method on submit"),
    ("wire:loading", "Show while loading"),
    ("wire:loading.remove", "Hide while loading"),
    ("wire:target", "Scope wire:loading"),
    ("wire:poll", "Refresh periodically"),
    ("wire:poll.5s", "Refresh every 5s"),
    ("wire:navigate", "SPA navigation"),
    ("wire:key", "Track in loops"),
    ("wire:ignore", "Don't morph this element"),
    ("wire:dirty", "Show when unsaved"),
    ("wire:confirm", "Confirm before an action"),
    ("wire:init", "Call on load"),
    ("wire:offline", "Show when offline"),
    ("wire:stream", "Stream content"),
    ("wire:transition", "Transition"),
    ("wire:replace", "Replace instead of morph"),
    ("wire:show", "Toggle display"),
    ("wire:text", "Set text from a property"),
    ("wire:cloak", "Hide until initialised"),
];

const VUE: &[(&str, &str)] = &[
    ("v-if", "Conditional"),
    ("v-else-if", "Else if"),
    ("v-else", "Else"),
    ("v-for", "Loop"),
    ("v-show", "Toggle display"),
    ("v-model", "Two-way binding"),
    ("v-bind:", "Bind an attribute"),
    ("v-on:", "Listen to an event"),
    ("v-slot", "Named slot"),
    ("v-html", "Inner HTML"),
    ("v-text", "Text content"),
    ("v-once", "Render once"),
    ("v-memo", "Memoize"),
    ("v-cloak", "Hide until compiled"),
    ("@click", "v-on:click"),
    ("@submit.prevent", "v-on:submit.prevent"),
    (":class", "v-bind:class"),
    (":style", "v-bind:style"),
    (":key", "v-bind:key"),
    ("#default", "Default slot"),
];

const COLORS: &[&str] = &[
    "slate", "gray", "zinc", "neutral", "stone", "red", "orange", "amber", "yellow", "lime", "green", "emerald", "teal", "cyan", "sky", "blue",
    "indigo", "violet", "purple", "fuchsia", "pink", "rose",
];
const SHADES: &[&str] = &["50", "100", "200", "300", "400", "500", "600", "700", "800", "900", "950"];
const SPACING: &[&str] = &[
    "0", "px", "0.5", "1", "1.5", "2", "2.5", "3", "3.5", "4", "5", "6", "7", "8", "9", "10", "11", "12", "14", "16", "20", "24", "28", "32", "36",
    "40", "44", "48", "52", "56", "60", "64", "72", "80", "96",
];

/// Tailwind utility classes (v3/v4 core set).
pub fn tailwind_classes() -> Vec<String> {
    let mut out: Vec<String> = [
        "block", "inline-block", "inline", "flex", "inline-flex", "grid", "inline-grid", "hidden", "contents", "table", "container", "sr-only",
        "static", "relative", "absolute", "fixed", "sticky", "flex-row", "flex-col", "flex-wrap", "flex-nowrap", "flex-1", "flex-auto",
        "flex-none", "grow", "shrink-0", "items-start", "items-center", "items-end", "items-stretch", "items-baseline", "justify-start",
        "justify-center", "justify-end", "justify-between", "justify-around", "justify-evenly", "self-start", "self-center", "self-end",
        "content-center", "place-items-center", "w-full", "w-screen", "w-auto", "w-fit", "h-full", "h-screen", "h-auto", "h-fit", "min-h-screen",
        "min-w-0", "max-w-sm", "max-w-md", "max-w-lg", "max-w-xl", "max-w-2xl", "max-w-3xl", "max-w-4xl", "max-w-5xl", "max-w-6xl",
        "max-w-7xl", "max-w-full", "max-w-prose", "mx-auto", "text-xs", "text-sm", "text-base", "text-lg", "text-xl", "text-2xl", "text-3xl",
        "text-4xl", "text-5xl", "text-6xl", "font-thin", "font-light", "font-normal", "font-medium", "font-semibold", "font-bold",
        "font-extrabold", "font-sans", "font-serif", "font-mono", "italic", "underline", "line-through", "uppercase", "lowercase",
        "capitalize", "truncate", "text-left", "text-center", "text-right", "text-justify", "leading-none", "leading-tight", "leading-snug",
        "leading-normal", "leading-relaxed", "tracking-tight", "tracking-wide", "whitespace-nowrap", "break-words", "text-white",
        "text-black", "bg-white", "bg-black", "bg-transparent", "border", "border-0", "border-2", "border-4", "border-t", "border-b",
        "border-l", "border-r", "border-dashed", "rounded", "rounded-none", "rounded-sm", "rounded-md", "rounded-lg", "rounded-xl",
        "rounded-2xl", "rounded-3xl", "rounded-full", "shadow", "shadow-sm", "shadow-md", "shadow-lg", "shadow-xl", "shadow-2xl",
        "shadow-none", "ring", "ring-1", "ring-2", "ring-inset", "outline-none", "overflow-hidden", "overflow-auto", "overflow-x-auto",
        "overflow-y-auto", "overflow-scroll", "cursor-pointer", "cursor-not-allowed", "pointer-events-none", "select-none", "transition",
        "transition-all", "transition-colors", "duration-150", "duration-200", "duration-300", "duration-500", "ease-in", "ease-out",
        "ease-in-out", "animate-spin", "animate-pulse", "animate-bounce", "opacity-0", "opacity-25", "opacity-50", "opacity-75",
        "opacity-100", "z-0", "z-10", "z-20", "z-30", "z-40", "z-50", "inset-0", "grid-cols-1", "grid-cols-2", "grid-cols-3", "grid-cols-4",
        "grid-cols-6", "grid-cols-12", "col-span-1", "col-span-2", "col-span-3", "col-span-full", "space-y-1", "space-y-2", "space-y-4",
        "space-x-2", "space-x-4", "divide-y", "object-cover", "object-contain", "aspect-square", "aspect-video",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    for prefix in ["p", "px", "py", "pt", "pr", "pb", "pl", "m", "mx", "my", "mt", "mr", "mb", "ml", "gap", "gap-x", "gap-y", "w", "h", "top", "right", "bottom", "left", "inset"] {
        for size in SPACING {
            out.push(format!("{prefix}-{size}"));
        }
    }
    for kind in ["text", "bg", "border", "ring", "from", "to", "via", "fill", "stroke", "divide", "placeholder", "decoration", "outline"] {
        for color in COLORS {
            for shade in SHADES {
                out.push(format!("{kind}-{color}-{shade}"));
            }
        }
    }
    out
}

/// The Tailwind variant prefixes offered before a class (`hover:`, `md:`…).
const VARIANTS: &[&str] = &[
    "hover:", "focus:", "active:", "disabled:", "group-hover:", "focus-within:", "focus-visible:", "first:", "last:", "odd:", "even:", "dark:",
    "sm:", "md:", "lg:", "xl:", "2xl:", "print:", "motion-safe:", "placeholder:", "before:", "after:",
];

/// Whether the project uses Tailwind CSS.
pub fn uses_tailwind(root: &Path) -> bool {
    ["tailwind.config.js", "tailwind.config.ts", "tailwind.config.cjs", "tailwind.config.mjs"].iter().any(|f| root.join(f).exists())
        || std::fs::read_to_string(root.join("package.json")).is_ok_and(|t| t.contains("tailwindcss"))
}

/// Livewire components as tag names: `app/Livewire/Posts/ShowPost.php` -> `posts.show-post`.
pub fn livewire_components(root: &Path) -> Vec<String> {
    let base = root.join("app/Livewire");
    let mut out = Vec::new();
    let mut stack = vec![base.clone()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "php")
                && let Ok(rel) = path.with_extension("").strip_prefix(&base)
            {
                out.push(rel.iter().map(|p| kebab(&p.to_string_lossy())).collect::<Vec<_>>().join("."));
            }
        }
    }
    out.sort();
    out
}

fn kebab(name: &str) -> String {
    let mut out = String::new();
    for (i, c) in name.chars().enumerate() {
        if c.is_uppercase() && i > 0 {
            out.push('-');
        }
        out.extend(c.to_lowercase());
    }
    out
}

/// Where the caret is inside markup.
#[derive(Debug, PartialEq, Eq)]
enum Spot {
    /// Typing an attribute name; the range is the typed part.
    AttributeName(Range<usize>),
    /// Inside the value of `class`/`:class`/`className`; the range is the current class word.
    ClassValue(Range<usize>),
    /// After `<livewire:`; the range is the component name typed so far.
    LivewireTag(Range<usize>),
}

fn spot(source: &str, offset: usize) -> Option<Spot> {
    let before = &source[..offset];
    let tag_start = before.rfind('<')?;
    if before[tag_start..].contains('>') {
        return None;
    }
    let inside = &before[tag_start + 1..];
    if let Some(name) = inside.strip_prefix("livewire:")
        && !name.contains(char::is_whitespace)
    {
        return Some(Spot::LivewireTag(offset - name.len()..offset));
    }
    // Inside a quoted value? Count quotes after the tag name.
    let mut quote: Option<(char, usize)> = None;
    for (i, c) in inside.char_indices() {
        match quote {
            Some((q, _)) if c == q => quote = None,
            None if c == '"' || c == '\'' => quote = Some((c, i)),
            _ => {}
        }
    }
    if let Some((_, at)) = quote {
        let attr = inside[..at].trim_end().trim_end_matches('=').trim_end();
        let name = attr.rsplit(|c: char| c.is_whitespace()).next().unwrap_or("");
        if matches!(name, "class" | ":class" | "x-bind:class" | "v-bind:class" | "className") {
            let value = &inside[at + 1..];
            let word = value.rsplit(|c: char| c.is_whitespace() || c == '\'' || c == '{' || c == ',').next().unwrap_or("");
            return Some(Spot::ClassValue(offset - word.len()..offset));
        }
        return None;
    }
    // Attribute name: after whitespace following the tag name.
    let word = inside.rsplit(|c: char| c.is_whitespace()).next().unwrap_or("");
    (inside.contains(char::is_whitespace) && !word.contains('=')).then(|| Spot::AttributeName(offset - word.len()..offset))
}

/// Completions at `offset`, or `None` when the caret isn't in a spot this module handles.
pub fn complete(root: &Path, path: &Path, source: &str, offset: usize) -> Option<Vec<CompletionItem>> {
    let name = path.to_string_lossy();
    let vue = name.ends_with(".vue");
    let blade = name.ends_with(".blade.php");
    let items = match spot(source, offset)? {
        Spot::AttributeName(range) => {
            let mut attrs: Vec<(&str, &str)> = Vec::new();
            if vue {
                attrs.extend(VUE);
            } else {
                attrs.extend(ALPINE);
                if blade {
                    attrs.extend(LIVEWIRE);
                }
            }
            attrs
                .into_iter()
                .map(|(label, detail)| {
                    let mut item = CompletionItem::simple(label, detail, CompletionKind::Property, range.clone());
                    if !label.ends_with(':') && !label.starts_with('#') && !matches!(label, "x-cloak" | "x-ignore" | "wire:ignore" | "v-else" | "v-once" | "v-cloak" | "wire:navigate" | "wire:cloak") {
                        item.insert = format!("{label}=\"\"");
                    }
                    item
                })
                .collect()
        }
        Spot::ClassValue(range) => {
            if !uses_tailwind(root) {
                return Some(Vec::new());
            }
            let typed = &source[range.clone()];
            // Keep variant prefixes (`md:hover:`) and complete the class after them.
            let split = typed.rfind(':').map_or(0, |i| i + 1);
            let replace = range.start + split..range.end;
            let mut items: Vec<CompletionItem> =
                tailwind_classes().into_iter().map(|class| CompletionItem::simple(class, "tailwind", CompletionKind::Value, replace.clone())).collect();
            items.extend(VARIANTS.iter().map(|v| CompletionItem::simple(*v, "variant", CompletionKind::Keyword, replace.clone())));
            items
        }
        Spot::LivewireTag(range) => livewire_components(root)
            .into_iter()
            .map(|name| CompletionItem::simple(name, "livewire", CompletionKind::Class, range.clone()))
            .collect(),
    };
    let typed_range = items.first().map(|i| i.replace.clone());
    let typed = typed_range.map(|r| source[r].to_string()).unwrap_or_default();
    let mut items: Vec<CompletionItem> = items.into_iter().filter(|i| typed.is_empty() || i.label.starts_with(&typed) || php_analysis::matcher::matches(&i.label, &typed)).collect();
    items.sort_by_key(|i| (php_analysis::matcher::degree(&i.label, &typed), i.label.len()));
    items.truncate(80);
    Some(items)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spots() {
        let s = r#"<div class="flex md:it" x-da"#;
        assert_eq!(spot(s, s.len()), Some(Spot::AttributeName(s.len() - 4..s.len())));
        let s = r#"<div class="flex md:it"#;
        assert_eq!(spot(s, s.len()), Some(Spot::ClassValue(s.len() - 5..s.len())));
        let s = "<livewire:posts.sh";
        assert_eq!(spot(s, s.len()), Some(Spot::LivewireTag(10..s.len())));
        assert_eq!(spot("<div>text", 9), None);
        let s = r#"<a href="x"#;
        assert_eq!(spot(s, s.len()), None);
    }

    #[test]
    fn completes_frameworks() {
        let root = std::env::temp_dir().join(format!("phpcraft-frontend-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("app/Livewire/Posts")).unwrap();
        std::fs::write(root.join("app/Livewire/Posts/ShowPost.php"), "<?php").unwrap();
        std::fs::write(root.join("package.json"), r#"{"devDependencies":{"tailwindcss":"^4"}}"#).unwrap();
        let blade = root.join("resources/views/a.blade.php");
        let labels = |path: &Path, s: &str| complete(&root, path, s, s.len()).unwrap().into_iter().map(|i| i.label).collect::<Vec<_>>();
        assert!(labels(&blade, "<input wire:mo").contains(&"wire:model".to_string()));
        assert!(labels(&blade, "<div x-sh").contains(&"x-show".to_string()));
        assert!(labels(&blade, r#"<div class="md:items-ce"#).contains(&"items-center".to_string()));
        assert!(labels(&blade, r#"<div class="bg-red-5"#).contains(&"bg-red-500".to_string()));
        assert_eq!(labels(&blade, "<livewire:po"), ["posts.show-post"]);
        let vue = root.join("resources/js/A.vue");
        assert!(labels(&vue, "<li v-fo").contains(&"v-for".to_string()));
        assert!(!labels(&vue, "<li wire:").iter().any(|l| l.starts_with("wire:")));
        std::fs::remove_dir_all(root).unwrap();
    }
}
