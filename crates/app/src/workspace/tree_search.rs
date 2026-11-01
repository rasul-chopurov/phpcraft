//! Keyboard navigation and speed search in the project tree (PhpStorm's): with the tree
//! focused, ↑/↓ move, ←/→ collapse and expand, Enter opens, and typing (or ⌘F) opens a search
//! bar that jumps between matching rows among the expanded ones.

use gpui::{AppContext, Context, Entity, FocusHandle, Focusable, KeyDownEvent, ScrollStrategy, Subscription, UniformListScrollHandle, Window, actions, div, prelude::*, px};

use super::Workspace;
use crate::text_field::{FieldEvent, TextField};
use crate::theme;

actions!(workspace, [TreeSearch, TreeUp, TreeDown, TreeOpen, TreeCollapse, TreeExpand, TreeCancel]);

pub(super) struct TreeNav {
    pub focus: FocusHandle,
    pub scroll: UniformListScrollHandle,
    search: Option<Entity<TextField>>,
    _subscription: Option<Subscription>,
}

impl TreeNav {
    pub fn new(cx: &mut Context<Workspace>) -> Self {
        Self { focus: cx.focus_handle(), scroll: UniformListScrollHandle::new(), search: None, _subscription: None }
    }
}

/// Speed-search match: the query's characters in order (case-insensitive), contiguous first.
pub fn matches(name: &str, query: &str) -> bool {
    if query.is_empty() {
        return false;
    }
    let name = name.to_lowercase();
    let query = query.to_lowercase();
    if name.contains(&query) {
        return true;
    }
    let mut chars = name.chars();
    query.chars().all(|q| chars.any(|c| c == q))
}

impl Workspace {

    fn tree_query(&self, cx: &gpui::App) -> Option<String> {
        self.tree_nav.search.as_ref().map(|f| f.read(cx).text().to_string())
    }

    /// Whether row `ix` matches the open speed search.
    pub(super) fn tree_row_matches(&self, ix: usize, cx: &gpui::App) -> bool {
        match (self.tree_query(cx), self.rows.get(ix)) {
            (Some(query), Some(row)) => matches(&row.entry.name, &query),
            _ => false,
        }
    }

    fn selected_row(&self) -> Option<usize> {
        let path = self.selected_path.as_ref()?;
        self.rows.iter().position(|r| &r.entry.path == path)
    }

    fn select_row(&mut self, ix: usize, cx: &mut Context<Self>) {
        if let Some(row) = self.rows.get(ix) {
            self.selected_path = Some(row.entry.path.clone());
            self.tree_nav.scroll.scroll_to_item(ix, ScrollStrategy::Center);
            cx.notify();
        }
    }

    /// The next row (in `step` direction) that's a candidate: any row, or a match while searching.
    fn step_row(&mut self, step: isize, cx: &mut Context<Self>) {
        let len = self.rows.len() as isize;
        if len == 0 {
            return;
        }
        let searching = self.tree_query(cx).is_some_and(|q| !q.is_empty());
        let mut ix = self.selected_row().map_or(if step > 0 { -1 } else { len }, |i| i as isize);
        for _ in 0..len {
            ix += step;
            if ix < 0 || ix >= len {
                return;
            }
            if !searching || self.tree_row_matches(ix as usize, cx) {
                self.select_row(ix as usize, cx);
                return;
            }
        }
    }

    pub(super) fn tree_up(&mut self, _: &TreeUp, _: &mut Window, cx: &mut Context<Self>) {
        self.step_row(-1, cx);
    }

    pub(super) fn tree_down(&mut self, _: &TreeDown, _: &mut Window, cx: &mut Context<Self>) {
        self.step_row(1, cx);
    }

