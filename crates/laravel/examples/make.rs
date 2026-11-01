//! Generates files like the Laravel Make dialog: `cargo run -p laravel --example make -- <root> <Kind> <Name>`
use laravel::make::{IdType, MakeKind, Maker, ModelSpec, parse_fields, parse_relations, write};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let kind = MakeKind::ALL
        .into_iter()
        .find(|k| format!("{k:?}").eq_ignore_ascii_case(&args[1]))
        .expect("unknown kind");
    let maker = Maker::new(std::path::Path::new(&args[0]));
    // Optional model spec via env: FIELDS="title:string" RELATIONS="belongsTo:User" ID=uuid SOFT=1
    let env = |k: &str| std::env::var(k).unwrap_or_default();
    let spec = ModelSpec {
        id: match env("ID").as_str() {
            "uuid" => IdType::Uuid,
            "ulid" => IdType::Ulid,
            _ => IdType::Id,
        },
        soft_deletes: !env("SOFT").is_empty(),
        fields: parse_fields(&env("FIELDS")),
        relations: parse_relations(&env("RELATIONS")),
        ..ModelSpec::default()
    };
    let module = args.get(3).map(String::as_str).filter(|m| !m.is_empty());
    let files = maker.plan_with(kind, &args[2], kind.extras(), module, &spec);
    for path in write(&files).expect("write failed") {
        println!("{}", path.display());
    }
}
