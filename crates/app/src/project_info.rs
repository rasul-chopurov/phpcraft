//! Facts for the status bar: Laravel and PHP versions and the EditorConfig indent.

use std::path::Path;
use std::sync::OnceLock;

/// [`read_laravel_version`], remembered per project.
pub fn laravel_version(root: &Path) -> Option<String> {
    use std::collections::HashMap;
    use std::sync::Mutex;
    static CACHE: Mutex<Option<HashMap<std::path::PathBuf, Option<String>>>> = Mutex::new(None);
    let mut cache = CACHE.lock().unwrap();
    cache.get_or_insert_with(HashMap::new).entry(root.to_path_buf()).or_insert_with(|| read_laravel_version(root)).clone()
}

/// `11.31.0` from composer.lock, or the installed framework's `VERSION` constant.
fn read_laravel_version(root: &Path) -> Option<String> {
    if let Ok(text) = std::fs::read_to_string(root.join("composer.lock"))
        && let Ok(lock) = serde_json::from_str::<serde_json::Value>(&text)
        && let Some(version) = lock["packages"]
            .as_array()
            .and_then(|p| p.iter().find(|p| p["name"] == "laravel/framework"))
            .and_then(|p| p["version"].as_str())
    {
        return Some(version.trim_start_matches('v').to_string());
    }
    let app = std::fs::read_to_string(root.join("vendor/laravel/framework/src/Illuminate/Foundation/Application.php")).ok()?;
    let at = app.find("const VERSION = '")? + "const VERSION = '".len();
    Some(app[at..].split('\'').next()?.to_string())
}

/// The `php` binary's version (asked once).
pub fn php_version() -> Option<String> {
    static VERSION: OnceLock<Option<String>> = OnceLock::new();
    VERSION
        .get_or_init(|| {
            let output = process::command("php").args(["-r", "echo PHP_VERSION;"]).output().ok()?;
            output.status.success().then(|| String::from_utf8_lossy(&output.stdout).trim().to_string()).filter(|v| !v.is_empty())
        })
        .clone()
}

/// `4 spaces` / `Tab` for `file` from the project's .editorconfig (last matching section wins).
pub fn editorconfig_indent(root: &Path, file: &Path) -> Option<String> {
    let text = std::fs::read_to_string(root.join(".editorconfig")).ok()?;
    let name = file.file_name()?.to_string_lossy().to_string();
    let ext = file.extension().map(|e| e.to_string_lossy().to_string()).unwrap_or_default();
    let mut matches = false;
    let (mut style, mut size) = (None::<String>, None::<String>);
    for line in text.lines().map(str::trim) {
        if let Some(section) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            matches = section_matches(section, &name, &ext);
            continue;
        }
        if !matches {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            match key.trim() {
                "indent_style" => style = Some(value.trim().to_string()),
                "indent_size" => size = Some(value.trim().to_string()),
                _ => {}
            }
        }
    }
    match (style.as_deref(), size) {
        (Some("tab"), _) => Some("Tab".into()),
        (_, Some(size)) => Some(format!("{size} spaces")),
        (Some("space"), None) => Some("Spaces".into()),
        _ => None,
    }
}

fn section_matches(section: &str, name: &str, ext: &str) -> bool {
    if section == "*" {
        return true;
    }
    if let Some(list) = section.strip_prefix("*.{").and_then(|s| s.strip_suffix('}')) {
        return list.split(',').any(|e| e.trim() == ext);
    }
    if let Some(e) = section.strip_prefix("*.") {
        return e == ext;
    }
    section == name
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editorconfig_sections() {
        let root = std::env::temp_dir().join(format!("phpcraft-ec-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join(".editorconfig"), "root = true\n[*]\nindent_style = space\nindent_size = 4\n[*.{yml,yaml}]\nindent_size = 2\n[Makefile]\nindent_style = tab\n").unwrap();
        assert_eq!(editorconfig_indent(&root, &root.join("a.php")).as_deref(), Some("4 spaces"));
        assert_eq!(editorconfig_indent(&root, &root.join("b.yaml")).as_deref(), Some("2 spaces"));
        assert_eq!(editorconfig_indent(&root, &root.join("Makefile")).as_deref(), Some("Tab"));
        std::fs::write(root.join("composer.lock"), r#"{"packages":[{"name":"laravel/framework","version":"v11.31.0"}]}"#).unwrap();
        assert_eq!(read_laravel_version(&root).as_deref(), Some("11.31.0"));
        std::fs::remove_dir_all(root).unwrap();
    }
}
