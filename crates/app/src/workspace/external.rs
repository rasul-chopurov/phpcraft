//! Files changed on disk by git, Artisan, a deploy or another editor: clean buffers are
//! reloaded, a buffer with unsaved edits asks before it is overwritten, a deleted file asks
//! before its tab closes.
//!
//! The platform watcher (FSEvents, ReadDirectoryChangesW, inotify) is registered on the
//! folders of the open tabs only — a project's `vendor` and `node_modules` rarely hold a tab,
//! so nothing watches a hundred thousand files. A stamp check on the reported path keeps a
//! burst of events (a save is often a write, a rename and a chmod) down to one read.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use gpui::{App, AppContext, Context, Entity, Window};
use notify::{EventKind, RecursiveMode, Watcher};

use super::Workspace;
use crate::editor_view::EditorView;
use crate::i18n::{tr, trf};

/// Modification time and size: enough to notice a rewrite without reading the file.
#[derive(Clone, Copy, PartialEq)]
pub(super) struct Stamp {
    time: Option<SystemTime>,
    len: u64,
}

/// The stamps of the open tabs, by path.
pub(super) type Disk = HashMap<std::path::PathBuf, Option<Stamp>>;

/// Something the poll found that only the user can decide about.
pub(super) enum Ask {
    /// The file changed on disk while its buffer had unsaved edits.
    Changed { path: PathBuf, text: String },
    /// The file is gone from disk.
    Deleted { path: PathBuf },
}

impl Ask {
    fn path(&self) -> &Path {
        match self {
            Ask::Changed { path, .. } | Ask::Deleted { path } => path,
        }
    }
}

fn stamp(path: &Path) -> Option<Stamp> {
    let meta = std::fs::metadata(path).ok()?;
    Some(Stamp { time: meta.modified().ok(), len: meta.len() })
}

/// Rebuild the index at most this often after a burst of external writes.
const RESCAN_AFTER: Duration = Duration::from_secs(5);

