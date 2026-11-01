//! Reading the live database schema through Artisan (`db:show`, `db:table`), for
//! "New Eloquent Model from Database Table".

use std::path::Path;

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
    let output = process::command(crate::php_binary())
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
    match run_php(root, &["tables"], "") {
        Ok(value) => {
            let mut tables: Vec<String> =
                value["tables"].as_array().map(|t| t.iter().filter_map(|t| t.as_str().map(str::to_string)).collect()).unwrap_or_default();
            tables.retain(|t| t != "migrations");
            tables.sort();
            Ok(tables)
        }
        // Older Laravel without Schema::getTables(): fall back to artisan.
        Err(err) => artisan_json(root, &["db:show"]).map(|json| parse_tables(&json)).map_err(|_| err),
    }
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
    match run_php(root, &["columns", table], "") {
        Ok(value) => Ok(value["columns"]
            .as_array()
            .map(|columns| {
                columns
                    .iter()
                    .map(|c| DbColumn {
                        name: c["name"].as_str().unwrap_or_default().to_string(),
                        ty: c["type"].as_str().unwrap_or_default().to_ascii_lowercase(),
                        nullable: c["nullable"].as_bool().unwrap_or(false),
                        default: c["default"].as_str().map(str::to_string),
                    })
                    .collect()
            })
            .unwrap_or_default()),
        Err(err) => artisan_json(root, &["db:table", table]).map(|json| parse_columns(&json)).map_err(|_| err),
    }
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

/// Result of running SQL through the project's own Laravel database connection.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct QueryResult {
    pub columns: Vec<String>,
    /// Cell values as text; `None` is SQL NULL. At most [`MAX_ROWS`] rows.
    pub rows: Vec<Vec<Option<String>>>,
    /// Rows the statement returned before truncation.
    pub total: usize,
    pub affected: usize,
    pub millis: f64,
}

pub const MAX_ROWS: usize = 500;

/// Boots the app and runs the SQL read from stdin on the default connection (via PDO, so
/// it works for every driver Laravel supports).
const RUNNER: &str = r#"
require 'vendor/autoload.php';
$app = require 'bootstrap/app.php';
$app->make(Illuminate\Contracts\Console\Kernel::class)->bootstrap();
use Illuminate\Support\Facades\DB;
use Illuminate\Support\Facades\Schema;

$mode = $argv[1] ?? 'query';
$name = config('database.default');
$key = "database.connections.$name";
$networked = in_array(config("$key.driver"), ['mysql', 'mariadb', 'pgsql', 'sqlsrv'], true);
// Like PhpStorm's data sources: connect over TCP from this machine. With `localhost`,
// PDO MySQL would use a unix socket that doesn't exist when the server runs in Docker.
if ($networked && config("$key.host") === 'localhost' && !config("$key.unix_socket")) {
    config(["$key.host" => '127.0.0.1']);
    // The app may have opened the connection while booting.
    DB::purge($name);
}
function phpcraft_connect(string $key, string $name, bool $networked) {
    try {
        $c = DB::connection();
        $c->getPdo();
        return $c;
    } catch (Throwable $e) {
        // A Docker service host (mysql, db, pgsql…): try the port published on this machine.
        if ($networked && config("$key.host") !== '127.0.0.1') {
            config(["$key.host" => '127.0.0.1']);
            DB::purge($name);
            try {
                $c = DB::connection();
                $c->getPdo();
                return $c;
            } catch (Throwable $ignored) {
            }
        }
        throw $e;
    }
}
$t = microtime(true);
try {
    $connection = phpcraft_connect($key, $name, $networked);
    if ($mode === 'config') {
        echo json_encode(config($key));
        exit;
    }
    if ($mode === 'info') {
        echo json_encode(['driver' => $connection->getDriverName(), 'database' => $connection->getDatabaseName(), 'name' => $name, 'host' => config("$key.host"), 'port' => config("$key.port")]);
        exit;
    }
    if ($mode === 'tables') {
        $tables = method_exists(Schema::getFacadeRoot(), 'getTables')
            ? array_map(fn ($t) => $t['name'], Schema::getTables())
            : Schema::getTableListing();
        echo json_encode(['tables' => array_values($tables)]);
        exit;
    }
    if ($mode === 'columns') {
        $columns = array_map(fn ($c) => ['name' => $c['name'], 'type' => $c['type'] ?? $c['type_name'] ?? '', 'nullable' => (bool) ($c['nullable'] ?? false), 'default' => $c['default'] ?? null], Schema::getColumns($argv[2] ?? ''));
        echo json_encode(['columns' => $columns]);
        exit;
    }
    $sql = stream_get_contents(STDIN);
    $stmt = $connection->getPdo()->query($sql);
    $columns = [];
    for ($i = 0; $i < $stmt->columnCount(); $i++) { $columns[] = $stmt->getColumnMeta($i)['name'] ?? "col$i"; }
    $rows = $stmt->columnCount() ? $stmt->fetchAll(PDO::FETCH_NUM) : [];
    $total = count($rows);
    $rows = array_map(fn ($r) => array_map(fn ($v) => $v === null ? null : (is_resource($v) ? '(binary)' : (string) $v), $r), array_slice($rows, 0, MAX_ROWS));
    echo json_encode(['columns' => $columns, 'rows' => $rows, 'total' => $total, 'affected' => $stmt->rowCount(), 'ms' => (microtime(true) - $t) * 1000], JSON_INVALID_UTF8_SUBSTITUTE | JSON_PARTIAL_OUTPUT_ON_ERROR);
} catch (Throwable $e) {
    $host = config("$key.host");
    $port = config("$key.port");
    echo json_encode(['error' => $e->getMessage() . ($networked ? " (connection '$name' at $host:$port)" : '')]);
}
"#;

