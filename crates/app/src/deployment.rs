// The Deployment window is wired up in a later step (see the plan).
#![allow(dead_code)]

//! SFTP deployment (PhpStorm's Deployment): servers saved in `.phpcraft/deployment.json`,
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
    let path = file(root);
    std::fs::create_dir_all(path.parent().unwrap())?;
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
    map.insert(format!("{}|{id}", root.display()), value.to_string());
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

/// Runs an sftp batch against `server`.
pub fn run(server: &Server, password: &str, batch: &str) -> Result<String, String> {
    use std::io::Write;
    let mut command = std::process::Command::new("sftp");
    command.args(["-b", "-", "-P", &server.port.to_string(), "-o", "StrictHostKeyChecking=accept-new", "-o", "ConnectTimeout=10"]);
    if !server.key.trim().is_empty() {
        command.args(["-i", server.key.trim()]);
    }
    // Password auth: OpenSSH asks SSH_ASKPASS, a throwaway script that prints it.
    let askpass = if server.use_password && !password.is_empty() {
        let script = std::env::temp_dir().join(format!("phpcraft-askpass-{}-{}", std::process::id(), server.id));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let escaped = password.replace('\'', "'\\''");
            std::fs::write(&script, format!("#!/bin/sh\nprintf '%s\\n' '{escaped}'\n")).map_err(|e| e.to_string())?;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).map_err(|e| e.to_string())?;
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
    command.arg(format!("{}@{}", server.user, server.host));
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

pub fn list(server: &Server, password: &str, dir: &str) -> Result<Vec<RemoteEntry>, String> {
    run(server, password, &format!("ls -la {}\n", quote(dir))).map(|out| parse_ls(&out))
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
}