impl Workspace {
    /// Watches the folders of the open tabs, and catches up on anything that happened while
    /// the window was away from the front.
    pub(crate) fn start_file_watching(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let handle = window.window_handle();
        self.check_disk(cx);
        self.show_disk_ask(window, cx);

        // Unbounded, so a storm of events never blocks the watcher's own thread.
        let (events, incoming) = async_channel::unbounded::<Vec<PathBuf>>();
        let watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
            let Ok(event) = event else { return };
            // Reading a file is not a change; only writes, renames, creations and removals are.
            if matches!(event.kind, EventKind::Access(_)) || event.paths.is_empty() {
                return;
            }
            let _ = events.send_blocking(event.paths);
        });
        // Without a watcher the tabs still catch up whenever the window is activated.
        let Ok(watcher) = watcher else { return };
        self.file_watcher = Some(watcher);
        self.watch_tab_dirs(cx);
        self._file_watch = Some(cx.spawn(async move |this, cx| {
            while let Ok(paths) = incoming.recv().await {
                let alive = cx
                    .update_window(handle, |_, window, cx| {
                        this.update(cx, |this, cx| {
                            // A tab opened since the last event brought a folder with it.
                            this.watch_tab_dirs(cx);
                            this.check_disk_paths(&paths, cx);
                            if this.window_active {
                                this.show_disk_ask(window, cx);
                            }
                        })
                        .is_ok()
                    })
                    .unwrap_or(false);
                if !alive {
                    break;
                }
            }
        }));
    }

    /// The folders holding an open tab.
    fn tab_dirs(&self, cx: &App) -> HashSet<PathBuf> {
        let mut dirs = HashSet::new();
        for tab in self.all_tabs() {
            if let Some(path) = tab.read(cx).buffer.path()
                && let Some(dir) = path.parent()
            {
                dirs.insert(dir.to_path_buf());
            }
        }
        dirs
    }

    /// Keeps the watched folders equal to the folders of the open tabs.
    pub(super) fn watch_tab_dirs(&mut self, cx: &App) {
        let dirs = self.tab_dirs(cx);
        if dirs == self.watched_dirs {
            return;
        }
        let added: Vec<PathBuf> = dirs.difference(&self.watched_dirs).cloned().collect();
        let removed: Vec<PathBuf> = self.watched_dirs.difference(&dirs).cloned().collect();
        if let Some(watcher) = self.file_watcher.as_mut() {
            // Non-recursive: a folder that cannot be watched (a permission, a network share)
            // only costs the events inside it.
            for dir in &added {
                let _ = watcher.watch(dir, RecursiveMode::NonRecursive);
            }
            for dir in &removed {
                let _ = watcher.unwatch(dir);
            }
        }
        self.watched_dirs = dirs;
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
        self.check_disk_of(None, cx);
    }

    /// The same, for the files a watcher event named: the rest of the tabs did not move.
    fn check_disk_paths(&mut self, paths: &[PathBuf], cx: &mut Context<Self>) {
        self.check_disk_of(Some(paths), cx);
    }

    fn check_disk_of(&mut self, only: Option<&[PathBuf]>, cx: &mut Context<Self>) {
        let mut stale = false;
        let mut reloads: Vec<(Entity<EditorView>, String)> = Vec::new();
        for tab in self.all_tabs() {
            let editor = tab.read(cx);
            let Some(path) = editor.buffer.path().map(|p| p.to_path_buf()) else { continue };
            if only.is_some_and(|only| !only.iter().any(|changed| changed == &path)) {
                continue;
            }
            let now = stamp(&path);
            let known = match self.disk.get(&path) {
                // First sight of a tab: remember the file as it is now.
                None => {
                    self.disk.insert(path, now);
                    continue;
                }
                Some(known) => *known,
            };
            if known == now {
                continue;
            }
            // A file that vanished stays in its tab until the user says otherwise.
            if now.is_none() {
                self.disk.insert(path.clone(), now);
                self.ask_disk(Ask::Deleted { path });
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else { continue };
            if text == editor.buffer.text() {
                self.disk.insert(path, now);
                continue;
            }
            if editor.buffer.is_dirty() {
                self.disk.insert(path.clone(), now);
                self.ask_disk(Ask::Changed { path, text });
                continue;
            }
            // Half a file would be worse than a late reload: a save that is still writing
            // changes the stamp again while we read.
            if stamp(&path) != now {
                continue;
            }
            self.disk.insert(path, now);
            reloads.push((tab.clone(), text));
            stale = true;
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

    /// Queues a question, once per path and pending change.
    fn ask_disk(&mut self, ask: Ask) {
        if !self.disk_asked.insert(ask.path().to_path_buf()) {
            return;
        }
        self.disk_asks.push(ask);
    }

    /// Asks the next queued question, one dialog at a time.
    pub(super) fn show_disk_ask(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.modal.is_some() || self.disk_asks.is_empty() {
            return;
        }
        let ask = self.disk_asks.remove(0);
        let path = ask.path().to_path_buf();
        let open = self.all_tabs().iter().any(|tab| tab.read(cx).buffer.path() == Some(path.as_path()));
        if !open {
            // The tab is gone (closed by hand): nothing left to ask about.
            self.disk_asked.remove(&path);
            self.show_disk_ask(window, cx);
            return;
        }
        let (title, message, label, id) = match &ask {
            Ask::Changed { .. } => (
                tr("File Changed on Disk").to_string(),
                trf("{} was changed by another program and has unsaved edits. Reload it and lose those edits?", &[name(&path).as_str()]),
                "Reload",
                format!("disk-reload:{}", path.display()),
            ),
            Ask::Deleted { .. } => (
                tr("File Deleted on Disk").to_string(),
                trf("{} is no longer on disk. Close its tab?", &[name(&path).as_str()]),
                "Close Tab",
                format!("disk-close:{}", path.display()),
            ),
        };
        let dialog = cx.new(|cx| crate::dialogs::ConfirmPrompt::new(id, title, message, label, window, cx));
        self.show_modal(dialog, window, cx);
        self.disk_current = Some(ask);
    }

    /// Esc, Cancel or a click outside: keep the buffer as it is and move on to the next question.
    pub(super) fn cancel_disk_ask(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ask) = self.disk_current.take() {
            self.disk_asked.remove(ask.path());
        }
        self.show_disk_ask(window, cx);
    }

    /// Handles the answer of a [`crate::dialogs::ConfirmPrompt`] about a file on disk.
    pub(super) fn answer_disk(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let (reload, rest) = match id.strip_prefix("disk-reload:") {
            Some(rest) => (true, rest),
            None => match id.strip_prefix("disk-close:") {
                Some(rest) => (false, rest),
                None => return false,
            },
        };
        let path = PathBuf::from(rest);
        let asked = self.disk_current.take();
        self.disk_asks.retain(|ask| ask.path() != path);
        self.disk_asked.remove(&path);
        if reload {
            // Prefer what is on disk now: the question may have been queued a moment before
            // the writer finished, and answering it must not restore half a file.
            let text = std::fs::read_to_string(&path).ok().or(match asked {
                Some(Ask::Changed { text, .. }) => Some(text),
                _ => None,
            });
            if let Some(text) = text {
                self.disk.insert(path.clone(), stamp(&path));
                for tab in self.all_tabs() {
                    if tab.read(cx).buffer.path() == Some(path.as_path()) {
                        tab.update(cx, |editor, cx| editor.reload_from_disk(&text, cx));
                    }
                }
            }
        } else {
            self.close_tabs_where(true, move |ix, this, cx| this.tab_path(ix, cx).as_deref() == Some(path.as_path()), window, cx);
        }
        self.show_disk_ask(window, cx);
        true
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

/// The file name alone keeps the question short.
fn name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string())
}
