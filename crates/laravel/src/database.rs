//! Reading the live database schema through Artisan (`db:show`, `db:table`), for
//! "New Eloquent Model from Database Table".

use std::path::Path;
use std::process::Command;

use crate::make::{Field, IdType, ModelSpec, Relation, RelationKind, studly, table_name};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DbColumn {
    pub name: String,
    /// Database type, e.g. `varchar`, `bigint unsigned`, `tinyint(1)`.
    pub ty: String,
    pub nullable: bool,
    pub default: Option<String>,
}

fn artisan_json(root: &Path, args: &[&str]) -> Result<serde_json::Value, String> {
    let output = Command::new("php")
        .arg("artisan")
        .args(args)
        .arg("--json")
        .current_dir(root)
        .output()
        .map_err(|e| format!("failed to run php: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        return Err(if stderr.trim().is_empty() { stdout.into_owned() } else { stderr.into_owned() });
    }
    serde_json::from_slice(&output.stdout).map_err(|e| format!("unexpected artisan output: {e}"))
}

/// Tables of the default connection (blocking).
pub fn list_tables(root: &Path) -> Result<Vec<String>, String> {
    let json = artisan_json(root, &["db:show"])?;
    Ok(parse_tables(&json))
}

fn parse_tables(json: &serde_json::Value) -> Vec<String> {
    let mut tables: Vec<String> = json["tables"]
        .as_array()
        .map(|t| t.iter().filter_map(|t| t["table"].as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    tables.retain(|t| t != "migrations");
    tables.sort();
    tables
}

/// Columns of one table (blocking).
pub fn table_columns(root: &Path, table: &str) -> Result<Vec<DbColumn>, String> {
    let json = artisan_json(root, &["db:table", table])?;
    Ok(parse_columns(&json))
}

fn parse_columns(json: &serde_json::Value) -> Vec<DbColumn> {
    json["columns"]
        .as_array()
        .map(|columns| {
            columns
                .iter()
                .map(|c| {
                    // `attributes` is a list or an object of flags like "nullable".
                    let attributes: Vec<String> = match &c["attributes"] {
                        serde_json::Value::Array(a) => a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect(),
                        serde_json::Value::Object(o) => o.values().filter_map(|v| v.as_str().map(str::to_string)).collect(),
                        _ => Vec::new(),
                    };
                    DbColumn {
                        name: c["column"].as_str().unwrap_or_default().to_string(),
                        ty: c["type"].as_str().unwrap_or_default().to_ascii_lowercase(),
                        nullable: attributes.iter().any(|a| a == "nullable"),
                        default: c["default"].as_str().map(str::to_string),
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

/// `blog_posts` -> `BlogPost`.
pub fn model_name(table: &str) -> String {
    let (head, last) = table.rsplit_once('_').map_or(("", table), |(h, l)| (h, l));
    let singular = singular(last);
    studly(&if head.is_empty() { singular } else { format!("{head}_{singular}") })
}

fn singular(word: &str) -> String {
    const IRREGULAR: &[(&str, &str)] = &[("people", "person"), ("men", "man"), ("children", "child"), ("data", "datum"), ("media", "medium")];
    if let Some((_, s)) = IRREGULAR.iter().find(|(p, _)| *p == word) {
        return s.to_string();
    }
    if let Some(stem) = word.strip_suffix("ies") {
        return format!("{stem}y");
    }
    for suffix in ["sses", "xes", "ches", "shes", "zes"] {
        if word.ends_with(suffix) {
            return word[..word.len() - 2].to_string();
        }
    }
    if word.ends_with('s') && !word.ends_with("ss") {
        return word[..word.len() - 1].to_string();
    }
    word.to_string()
}

fn blueprint_type(db: &str) -> &'static str {
    let ty = db.to_ascii_lowercase();
    match () {
        _ if ty.starts_with("tinyint(1)") || ty == "boolean" || ty == "bool" => "boolean",
        _ if ty.contains("bigint") => "bigInteger",
        _ if ty.contains("int") => "integer",
        _ if ty.contains("decimal") || ty.contains("numeric") => "decimal",
        _ if ty.contains("double") || ty.contains("float") || ty.contains("real") => "float",
        _ if ty.contains("timestamp") => "timestamp",
        _ if ty.contains("datetime") => "dateTime",
        _ if ty == "date" => "date",
        _ if ty == "time" => "time",
        _ if ty.contains("json") => "json",
        _ if ty.contains("uuid") => "uuid",
        _ if ty.contains("text") => "text",
        _ if ty.contains("blob") || ty.contains("binary") => "binary",
        _ => "string",
    }
}

/// Model name and a dialog prefill for an existing table: key type, timestamps, soft
/// deletes, `*_id` columns as belongsTo relations, the rest as fields.
pub fn spec_from_table(table: &str, columns: &[DbColumn]) -> (String, ModelSpec) {
    let model = model_name(table);
    let has = |name: &str| columns.iter().any(|c| c.name == name);
    let mut spec = ModelSpec {
        timestamps: has("created_at") && has("updated_at"),
        soft_deletes: has("deleted_at"),
        with_id: has("id"),
        table: (table_name(&model) != table).then(|| table.to_string()),
        ..ModelSpec::default()
    };
    if let Some(id) = columns.iter().find(|c| c.name == "id") {
        spec.id = if id.ty.contains("uuid") || id.ty.contains("char(36)") {
            IdType::Uuid
        } else if id.ty.contains("char(26)") {
            IdType::Ulid
        } else {
            IdType::Id
        };
    }
    let skip = ["id", "created_at", "updated_at", "deleted_at"];
    for column in columns.iter().filter(|c| !skip.contains(&c.name.as_str())) {
        if let Some(target) = column.name.strip_suffix("_id")
            && !target.is_empty()
            && blueprint_type(&column.ty).contains("nteger")
        {
            spec.relations.push(Relation { kind: RelationKind::BelongsTo, target: studly(target), extra: None, target_fqn: None });
            continue;
        }
        spec.fields.push(Field {
            name: column.name.clone(),
            column: blueprint_type(&column.ty).to_string(),
            nullable: column.nullable,
            unique: false,
            index: false,
            default: None,
        });
    }
    (model, spec)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_artisan_output_and_builds_spec() {
        let show: serde_json::Value = serde_json::from_str(r#"{"platform":{},"tables":[{"table":"users"},{"table":"migrations"},{"table":"blog_categories"}]}"#).unwrap();
        assert_eq!(parse_tables(&show), ["blog_categories", "users"]);
        let table: serde_json::Value = serde_json::from_str(r#"{"columns":[
            {"column":"id","attributes":{"0":"integer","2":"autoincrement"},"type":"integer"},
            {"column":"user_id","attributes":["integer"],"type":"bigint"},
            {"column":"title","attributes":["varchar"],"type":"varchar"},
            {"column":"published_at","attributes":{"0":"datetime","3":"nullable"},"type":"datetime"},
            {"column":"is_draft","attributes":["tinyint(1)"],"type":"tinyint(1)"},
            {"column":"created_at","attributes":{"3":"nullable"},"type":"datetime"},
            {"column":"updated_at","attributes":{"3":"nullable"},"type":"datetime"},
            {"column":"deleted_at","attributes":{"3":"nullable"},"type":"datetime"}
        ]}"#).unwrap();
        let columns = parse_columns(&table);
        assert!(columns[3].nullable && !columns[2].nullable);
        let (model, spec) = spec_from_table("blog_categories", &columns);
        assert_eq!(model, "BlogCategory");
        assert!(spec.timestamps && spec.soft_deletes && spec.with_id && spec.table.is_none());
        assert_eq!(spec.relations[0].target, "User");
        let fields: Vec<_> = spec.fields.iter().map(|f| (f.name.as_str(), f.column.as_str(), f.nullable)).collect();
        assert_eq!(fields, [("title", "string", false), ("published_at", "dateTime", true), ("is_draft", "boolean", false)]);
        assert_eq!(model_name("people"), "Person");
        assert_eq!(model_name("addresses"), "Address");
    }
}
