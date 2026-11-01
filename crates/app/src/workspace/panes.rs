//! Split editor: up to three editor panes side by side, each with its own tabs.
//!
//! `Workspace::tabs`/`active` always hold the focused pane; the others wait in `panes`
//! (the focused slot is `None`) and swap in when focused. A file is open in one pane at a
//! time, so two panes never edit separate copies of it.

use gpui::{AnyElement, Context, Entity, Focusable, MouseButton, SharedString, Window, actions, div, prelude::*, px};

use super::{Workspace, file_icon};
use crate::editor_view::EditorView;
use crate::theme;

actions!(workspace, [SplitRight, UnsplitPane, FocusNextPane]);

pub(super) const MAX_PANES: usize = 3;

pub(super) struct Pane {
    pub tabs: Vec<Entity<EditorView>>,
    pub active: usize,
}

impl Workspace {
    pub(super) fn pane_count(&self) -> usize {
        self.panes.len()
    }

    /// Makes pane `ix` the focused one (swapping its tabs in).
    pub(super) fn focus_pane(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if ix == self.focused_pane || ix >= self.panes.len() {
            return;
        }
        let current = Pane { tabs: std::mem::take(&mut self.tabs), active: self.active };
        self.panes[self.focused_pane] = Some(current);
        let next = self.panes[ix].take().unwrap_or(Pane { tabs: Vec::new(), active: 0 });
        self.tabs = next.tabs;
        self.active = next.active;
        self.focused_pane = ix;
        self.diff_active = false;
        match self.tabs.get(self.active) {
            Some(tab) => window.focus(&tab.focus_handle(cx)),
            None => window.focus(&self.focus_handle),
        }
        cx.notify();
    }

    /// The pane that has `path` open, if any (including the focused one).
    pub(super) fn pane_with(&self, path: &std::path::Path, cx: &gpui::App) -> Option<(usize, usize)> {
        if let Some(ix) = self.tabs.iter().position(|t| t.read(cx).buffer.path() == Some(path)) {
            return Some((self.focused_pane, ix));
        }
        self.panes.iter().enumerate().find_map(|(pane, slot)| {
            let tab = slot.as_ref()?.tabs.iter().position(|t| t.read(cx).buffer.path() == Some(path))?;
            Some((pane, tab))
        })
    }

    /// Adds an empty pane right of the focused one and focuses it.
    fn insert_pane_right(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.panes.len() >= MAX_PANES {
            return false;
        }
        let at = self.focused_pane + 1;
        self.panes.insert(at, Some(Pane { tabs: Vec::new(), active: 0 }));
        self.focus_pane(at, window, cx);
        true
    }

    /// ⌘\: moves the active tab into a new pane on the right (or opens an empty one).
    pub(super) fn split_right(&mut self, _: &SplitRight, window: &mut Window, cx: &mut Context<Self>) {
        if self.panes.len() >= MAX_PANES {
            return;
        }
        let moving = (self.tabs.len() > 1).then(|| {
            let tab = self.tabs.remove(self.active);
            self.active = self.active.min(self.tabs.len().saturating_sub(1));
            tab
        });
        if self.insert_pane_right(window, cx)
            && let Some(tab) = moving
        {
            self.tabs.push(tab);
            self.activate(0, window, cx);
        }
    }

    /// Opens a file in the pane right of the focused one (PhpStorm's "Open in Right Split").
    pub(super) fn open_in_right_split(&mut self, path: &std::path::Path, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((pane, tab)) = self.pane_with(path, cx) {
            self.focus_pane(pane, window, cx);
            self.activate(tab, window, cx);
            return;
        }
        if self.focused_pane + 1 < self.panes.len() {
            self.focus_pane(self.focused_pane + 1, window, cx);
        } else if !self.insert_pane_right(window, cx) {
            self.focus_pane(self.panes.len() - 1, window, cx);
        }
        self.open_file(path, window, cx);
    }

    /// Closes the focused pane when it has no tabs left (the first pane always stays).
    pub(super) fn drop_empty_pane(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.tabs.is_empty() || self.panes.len() <= 1 {
            return;
        }
        self.panes.remove(self.focused_pane);
        let next = self.focused_pane.min(self.panes.len() - 1);
        let pane = self.panes[next].take().unwrap_or(Pane { tabs: Vec::new(), active: 0 });
        self.tabs = pane.tabs;
        self.active = pane.active;
        self.focused_pane = next;
        if let Some(tab) = self.tabs.get(self.active) {
            window.focus(&tab.focus_handle(cx));
        }
        cx.notify();
    }

    pub(super) fn unsplit_pane(&mut self, _: &UnsplitPane, window: &mut Window, cx: &mut Context<Self>) {
        if self.panes.len() <= 1 {
            return;
        }
        // Move the focused pane's tabs into its neighbour, then drop it.
        let tabs = std::mem::take(&mut self.tabs);
        let target = if self.focused_pane > 0 { self.focused_pane - 1 } else { 1 };
        if let Some(Some(pane)) = self.panes.get_mut(target) {
            pane.tabs.extend(tabs);
        }
        self.drop_empty_pane(window, cx);
    }

    pub(super) fn focus_next_pane(&mut self, _: &FocusNextPane, window: &mut Window, cx: &mut Context<Self>) {
        let next = (self.focused_pane + 1) % self.panes.len().max(1);
        self.focus_pane(next, window, cx);
    }

    /// Tab bar and editor of a pane that isn't focused.
    pub(super) fn render_other_pane(&self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let Some(Some(pane)) = self.panes.get(ix) else { return div().into_any_element() };
        let tabs = pane.tabs.iter().enumerate().map(|(tab_ix, tab)| {
            let buffer = &tab.read(cx).buffer;
            let title: SharedString = buffer.title().into();
            let icon = file_icon(&buffer.title(), false, false);
            let active = tab_ix == pane.active;
            div()
                .id(("pane-tab", ix * 1000 + tab_ix))
                .flex()
                .items_center()
                .gap_1p5()
                .px_3()
                .h_full()
                .border_r_1()
                .border_color(theme::border())
                .cursor_pointer()
                .when(active, |d| d.bg(theme::tab_active().opacity(0.6)).text_color(theme::ui_text()))
                .when(!active, |d| d.text_color(theme::muted()).hover(|d| d.bg(theme::hover())))
                .child(icon)
                .child(title)
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.focus_pane(ix, window, cx);
                    this.activate(tab_ix, window, cx);
                }))
        });
        let editor = pane.tabs.get(pane.active).cloned();
        div()
            .flex_1()
            .min_w(px(0.))
            .flex()
            .flex_col()
            .border_l_1()
            .border_color(theme::border_strong())
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, window, cx| this.focus_pane(ix, window, cx)))
            .child(
                div()
                    .id(("pane-tabs", ix))
                    .flex()
                    .flex_none()
                    .h(px(32.))
                    .w_full()
                    .overflow_x_scroll()
                    .bg(theme::panel_bg())
                    .border_b_1()
                    .border_color(theme::border())
                    .text_size(px(13.))
                    .children(tabs),
            )
            .child(match editor {
                Some(editor) => div().flex_1().overflow_hidden().opacity(0.92).child(editor).into_any_element(),
                None => div().flex_1().bg(theme::editor_bg()).into_any_element(),
            })
            .into_any_element()
    }
}
