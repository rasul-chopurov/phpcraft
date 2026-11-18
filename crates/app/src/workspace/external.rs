//! Files changed on disk by git, Artisan, a deploy or another editor: clean buffers are
//! reloaded, a buffer with unsaved edits asks before it is overwritten, a deleted file asks
//! before its tab closes. Polled every two seconds while the window is active (gpui has no
//! cross-platform file watcher here).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use gpui::{AppContext, Context, Entity, Window};

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
    /// Starts polling the files of open tabs and the expanded tree.
    pub(crate) fn start_file_polling(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let handle = window.window_handle();
        self.check_disk(cx);
        self.show_disk_ask(window, cx);
        self._file_poll = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(2)).await;
                let alive = cx
                    .update_window(handle, |_, window, cx| {
                        this.update(cx, |this, cx| {
                            if this.window_active {
                                this.check_disk(cx);
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
            self.disk.insert(path.clone(), now);
            // A file that vanished stays in its tab until the user says otherwise.
            if now.is_none() {
                self.ask_disk(Ask::Deleted { path });
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else { continue };
            if text == editor.buffer.text() {
                continue;
            }
            if editor.buffer.is_dirty() {
                self.ask_disk(Ask::Changed { path, text });
            } else {
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
            // The text the question was about, or a fresh read if it was queued before a save.
            let text = match asked {
                Some(Ask::Changed { text, .. }) => Some(text),
                _ => std::fs::read_to_string(&path).ok(),
            };
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
