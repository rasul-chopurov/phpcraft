//! Model specification for the Make dialog: primary key type, timestamps, soft deletes,
//! columns and relations. Drives the model, migration, factory and pivot-table output.

use super::{plural, snake};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum IdType {
    #[default]
    Id,
    Uuid,
    Ulid,
}

impl IdType {
    pub const ALL: [IdType; 3] = [Self::Id, Self::Uuid, Self::Ulid];

    pub fn label(self) -> &'static str {
        match self {
            Self::Id => "id (auto-increment)",
            Self::Uuid => "UUID",
            Self::Ulid => "ULID",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelationKind {
    BelongsTo,
    HasOne,
    HasMany,
    BelongsToMany,
    HasOneThrough,
    HasManyThrough,
    MorphTo,
    MorphOne,
    MorphMany,
    MorphToMany,
    MorphedByMany,
}

impl RelationKind {
    pub const ALL: [RelationKind; 11] = [
        Self::BelongsTo,
        Self::HasOne,
        Self::HasMany,
        Self::BelongsToMany,
        Self::HasOneThrough,
        Self::HasManyThrough,
        Self::MorphTo,
        Self::MorphOne,
        Self::MorphMany,
        Self::MorphToMany,
        Self::MorphedByMany,
    ];

    /// Name as written in Eloquent, e.g. `belongsToMany`.
    pub fn method(self) -> &'static str {
        match self {
            Self::BelongsTo => "belongsTo",
            Self::HasOne => "hasOne",
            Self::HasMany => "hasMany",
            Self::BelongsToMany => "belongsToMany",
            Self::HasOneThrough => "hasOneThrough",
            Self::HasManyThrough => "hasManyThrough",
            Self::MorphTo => "morphTo",
            Self::MorphOne => "morphOne",
            Self::MorphMany => "morphMany",
            Self::MorphToMany => "morphToMany",
            Self::MorphedByMany => "morphedByMany",
        }
    }

    fn class(self) -> String {
        let mut chars = self.method().chars();
        chars.next().map(|c| c.to_uppercase().chain(chars).collect()).unwrap_or_default()
    }

    fn is_many(self) -> bool {
        matches!(
            self,
            Self::HasMany | Self::BelongsToMany | Self::HasManyThrough | Self::MorphMany | Self::MorphToMany | Self::MorphedByMany
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Relation {
    pub kind: RelationKind,
    /// Related model, or the morph name for `morphTo`.
    pub target: String,
    /// `through` model for has-through relations; morph name for morphOne/Many/ToMany.
    pub extra: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    pub name: String,
    /// Blueprint column method: `string`, `text`, `decimal`, ...
    pub column: String,
    pub nullable: bool,
    pub unique: bool,
    pub default: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelSpec {
    pub id: IdType,
    pub timestamps: bool,
    pub soft_deletes: bool,
    pub fields: Vec<Field>,
    pub relations: Vec<Relation>,
}

impl Default for ModelSpec {
    fn default() -> Self {
        Self {
            id: IdType::Id,
            timestamps: true,
            soft_deletes: false,
            fields: Vec::new(),
            relations: Vec::new(),
        }
    }
}

fn normalize_column(column: &str) -> String {
    match column.to_ascii_lowercase().as_str() {
        "str" | "varchar" => "string".into(),
        "int" => "integer".into(),
        "bigint" => "bigInteger".into(),
        "bool" => "boolean".into(),
        "datetime" => "dateTime".into(),
        "float" | "double" => "float".into(),
        "money" => "decimal".into(),
        "longtext" => "longText".into(),
        "unsignedbiginteger" => "unsignedBigInteger".into(),
        _ => column.to_string(),
    }
}

/// Parses `title:string, body:text:nullable, slug:string:unique, price:decimal:default(0)`.
pub fn parse_fields(input: &str) -> Vec<Field> {
    input
        .split([',', '\n'])
        .filter_map(|item| {
            let mut parts = item.trim().split(':').map(str::trim);
            let name = snake(parts.next().filter(|n| !n.is_empty())?);
            let mut field = Field {
                name,
                column: "string".into(),
                nullable: false,
                unique: false,
                default: None,
            };
            for (i, part) in parts.enumerate() {
                match part {
                    "nullable" | "null" | "?" => field.nullable = true,
                    "unique" => field.unique = true,
                    p if p.starts_with("default(") && p.ends_with(')') => {
                        field.default = Some(p[8..p.len() - 1].to_string());
                    }
                    p if i == 0 && !p.is_empty() => field.column = normalize_column(p),
                    _ => {}
                }
            }
            Some(field)
        })
        .collect()
}

/// Parses `belongsTo:User, hasMany:Comment, morphMany:Image:imageable, hasManyThrough:Deployment:Environment`.
pub fn parse_relations(input: &str) -> Vec<Relation> {
    input
        .split([',', '\n'])
        .filter_map(|item| {
            let mut parts = item.trim().split(':').map(str::trim).filter(|p| !p.is_empty());
            let method = parts.next()?;
            let kind = RelationKind::ALL.into_iter().find(|k| k.method().eq_ignore_ascii_case(method))?;
            let target = parts.next()?.to_string();
            let extra = parts.next().map(str::to_string);
            Some(Relation { kind, target, extra })
        })
        .collect()
}

fn camel(s: &str) -> String {
    let mut chars = s.chars();
    chars.next().map(|c| c.to_lowercase().chain(chars).collect()).unwrap_or_default()
}

fn studly_plural(model: &str) -> String {
    let words = snake(model);
    let (head, last) = words.rsplit_once('_').map_or(("", words.as_str()), |(h, l)| (h, l));
    let plural = if head.is_empty() { plural(last) } else { format!("{head}_{}", plural(last)) };
    super::studly(&plural)
}

/// Method name for a relation: `user`, `comments`, `commentable`.
pub fn relation_name(relation: &Relation) -> String {
    if relation.kind == RelationKind::MorphTo {
        return camel(&relation.target);
    }
    let base = if relation.kind.is_many() { studly_plural(&relation.target) } else { relation.target.clone() };
    camel(&base)
}

/// `post_tag` for Post + Tag (alphabetical, singular snake case).
pub fn pivot_table(a: &str, b: &str) -> String {
    let mut names = [snake(a), snake(b)];
    names.sort();
    names.join("_")
}

pub fn foreign_key(model: &str) -> String {
    format!("{}_id", snake(model))
}

/// The model class body for `spec`.
pub fn model_source(namespace: &str, name: &str, factory: bool, spec: &ModelSpec) -> String {
    let mut uses = vec!["Illuminate\\Database\\Eloquent\\Model".to_string()];
    let mut traits = Vec::new();
    if factory {
        uses.push("Illuminate\\Database\\Eloquent\\Factories\\HasFactory".into());
        traits.push("HasFactory");
    }
    match spec.id {
        IdType::Uuid => {
            uses.push("Illuminate\\Database\\Eloquent\\Concerns\\HasUuids".into());
            traits.push("HasUuids");
        }
        IdType::Ulid => {
            uses.push("Illuminate\\Database\\Eloquent\\Concerns\\HasUlids".into());
            traits.push("HasUlids");
        }
        IdType::Id => {}
    }
    if spec.soft_deletes {
        uses.push("Illuminate\\Database\\Eloquent\\SoftDeletes".into());
        traits.push("SoftDeletes");
    }
    for relation in &spec.relations {
        uses.push(format!("Illuminate\\Database\\Eloquent\\Relations\\{}", relation.kind.class()));
    }
    uses.sort_by_key(|u| u.to_ascii_lowercase());
    uses.dedup();

    let mut body = Vec::new();
    if !traits.is_empty() {
        body.push(format!("    use {};\n", traits.join(", ")));
    }
    if !spec.timestamps {
        body.push("    public $timestamps = false;\n".into());
    }
    let mut fillable: Vec<String> = spec.fields.iter().map(|f| f.name.clone()).collect();
    for relation in spec.relations.iter().filter(|r| r.kind == RelationKind::BelongsTo) {
        fillable.push(foreign_key(&relation.target));
    }
    let fillable = fillable.iter().map(|f| format!("        '{f}',\n")).collect::<String>();
    body.push(format!(
        "    /**\n     * The attributes that are mass assignable.\n     *\n     * @var list<string>\n     */\n    protected $fillable = [\n{fillable}    ];\n"
    ));
    let casts: Vec<String> = spec
        .fields
        .iter()
        .filter_map(|f| {
            let cast = match f.column.as_str() {
                "boolean" => "boolean",
                "json" | "jsonb" => "array",
                "date" => "date",
                "dateTime" | "timestamp" | "dateTimeTz" | "timestampTz" => "datetime",
                "decimal" => "decimal:2",
                _ => return None,
            };
            Some(format!("            '{}' => '{cast}',\n", f.name))
        })
        .collect();
    if !casts.is_empty() {
        body.push(format!(
            "    /**\n     * Get the attributes that should be cast.\n     *\n     * @return array<string, string>\n     */\n    protected function casts(): array\n    {{\n        return [\n{}        ];\n    }}\n",
            casts.concat()
        ));
    }
    for relation in &spec.relations {
        let class = relation.kind.class();
        let method = relation_name(relation);
        let call = match relation.kind {
            RelationKind::MorphTo => "morphTo()".to_string(),
            RelationKind::HasOneThrough | RelationKind::HasManyThrough => format!(
                "{}({}::class, {}::class)",
                relation.kind.method(),
                relation.target,
                relation.extra.as_deref().unwrap_or("Model")
            ),
            RelationKind::MorphOne | RelationKind::MorphMany | RelationKind::MorphToMany | RelationKind::MorphedByMany => {
                let morph = relation.extra.clone().unwrap_or_else(|| format!("{}able", snake(name)));
                format!("{}({}::class, '{morph}')", relation.kind.method(), relation.target)
            }
            _ => format!("{}({}::class)", relation.kind.method(), relation.target),
        };
        let generic = match relation.kind {
            RelationKind::MorphTo => "<Model, $this>".to_string(),
            RelationKind::HasOneThrough | RelationKind::HasManyThrough => {
                format!("<{}, {}, $this>", relation.target, relation.extra.as_deref().unwrap_or("Model"))
            }
            _ => format!("<{}, $this>", relation.target),
        };
        body.push(format!(
            "    /**\n     * @return {class}{generic}\n     */\n    public function {method}(): {class}\n    {{\n        return $this->{call};\n    }}\n"
        ));
    }
    let uses = uses.iter().map(|u| format!("use {u};\n")).collect::<String>();
    format!(
        "<?php\n\nnamespace {namespace};\n\n{uses}\nclass {name} extends Model\n{{\n{}}}\n",
        body.join("\n")
    )
}

fn column_line(field: &Field) -> String {
    let mut line = match field.column.as_str() {
        "decimal" => format!("$table->decimal('{}', 10, 2)", field.name),
        "foreignId" => format!("$table->foreignId('{}')", field.name),
        column => format!("$table->{column}('{}')", field.name),
    };
    if field.nullable {
        line.push_str("->nullable()");
    }
    if field.unique {
        line.push_str("->unique()");
    }
    if let Some(default) = &field.default {
        line.push_str(&format!("->default({default})"));
    }
    format!("            {line};\n")
}

/// The `Schema::create` body for the model's table.
pub fn create_columns(spec: &ModelSpec) -> String {
    let mut out = String::new();
    out.push_str(match spec.id {
        IdType::Id => "            $table->id();\n",
        IdType::Uuid => "            $table->uuid('id')->primary();\n",
        IdType::Ulid => "            $table->ulid('id')->primary();\n",
    });
    for relation in &spec.relations {
        match relation.kind {
            RelationKind::BelongsTo => out.push_str(&format!(
                "            $table->foreignId('{}')->constrained()->cascadeOnDelete();\n",
                foreign_key(&relation.target)
            )),
            RelationKind::MorphTo => {
                let method = match spec.id {
                    IdType::Id => "morphs",
                    IdType::Uuid => "uuidMorphs",
                    IdType::Ulid => "ulidMorphs",
                };
                out.push_str(&format!("            $table->{method}('{}');\n", snake(&relation.target)));
            }
            _ => {}
        }
    }
    for field in &spec.fields {
        out.push_str(&column_line(field));
    }
    if spec.timestamps {
        out.push_str("            $table->timestamps();\n");
    }
    if spec.soft_deletes {
        out.push_str("            $table->softDeletes();\n");
    }
    out
}

/// Pivot migration columns for two models and their key types.
pub fn pivot_columns(a: (&str, IdType), b: (&str, IdType)) -> String {
    let column = |id: IdType| match id {
        IdType::Id => "foreignId",
        IdType::Uuid => "foreignUuid",
        IdType::Ulid => "foreignUlid",
    };
    let mut keys = [(foreign_key(a.0), column(a.1)), (foreign_key(b.0), column(b.1))];
    keys.sort();
    let [(first, first_col), (second, second_col)] = keys;
    format!(
        "            $table->{first_col}('{first}')->constrained()->cascadeOnDelete();\n            $table->{second_col}('{second}')->constrained()->cascadeOnDelete();\n            $table->primary(['{first}', '{second}']);\n"
    )
}

/// Factory `definition()` entries and the models they reference.
pub fn factory_definition(spec: &ModelSpec) -> (String, Vec<String>) {
    let mut lines = String::new();
    let mut models = Vec::new();
    for relation in spec.relations.iter().filter(|r| r.kind == RelationKind::BelongsTo) {
        lines.push_str(&format!("            '{}' => {}::factory(),\n", foreign_key(&relation.target), relation.target));
        models.push(relation.target.clone());
    }
    for field in &spec.fields {
        let name = field.name.as_str();
        let value = match (name, field.column.as_str()) {
            ("name", _) => "fake()->name()",
            ("email", _) => "fake()->unique()->safeEmail()",
            ("title", _) => "fake()->sentence()",
            ("slug", _) => "fake()->unique()->slug()",
            ("phone", _) => "fake()->phoneNumber()",
            ("address", _) => "fake()->address()",
            ("url" | "website", _) => "fake()->url()",
            (_, "text" | "longText" | "mediumText") => "fake()->paragraph()",
            (_, "boolean") => "fake()->boolean()",
            (_, "integer" | "bigInteger" | "unsignedBigInteger" | "smallInteger" | "tinyInteger") => "fake()->numberBetween(1, 100)",
            (_, "decimal" | "float") => "fake()->randomFloat(2, 1, 1000)",
            (_, "date") => "fake()->date()",
            (_, "dateTime" | "timestamp" | "dateTimeTz" | "timestampTz") => "fake()->dateTime()",
            (_, "json" | "jsonb") => "[]",
            (_, "uuid") => "fake()->uuid()",
            _ => "fake()->word()",
        };
        lines.push_str(&format!("            '{name}' => {value},\n"));
    }
    if lines.is_empty() {
        lines.push_str("            //\n");
    }
    (lines, models)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fields_and_relations() {
        let fields = parse_fields("title:string, body:text:nullable, isPublished:bool:default(false), price:decimal:unique");
        assert_eq!(fields.len(), 4);
        assert_eq!(fields[2].name, "is_published");
        assert_eq!(fields[2].column, "boolean");
        assert_eq!(fields[2].default.as_deref(), Some("false"));
        assert!(fields[1].nullable && fields[3].unique);

        let relations = parse_relations("belongsTo:User, hasMany:Comment, belongsToMany:Tag, morphMany:Image:imageable, hasManyThrough:Deployment:Environment, bogus:X");
        let names: Vec<_> = relations.iter().map(relation_name).collect();
        assert_eq!(names, ["user", "comments", "tags", "images", "deployments"]);
        assert_eq!(pivot_table("Tag", "Post"), "post_tag");
        assert_eq!(pivot_table("BlogPost", "Category"), "blog_post_category");
    }
}
