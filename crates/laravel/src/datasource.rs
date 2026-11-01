//! Database data sources like PhpStorm's: MySQL, MariaDB, PostgreSQL and SQLite connections
//! saved per project in `.phpcraft/datasources.json` (passwords are kept by the caller,
//! outside the project). Every operation runs through PHP's PDO, so no native drivers.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::database::{DbColumn, QueryResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Driver {
    Mysql,
    Mariadb,
    Pgsql,
    Sqlite,
}

impl Driver {
    pub const ALL: [Driver; 4] = [Driver::Mysql, Driver::Mariadb, Driver::Pgsql, Driver::Sqlite];

    pub fn label(self) -> &'static str {
        match self {
            Driver::Mysql => "MySQL",
            Driver::Mariadb => "MariaDB",
            Driver::Pgsql => "PostgreSQL",
            Driver::Sqlite => "SQLite",
        }
    }

    pub fn default_port(self) -> u16 {
        match self {
            Driver::Mysql | Driver::Mariadb => 3306,
            Driver::Pgsql => 5432,
            Driver::Sqlite => 0,
        }
    }

    fn from_laravel(name: &str) -> Option<Driver> {
        Some(match name {
            "mysql" => Driver::Mysql,
            "mariadb" => Driver::Mariadb,
            "pgsql" => Driver::Pgsql,
            "sqlite" => Driver::Sqlite,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionType {
    /// Host and port over TCP.
    Default,
    UnixSocket,
    /// A full PDO DSN typed by the user.
    Url,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DataSource {
    pub id: String,
    pub name: String,
    pub driver: Driver,
    pub connection: ConnectionType,
    pub host: String,
    pub port: u16,
    pub user: String,
    pub database: String,
    /// SQLite file (relative to the project) or the unix socket path.
    pub path: String,
    /// PDO DSN for `ConnectionType::Url`.
    pub url: String,
}

impl Default for DataSource {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            driver: Driver::Mysql,
            connection: ConnectionType::Default,
            host: "127.0.0.1".into(),
            port: 3306,
            user: "root".into(),
            database: String::new(),
            path: String::new(),
            url: String::new(),
        }
    }
}

impl DataSource {
    pub fn new(driver: Driver) -> Self {
        let id = format!("{:x}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
        Self {
            id,
            name: if driver == Driver::Sqlite { "SQLite".into() } else { "@127.0.0.1".into() },
            driver,
            port: driver.default_port(),
            user: if driver == Driver::Pgsql { "postgres".into() } else { "root".into() },
            ..Default::default()
        }
    }

    /// The PDO DSN (like the JDBC URL shown in PhpStorm's dialog).
    pub fn dsn(&self, root: &Path) -> String {
        if self.connection == ConnectionType::Url && !self.url.trim().is_empty() {
            return self.url.trim().to_string();
        }
        let db = if self.database.is_empty() { String::new() } else { format!(";dbname={}", self.database) };
        match self.driver {
            Driver::Sqlite => {
                let path = Path::new(&self.path);
                let full = if path.is_absolute() { path.to_path_buf() } else { root.join(path) };
                format!("sqlite:{}", full.display())
            }
            Driver::Mysql | Driver::Mariadb => match self.connection {
                ConnectionType::UnixSocket => format!("mysql:unix_socket={}{db};charset=utf8mb4", self.path),
                _ => {
                    // `localhost` would make PDO use a socket; PhpStorm talks TCP.
                    let host = if self.host == "localhost" { "127.0.0.1" } else { &self.host };
                    format!("mysql:host={host};port={}{db};charset=utf8mb4", self.port)
                }
            },
            Driver::Pgsql => match self.connection {
                ConnectionType::UnixSocket => format!("pgsql:host={}{db}", self.path),
                _ => format!("pgsql:host={};port={}{db}", self.host, self.port),
            },
        }
    }
}

fn file(root: &Path) -> PathBuf {
    root.join(".phpcraft").join("datasources.json")
}

pub fn load(root: &Path) -> Vec<DataSource> {
    std::fs::read_to_string(file(root)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

pub fn save(root: &Path, sources: &[DataSource]) -> std::io::Result<()> {
    let path = file(root);
    std::fs::create_dir_all(path.parent().unwrap())?;
    std::fs::write(path, serde_json::to_string_pretty(sources).unwrap_or_default())
}

/// PDO work: connects with the DSN, then runs one operation. Input arrives as JSON on stdin.
const PDO_RUNNER: &str = r#"
$in = json_decode(stream_get_contents(STDIN), true);
$t = microtime(true);
try {
    $pdo = new PDO($in['dsn'], $in['user'] ?: null, $in['password'] ?: null, [PDO::ATTR_ERRMODE => PDO::ERRMODE_EXCEPTION, PDO::ATTR_TIMEOUT => 5]);
    $driver = $pdo->getAttribute(PDO::ATTR_DRIVER_NAME);
    $op = $in['op'];
    if ($op === 'test') {
        echo json_encode(['ok' => true, 'version' => $pdo->getAttribute(PDO::ATTR_SERVER_VERSION), 'driver' => $driver]);
        exit;
    }
    if ($op === 'tables') {
        $sql = match ($driver) {
            'mysql' => 'SHOW FULL TABLES',
            'pgsql' => "SELECT table_name FROM information_schema.tables WHERE table_schema = current_schema() ORDER BY table_name",
            'sqlite' => "SELECT name FROM sqlite_master WHERE type IN ('table','view') AND name NOT LIKE 'sqlite_%' ORDER BY name",
        };
        $tables = array_map(fn ($r) => $r[0], $pdo->query($sql)->fetchAll(PDO::FETCH_NUM));
        echo json_encode(['tables' => $tables]);
        exit;
    }
    if ($op === 'columns') {
        $t = $in['table'];
        if ($driver === 'mysql') {
            $rows = $pdo->query('SHOW COLUMNS FROM `' . str_replace('`', '``', $t) . '`')->fetchAll(PDO::FETCH_ASSOC);
            $cols = array_map(fn ($r) => ['name' => $r['Field'], 'type' => $r['Type'], 'nullable' => $r['Null'] === 'YES', 'default' => $r['Default'], 'pk' => $r['Key'] === 'PRI'], $rows);
        } elseif ($driver === 'pgsql') {
            $s = $pdo->prepare("SELECT c.column_name, c.data_type, c.is_nullable, c.column_default, (SELECT COUNT(*) FROM information_schema.table_constraints tc JOIN information_schema.key_column_usage k ON tc.constraint_name = k.constraint_name AND tc.table_schema = k.table_schema WHERE tc.constraint_type = 'PRIMARY KEY' AND k.table_name = c.table_name AND k.column_name = c.column_name AND tc.table_schema = c.table_schema) AS pk FROM information_schema.columns c WHERE c.table_schema = current_schema() AND c.table_name = ? ORDER BY c.ordinal_position");
            $s->execute([$t]);
            $cols = array_map(fn ($r) => ['name' => $r['column_name'], 'type' => $r['data_type'], 'nullable' => $r['is_nullable'] === 'YES', 'default' => $r['column_default'], 'pk' => $r['pk'] > 0], $s->fetchAll(PDO::FETCH_ASSOC));
        } else {
            $rows = $pdo->query('PRAGMA table_info("' . str_replace('"', '""', $t) . '")')->fetchAll(PDO::FETCH_ASSOC);
            $cols = array_map(fn ($r) => ['name' => $r['name'], 'type' => $r['type'], 'nullable' => !$r['notnull'], 'default' => $r['dflt_value'], 'pk' => $r['pk'] > 0], $rows);
        }
        echo json_encode(['columns' => $cols]);
        exit;
    }
    if ($op === 'execute') {
        // Several parameterised statements in one transaction (grid edits).
        $pdo->beginTransaction();
        $affected = 0;
        foreach ($in['statements'] as $st) {
            $s = $pdo->prepare($st['sql']);
            $s->execute($st['params']);
            $affected += $s->rowCount();
        }
        $pdo->commit();
        echo json_encode(['affected' => $affected]);
        exit;
    }
    $stmt = $pdo->query($in['sql']);
    $columns = [];
    for ($i = 0; $i < $stmt->columnCount(); $i++) { $columns[] = $stmt->getColumnMeta($i)['name'] ?? "col$i"; }
    $rows = $stmt->columnCount() ? $stmt->fetchAll(PDO::FETCH_NUM) : [];
    $total = count($rows);
    $rows = array_map(fn ($r) => array_map(fn ($v) => $v === null ? null : (is_resource($v) ? '(binary)' : (string) $v), $r), array_slice($rows, 0, MAX_ROWS));
    echo json_encode(['columns' => $columns, 'rows' => $rows, 'total' => $total, 'affected' => $stmt->rowCount(), 'ms' => (microtime(true) - $t) * 1000], JSON_INVALID_UTF8_SUBSTITUTE | JSON_PARTIAL_OUTPUT_ON_ERROR);
} catch (Throwable $e) {
    if (isset($pdo) && $pdo->inTransaction()) { $pdo->rollBack(); }
    echo json_encode(['error' => $e->getMessage()]);
}
"#;

fn run(root: &Path, source: &DataSource, password: &str, input: serde_json::Value) -> Result<serde_json::Value, String> {
    use std::io::Write;
    let mut input = input;
    input["dsn"] = source.dsn(root).into();
    input["user"] = source.user.clone().into();
    input["password"] = password.into();
    let code = PDO_RUNNER.replace("MAX_ROWS", &crate::database::MAX_ROWS.to_string());
    let mut child = std::process::Command::new(crate::php_binary())
        .arg("-r")
        .arg(code)
        .current_dir(root)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("php: {e}"))?;
    child.stdin.take().ok_or("no stdin")?.write_all(input.to_string().as_bytes()).map_err(|e| e.to_string())?;
    let output = child.wait_with_output().map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&output.stdout);
    let value: serde_json::Value = serde_json::from_str(text.trim())
        .map_err(|_| format!("{}{}", text.trim(), String::from_utf8_lossy(&output.stderr).trim()).chars().take(2000).collect::<String>())?;
    match value["error"].as_str() {
        Some(error) => Err(error.to_string()),
        None => Ok(value),
    }
}

/// "Test Connection": the server version on success.
pub fn test(root: &Path, source: &DataSource, password: &str) -> Result<String, String> {
    let value = run(root, source, password, serde_json::json!({"op": "test"}))?;
    Ok(format!("{} {}", value["driver"].as_str().unwrap_or(""), value["version"].as_str().unwrap_or("")))
}

pub fn tables(root: &Path, source: &DataSource, password: &str) -> Result<Vec<String>, String> {
    let value = run(root, source, password, serde_json::json!({"op": "tables"}))?;
    Ok(value["tables"].as_array().map(|t| t.iter().filter_map(|t| t.as_str().map(str::to_string)).collect()).unwrap_or_default())
}

/// Columns with primary-key flags.
pub fn columns(root: &Path, source: &DataSource, password: &str, table: &str) -> Result<Vec<(DbColumn, bool)>, String> {
    let value = run(root, source, password, serde_json::json!({"op": "columns", "table": table}))?;
    Ok(value["columns"]
        .as_array()
        .map(|cols| {
            cols.iter()
                .map(|c| {
                    let default = match &c["default"] {
                        serde_json::Value::Null => None,
                        serde_json::Value::String(s) => Some(s.clone()),
                        other => Some(other.to_string()),
                    };
                    (
                        DbColumn {
                            name: c["name"].as_str().unwrap_or("").to_string(),
                            ty: c["type"].as_str().unwrap_or("").to_lowercase(),
                            nullable: c["nullable"].as_bool().unwrap_or(false),
                            default,
                        },
                        c["pk"].as_bool().unwrap_or(false),
                    )
                })
                .collect()
        })
        .unwrap_or_default())
}

pub fn query(root: &Path, source: &DataSource, password: &str, sql: &str) -> Result<QueryResult, String> {
    let value = run(root, source, password, serde_json::json!({"op": "query", "sql": sql}))?;
    Ok(crate::database::parse_result(&value))
}

/// A changed cell to save: `UPDATE table SET column = value WHERE <primary key>`.
#[derive(Debug, Clone, PartialEq)]
pub struct CellEdit {
    pub column: String,
    /// `None` sets NULL.
    pub value: Option<String>,
    /// Primary key columns and their current values.
    pub key: Vec<(String, Option<String>)>,
}

/// Builds the parameterised UPDATE for an edit.
pub fn update_statement(driver: Driver, table: &str, edit: &CellEdit) -> (String, Vec<Option<String>>) {
    let laravel = match driver {
        Driver::Mysql | Driver::Mariadb => "mysql",
        Driver::Pgsql => "pgsql",
        Driver::Sqlite => "sqlite",
    };
    let q = |name: &str| crate::database::quote_ident(laravel, name);
    let condition = edit.key.iter().map(|(k, v)| if v.is_some() { format!("{} = ?", q(k)) } else { format!("{} IS NULL", q(k)) }).collect::<Vec<_>>().join(" AND ");
    let sql = format!("UPDATE {} SET {} = ? WHERE {condition}", q(table), q(&edit.column));
    let mut params = vec![edit.value.clone()];
    params.extend(edit.key.iter().filter(|(_, v)| v.is_some()).map(|(_, v)| v.clone()));
    (sql, params)
}

/// Saves grid edits in one transaction ("Submit").
pub fn apply_edits(root: &Path, source: &DataSource, password: &str, table: &str, edits: &[CellEdit]) -> Result<usize, String> {
    let statements: Vec<serde_json::Value> = edits
        .iter()
        .map(|edit| {
            let (sql, params) = update_statement(source.driver, table, edit);
            serde_json::json!({"sql": sql, "params": params})
        })
        .collect();
    let value = run(root, source, password, serde_json::json!({"op": "execute", "statements": statements}))?;
    Ok(value["affected"].as_u64().unwrap_or(0) as usize)
}

/// "Add Application Database": the Laravel default connection as a data source, with its
/// password. Booting the app reads `.env` and `config/database.php` like Laravel does.
pub fn from_laravel(root: &Path) -> Result<(DataSource, String), String> {
    let config = crate::database::laravel_connection_config(root)?;
    let driver = Driver::from_laravel(config["driver"].as_str().unwrap_or("")).ok_or("Unsupported driver in config/database.php")?;
    let text = |k: &str| match &config[k] {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Number(n) => n.to_string(),
        _ => String::new(),
    };
    let mut source = DataSource::new(driver);
    source.name = if driver == Driver::Sqlite { "Laravel (sqlite)".into() } else { format!("{}@{}", text("username"), text("host")) };
    source.host = if text("host") == "localhost" { "127.0.0.1".into() } else { text("host") };
    source.port = text("port").parse().unwrap_or(driver.default_port());
    source.user = text("username");
    source.database = if driver == Driver::Sqlite { String::new() } else { text("database") };
    source.path = if driver == Driver::Sqlite { text("database") } else { String::new() };
    Ok((source, text("password")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dsns_and_updates() {
        let root = Path::new("/p");
        let mut ds = DataSource::new(Driver::Mysql);
        ds.host = "localhost".into();
        ds.database = "shop".into();
        assert_eq!(ds.dsn(root), "mysql:host=127.0.0.1;port=3306;dbname=shop;charset=utf8mb4");
        let mut pg = DataSource::new(Driver::Pgsql);
        pg.database = "app".into();
        assert_eq!(pg.dsn(root), "pgsql:host=127.0.0.1;port=5432;dbname=app");
        let mut lite = DataSource::new(Driver::Sqlite);
        lite.path = "database/database.sqlite".into();
        assert_eq!(lite.dsn(root), "sqlite:/p/database/database.sqlite");
        let edit = CellEdit { column: "name".into(), value: Some("Ann".into()), key: vec![("id".into(), Some("7".into()))] };
        assert_eq!(update_statement(Driver::Mysql, "users", &edit), ("UPDATE `users` SET `name` = ? WHERE `id` = ?".to_string(), vec![Some("Ann".to_string()), Some("7".to_string())]));
        assert_eq!(update_statement(Driver::Pgsql, "users", &edit).0, "UPDATE \"users\" SET \"name\" = ? WHERE \"id\" = ?");
    }
}

#[cfg(test)]
mod live {
    use super::*;

    /// `PHPCRAFT_LARAVEL_APP=/path cargo test -p laravel -- --ignored datasource_live`
    #[test]
    #[ignore]
    fn datasource_live() {
        let root = PathBuf::from(std::env::var("PHPCRAFT_LARAVEL_APP").unwrap());
        let (source, password) = from_laravel(&root).unwrap();
        eprintln!("source={source:?}");
        eprintln!("test={:?}", test(&root, &source, &password));
        let tables = tables(&root, &source, &password).unwrap();
        eprintln!("tables={tables:?}");
        let cols = columns(&root, &source, &password, "tags").unwrap();
        eprintln!("cols={:?}", cols.iter().map(|(c, pk)| (c.name.clone(), *pk)).collect::<Vec<_>>());
        query(&root, &source, &password, "INSERT INTO tags (created_at) VALUES ('2000-01-01')").unwrap();
        let r = query(&root, &source, &password, "SELECT id FROM tags WHERE created_at = '2000-01-01'").unwrap();
        let id = r.rows[0][0].clone();
        let edit = CellEdit { column: "created_at".into(), value: Some("2001-02-03".into()), key: vec![("id".into(), id.clone())] };
        assert_eq!(apply_edits(&root, &source, &password, "tags", &[edit]).unwrap(), 1);
        let r = query(&root, &source, &password, &format!("SELECT created_at FROM tags WHERE id = {}", id.clone().unwrap())).unwrap();
        assert_eq!(r.rows[0][0].as_deref(), Some("2001-02-03"));
        query(&root, &source, &password, &format!("DELETE FROM tags WHERE id = {}", id.unwrap())).unwrap();
    }
}
