//! A window holding one or more projects as tabs (like macOS window tabs in PhpStorm).
//! Each tab is a whole workspace; the tab bar shows when two or more are open.

use std::path::Path;

use gpui::{AppContext, Context, Entity, Focusable, SharedString, Subscription, Window, actions, div, prelude::*, px};

actions!(phpcraft, [NextProject, PrevProject]);
use project::Project;

use crate::theme;
use crate::workspace::{Workspace, WorkspaceEvent};

pub struct ProjectWindow {
    tabs: Vec<(Entity<Workspace>, Subscription)>,
    active: usize,
}

/// A workspace for `root`, remembered in Recent Projects.
pub fn new_workspace(root: &Path, window: &mut Window, cx: &mut gpui::App) -> anyhow::Result<Entity<Workspace>> {
    let project = Project::open(root)?;
    crate::config::remember_project(project.root());
    Ok(cx.new(|cx| {
        let mut workspace = Workspace::new(project, window, cx);
        workspace.start_git_polling(cx);
        workspace
    }))
}

static RETURN_TO_WELCOME: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Whether the window that just closed asked for the welcome window (and clears the request).
pub fn take_return_to_welcome() -> bool {
    RETURN_TO_WELCOME.swap(false, std::sync::atomic::Ordering::Relaxed)
}

impl ProjectWindow {
    pub fn new(workspace: Entity<Workspace>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        // The window's size and position are remembered under its first project.
        let root = workspace.read(cx).project_root().to_path_buf();
        cx.observe_window_bounds(window, move |_, window, _| crate::window_state::save(&root, window)).detach();
        let mut this = Self { tabs: Vec::new(), active: 0 };
        this.push(workspace, window, cx);
        this
    }

    /// Activates the tab showing `root`, if this window has one.
    pub fn show_project(&mut self, root: &Path, window: &mut Window, cx: &mut Context<Self>) -> Option<Entity<Workspace>> {
        let ix = self.tabs.iter().position(|(w, _)| w.read(cx).project_root() == root)?;
        self.activate(ix, window, cx);
        cx.notify();
        Some(self.tabs[ix].0.clone())
    }

    /// Opens another project as a tab.
    pub fn open_tab(&mut self, root: &Path, window: &mut Window, cx: &mut Context<Self>) {
        match new_workspace(root, window, cx) {
            Ok(workspace) => self.push(workspace, window, cx),
            Err(err) => eprintln!("open project: {err:#}"),
        }
    }

    fn push(&mut self, workspace: Entity<Workspace>, window: &mut Window, cx: &mut Context<Self>) {
        let subscription = cx.subscribe_in(&workspace, window, |this, workspace, event: &WorkspaceEvent, window, cx| this.on_event(workspace, event, window, cx));
        self.tabs.push((workspace, subscription));
        self.activate(self.tabs.len() - 1, window, cx);
    }

    fn on_event(&mut self, workspace: &Entity<Workspace>, event: &WorkspaceEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ix) = self.tabs.iter().position(|(w, _)| w == workspace) else { return };
        match event {
            WorkspaceEvent::OpenTab(path) => self.open_tab(path, window, cx),
            WorkspaceEvent::Replace(path) => match new_workspace(path, window, cx) {
                Ok(new) => {
                    let subscription = cx.subscribe_in(&new, window, |this, workspace, event: &WorkspaceEvent, window, cx| this.on_event(workspace, event, window, cx));
                    self.tabs[ix] = (new, subscription);
                    self.activate(ix, window, cx);
                }
                Err(err) => eprintln!("open project: {err:#}"),
            },
            WorkspaceEvent::Close => self.close(ix, window, cx),
        }
    }

    fn activate(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.active = ix.min(self.tabs.len().saturating_sub(1));
        if let Some(workspace) = self.tabs.get(self.active).map(|(w, _)| w.clone()) {
            workspace.update(cx, |w, cx| w.focus_in(window, cx));
        }
        self.update_title(window, cx);
        cx.notify();
    }

    fn close(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if ix < self.tabs.len() {
            let (workspace, _) = self.tabs.remove(ix);
            workspace.update(cx, |w, cx| w.save_all(cx));
        }
        if self.tabs.is_empty() {
            // Close Project on the last tab: the welcome window comes back (the window's close
            // button quits instead).
            RETURN_TO_WELCOME.store(true, std::sync::atomic::Ordering::Relaxed);
            window.remove_window();
            return;
        }
        self.activate(self.active.min(self.tabs.len() - 1), window, cx);
    }

    fn update_title(&self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((workspace, _)) = self.tabs.get(self.active) {
            window.set_window_title(&workspace.read(cx).title());
        }
    }
}

impl Render for ProjectWindow {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let active = self.active;
        let bar = (self.tabs.len() > 1).then(|| {
            div()
                .flex()
                .flex_none()
                .h(px(30.))
                .bg(theme::panel_bg())
                .border_b_1()
                .border_color(theme::border_strong())
                .text_size(px(12.))
                .children(self.tabs.iter().enumerate().map(|(ix, (workspace, _))| {
                    let name = workspace.read(cx).project_name();
                    div()
                        .id(("project-tab", ix))
                        .group("project-tab")
                        .flex_1()
                        .max_w(px(260.))
                        .flex()
                        .items_center()
                        .gap_2()
                        .px_3()
                        .border_r_1()
                        .border_color(theme::border())
                        .cursor_pointer()
                        .when(ix == active, |d| d.bg(theme::editor_bg()).text_color(theme::ui_text()))
                        .when(ix != active, |d| d.text_color(theme::muted()).hover(|d| d.bg(theme::hover())))
                        .child(crate::dialogs::avatar(&name, 16.))
                        .child(div().flex_1().overflow_hidden().whitespace_nowrap().child(SharedString::from(name)))
                        .child(
                            div()
                                .id(("project-tab-close", ix))
                                .invisible()
                                .group_hover("project-tab", |d| d.visible())
                                .px_1()
                                .rounded_sm()
                                .hover(|d| d.bg(theme::hover()))
                                .child("×")
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    cx.stop_propagation();
                                    this.close(ix, window, cx);
                                })),
                        )
                        .on_click(cx.listener(move |this, _, window, cx| this.activate(ix, window, cx)))
                }))
        });
        div()
            .on_action(cx.listener(|this, _: &NextProject, window, cx| {
                let next = (this.active + 1) % this.tabs.len().max(1);
                this.activate(next, window, cx);
            }))
            .on_action(cx.listener(|this, _: &PrevProject, window, cx| {
                let len = this.tabs.len().max(1);
                this.activate((this.active + len - 1) % len, window, cx);
            }))
            .size_full()
            .flex()
            .flex_col()
            .bg(theme::editor_bg())
            .children(bar)
            .children(self.tabs.get(active).map(|(w, _)| div().flex_1().min_h(px(0.)).child(w.clone())))
    }
}

impl Focusable for ProjectWindow {
    fn focus_handle(&self, cx: &gpui::App) -> gpui::FocusHandle {
        self.tabs[self.active].0.focus_handle(cx)
    }
}
