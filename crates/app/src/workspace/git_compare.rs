//! Git ▸ Compare with Branch / Revision… and Compare with Clipboard for the active file, and
//! merge conflicts: Resolve Conflicts… lists the conflicted files, Merge Conflicts… (⌥↵ in a
//! conflicted file, or the editor's context menu) opens the three-pane merge window, and Mark File as Resolved stages one.

use std::path::PathBuf;
use std::sync::Arc;

use gpui::{AppContext, Context, Window, actions};

use super::Workspace;
use crate::diff_view::DiffSource;
use crate::dialogs::{FileOp, InputDialog, NavItem, NavMode, NavTarget};
use crate::i18n::{tr, trf};

actions!(workspace, [CompareWithRevision, CompareWithClipboard, ResolveConflicts, MarkResolved]);

impl Workspace {
    fn active_file_and_text(&self, cx: &gpui::App) -> Option<(PathBuf, String, String)> {
        let editor = self.tabs.get(self.active)?.read(cx);
        let path = editor.buffer.path()?.to_path_buf();
        let selected = editor.buffer.selected_text();
        Some((path, editor.buffer.text(), selected))
    }

    pub(super) fn compare_with_revision(&mut self, _: &CompareWithRevision, window: &mut Window, cx: &mut Context<Self>) {
        let Some((path, ..)) = self.active_file_and_text(cx) else { return };
        let dialog = cx.new(|cx| InputDialog::new(FileOp::CompareRevision(path), window, cx));
        self.show_modal(dialog, window, cx);
    }

    /// Opens the diff of the active file against `revision` (a branch, tag or commit).
    pub(super) fn compare_file_with(&mut self, path: &std::path::Path, revision: &str, window: &mut Window, cx: &mut Context<Self>) -> Result<Option<PathBuf>, String> {
        let repo = self.intel.repo().cloned().ok_or_else(|| tr("Not a Git repository").to_string())?;
        let rel = repo.relative(path).ok_or_else(|| tr("The file is outside the repository").to_string())?;
        let revision = revision.trim().to_string();
        if revision.is_empty() || revision.starts_with('-') {
            return Err(tr("Enter a branch, tag or commit").to_string());
        }
        if repo.show(&revision, &rel).is_none() {
            return Err(trf("{} doesn't exist in {}", &[&rel, &revision]));
        }
        let current = self.active_file_and_text(cx).map(|(_, text, _)| text).unwrap_or_default();
        let title = rel.rsplit('/').next().unwrap_or(&rel).to_string();
        let (repo2, rel2, rev2) = (repo.clone(), rel.clone(), revision.clone());
        let source = DiffSource {
            title,
            subtitle: format!("{revision} · {rel}"),
            load: Arc::new(move || (repo2.show(&rev2, &rel2).unwrap_or_default(), current.clone())),
            left_label: revision,
            right_label: tr("Your version").to_string(),
            open_path: Some(path.to_path_buf()),
        };
        self.open_diff_tab(tr("Compare").as_str(), vec![source], 0, window, cx);
        Ok(None)
    }

    pub(super) fn compare_with_clipboard(&mut self, _: &CompareWithClipboard, window: &mut Window, cx: &mut Context<Self>) {
        let Some((path, text, selected)) = self.active_file_and_text(cx) else { return };
        let clipboard = cx.read_from_clipboard().and_then(|c| c.text()).unwrap_or_default();
        let mine = if selected.is_empty() { text } else { selected };
        let title = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let source = DiffSource {
            title,
            subtitle: tr("Clipboard ↔ editor").to_string(),
            load: Arc::new(move || (clipboard.clone(), mine.clone())),
            left_label: tr("Clipboard").to_string(),
            right_label: tr("Editor").to_string(),
            open_path: Some(path),
        };
        self.open_diff_tab(tr("Clipboard").as_str(), vec![source], 0, window, cx);
    }

