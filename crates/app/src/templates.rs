//! "New ▸" file templates: PHP class/interface/trait/enum with the namespace from
//! composer.json's PSR-4 map, Blade, Livewire, Vue, tests, web files and Docker files.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Template {
    PhpFile,
    PhpClass,
    PhpInterface,
    PhpTrait,
    PhpEnum,
    PhpUnitTest,
    PestTest,
    Blade,
    Livewire,
    Vue,
    JavaScript,
    TypeScript,
    Html,
    Css,
    Dockerfile,
    DockerCompose,
    EditorConfig,
    HttpRequest,
}

impl Template {
    pub const ALL: [Template; 18] = [
        Template::PhpFile,
        Template::PhpClass,
        Template::PhpInterface,
        Template::PhpTrait,
        Template::PhpEnum,
        Template::PhpUnitTest,
        Template::PestTest,
        Template::Blade,
        Template::Livewire,
        Template::Vue,
        Template::JavaScript,
        Template::TypeScript,
        Template::Html,
        Template::Css,
        Template::Dockerfile,
        Template::DockerCompose,
        Template::EditorConfig,
        Template::HttpRequest,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Template::PhpFile => "PHP File",
            Template::PhpClass => "PHP Class",
            Template::PhpInterface => "PHP Interface",
            Template::PhpTrait => "PHP Trait",
            Template::PhpEnum => "PHP Enum",
            Template::PhpUnitTest => "PHPUnit Test",
            Template::PestTest => "Pest Test",
            Template::Blade => "Blade View",
            Template::Livewire => "Livewire Component",
            Template::Vue => "Vue Component",
            Template::JavaScript => "JavaScript File",
            Template::TypeScript => "TypeScript File",
            Template::Html => "HTML File",
            Template::Css => "Stylesheet",
            Template::Dockerfile => "Dockerfile",
            Template::DockerCompose => "Docker Compose File",
            Template::EditorConfig => "EditorConfig File",
            Template::HttpRequest => "HTTP Request",
        }
    }

    /// Whether the user types a name (fixed-name files don't ask).
    pub fn needs_name(self) -> bool {
        !matches!(self, Template::Dockerfile | Template::DockerCompose | Template::EditorConfig)
    }
}

/// `App\Http\Controllers` for `<root>/app/Http/Controllers`, from the nearest composer.json
/// PSR-4 map (modules have their own composer.json).
pub fn namespace_for(dir: &Path, root: &Path) -> String {
    let mut current = Some(dir);
    while let Some(base) = current {
        if let Some(ns) = psr4_namespace(base, dir) {
            return ns;
        }
        if base == root {
            break;
        }
        current = base.parent();
    }
    String::new()
}

fn psr4_namespace(composer_dir: &Path, dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(composer_dir.join("composer.json")).ok()?;
    let json: serde_json::Value = serde_json::from_str(&text).ok()?;
    let mut best: Option<(usize, String)> = None;
    for section in ["autoload", "autoload-dev"] {
        let Some(map) = json[section]["psr-4"].as_object() else { continue };
        for (prefix, paths) in map {
            let paths: Vec<&str> = match paths {
                serde_json::Value::String(p) => vec![p.as_str()],
                serde_json::Value::Array(a) => a.iter().filter_map(|p| p.as_str()).collect(),
                _ => continue,
            };
            for p in paths {
                let base = composer_dir.join(p.trim_end_matches('/'));
                if let Ok(rest) = dir.strip_prefix(&base) {
                    let len = base.components().count();
                    if best.as_ref().is_none_or(|(l, _)| len > *l) {
                        let mut ns = prefix.trim_end_matches('\\').to_string();
                        for part in rest.components() {
                            ns.push('\\');
                            ns.push_str(&part.as_os_str().to_string_lossy());
                        }
                        best = Some((len, ns));
                    }
                }
            }
        }
    }
    best.map(|(_, ns)| ns)
}

fn kebab(name: &str) -> String {
    let mut out = String::new();
    for (i, c) in name.chars().enumerate() {
        if c.is_uppercase() && i > 0 {
            out.push('-');
        }
        out.extend(c.to_lowercase());
    }
    out
}

