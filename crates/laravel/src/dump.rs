//! "Export to SQL": dumps a data source with the database's own tool found on this system
//! (mysqldump / mariadb-dump, pg_dump, sqlite3), looking in PATH and in the usual Homebrew,
//! Herd, DBngin, MAMP and Postgres.app locations.

use std::path::{Path, PathBuf};

use crate::datasource::{ConnectionType, DataSource, Driver};

fn candidates(tool: &str) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    for dir in [
        "/opt/homebrew/bin",
        "/usr/local/bin",
        "/opt/homebrew/opt/mysql-client/bin",
        "/usr/local/opt/mysql-client/bin",
        "/opt/homebrew/opt/libpq/bin",
        "/usr/local/opt/libpq/bin",
        "/Applications/Postgres.app/Contents/Versions/latest/bin",
        "/Applications/MAMP/Library/bin",
        "/usr/bin",
    ] {
        dirs.push(dir.into());
    }
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        // Herd and DBngin keep their binaries in versioned folders.
        for base in [home.join("Library/Application Support/Herd/bin"), PathBuf::from("/Users/Shared/DBngin")] {
            dirs.push(base.clone());
            if let Ok(entries) = std::fs::read_dir(&base) {
                dirs.extend(entries.flatten().map(|e| e.path().join("bin")));
            }
        }
    }
    let exe = if cfg!(windows) { format!("{tool}.exe") } else { tool.to_string() };
    dirs.into_iter().map(|d| d.join(&exe)).collect()
}

/// The first installed tool among `names`.
pub fn find_tool(names: &[&str]) -> Option<PathBuf> {
    names.iter().flat_map(|n| candidates(n)).find(|p| p.is_file())
}

/// The dump command for `source`, writing SQL to `out`.
pub fn command(root: &Path, source: &DataSource, password: &str, out: &Path) -> Result<std::process::Command, String> {
    let missing = |what: &str| format!("{what} was not found. Install it (e.g. `brew install {what}`) or add it to PATH.");
    match source.driver {
        Driver::Mysql | Driver::Mariadb => {
            let tool = find_tool(&["mysqldump", "mariadb-dump"]).ok_or_else(|| missing("mysqldump"))?;
            let mut cmd = process::command(tool);
            cmd.env("MYSQL_PWD", password).arg("--single-transaction").arg("--routines").arg("--triggers").arg(format!("--result-file={}", out.display()));
            match source.connection {
                ConnectionType::UnixSocket => cmd.arg(format!("--socket={}", source.path)),
                _ => cmd.arg(format!("--host={}", if source.host == "localhost" { "127.0.0.1" } else { &source.host })).arg(format!("--port={}", source.port)),
            };
            if !source.user.is_empty() {
                cmd.arg(format!("--user={}", source.user));
            }
            if source.database.is_empty() {
                return Err("Set the database name in the data source to export it.".into());
            }
            cmd.arg(&source.database);
            Ok(cmd)
        }
        Driver::Pgsql => {
            let tool = find_tool(&["pg_dump"]).ok_or_else(|| missing("pg_dump"))?;
            let mut cmd = process::command(tool);
            cmd.env("PGPASSWORD", password).arg("--no-owner").arg("--file").arg(out);
            let host = if source.connection == ConnectionType::UnixSocket { source.path.clone() } else { source.host.clone() };
            cmd.arg("--host").arg(host).arg("--port").arg(source.port.to_string());
            if !source.user.is_empty() {
                cmd.arg("--username").arg(&source.user);
            }
            if !source.database.is_empty() {
                cmd.arg(&source.database);
            }
            Ok(cmd)
        }
        Driver::Sqlite => {
            let tool = find_tool(&["sqlite3"]).ok_or_else(|| missing("sqlite3"))?;
            let db = Path::new(&source.path);
            let db = if db.is_absolute() { db.to_path_buf() } else { root.join(db) };
            let mut cmd = process::command(tool);
            cmd.arg(db).arg(format!(".output {}", out.display())).arg(".dump");
            Ok(cmd)
        }
        Driver::Sqlsrv => Err("SQL Server export isn't supported yet: use SQL Server Management Studio or sqlpackage.".into()),
        Driver::Redis => Err("Redis has no SQL export.".into()),
    }
}

/// Runs the dump (blocking); returns the file size on success.
pub fn export(root: &Path, source: &DataSource, password: &str, out: &Path) -> Result<u64, String> {
    let output = command(root, source, password, out)?.current_dir(root).output().map_err(|e| e.to_string())?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        return Err(err.lines().filter(|l| !l.contains("Using a password")).last().unwrap_or("export failed").to_string());
    }
    std::fs::metadata(out).map(|m| m.len()).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_commands() {
        let mut source = DataSource::new(Driver::Mysql);
        source.database = "shop".into();
        if let Ok(cmd) = command(Path::new("/p"), &source, "secret", Path::new("/tmp/shop.sql")) {
            let args: Vec<String> = cmd.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
            assert!(args.contains(&"--host=127.0.0.1".to_string()) && args.last().map(String::as_str) == Some("shop"));
            assert!(cmd.get_envs().any(|(k, v)| k == "MYSQL_PWD" && v.is_some_and(|v| v == "secret")));
        }
        assert!(command(Path::new("/p"), &DataSource::new(Driver::Redis), "", Path::new("/tmp/x.sql")).is_err());
    }
}
