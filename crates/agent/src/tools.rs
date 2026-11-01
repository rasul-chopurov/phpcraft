//! Project tools the agent can call. Paths are relative to the project root and may not
//! leave it.

use std::path::{Component, Path, PathBuf};
use std::sync::mpsc;

use serde_json::{Value, json};

use crate::AgentEvent;

const MAX_OUTPUT: usize = 60_000;

pub fn tool_definitions() -> Value {
    let tool = |name: &str, description: &str, properties: Value, required: &[&str]| {
        json!({
            "name": name,
            "description": description,
            "input_schema": {"type": "object", "properties": properties, "required": required, "additionalProperties": false},
            "eager_input_streaming": true,
        })
    };
    let path = json!({"type": "string", "description": "Path relative to the project root"});
    json!([
        tool("read_file", "Read a text file of the project. Returns numbered lines.", json!({"path": path}), &["path"]),
        tool(
            "list_files",
            "List project files (respecting .gitignore, without vendor/), optionally only those whose path contains `filter`.",
            json!({"filter": {"type": "string", "description": "Substring of the path, e.g. app/Models or .blade.php"}}),
            &[],
        ),
        tool(
            "search",
            "Search the project's files for text (or a regex). Returns matching lines with paths and line numbers.",
            json!({
                "query": {"type": "string"},
                "regex": {"type": "boolean", "description": "Treat query as a regular expression"},
                "mask": {"type": "string", "description": "Comma-separated globs like *.php, *.blade.php"}
            }),
            &["query"],
        ),
        tool(
            "edit_file",
            "Replace an exact, unique snippet of a file with new text. Read the file first; include enough context in old_text to be unique.",
            json!({"path": path, "old_text": {"type": "string"}, "new_text": {"type": "string"}}),
            &["path", "old_text", "new_text"],
        ),
        tool("write_file", "Create a file or overwrite it completely.", json!({"path": path, "content": {"type": "string"}}), &["path", "content"]),
        tool(
            "run_command",
            "Run a shell command in the project root (e.g. php artisan make:model, composer require, phpunit). Returns its output. Not for long-running servers.",
            json!({"command": {"type": "string"}}),
            &["command"],
        ),
    ])
}

/// Tools that change the project or run programs need the user's approval.
pub fn needs_approval(name: &str) -> bool {
    matches!(name, "edit_file" | "write_file" | "run_command")
}

/// Resolves a project-relative path, refusing anything outside the root.
fn resolve(root: &Path, path: &str) -> Result<PathBuf, String> {
    let relative = Path::new(path.trim_start_matches("./"));
    if relative.is_absolute() || relative.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(format!("{path}: only paths inside the project are allowed"));
    }
    Ok(root.join(relative))
}

fn truncate(mut text: String) -> String {
    if text.len() > MAX_OUTPUT {
        let mut cut = MAX_OUTPUT;
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        text.truncate(cut);
        text.push_str("\n… (output truncated)");
    }
    text
}

fn str_arg<'a>(input: &'a Value, key: &str) -> Result<&'a str, String> {
    input[key].as_str().ok_or_else(|| format!("missing string field `{key}`"))
}

/// Runs a tool; returns `(output, is_error)`.
pub fn run(root: &Path, name: &str, input: &Value, events: &mpsc::Sender<AgentEvent>) -> (String, bool) {
    match run_inner(root, name, input, events) {
        Ok(output) => (truncate(output), false),
        Err(err) => (err, true),
    }
}

