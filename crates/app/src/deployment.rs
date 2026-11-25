
//! SFTP deployment: servers saved in `.phpcraft/deployment.json`,
//! upload/download through the system OpenSSH `sftp` (key or password auth), optional
//! upload on save. Passwords live in the PhpCraft config folder, not in the project.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Server {
    pub id: String,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub user: String,
    /// Remote folder that maps to the project root.
    pub remote_root: String,
    /// Private key file; empty uses the SSH agent/default keys or the password.
    pub key: String,
    pub use_password: bool,
}

impl Default for Server {
    fn default() -> Self {
        Self { id: String::new(), name: String::new(), host: String::new(), port: 22, user: String::new(), remote_root: "/var/www/html".into(), key: String::new(), use_password: false }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Deployment {
    pub servers: Vec<Server>,
    /// Server that uploads go to.
    pub default: Option<String>,
    /// Upload a file to the default server whenever it is saved.
    pub auto_upload: bool,
}

fn file(root: &Path) -> PathBuf {
    root.join(".phpcraft").join("deployment.json")
}

pub fn load(root: &Path) -> Deployment {
    std::fs::read_to_string(file(root)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

pub fn save(root: &Path, deployment: &Deployment) -> std::io::Result<()> {
    let path = crate::config::project_dir(root).join("deployment.json");
    std::fs::write(path, serde_json::to_string_pretty(deployment).unwrap_or_default())
}

impl Deployment {
    pub fn default_server(&self) -> Option<&Server> {
        self.default.as_ref().and_then(|id| self.servers.iter().find(|s| &s.id == id)).or(self.servers.first())
    }
}

fn secrets_path() -> PathBuf {
    crate::config::dir().join("deployment_secrets.json")
}

pub fn password(root: &Path, id: &str) -> String {
    std::fs::read_to_string(secrets_path())
        .ok()
        .and_then(|t| serde_json::from_str::<std::collections::HashMap<String, String>>(&t).ok())
        .and_then(|m| m.get(&format!("{}|{id}", root.display())).cloned())
        .unwrap_or_default()
}

pub fn set_password(root: &Path, id: &str, value: &str) {
    let mut map: std::collections::HashMap<String, String> =
        std::fs::read_to_string(secrets_path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    let key = format!("{}|{id}", root.display());
    if value.is_empty() {
        map.remove(&key);
    } else {
        map.insert(key, value.to_string());
    }
    let _ = std::fs::create_dir_all(crate::config::dir());
    let _ = std::fs::write(secrets_path(), serde_json::to_string_pretty(&map).unwrap_or_default());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(secrets_path(), std::fs::Permissions::from_mode(0o600));
    }
}

/// `remote_root/relative` for a project file.
pub fn remote_path(server: &Server, root: &Path, local: &Path) -> Option<String> {
    let rel = local.strip_prefix(root).ok()?.to_string_lossy().replace('\\', "/");
    Some(format!("{}/{}", server.remote_root.trim_end_matches('/'), rel))
}

/// Quotes a path for an sftp batch line.
fn quote(path: &str) -> String {
    format!("\"{}\"", path.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Batch that uploads `local` to `remote`, creating the remote folders first.
pub fn upload_batch(local: &Path, remote: &str) -> String {
    let mut batch = String::new();
    let parts: Vec<&str> = remote.trim_end_matches('/').split('/').collect();
    let mut dir = String::new();
    for part in &parts[..parts.len().saturating_sub(1)] {
        if part.is_empty() {
            continue;
        }
        dir.push('/');
        dir.push_str(part);
        // `-` ignores "already exists".
        batch.push_str(&format!("-mkdir {}\n", quote(&dir)));
    }
    batch.push_str(&format!("put {} {}\n", quote(&local.to_string_lossy()), quote(remote)));
    batch
}

pub fn download_batch(remote: &str, local: &Path) -> String {
    format!("get {} {}\n", quote(remote), quote(&local.to_string_lossy()))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: String,
}

/// Parses `ls -la` output from sftp.
pub fn parse_ls(output: &str) -> Vec<RemoteEntry> {
    let mut entries: Vec<RemoteEntry> = output
        .lines()
        .filter(|l| l.starts_with('d') || l.starts_with('-') || l.starts_with('l'))
        .filter_map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            if fields.len() < 9 {
                return None;
            }
            let name = fields[8..].join(" ");
            let name = name.rsplit('/').next().unwrap_or(&name).to_string();
            (name != "." && name != "..").then(|| RemoteEntry { is_dir: line.starts_with('d'), size: fields[4].to_string(), name })
        })
        .collect();
    entries.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then(a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    entries
}

/// The private key path with `~` expanded (sftp/ssh get it without a shell).
pub fn key_path(server: &Server) -> Option<String> {
    let key = server.key.trim();
    if key.is_empty() {
        return None;
    }
    Some(match (key.strip_prefix("~/"), dirs::home_dir()) {
        (Some(rest), Some(home)) => home.join(rest).display().to_string(),
        _ => key.to_string(),
    })
}

/// Runs an sftp batch against `server`.
pub fn run(server: &Server, password: &str, batch: &str) -> Result<String, String> {
    use std::io::Write;
    let mut command = process::command("sftp");
    command.args(["-b", "-", "-P", &server.port.to_string(), "-o", "StrictHostKeyChecking=accept-new", "-o", "ConnectTimeout=10"]);
    if let Some(key) = key_path(server) {
        command.arg("-i").arg(key);
    }
    // Password auth: OpenSSH asks SSH_ASKPASS, a throwaway script that prints it.
    // The password, or a key's passphrase: OpenSSH asks for either through SSH_ASKPASS.
    let askpass = if (server.use_password || !server.key.trim().is_empty()) && !password.is_empty() {
        // A fresh name per run: parallel runs (listing + test) must not delete each other's script.
        static RUNS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let run = RUNS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let script = std::env::temp_dir().join(format!("phpcraft-askpass-{}-{run}", std::process::id()));
        #[cfg(unix)]
        {
            use std::io::Write as _;
            use std::os::unix::fs::OpenOptionsExt;
            let escaped = password.replace('\'', "'\\''");
            // Created executable and owner-only in one step; never follows an existing file.
            let mut file = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o700).open(&script).map_err(|e| e.to_string())?;
            file.write_all(format!("#!/bin/sh\nprintf '%s\\n' '{escaped}'\n").as_bytes()).map_err(|e| e.to_string())?;
        }
        #[cfg(windows)]
        {
            let script = script.with_extension("cmd");
            std::fs::write(&script, format!("@echo {password}\r\n")).map_err(|e| e.to_string())?;
        }
        command.env("SSH_ASKPASS", &script).env("SSH_ASKPASS_REQUIRE", "force").env("DISPLAY", ":0").args(["-o", "BatchMode=no", "-o", "NumberOfPasswordPrompts=1"]);
        Some(script)
    } else {
        command.args(["-o", "BatchMode=yes"]);
        None
    };
    command.arg("--").arg(format!("{}@{}", server.user, server.host));
    let result = (|| {
        let mut child = command
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| format!("sftp: {e}"))?;
        child.stdin.take().ok_or("no stdin")?.write_all(batch.as_bytes()).map_err(|e| e.to_string())?;
        let output = child.wait_with_output().map_err(|e| e.to_string())?;
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        if output.status.success() {
            Ok(stdout)
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(stderr.lines().filter(|l| !l.trim().is_empty()).last().unwrap_or("sftp failed").to_string())
        }
    })();
    if let Some(script) = askpass {
        let _ = std::fs::remove_file(&script);
        let _ = std::fs::remove_file(script.with_extension("cmd"));
    }
    result
}

pub fn test(server: &Server, password: &str) -> Result<String, String> {
    run(server, password, &format!("cd {}\npwd\n", quote(&server.remote_root))).map(|out| {
        out.lines().find(|l| l.contains("Remote working directory")).map(str::trim).unwrap_or("Connected").to_string()
    })
}

pub fn upload(server: &Server, password: &str, root: &Path, local: &Path) -> Result<String, String> {
    let remote = remote_path(server, root, local).ok_or("The file is outside the project")?;
    run(server, password, &upload_batch(local, &remote))?;
    Ok(remote)
}

pub fn download(server: &Server, password: &str, remote: &str, local: &Path) -> Result<(), String> {
    if let Some(parent) = local.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    run(server, password, &download_batch(remote, local)).map(|_| ())
}

/// Folders that never travel to a server: they are machine-local or rebuilt on the host.
pub const SKIP_DIRS: &[&str] = &[".git", ".idea", ".phpcraft", "node_modules", "vendor"];

/// Every file under `path`; a file yields itself, a folder is walked (skipping [`SKIP_DIRS`]).
///
/// Dragging a folder onto the remote browser uploads what is inside it, the way a
/// file manager would, without a second round trip per file.
pub fn files_under(path: &Path) -> Vec<PathBuf> {
    fn walk(path: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(path) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if path.file_name().is_some_and(|name| SKIP_DIRS.iter().any(|skip| name == *skip)) {
                    continue;
                }
                walk(&path, out);
            } else if path.is_file() {
                out.push(path);
            }
        }
    }
    if path.is_file() {
        return vec![path.to_path_buf()];
    }
    let mut out = Vec::new();
    walk(path, &mut out);
    out.sort();
    out
}

/// The remote path of `local`: inside the project it keeps its place under `remote_root`,
/// outside it lands in `dir` (the folder the file was dropped on).
pub fn remote_for(server: &Server, root: &Path, local: &Path, dir: &str) -> String {
    remote_path(server, root, local).unwrap_or_else(|| {
        let name = local.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "file".into());
        let dir = if dir.trim().is_empty() { server.remote_root.clone() } else { dir.to_string() };
        if dir.ends_with('/') { format!("{dir}{name}") } else { format!("{dir}/{name}") }
    })
}

