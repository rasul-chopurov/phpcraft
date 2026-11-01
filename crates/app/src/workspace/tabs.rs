//! Editor tab management (Window menu): Reopen Closed Tab, Pin Tab, Close Tabs to the Left /
//! Right, Close Unmodified, Close All but Pinned; and bookmarks across files (Show Bookmarks,
//! ⌃0-9 to jump to a mnemonic).

use std::path::PathBuf;

use gpui::{Context, Window, actions};

use super::Workspace;

actions!(
    workspace,
    [
        ReopenClosedTab,
        TogglePinTab,
        CloseTabsLeft,
        CloseTabsRight,
        CloseUnmodifiedTabs,
        CloseAllButPinned,
        CloseOtherTabs,
        CloseAllTabs,
        ShowBookmarks,
        GoToBookmark0,
        GoToBookmark1,
        GoToBookmark2,
        GoToBookmark3,
        GoToBookmark4,
        GoToBookmark5,
        GoToBookmark6,
        GoToBookmark7,
        GoToBookmark8,
        GoToBookmark9,
    ]
);

/// Closed files remembered for Reopen Closed Tab.
const CLOSED_HISTORY: usize = 30;

impl Workspace {
    fn tab_path(&self, ix: usize, cx: &gpui::App) -> Option<PathBuf> {
        self.tabs.get(ix).and_then(|t| t.read(cx).buffer.path().map(|p| p.to_path_buf()))
    }

    pub(super) fn is_pinned(&self, ix: usize, cx: &gpui::App) -> bool {
        self.tab_path(ix, cx).is_some_and(|p| self.pinned.contains(&p))
    }

    /// Remembers a tab's file before it closes (Reopen Closed Tab).
    pub(super) fn remember_closed(&mut self, ix: usize, cx: &gpui::App) {
        if let Some(path) = self.tab_path(ix, cx) {
            self.closed_tabs.retain(|p| *p != path);
            self.closed_tabs.push(path);
            if self.closed_tabs.len() > CLOSED_HISTORY {
                self.closed_tabs.remove(0);
            }
        }
    }

    /// Closes the tabs for which `close` is true (pinned ones only when `include_pinned`).
    pub(super) fn close_tabs_where(&mut self, include_pinned: bool, close: impl Fn(usize, &Self, &gpui::App) -> bool, window: &mut Window, cx: &mut Context<Self>) {
        for ix in (0..self.tabs.len()).rev() {
            if close(ix, self, cx) && (include_pinned || !self.is_pinned(ix, cx)) {
                self.close_tab_at(ix, window, cx);
            }
        }
    }

    pub(super) fn reopen_closed_tab(&mut self, _: &ReopenClosedTab, window: &mut Window, cx: &mut Context<Self>) {
        while let Some(path) = self.closed_tabs.pop() {
            if path.is_file() {
                self.open_file(&path, window, cx);
                return;
            }
        }
    }

    pub(super) fn toggle_pin_tab(&mut self, _: &TogglePinTab, _: &mut Window, cx: &mut Context<Self>) {
        self.toggle_pin_at(self.active, cx);
    }

    pub(super) fn toggle_pin_at(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(path) = self.tab_path(ix, cx) else { return };
        if !self.pinned.remove(&path) {
            self.pinned.insert(path);
            // Pinned tabs move to the front.
            let tab = self.tabs.remove(ix);
            let at = (0..self.tabs.len()).take_while(|&i| self.is_pinned(i, cx)).count();
            self.tabs.insert(at, tab);
            self.active = at;
        }
        cx.notify();
    }

    pub(super) fn close_tabs_left(&mut self, _: &CloseTabsLeft, window: &mut Window, cx: &mut Context<Self>) {
        let active = self.active;
        self.close_tabs_where(false, move |ix, _, _| ix < active, window, cx);
    }

    pub(super) fn close_tabs_right(&mut self, _: &CloseTabsRight, window: &mut Window, cx: &mut Context<Self>) {
        let active = self.active;
        self.close_tabs_where(false, move |ix, _, _| ix > active, window, cx);
    }

    pub(super) fn close_unmodified_tabs(&mut self, _: &CloseUnmodifiedTabs, window: &mut Window, cx: &mut Context<Self>) {
        self.close_tabs_where(false, |ix, this, cx| !this.tabs[ix].read(cx).buffer.is_dirty(), window, cx);
    }

    pub(super) fn close_all_but_pinned(&mut self, _: &CloseAllButPinned, window: &mut Window, cx: &mut Context<Self>) {
        self.close_tabs_where(false, |_, _, _| true, window, cx);
    }

    pub(super) fn close_other_tabs(&mut self, _: &CloseOtherTabs, window: &mut Window, cx: &mut Context<Self>) {
        let active = self.active;
        self.close_tabs_where(false, move |ix, _, _| ix != active, window, cx);
    }

    pub(super) fn close_all_tabs(&mut self, _: &CloseAllTabs, window: &mut Window, cx: &mut Context<Self>) {
        self.close_tabs_where(true, |_, _, _| true, window, cx);
    }

