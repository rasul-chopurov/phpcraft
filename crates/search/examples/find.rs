//! `cargo run --release -p search --example find -- <root> <pattern> [--vendor]`
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let root = std::path::Path::new(&args[0]);
    let options = search::FileSearch { include_vendor: args.iter().any(|a| a == "--vendor"), ..Default::default() };
    let started = Instant::now();
    let files = search::list_files(root, options.include_vendor).len();
    let found = search::search_files(root, &search::TextQuery::new(args[1].clone()), &options).unwrap();
    println!("{files} files, {} matching lines in {:.2?}", found.len(), started.elapsed());
    for m in found.iter().take(3) {
        println!("  {}:{} {}", m.path.strip_prefix(root).unwrap().display(), m.line + 1, m.text.trim());
    }
}
