//! `cargo run -p laravel --example table -- <root> [table]`
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let root = std::path::Path::new(&args[0]);
    println!("{:?}", laravel::database::list_tables(root));
    if let Some(table) = args.get(1) {
        let columns = laravel::database::table_columns(root, table).unwrap();
        let (model, spec) = laravel::database::spec_from_table(table, &columns);
        println!("{model}: id={:?} timestamps={} fields={:?} relations={:?}", spec.id, spec.timestamps,
            spec.fields.iter().map(|f| format!("{}:{}{}", f.name, f.column, if f.nullable { "?" } else { "" })).collect::<Vec<_>>(),
            spec.relations.iter().map(|r| r.target.clone()).collect::<Vec<_>>());
    }
}
