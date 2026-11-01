//! Project workspace: root folder and a lazily expanded file tree.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;

/// Directories hidden from the tree, like PhpStorm's default excludes.
const HIDDEN_DIRS: &[&str] = &[".git", ".idea", ".phpcraft", "node_modules", "target"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
}

/// One row of the flattened, visible tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub entry: Entry,
    pub depth: usize,
    pub expanded: bool,
}

pub struct Project {
    root: PathBuf,
    expanded: HashSet<PathBuf>,
}

impl Project {
    pub fn open(root: impl Into<PathBuf>) -> Result<Self> {
        let root = fs::canonicalize(root.into())?;
        let mut expanded = HashSet::new();
        expanded.insert(root.clone());
        Ok(Self { root, expanded })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn name(&self) -> String {
        file_name(&self.root)
    }

    pub fn is_expanded(&self, path: &Path) -> bool {
        self.expanded.contains(path)
    }

    pub fn toggle(&mut self, path: &Path) {
        if !self.expanded.remove(path) {
            self.expanded.insert(path.to_path_buf());
        }
    }

    /// Flattens the expanded part of the tree into display rows.
    pub fn rows(&self) -> Vec<Row> {
        let mut rows = vec![Row {
            entry: Entry {
                path: self.root.clone(),
                name: self.name(),
                is_dir: true,
            },
            depth: 0,
            expanded: self.is_expanded(&self.root),
        }];
        if self.is_expanded(&self.root) {
            self.push_children(&self.root, 1, &mut rows);
        }
        rows
    }

    fn push_children(&self, dir: &Path, depth: usize, rows: &mut Vec<Row>) {
        for entry in read_dir_sorted(dir) {
            let expanded = entry.is_dir && self.is_expanded(&entry.path);
            let path = entry.path.clone();
            rows.push(Row {
                entry,
                depth,
                expanded,
            });
            if expanded {
                self.push_children(&path, depth + 1, rows);
            }
        }
    }
}

/// Directory entries with folders first, each group sorted case-insensitively.
pub fn read_dir_sorted(dir: &Path) -> Vec<Entry> {
    let Ok(read) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut entries: Vec<Entry> = read
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let path = e.path();
            let name = file_name(&path);
            let is_dir = e.file_type().ok()?.is_dir();
            if is_dir && HIDDEN_DIRS.contains(&name.as_str()) || name == ".DS_Store" {
                return None;
            }
            Some(Entry { path, name, is_dir })
        })
        .collect();
    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    entries
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_list_dirs_first_and_expand() {
        let tmp = std::env::temp_dir().join(format!("phpcraft-project-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("app/Models")).unwrap();
        fs::create_dir_all(tmp.join(".git")).unwrap();
        fs::write(tmp.join("artisan"), "").unwrap();
        fs::write(tmp.join("app/Models/User.php"), "").unwrap();

        let mut project = Project::open(&tmp).unwrap();
        let names: Vec<_> = project.rows().iter().map(|r| r.entry.name.clone()).collect();
        assert_eq!(names[1..], ["app", "artisan"]);

        let app = project.root().join("app");
        project.toggle(&app);
        project.toggle(&app.join("Models"));
        let names: Vec<_> = project.rows().iter().map(|r| (r.entry.name.clone(), r.depth)).collect();
        assert_eq!(
            names[1..],
            [("app".into(), 1), ("Models".into(), 2), ("User.php".into(), 3), ("artisan".into(), 1)]
        );
        fs::remove_dir_all(&tmp).unwrap();
    }
}
