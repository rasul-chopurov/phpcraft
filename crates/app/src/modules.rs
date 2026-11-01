//! nwidart/laravel-modules: the modules under `Modules/` and their enabled state, kept in
//! `modules_statuses.json` (the package's default file activator).

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleInfo {
    pub name: String,
    pub path: PathBuf,
    pub description: String,
    /// `None` when the statuses file doesn't mention the module.
    pub enabled: Option<bool>,
}

fn statuses_path(root: &Path) -> PathBuf {
    root.join("modules_statuses.json")
}

fn read_statuses(root: &Path) -> serde_json::Map<String, serde_json::Value> {
    std::fs::read_to_string(statuses_path(root))
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default()
}

pub fn has_modules(root: &Path) -> bool {
    root.join("Modules").is_dir()
}

pub fn list(root: &Path) -> Vec<ModuleInfo> {
    let statuses = read_statuses(root);
    let mut modules: Vec<ModuleInfo> = std::fs::read_dir(root.join("Modules"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.join("module.json").is_file())
        .map(|path| {
            let json: serde_json::Value =
                std::fs::read_to_string(path.join("module.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
            let folder = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let name = json["name"].as_str().map(str::to_string).unwrap_or(folder);
            ModuleInfo {
                enabled: statuses.get(&name).and_then(|v| v.as_bool()),
                description: json["description"].as_str().unwrap_or("").to_string(),
                name,
                path,
            }
        })
        .collect();
    modules.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    modules
}

/// Same effect as `php artisan module:enable|disable <name>` with the file activator.
pub fn set_enabled(root: &Path, name: &str, enabled: bool) -> std::io::Result<()> {
    let mut statuses = read_statuses(root);
    statuses.insert(name.to_string(), serde_json::Value::Bool(enabled));
    let text = serde_json::to_string_pretty(&serde_json::Value::Object(statuses)).unwrap_or_default();
    std::fs::write(statuses_path(root), text + "\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_and_toggles() {
        let root = std::env::temp_dir().join(format!("phpcraft-modules-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for name in ["Blog", "Archive"] {
            std::fs::create_dir_all(root.join("Modules").join(name)).unwrap();
            std::fs::write(root.join("Modules").join(name).join("module.json"), format!(r#"{{"name":"{name}","description":"The {name}"}}"#)).unwrap();
        }
        std::fs::write(root.join("modules_statuses.json"), r#"{"Blog": true}"#).unwrap();
        let list = list(&root);
        assert_eq!(list.iter().map(|m| (m.name.as_str(), m.enabled)).collect::<Vec<_>>(), [("Archive", None), ("Blog", Some(true))]);
        set_enabled(&root, "Blog", false).unwrap();
        set_enabled(&root, "Archive", true).unwrap();
        let list = super::list(&root);
        assert_eq!(list.iter().map(|m| m.enabled).collect::<Vec<_>>(), [Some(true), Some(false)]);
        std::fs::remove_dir_all(root).unwrap();
    }
}
