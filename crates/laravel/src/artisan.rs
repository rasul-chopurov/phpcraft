//! Artisan integration: list commands and run them.

use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtisanCommand {
    pub name: String,
    pub description: String,
}

/// Commands from `php artisan list --format=json`. Blocking; call off the UI thread.
pub fn list_commands(root: &Path) -> Result<Vec<ArtisanCommand>, String> {
    let output = Command::new("php")
        .args(["artisan", "list", "--format=json"])
        .current_dir(root)
        .output()
        .map_err(|e| format!("failed to run php: {e}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned());
    }
    Ok(parse_list(&String::from_utf8_lossy(&output.stdout)))
}

/// Extracts `name`/`description` pairs from the `commands` array of the JSON listing
/// without a JSON dependency: each command object starts with `"name":` then `"description":`.
fn parse_list(json: &str) -> Vec<ArtisanCommand> {
    let Some(commands) = json.find("\"commands\"").map(|i| &json[i..]) else { return Vec::new() };
    let mut out = Vec::new();
    let mut rest = commands;
    while let Some(at) = rest.find("{\"name\":") {
        rest = &rest[at + 8..];
        let Some((name, after)) = json_string(rest) else { break };
        let description = after
            .find("\"description\":")
            .and_then(|i| json_string(&after[i + 14..]))
            .map(|(d, _)| d)
            .unwrap_or_default();
        if !name.starts_with('_') {
            out.push(ArtisanCommand { name, description });
        }
        rest = after;
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Parses a JSON string literal at the start of `s` (after optional whitespace).
fn json_string(s: &str) -> Option<(String, &str)> {
    let s = s.trim_start().strip_prefix('"')?;
    let mut out = String::new();
    let mut chars = s.char_indices();
    while let Some((i, c)) = chars.next() {
        match c {
            '"' => return Some((out, &s[i + 1..])),
            '\\' => match chars.next()?.1 {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                'u' => {
                    let hex: String = (0..4).filter_map(|_| chars.next().map(|(_, c)| c)).collect();
                    out.extend(u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32));
                }
                other => out.push(other),
            },
            c => out.push(c),
        }
    }
    None
}

/// Splits a command line into arguments, honouring single and double quotes.
pub fn split_args(line: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut has_token = false;
    for c in line.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => current.push(c),
            (None, '\'' | '"') => {
                quote = Some(c);
                has_token = true;
            }
            (None, c) if c.is_whitespace() => {
                if has_token || !current.is_empty() {
                    args.push(std::mem::take(&mut current));
                    has_token = false;
                }
            }
            (None, c) => current.push(c),
        }
    }
    if has_token || !current.is_empty() {
        args.push(current);
    }
    args
}

/// Runs `php artisan <line>` and returns the combined output and success flag. Blocking.
pub fn run(root: &Path, line: &str) -> (String, bool) {
    let mut args = split_args(line);
    if args.first().is_some_and(|a| a == "php") {
        args.remove(0);
    }
    if args.first().is_some_and(|a| a == "artisan") {
        args.remove(0);
    }
    let output = Command::new("php")
        .arg("artisan")
        .args(&args)
        .arg("--no-interaction")
        .arg("--ansi")
        .current_dir(root)
        .output();
    match output {
        Ok(out) => {
            let mut text = strip_ansi(&String::from_utf8_lossy(&out.stdout));
            let stderr = strip_ansi(&String::from_utf8_lossy(&out.stderr));
            if !stderr.trim().is_empty() {
                text.push_str(&stderr);
            }
            (text, out.status.success())
        }
        Err(err) => (format!("failed to run php: {err}"), false),
    }
}

fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            for c in chars.by_ref() {
                if c.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_listing() {
        let json = r#"{"application":{"name":"Laravel"},"commands":[{"name":"_complete","description":"x"},{"name":"make:model","description":"Create a new \"Eloquent\" model class","usage":[]},{"name":"about","description":"Display basic information"}]}"#;
        let commands = parse_list(json);
        let names: Vec<_> = commands.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["about", "make:model"]);
        assert_eq!(commands[1].description, "Create a new \"Eloquent\" model class");
    }

    #[test]
    fn splits_quoted_args() {
        assert_eq!(split_args(r#"make:model Post -m --name="a b" ''"#), ["make:model", "Post", "-m", "--name=a b", ""]);
        assert_eq!(strip_ansi("\u{1b}[32mOK\u{1b}[39m"), "OK");
    }
}
