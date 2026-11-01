//! Generate Helper Code: model columns (from migrations or the database), relations,
//! `*_count` properties and `where{Column}()` methods, applied to the index so completion,
//! navigation and type inference see them — without writing phpdoc into the models.

use std::collections::BTreeMap;
use std::fs;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use php_analysis::Document;
use php_index::{ClassInfo, Index, Member, MemberKind};
use tree_sitter::Node;

use crate::make::{studly, table_name};
use crate::settings::{HelperSettings, HelperSource};

const MODEL: &str = "Illuminate\\Database\\Eloquent\\Model";
const CARBON: &str = "Illuminate\\Support\\Carbon";
const COLLECTION: &str = "Illuminate\\Database\\Eloquent\\Collection";
const BUILDER: &str = "Illuminate\\Database\\Eloquent\\Builder";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Column {
    pub name: String,
    /// Blueprint method that created it (`string`, `foreignId`, `timestamp`, ...).
    pub ty: String,
    pub nullable: bool,
    /// Where the column is declared.
    pub path: PathBuf,
    pub range: Range<usize>,
}

/// Table name -> columns in declaration order.
pub type Schema = BTreeMap<String, Vec<Column>>;

fn migration_dirs(root: &Path) -> Vec<PathBuf> {
    let mut dirs = vec![root.join("database/migrations")];
    if let Ok(modules) = fs::read_dir(root.join("Modules")) {
        dirs.extend(modules.flatten().map(|m| m.path().join("database/migrations")));
    }
    dirs
}

/// Replays migrations (sorted by file name, i.e. timestamp) into a schema.
pub fn schema_from_migrations(root: &Path) -> Schema {
    let mut files: Vec<PathBuf> = migration_dirs(root)
        .iter()
        .filter_map(|dir| fs::read_dir(dir).ok())
        .flat_map(|entries| entries.flatten().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "php"))
        .collect();
    files.sort_by_key(|p| p.file_name().map(|n| n.to_os_string()));
    let mut schema = Schema::new();
    for path in files {
        if let Ok(source) = fs::read_to_string(&path) {
            apply_migration(&path, &source, &mut schema);
        }
    }
    schema
}

fn text<'a>(node: Node, source: &'a str) -> &'a str {
    &source[node.byte_range()]
}

fn string_value(node: Node, source: &str) -> Option<(String, Range<usize>)> {
    let raw = text(node, source);
    let quote = raw.chars().next().filter(|c| *c == '\'' || *c == '"')?;
    let inner = raw.trim_matches(quote);
    let start = node.start_byte() + 1;
    Some((inner.to_string(), start..start + inner.len()))
}

fn arguments(call: Node) -> Vec<Node> {
    let Some(args) = call.child_by_field_name("arguments") else { return Vec::new() };
    let mut cursor = args.walk();
    args.named_children(&mut cursor).filter_map(|a| a.named_child(0)).collect()
}

fn visit<'t>(node: Node<'t>, f: &mut impl FnMut(Node<'t>)) {
    f(node);
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        visit(child, f);
    }
}

fn apply_migration(path: &Path, source: &str, schema: &mut Schema) {
    let Some(tree) = php_syntax::parse(source) else { return };
    // Only `up()`: `down()` reverses it.
    let mut up = None;
    visit(tree.root_node(), &mut |n| {
        if n.kind() == "method_declaration" && n.child_by_field_name("name").is_some_and(|name| text(name, source) == "up") {
            up = Some(n);
        }
    });
    let Some(up) = up else { return };
    visit(up, &mut |n| {
        if n.kind() != "scoped_call_expression" {
            return;
        }
        let (Some(scope), Some(name)) = (n.child_by_field_name("scope"), n.child_by_field_name("name")) else { return };
        let method = text(name, source);
        if !text(scope, source).ends_with("Schema") {
            return;
        }
        let args = arguments(n);
        let Some((table, _)) = args.first().and_then(|a| string_value(*a, source)) else { return };
        match method {
            "create" => {
                schema.insert(table.clone(), Vec::new());
            }
            "table" => {}
            "drop" | "dropIfExists" => {
                schema.remove(&table);
                return;
            }
            "rename" => {
                if let Some((to, _)) = args.get(1).and_then(|a| string_value(*a, source))
                    && let Some(columns) = schema.remove(&table)
                {
                    schema.insert(to, columns);
                }
                return;
            }
            _ => return,
        }
        let Some(closure) = args.get(1).copied() else { return };
        let columns = schema.entry(table).or_default();
        visit(closure, &mut |stmt| {
            if stmt.kind() == "expression_statement"
                && let Some(expr) = stmt.named_child(0)
            {
                apply_blueprint_call(path, source, expr, columns);
            }
        });
    });
}

