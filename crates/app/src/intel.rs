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
    laravel: Option<Laravel>,
    ready: AtomicBool,
}

impl Intel {
    pub fn new(root: &Path) -> Arc<Self> {
        Arc::new(Self {
            root: root.to_path_buf(),
            index: RwLock::new(Index::new()),
            laravel: laravel::is_laravel_project(root).then(|| Laravel::new(root)),
            ready: AtomicBool::new(false),
        })
    }

    /// Builds the index; runs on a background thread.
    pub fn scan(&self) {
        let started = Instant::now();
        let mut index = Index::scan(&self.root);
        if self.laravel.is_some() {
            laravel::register_mixins(&mut index);
        }
        eprintln!("indexed {} classes in {:.1?}", index.class_count(), started.elapsed());
        *self.index.write().unwrap() = index;
        self.ready.store(true, Ordering::Release);
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn is_ready(&self) -> bool {
        self.ready.load(Ordering::Acquire)
    }

    pub fn is_laravel(&self) -> bool {
        self.laravel.is_some()
    }

    pub fn class_count(&self) -> usize {
        self.index.read().unwrap().class_count()
    }

    fn providers(&self) -> Vec<&dyn StringProvider> {
        self.laravel.iter().map(|l| l as &dyn StringProvider).collect()
    }

    pub fn update_file(&self, path: &Path, source: &str) {
        if path.extension().is_some_and(|e| e == "php") {
            self.index.write().unwrap().update_file(path, source);
        }
    }

    pub fn goto(&self, path: &Path, source: &str, offset: usize) -> Vec<Location> {
        let index = self.index.read().unwrap();
        let found = Document::parse(path, source)
            .map(|doc| php_analysis::goto_definition(&doc, &index, offset, &self.providers()))
            .unwrap_or_default();
        if !found.is_empty() {
            return found;
        }
        // Blade templates aren't PHP code: fall back to text-based Laravel navigation.
        self.laravel
            .as_ref()
            .map(|l| l.text_goto(source, offset))
            .unwrap_or_default()
    }

    /// Byte range to underline on ⌘+hover, if the symbol under `offset` navigates somewhere.
    pub fn link_at(&self, path: &Path, source: &str, offset: usize) -> Option<std::ops::Range<usize>> {
        let doc = Document::parse(path, source)?;
        let range = php_analysis::reference_range(&doc, offset).or_else(|| word_range(source, offset))?;
        (!self.goto(path, source, offset).is_empty()).then_some(range)
    }

    pub fn complete(&self, path: &Path, source: &str, offset: usize) -> Vec<CompletionItem> {
        let index = self.index.read().unwrap();
        Document::parse(path, source)
            .map(|doc| php_analysis::complete(&doc, &index, offset, &self.providers()))
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
    let end = source[offset..].find(|c| !is_word(c)).map_or(source.len(), |i| offset + i);
    (start < end).then_some(start..end)
}
