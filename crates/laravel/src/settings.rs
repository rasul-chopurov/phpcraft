//! Project settings for Laravel support, stored in `.phpcraft/laravel.json`.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::make::{Extra, IdType};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LaravelSettings {
    /// Laravel application directory relative to the project root (empty = root).
    pub laravel_dir: String,
    pub default_locale: String,
    pub controller_base: String,
    pub app_dir: String,
    pub config_dir: String,
    pub public_dir: String,
    pub model: ModelSettings,
    pub helper: HelperSettings,
}

impl Default for LaravelSettings {
    fn default() -> Self {
        Self {
            laravel_dir: String::new(),
            default_locale: "en".into(),
            controller_base: "App\\Http\\Controllers\\Controller".into(),
            app_dir: "app".into(),
            config_dir: "config".into(),
            public_dir: "public".into(),
            model: ModelSettings::default(),
            helper: HelperSettings::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum RelationColumns {
    /// `foreignId` / `foreignUuid` / `foreignUlid`
    #[default]
    ForeignId,
    /// `unsignedBigInteger` (+ `foreign()->references()->on()` when constrained)
    UnsignedBigInteger,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelSettings {
    pub id_name: String,
    pub id_type: IdType,
    pub relation_columns: RelationColumns,
    /// Add `->constrained()->cascadeOnDelete()` to relation columns.
    pub constrained: bool,
    /// `casts()` method instead of the `$casts` property.
    pub casts_method: bool,
    /// `dateTimeTz` / `timestampsTz` instead of their plain forms.
    pub timezones: bool,
    pub omit_down: bool,
    /// Refresh helper code (model properties) after creating a model.
    pub generate_helper_after: bool,
    /// Post-generations checked when the dialog opens.
    pub enabled: Vec<Extra>,
    /// Name templates with `${MODEL}` and `${TABLE_NAME}`, keyed by post-generation.
    pub templates: BTreeMap<Extra, String>,
}

impl Default for ModelSettings {
    fn default() -> Self {
        Self {
            id_name: "id".into(),
            id_type: IdType::Id,
            relation_columns: RelationColumns::ForeignId,
            constrained: true,
            casts_method: true,
            timezones: false,
            omit_down: false,
            generate_helper_after: true,
            enabled: vec![Extra::Migration, Extra::Factory],
            templates: BTreeMap::new(),
        }
    }
}

impl ModelSettings {
    pub fn default_template(extra: Extra) -> &'static str {
        match extra {
            Extra::Migration => "create_${TABLE_NAME}_table",
            Extra::Factory => "${MODEL}Factory",
            Extra::Seeder => "${MODEL}Seeder",
            Extra::JsonResource | Extra::FilamentResource => "${MODEL}Resource",
            Extra::Controller => "${MODEL}Controller",
            Extra::Requests => "Store${MODEL}Request, Update${MODEL}Request",
            Extra::Policy => "${MODEL}Policy",
            Extra::Observer => "${MODEL}Observer",
            Extra::Resource | Extra::Unit => "",
        }
    }

    pub fn template(&self, extra: Extra) -> String {
        self.templates
            .get(&extra)
            .filter(|t| !t.trim().is_empty())
            .cloned()
            .unwrap_or_else(|| Self::default_template(extra).to_string())
    }

    /// The name a post-generation gets for `model` (short class name) and `table`.
    pub fn name_for(&self, extra: Extra, model: &str, table: &str) -> String {
        self.template(extra).replace("${MODEL}", model).replace("${TABLE_NAME}", table)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum HelperSource {
    #[default]
    Migrations,
    Database,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct HelperSettings {
    pub source: HelperSource,
    /// `*_count` properties for to-many relations.
    pub relation_counts: bool,
    /// `where{Column}()` static methods.
    pub where_methods: bool,
}

impl Default for HelperSettings {
    fn default() -> Self {
        Self { source: HelperSource::Migrations, relation_counts: true, where_methods: true }
    }
}

fn settings_path(root: &Path) -> PathBuf {
    root.join(".phpcraft/laravel.json")
}

impl LaravelSettings {
    pub fn load(root: &Path) -> Self {
        fs::read_to_string(settings_path(root))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, root: &Path) -> std::io::Result<()> {
        let path = settings_path(root);
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        fs::write(path, serde_json::to_string_pretty(self).unwrap_or_default())
    }

    /// The Laravel application root.
    pub fn laravel_root(&self, project_root: &Path) -> PathBuf {
        let dir = self.laravel_dir.trim().trim_matches('/');
        if dir.is_empty() { project_root.to_path_buf() } else { project_root.join(dir) }
    }
}

/// `laravel/framework` version from composer.lock, e.g. `12.69.0`.
pub fn framework_version(root: &Path) -> Option<String> {
    let lock: serde_json::Value = serde_json::from_str(&fs::read_to_string(root.join("composer.lock")).ok()?).ok()?;
    lock["packages"]
        .as_array()?
        .iter()
        .find(|p| p["name"] == "laravel/framework")
        .and_then(|p| p["version"].as_str())
        .map(|v| v.trim_start_matches('v').to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_templates() {
        let root = std::env::temp_dir().join(format!("phpcraft-settings-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        assert_eq!(LaravelSettings::load(&root), LaravelSettings::default());
        let mut settings = LaravelSettings::default();
        settings.model.templates.insert(Extra::Controller, "Api\\${MODEL}ApiController".into());
        settings.model.timezones = true;
        settings.save(&root).unwrap();
        let loaded = LaravelSettings::load(&root);
        assert_eq!(loaded, settings);
        assert_eq!(loaded.model.name_for(Extra::Controller, "Post", "posts"), "Api\\PostApiController");
        assert_eq!(loaded.model.name_for(Extra::Migration, "Post", "posts"), "create_posts_table");
        fs::write(root.join("composer.lock"), r#"{"packages":[{"name":"laravel/framework","version":"v12.69.0"}]}"#).unwrap();
        assert_eq!(framework_version(&root).as_deref(), Some("12.69.0"));
        fs::remove_dir_all(&root).unwrap();
    }
}