/// Unwinds `$table->string('x')->nullable()->unique()` into the base call and its modifiers.
fn apply_blueprint_call(path: &Path, source: &str, expr: Node, columns: &mut Vec<Column>) {
    let mut modifiers = Vec::new();
    let mut call = expr;
    while call.kind() == "member_call_expression" {
        let Some(object) = call.child_by_field_name("object") else { return };
        if object.kind() == "variable_name" {
            break;
        }
        if let Some(name) = call.child_by_field_name("name") {
            modifiers.push(text(name, source).to_string());
        }
        call = object;
    }
    if call.kind() != "member_call_expression" {
        return;
    }
    let Some(name) = call.child_by_field_name("name") else { return };
    let method = text(name, source);
    let args = arguments(call);
    let first = args.first().and_then(|a| string_value(*a, source));
    let nullable = modifiers.iter().any(|m| m == "nullable");
    let call_range = name.byte_range();
    fn add(columns: &mut Vec<Column>, path: &Path, name: &str, ty: &str, nullable: bool, range: Range<usize>) {
        columns.retain(|c| c.name != name);
        columns.push(Column { name: name.to_string(), ty: ty.to_string(), nullable, path: path.to_path_buf(), range });
    }
    match method {
        "id" | "bigIncrements" | "increments" => {
            let (name, range) = first.unwrap_or(("id".into(), call_range));
            add(columns, path, &name, "id", false, range);
        }
        "timestamps" | "timestampsTz" | "nullableTimestamps" => {
            add(columns, path, "created_at", "timestamp", true, call_range.clone());
            add(columns, path, "updated_at", "timestamp", true, call_range);
        }
        "softDeletes" | "softDeletesTz" => {
            let (name, range) = first.unwrap_or(("deleted_at".into(), call_range));
            add(columns, path, &name, "timestamp", true, range);
        }
        "rememberToken" => add(columns, path, "remember_token", "string", true, call_range),
        "morphs" | "nullableMorphs" | "uuidMorphs" | "ulidMorphs" | "nullableUuidMorphs" | "nullableUlidMorphs" => {
            if let Some((name, range)) = first {
                let nullable = nullable || method.starts_with("nullable");
                let id_type = if method.contains("Uuid") || method.contains("Ulid") { "uuid" } else { "unsignedBigInteger" };
                add(columns, path, &format!("{name}_id"), id_type, nullable, range.clone());
                add(columns, path, &format!("{name}_type"), "string", nullable, range);
            }
        }
        "foreignIdFor" => {
            if let Some(arg) = args.first() {
                let class = text(*arg, source).trim_end_matches("::class").rsplit('\\').next().unwrap_or("").to_string();
                let column = args
                    .get(1)
                    .and_then(|a| string_value(*a, source))
                    .map(|(n, _)| n)
                    .unwrap_or_else(|| format!("{}_id", crate::make::snake(&class)));
                add(columns, path, &column, "foreignId", nullable, arg.byte_range());
            }
        }
        "dropColumn" | "dropColumns" => {
            let mut names = Vec::new();
            for arg in &args {
                if arg.kind() == "array_creation_expression" {
                    visit(*arg, &mut |n| {
                        if let Some((v, _)) = (n.kind() == "string" || n.kind() == "encapsed_string").then(|| string_value(n, source)).flatten() {
                            names.push(v);
                        }
                    });
                } else if let Some((v, _)) = string_value(*arg, source) {
                    names.push(v);
                }
            }
            columns.retain(|c| !names.contains(&c.name));
        }
        "dropTimestamps" | "dropTimestampsTz" => columns.retain(|c| c.name != "created_at" && c.name != "updated_at"),
        "dropSoftDeletes" | "dropSoftDeletesTz" => columns.retain(|c| c.name != "deleted_at"),
        "renameColumn" => {
            if let (Some((from, _)), Some((to, range))) = (first, args.get(1).and_then(|a| string_value(*a, source))) {
                if let Some(column) = columns.iter_mut().find(|c| c.name == from) {
                    column.name = to;
                    column.range = range;
                }
            }
        }
        // Index/constraint helpers don't create columns.
        "index" | "unique" | "primary" | "foreign" | "dropIndex" | "dropUnique" | "dropForeign" | "dropPrimary" | "fullText" | "spatialIndex" => {}
        _ => {
            // Any other column type: `$table->string('name')`.
            if let Some((name, range)) = first {
                let changed = modifiers.iter().any(|m| m == "change");
                if changed {
                    if let Some(existing) = columns.iter_mut().find(|c| c.name == name) {
                        existing.ty = method.to_string();
                        existing.nullable = nullable;
                        return;
                    }
                }
                add(columns, path, &name, method, nullable, range);
            }
        }
    }
}

