//! Restores the open tabs, split panes, pinned tabs and carets of a project on startup,
//! the way modern IDEs do. Kept per project in `.phpcraft/session.json`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use gpui::{App, Context, Window};
use serde::{Deserialize, Serialize};

use super::panes::{MAX_PANES, Pane};
use super::Workspace;

#[derive(Default, Serialize, Deserialize)]
struct Session {
    #[serde(default)]
    panes: Vec<PaneSession>,
    #[serde(default)]
    focused: usize,
    #[serde(default)]
    pinned: Vec<String>,
    /// Caret (char offset) per file, so a reopened file lands where it was.
    #[serde(default)]
    carets: BTreeMap<String, usize>,
}

#[derive(Default, Serialize, Deserialize)]
struct PaneSession {
    #[serde(default)]
    tabs: Vec<String>,
    #[serde(default)]
    active: usize,
}

impl PaneSession {
    fn save(pane: &Pane, carets: &mut BTreeMap<String, usize>, cx: &App) -> Self {
        let mut saved = Self::default();
        for tab in &pane.tabs {
            let editor = tab.read(cx);
            let Some(path) = editor.buffer.path() else { continue };
            let key = path.display().to_string();
            if let Some(selection) = editor.buffer.selections().first() {
                carets.insert(key.clone(), selection.head);
            }
            saved.tabs.push(key);
        }
        saved.active = pane.active.min(saved.tabs.len().saturating_sub(1));
        saved
    }
}

fn file(root: &Path) -> PathBuf {
    root.join(".phpcraft").join("session.json")
}

fn load(root: &Path) -> Option<Session> {
    let text = std::fs::read_to_string(file(root)).ok()?;
    serde_json::from_str(&text).ok()
}

impl Workspace {
    /// The tabs of pane `ix` (the focused pane keeps them in `tabs`).
    fn pane_at(&self, ix: usize) -> Pane {
        if ix == self.focused_pane {
            return Pane { tabs: self.tabs.clone(), active: self.active };
        }
        match self.panes.get(ix).and_then(Option::as_ref) {
            Some(pane) => Pane { tabs: pane.tabs.clone(), active: pane.active },
            None => Pane { tabs: Vec::new(), active: 0 },
        }
    }

    /// Writes the open tabs and carets to `.phpcraft/session.json`.
    pub(super) fn save_session(&self, cx: &App) {
        if self.restoring_session {
            return;
        }
        let mut session = Session { focused: self.focused_pane, ..Default::default() };
        for ix in 0..self.panes.len() {
            session.panes.push(PaneSession::save(&self.pane_at(ix), &mut session.carets, cx));
        }
        session.pinned = self.pinned.iter().map(|p| p.display().to_string()).collect();
        let file = file(self.project.root());
        if let Some(dir) = file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(text) = serde_json::to_string_pretty(&session) {
            let _ = std::fs::write(file, text);
        }
    }

    /// Reopens the tabs saved by the last run; stale paths are skipped.
    pub(crate) fn restore_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let root = self.project.root().to_path_buf();
        let Some(session) = load(&root) else { return };
        if session.panes.is_empty() {
            return;
        }
        let panes = session.panes.len().clamp(1, MAX_PANES);
        self.restoring_session = true;
        self.tabs = Vec::new();
        self.active = 0;
        self.panes = (0..panes).map(|_| None).collect();
        self.focused_pane = 0;
        for (ix, saved) in session.panes.iter().take(panes).enumerate() {
            self.focus_pane(ix, window, cx);
            for path in &saved.tabs {
                let path = PathBuf::from(path);
                if path.starts_with(&root) && path.is_file() {
                    self.open_file(&path, window, cx);
                }
            }
            self.active = saved.active.min(self.tabs.len().saturating_sub(1));
        }
        // Swap the focused pane's tabs in last, so `tabs`/`active` end on it.
        self.focused_pane = session.focused.min(panes - 1);
        if let Some(pane) = self.panes[self.focused_pane].take() {
            self.panes[self.focused_pane] = Some(Pane {
                tabs: std::mem::replace(&mut self.tabs, pane.tabs),
                active: std::mem::replace(&mut self.active, pane.active),
            });
        }
        self.pinned = session
            .pinned
            .iter()
            .map(PathBuf::from)
            .filter(|p| p.starts_with(&root))
            .collect();
        for tab in self.tabs.clone() {
            let Some(path) = tab.read(cx).buffer.path().map(|p| p.display().to_string()) else { continue };
            let Some(&caret) = session.carets.get(&path) else { continue };
            tab.update(cx, |editor, cx| editor.restore_caret(caret, cx));
        }
        self.restoring_session = false;
        // Drops files deleted since the last run from the saved session.
        self.save_session(cx);
        self.focus_active(window, cx);
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sessions_round_trip() {
        let session = Session {
            panes: vec![
                PaneSession { tabs: vec!["/p/a.php".into()], active: 0 },
                PaneSession { tabs: vec!["/p/b.php".into(), "/p/c.php".into()], active: 1 },
            ],
            focused: 1,
            pinned: vec!["/p/a.php".into()],
            carets: [("/p/c.php".to_string(), 42)].into_iter().collect(),
        };
        let text = serde_json::to_string(&session).unwrap();
        let back: Session = serde_json::from_str(&text).unwrap();
        assert_eq!(back.panes.len(), 2);
        assert_eq!(back.focused, 1);
        assert_eq!(back.panes[1].active, 1);
        assert_eq!(back.pinned, vec!["/p/a.php".to_string()]);
        assert_eq!(back.carets["/p/c.php"], 42);
    }

    /// A session written by an older build (no pinned/carets) still opens.
    #[test]
    fn older_sessions_still_load() {
        let session: Session = serde_json::from_str(r#"{"panes":[{"tabs":["/p/a.php"],"active":0}]}"#).unwrap();
        assert_eq!(session.panes.len(), 1);
        assert!(session.carets.is_empty());
        assert!(session.pinned.is_empty());
    }
}
