//! Prints the named-node tree with field names: `cargo run -p php-syntax --example dump -- file.php`
fn main() {
    let path = std::env::args().nth(1).expect("usage: dump <file.php>");
    let src = std::fs::read_to_string(path).unwrap();
    let tree = php_syntax::parse(&src).unwrap();
    let mut cursor = tree.walk();
    let mut depth = 0;
    loop {
        let node = cursor.node();
        if node.is_named() || std::env::var("ALL").is_ok() {
            let field = cursor.field_name().map(|f| format!("{f}: ")).unwrap_or_default();
            let text = if node.child_count() == 0 { format!(" {:?}", &src[node.byte_range()]) } else { String::new() };
            println!("{}{field}{}{text}", "  ".repeat(depth), node.kind());
        }
        if cursor.goto_first_child() { depth += 1; continue; }
        while !cursor.goto_next_sibling() {
            if !cursor.goto_parent() { return; }
            depth -= 1;
        }
    }
}
