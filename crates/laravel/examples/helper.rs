//! Generate Helper Code on a real project and query it:
//! `cargo run --release -p laravel --example helper -- <root> <file> <needle>...`
use std::path::Path;

use php_analysis::{Document, StringProvider, complete, goto_definition};
use php_index::Index;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let root = Path::new(&args[0]);
    let mut index = Index::scan(root);
    laravel::register_mixins(&mut index);
    let helpers = laravel::helper::model_helpers(&index, root, &Default::default());
    for help in &helpers {
        let names: Vec<_> = help.members.iter().map(|m| m.name.as_str()).collect();
        println!("{}: {names:?}", help.fqn);
        index.augment(&help.fqn, help.members.clone());
    }
    let columns = laravel::columns::ColumnProvider::new(&helpers);
    let framework = laravel::framework::FrameworkProvider::new(root, &index, &helpers);
    let laravel = laravel::Laravel::new(root);
    let providers: [&dyn StringProvider; 3] = [&laravel, &columns, &framework];
    let path = root.join(&args[1]);
    let source = std::fs::read_to_string(&path).unwrap();
    let doc = Document::parse(&path, &source).unwrap();
    for needle in &args[2..] {
        let (needle, delta, mode) = match needle.split_once('@') {
            Some((n, d)) if d.starts_with('c') => (n, d[1..].parse().unwrap(), "complete"),
            Some((n, d)) => (n, d.parse().unwrap(), "goto"),
            None => (needle.as_str(), 1, "goto"),
        };
        let offset = source.find(needle).expect("needle") + delta;
        if mode == "complete" {
            let items: Vec<_> = complete(&doc, &index, offset, &providers).into_iter().take(8).map(|i| i.label).collect();
            println!("{needle:>28} complete -> {items:?}");
        } else {
            let found: Vec<_> = goto_definition(&doc, &index, offset, &providers)
                .iter()
                .map(|l| format!("{}:{}", l.path.strip_prefix(root).unwrap_or(&l.path).display(), l.range.start))
                .collect();
            println!("{needle:>28} goto -> {found:?}");
        }
    }
}
