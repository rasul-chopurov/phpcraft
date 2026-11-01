//! Lists artisan commands with their argument/option counts: `cargo run -p laravel --example artisan -- <root>`
fn main() {
    let root = std::env::args().nth(1).expect("usage: artisan <root>");
    let commands = laravel::artisan::list_commands(std::path::Path::new(&root)).expect("artisan list failed");
    println!("{} commands", commands.len());
    for c in commands.iter().filter(|c| c.name == "make:model" || c.name == "migrate") {
        let options: Vec<_> = c.options.iter().map(|o| if o.accepts_value { format!("--{}=", o.name) } else { format!("--{}", o.name) }).collect();
        println!("{}: args={:?} options={options:?}", c.name, c.arguments.iter().map(|a| &a.name).collect::<Vec<_>>());
    }
}