/// Uploads several files over one connection: a folder expands to the files it holds.
/// Returns how many files went up.
pub fn upload_many(server: &Server, password: &str, pairs: &[(PathBuf, String)]) -> Result<usize, String> {
    let mut batch = String::new();
    let mut count = 0;
    for (local, remote) in pairs {
        for file in files_under(local) {
            let remote = if file.as_path() == local.as_path() {
                remote.clone()
            } else {
                let rel = file.strip_prefix(local).map(|r| r.to_string_lossy().replace('\\', "/")).unwrap_or_default();
                format!("{}/{}", remote.trim_end_matches('/'), rel)
            };
            batch.push_str(&upload_batch(&file, &remote));
            count += 1;
        }
    }
    if count == 0 {
        return Err("Nothing to upload".into());
    }
    run(server, password, &batch)?;
    Ok(count)
}

/// Downloads several remote files over one connection, creating the local folders.
pub fn download_many(server: &Server, password: &str, pairs: &[(String, PathBuf)]) -> Result<usize, String> {
    let mut batch = String::new();
    for (remote, local) in pairs {
        if let Some(parent) = local.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        batch.push_str(&download_batch(remote, local));
    }
    if pairs.is_empty() {
        return Ok(0);
    }
    run(server, password, &batch)?;
    Ok(pairs.len())
}

