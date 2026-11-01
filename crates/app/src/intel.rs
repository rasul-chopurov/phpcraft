//! Code intelligence service shared by all editors: the project index plus framework providers.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Instant;

use laravel::Laravel;
use php_analysis::{CompletionItem, Document, StringProvider, TextEdit, Unresolved};
use php_index::{Index, Location};

pub struct Intel {
    root: PathBuf,
    index: RwLock<Index>,
    laravel: RwLock<Option<Laravel>>,
    /// Model columns/relations from Generate Helper Code, re-applied after reindexing.
    helpers: RwLock<Vec<laravel::helper::ModelHelp>>,
    columns: RwLock<laravel::columns::ColumnProvider>,
    framework: RwLock<laravel::framework::FrameworkProvider>,
    ready: AtomicBool,
}

impl Intel {
    pub fn new(root: &Path) -> Arc<Self> {
        Arc::new(Self {
            root: root.to_path_buf(),
            index: RwLock::new(Index::new()),
            laravel: RwLock::new(
                laravel::is_laravel_project(root).then(|| Laravel::for_project(root)),
            ),
            helpers: RwLock::new(Vec::new()),
            columns: RwLock::new(Default::default()),
            framework: RwLock::new(Default::default()),
            ready: AtomicBool::new(false),
        })
    }

    /// Builds the index; runs on a background thread.
    pub fn scan(&self) {
        let started = Instant::now();
        let mut index = Index::scan(&self.root);
        if self.is_laravel() {
            laravel::register_mixins(&mut index);
        }
        eprintln!(
            "indexed {} classes in {:.1?}",
            index.class_count(),
            started.elapsed()
        );
        *self.index.write().unwrap() = index;
        self.ready.store(true, Ordering::Release);
        if self.is_laravel() {
            self.generate_helpers();
        }
    }

    /// Generate Helper Code: computes model columns/relations and adds them to the index.
    /// Returns (models, members). Blocking.
    pub fn generate_helpers(&self) -> (usize, usize) {
        let Some(root) = self.laravel_root() else {
            return (0, 0);
        };
        let settings = laravel::settings::LaravelSettings::load(&self.root).helper;
        let helpers = {
            let index = self.index.read().unwrap();
            laravel::helper::model_helpers(&index, &root, &settings)
        };
        let members = helpers.iter().map(|h| h.members.len()).sum();
        let models = helpers.len();
        *self.columns.write().unwrap() = laravel::columns::ColumnProvider::new(&helpers);
        let framework = {
            let index = self.index.read().unwrap();
            laravel::framework::FrameworkProvider::new(&root, &index, &helpers)
        };
        *self.framework.write().unwrap() = framework;
        *self.helpers.write().unwrap() = helpers;
        self.apply_helpers(None);
        (models, members)
    }

    /// Re-applies helper members, to every model or only those declared in `path`.
    fn apply_helpers(&self, path: Option<&Path>) {
        let helpers = self.helpers.read().unwrap();
        let mut index = self.index.write().unwrap();
        for help in helpers.iter() {
            let in_file =
                path.is_none_or(|p| index.class(&help.fqn).is_some_and(|c| &*c.path == p));
            if in_file {
                index.augment(&help.fqn, help.members.clone());
            }
        }
    }

