//! "Laravel Make" code generation: models with their migration/factory/seeder/controller/
//! policy/requests, and the other `make:*` classes. Files are produced from built-in
//! templates (no `php artisan` needed) so the result can be previewed before writing.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[path = "make_filament.rs"]
mod filament;
#[path = "make_model.rs"]
pub mod model_spec;

pub use model_spec::{COLUMN_TYPES, Field, IdType, ModelSpec, Relation, RelationKind, parse_fields, parse_relations};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MakeKind {
    Model,
    Migration,
    PivotTable,
    Controller,
    Request,
    Resource,
    Policy,
    Observer,
    Factory,
    Seeder,
    Event,
    Listener,
    Job,
    Middleware,
    Command,
    Enum,
    Test,
    FilamentResource,
    FilamentPage,
    FilamentWidget,
}

impl MakeKind {
    pub const ALL: [MakeKind; 20] = [
        Self::Model,
        Self::Migration,
        Self::PivotTable,
        Self::Controller,
        Self::Request,
        Self::Resource,
        Self::Policy,
        Self::Observer,
        Self::Factory,
        Self::Seeder,
        Self::Event,
        Self::Listener,
        Self::Job,
        Self::Middleware,
        Self::Command,
        Self::Enum,
        Self::Test,
        Self::FilamentResource,
        Self::FilamentPage,
        Self::FilamentWidget,
    ];