pub fn list(server: &Server, password: &str, dir: &str) -> Result<Vec<RemoteEntry>, String> {
    run(server, password, &format!("ls -la {}\n", quote(dir))).map(|out| parse_ls(&out))
}

/// `dir/name` without doubling the slash.
fn remote_join(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_string()
    } else {
        format!("{}/{}", dir.trim_end_matches('/'), name)
    }
}

/// Splits the answer of a batch of `ls -la` lines into `(folder, entries)` pairs.
///
/// sftp echoes every command as `sftp> …` before its output, so the folder of each block is
/// the one that was asked for — the alternative would be parsing absolute paths out of the
/// listing, which breaks on names with spaces.
fn parse_ls_batch(output: &str) -> Vec<(String, Vec<RemoteEntry>)> {
    let mut blocks: Vec<(String, String)> = Vec::new();
    for line in output.lines() {
        if let Some(rest) = line.strip_prefix("sftp> ") {
            let command = rest.trim_start_matches('-');
            let dir = command.strip_prefix("ls -la ").unwrap_or(command).trim();
            let dir = dir.trim_matches('"').replace("\\\"", "\"").replace("\\\\", "\\");
            blocks.push((dir, String::new()));
        } else if let Some((_, text)) = blocks.last_mut() {
            text.push_str(line);
            text.push('\n');
        }
    }
    blocks.into_iter().map(|(dir, text)| (dir, parse_ls(&text))).collect()
}