    /// Remove Model PhpDocs: strips `@property`/`@method` docblock lines from project models.
    /// Returns the files changed.
    pub fn remove_model_phpdocs(&self) -> Vec<PathBuf> {
        let Some(root) = self.laravel_root() else {
            return Vec::new();
        };
        let models = laravel::helper::project_models(&self.index.read().unwrap(), &root);
        let mut changed = Vec::new();
        for model in models {
            let Ok(source) = std::fs::read_to_string(&model.path) else {
                continue;
            };
            if let Some(updated) = laravel::helper::remove_model_phpdocs(&source)
                && std::fs::write(&model.path, &updated).is_ok()
            {
                self.update_file(&model.path, &updated);
                changed.push(model.path.to_path_buf());
            }
        }
        changed
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn is_ready(&self) -> bool {
        self.ready.load(Ordering::Acquire)
    }

    pub fn is_laravel(&self) -> bool {
        self.laravel.read().unwrap().is_some()
    }

    pub fn class_count(&self) -> usize {
        self.index.read().unwrap().class_count()
    }

    /// Re-reads `.phpcraft/laravel.json` (after the settings dialog).
    pub fn reload_settings(&self) {
        *self.laravel.write().unwrap() =
            laravel::is_laravel_project(&self.root).then(|| Laravel::for_project(&self.root));
    }

    fn with_providers<R>(&self, f: impl FnOnce(&[&dyn StringProvider]) -> R) -> R {
        let laravel = self.laravel.read().unwrap();
        let columns = self.columns.read().unwrap();
        let framework = self.framework.read().unwrap();
        let mut providers: Vec<&dyn StringProvider> =
            laravel.iter().map(|l| l as &dyn StringProvider).collect();
        providers.push(&*columns);
        providers.push(&*framework);
        f(&providers)
    }

    /// Laravel application root (may differ from the project root via settings).
    pub fn laravel_root(&self) -> Option<PathBuf> {
        self.laravel
            .read()
            .unwrap()
            .as_ref()
            .map(|l| l.root().to_path_buf())
    }

    pub fn update_file(&self, path: &Path, source: &str) {
        if path.extension().is_some_and(|e| e == "php") {
            self.index.write().unwrap().update_file(path, source);
            // Migrations change columns; everything else just needs its helpers back.
            let is_model = self.helpers.read().unwrap().iter().any(|h| {
                self.index
                    .read()
                    .unwrap()
                    .class(&h.fqn)
                    .is_some_and(|c| &*c.path == path)
            });
            if is_model || path.components().any(|c| c.as_os_str() == "migrations") {
                self.generate_helpers();
            } else {
                self.apply_helpers(Some(path));
            }
        }
    }

    pub fn goto(&self, path: &Path, source: &str, offset: usize) -> Vec<Location> {
        let index = self.index.read().unwrap();
        let doc = Document::parse(path, source);
        let found = doc
            .as_ref()
            .map(|doc| {
                self.with_providers(|p| php_analysis::goto_definition(doc, &index, offset, p))
            })
            .unwrap_or_default();
        if !found.is_empty() {
            return found;
        }
        // Blade templates aren't PHP code: fall back to text-based Laravel navigation.
        let text = self
            .laravel
            .read()
            .unwrap()
            .as_ref()
            .map(|l| l.text_goto(source, offset))
            .unwrap_or_default();
        if !text.is_empty() {
            return text;
        }
        // File paths in strings: `__DIR__.'/../bootstrap/app.php'`, `base_path('routes/web.php')`.
        let laravel_root = self.laravel_root().unwrap_or_else(|| self.root.clone());
        let helpers: Vec<(&str, PathBuf)> = [
            ("base_path", ""),
            ("app_path", "app"),
            ("config_path", "config"),
            ("database_path", "database"),
            ("lang_path", "lang"),
            ("public_path", "public"),
            ("resource_path", "resources"),
            ("storage_path", "storage"),
        ]
        .into_iter()
        .map(|(f, dir)| (f, laravel_root.join(dir)))
        .collect();
        let roots = [self.root.clone(), laravel_root.clone()];
        doc.and_then(|doc| php_analysis::file_path_at(&doc, offset, &helpers, &roots))
            .map(|path| vec![Location { path, range: 0..0 }])
            .unwrap_or_default()
    }

    /// Byte range to underline on ⌘+hover, if the symbol under `offset` navigates somewhere.
    pub fn link_at(
        &self,
        path: &Path,
        source: &str,
        offset: usize,
    ) -> Option<std::ops::Range<usize>> {
        let doc = Document::parse(path, source)?;
        let range =
            php_analysis::reference_range(&doc, offset).or_else(|| word_range(source, offset))?;
        (!self.goto(path, source, offset).is_empty()).then_some(range)
    }

    pub fn complete(&self, path: &Path, source: &str, offset: usize) -> Vec<CompletionItem> {
        // Blade directives: `@fore` -> `@foreach(...)`.
        if is_blade(path) {
            let word_start = source[..offset]
                .char_indices()
                .rev()
                .take_while(|(_, c)| c.is_ascii_alphanumeric())
                .last()
                .map_or(offset, |(i, _)| i);
            let at_sign = word_start.checked_sub(1).filter(|&i| source.as_bytes()[i] == b'@');
            let after_word_char = at_sign.and_then(|i| i.checked_sub(1)).is_some_and(|i| source.as_bytes()[i].is_ascii_alphanumeric());
            if let Some(at) = at_sign.filter(|_| !after_word_char) {
                let typed = source[word_start..offset].to_ascii_lowercase();
                return laravel::framework::blade_directive_items(at..offset)
                    .into_iter()
                    .filter(|item| item.label[1..].to_ascii_lowercase().starts_with(&typed))
                    .collect();
            }
        }
        let index = self.index.read().unwrap();
        Document::parse(path, source)
            .map(|doc| self.with_providers(|p| php_analysis::complete(&doc, &index, offset, p)))
            .unwrap_or_default()
    }

    pub fn unresolved(&self, path: &Path, source: &str) -> Vec<Unresolved> {
        if !self.is_ready() || is_blade(path) {
            return Vec::new();
        }
        let index = self.index.read().unwrap();
        Document::parse(path, source)
            .map(|doc| php_analysis::unresolved_classes(&doc, &index))
            .unwrap_or_default()
    }

    /// Where a route's action is defined: controller method, invokable class, or closure line.
    pub fn route_location(&self, route: &laravel::routes::RouteInfo) -> Option<Location> {
        if let Some((class, method)) = route.controller() {
            let index = self.index.read().unwrap();
            if let Some((owner, member)) =
                index.find_member(class, method, &[php_index::MemberKind::Method])
            {
                return Some(Location {
                    path: owner.path.to_path_buf(),
                    range: member.range,
                });
            }
            return index.class_location(class);
        }
        let (file, line) = route.defined_at.as_ref()?;
        let path = self.root.join(file);
        let source = std::fs::read_to_string(&path).ok()?;
        let offset: usize = source
            .split_inclusive('\n')
            .take(line.saturating_sub(1))
            .map(str::len)
            .sum();
        Some(Location {
            path,
            range: offset..offset,
        })
    }

    /// All indexed classes: (short name, FQN, location). Project classes first.
    pub fn class_entries(&self) -> Vec<(String, String, Location)> {
        let index = self.index.read().unwrap();
        let vendor = self.root.join("vendor");
        let mut out: Vec<(bool, String, String, Location)> = index
            .classes()
            .map(|c| {
                let location = Location {
                    path: c.path.to_path_buf(),
                    range: c.range.clone(),
                };
                (
                    c.path.starts_with(&vendor),
                    c.name.clone(),
                    c.fqn.clone(),
                    location,
                )
            })
            .collect();
        out.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));
        out.into_iter()
            .map(|(_, name, fqn, loc)| (name, fqn, loc))
            .collect()
    }

    /// Project (non-vendor) members and functions: (label, container, icon, location).
    pub fn symbol_entries(&self) -> Vec<(String, String, &'static str, Location)> {
        let index = self.index.read().unwrap();
        let vendor = self.root.join("vendor");
        let mut out = Vec::new();
        for class in index.classes().filter(|c| !c.path.starts_with(&vendor)) {
            for member in class.members.iter().filter(|m| !m.virtual_member) {
                let icon = match member.kind {
                    php_index::MemberKind::Method => "m",
                    php_index::MemberKind::Property => "f",
                    _ => "k",
                };
                out.push((
                    member.name.clone(),
                    class.fqn.clone(),
                    icon,
                    Location {
                        path: class.path.to_path_buf(),
                        range: member.range.clone(),
                    },
                ));
            }
        }
        for function in index.functions().filter(|f| !f.path.starts_with(&vendor)) {
            out.push((
                php_index::short_name(&function.fqn).to_string(),
                php_index::namespace_of(&function.fqn).to_string(),
                "ƒ",
                Location {
                    path: function.path.to_path_buf(),
                    range: function.range.clone(),
                },
            ));
        }
        out
    }

    /// Eloquent models declared in the project (not vendor), as FQNs sorted by short name.
    pub fn models(&self) -> Vec<String> {
        const MODEL: &str = "illuminate\\database\\eloquent\\model";
        let index = self.index.read().unwrap();
        let vendor = self.root.join("vendor");
        let mut models: Vec<String> = index
            .classes()
            .filter(|c| !c.path.starts_with(&vendor))
            .filter(|c| {
                index
                    .ancestors(&c.fqn)
                    .iter()
                    .skip(1)
                    .any(|a| a.fqn.eq_ignore_ascii_case(MODEL))
            })
            .map(|c| c.fqn.clone())
            .collect();
        models.sort_by_key(|f| php_index::short_name(f).to_ascii_lowercase());
        models
    }

    pub fn import(&self, path: &Path, source: &str, fqn: &str) -> Option<TextEdit> {
        Document::parse(path, source).and_then(|doc| php_analysis::import_edit(&doc, fqn))
    }

    pub fn optimize_imports(&self, path: &Path, source: &str) -> Option<TextEdit> {
        Document::parse(path, source).and_then(|doc| php_analysis::optimize_imports(&doc))
    }
}

fn is_blade(path: &Path) -> bool {
    path.to_string_lossy().ends_with(".blade.php")
}

/// Identifier-ish range around `offset` (used in Blade, where there is no syntax tree).
fn word_range(source: &str, offset: usize) -> Option<std::ops::Range<usize>> {
    let is_word = |c: char| c.is_alphanumeric() || matches!(c, '_' | '.' | '-' | ':');
    let start = source[..offset]
        .char_indices()
        .rev()
        .take_while(|(_, c)| is_word(*c))
        .last()
        .map_or(offset, |(i, _)| i);
    let end = source[offset..]
        .find(|c| !is_word(c))
        .map_or(source.len(), |i| offset + i);
    (start < end).then_some(start..end)
}
