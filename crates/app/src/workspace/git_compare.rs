//! Git ▸ Compare with Branch / Revision… and Compare with Clipboard for the active file, and
//! merge conflicts: Resolve Conflicts… lists the conflicted files, Merge Conflicts… (⌥↵ in a
//! conflicted file, or the editor's context menu) opens the three-pane merge window, and Mark File as Resolved stages one.

use std::path::PathBuf;
use std::sync::Arc;

use gpui::{AppContext, Context, Window, actions};

use super::Workspace;
use crate::diff_view::DiffSource;
use crate::dialogs::{FileOp, InputDialog, NavItem, NavMode, NavTarget};

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
        let repo = self.intel.repo().cloned().ok_or("Not a Git repository")?;
        let rel = repo.relative(path).ok_or("The file is outside the repository")?;
        let revision = revision.trim().to_string();
        if revision.is_empty() || revision.starts_with('-') {
            return Err("Enter a branch, tag or commit".into());
        }
        if repo.show(&revision, &rel).is_none() {
            return Err(format!("{rel} doesn't exist in {revision}"));
        }
        let current = self.active_file_and_text(cx).map(|(_, text, _)| text).unwrap_or_default();
        let title = rel.rsplit('/').next().unwrap_or(&rel).to_string();
        let (repo2, rel2, rev2) = (repo.clone(), rel.clone(), revision.clone());
        let source = DiffSource {
            title,
            subtitle: format!("{revision} · {rel}"),
            load: Arc::new(move || (repo2.show(&rev2, &rel2).unwrap_or_default(), current.clone())),
            left_label: revision,
            right_label: "Your version".into(),
            open_path: Some(path.to_path_buf()),
        };
        self.open_diff_tab("Compare", vec![source], 0, window, cx);
        Ok(None)
    }

    pub(super) fn compare_with_clipboard(&mut self, _: &CompareWithClipboard, window: &mut Window, cx: &mut Context<Self>) {
        let Some((path, text, selected)) = self.active_file_and_text(cx) else { return };
        let clipboard = cx.read_from_clipboard().and_then(|c| c.text()).unwrap_or_default();
        let mine = if selected.is_empty() { text } else { selected };
        let title = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let source = DiffSource {
            title,
            subtitle: "Clipboard ↔ editor".into(),
            load: Arc::new(move || (clipboard.clone(), mine.clone())),
            left_label: "Clipboard".into(),
            right_label: "Editor".into(),
            open_path: Some(path),
        };
        self.open_diff_tab("Clipboard", vec![source], 0, window, cx);
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
                    detail: format!("{} conflict(s) — ⌥↵ to merge", found.len()),
                    icon: "⇆",
                    target: NavTarget::Location(php_index::Location { path, range: at..at }),
                }
            })
            .collect();
        if items.is_empty() {
            self.output = Some(super::tools::Output { title: "Merge".into(), text: "No conflicted files.".into(), running: false, ok: true });
            cx.notify();
            return;
        }
        self.open_list(NavMode::Conflicts, items, window, cx);
    }

    /// The three-pane merge window for a file whose text still has `<<<<<<<` blocks.
    pub(crate) fn open_merge(&mut self, path: &std::path::Path, text: String, window: &mut Window, cx: &mut Context<Self>) {
        if git::conflicts(&text).is_empty() {
            self.output = Some(super::tools::Output { title: "Merge".into(), text: "No merge conflicts in this file.".into(), running: false, ok: true });
            cx.notify();
            return;
        }
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
            format!("{} resolved", path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().to_string()))
        } else {
            format!("{remaining} conflict(s) left in {}", path.display())
        };
        self.output = Some(super::tools::Output { title: "Merge".into(), text: message, running: false, ok: true });
        self.refresh_git(cx);
        cx.notify();
    }

    /// Stages the active file once no conflict markers are left in it.
    pub(super) fn mark_resolved(&mut self, _: &MarkResolved, _: &mut Window, cx: &mut Context<Self>) {
        let Some(repo) = self.intel.repo().cloned() else { return };
        let Some((path, text, _)) = self.active_file_and_text(cx) else { return };
        let message = if !git::conflicts(&text).is_empty() {
            "Resolve the remaining conflicts first (⌥↵ on each)".to_string()
        } else {
            self.save_all(cx);
            match repo.relative(&path).map(|rel| repo.stage(&[rel.clone()]).map(|_| rel)) {
                Some(Ok(rel)) => format!("{rel} marked as resolved"),
                Some(Err(e)) => e,
                None => "The file is outside the repository".into(),
            }
        };
        self.output = Some(super::tools::Output { title: "Merge".into(), text: message, running: false, ok: true });
        self.refresh_git(cx);
        cx.notify();
    }
}