fn run_php(root: &Path, args: &[&str], stdin: &str) -> Result<serde_json::Value, String> {
    use std::io::Write;
    let code = RUNNER.replace("MAX_ROWS", &MAX_ROWS.to_string());
    let mut child = process::command(crate::php_binary())
        .arg("-r")
        .arg(code)
        .args(args)
        .current_dir(root)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("php: {e}"))?;
    child.stdin.take().ok_or("no stdin")?.write_all(stdin.as_bytes()).map_err(|e| e.to_string())?;
    let output = child.wait_with_output().map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&output.stdout);
    let value: serde_json::Value = serde_json::from_str(text.trim()).map_err(|_| {
        let err = String::from_utf8_lossy(&output.stderr);
        format!("{}{}", text.trim(), err.trim()).chars().take(2000).collect::<String>()
    })?;
    if let Some(error) = value["error"].as_str() {
        return Err(error.to_string());
    }
    Ok(value)
}

/// The default connection's settings from `config/database.php` (including the password).
pub fn laravel_connection_config(root: &Path) -> Result<serde_json::Value, String> {
    run_php_raw(root, &["config"])
}

fn run_php_raw(root: &Path, args: &[&str]) -> Result<serde_json::Value, String> {
    run_php(root, args, "")
}

/// `mysql · shop` for the default connection.
pub fn connection_info(root: &Path) -> Result<(String, String), String> {
    let value = run_php(root, &["info"], "")?;
    Ok((value["driver"].as_str().unwrap_or("").to_string(), value["database"].as_str().unwrap_or("").to_string()))
}

/// Runs one SQL statement on the project's default connection.
pub fn run_query(root: &Path, sql: &str) -> Result<QueryResult, String> {
    let value = run_php(root, &["query"], sql)?;
    Ok(parse_result(&value))
}

pub(crate) fn parse_result(value: &serde_json::Value) -> QueryResult {
    let text = |v: &serde_json::Value| match v {
        serde_json::Value::Null => None,
        serde_json::Value::String(s) => Some(s.clone()),
        other => Some(other.to_string()),
    };
    QueryResult {
        columns: value["columns"].as_array().map(|c| c.iter().map(|c| c.as_str().unwrap_or("").to_string()).collect()).unwrap_or_default(),
        rows: value["rows"].as_array().map(|rows| rows.iter().map(|r| r.as_array().map(|r| r.iter().map(text).collect()).unwrap_or_default()).collect()).unwrap_or_default(),
        total: value["total"].as_u64().unwrap_or(0) as usize,
        affected: value["affected"].as_u64().unwrap_or(0) as usize,
        millis: value["ms"].as_f64().unwrap_or(0.),
    }
}

/// Quotes an identifier for `driver` (`mysql` uses backticks).
pub fn quote_ident(driver: &str, name: &str) -> String {
    match driver {
        "mysql" | "mariadb" => format!("`{}`", name.replace('`', "``")),
        "sqlsrv" => format!("[{}]", name.replace(']', "]]")),
        _ => format!("\"{}\"", name.replace('"', "\"\"")),
    }
}

/// `SELECT * FROM <table> [WHERE …] LIMIT n` for the data view.
pub fn select_sql(driver: &str, table: &str, filter: &str, limit: usize) -> String {
    let filter = filter.trim();
    let where_clause = if filter.is_empty() { String::new() } else { format!(" WHERE {filter}") };
    if driver == "sqlsrv" {
        format!("SELECT TOP {limit} * FROM {}{where_clause}", quote_ident(driver, table))
    } else {
        format!("SELECT * FROM {}{where_clause} LIMIT {limit}", quote_ident(driver, table))
    }
}

#[cfg(test)]
mod query_tests {
    use super::*;

    #[test]
    fn builds_and_parses() {
        assert_eq!(select_sql("mysql", "users", "", 100), "SELECT * FROM `users` LIMIT 100");
        assert_eq!(select_sql("pgsql", "users", "id > 5", 10), "SELECT * FROM \"users\" WHERE id > 5 LIMIT 10");
        assert_eq!(select_sql("sqlsrv", "users", "", 5), "SELECT TOP 5 * FROM [users]");
        let value = serde_json::json!({"columns": ["id", "name"], "rows": [[1, "Ann"], [2, null]], "total": 2, "affected": 2, "ms": 1.5});
        let result = parse_result(&value);
        assert_eq!(result.rows, vec![vec![Some("1".to_string()), Some("Ann".to_string())], vec![Some("2".to_string()), None]]);
        assert_eq!(result.columns, ["id", "name"]);
    }
}

#[cfg(test)]
mod live {
    /// `PHPCRAFT_LARAVEL_APP=/path cargo test -p laravel -- --ignored live_query`
    #[test]
    #[ignore]
    fn live_query() {
        let root = std::path::PathBuf::from(std::env::var("PHPCRAFT_LARAVEL_APP").unwrap());
        let (driver, db) = super::connection_info(&root).unwrap();
        eprintln!("driver={driver} db={db}");
        let tables = super::list_tables(&root).unwrap();
        eprintln!("tables={tables:?}");
        let result = super::run_query(&root, &super::select_sql(&driver, &tables[0], "", 5)).unwrap();
        eprintln!("columns={:?} rows={}", result.columns, result.rows.len());
        assert!(!result.columns.is_empty());
    }
}