fn run_inner(root: &Path, name: &str, input: &Value, events: &mpsc::Sender<AgentEvent>) -> Result<String, String> {
    match name {
        "read_file" => {
            let path = resolve(root, str_arg(input, "path")?)?;
            let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            Ok(text.lines().enumerate().map(|(i, l)| format!("{:>5}  {l}\n", i + 1)).collect())
        }
        "list_files" => {
            let filter = input["filter"].as_str().unwrap_or("");
            let files: Vec<String> = search::list_files(root, false)
                .into_iter()
                .map(|p| p.strip_prefix(root).unwrap_or(&p).to_string_lossy().replace('\\', "/"))
                .filter(|p| filter.is_empty() || p.contains(filter))
                .take(2000)
                .collect();
            Ok(if files.is_empty() { "No files.".into() } else { files.join("\n") })
        }
        "search" => {
            let mut query = search::TextQuery::new(str_arg(input, "query")?);
            query.regex = input["regex"].as_bool().unwrap_or(false);
            let options = search::FileSearch { mask: input["mask"].as_str().unwrap_or("").into(), include_vendor: false, max_results: 200 };
            let matches = search::search_files(root, &query, &options)?;
            if matches.is_empty() {
                return Ok("No matches.".into());
            }
            Ok(matches
                .iter()
                .map(|m| format!("{}:{}: {}", m.path.strip_prefix(root).unwrap_or(&m.path).display(), m.line + 1, m.text.trim()))
                .collect::<Vec<_>>()
                .join("\n"))
        }
        "edit_file" => {
            let path = resolve(root, str_arg(input, "path")?)?;
            let old = str_arg(input, "old_text")?;
            let new = str_arg(input, "new_text")?;
            let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            match text.matches(old).count() {
                0 => Err("old_text was not found in the file; read it again and copy the snippet exactly.".into()),
                1 => {
                    std::fs::write(&path, text.replacen(old, new, 1)).map_err(|e| e.to_string())?;
                    let _ = events.send(AgentEvent::FilesChanged(vec![path]));
                    Ok("Edited.".into())
                }
                n => Err(format!("old_text occurs {n} times; include more context to make it unique.")),
            }
        }
        "write_file" => {
            let path = resolve(root, str_arg(input, "path")?)?;
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            std::fs::write(&path, str_arg(input, "content")?).map_err(|e| e.to_string())?;
            let _ = events.send(AgentEvent::FilesChanged(vec![path]));
            Ok("Written.".into())
        }
        "run_command" => {
            let command = str_arg(input, "command")?;
            let output = if cfg!(windows) {
                process::command("cmd").args(["/C", command]).current_dir(root).output()
            } else {
                process::command("/bin/sh").args(["-c", command]).current_dir(root).output()
            }
            .map_err(|e| e.to_string())?;
            let _ = events.send(AgentEvent::FilesChanged(Vec::new()));
            let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
            text.push_str(&String::from_utf8_lossy(&output.stderr));
            Ok(format!("exit code {}\n{text}", output.status.code().unwrap_or(-1)))
        }
        other => Err(format!("unknown tool {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edits_inside_the_project_only() {
        let root = std::env::temp_dir().join(format!("phpcraft-agent-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("app")).unwrap();
        std::fs::write(root.join("app/a.php"), "<?php\necho 1;\necho 1;\n").unwrap();
        let (tx, _rx) = mpsc::channel();
        let (out, err) = run(&root, "edit_file", &json!({"path": "app/a.php", "old_text": "echo 1;", "new_text": "echo 2;"}), &tx);
        assert!(err && out.contains("2 times"), "{out}");
        let (_, err) = run(&root, "edit_file", &json!({"path": "app/a.php", "old_text": "<?php\necho 1;", "new_text": "<?php\necho 2;"}), &tx);
        assert!(!err);
        assert_eq!(std::fs::read_to_string(root.join("app/a.php")).unwrap(), "<?php\necho 2;\necho 1;\n");
        let (out, err) = run(&root, "read_file", &json!({"path": "../etc/passwd"}), &tx);
        assert!(err && out.contains("inside the project"));
        let (out, err) = run(&root, "write_file", &json!({"path": "new/b.txt", "content": "hi"}), &tx);
        assert!(!err, "{out}");
        let (out, _) = run(&root, "read_file", &json!({"path": "new/b.txt"}), &tx);
        assert!(out.contains("1  hi"));
        std::fs::remove_dir_all(root).unwrap();
    }
}
