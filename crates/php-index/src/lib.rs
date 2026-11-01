//! Project-wide index of PHP declarations (the PhpCraft analogue of PhpStorm's stub index).

mod cache;
pub mod extract;
pub mod names;

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub use extract::{
    ClassInfo, ClassKind, FileSymbols, FunctionInfo, Member, MemberKind, class_type, doc_comment,
    doc_tag_type, extract, is_class_like, text,
};
pub use names::{NameContext, UseItem, UseKind, is_builtin_class, join, namespace_of, short_name};

/// Directories never indexed.
const SKIP_DIRS: &[&str] = &[".git", ".idea", "node_modules", "storage", "target", ".phpcraft"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Location {
    pub path: PathBuf,
    pub range: std::ops::Range<usize>,
}

#[derive(Default)]
pub struct Index {
    classes: HashMap<String, Arc<ClassInfo>>,
    functions: HashMap<String, Arc<FunctionInfo>>,
    by_short_name: HashMap<String, Vec<String>>,
    by_file: HashMap<PathBuf, (Vec<String>, Vec<String>)>,
    /// Framework-provided mixins (e.g. Eloquent models forward to the query builder).
    extra_mixins: HashMap<String, Vec<String>>,
    /// Classes as parsed, before `augment` added members.
    unaugmented: HashMap<String, Arc<ClassInfo>>,
    /// Classes from generated IDE helper files (Laravel Idea's `vendor/_laravel_idea`,
    /// barryvdh's `_ide_helper*.php`). They only contribute phpdoc members: navigation goes
    /// to the real class when there is one.
    stubs: HashMap<String, Arc<ClassInfo>>,
}

/// A generated helper file that redeclares real classes for IDEs.
pub fn is_ide_helper(path: &Path) -> bool {
    path.components().any(|c| c.as_os_str() == "_laravel_idea")
        || path.file_name().is_some_and(|n| n.to_string_lossy().starts_with("_ide_helper"))
}

fn key(fqn: &str) -> String {
    fqn.trim_start_matches('\\').to_ascii_lowercase()
}

impl Index {
    pub fn new() -> Self {
        Self::default()
    }

    /// Parses every PHP file under `root` (including `vendor/`) using all CPU cores: folders
    /// are walked in parallel and each file is parsed as soon as it is found.
    pub fn scan(root: &Path) -> Self {
        Self::scan_cached(root, None)
    }

    /// Like `scan`, reusing `cache_file` for files whose modification time and size haven't
    /// changed, and rewriting it when anything did.
    pub fn scan_cached(root: &Path, cache_file: Option<&Path>) -> Self {
        let cached = cache_file.map(cache::load).unwrap_or_default();
        let reused = std::sync::atomic::AtomicUsize::new(0);
        let found = std::sync::Mutex::new(Vec::new());
        let walk = Walk { cached: &cached, reused: &reused, found: &found };
        rayon::scope(|scope| scan_dir(root.to_path_buf(), walk, scope));
        let mut results: Vec<(PathBuf, cache::Entry)> = found.into_inner().unwrap_or_default();
        // A stable order, so duplicate class names always resolve the same way.
        results.sort_unstable_by(|a, b| a.0.cmp(&b.0));
        let reused = reused.into_inner();
        if let Some(file) = cache_file
            && (reused != results.len() || cached.len() != results.len())
        {
            let bytes = cache::encode(&results);
            let file = file.to_path_buf();
            std::thread::spawn(move || cache::write(&file, &bytes));
        }
        let mut index = Self::new();
        for (_, entry) in results {
            index.insert(FileSymbols { classes: entry.classes, functions: entry.functions, ..Default::default() });
        }
        index
    }

    /// Re-indexes one file from (possibly unsaved) source.
    pub fn update_file(&mut self, path: &Path, source: &str) {
        self.remove_file(path);
        if let Some(tree) = php_syntax::parse(source) {
            self.insert(extract(path, source, &tree));
        }
    }

    pub fn remove_file(&mut self, path: &Path) {
        let Some((classes, functions)) = self.by_file.remove(path) else { return };
        for k in classes {
            let mut removed = None;
            if self.classes.get(&k).is_some_and(|c| &*c.path == path) {
                removed = self.classes.remove(&k);
            }
            if self.stubs.get(&k).is_some_and(|c| &*c.path == path) {
                removed = self.stubs.remove(&k).or(removed);
            }
            if let Some(class) = removed
                && !self.classes.contains_key(&k)
                && !self.stubs.contains_key(&k)
                && let Some(list) = self.by_short_name.get_mut(&class.name.to_ascii_lowercase())
            {
                list.retain(|fqn| key(fqn) != k);
            }
        }
        for k in functions {
            self.functions.remove(&k);
        }
    }

    fn insert(&mut self, symbols: FileSymbols) {
        let path = match (symbols.classes.first(), symbols.functions.first()) {
            (Some(c), _) => c.path.to_path_buf(),
            (None, Some(f)) => f.path.to_path_buf(),
            _ => return,
        };
        let helper = is_ide_helper(&path);
        let entry = self.by_file.entry(path).or_default();
        for class in symbols.classes {
            let k = key(&class.fqn);
            entry.0.push(k.clone());
            let names = self.by_short_name.entry(class.name.to_ascii_lowercase()).or_default();
            if !names.iter().any(|n| key(n) == k) {
                names.push(class.fqn.clone());
            }
            if helper {
                self.stubs.insert(k, Arc::new(class));
            } else {
                self.unaugmented.remove(&k);
                self.classes.insert(k, Arc::new(class));
            }
        }
        for function in symbols.functions {
            let k = key(&function.fqn);
            entry.1.push(k.clone());
            self.functions.insert(k, Arc::new(function));
        }
    }

    /// Adds members (e.g. model columns and relations from Generate Helper Code) to an
    /// indexed class. Replaces members previously added this way; re-apply after reindexing.
    pub fn augment(&mut self, fqn: &str, members: Vec<Member>) {
        let k = key(fqn);
        let Some(current) = self.classes.get(&k).cloned() else { return };
        let base = self.unaugmented.entry(k.clone()).or_insert(current);
        let mut class = (**base).clone();
        class.members.extend(members);
        self.classes.insert(k, Arc::new(class));
    }

    /// Makes members of `mixin` reachable from `class` (and its subclasses).
    pub fn add_mixin(&mut self, class: &str, mixin: &str) {
        self.extra_mixins.entry(key(class)).or_default().push(mixin.to_string());
    }

    pub fn class_count(&self) -> usize {
        self.classes.len()
    }

    pub fn class(&self, fqn: &str) -> Option<&Arc<ClassInfo>> {
        let k = key(fqn);
        self.classes.get(&k).or_else(|| self.stubs.get(&k))
    }

    pub fn function(&self, fqn: &str) -> Option<&Arc<FunctionInfo>> {
        self.functions.get(&key(fqn))
    }

    pub fn classes_by_short_name(&self, name: &str) -> Vec<&Arc<ClassInfo>> {
        self.by_short_name
            .get(&name.to_ascii_lowercase())
            .map(|list| list.iter().filter_map(|fqn| self.class(fqn)).collect())
            .unwrap_or_default()
    }

    pub fn classes(&self) -> impl Iterator<Item = &Arc<ClassInfo>> {
        self.classes.values().chain(self.stubs.iter().filter(|(k, _)| !self.classes.contains_key(*k)).map(|(_, c)| c))
    }

    pub fn functions(&self) -> impl Iterator<Item = &Arc<FunctionInfo>> {
        self.functions.values()
    }

    /// Classes reachable for member lookup, nearest first: the class, its traits,
    /// parents, interfaces and `@mixin`s.
    pub fn ancestors(&self, fqn: &str) -> Vec<Arc<ClassInfo>> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        let mut queue = vec![fqn.to_string()];
        while let Some(next) = queue.pop() {
            if !seen.insert(key(&next)) {
                continue;
            }
            let Some(class) = self.class(&next) else { continue };
            out.push(class.clone());
            // Members the IDE helper declares (columns, relations, scopes) come right after
            // the real class's own.
            if let Some(stub) = self.stubs.get(&key(&next)).filter(|s| !Arc::ptr_eq(s, class)) {
                out.push(stub.clone());
                queue.extend(stub.mixins.iter().cloned());
            }
            // Stack order: traits pop first, then parents, interfaces and mixins.
            let next_names = class
                .mixins
                .iter()
                .chain(&class.implements)
                .chain(&class.extends)
                .chain(class.traits.iter().rev());
            queue.extend(next_names.cloned());
            if let Some(extra) = self.extra_mixins.get(&key(&class.fqn)) {
                queue.splice(0..0, extra.iter().cloned());
            }
        }
        out
    }

    /// Finds a member by name, walking the inheritance chain. Real declarations win over
    /// phpdoc-declared ones so navigation lands on code when it exists.
    pub fn find_member(&self, class_fqn: &str, name: &str, kinds: &[MemberKind]) -> Option<(Arc<ClassInfo>, Member)> {
        let matches = |m: &Member| {
            kinds.contains(&m.kind)
                && if m.kind == MemberKind::Method {
                    m.name.eq_ignore_ascii_case(name)
                } else {
                    m.name == name
                }
        };
        let ancestors = self.ancestors(class_fqn);
        let mut documented: Option<(Arc<ClassInfo>, Member)> = None;
        for class in &ancestors {
            for member in class.members.iter().filter(|m| matches(m)) {
                if !member.virtual_member {
                    return Some((class.clone(), member.clone()));
                }
                documented.get_or_insert((class.clone(), member.clone()));
            }
        }
        // Facades document proxied methods with `@method` and name the target with `@see`:
        // navigate to the real implementation, keeping the documented return type.
        if let Some((owner, doc_member)) = &documented {
            for target in &owner.see {
                if let Some((real_owner, mut real)) = self
                    .ancestors(target)
                    .iter()
                    .find_map(|c| c.members.iter().find(|m| matches(m) && !m.virtual_member).map(|m| (c.clone(), m.clone())))
                {
                    real.type_fqn = doc_member.type_fqn.clone().or(real.type_fqn);
                    real.is_static = doc_member.is_static;
                    return Some((real_owner, real));
                }
            }
        }
        documented
    }

    /// All members visible on a class, deduplicated by name (nearest declaration wins).
    pub fn members(&self, class_fqn: &str) -> Vec<(Arc<ClassInfo>, Member)> {
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for class in self.ancestors(class_fqn) {
            for member in &class.members {
                if seen.insert((member.kind as u8, member.name.to_ascii_lowercase())) {
                    out.push((class.clone(), member.clone()));
                }
            }
        }
        out
    }

    pub fn class_location(&self, fqn: &str) -> Option<Location> {
        self.class(fqn).map(|c| Location {
            path: c.path.to_path_buf(),
            range: c.range.clone(),
        })
    }
}