/// Files to create for `template` named `name` in `dir`: `(path, content)`. The first one
/// is opened.
pub fn render(template: Template, name: &str, dir: &Path, root: &Path) -> Vec<(PathBuf, String)> {
    let name = name.trim();
    let base = name.trim_end_matches(".php").trim_end_matches(".blade").trim_end_matches(".vue");
    let class = base.rsplit('/').next().unwrap_or(base);
    let php = |kind: &str, body: &str| {
        let ns = namespace_for(dir, root);
        let ns_line = if ns.is_empty() { String::new() } else { format!("namespace {ns};\n\n") };
        (dir.join(format!("{class}.php")), format!("<?php\n\n{ns_line}{kind} {class}\n{{\n{body}}}\n"))
    };
    match template {
        Template::PhpFile => vec![(dir.join(format!("{base}.php")), "<?php\n\n".into())],
        Template::PhpClass => vec![php("class", "")],
        Template::PhpInterface => vec![php("interface", "")],
        Template::PhpTrait => vec![php("trait", "")],
        Template::PhpEnum => vec![php("enum", "")],
        Template::PhpUnitTest => {
            let ns = namespace_for(dir, root);
            let ns_line = if ns.is_empty() { String::new() } else { format!("namespace {ns};\n\n") };
            vec![(
                dir.join(format!("{class}.php")),
                format!("<?php\n\n{ns_line}use Tests\\TestCase;\n\nclass {class} extends TestCase\n{{\n    public function test_example(): void\n    {{\n        $this->assertTrue(true);\n    }}\n}}\n"),
            )]
        }
        Template::PestTest => vec![(dir.join(format!("{class}.php")), "<?php\n\nit('works', function () {\n    expect(true)->toBeTrue();\n});\n".into())],
        Template::Blade => vec![(dir.join(format!("{}.blade.php", kebab(base))), "<div>\n    \n</div>\n".into())],
        Template::Livewire => {
            let view = kebab(class);
            vec![
                (
                    root.join("app/Livewire").join(format!("{class}.php")),
                    format!("<?php\n\nnamespace App\\Livewire;\n\nuse Livewire\\Component;\n\nclass {class} extends Component\n{{\n    public function render()\n    {{\n        return view('livewire.{view}');\n    }}\n}}\n"),
                ),
                (root.join("resources/views/livewire").join(format!("{view}.blade.php")), "<div>\n    {{-- … --}}\n</div>\n".into()),
            ]
        }
        Template::Vue => vec![(
            dir.join(format!("{class}.vue")),
            "<script setup>\n\n</script>\n\n<template>\n  <div>\n\n  </div>\n</template>\n\n<style scoped>\n\n</style>\n".into(),
        )],
        Template::JavaScript => vec![(dir.join(format!("{base}.js")), String::new())],
        Template::TypeScript => vec![(dir.join(format!("{base}.ts")), String::new())],
        Template::Html => vec![(
            dir.join(format!("{base}.html")),
            format!("<!doctype html>\n<html lang=\"en\">\n<head>\n    <meta charset=\"utf-8\">\n    <title>{class}</title>\n</head>\n<body>\n\n</body>\n</html>\n"),
        )],
        Template::Css => vec![(dir.join(format!("{base}.css")), String::new())],
        Template::Dockerfile => vec![(dir.join("Dockerfile"), "FROM php:8.4-fpm\n\nWORKDIR /var/www/html\n".into())],
        Template::DockerCompose => vec![(dir.join("compose.yaml"), "services:\n  app:\n    build: .\n    volumes:\n      - .:/var/www/html\n".into())],
        Template::HttpRequest => vec![(dir.join(format!("{}.http", base.trim_end_matches(".http"))), crate::http_client::TEMPLATE.into())],
        Template::EditorConfig => vec![(
            dir.join(".editorconfig"),
            "root = true\n\n[*]\ncharset = utf-8\nend_of_line = lf\nindent_size = 4\nindent_style = space\ninsert_final_newline = true\ntrim_trailing_whitespace = true\n\n[*.md]\ntrim_trailing_whitespace = false\n\n[*.{yml,yaml}]\nindent_size = 2\n".into(),
        )],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn namespaces_from_psr4_and_modules() {
        let root = std::env::temp_dir().join(format!("phpcraft-tpl-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("app/Http/Controllers")).unwrap();
        std::fs::create_dir_all(root.join("Modules/Blog/app/Models")).unwrap();
        std::fs::write(root.join("composer.json"), r#"{"autoload":{"psr-4":{"App\\":"app/"}}}"#).unwrap();
        std::fs::write(root.join("Modules/Blog/composer.json"), r#"{"autoload":{"psr-4":{"Modules\\Blog\\":"app/"}}}"#).unwrap();
        assert_eq!(namespace_for(&root.join("app/Http/Controllers"), &root), "App\\Http\\Controllers");
        assert_eq!(namespace_for(&root.join("Modules/Blog/app/Models"), &root), "Modules\\Blog\\Models");
        let files = render(Template::PhpClass, "PostController", &root.join("app/Http/Controllers"), &root);
        assert!(files[0].1.contains("namespace App\\Http\\Controllers;\n\nclass PostController"));
        let files = render(Template::Livewire, "ShowPosts", &root.join("app"), &root);
        assert!(files[0].1.contains("view('livewire.show-posts')"));
        assert!(files[1].0.ends_with("resources/views/livewire/show-posts.blade.php"));
        std::fs::remove_dir_all(root).unwrap();
    }
}
