//! Project-wide index of PHP declarations (the PhpCraft analogue of PhpStorm's stub index).

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
}

fn key(fqn: &str) -> String {
    fqn.trim_start_matches('\\').to_ascii_lowercase()
}

impl Index {
    pub fn new() -> Self {
        Self::default()
    }

    /// Parses every PHP file under `root` (including `vendor/`) using all CPU cores.
    pub fn scan(root: &Path) -> Self {
        let mut files = Vec::new();
        collect_php_files(root, &mut files);
        let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
        let chunk = files.len().div_ceil(threads).max(1);
        let results: Vec<FileSymbols> = std::thread::scope(|scope| {
            let handles: Vec<_> = files
                .chunks(chunk)
                .map(|paths| {
                    scope.spawn(move || {
                        paths
                            .iter()
                            .filter_map(|path| {
                                let source = fs::read_to_string(path).ok()?;
                                let tree = php_syntax::parse(&source)?;
                                Some(extract(path, &source, &tree))
                            })
                            .collect::<Vec<_>>()
                    })
                })
                .collect();
            handles.into_iter().flat_map(|h| h.join().unwrap_or_default()).collect()
        });
        let mut index = Self::new();
        for symbols in results {
            index.insert(symbols);
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
            if let Some(class) = self.classes.remove(&k)
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
        let entry = self.by_file.entry(path).or_default();
        for class in symbols.classes {
            let k = key(&class.fqn);
            entry.0.push(k.clone());
            self.by_short_name
                .entry(class.name.to_ascii_lowercase())
                .or_default()
                .push(class.fqn.clone());
            self.unaugmented.remove(&k);
            self.classes.insert(k, Arc::new(class));
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
        self.classes.get(&key(fqn))
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
        self.classes.values()
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

fn collect_php_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else { continue };
        if file_type.is_dir() {
            let name = entry.file_name();
            if !SKIP_DIRS.contains(&name.to_string_lossy().as_ref()) {
                collect_php_files(&path, out);
            }
        } else if path.extension().is_some_and(|e| e == "php")
            && !path.to_string_lossy().ends_with(".blade.php")
        {
            out.push(path);
        }
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
