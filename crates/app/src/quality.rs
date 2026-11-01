//! The project's PHP tools from `vendor/bin`: the code formatter (Laravel Pint or PHP CS
//! Fixer) and the static analysers (PHPStan, Psalm, PHP_CodeSniffer).

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq)]
pub enum Formatter {
    Pint(PathBuf),
    PhpCsFixer(PathBuf),
}

impl Formatter {
    pub fn name(&self) -> &'static str {
        match self {
            Formatter::Pint(_) => "Laravel Pint",
            Formatter::PhpCsFixer(_) => "PHP CS Fixer",
        }
    }
}

/// A tool installed in the project (`vendor/bin/<name>`).
pub fn vendor_tool(root: &Path, name: &str) -> Option<PathBuf> {
    let path = root.join("vendor").join("bin").join(name);
    path.is_file().then_some(path)
}

/// The formatter the project uses: Pint when installed (or `pint.json` exists), otherwise
/// PHP CS Fixer.
pub fn formatter(root: &Path) -> Option<Formatter> {
    vendor_tool(root, "pint").map(Formatter::Pint).or_else(|| vendor_tool(root, "php-cs-fixer").map(Formatter::PhpCsFixer))
}

/// Runs `php <script> args…` in the project (the vendor/bin scripts are PHP files, which also
/// works on Windows where they aren't executable).
pub fn run_php_tool(root: &Path, script: &Path, args: &[&str]) -> Result<String, String> {
    // Older tools on newer PHP print pages of deprecation notices; keep real errors only.
    let output = process::command(laravel::php_binary())
        .args(["-d", "error_reporting=8191"])
        .arg(script)
        .args(args)
        .current_dir(root)
        .output()
        .map_err(|e| format!("cannot run php: {e}"))?;
    let text = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    if output.status.success() { Ok(text) } else { Err(text) }
}

/// Formats one file in place with the project's formatter.
pub fn format_file(root: &Path, formatter: &Formatter, file: &Path) -> Result<(), String> {
    let file = file.to_string_lossy();
    match formatter {
        Formatter::Pint(script) => run_php_tool(root, script, &[&file, "--quiet"]).map(|_| ()),
        Formatter::PhpCsFixer(script) => run_php_tool(root, script, &["fix", &file, "--quiet"]).map(|_| ()),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Analyser {
    PhpStan,
    Psalm,
    PhpCs,
}

impl Analyser {
    pub const ALL: [Analyser; 3] = [Analyser::PhpStan, Analyser::Psalm, Analyser::PhpCs];

    pub fn name(self) -> &'static str {
        match self {
            Analyser::PhpStan => "PHPStan",
            Analyser::Psalm => "Psalm",
            Analyser::PhpCs => "PHP_CodeSniffer",
        }
    }

    fn script(self) -> &'static str {
        match self {
            Analyser::PhpStan => "phpstan",
            Analyser::Psalm => "psalm",
            Analyser::PhpCs => "phpcs",
        }
    }

    pub fn enabled(self, settings: &crate::config::Settings) -> bool {
        match self {
            Analyser::PhpStan => settings.phpstan,
            Analyser::Psalm => settings.psalm,
            Analyser::PhpCs => settings.phpcs,
        }
    }

    fn args(self, file: &str) -> Vec<String> {
        let args: &[&str] = match self {
            Analyser::PhpStan => &["analyse", "--error-format=json", "--no-progress", "--no-interaction"],
            Analyser::Psalm => &["--output-format=json", "--no-progress", "--no-cache"],
            Analyser::PhpCs => &["--report=json", "-q"],
        };
        args.iter().map(|a| a.to_string()).chain(std::iter::once(file.to_string())).collect()
    }
}

/// A problem reported by an analyser: 1-based line, message, and whether it is minor.
#[derive(Debug, Clone, PartialEq)]
pub struct Finding {
    pub line: usize,
    pub message: String,
    pub minor: bool,
}

/// Every `{ "message": …, "line" | "line_from": … }` object in a JSON report, whatever its
/// layout (PHPStan, wrappers around it, Psalm and PHPCS all nest them differently).
fn collect_findings(value: &serde_json::Value, out: &mut Vec<Finding>) {
    match value {
        serde_json::Value::Object(map) => {
            let line = map.get("line").or_else(|| map.get("line_from")).and_then(|l| l.as_u64());
            if let (Some(message), Some(line)) = (map.get("message").and_then(|m| m.as_str()), line) {
                let level = map.get("severity").or_else(|| map.get("type")).and_then(|s| s.as_str()).unwrap_or("").to_lowercase();
                out.push(Finding { line: line as usize, message: message.to_string(), minor: level == "info" || level == "warning" });
            }
            for v in map.values() {
                collect_findings(v, out);
            }
        }
        serde_json::Value::Array(items) => items.iter().for_each(|v| collect_findings(v, out)),
        _ => {}
    }
}

/// Parses a report: one JSON document, or several on separate lines.
pub fn parse_report(text: &str) -> Vec<Finding> {
    let mut out = Vec::new();
    match serde_json::from_str::<serde_json::Value>(text.trim()) {
        Ok(value) => collect_findings(&value, &mut out),
        Err(_) => {
            for line in text.lines().filter(|l| l.trim_start().starts_with(['{', '['])) {
                if let Ok(value) = serde_json::from_str::<serde_json::Value>(line) {
                    collect_findings(&value, &mut out);
                }
            }
        }
    }
    out.dedup();
    out
}

/// Runs the enabled, installed analysers on one file.
pub fn analyse(root: &Path, file: &Path, settings: &crate::config::Settings) -> Vec<(Analyser, Finding)> {
    let file = file.to_string_lossy().to_string();
    let mut found = Vec::new();
    for analyser in Analyser::ALL {
        if !analyser.enabled(settings) {
            continue;
        }
        let Some(script) = vendor_tool(root, analyser.script()) else { continue };
        let args = analyser.args(&file);
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        // Analysers exit non-zero when they find problems: read the report either way.
        let report = match run_php_tool(root, &script, &args) {
            Ok(text) | Err(text) => text,
        };
        found.extend(parse_report(&report).into_iter().map(|f| (analyser, f)));
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_reports() {
        let phpstan = r#"{"totals":{"errors":0,"file_errors":1},"files":{"/p/a.php":{"errors":1,"messages":[{"message":"Undefined variable: $x","line":4,"ignorable":true}]}},"errors":[]}"#;
        assert_eq!(parse_report(phpstan), [Finding { line: 4, message: "Undefined variable: $x".into(), minor: false }]);
        let wrapped = "{\"tool\":\"phpstan\",\"raw\":[\"Note\"]}\n{\"error_details\":{\"/p/a.php\":[{\"line\":15,\"message\":\"Bad type\"}]}}";
        assert_eq!(parse_report(wrapped)[0].line, 15);
        let psalm = r#"[{"severity":"info","line_from":7,"message":"Unused","file_path":"/p/a.php"}]"#;
        assert_eq!(parse_report(psalm), [Finding { line: 7, message: "Unused".into(), minor: true }]);
        let phpcs = r#"{"files":{"/p/a.php":{"messages":[{"message":"Missing doc","line":2,"type":"ERROR"}]}}}"#;
        assert_eq!(parse_report(phpcs)[0].message, "Missing doc");
    }
}
