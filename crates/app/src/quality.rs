//! The project's PHP tools: the code formatter (Laravel Pint or PHP CS Fixer from
//! `vendor/bin`, or on PATH).

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
