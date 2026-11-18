//! Files changed on disk by git, Artisan, a deploy or another editor: clean buffers are
//! reloaded, the project tree is refreshed and the index is rebuilt. Polled every two
//! seconds while the window is active (gpui has no cross-platform file watcher here).

use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, Instant, SystemTime};

use gpui::{Context, Entity};

use super::Workspace;
use crate::editor_view::EditorView;

/// Modification time and size: enough to notice a rewrite without reading the file.
#[derive(Clone, Copy, PartialEq)]
pub(super) struct Stamp {
    time: Option<SystemTime>,
    len: u64,
}

/// The stamps of the open tabs, by path.
pub(super) type Disk = HashMap<std::path::PathBuf, Option<Stamp>>;

fn stamp(path: &Path) -> Option<Stamp> {
    let meta = std::fs::metadata(path).ok()?;
    Some(Stamp { time: meta.modified().ok(), len: meta.len() })
}

/// Rebuild the index at most this often after a burst of external writes.
const RESCAN_AFTER: Duration = Duration::from_secs(5);

impl Workspace {
    /// Starts polling the files of open tabs and the expanded tree.
    pub(crate) fn start_file_polling(&mut self, cx: &mut Context<Self>) {
        self.check_disk(cx);
        self._file_poll = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(2)).await;
                let poll = |this: &mut Self, cx: &mut Context<Self>| {
                    if this.window_active {
                        this.check_disk(cx);
                    }
                };
                if this.update(cx, poll).is_err() {
                    break;
                }
            }
        }));
    }

    /// Every tab of every pane (the focused pane keeps its tabs in `tabs`).
    fn all_tabs(&self) -> Vec<Entity<EditorView>> {
        let mut tabs = self.tabs.clone();
        for pane in self.panes.iter().flatten() {
            tabs.extend(pane.tabs.iter().cloned());
        }
        tabs
    }

    /// Reloads buffers whose file changed on disk, refreshes the tree, reindexes.
    pub(super) fn check_disk(&mut self, cx: &mut Context<Self>) {
        let mut stale = false;
        let mut reloads: Vec<(Entity<EditorView>, String)> = Vec::new();
        for tab in self.all_tabs() {
            let editor = tab.read(cx);
            let Some(path) = editor.buffer.path().map(|p| p.to_path_buf()) else { continue };
            let now = stamp(&path);
            match self.disk.get(&path) {
                // First sight of a tab: remember the file as it is now.
                None => {
                    self.disk.insert(path, now);
                    continue;
                }
                Some(known) if *known == now => continue,
                Some(_) => {}
            }
            self.disk.insert(path.clone(), now);
            // Unsaved edits win: the file is reloaded on the next save instead.
            if editor.buffer.is_dirty() {
                continue;
            }
            if let Ok(text) = std::fs::read_to_string(&path)
                && text != editor.buffer.text()
            {
                reloads.push((tab.clone(), text));
                stale = true;
            }
        }
        for (tab, text) in reloads {
            tab.update(cx, |editor, cx| editor.reload_from_disk(&text, cx));
        }
        // New or deleted files in the expanded part of the tree.
        let rows = self.project.rows();
        if rows != self.rows {
            self.rows = rows;
            stale = true;
            cx.notify();
        }
        if stale {
            self.reindex_after_disk_change(cx);
        }
    }

    /// Classes come from the index, so new files (Artisan, git pull) need a rescan.
    fn reindex_after_disk_change(&mut self, cx: &mut Context<Self>) {
        if !self.intel.is_ready() {
            return;
        }
        if self.last_disk_scan.is_some_and(|at| at.elapsed() < RESCAN_AFTER) {
            return;
        }
        self.last_disk_scan = Some(Instant::now());
        let id = self.begin_task("Reindexing…", cx);
        let intel = self.intel.clone();
        cx.spawn(async move |this, cx| {
            cx.background_executor().spawn(async move { intel.scan() }).await;
            this.update(cx, |this, cx| this.end_task(id, cx)).ok();
        })
        .detach();
    }
}