fn is_php_source(path: &Path) -> bool {
    path.extension().is_some_and(|e| e == "php") && !path.to_string_lossy().ends_with(".blade.php")
}

/// Shared state of a parallel scan.
#[derive(Clone, Copy)]
struct Walk<'s> {
    cached: &'s HashMap<PathBuf, cache::Entry>,
    reused: &'s std::sync::atomic::AtomicUsize,
    found: &'s std::sync::Mutex<Vec<(PathBuf, cache::Entry)>>,
}

/// Walks `dir`, indexing its PHP files (from the cache when unchanged) and spawning a task
/// per subfolder.
fn scan_dir<'s>(dir: PathBuf, walk: Walk<'s>, scope: &rayon::Scope<'s>) {
    let Ok(entries) = fs::read_dir(&dir) else { return };
    let mut local = Vec::new();
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else { continue };
        let path = entry.path();
        if file_type.is_dir() {
            if !SKIP_DIRS.contains(&entry.file_name().to_string_lossy().as_ref()) {
                scope.spawn(move |scope| scan_dir(path, walk, scope));
            }
            continue;
        }
        if !is_php_source(&path) {
            continue;
        }
        let Ok(metadata) = entry.metadata() else { continue };
        let stamp = cache::stamp(&metadata);
        if let Some(hit) = walk.cached.get(&path).filter(|e| e.stamp == stamp) {
            walk.reused.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            local.push((path, cache::Entry { stamp, classes: hit.classes.clone(), functions: hit.functions.clone() }));
            continue;
        }
        let Ok(source) = fs::read_to_string(&path) else { continue };
        let symbols = php_syntax::parse(&source).map(|tree| extract(&path, &source, &tree)).unwrap_or_default();
        local.push((path, cache::Entry { stamp, classes: symbols.classes, functions: symbols.functions }));
    }
    if !local.is_empty() {
        walk.found.lock().unwrap().extend(local);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index_of(files: &[(&str, &str)]) -> Index {
        let mut index = Index::new();
        for (path, source) in files {
            index.update_file(Path::new(path), source);
        }
        index
    }

    #[test]
    fn ide_helper_classes_never_shadow_real_ones() {
        let mut index = Index::new();
        let helper = Path::new("/p/vendor/_laravel_idea/_ide_helper_models_App_Models.php");
        let helper_src = "<?php\nnamespace App\\Models;\n/** @property string $title */\nclass Post {}\n";
        let real = Path::new("/p/app/Models/Post.php");
        let real_src = "<?php\nnamespace App\\Models;\nclass Post { public function publish() {} }\n";
        // Either order of scanning gives the same answer.
        for order in [[(helper, helper_src), (real, real_src)], [(real, real_src), (helper, helper_src)]] {
            let mut index2 = Index::new();
            for (path, src) in order {
                index2.update_file(path, src);
            }
            assert_eq!(index2.class_location("App\\Models\\Post").unwrap().path, real);
            assert!(index2.find_member("App\\Models\\Post", "title", &[MemberKind::Property]).is_some(), "helper members still merge");
            assert!(index2.find_member("App\\Models\\Post", "publish", &[MemberKind::Method]).is_some());
            assert_eq!(index2.classes().filter(|c| c.name == "Post").count(), 1);
        }
        // A helper-only class is still known.
        index.update_file(helper, helper_src);
        assert!(index.class("App\\Models\\Post").is_some());
    }

    #[test]
    fn indexes_classes_members_and_inheritance() {
        let index = index_of(&[
            (
                "/p/Model.php",
                "<?php\nnamespace Illuminate\\Database\\Eloquent;\n/** @mixin Builder */\nabstract class Model { public function save(): bool {} public static function query(): Builder {} }",
            ),
            (
                "/p/Builder.php",
                "<?php\nnamespace Illuminate\\Database\\Eloquent;\nclass Builder { /** @return static */ public function where($c) {} }",
            ),
            (
                "/p/User.php",
                "<?php\nnamespace App\\Models;\nuse Illuminate\\Database\\Eloquent\\Model;\n/**\n * @property string $email\n * @method static User|null find(int $id)\n */\nclass User extends Model { use HasPosts; protected $fillable = []; public function __construct(private Mailer $mailer) {} }",
            ),
        ]);
        let user = index.class("App\\Models\\User").unwrap();
        assert_eq!(user.extends, ["Illuminate\\Database\\Eloquent\\Model"]);
        assert_eq!(user.traits, ["App\\Models\\HasPosts"]);
        assert!(index.find_member("App\\Models\\User", "save", &[MemberKind::Method]).is_some());
        let (owner, _) = index.find_member("App\\Models\\User", "where", &[MemberKind::Method]).unwrap();
        assert_eq!(owner.name, "Builder");
        let (_, find) = index.find_member("App\\Models\\User", "find", &[MemberKind::Method]).unwrap();
        assert!(find.is_static && find.virtual_member);
        assert_eq!(find.type_fqn.as_deref(), Some("App\\Models\\User"));
        let (_, mailer) = index.find_member("App\\Models\\User", "mailer", &[MemberKind::Property]).unwrap();
        assert_eq!(mailer.type_fqn.as_deref(), Some("App\\Models\\Mailer"));
        assert_eq!(index.classes_by_short_name("user").len(), 1);

        let mut index = index;
        index.remove_file(Path::new("/p/User.php"));
        assert!(index.class("App\\Models\\User").is_none());
        assert!(index.classes_by_short_name("User").is_empty());
    }

    #[test]
    fn parses_grouped_and_function_uses() {
        let src = "<?php\nnamespace A;\nuse App\\Models\\{User, Post as P};\nuse function Foo\\helper;\n";
        let tree = php_syntax::parse(src).unwrap();
        let symbols = extract(Path::new("/x.php"), src, &tree);
        let uses: Vec<_> = symbols.uses.iter().map(|u| (u.fqn.as_str(), u.alias.as_str(), u.kind)).collect();
        assert_eq!(
            uses,
            [
                ("App\\Models\\User", "User", UseKind::Class),
                ("App\\Models\\Post", "P", UseKind::Class),
                ("Foo\\helper", "helper", UseKind::Function),
            ]
        );
    }
}