/// PHP type of a column, considering the model's cast: (doc type, class FQN if any).
fn php_type(column_type: &str, cast: Option<&str>) -> (String, Option<String>) {
    if let Some(cast) = cast {
        let base = cast.split(':').next().unwrap_or(cast);
        return match base {
            "date" | "datetime" | "immutable_date" | "immutable_datetime" | "timestamp" => (format!("\\{CARBON}"), Some(CARBON.into())),
            "bool" | "boolean" => ("bool".into(), None),
            "int" | "integer" => ("int".into(), None),
            "float" | "double" | "real" => ("float".into(), None),
            "decimal" | "string" | "hashed" | "encrypted" => ("string".into(), None),
            "array" | "json" => ("array".into(), None),
            "object" => ("object".into(), None),
            "collection" => ("\\Illuminate\\Support\\Collection".into(), Some("Illuminate\\Support\\Collection".into())),
            class if class.contains('\\') => (format!("\\{class}"), Some(class.trim_start_matches('\\').to_string())),
            _ => ("mixed".into(), None),
        };
    }
    match column_type {
        "id" | "increments" | "bigIncrements" | "integer" | "bigInteger" | "smallInteger" | "tinyInteger" | "mediumInteger"
        | "unsignedBigInteger" | "unsignedInteger" | "unsignedSmallInteger" | "unsignedTinyInteger" | "foreignId" | "year" => {
            ("int".into(), None)
        }
        "boolean" => ("bool".into(), None),
        "float" | "double" => ("float".into(), None),
        "date" | "dateTime" | "dateTimeTz" | "timestamp" | "timestampTz" => (format!("\\{CARBON}"), Some(CARBON.into())),
        _ => ("string".into(), None),
    }
}

/// `'key' => 'value'` / `'key' => Enum::class` pairs inside a model's casts.
fn model_casts(source: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let start = source.find("function casts(").or_else(|| source.find("$casts"));
    let Some(start) = start else { return out };
    let Some(open) = source[start..].find('[').map(|i| start + i) else { return out };
    let close = source[open..].find("];").map_or(source.len(), |i| open + i);
    for line in source[open..close].lines() {
        let Some((key, value)) = line.split_once("=>") else { continue };
        let key = key.trim().trim_matches(['\'', '"']).to_string();
        let value = value.trim().trim_end_matches(',').trim();
        let value = match value.strip_suffix("::class") {
            Some(class) => class.to_string(),
            None => value.trim_matches(['\'', '"']).to_string(),
        };
        out.insert(key, value);
    }
    out
}

fn model_table(source: &str, class: &ClassInfo) -> String {
    source
        .find("$table")
        .and_then(|i| {
            let rest = &source[i..];
            let line = &rest[..rest.find(';')?];
            let eq = line.find('=')?;
            Some(line[eq + 1..].trim().trim_matches(['\'', '"']).to_string())
        })
        .filter(|t| !t.is_empty() && !t.contains(' '))
        .unwrap_or_else(|| table_name(&class.name))
}

