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

/// The package is set up: its statuses file, its folder, or the dependency itself.
pub fn has_modules(root: &Path) -> bool {
    root.join("Modules").is_dir()
        || root.join("modules_statuses.json").is_file()
        || std::fs::read_to_string(root.join("composer.json")).is_ok_and(|t| t.contains("nwidart/laravel-modules"))
}

/// The module name as laravel-modules spells it (`blog`, `my-blog` → `Blog`, `MyBlog`), or `None`
/// when the text cannot name a module (empty, punctuation, a leading digit).
pub fn studly(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() || !trimmed.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '-' | '_')) {
        return None;
    }
    let mut out = String::new();
    for word in trimmed.split(|c: char| matches!(c, ' ' | '-' | '_')) {
        let mut chars = word.chars();
        if let Some(first) = chars.next() {
            out.extend(first.to_uppercase());
            out.push_str(chars.as_str());
        }
    }
    match out.chars().next() {
        Some(c) if !c.is_ascii_digit() => Some(out),
        _ => None,
    }
}

/// The layout `module:make` creates: the package's default, or one of its `--plain` / `--api` /
/// `--web` presets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModuleKind {
    Standard,
    Plain,
    Api,
    Web,
}

impl ModuleKind {
    pub const ALL: [ModuleKind; 4] = [ModuleKind::Standard, ModuleKind::Plain, ModuleKind::Api, ModuleKind::Web];

    pub fn label(self) -> &'static str {
        match self {
            ModuleKind::Standard => "Default",
            ModuleKind::Plain => "Plain",
            ModuleKind::Api => "API",
            ModuleKind::Web => "Web",
        }
    }

    /// The switch `module:make` understands; the package's config decides the default.
    pub fn flag(self) -> &'static str {
        match self {
            ModuleKind::Standard => "",
            ModuleKind::Plain => " --plain",
            ModuleKind::Api => " --api",
            ModuleKind::Web => " --web",
        }
    }
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
    fn names_become_studly_and_refuse_punctuation() {
        assert_eq!(studly("blog").as_deref(), Some("Blog"));
        assert_eq!(studly(" my-blog ").as_deref(), Some("MyBlog"));
        assert_eq!(studly("crm_v2").as_deref(), Some("CrmV2"));
        assert_eq!(studly(""), None);
        assert_eq!(studly("../evil"), None);
        assert_eq!(studly("1st"), None);
    }

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