/// Lists several folders over one connection.
fn ls_many(server: &Server, password: &str, dirs: &[String]) -> Result<Vec<(String, Vec<RemoteEntry>)>, String> {
    if dirs.is_empty() {
        return Ok(Vec::new());
    }
    let mut batch = String::new();
    for dir in dirs {
        // `-` keeps one unreadable folder from ending the batch.
        batch.push_str(&format!("-ls -la {}\n", quote(dir)));
    }
    run(server, password, &batch).map(|out| parse_ls_batch(&out))
}

/// Every folder and file under `remote_dir`, walking one level per connection.
pub fn tree_under(server: &Server, password: &str, remote_dir: &str) -> Result<(Vec<String>, Vec<String>), String> {
    let mut dirs = Vec::new();
    let mut files = Vec::new();
    let mut frontier = vec![remote_dir.to_string()];
    while !frontier.is_empty() {
        let mut next = Vec::new();
        for (dir, entries) in ls_many(server, password, &frontier)? {
            for entry in entries {
                let path = remote_join(&dir, &entry.name);
                if entry.is_dir {
                    if SKIP_DIRS.contains(&entry.name.as_str()) {
                        continue;
                    }
                    dirs.push(path.clone());
                    next.push(path);
                } else {
                    files.push(path);
                }
            }
        }
        frontier = next;
    }
    Ok((dirs, files))
}

/// Downloads a remote folder into `local_dir`, keeping its shape. Returns the files written.
pub fn download_tree(server: &Server, password: &str, remote_dir: &str, local_dir: &Path) -> Result<Vec<PathBuf>, String> {
    let (_, files) = tree_under(server, password, remote_dir)?;
    let base = remote_dir.trim_end_matches('/');
    let mut batch = String::new();
    let mut locals = Vec::new();
    for remote in &files {
        let rel = remote.strip_prefix(base).unwrap_or(remote).trim_start_matches('/');
        let local = local_dir.join(rel.replace('/', std::path::MAIN_SEPARATOR_STR));
        if let Some(parent) = local.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        batch.push_str(&download_batch(remote, &local));
        locals.push(local);
    }
    if !locals.is_empty() {
        run(server, password, &batch)?;
    }
    Ok(locals)
}

/// Deletes a file, or a folder with everything under it (files first, then the folders).
pub fn delete_remote_path(server: &Server, password: &str, path: &str, is_dir: bool) -> Result<usize, String> {
    if !is_dir {
        run(server, password, &format!("rm {}\n", quote(path)))?;
        return Ok(1);
    }
    let (dirs, files) = tree_under(server, password, path)?;
    let mut batch = String::new();
    for file in &files {
        batch.push_str(&format!("-rm {}\n", quote(file)));
    }
    for dir in dirs.iter().rev() {
        batch.push_str(&format!("-rmdir {}\n", quote(dir)));
    }
    batch.push_str(&format!("rmdir {}\n", quote(path)));
    run(server, password, &batch)?;
    Ok(files.len() + dirs.len() + 1)
}