const RELATIONS: &[(&str, bool)] = &[
    ("belongsTo", false), ("hasOne", false), ("morphOne", false), ("morphTo", false), ("hasOneThrough", false),
    ("hasMany", true), ("belongsToMany", true), ("morphMany", true), ("morphToMany", true), ("morphedByMany", true),
    ("hasManyThrough", true),
];

/// Relation methods of a model: (method name, name range, related FQN, to-many).
fn model_relations(path: &Path, source: &str) -> Vec<(String, Range<usize>, Option<String>, bool)> {
    let Some(doc) = Document::parse(path, source) else { return Vec::new() };
    let mut out = Vec::new();
    visit(doc.tree.root_node(), &mut |method| {
        if method.kind() != "method_declaration" {
            return;
        }
        let Some(name) = method.child_by_field_name("name") else { return };
        let mut found = None;
        visit(method, &mut |call| {
            if found.is_some() || call.kind() != "member_call_expression" {
                return;
            }
            let object = call.child_by_field_name("object").map(|o| text(o, source));
            let Some(rel) = call.child_by_field_name("name").map(|n| text(n, source)) else { return };
            if object != Some("$this") {
                return;
            }
            if let Some((_, many)) = RELATIONS.iter().find(|(r, _)| *r == rel) {
                let related = arguments(call).first().and_then(|arg| {
                    let raw = text(*arg, source);
                    let class = raw.strip_suffix("::class")?;
                    doc.ctx.resolve_class(class)
                });
                found = Some((related, *many));
            }
        });
        if let Some((related, many)) = found {
            out.push((text(name, source).to_string(), name.byte_range(), related, many));
        }
    });
    out
}

fn property(name: String, doc_type: String, type_fqn: Option<String>, source: Option<Arc<Path>>, range: Range<usize>) -> Member {
    Member {
        name,
        kind: MemberKind::Property,
        is_static: false,
        range,
        type_fqn,
        detail: doc_type,
        virtual_member: true,
        source,
    }
}

/// Project models (not vendor) found in the index.
pub fn project_models(index: &Index, root: &Path) -> Vec<Arc<ClassInfo>> {
    let vendor = root.join("vendor");
    index
        .classes()
        .filter(|c| !c.path.starts_with(&vendor))
        .filter(|c| index.ancestors(&c.fqn).iter().skip(1).any(|a| a.fqn.eq_ignore_ascii_case(MODEL)))
        .cloned()
        .collect()
}

/// Columns and relations reported by `php artisan model:show --json` (needs a database).
fn model_show(root: &Path, fqn: &str) -> Option<(Vec<(String, String, bool, Option<String>)>, Vec<(String, String)>)> {
    let output = process::command(crate::php_binary()).args(["artisan", "model:show", fqn, "--json"]).current_dir(root).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
    let attributes = json["attributes"]
        .as_array()?
        .iter()
        .map(|a| {
            (
                a["name"].as_str().unwrap_or_default().to_string(),
                a["type"].as_str().unwrap_or_default().to_string(),
                a["nullable"].as_bool().unwrap_or(false),
                a["cast"].as_str().map(str::to_string),
            )
        })
        .collect();
    let relations = json["relations"]
        .as_array()
        .map(|rels| {
            rels.iter()
                .map(|r| (r["name"].as_str().unwrap_or_default().to_string(), r["related"].as_str().unwrap_or_default().to_string()))
                .collect()
        })
        .unwrap_or_default();
    Some((attributes, relations))
}

/// Normalizes a database column type (`varchar(255)`, `bigint unsigned`) to a blueprint-ish name.
fn db_type(ty: &str) -> &'static str {
    let ty = ty.to_ascii_lowercase();
    if ty.starts_with("tinyint(1)") || ty == "boolean" || ty == "bool" {
        "boolean"
    } else if ty.contains("int") {
        "integer"
    } else if ty.contains("decimal") || ty.contains("numeric") {
        "decimal"
    } else if ty.contains("float") || ty.contains("double") || ty.contains("real") {
        "float"
    } else if ty.contains("timestamp") || ty.contains("datetime") {
        "timestamp"
    } else if ty == "date" {
        "date"
    } else {
        "string"
    }
}

#[derive(Debug, Clone)]
pub struct ModelHelp {
    pub fqn: String,
    pub members: Vec<Member>,
}