    pub(super) fn tree_open(&mut self, _: &TreeOpen, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ix) = self.selected_row() else { return };
        let row = &self.rows[ix];
        if row.entry.is_dir {
            let path = row.entry.path.clone();
            self.project.toggle(&path);
            self.rows = self.project.rows();
            cx.notify();
        } else {
            let path = row.entry.path.clone();
            self.close_tree_search(window, cx);
            self.open_file(&path, window, cx);
        }
    }

    fn set_expanded(&mut self, expand: bool, cx: &mut Context<Self>) {
        let Some(ix) = self.selected_row() else { return };
        let row = &self.rows[ix];
        if row.entry.is_dir && row.expanded != expand {
            let path = row.entry.path.clone();
            self.project.toggle(&path);
            self.rows = self.project.rows();
        } else if !expand && let Some(parent) = row.entry.path.parent().map(|p| p.to_path_buf()) {
            // ← on a file or a collapsed folder: go to the parent folder.
            if let Some(pix) = self.rows.iter().position(|r| r.entry.path == parent) {
                self.select_row(pix, cx);
            }
        }
        cx.notify();
    }

    pub(super) fn tree_collapse(&mut self, _: &TreeCollapse, _: &mut Window, cx: &mut Context<Self>) {
        self.set_expanded(false, cx);
    }

    pub(super) fn tree_expand(&mut self, _: &TreeExpand, _: &mut Window, cx: &mut Context<Self>) {
        self.set_expanded(true, cx);
    }

    pub(super) fn open_tree_search(&mut self, initial: &str, window: &mut Window, cx: &mut Context<Self>) {
        let field = match &self.tree_nav.search {
            Some(field) => field.clone(),
            None => {
                let field = cx.new(|cx| TextField::new("Search in tree", cx));
                self.tree_nav._subscription = Some(cx.subscribe(&field, |this, field, _: &FieldEvent, cx| {
                    // Jump to the first match from the top on every change.
                    let query = field.read(cx).text().to_string();
                    if let Some(ix) = (0..this.rows.len()).find(|&ix| matches(&this.rows[ix].entry.name, &query)) {
                        this.select_row(ix, cx);
                    }
                    cx.notify();
                }));
                self.tree_nav.search = Some(field.clone());
                field
            }
        };
        if !initial.is_empty() {
            field.update(cx, |f, cx| f.set_text(format!("{}{initial}", f.text()), cx));
        }
        window.focus(&field.focus_handle(cx));
        cx.notify();
    }

    pub(super) fn tree_search(&mut self, _: &TreeSearch, window: &mut Window, cx: &mut Context<Self>) {
        self.open_tree_search("", window, cx);
    }

    fn close_tree_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.tree_nav.search = None;
        self.tree_nav._subscription = None;
        window.focus(&self.tree_nav.focus);
        cx.notify();
    }

    pub(super) fn tree_cancel(&mut self, _: &TreeCancel, window: &mut Window, cx: &mut Context<Self>) {
        self.close_tree_search(window, cx);
    }

    /// Typing a letter in the focused tree starts a speed search with it.
    pub(super) fn tree_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = &event.keystroke;
        if k.modifiers.platform || k.modifiers.control || k.modifiers.alt || self.tree_nav.search.is_some() {
            return;
        }
        if let Some(ch) = k.key_char.as_deref().filter(|c| c.chars().count() == 1 && c.chars().all(|c| !c.is_control() && c != ' ')) {
            self.open_tree_search(ch, window, cx);
            cx.stop_propagation();
        }
    }

    /// The search bar shown above the tree while searching.
    pub(super) fn render_tree_search(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let field = self.tree_nav.search.clone()?;
        let query = field.read(cx).text().to_string();
        let count = self.rows.iter().filter(|r| matches(&r.entry.name, &query)).count();
        Some(
            div()
                .key_context("TreeSearch")
                .flex()
                .items_center()
                .gap_2()
                .px_2()
                .py_1()
                .border_b_1()
                .border_color(theme::border())
                .child(crate::icons::icon(crate::icons::Icon::search, 13., theme::muted()))
                .child(div().flex_1().flex().child(field))
                .child(div().text_size(px(11.)).text_color(if count == 0 && !query.is_empty() { theme::error() } else { theme::muted() }).child(if query.is_empty() {
                    String::new()
                } else {
                    format!("{count} match{}", if count == 1 { "" } else { "es" })
                })),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speed_search() {
        assert!(matches("UserController.php", "userc"));
        assert!(matches("UserController.php", "ucp"));
        assert!(!matches("UserController.php", "xyz"));
        assert!(!matches("anything", ""));
    }
}
