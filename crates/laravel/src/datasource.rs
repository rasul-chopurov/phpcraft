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
    /// SQL Server (PDO's `sqlsrv` driver).
    Sqlsrv,
    /// Redis: keys are listed like tables; the console runs commands.
    Redis,
}

impl Driver {
    pub const ALL: [Driver; 6] = [Driver::Mysql, Driver::Mariadb, Driver::Pgsql, Driver::Sqlite, Driver::Sqlsrv, Driver::Redis];

    /// The name Laravel's query grammar uses (identifier quoting).
    pub fn grammar(self) -> &'static str {
        match self {
            Driver::Mysql | Driver::Mariadb | Driver::Redis => "mysql",
            Driver::Pgsql => "pgsql",
            Driver::Sqlite => "sqlite",
            Driver::Sqlsrv => "sqlsrv",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Driver::Mysql => "MySQL",
            Driver::Mariadb => "MariaDB",
            Driver::Pgsql => "PostgreSQL",
            Driver::Sqlite => "SQLite",
            Driver::Sqlsrv => "SQL Server",
            Driver::Redis => "Redis",
        }
    }

    pub fn default_port(self) -> u16 {
        match self {
            Driver::Mysql | Driver::Mariadb => 3306,
            Driver::Pgsql => 5432,
            Driver::Sqlite => 0,
            Driver::Sqlsrv => 1433,
            Driver::Redis => 6379,
        }
    }

    fn from_laravel(name: &str) -> Option<Driver> {
        Some(match name {
            "mysql" => Driver::Mysql,
            "mariadb" => Driver::Mariadb,
            "pgsql" => Driver::Pgsql,
            "sqlite" => Driver::Sqlite,
            "sqlsrv" => Driver::Sqlsrv,
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
            user: match driver {
                Driver::Pgsql => "postgres".into(),
                Driver::Sqlsrv => "sa".into(),
                Driver::Redis => String::new(),
                _ => "root".into(),
            },
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
            Driver::Sqlsrv => {
                let db = if self.database.is_empty() { String::new() } else { format!(";Database={}", self.database) };
                format!("sqlsrv:Server={},{}{db}", self.host, self.port)
            }
            Driver::Redis => format!("redis://{}:{}/{}", self.host, self.port, if self.database.is_empty() { "0" } else { &self.database }),
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
            'sqlsrv' => "SELECT TABLE_NAME FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_TYPE IN ('BASE TABLE','VIEW') ORDER BY TABLE_NAME",
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
        } elseif ($driver === 'sqlsrv') {
            $s = $pdo->prepare("SELECT c.COLUMN_NAME, c.DATA_TYPE, c.IS_NULLABLE, c.COLUMN_DEFAULT, (SELECT COUNT(*) FROM INFORMATION_SCHEMA.TABLE_CONSTRAINTS tc JOIN INFORMATION_SCHEMA.KEY_COLUMN_USAGE k ON tc.CONSTRAINT_NAME = k.CONSTRAINT_NAME WHERE tc.CONSTRAINT_TYPE = 'PRIMARY KEY' AND k.TABLE_NAME = c.TABLE_NAME AND k.COLUMN_NAME = c.COLUMN_NAME) AS PK FROM INFORMATION_SCHEMA.COLUMNS c WHERE c.TABLE_NAME = ? ORDER BY c.ORDINAL_POSITION");
            $s->execute([$t]);
            $cols = array_map(fn ($r) => ['name' => $r['COLUMN_NAME'], 'type' => $r['DATA_TYPE'], 'nullable' => $r['IS_NULLABLE'] === 'YES', 'default' => $r['COLUMN_DEFAULT'], 'pk' => $r['PK'] > 0], $s->fetchAll(PDO::FETCH_ASSOC));
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
    let mut child = process::command(crate::php_binary())
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
fn redis(source: &DataSource, password: &str) -> Result<crate::redis::Connection, String> {
    crate::redis::Connection::open(&source.host, source.port, password, &source.user, &source.database)
}

pub fn test(root: &Path, source: &DataSource, password: &str) -> Result<String, String> {
    if source.driver == Driver::Redis {
        let info = redis(source, password)?.command(&["INFO", "server"])?;
        let version = match info {
            crate::redis::Reply::Bulk(Some(text)) => text.lines().find_map(|l| l.strip_prefix("redis_version:")).unwrap_or("").to_string(),
            _ => String::new(),
        };
        return Ok(format!("redis {version}"));
    }
    let value = run(root, source, password, serde_json::json!({"op": "test"}))?;
    Ok(format!("{} {}", value["driver"].as_str().unwrap_or(""), value["version"].as_str().unwrap_or("")))
}

pub fn tables(root: &Path, source: &DataSource, password: &str) -> Result<Vec<String>, String> {
    if source.driver == Driver::Redis {
        return redis(source, password)?.keys();
    }
    let value = run(root, source, password, serde_json::json!({"op": "tables"}))?;
    Ok(value["tables"].as_array().map(|t| t.iter().filter_map(|t| t.as_str().map(str::to_string)).collect()).unwrap_or_default())
}

/// Columns with primary-key flags.
pub fn columns(root: &Path, source: &DataSource, password: &str, table: &str) -> Result<Vec<(DbColumn, bool)>, String> {
    if source.driver == Driver::Redis {
        // Redis values have no schema (and no primary key: the grid is read-only).
        return Ok(Vec::new());
    }
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
    if source.driver == Driver::Redis {
        let mut connection = redis(source, password)?;
        // The data view asks for `SELECT * FROM `key` …`: show the key's value.
        if let Some(rest) = sql.trim().strip_prefix("SELECT * FROM `")
            && let Some(end) = rest.find("` ").or_else(|| rest.strip_suffix('`').map(|r| r.len()))
        {
            return connection.value(&rest[..end].replace("``", "`"));
        }
        return connection.run_line(sql);
    }
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
    let laravel = driver.grammar();
    let q = |name: &str| crate::database::quote_ident(laravel, name);
    let condition = edit.key.iter().map(|(k, v)| if v.is_some() { format!("{} = ?", q(k)) } else { format!("{} IS NULL", q(k)) }).collect::<Vec<_>>().join(" AND ");
    let sql = format!("UPDATE {} SET {} = ? WHERE {condition}", q(table), q(&edit.column));
    let mut params = vec![edit.value.clone()];
    params.extend(edit.key.iter().filter(|(_, v)| v.is_some()).map(|(_, v)| v.clone()));
    (sql, params)
}

/// A parameterised statement for [`execute`].
#[derive(Debug, Clone, PartialEq)]
pub struct Statement {
    pub sql: String,
    pub params: Vec<Option<String>>,
}

fn quoter(driver: Driver) -> impl Fn(&str) -> String {
    let laravel = driver.grammar();
    move |name: &str| crate::database::quote_ident(laravel, name)
}

fn key_condition(driver: Driver, key: &[(String, Option<String>)]) -> (String, Vec<Option<String>>) {
    let q = quoter(driver);
    let sql = key.iter().map(|(k, v)| if v.is_some() { format!("{} = ?", q(k)) } else { format!("{} IS NULL", q(k)) }).collect::<Vec<_>>().join(" AND ");
    (sql, key.iter().filter(|(_, v)| v.is_some()).map(|(_, v)| v.clone()).collect())
}

/// `DELETE FROM table WHERE <primary key>`.
pub fn delete_statement(driver: Driver, table: &str, key: &[(String, Option<String>)]) -> Statement {
    let (condition, params) = key_condition(driver, key);
    Statement { sql: format!("DELETE FROM {} WHERE {condition}", quoter(driver)(table)), params }
}

/// `INSERT INTO table (…) VALUES (…)`, or `DEFAULT VALUES` when nothing was filled in.
pub fn insert_statement(driver: Driver, table: &str, values: &[(String, Option<String>)]) -> Statement {
    let q = quoter(driver);
    if values.is_empty() {
        let sql = match driver {
            Driver::Mysql | Driver::Mariadb => format!("INSERT INTO {} () VALUES ()", q(table)),
            _ => format!("INSERT INTO {} DEFAULT VALUES", q(table)),
        };
        return Statement { sql, params: Vec::new() };
    }
    let columns = values.iter().map(|(c, _)| q(c)).collect::<Vec<_>>().join(", ");
    let marks = vec!["?"; values.len()].join(", ");
    Statement { sql: format!("INSERT INTO {} ({columns}) VALUES ({marks})", q(table)), params: values.iter().map(|(_, v)| v.clone()).collect() }
}

/// Runs statements in one transaction; returns affected rows.
pub fn execute(root: &Path, source: &DataSource, password: &str, statements: &[Statement]) -> Result<usize, String> {
    if source.driver == Driver::Redis {
        return Err("Redis values are changed with commands in the console (SET, HSET, …).".into());
    }
    let statements: Vec<serde_json::Value> = statements.iter().map(|s| serde_json::json!({"sql": s.sql, "params": s.params})).collect();
    let value = run(root, source, password, serde_json::json!({"op": "execute", "statements": statements}))?;
    Ok(value["affected"].as_u64().unwrap_or(0) as usize)
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

/// "Import from .env": a data source from the `DB_*` variables, without booting the app.
pub fn from_env(root: &Path) -> Result<(DataSource, String), String> {
    let env = crate::dashboard::env_values(root);
    if env.is_empty() {
        return Err("No .env in the project".into());
    }
    let get = |k: &str| crate::dashboard::env_value(&env, k).unwrap_or_default();
    let connection = get("DB_CONNECTION");
    let driver = Driver::from_laravel(if connection.is_empty() { "mysql" } else { &connection }).ok_or_else(|| format!("Unsupported DB_CONNECTION={connection}"))?;
    let mut source = DataSource::new(driver);
    if driver == Driver::Sqlite {
        let db = get("DB_DATABASE");
        source.path = if db.is_empty() { "database/database.sqlite".into() } else { db };
        source.name = ".env (sqlite)".into();
    } else {
        let host = get("DB_HOST");
        source.host = if host.is_empty() || host == "localhost" { "127.0.0.1".into() } else { host };
        source.port = get("DB_PORT").parse().unwrap_or(driver.default_port());
        source.database = get("DB_DATABASE");
        source.user = get("DB_USERNAME");
        source.name = format!("{}@{}", source.database, source.host);
    }
    Ok((source, get("DB_PASSWORD")))
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
        assert_eq!(lite.dsn(root), format!("sqlite:{}", root.join("database/database.sqlite").display()));
        let edit = CellEdit { column: "name".into(), value: Some("Ann".into()), key: vec![("id".into(), Some("7".into()))] };
        assert_eq!(update_statement(Driver::Mysql, "users", &edit), ("UPDATE `users` SET `name` = ? WHERE `id` = ?".to_string(), vec![Some("Ann".to_string()), Some("7".to_string())]));
        let del = delete_statement(Driver::Mysql, "users", &[("id".into(), Some("7".into()))]);
        assert_eq!((del.sql.as_str(), del.params.clone()), ("DELETE FROM `users` WHERE `id` = ?", vec![Some("7".to_string())]));
        let ins = insert_statement(Driver::Pgsql, "users", &[("name".into(), Some("Ann".into())), ("email".into(), None)]);
        assert_eq!(ins.sql, "INSERT INTO \"users\" (\"name\", \"email\") VALUES (?, ?)");
        assert_eq!(insert_statement(Driver::Sqlite, "t", &[]).sql, "INSERT INTO \"t\" DEFAULT VALUES");
        assert_eq!(update_statement(Driver::Pgsql, "users", &edit).0, "UPDATE \"users\" SET \"name\" = ? WHERE \"id\" = ?");
        let env_root = std::env::temp_dir().join(format!("phpcraft-env-{}", std::process::id()));
        std::fs::create_dir_all(&env_root).unwrap();
        std::fs::write(env_root.join(".env"), "DB_CONNECTION=pgsql\nDB_HOST=localhost\nDB_PORT=5433\nDB_DATABASE=shop\nDB_USERNAME=app\nDB_PASSWORD=secret\n").unwrap();
        let (ds, password) = from_env(&env_root).unwrap();
        assert_eq!((ds.driver, ds.host.as_str(), ds.port, ds.database.as_str(), ds.user.as_str(), password.as_str()), (Driver::Pgsql, "127.0.0.1", 5433, "shop", "app", "secret"));
        std::fs::remove_dir_all(env_root).unwrap();
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