/// Computes helper members for every project model.
pub fn model_helpers(index: &Index, root: &Path, settings: &HelperSettings) -> Vec<ModelHelp> {
    let schema = match settings.source {
        HelperSource::Migrations => schema_from_migrations(root),
        HelperSource::Database => Schema::new(),
    };
    project_models(index, root)
        .into_iter()
        .filter_map(|class| {
            let source = fs::read_to_string(&class.path).ok()?;
            let casts = model_casts(&source);
            let mut members = Vec::new();
            let mut columns: Vec<(String, String, bool, Option<Arc<Path>>, Range<usize>)> = Vec::new();
            let mut relations = model_relations(&class.path, &source);
            match settings.source {
                HelperSource::Migrations => {
                    let table = model_table(&source, &class);
                    for column in schema.get(&table).into_iter().flatten() {
                        columns.push((column.name.clone(), column.ty.clone(), column.nullable, Some(Arc::from(column.path.as_path())), column.range.clone()));
                    }
                }
                HelperSource::Database => {
                    if let Some((attributes, db_relations)) = model_show(root, &class.fqn) {
                        for (name, ty, nullable, cast) in attributes {
                            let ty = cast.filter(|c| !c.is_empty()).map(|c| c.to_string()).unwrap_or_else(|| db_type(&ty).to_string());
                            columns.push((name, ty, nullable, None, class.range.clone()));
                        }
                        for (name, related) in db_relations {
                            if !relations.iter().any(|r| r.0 == name) {
                                relations.push((name, class.range.clone(), Some(related), false));
                            }
                        }
                    }
                }
            }
            for (name, ty, nullable, path, range) in &columns {
                let (doc_type, fqn) = php_type(ty, casts.get(name).map(String::as_str));
                let doc_type = if *nullable { format!("{doc_type}|null") } else { doc_type };
                members.push(property(name.clone(), doc_type, fqn, path.clone(), range.clone()));
                if settings.where_methods {
                    members.push(Member {
                        name: format!("where{}", studly(name)),
                        kind: MemberKind::Method,
                        is_static: true,
                        range: range.clone(),
                        type_fqn: Some(BUILDER.into()),
                        detail: "($value)".into(),
                        virtual_member: true,
                        source: path.clone(),
                    });
                }
            }
            for (name, range, related, many) in relations {
                let (doc_type, fqn) = if many {
                    let element = related.as_deref().map(|r| format!("\\{r}")).unwrap_or_else(|| "mixed".into());
                    (format!("\\{COLLECTION}<int, {element}>"), Some(COLLECTION.to_string()))
                } else {
                    let r = related.clone().unwrap_or_default();
                    (format!("\\{r}|null"), related.clone())
                };
                if many && settings.relation_counts {
                    members.push(property(format!("{name}_count"), "int|null".into(), None, None, range.clone()));
                }
                members.push(property(name, doc_type, fqn, None, range));
            }
            Some(ModelHelp { fqn: class.fqn.clone(), members })
        })
        .collect()
}