    pub fn is_filament(self) -> bool {
        matches!(self, Self::FilamentResource | Self::FilamentPage | Self::FilamentWidget)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Model => "Eloquent Model",
            Self::Migration => "Migration",
            Self::PivotTable => "Pivot Table",
            Self::Controller => "Controller",
            Self::Request => "Form Request",
            Self::Resource => "JSON Resource",
            Self::Policy => "Policy",
            Self::Observer => "Observer",
            Self::Factory => "Factory",
            Self::Seeder => "Seeder",
            Self::Event => "Event",
            Self::Listener => "Listener",
            Self::Job => "Job",
            Self::Middleware => "Middleware",
            Self::Command => "Console Command",
            Self::Enum => "Enum",
            Self::Test => "Test",
            Self::FilamentResource => "Filament Resource",
            Self::FilamentPage => "Filament Page",
            Self::FilamentWidget => "Filament Widget",
        }
    }

    pub fn placeholder(self) -> &'static str {
        match self {
            Self::Model => "Post or Blog/Post",
            Self::Migration => "create_posts_table",
            Self::PivotTable => "Post, Tag",
            Self::Controller => "PostController",
            Self::Request => "StorePostRequest",
            Self::Resource => "PostResource",
            Self::Policy => "PostPolicy",
            Self::Observer => "PostObserver",
            Self::Factory => "PostFactory",
            Self::Seeder => "PostSeeder",
            Self::Event => "OrderShipped",
            Self::Listener => "SendShipmentNotification",
            Self::Job => "ProcessPodcast",
            Self::Middleware => "EnsureTokenIsValid",
            Self::Command => "SendEmails",
            Self::Enum => "Status",
            Self::Test => "UserTest",
            Self::FilamentResource => "PostResource",
            Self::FilamentPage => "Settings",
            Self::FilamentWidget => "StatsOverview",
        }
    }

    /// Optional extras shown as checkboxes in the dialog.
    pub fn extras(self) -> &'static [Extra] {
        match self {
            Self::Model => &[
                Extra::Migration,
                Extra::Factory,
                Extra::JsonResource,
                Extra::FilamentResource,
                Extra::Seeder,
                Extra::Controller,
                Extra::Requests,
                Extra::Policy,
                Extra::Observer,
            ],
            Self::Controller => &[Extra::Resource, Extra::Requests],
            Self::Test => &[Extra::Unit],
            _ => &[],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Extra {
    Migration,
    Factory,
    Seeder,
    JsonResource,
    FilamentResource,
    /// REST-API (resource) controller for the model.
    Controller,
    Policy,
    Requests,
    Observer,
    /// Resource controller (index/create/store/show/edit/update/destroy).
    Resource,
    /// Unit test instead of feature test.
    Unit,
}

impl Extra {
    pub fn label(self) -> &'static str {
        match self {
            Self::Migration => "Migration",
            Self::Factory => "Database factory",
            Self::Seeder => "Seeder",
            Self::JsonResource => "Json resource",
            Self::FilamentResource => "Filament resource",
            Self::Controller => "REST-API Controller",
            Self::Policy => "Policy",
            Self::Requests => "Form request",
            Self::Observer => "Observer",
            Self::Resource => "Resource methods",
            Self::Unit => "Unit test",
        }
    }

    /// Checked by default when the dialog opens.
    pub fn default_on(self) -> bool {
        matches!(self, Self::Migration | Self::Factory | Self::Resource)
    }

    /// Default class/file name generated for a model, shown in the dialog's name field.
    pub fn default_name(self, model: &str) -> String {
        let model = ClassName::parse(model).map(|c| c.name).unwrap_or_default();
        match self {
            Self::Migration => format!("create_{}_table", table_name(&model)),
            Self::Factory => format!("{model}Factory"),
            Self::Seeder => format!("{model}Seeder"),
            Self::JsonResource | Self::FilamentResource => format!("{model}Resource"),
            Self::Controller => format!("{model}Controller"),
            Self::Requests => format!("Store{model}Request, Update{model}Request"),
            Self::Policy => format!("{model}Policy"),
            Self::Observer => format!("{model}Observer"),
            Self::Resource | Self::Unit => String::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedFile {
    pub path: PathBuf,
    pub contents: String,
    pub exists: bool,
}

/// A class name with optional sub-namespace, e.g. `Blog/Post` or `Blog\Post`.
pub(crate) struct ClassName {
    /// `["Blog"]`
    pub dirs: Vec<String>,
    /// `Post`
    pub name: String,
}

impl ClassName {
    fn parse(input: &str) -> Option<Self> {
        let mut parts: Vec<String> = input
            .trim()
            .trim_end_matches(".php")
            .split(['/', '\\'])
            .filter(|p| !p.is_empty())
            .map(studly)
            .collect();
        let name = parts.pop().filter(|n| n.chars().next().is_some_and(|c| c.is_alphabetic()))?;
        Some(Self { dirs: parts, name })
    }

    fn with_suffix(&self, suffix: &str) -> Self {
        let name = if self.name.ends_with(suffix) { self.name.clone() } else { format!("{}{suffix}", self.name) };
        Self { dirs: self.dirs.clone(), name }
    }

    pub fn namespace(&self, base: &str) -> String {
        std::iter::once(base.to_string()).chain(self.dirs.iter().cloned()).collect::<Vec<_>>().join("\\")
    }

    fn fqn(&self, base: &str) -> String {
        format!("{}\\{}", self.namespace(base), self.name)
    }

    pub fn path(&self, root: &Path, base_dir: &str) -> PathBuf {
        let mut path = root.join(base_dir);
        for dir in &self.dirs {
            path.push(dir);
        }
        path.join(format!("{}.php", self.name))
    }
}

pub struct Maker {
    root: PathBuf,
    /// PSR-4 namespace for `app/`, usually `App`.
    app_namespace: String,
    uses_pest: bool,
    uses_filament: bool,
    /// Filament 3 nests pages under `PostResource/`; 4+ uses `Posts/` with schema classes.
    filament_v3: bool,
}

/// Where generated files go: the application or one laravel-modules module.
pub(crate) struct Target {
    pub root: PathBuf,
    pub app_namespace: String,
    pub database_namespace: String,
    pub tests_namespace: String,
}

impl Target {
    pub fn app(&self, sub: &str) -> String {
        if sub.is_empty() {
            self.app_namespace.clone()
        } else {
            format!("{}\\{sub}", self.app_namespace)
        }
    }

    fn app_dir(sub: &str) -> String {
        if sub.is_empty() { "app".into() } else { format!("app/{}", sub.replace('\\', "/")) }
    }

    pub fn file(&self, path: PathBuf, contents: String) -> PlannedFile {
        PlannedFile { exists: path.exists(), path, contents }
    }

    fn class_file(&self, class: &ClassName, sub: &str, contents: String) -> PlannedFile {
        self.file(class.path(&self.root, &Self::app_dir(sub)), contents)
    }

    fn migration_path(&self, name: &str) -> PathBuf {
        self.migration_path_after(name, 0)
    }

    /// `seconds` shifts the timestamp so dependent migrations (pivots) sort after their tables.
    fn migration_path_after(&self, name: &str, seconds: i64) -> PathBuf {
        self.root.join("database/migrations").join(format!("{}_{name}.php", migration_timestamp(seconds)))
    }

    fn migration(&self, name: &str) -> PlannedFile {
        self.file(self.migration_path(name), migration_file(name))
    }

    fn has_migration(&self, name: &str) -> bool {
        fs::read_dir(self.root.join("database/migrations"))
            .map(|entries| entries.flatten().any(|e| e.file_name().to_string_lossy().ends_with(&format!("_{name}.php"))))
            .unwrap_or(false)
    }

    fn pivot_migration(&self, a: (&str, IdType), b: (&str, IdType)) -> PlannedFile {
        let table = model_spec::pivot_table(a.0, b.0);
        let path = self.migration_path_after(&format!("create_{table}_table"), 1);
        self.file(path, create_migration(&table, &model_spec::pivot_columns(a, b)))
    }

    fn factory(&self, factory: &ClassName, model_fqn: &str) -> PlannedFile {
        let path = factory.path(&self.root, "database/factories");
        let ns = format!("{}\\Factories", self.database_namespace);
        self.file(path, factory_source(factory, &ns, model_fqn, &[model_fqn.to_string()], "            //\n"))
    }

    fn seeder(&self, seeder: &ClassName) -> PlannedFile {
        let path = seeder.path(&self.root, "database/seeders");
        self.file(path, seeder_file(seeder, &format!("{}\\Seeders", self.database_namespace)))
    }
}

impl Maker {
    pub fn new(root: &Path) -> Self {
        let composer = fs::read_to_string(root.join("composer.json")).unwrap_or_default();
        let lock = fs::read_to_string(root.join("composer.lock")).unwrap_or_default();
        let app_namespace = composer
            .find("\"app/\"")
            .and_then(|at| {
                let before = &composer[..at];
                let key_end = before.rfind("\":")?;
                let key_start = before[..key_end].rfind('"')? + 1;
                Some(before[key_start..key_end].trim_end_matches("\\\\").to_string())
            })
            .unwrap_or_else(|| "App".into());
        Self {
            root: root.to_path_buf(),
            app_namespace,
            // `pestphp/pest-plugin` alone (allow-plugins config) doesn't mean Pest is installed.
            uses_pest: composer.contains("\"pestphp/pest\":") || root.join("vendor/pestphp/pest").is_dir(),
            uses_filament: has_filament(root, &composer, &lock),
            filament_v3: package_version(&lock, "filament/filament").is_some_and(|v| v.starts_with('3')),
        }
    }

    /// Kinds available for this project (Filament generators only when it is installed).
    pub fn kinds(&self) -> Vec<MakeKind> {
        MakeKind::ALL
            .into_iter()
            .filter(|k| self.uses_filament || !k.is_filament())
            .collect()
    }

    /// Post-generation options for `kind` available in this project.
    pub fn extras(&self, kind: MakeKind) -> Vec<Extra> {
        kind.extras()
            .iter()
            .copied()
            .filter(|e| self.uses_filament || *e != Extra::FilamentResource)
            .collect()
    }

    /// laravel-modules (nwidart) modules: `Modules/<Name>/module.json`.
    pub fn modules(&self) -> Vec<String> {
        let Ok(entries) = fs::read_dir(self.root.join("Modules")) else { return Vec::new() };
        let mut modules: Vec<String> = entries
            .flatten()
            .filter(|e| e.path().join("module.json").is_file())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        modules.sort();
        modules
    }

    pub(crate) fn target(&self, module: Option<&str>) -> Target {
        match module {
            Some(module) => {
                let namespace = format!("Modules\\{module}");
                Target {
                    root: self.root.join("Modules").join(module),
                    app_namespace: namespace.clone(),
                    database_namespace: format!("{namespace}\\Database"),
                    tests_namespace: format!("{namespace}\\Tests"),
                }
            }
            None => Target {
                root: self.root.clone(),
                app_namespace: self.app_namespace.clone(),
                database_namespace: "Database".into(),
                tests_namespace: "Tests".into(),
            },
        }
    }

    /// Files that would be created for `name` with the given extras.
    /// `module` selects a laravel-modules module (see [`Maker::modules`]); `None` is the app itself.
    pub fn plan(&self, kind: MakeKind, name: &str, extras: &[Extra], module: Option<&str>) -> Vec<PlannedFile> {
        self.plan_with(kind, name, extras, module, &ModelSpec::default())
    }

    /// Like [`Maker::plan`], with the model dialog's columns, key type and relations.
    pub fn plan_with(
        &self,
        kind: MakeKind,
        name: &str,
        extras: &[Extra],
        module: Option<&str>,
        spec: &ModelSpec,
    ) -> Vec<PlannedFile> {
        let t = self.target(module);
        if kind == MakeKind::PivotTable {
            let models: Vec<&str> = name.split([',', ' ', '/']).filter(|m| !m.is_empty()).collect();
            return match models.as_slice() {
                [a, b] => vec![t.pivot_migration((&studly(a), IdType::Id), (&studly(b), IdType::Id))],
                _ => Vec::new(),
            };
        }
        let Some(class) = ClassName::parse(name) else { return Vec::new() };
        let has = |e: Extra| extras.contains(&e);
        let mut files = Vec::new();
        match kind {
            MakeKind::Model => {
                let model_fqn = class.fqn(&t.app("Models"));
                // A name typed in the dialog overrides the derived one.
                let named = |extra: Extra| {
                    spec.class_names
                        .get(&extra)
                        .map(|n| n.trim().to_string())
                        .filter(|n| !n.is_empty() && *n != extra.default_name(&class.name))
                };
                let sibling = |extra: Extra, suffix: &str| match named(extra) {
                    Some(name) => ClassName { dirs: class.dirs.clone(), name: studly(&name) },
                    None => class.with_suffix(suffix),
                };
                let source = model_spec::model_source(&class.namespace(&t.app("Models")), &class.name, has(Extra::Factory), spec);
                files.push(t.class_file(&class, "Models", source));
                let table = spec.table.clone().filter(|t| !t.trim().is_empty()).unwrap_or_else(|| table_name(&class.name));
                if has(Extra::Migration) {
                    let name = named(Extra::Migration).map(|n| snake(&n)).unwrap_or_else(|| format!("create_{table}_table"));
                    let path = t.migration_path(&name);
                    files.push(t.file(path, create_migration(&table, &model_spec::create_columns(spec))));
                    for relation in spec.relations.iter().filter(|r| r.kind == RelationKind::BelongsToMany) {
                        let pivot = model_spec::pivot_table(&class.name, &relation.target);
                        if !t.has_migration(&format!("create_{pivot}_table")) {
                            files.push(t.pivot_migration((&class.name, spec.id), (&relation.target, IdType::Id)));
                        }
                    }
                }
                if has(Extra::Factory) {
                    let factory = sibling(Extra::Factory, "Factory");
                    let (definition, models) = model_spec::factory_definition(spec);
                    let mut uses: Vec<String> = models
                        .iter()
                        .map(|(m, fqn)| fqn.clone().unwrap_or_else(|| format!("{}\\{m}", t.app("Models"))))
                        .collect();
                    uses.push(model_fqn.clone());
                    let path = factory.path(&t.root, "database/factories");
                    let ns = format!("{}\\Factories", t.database_namespace);
                    files.push(t.file(path, factory_source(&factory, &ns, &model_fqn, &uses, &definition)));
                }
                if has(Extra::JsonResource) {
                    let resource = sibling(Extra::JsonResource, "Resource");
                    files.push(t.class_file(&resource, "Http\\Resources", json_resource(&resource, &t.app("Http\\Resources"))));
                }
                if has(Extra::FilamentResource) {
                    let model = named(Extra::FilamentResource)
                        .map(|n| studly(n.trim_end_matches("Resource")))
                        .unwrap_or_else(|| class.name.clone());
                    files.extend(filament::resource(&t, &model, &model_fqn, self.filament_v3));
                }
                if has(Extra::Seeder) {
                    files.push(t.seeder(&sibling(Extra::Seeder, "Seeder")));
                }
                let mut requests = Vec::new();
                if has(Extra::Requests) {
                    let names: Vec<String> = match named(Extra::Requests) {
                        Some(names) => names.split(',').map(|n| studly(n.trim())).filter(|n| !n.is_empty()).collect(),
                        None => vec![format!("Store{}Request", class.name), format!("Update{}Request", class.name)],
                    };
                    for name in names {
                        let request = ClassName { dirs: class.dirs.clone(), name };
                        files.push(t.class_file(&request, "Http\\Requests", form_request(&request, &t.app("Http\\Requests"))));
                        requests.push(request.name);
                    }
                }
                if has(Extra::Controller) {
                    let controller = sibling(Extra::Controller, "Controller");
                    let contents = controller_file(
                        &controller,
                        &t.app("Http\\Controllers"),
                        &t.app_namespace,
                        Some((&model_fqn, &class.name)),
                        ControllerStyle::Api,
                        (requests.len() == 2).then(|| (requests[0].clone(), requests[1].clone())),
                    );
                    files.push(t.class_file(&controller, "Http\\Controllers", contents));
                }
                if has(Extra::Policy) {
                    let policy = sibling(Extra::Policy, "Policy");
                    files.push(t.class_file(&policy, "Policies", policy_file(&policy, &t.app("Policies"), &t.app_namespace, &model_fqn, &class.name)));
                }
                if has(Extra::Observer) {
                    let observer = sibling(Extra::Observer, "Observer");
                    files.push(t.class_file(&observer, "Observers", observer_file(&observer, &t.app("Observers"), &model_fqn, &class.name)));
                }
            }
            MakeKind::Migration => files.push(t.migration(&snake(name))),
            MakeKind::PivotTable => {} // handled above: takes two model names
            MakeKind::Controller => {
                let controller = class.with_suffix("Controller");
                let model = controller.name.strip_suffix("Controller").filter(|_| has(Extra::Resource)).map(|m| m.to_string());
                let model_fqn = model.as_ref().map(|m| format!("{}\\{m}", t.app("Models")));
                let requests = model
                    .as_ref()
                    .filter(|_| has(Extra::Requests))
                    .map(|m| (format!("Store{m}Request"), format!("Update{m}Request")));
                let contents = controller_file(
                    &controller,
                    &t.app("Http\\Controllers"),
                    &t.app_namespace,
                    model_fqn.as_deref().zip(model.as_deref()),
                    if has(Extra::Resource) { ControllerStyle::Resource } else { ControllerStyle::Plain },
                    requests,
                );
                files.push(t.class_file(&controller, "Http\\Controllers", contents));
                if has(Extra::Requests) && let Some(model) = &model {
                    for prefix in ["Store", "Update"] {
                        let request = ClassName { dirs: class.dirs.clone(), name: format!("{prefix}{model}Request") };
                        files.push(t.class_file(&request, "Http\\Requests", form_request(&request, &t.app("Http\\Requests"))));
                    }
                }
            }
            MakeKind::Request => files.push(t.class_file(&class, "Http\\Requests", form_request(&class, &t.app("Http\\Requests")))),
            MakeKind::Resource => {
                let resource = class.with_suffix("Resource");
                files.push(t.class_file(&resource, "Http\\Resources", json_resource(&resource, &t.app("Http\\Resources"))));
            }
            MakeKind::Policy => {
                let policy = class.with_suffix("Policy");
                let model = policy.name.trim_end_matches("Policy").to_string();
                let model_fqn = format!("{}\\{model}", t.app("Models"));
                files.push(t.class_file(&policy, "Policies", policy_file(&policy, &t.app("Policies"), &t.app_namespace, &model_fqn, &model)));
            }
            MakeKind::Observer => {
                let observer = class.with_suffix("Observer");
                let model = observer.name.trim_end_matches("Observer").to_string();
                let model_fqn = format!("{}\\{model}", t.app("Models"));
                files.push(t.class_file(&observer, "Observers", observer_file(&observer, &t.app("Observers"), &model_fqn, &model)));
            }
            MakeKind::Factory => {
                let factory = class.with_suffix("Factory");
                let model_fqn = format!("{}\\{}", t.app("Models"), factory.name.trim_end_matches("Factory"));
                files.push(t.factory(&factory, &model_fqn));
            }
            MakeKind::Seeder => files.push(t.seeder(&class.with_suffix("Seeder"))),
            MakeKind::Event => files.push(t.class_file(&class, "Events", event_file(&class, &t.app("Events")))),
            MakeKind::Listener => files.push(t.class_file(&class, "Listeners", listener_file(&class, &t.app("Listeners")))),
            MakeKind::Job => files.push(t.class_file(&class, "Jobs", job_file(&class, &t.app("Jobs")))),
            MakeKind::Middleware => files.push(t.class_file(&class, "Http\\Middleware", middleware_file(&class, &t.app("Http\\Middleware")))),
            MakeKind::Command => files.push(t.class_file(&class, "Console\\Commands", command_file(&class, &t.app("Console\\Commands")))),
            MakeKind::Enum => files.push(t.class_file(&class, "Enums", enum_file(&class, &t.app("Enums")))),
            MakeKind::FilamentResource => {
                let model = class.name.trim_end_matches("Resource").to_string();
                let model_fqn = format!("{}\\{model}", t.app("Models"));
                files.extend(filament::resource(&t, &model, &model_fqn, self.filament_v3));
            }
            MakeKind::FilamentPage => files.extend(filament::page(&t, &class, self.filament_v3)),
            MakeKind::FilamentWidget => files.extend(filament::widget(&t, &class, self.filament_v3)),
            MakeKind::Test => {
                let test = class.with_suffix("Test");
                let suite = if has(Extra::Unit) { "Unit" } else { "Feature" };
                let contents = if self.uses_pest { pest_test(suite) } else { phpunit_test(&test, &t.tests_namespace, suite) };
                files.push(t.file(test.path(&t.root, &format!("tests/{suite}")), contents));
            }
        }
        files
    }

}

/// Filament may be required directly, pulled in transitively, or only by a laravel-modules module.
fn has_filament(root: &Path, composer: &str, lock: &str) -> bool {
    let named = |text: &str| text.contains("\"filament/filament\"");
    named(composer)
        || named(lock)
        || root.join("vendor/filament/filament").is_dir()
        || fs::read_dir(root.join("Modules")).is_ok_and(|entries| {
            entries
                .flatten()
                .any(|e| fs::read_to_string(e.path().join("composer.json")).is_ok_and(|c| named(&c)))
        })
}

/// `"version": "v3.2.1"` of a package in composer.lock, without the leading `v`.
fn package_version(lock: &str, package: &str) -> Option<String> {
    let at = lock.find(&format!("\"name\": \"{package}\""))?;
    let rest = &lock[at..];
    let version = rest.find("\"version\":").map(|i| &rest[i + 10..])?;
    let start = version.find('"')? + 1;
    let end = version[start..].find('"')? + start;
    Some(version[start..end].trim_start_matches('v').to_string())
}

/// Writes planned files, never overwriting existing ones. Returns the paths written.
pub fn write(files: &[PlannedFile]) -> std::io::Result<Vec<PathBuf>> {
    let mut written = Vec::new();
    for file in files.iter().filter(|f| !f.exists && !f.path.exists()) {
        if let Some(dir) = file.path.parent() {
            fs::create_dir_all(dir)?;
        }
        fs::write(&file.path, &file.contents)?;
        written.push(file.path.clone());
    }
    Ok(written)
}

// --- naming ------------------------------------------------------------------

pub fn studly(s: &str) -> String {
    s.split(|c: char| c == '_' || c == '-' || c == ' ')
        .filter(|p| !p.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            chars.next().map(|c| c.to_uppercase().chain(chars).collect::<String>()).unwrap_or_default()
        })
        .collect()
}

pub fn snake(s: &str) -> String {
    let mut out = String::new();
    for (i, c) in s.trim().chars().enumerate() {
        if c.is_uppercase() {
            if i > 0 && !out.ends_with('_') {
                out.push('_');
            }
            out.extend(c.to_lowercase());
        } else if c == ' ' || c == '-' {
            out.push('_');
        } else {
            out.push(c);
        }
    }
    out
}

pub fn plural(word: &str) -> String {
    const IRREGULAR: &[(&str, &str)] = &[
        ("person", "people"), ("man", "men"), ("child", "children"), ("mouse", "mice"),
        ("foot", "feet"), ("tooth", "teeth"), ("datum", "data"), ("medium", "media"),
    ];
    if let Some((_, p)) = IRREGULAR.iter().find(|(s, _)| *s == word) {
        return p.to_string();
    }
    let vowel_before_y = word.len() > 1 && "aeiou".contains(&word[word.len() - 2..word.len() - 1]);
    if word.ends_with('y') && !vowel_before_y {
        format!("{}ies", &word[..word.len() - 1])
    } else if ["s", "x", "z", "ch", "sh"].iter().any(|e| word.ends_with(e)) {
        format!("{word}es")
    } else {
        format!("{word}s")
    }
}

/// `BlogPost` -> `blog_posts`.
pub fn table_name(model: &str) -> String {
    let snake = snake(model);
    match snake.rsplit_once('_') {
        Some((head, last)) => format!("{head}_{}", plural(last)),
        None => plural(&snake),
    }
}

fn camel(s: &str) -> String {
    let mut chars = s.chars();
    chars.next().map(|c| c.to_lowercase().chain(chars).collect()).unwrap_or_default()
}

fn migration_timestamp(offset_secs: i64) -> String {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs()) as i64 + offset_secs;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Civil-from-days (Howard Hinnant).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}_{month:02}_{day:02}_{:02}{:02}{:02}",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

// --- templates ---------------------------------------------------------------

fn create_migration(table: &str, columns: &str) -> String {
    migration_source(
        &format!("        Schema::create('{table}', function (Blueprint $table) {{\n{columns}        }});"),
        &format!("        Schema::dropIfExists('{table}');"),
    )
}

fn migration_source(up: &str, down: &str) -> String {
    format!(
        "<?php\n\nuse Illuminate\\Database\\Migrations\\Migration;\nuse Illuminate\\Database\\Schema\\Blueprint;\nuse Illuminate\\Support\\Facades\\Schema;\n\nreturn new class extends Migration\n{{\n    /**\n     * Run the migrations.\n     */\n    public function up(): void\n    {{\n{up}\n    }}\n\n    /**\n     * Reverse the migrations.\n     */\n    public function down(): void\n    {{\n{down}\n    }}\n}};\n"
    )
}

fn migration_file(name: &str) -> String {
    let create = name.strip_prefix("create_").and_then(|n| n.strip_suffix("_table"));
    let alter = name
        .rsplit_once("_to_")
        .or_else(|| name.rsplit_once("_from_"))
        .or_else(|| name.rsplit_once("_in_"))
        .and_then(|(_, t)| t.strip_suffix("_table"));
    let (up, down) = match (create, alter) {
        (Some(table), _) => (
            format!("        Schema::create('{table}', function (Blueprint $table) {{\n            $table->id();\n            $table->timestamps();\n        }});"),
            format!("        Schema::dropIfExists('{table}');"),
        ),
        (None, Some(table)) => (
            format!("        Schema::table('{table}', function (Blueprint $table) {{\n            //\n        }});"),
            format!("        Schema::table('{table}', function (Blueprint $table) {{\n            //\n        }});"),
        ),
        _ => ("        //".to_string(), "        //".to_string()),
    };
    migration_source(&up, &down)
}

fn factory_source(factory: &ClassName, base: &str, model_fqn: &str, uses: &[String], definition: &str) -> String {
    let ns = factory.namespace(base);
    let name = &factory.name;
    let mut uses: Vec<String> = uses.to_vec();
    uses.push("Illuminate\\Database\\Eloquent\\Factories\\Factory".into());
    uses.sort_by_key(|u| u.to_ascii_lowercase());
    uses.dedup();
    let uses = uses.iter().map(|u| format!("use {u};\n")).collect::<String>();
    format!(
        "<?php\n\nnamespace {ns};\n\n{uses}\n/**\n * @extends Factory<{model}>\n */\nclass {name} extends Factory\n{{\n    /**\n     * Define the model's default state.\n     *\n     * @return array<string, mixed>\n     */\n    public function definition(): array\n    {{\n        return [\n{definition}        ];\n    }}\n}}\n",
        model = php_index::short_name(model_fqn),
    )
}

fn seeder_file(seeder: &ClassName, base: &str) -> String {
    let ns = seeder.namespace(base);
    let name = &seeder.name;
    format!(
        "<?php\n\nnamespace {ns};\n\nuse Illuminate\\Database\\Seeder;\n\nclass {name} extends Seeder\n{{\n    /**\n     * Run the database seeds.\n     */\n    public function run(): void\n    {{\n        //\n    }}\n}}\n"
    )
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ControllerStyle {
    Plain,
    /// index/create/store/show/edit/update/destroy
    Resource,
    /// index/store/show/update/destroy
    Api,
}

fn controller_file(
    controller: &ClassName,
    namespace: &str,
    app_namespace: &str,
    model: Option<(&str, &str)>,
    style: ControllerStyle,
    requests: Option<(String, String)>,
) -> String {
    let ns = controller.namespace(namespace);
    let name = &controller.name;
    let mut uses = Vec::new();
    // Controllers in sub-namespaces still extend the base controller.
    if !controller.dirs.is_empty() {
        uses.push(format!("{app_namespace}\\Http\\Controllers\\Controller"));
    }
    let body = if style == ControllerStyle::Plain {
        "    //\n".to_string()
    } else {
        let (model_fqn, model_name) = model.unwrap_or(("", ""));
        let var = camel(model_name);
        let (store_req, update_req) = match requests {
            Some((store, update)) => {
                uses.push(format!("{app_namespace}\\Http\\Requests\\{store}"));
                uses.push(format!("{app_namespace}\\Http\\Requests\\{update}"));
                (store, update)
            }
            None => {
                uses.push("Illuminate\\Http\\Request".into());
                ("Request".to_string(), "Request".to_string())
            }
        };
        let bound = if model_name.is_empty() {
            "string $id".to_string()
        } else {
            uses.push(model_fqn.to_string());
            format!("{model_name} ${var}")
        };
        let method = |doc: &str, sig: &str| format!("    /**\n     * {doc}\n     */\n    public function {sig}\n    {{\n        //\n    }}\n");
        let mut methods = vec![method("Display a listing of the resource.", "index()")];
        if style == ControllerStyle::Resource {
            methods.push(method("Show the form for creating a new resource.", "create()"));
        }
        methods.push(method("Store a newly created resource in storage.", &format!("store({store_req} $request)")));
        methods.push(method("Display the specified resource.", &format!("show({bound})")));
        if style == ControllerStyle::Resource {
            methods.push(method("Show the form for editing the specified resource.", &format!("edit({bound})")));
        }
        methods.push(method("Update the specified resource in storage.", &format!("update({update_req} $request, {bound})")));
        methods.push(method("Remove the specified resource from storage.", &format!("destroy({bound})")));
        methods.join("\n")
    };
    uses.sort();
    uses.dedup();
    let uses = uses.iter().map(|u| format!("use {u};\n")).collect::<String>();
    let uses = if uses.is_empty() { String::new() } else { format!("{uses}\n") };
    format!("<?php\n\nnamespace {ns};\n\n{uses}class {name} extends Controller\n{{\n{body}}}\n")
}

fn form_request(request: &ClassName, namespace: &str) -> String {
    let ns = request.namespace(namespace);
    let name = &request.name;
    format!(
        "<?php\n\nnamespace {ns};\n\nuse Illuminate\\Foundation\\Http\\FormRequest;\n\nclass {name} extends FormRequest\n{{\n    /**\n     * Determine if the user is authorized to make this request.\n     */\n    public function authorize(): bool\n    {{\n        return true;\n    }}\n\n    /**\n     * Get the validation rules that apply to the request.\n     *\n     * @return array<string, \\Illuminate\\Contracts\\Validation\\ValidationRule|array<mixed>|string>\n     */\n    public function rules(): array\n    {{\n        return [\n            //\n        ];\n    }}\n}}\n"
    )
}

fn json_resource(resource: &ClassName, namespace: &str) -> String {
    let ns = resource.namespace(namespace);
    let name = &resource.name;
    format!(
        "<?php\n\nnamespace {ns};\n\nuse Illuminate\\Http\\Request;\nuse Illuminate\\Http\\Resources\\Json\\JsonResource;\n\nclass {name} extends JsonResource\n{{\n    /**\n     * Transform the resource into an array.\n     *\n     * @return array<string, mixed>\n     */\n    public function toArray(Request $request): array\n    {{\n        return parent::toArray($request);\n    }}\n}}\n"
    )
}

fn policy_file(policy: &ClassName, namespace: &str, app_namespace: &str, model_fqn: &str, model: &str) -> String {
    let ns = policy.namespace(namespace);
    let name = &policy.name;
    let var = camel(model);
    let user_fqn = format!("{app_namespace}\\Models\\User");
    let uses = if model == "User" {
        format!("use {user_fqn};\n")
    } else {
        let mut list = [model_fqn.to_string(), user_fqn];
        list.sort();
        list.iter().map(|u| format!("use {u};\n")).collect()
    };
    let method = |doc: &str, sig: &str| format!("    /**\n     * {doc}\n     */\n    public function {sig}: bool\n    {{\n        return false;\n    }}\n");
    let model_arg = if model == "User" { "User $model".to_string() } else { format!("{model} ${var}") };
    let body = [
        method("Determine whether the user can view any models.", "viewAny(User $user)"),
        method("Determine whether the user can view the model.", &format!("view(User $user, {model_arg})")),
        method("Determine whether the user can create models.", "create(User $user)"),
        method("Determine whether the user can update the model.", &format!("update(User $user, {model_arg})")),
        method("Determine whether the user can delete the model.", &format!("delete(User $user, {model_arg})")),
    ]
    .join("\n");
    format!("<?php\n\nnamespace {ns};\n\n{uses}\nclass {name}\n{{\n{body}}}\n")
}

fn observer_file(observer: &ClassName, namespace: &str, model_fqn: &str, model: &str) -> String {
    let ns = observer.namespace(namespace);
    let name = &observer.name;
    let var = camel(model);
    let body = ["created", "updated", "deleted", "restored", "forceDeleted"]
        .iter()
        .map(|event| {
            format!(
                "    /**\n     * Handle the {model} \"{event}\" event.\n     */\n    public function {event}({model} ${var}): void\n    {{\n        //\n    }}\n"
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!("<?php\n\nnamespace {ns};\n\nuse {model_fqn};\n\nclass {name}\n{{\n{body}}}\n")
}

fn event_file(event: &ClassName, namespace: &str) -> String {
    let ns = event.namespace(namespace);
    let name = &event.name;
    format!(
        "<?php\n\nnamespace {ns};\n\nuse Illuminate\\Broadcasting\\InteractsWithSockets;\nuse Illuminate\\Foundation\\Events\\Dispatchable;\nuse Illuminate\\Queue\\SerializesModels;\n\nclass {name}\n{{\n    use Dispatchable, InteractsWithSockets, SerializesModels;\n\n    /**\n     * Create a new event instance.\n     */\n    public function __construct()\n    {{\n        //\n    }}\n}}\n"
    )
}

fn listener_file(listener: &ClassName, namespace: &str) -> String {
    let ns = listener.namespace(namespace);
    let name = &listener.name;
    format!(
        "<?php\n\nnamespace {ns};\n\nclass {name}\n{{\n    /**\n     * Create the event listener.\n     */\n    public function __construct()\n    {{\n        //\n    }}\n\n    /**\n     * Handle the event.\n     */\n    public function handle(object $event): void\n    {{\n        //\n    }}\n}}\n"
    )
}

fn job_file(job: &ClassName, namespace: &str) -> String {
    let ns = job.namespace(namespace);
    let name = &job.name;
    format!(
        "<?php\n\nnamespace {ns};\n\nuse Illuminate\\Contracts\\Queue\\ShouldQueue;\nuse Illuminate\\Foundation\\Queue\\Queueable;\n\nclass {name} implements ShouldQueue\n{{\n    use Queueable;\n\n    /**\n     * Create a new job instance.\n     */\n    public function __construct()\n    {{\n        //\n    }}\n\n    /**\n     * Execute the job.\n     */\n    public function handle(): void\n    {{\n        //\n    }}\n}}\n"
    )
}

fn middleware_file(middleware: &ClassName, namespace: &str) -> String {
    let ns = middleware.namespace(namespace);
    let name = &middleware.name;
    format!(
        "<?php\n\nnamespace {ns};\n\nuse Closure;\nuse Illuminate\\Http\\Request;\nuse Symfony\\Component\\HttpFoundation\\Response;\n\nclass {name}\n{{\n    /**\n     * Handle an incoming request.\n     *\n     * @param  Closure(Request): (Response)  $next\n     */\n    public function handle(Request $request, Closure $next): Response\n    {{\n        return $next($request);\n    }}\n}}\n"
    )
}

fn command_file(command: &ClassName, namespace: &str) -> String {
    let ns = command.namespace(namespace);
    let name = &command.name;
    let signature = format!("app:{}", snake(name).replace('_', "-"));
    format!(
        "<?php\n\nnamespace {ns};\n\nuse Illuminate\\Console\\Command;\n\nclass {name} extends Command\n{{\n    /**\n     * The name and signature of the console command.\n     *\n     * @var string\n     */\n    protected $signature = '{signature}';\n\n    /**\n     * The console command description.\n     *\n     * @var string\n     */\n    protected $description = 'Command description';\n\n    /**\n     * Execute the console command.\n     */\n    public function handle()\n    {{\n        //\n    }}\n}}\n"
    )
}

fn enum_file(class: &ClassName, namespace: &str) -> String {
    let ns = class.namespace(namespace);
    let name = &class.name;
    format!("<?php\n\nnamespace {ns};\n\nenum {name}: string\n{{\n    //\n}}\n")
}

fn phpunit_test(test: &ClassName, base: &str, suite: &str) -> String {
    let ns = test.namespace(&format!("{base}\\{suite}"));
    let name = &test.name;
    let (uses, method) = if suite == "Unit" {
        ("use PHPUnit\\Framework\\TestCase;\n", "    public function test_that_true_is_true(): void\n    {\n        $this->assertTrue(true);\n    }\n")
    } else {
        ("use Tests\\TestCase;\n", "    public function test_the_application_returns_a_successful_response(): void\n    {\n        $response = $this->get('/');\n\n        $response->assertStatus(200);\n    }\n")
    };
    format!("<?php\n\nnamespace {ns};\n\n{uses}\nclass {name} extends TestCase\n{{\n{method}}}\n")
}

fn pest_test(suite: &str) -> String {
    if suite == "Unit" {
        "<?php\n\ntest('example', function () {\n    expect(true)->toBeTrue();\n});\n".into()
    } else {
        "<?php\n\ntest('example', function () {\n    $response = $this->get('/');\n\n    $response->assertStatus(200);\n});\n".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn naming() {
        assert_eq!(table_name("Post"), "posts");
        assert_eq!(table_name("BlogCategory"), "blog_categories");
        assert_eq!(table_name("Address"), "addresses");
        assert_eq!(table_name("Person"), "people");
        assert_eq!(snake("CreatePostsTable"), "create_posts_table");
        assert_eq!(studly("user-profile"), "UserProfile");
        let ts = migration_timestamp(0);
        assert_eq!(ts.len(), 17, "{ts}");
    }

    #[test]
    fn model_with_everything() {
        let root = std::env::temp_dir().join(format!("phpcraft-make-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("composer.json"), r#"{"autoload":{"psr-4":{"App\\":"app/"}}}"#).unwrap();
        let maker = Maker::new(&root);
        let files = maker.plan(MakeKind::Model, "Blog/Post", &maker.extras(MakeKind::Model), None);
        let rel: Vec<String> = files
            .iter()
            .map(|f| f.path.strip_prefix(&root).unwrap().display().to_string())
            .map(|p| if p.starts_with("database/migrations/") { "database/migrations/*_create_posts_table.php".into() } else { p })
            .collect();
        assert_eq!(
            rel,
            [
                "app/Models/Blog/Post.php",
                "database/migrations/*_create_posts_table.php",
                "database/factories/Blog/PostFactory.php",
                "app/Http/Resources/Blog/PostResource.php",
                "database/seeders/Blog/PostSeeder.php",
                "app/Http/Requests/Blog/StorePostRequest.php",
                "app/Http/Requests/Blog/UpdatePostRequest.php",
                "app/Http/Controllers/Blog/PostController.php",
                "app/Policies/Blog/PostPolicy.php",
                "app/Observers/Blog/PostObserver.php",
            ]
        );
        let model = &files[0].contents;
        assert!(model.contains("namespace App\\Models\\Blog;") && model.contains("use HasFactory;"));
        let controller = &files[7].contents;
        assert!(!controller.contains("function create()"), "REST-API controller has no create/edit");
        assert!(controller.contains("use App\\Http\\Controllers\\Controller;"));
        assert!(controller.contains("public function update(UpdatePostRequest $request, Post $post)"));
        assert!(controller.contains("use App\\Models\\Blog\\Post;"));

        // Every generated PHP file parses.
        for file in &files {
            let tree = php_syntax_check(&file.contents);
            assert!(tree, "{} does not parse:\n{}", file.path.display(), file.contents);
        }
        let written = write(&files).unwrap();
        assert_eq!(written.len(), files.len());
        assert!(maker.plan(MakeKind::Model, "Blog/Post", &[], None).iter().all(|f| f.exists));

        // Spec-driven model: UUID key, soft deletes, columns, relations and the pivot migration.
        let spec = ModelSpec {
            id: IdType::Uuid,
            soft_deletes: true,
            fields: parse_fields("title:string, body:text:nullable, published:bool:default(false)"),
            relations: parse_relations("belongsTo:User, hasMany:Comment, belongsToMany:Tag, morphMany:Image:imageable"),
            ..ModelSpec::default()
        };
        let files = maker.plan_with(MakeKind::Model, "Article", &[Extra::Migration, Extra::Factory], None, &spec);
        assert_eq!(files.len(), 4);
        let model = &files[0].contents;
        assert!(model.contains("use HasFactory, HasUuids, SoftDeletes;"), "{model}");
        assert!(model.contains("public function tags(): BelongsToMany"));
        assert!(model.contains("return $this->morphMany(Image::class, 'imageable');"));
        assert!(model.contains("'user_id',") && model.contains("'published' => 'boolean'"));
        let migration = &files[1].contents;
        assert!(migration.contains("$table->uuid('id')->primary();"));
        assert!(migration.contains("$table->foreignId('user_id')->constrained()->cascadeOnDelete();"));
        assert!(migration.contains("$table->boolean('published')->default(false);"));
        assert!(migration.contains("$table->softDeletes();"));
        assert!(files[2].path.to_string_lossy().ends_with("_create_article_tag_table.php"));
        assert!(files[2].path > files[1].path, "pivot must sort after its table");
        assert!(files[2].contents.contains("$table->foreignUuid('article_id')"));
        assert!(files[3].contents.contains("'user_id' => User::factory(),") && files[3].contents.contains("use App\\Models\\User;"));
        for file in &files {
            assert!(php_syntax_check(&file.contents), "{}", file.contents);
        }
        let pivot = maker.plan(MakeKind::PivotTable, "Tag, Post", &[], None);
        assert!(pivot[0].contents.contains("$table->primary(['post_id', 'tag_id']);"));
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn modules_and_filament() {
        let root = std::env::temp_dir().join(format!("phpcraft-make-mod-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("Modules/Blog")).unwrap();
        fs::write(root.join("Modules/Blog/module.json"), "{}").unwrap();
        fs::create_dir_all(root.join("Modules/NotAModule")).unwrap();
        fs::write(root.join("composer.json"), r#"{"require":{"filament/filament":"^4.0"}}"#).unwrap();
        let maker = Maker::new(&root);
        assert_eq!(maker.modules(), ["Blog"]);
        assert!(maker.kinds().contains(&MakeKind::FilamentResource));

        let files = maker.plan(MakeKind::Model, "Post", &[Extra::Factory, Extra::Controller], Some("Blog"));
        let rel = |f: &PlannedFile| f.path.strip_prefix(&root).unwrap().display().to_string();
        assert_eq!(rel(&files[0]), "Modules/Blog/app/Models/Post.php");
        assert!(files[0].contents.contains("namespace Modules\\Blog\\Models;"));
        assert_eq!(rel(&files[1]), "Modules/Blog/database/factories/PostFactory.php");
        assert!(files[1].contents.contains("namespace Modules\\Blog\\Database\\Factories;"));
        assert!(files[1].contents.contains("use Modules\\Blog\\Models\\Post;"));

        // Names typed in the dialog override the derived ones; Filament option appears when installed.
        assert!(maker.extras(MakeKind::Model).contains(&Extra::FilamentResource));
        let mut spec = ModelSpec { fillable: false, table: Some("blog_posts".into()), ..ModelSpec::default() };
        spec.class_names.insert(Extra::Controller, "Api/PostApiController".into());
        spec.class_names.insert(Extra::Requests, "SavePostRequest".into());
        let files = maker.plan_with(MakeKind::Model, "Post", &[Extra::Controller, Extra::Requests, Extra::Migration], None, &spec);
        let paths: Vec<_> = files.iter().map(rel).map(|p| if p.contains("migrations") { p.split('_').skip(4).collect::<Vec<_>>().join("_") } else { p }).collect();
        assert_eq!(paths, ["app/Models/Post.php", "create_blog_posts_table.php", "app/Http/Requests/SavePostRequest.php", "app/Http/Controllers/Api/PostApiController.php"]);
        assert!(files[0].contents.contains("protected $table = 'blog_posts';") && !files[0].contents.contains("$fillable"));

        let files = maker.plan(MakeKind::FilamentResource, "BlogCategory", &[], None);
        let paths: Vec<_> = files.iter().map(rel).collect();
        assert_eq!(
            paths,
            [
                "app/Filament/Resources/BlogCategories/BlogCategoryResource.php",
                "app/Filament/Resources/BlogCategories/Schemas/BlogCategoryForm.php",
                "app/Filament/Resources/BlogCategories/Tables/BlogCategoriesTable.php",
                "app/Filament/Resources/BlogCategories/Pages/ListBlogCategories.php",
                "app/Filament/Resources/BlogCategories/Pages/CreateBlogCategory.php",
                "app/Filament/Resources/BlogCategories/Pages/EditBlogCategory.php",
            ]
        );
        let mut all = files;
        all.extend(maker.plan(MakeKind::FilamentPage, "Settings", &[], None));
        all.extend(maker.plan(MakeKind::FilamentWidget, "StatsOverview", &[], None));
        fs::write(root.join("composer.lock"), r#"{"packages":[{"name": "filament/filament", "version": "v3.2.0"}]}"#).unwrap();
        let v3 = Maker::new(&root).plan(MakeKind::FilamentResource, "Post", &[], None);
        assert!(v3[0].contents.contains("use App\\Filament\\Resources\\PostResource\\Pages;"), "{}", v3[0].contents);
        assert!(v3[0].contents.contains("'index' => Pages\\ListPosts::route('/'),"));
        all.extend(v3);
        for file in all.iter().filter(|f| !f.path.to_string_lossy().ends_with(".blade.php")) {
            assert!(php_syntax_check(&file.contents), "{} does not parse:\n{}", file.path.display(), file.contents);
        }
        fs::remove_dir_all(&root).unwrap();
    }

    fn php_syntax_check(source: &str) -> bool {
        php_analysis::Document::parse(Path::new("/x.php"), source)
            .is_some_and(|doc| !doc.tree.root_node().has_error())
    }
}