    /// Show Bookmarks (⌘F3): every bookmark, to jump to.
    pub(super) fn show_bookmarks(&mut self, _: &ShowBookmarks, window: &mut Window, cx: &mut Context<Self>) {
        let root = self.project.root().to_path_buf();
        let items = crate::bookmarks::all()
            .into_iter()
            .filter_map(|b| {
                let text = std::fs::read_to_string(&b.path).ok()?;
                let start: usize = text.split_inclusive('\n').take(b.line).map(str::len).sum();
                let line_text = text[start..].lines().next().unwrap_or("").trim().to_string();
                let file = b.path.strip_prefix(&root).unwrap_or(&b.path).display().to_string();
                Some(crate::dialogs::NavItem {
                    label: format!("{}{}", b.mnemonic.map(|m| format!("[{m}] ")).unwrap_or_default(), line_text),
                    detail: format!("{file}:{}", b.line + 1),
                    icon: "◆",
                    target: crate::dialogs::NavTarget::Location(php_index::Location { path: b.path, range: start..start }),
                })
            })
            .collect();
        self.open_list(crate::dialogs::NavMode::Bookmarks, items, window, cx);
    }

    /// Find Usages results as a list with a preview (or straight to the only one).
    pub(crate) fn show_usages(&mut self, name: &str, found: Vec<php_index::Location>, window: &mut Window, cx: &mut Context<Self>) {
        let _ = name;
        if found.len() == 1 {
            let location = found.into_iter().next().unwrap();
            self.open_location(&location, None, window, cx);
            return;
        }
        let root = self.project.root().to_path_buf();
        let mut texts: std::collections::HashMap<PathBuf, String> = Default::default();
        let items = found
            .into_iter()
            .map(|location| {
                let text = texts.entry(location.path.clone()).or_insert_with(|| std::fs::read_to_string(&location.path).unwrap_or_default());
                let start = location.range.start.min(text.len());
                let line = text[..start].matches('\n').count();
                let line_start = text[..start].rfind('\n').map_or(0, |i| i + 1);
                let line_text = text[line_start..].lines().next().unwrap_or("").trim().to_string();
                let file = location.path.strip_prefix(&root).unwrap_or(&location.path).display().to_string();
                crate::dialogs::NavItem {
                    label: line_text,
                    detail: format!("{file}:{}", line + 1),
                    icon: "→",
                    target: crate::dialogs::NavTarget::Location(location),
                }
            })
            .collect();
        self.open_list(crate::dialogs::NavMode::Usages, items, window, cx);
    }

    /// Call Hierarchy results: one row per call, labelled with the calling function.
    pub(crate) fn show_callers(&mut self, callers: Vec<(String, php_index::Location)>, window: &mut Window, cx: &mut Context<Self>) {
        let root = self.project.root().to_path_buf();
        let items = callers
            .into_iter()
            .map(|(caller, location)| {
                let text = std::fs::read_to_string(&location.path).unwrap_or_default();
                let line = text[..location.range.start.min(text.len())].matches('\n').count() + 1;
                let file = location.path.strip_prefix(&root).unwrap_or(&location.path).display().to_string();
                crate::dialogs::NavItem { label: caller, detail: format!("{file}:{line}"), icon: "←", target: crate::dialogs::NavTarget::Location(location) }
            })
            .collect();
        self.open_list(crate::dialogs::NavMode::Callers, items, window, cx);
    }

    fn go_to_bookmark(&mut self, mnemonic: char, window: &mut Window, cx: &mut Context<Self>) {
        let Some(b) = crate::bookmarks::by_mnemonic(mnemonic) else { return };
        let Ok(text) = std::fs::read_to_string(&b.path) else { return };
        let start: usize = text.split_inclusive('\n').take(b.line).map(str::len).sum();
        let location = php_index::Location { path: b.path, range: start..start };
        self.open_location(&location, None, window, cx);
    }
}

macro_rules! go_to_bookmark {
    ($($action:ident => $fn:ident, $c:literal;)*) => {
        impl Workspace {
            $(pub(super) fn $fn(&mut self, _: &$action, window: &mut Window, cx: &mut Context<Self>) {
                self.go_to_bookmark($c, window, cx);
            })*
        }
    };
}

go_to_bookmark! {
    GoToBookmark0 => go_to_bookmark_0, '0';
    GoToBookmark1 => go_to_bookmark_1, '1';
    GoToBookmark2 => go_to_bookmark_2, '2';
    GoToBookmark3 => go_to_bookmark_3, '3';
    GoToBookmark4 => go_to_bookmark_4, '4';
    GoToBookmark5 => go_to_bookmark_5, '5';
    GoToBookmark6 => go_to_bookmark_6, '6';
    GoToBookmark7 => go_to_bookmark_7, '7';
    GoToBookmark8 => go_to_bookmark_8, '8';
    GoToBookmark9 => go_to_bookmark_9, '9';
}
