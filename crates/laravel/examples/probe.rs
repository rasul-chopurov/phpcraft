//! Runs go-to-declaration at `needle` occurrences in a real project:
//! `cargo run --release -p laravel --example probe -- <root> <file> <needle> [<needle>...]`
use std::path::Path;
use std::time::Instant;

use php_analysis::{Document, StringProvider, goto_definition, unresolved_classes};
use php_index::Index;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let root = Path::new(&args[0]);
    let started = Instant::now();
    let mut index = Index::scan(root);
    laravel::register_mixins(&mut index);
    println!("indexed {} classes in {:.2?}", index.class_count(), started.elapsed());

    let laravel = laravel::Laravel::new(root);
    let providers: [&dyn StringProvider; 1] = [&laravel];
    let path = root.join(&args[1]);
    let source = std::fs::read_to_string(&path).unwrap();
    let doc = Document::parse(&path, &source).unwrap();
    for needle in &args[2..] {
        let (needle, delta) = match needle.split_once('@') {
            Some((n, d)) => (n, d.parse().unwrap()),
            None => (needle.as_str(), 1),
        };
        let offset = source.find(needle).expect("needle") + delta;
        let mut found = goto_definition(&doc, &index, offset, &providers);
        if found.is_empty() {
            found = laravel.text_goto(&source, offset);
        }
        let shown: Vec<_> = found
            .iter()
            .map(|l| format!("{}:{}", l.path.strip_prefix(root).unwrap_or(&l.path).display(), l.range.start))
            .collect();
        println!("{needle:>30} -> {shown:?}");
    }
    let unresolved: Vec<_> = unresolved_classes(&doc, &index).into_iter().map(|u| u.name).collect();
    println!("unresolved: {unresolved:?}");
}