/// Removes `@property`, `@property-read`, `@property-write` and `@method` lines from model
/// docblocks (Laravel Idea's "Remove Model PhpDocs"). Returns the new source if it changed.
pub fn remove_model_phpdocs(source: &str) -> Option<String> {
    let class_at = source.find("\nclass ").or_else(|| source.find("\nfinal class ")).or_else(|| source.find("\nabstract class "))?;
    let doc_end = source[..class_at].rfind("*/")?;
    let doc_start = source[..doc_end].rfind("/**")?;
    // The docblock must directly precede the class (attributes allowed in between).
    let between = &source[doc_end + 2..class_at];
    if between.lines().any(|l| !l.trim().is_empty() && !l.trim().starts_with("#[")) {
        return None;
    }
    let doc = &source[doc_start..doc_end + 2];
    let kept: Vec<&str> = doc
        .lines()
        .filter(|line| {
            let tag = line.trim().trim_start_matches('*').trim();
            !(tag.starts_with("@property") || tag.starts_with("@method"))
        })
        .collect();
    if kept.len() == doc.lines().count() {
        return None;
    }
    // Drop the docblock when only its delimiters (and blank lines) remain.
    let empty = kept.iter().all(|l| matches!(l.trim(), "/**" | "*/" | "*" | ""));
    let new_doc = if empty { String::new() } else { kept.join("\n") };
    let mut end = doc_end + 2;
    if empty && source[end..].starts_with('\n') {
        end += 1;
    }
    Some(format!("{}{}{}", &source[..doc_start], new_doc, &source[end..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrations_to_schema() {
        let create = "<?php\nreturn new class extends Migration {\n public function up(): void {\n  Schema::create('posts', function (Blueprint $table) {\n   $table->id();\n   $table->foreignId('user_id')->constrained();\n   $table->string('title');\n   $table->text('body')->nullable();\n   $table->boolean('draft')->default(true);\n   $table->morphs('imageable');\n   $table->timestamps();\n   $table->softDeletes();\n   $table->index(['title']);\n  });\n }\n public function down(): void { Schema::dropIfExists('posts'); }\n};\n";
        let alter = "<?php\nreturn new class extends Migration {\n public function up(): void {\n  Schema::table('posts', function (Blueprint $table) {\n   $table->dropColumn(['draft']);\n   $table->renameColumn('body', 'content');\n   $table->string('slug')->nullable()->unique();\n  });\n }\n};\n";
        let mut schema = Schema::new();
        apply_migration(Path::new("/m/1.php"), create, &mut schema);
        apply_migration(Path::new("/m/2.php"), alter, &mut schema);
        let names: Vec<(&str, &str, bool)> = schema["posts"].iter().map(|c| (c.name.as_str(), c.ty.as_str(), c.nullable)).collect();
        assert_eq!(
            names,
            [
                ("id", "id", false),
                ("user_id", "foreignId", false),
                ("title", "string", false),
                ("content", "text", true),
                ("imageable_id", "unsignedBigInteger", false),
                ("imageable_type", "string", false),
                ("created_at", "timestamp", true),
                ("updated_at", "timestamp", true),
                ("deleted_at", "timestamp", true),
                ("slug", "string", true),
            ]
        );
        let title = &schema["posts"][2];
        assert_eq!(&create[title.range.clone()], "title");
    }

    #[test]
    fn casts_types_and_relations() {
        let model = "<?php\nnamespace App\\Models;\nuse App\\Enums\\Status;\nclass Post extends Model {\n protected $table = 'blog_posts';\n protected function casts(): array {\n  return [\n   'published_at' => 'datetime',\n   'status' => Status::class,\n   'meta' => 'array',\n  ];\n }\n public function user(): BelongsTo { return $this->belongsTo(User::class); }\n public function tags() { return $this->belongsToMany(Tag::class)->withTimestamps(); }\n}\n";
        let casts = model_casts(model);
        assert_eq!(casts["published_at"], "datetime");
        assert_eq!(casts["status"], "Status");
        assert_eq!(php_type("timestamp", None).1.as_deref(), Some(CARBON));
        assert_eq!(php_type("string", Some("array")).0, "array");
        let relations = model_relations(Path::new("/p/Post.php"), model);
        let summary: Vec<_> = relations.iter().map(|(n, _, r, many)| (n.as_str(), r.as_deref(), *many)).collect();
        assert_eq!(summary, [("user", Some("App\\Models\\User"), false), ("tags", Some("App\\Models\\Tag"), true)]);
    }

    #[test]
    fn removes_phpdocs() {
        let src = "<?php\n\n/**\n * @property int $id\n * @property-read User $user\n * @method static Builder whereId($v)\n */\nclass Post extends Model {}\n";
        assert_eq!(remove_model_phpdocs(src).unwrap(), "<?php\n\nclass Post extends Model {}\n");
        let mixed = "<?php\n/**\n * Blog post.\n * @property int $id\n */\n#[Fillable(['x'])]\nclass Post extends Model {}\n";
        assert_eq!(remove_model_phpdocs(mixed).unwrap(), "<?php\n/**\n * Blog post.\n */\n#[Fillable(['x'])]\nclass Post extends Model {}\n");
        assert_eq!(remove_model_phpdocs("<?php\nclass A {}\n"), None);
    }
}