pub fn rename_remote(server: &Server, password: &str, from: &str, to: &str) -> Result<(), String> {
    run(server, password, &format!("rename {} {}\n", quote(from), quote(to))).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batches_and_listing() {
        let server = Server { remote_root: "/var/www/app/".into(), ..Default::default() };
        let root = Path::new("/p");
        assert_eq!(remote_path(&server, root, Path::new("/p/app/User.php")).as_deref(), Some("/var/www/app/app/User.php"));
        let batch = upload_batch(Path::new("/p/app/User.php"), "/var/www/app/app/User.php");
        assert_eq!(batch, "-mkdir \"/var\"\n-mkdir \"/var/www\"\n-mkdir \"/var/www/app\"\n-mkdir \"/var/www/app/app\"\nput \"/p/app/User.php\" \"/var/www/app/app/User.php\"\n");
        let ls = "sftp> ls -la /var/www\ndrwxr-xr-x    5 www  www   4096 Nov  1 10:00 .\ndrwxr-xr-x    5 www  www   4096 Nov  1 10:00 ..\n-rw-r--r--    1 www  www    120 Nov  1 10:00 /var/www/index.php\ndrwxr-xr-x    2 www  www   4096 Nov  1 10:00 /var/www/app\n";
        let entries = parse_ls(ls);
        assert_eq!(entries.iter().map(|e| (e.name.as_str(), e.is_dir)).collect::<Vec<_>>(), [("app", true), ("index.php", false)]);
    }

    #[test]
    fn folders_expand_to_their_files_and_dropped_files_keep_their_name() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "phpcraft-upload-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src/Models")).unwrap();
        std::fs::create_dir_all(dir.join(".git/objects")).unwrap();
        std::fs::write(dir.join("src/Models/User.php"), "<?php").unwrap();
        std::fs::write(dir.join("README.md"), "# app").unwrap();
        std::fs::write(dir.join(".git/HEAD"), "ref").unwrap();

        let files: Vec<String> = files_under(&dir)
            .iter()
            .map(|p| p.strip_prefix(&dir).unwrap().to_string_lossy().replace('\\', "/"))
            .collect();
        assert_eq!(files, ["README.md", "src/Models/User.php"]);
        // A single file is itself, not a folder to walk.
        assert_eq!(files_under(&dir.join("README.md")), vec![dir.join("README.md")]);

        let server = Server { remote_root: "/var/www/app".into(), ..Default::default() };
        let root = Path::new("/p");
        assert_eq!(remote_for(&server, root, Path::new("/p/app/User.php"), "/var/www/app"), "/var/www/app/app/User.php");
        assert_eq!(remote_for(&server, root, Path::new("/tmp/notes.txt"), "/var/www/app/notes"), "/var/www/app/notes/notes.txt");
        assert_eq!(remote_for(&server, root, Path::new("/tmp/notes.txt"), ""), "/var/www/app/notes.txt");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn listing_batches_split_by_the_folder_they_were_asked_for() {
        let output = "sftp> -ls -la \"/var/www/my app\"\ndrwxr-xr-x    2 www  www   4096 Nov  1 10:00 /var/www/my app/css\n-rw-r--r--    1 www  www    120 Nov  1 10:00 /var/www/my app/index.php\nsftp> -ls -la /var/www/my app/css\n-rw-r--r--    1 www  www     12 Nov  1 10:00 /var/www/my app/css/app.css\n";
        let blocks = parse_ls_batch(output);
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].0, "/var/www/my app");
        assert_eq!(blocks[0].1.iter().map(|e| (e.name.as_str(), e.is_dir)).collect::<Vec<_>>(), [("css", true), ("index.php", false)]);
        assert_eq!(blocks[1].0, "/var/www/my app/css");
        assert_eq!(blocks[1].1.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), ["app.css"]);
        assert_eq!(remote_join("/var/www", "app"), "/var/www/app");
        assert_eq!(remote_join("", "app"), "app");
    }

    #[test]
    fn rebuilt_folders_do_not_travel_with_a_drag() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!("phpcraft-skip-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("vendor/laravel")).unwrap();
        std::fs::create_dir_all(dir.join("node_modules")).unwrap();
        std::fs::create_dir_all(dir.join("app")).unwrap();
        std::fs::write(dir.join("vendor/laravel/autoload.php"), "").unwrap();
        std::fs::write(dir.join("node_modules/index.js"), "").unwrap();
        std::fs::write(dir.join("app/User.php"), "").unwrap();
        let files: Vec<String> =
            files_under(&dir).iter().map(|p| p.strip_prefix(&dir).unwrap().to_string_lossy().replace('\\', "/")).collect();
        assert_eq!(files, ["app/User.php"]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
