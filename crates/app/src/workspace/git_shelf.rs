//! Git ▸ Shelve Changes… / Shelf: like stash, but for chosen files, kept in .phpcraft/shelf.

use gpui::{AppContext, Context, Window, actions};

use super::Workspace;
use crate::dialogs::{ShelfDialog, ShelfMode};

actions!(workspace, [GitShelve, GitShelf]);

impl Workspace {
    fn shelf_dir(&self) -> std::path::PathBuf {
        self.project.root().join(".phpcraft/shelf")
    }

    pub(super) fn git_shelve(&mut self, _: &GitShelve, window: &mut Window, cx: &mut Context<Self>) {
        self.save_all(cx);
        let files: Vec<String> = self.git.status.iter().map(|s| s.path.clone()).collect();
        let dialog = cx.new(|cx| ShelfDialog::new(ShelfMode::Shelve(files), window, cx));
        self.show_modal(dialog, window, cx);
    }

    pub(super) fn git_shelf(&mut self, _: &GitShelf, window: &mut Window, cx: &mut Context<Self>) {
        let shelves = git::shelves(&self.shelf_dir());
        let dialog = cx.new(|cx| ShelfDialog::new(ShelfMode::Browse(shelves), window, cx));
        self.show_modal(dialog, window, cx);
    }

    pub(super) fn shelve_files(&mut self, name: &str, files: &[String], window: &mut Window, cx: &mut Context<Self>) {
        self.dismiss_modal(window, cx);
        let Some(repo) = self.intel.repo().cloned() else { return };
        crate::local_history::record_disk(&files.iter().map(|f| repo.root().join(f)).collect::<Vec<_>>(), "Before Shelve");
        let text = match repo.shelve(&self.shelf_dir(), name, files) {
            Ok(shelf) => format!("Shelved {} file(s) as '{}'. Git ▸ Shelf to bring them back.", shelf.files.len(), shelf.name),
            Err(e) => e,
        };
        self.output = Some(super::tools::Output { title: "Shelve".into(), text, running: false, ok: true });
        self.reload_open_files(cx);
        self.refresh_git(cx);
        cx.notify();
    }

    pub(super) fn unshelve(&mut self, id: &str, delete_only: bool, window: &mut Window, cx: &mut Context<Self>) {
        let dir = self.shelf_dir();
        if delete_only {
            let _ = std::fs::remove_dir_all(dir.join(id));
            // Refresh the list in place.
            self.git_shelf(&GitShelf, window, cx);
            return;
        }
        self.dismiss_modal(window, cx);
        let Some(repo) = self.intel.repo().cloned() else { return };
        self.save_all(cx);
        let text = match repo.unshelve(&dir, id, false) {
            Ok(()) => "Unshelved.".to_string(),
            Err(e) => format!("Unshelve failed: {e}"),
        };
        self.output = Some(super::tools::Output { title: "Unshelve".into(), text, running: false, ok: true });
        self.reload_open_files(cx);
        self.refresh_git(cx);
        cx.notify();
    }
}
