//! Artisan integration: list commands and run them.

use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtisanArgument {
    pub name: String,
    pub required: bool,
    pub is_array: bool,
    pub description: String,
    pub default: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtisanOption {
    /// Without the leading `--`.
    pub name: String,
    /// e.g. `-m`, empty when none.
    pub shortcut: String,
    /// `false` for plain flags like `--force`.
    pub accepts_value: bool,
    pub description: String,
    pub default: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtisanCommand {
    pub name: String,
    pub description: String,
    pub arguments: Vec<ArtisanArgument>,
    pub options: Vec<ArtisanOption>,
}

/// Options every Symfony console command has; not worth showing in the run dialog.
const GLOBAL_OPTIONS: &[&str] = &[
    "help", "silent", "quiet", "verbose", "version", "ansi", "no-ansi", "no-interaction", "env",
];

/// Commands from `php artisan list --format=json`. Blocking; call off the UI thread.
pub fn list_commands(root: &Path) -> Result<Vec<ArtisanCommand>, String> {
    let output = Command::new(crate::php_binary())
        .args(["artisan", "list", "--format=json"])
        .current_dir(root)
        .output()
        .map_err(|e| format!("failed to run php: {e}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned());
    }
    parse_list(&String::from_utf8_lossy(&output.stdout))
}

fn scalar(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::Null | serde_json::Value::Bool(false) => None,
        serde_json::Value::String(s) if s.is_empty() => None,
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Array(a) if a.is_empty() => None,
        other => Some(other.to_string()),
    }
}

fn parse_list(json: &str) -> Result<Vec<ArtisanCommand>, String> {
    let root: serde_json::Value = serde_json::from_str(json).map_err(|e| format!("unexpected artisan output: {e}"))?;
    let commands = root["commands"].as_array().cloned().unwrap_or_default();
    let text = |v: &serde_json::Value| v.as_str().unwrap_or_default().to_string();
    let mut out: Vec<ArtisanCommand> = commands
        .iter()
        .filter(|c| !c["hidden"].as_bool().unwrap_or(false))
        .filter(|c| !text(&c["name"]).starts_with('_'))
        .map(|c| {
            let definition = &c["definition"];
            let arguments = definition["arguments"]
                .as_object()
                .map(|args| {
                    args.values()
                        .map(|a| ArtisanArgument {
                            name: text(&a["name"]),
                            required: a["is_required"].as_bool().unwrap_or(false),
                            is_array: a["is_array"].as_bool().unwrap_or(false),
                            description: text(&a["description"]),
                            default: scalar(&a["default"]),
                        })
                        .collect()
                })
                .unwrap_or_default();
            let options = definition["options"]
                .as_object()
                .map(|opts| {
                    opts.iter()
                        .filter(|(key, _)| !GLOBAL_OPTIONS.contains(&key.as_str()))
                        .map(|(key, o)| ArtisanOption {
                            name: key.clone(),
                            shortcut: text(&o["shortcut"]),
                            accepts_value: o["accept_value"].as_bool().unwrap_or(false),
                            description: text(&o["description"]),
                            default: scalar(&o["default"]),
                        })
                        .collect()
                })
                .unwrap_or_default();
            ArtisanCommand {
                name: text(&c["name"]),
                description: text(&c["description"]),
                arguments,
                options,
            }
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// Quotes an argument for display/splitting when it contains spaces or quotes.
pub fn quote(arg: &str) -> String {
    if arg.is_empty() || arg.contains(|c: char| c.is_whitespace() || c == '"' || c == '\'') {
        format!("\"{}\"", arg.replace('"', "\\\""))
    } else {
        arg.to_string()
    }
}

/// Builds `name arg1 arg2 --flag --opt=value` from filled-in values.
pub fn command_line(command: &ArtisanCommand, arguments: &[String], options: &[(String, Option<String>)]) -> String {
    let mut parts = vec![command.name.clone()];
    for (argument, value) in command.arguments.iter().zip(arguments) {
        let value = value.trim();
        if value.is_empty() {
            continue;
        }
        if argument.is_array {
            parts.extend(value.split_whitespace().map(quote));
        } else {
            parts.push(quote(value));
        }
    }
    for (name, value) in options {
        match value {
            Some(v) if !v.trim().is_empty() => parts.push(format!("--{name}={}", quote(v.trim()))),
            Some(_) => {}
            None => parts.push(format!("--{name}")),
        }
    }
    parts.join(" ")
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
    let output = Command::new(crate::php_binary())
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
        let json = r#"{"commands":[
            {"name":"_complete","description":"x","hidden":true,"definition":{"arguments":[],"options":[]}},
            {"name":"make:model","description":"Create a new \"Eloquent\" model class","hidden":false,"definition":{
                "arguments":{"name":{"name":"name","is_required":true,"is_array":false,"description":"The name of the model","default":null}},
                "options":{"migration":{"name":"--migration","shortcut":"-m","accept_value":false,"is_value_required":false,"description":"Create a migration","default":false},
                           "path":{"name":"--path","shortcut":"","accept_value":true,"is_value_required":true,"description":"Path","default":null},
                           "help":{"name":"--help","shortcut":"-h","accept_value":false,"description":"","default":false}}}},
            {"name":"about","description":"Display basic information","hidden":false,"definition":{"arguments":[],"options":{}}}
        ]}"#;
        let commands = parse_list(json).unwrap();
        let names: Vec<_> = commands.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["about", "make:model"]);
        let model = &commands[1];
        assert_eq!(model.description, "Create a new \"Eloquent\" model class");
        assert!(model.arguments[0].required);
        let options: Vec<_> = model.options.iter().map(|o| (o.name.as_str(), o.accepts_value)).collect();
        assert_eq!(options, [("migration", false), ("path", true)]);
        let line = command_line(model, &["Blog Post".into()], &[("migration".into(), None), ("path".into(), Some("app/X".into()))]);
        assert_eq!(line, "make:model \"Blog Post\" --migration --path=app/X");
        assert_eq!(split_args(&line), ["make:model", "Blog Post", "--migration", "--path=app/X"]);
    }

    #[test]
    fn splits_quoted_args() {
        assert_eq!(split_args(r#"make:model Post -m --name="a b" ''"#), ["make:model", "Post", "-m", "--name=a b", ""]);
        assert_eq!(strip_ansi("\u{1b}[32mOK\u{1b}[39m"), "OK");
    }
}
