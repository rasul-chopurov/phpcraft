//! Settings Sync through a secret GitHub Gist (like VS Code / JetBrains settings sync).
//! Uses the GitHub CLI (`gh`), so no token is stored by PhpCraft: run `gh auth login` once.
//!
//! Synced: `settings.json`, `keymap.json` and user themes (`themes/*.json`, stored in the gist
//! as `theme--<file>.json` because gists have no folders).

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::Stdio;

use serde::{Deserialize, Serialize};

use crate::config;

const DESCRIPTION: &str = "PhpCraft Settings Sync";
const THEME_PREFIX: &str = "theme--";

#[derive(Debug, Default, Serialize, Deserialize)]
struct SyncState {
    gist_id: Option<String>,
    last_sync: Option<String>,
}

fn state_path() -> PathBuf {
    config::dir().join("sync.json")
}

fn load_state() -> SyncState {
    fs::read_to_string(state_path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn save_state(state: &SyncState) {
    let _ = fs::create_dir_all(config::dir());
    let _ = fs::write(state_path(), serde_json::to_string_pretty(state).unwrap_or_default());
}

/// `gh api` with an optional JSON body on stdin.
fn gh_api(args: &[&str], body: Option<&serde_json::Value>) -> Result<serde_json::Value, String> {
    let mut command = process::command("gh");
    command.arg("api").args(args).stdout(Stdio::piped()).stderr(Stdio::piped());
    if body.is_some() {
        command.args(["--input", "-"]).stdin(Stdio::piped());
    }
    let mut child = command
        .spawn()
        .map_err(|_| "GitHub CLI (gh) not found. Install it from https://cli.github.com and run `gh auth login`.".to_string())?;
    if let Some(body) = body
        && let Some(mut stdin) = child.stdin.take()
    {
        stdin.write_all(body.to_string().as_bytes()).map_err(|e| e.to_string())?;
    }
    let output = child.wait_with_output().map_err(|e| e.to_string())?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if err.contains("auth login") || err.contains("401") {
            format!("Not signed in to GitHub CLI. Run `gh auth login`.\n{err}")
        } else {
            err
        });
    }
    serde_json::from_slice(&output.stdout).map_err(|e| format!("unexpected gh output: {e}"))
}

/// Local files to sync: gist file name -> contents.
fn local_files() -> BTreeMap<String, String> {
    let dir = config::dir();
    let mut files = BTreeMap::new();
    for name in ["settings.json", "keymap.json"] {
        if let Ok(text) = fs::read_to_string(dir.join(name)) {
            files.insert(name.to_string(), text);
        }
    }
    if let Ok(entries) = fs::read_dir(dir.join("themes")) {
        for entry in entries.flatten().filter(|e| e.path().extension().is_some_and(|x| x == "json")) {
            if let Ok(text) = fs::read_to_string(entry.path()) {
                files.insert(format!("{THEME_PREFIX}{}", entry.file_name().to_string_lossy()), text);
            }
        }
    }
    files
}

/// The sync gist: the remembered one, else one with our description in the user's gists.
fn find_gist(state: &SyncState) -> Result<Option<String>, String> {
    if let Some(id) = &state.gist_id {
        return Ok(Some(id.clone()));
    }
    let gists = gh_api(&["gists", "--paginate"], None)?;
    Ok(gists.as_array().and_then(|list| {
        list.iter()
            .find(|g| g["description"] == DESCRIPTION)
            .and_then(|g| g["id"].as_str().map(str::to_string))
    }))
}

fn now() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    format!("unix:{secs}")
}

/// Uploads local settings, creating the secret gist on first use. Blocking.
pub fn push() -> Result<String, String> {
    let mut state = load_state();
    let files = local_files();
    if files.is_empty() {
        return Err("Nothing to sync yet: no settings.json, keymap.json or themes.".into());
    }
    let payload: serde_json::Map<String, serde_json::Value> =
        files.iter().map(|(name, content)| (name.clone(), serde_json::json!({ "content": content }))).collect();
    let id = match find_gist(&state)? {
        Some(id) => {
            gh_api(&["-X", "PATCH", &format!("gists/{id}")], Some(&serde_json::json!({ "files": payload })))?;
            id
        }
        None => {
            let created = gh_api(
                &["-X", "POST", "gists"],
                Some(&serde_json::json!({ "description": DESCRIPTION, "public": false, "files": payload })),
            )?;
            created["id"].as_str().ok_or("GitHub did not return a gist id")?.to_string()
        }
    };
    state.gist_id = Some(id.clone());
    state.last_sync = Some(now());
    save_state(&state);
    Ok(format!("Pushed {} file(s) to secret gist {id}.", files.len()))
}

/// Downloads settings from the gist and writes them locally. Blocking; the caller re-applies
/// the theme and keymap afterwards.
pub fn pull() -> Result<String, String> {
    let mut state = load_state();
    let id = find_gist(&state)?.ok_or("No PhpCraft Settings Sync gist yet. Push from a machine first.")?;
    let gist = gh_api(&[&format!("gists/{id}")], None)?;
    let dir = config::dir();
    fs::create_dir_all(dir.join("themes")).map_err(|e| e.to_string())?;
    let mut count = 0;
    if let Some(files) = gist["files"].as_object() {
        for (name, file) in files {
            let Some(content) = file["content"].as_str() else { continue };
            let target = match name.strip_prefix(THEME_PREFIX) {
                Some(theme) => dir.join("themes").join(theme),
                None if name == "settings.json" || name == "keymap.json" => dir.join(name),
                None => continue,
            };
            fs::write(target, content).map_err(|e| e.to_string())?;
            count += 1;
        }
    }
    state.gist_id = Some(id.clone());
    state.last_sync = Some(now());
    save_state(&state);
    Ok(format!("Pulled {count} file(s) from gist {id}."))
}

/// Gist id when sync has been set up on this machine.
pub fn gist_id() -> Option<String> {
    load_state().gist_id
}