    pub(super) fn resolve_conflicts(&mut self, _: &ResolveConflicts, window: &mut Window, cx: &mut Context<Self>) {
        let Some(repo) = self.intel.repo().cloned() else { return };
        let items: Vec<NavItem> = repo
            .status()
            .unwrap_or_default()
            .into_iter()
            .filter(|s| s.change == git::Change::Conflicted)
            .map(|s| {
                let path = repo.root().join(&s.path);
                let text = std::fs::read_to_string(&path).unwrap_or_default();
                let found = git::conflicts(&text);
                let at = found.first().map_or(0, |c| c.whole.start);
                NavItem {
                    label: s.path.clone(),
                    detail: trf("{} conflict(s) — ⌥↵ to merge", &[&found.len().to_string()]),
                    icon: "⇆",
                    target: NavTarget::Location(php_index::Location { path, range: at..at }),
                }
            })
            .collect();
        if items.is_empty() {
            self.output = Some(super::tools::Output { title: tr("Merge").to_string(), text: tr("No conflicted files.").to_string(), running: false, ok: true });
            cx.notify();
            return;
        }
        self.open_list(NavMode::Conflicts, items, window, cx);
    }

    /// The three-pane merge window for a file whose text still has `<<<<<<<` blocks.
    pub(crate) fn open_merge(&mut self, path: &std::path::Path, text: String, window: &mut Window, cx: &mut Context<Self>) {
        if git::conflicts(&text).is_empty() {
            self.output = Some(super::tools::Output { title: tr("Merge").to_string(), text: tr("No merge conflicts in this file.").to_string(), running: false, ok: true });
            cx.notify();
            return;
        }
        let text = match self.intel.repo() {
            Some(repo) => with_base(repo, path, text),
            None => text,
        };
        let dialog = cx.new(|cx| crate::dialogs::MergeDialog::new(path, text, window, cx));
        self.show_modal(dialog, window, cx);
    }

    /// Puts the resolved text back: into the open editor when the file is open, else on disk.
    pub(super) fn apply_merge(&mut self, path: PathBuf, text: String, remaining: usize, cx: &mut Context<Self>) {
        let open = self.tabs.iter().find(|tab| tab.read(cx).buffer.path() == Some(path.as_path())).cloned();
        if let Some(tab) = open {
            tab.update(cx, |editor, cx| editor.replace_text(&text, cx));
        } else if std::fs::write(&path, &text).is_ok() {
            self.intel.update_file(&path, &text);
        }
        let message = if remaining == 0 {
            trf("{} resolved", &[&path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().to_string())])
        } else {
            trf("{} conflict(s) left in {}", &[&remaining.to_string(), &path.display().to_string()])
        };
        self.output = Some(super::tools::Output { title: tr("Merge").to_string(), text: message, running: false, ok: true });
        self.refresh_git(cx);
        cx.notify();
    }

    /// Stages the active file once no conflict markers are left in it.
    pub(super) fn mark_resolved(&mut self, _: &MarkResolved, _: &mut Window, cx: &mut Context<Self>) {
        let Some(repo) = self.intel.repo().cloned() else { return };
        let Some((path, text, _)) = self.active_file_and_text(cx) else { return };
        let message = if !git::conflicts(&text).is_empty() {
            tr("Resolve the remaining conflicts first (⌥↵ on each)").to_string()
        } else {
            self.save_all(cx);
            match repo.relative(&path).map(|rel| repo.stage(&[rel.clone()]).map(|_| rel)) {
                Some(Ok(rel)) => trf("{} marked as resolved", &[&rel]),
                Some(Err(e)) => e,
                None => tr("The file is outside the repository").to_string(),
            }
        };
        self.output = Some(super::tools::Output { title: tr("Merge").to_string(), text: message, running: false, ok: true });
        self.refresh_git(cx);
        cx.notify();
    }
}

/// Rebuilds the conflict text with `|||||||` ancestor sections from the index stages, so the merge
/// window can show BASE and merge single lines. The working file is kept when it no longer matches
/// what git would produce — the user may have resolved some conflicts there by hand.
fn with_base(repo: &git::Repo, path: &std::path::Path, text: String) -> String {
    let Some(rel) = repo.relative(path) else { return text };
    let Some((base, ours, theirs)) = repo.merge_stages(&rel) else { return text };
    let rebuilt = git::merge3(&base, &ours, &theirs, "LOCAL", "REMOTE");
    let old = git::conflicts(&text);
    let new = git::conflicts(&rebuilt);
    let same = old.len() == new.len()
        && old
            .iter()
            .zip(&new)
            .all(|(a, b)| text[a.ours.clone()] == rebuilt[b.ours.clone()] && text[a.theirs.clone()] == rebuilt[b.theirs.clone()]);
    if same { rebuilt } else { text }
}
