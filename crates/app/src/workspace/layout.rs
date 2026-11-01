//! Tool window layout like PhpStorm: icon stripes on the left and right edges (with
//! tooltips), side panels you can drag to the other edge, and resizable side and bottom
//! panels. Saved in `<config dir>/layout.json`.

use std::collections::HashMap;

use gpui::{
    AnyElement, AppContext, Context, DragMoveEvent, Pixels, Render, SharedString, Window, div, prelude::*, px,
};
use serde::{Deserialize, Serialize};

use super::Workspace;

gpui::actions!(workspace, [ToggleStructure]);
use crate::theme;

pub(super) const STRIPE_WIDTH: f32 = 34.;
const STATUS_HEIGHT: f32 = 26.;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ToolWindow {
    Project,
    Structure,
    Modules,
    Database,
    Laravel,
    Commit,
    Ai,
    Terminal,
    Git,
}

impl ToolWindow {
    pub const ALL: [ToolWindow; 9] = [
        ToolWindow::Project,
        ToolWindow::Structure,
        ToolWindow::Modules,
        ToolWindow::Database,
        ToolWindow::Commit,
        ToolWindow::Ai,
        ToolWindow::Terminal,
        ToolWindow::Git,
        ToolWindow::Laravel,
    ];

    fn icon(self) -> &'static str {
        match self {
            ToolWindow::Project => "▤",
            ToolWindow::Structure => "≣",
            ToolWindow::Modules => "⧉",
            ToolWindow::Database => "⛁",
            ToolWindow::Laravel => "◈",
            ToolWindow::Commit => "✓",
            ToolWindow::Ai => "✦",
            ToolWindow::Terminal => "›_",
            ToolWindow::Git => "⎇",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            ToolWindow::Project => "Project",
            ToolWindow::Structure => "Structure",
            ToolWindow::Modules => "Modules",
            ToolWindow::Database => "Database",
            ToolWindow::Laravel => "Laravel",
            ToolWindow::Commit => "Commit",
            ToolWindow::Ai => "AI Assistant",
            ToolWindow::Terminal => "Terminal",
            ToolWindow::Git => "Git",
        }
    }

    fn shortcut(self) -> &'static str {
        match self {
            ToolWindow::Project => "⌘1",
            ToolWindow::Structure => "⌘7",
            ToolWindow::Modules => "",
            ToolWindow::Database => "",
            ToolWindow::Laravel => "",
            ToolWindow::Commit => "⌘0",
            ToolWindow::Ai => "⌘⌥I",
            ToolWindow::Terminal => "⌥F12",
            ToolWindow::Git => "⌘9",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Side {
    Left,
    Right,
    Bottom,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Layout {
    /// Which edge each side tool window lives on.
    docks: HashMap<ToolWindow, Side>,
    pub left: Option<ToolWindow>,
    pub right: Option<ToolWindow>,
    pub bottom: Option<ToolWindow>,
    pub left_width: f32,
    pub right_width: f32,
    pub bottom_height: f32,
}

impl Default for Layout {
    fn default() -> Self {
        Self {
            docks: HashMap::from([
                (ToolWindow::Project, Side::Left),
                (ToolWindow::Commit, Side::Left),
                (ToolWindow::Ai, Side::Right),
                (ToolWindow::Terminal, Side::Bottom),
                (ToolWindow::Git, Side::Bottom),
            ]),
            left: Some(ToolWindow::Project),
            right: None,
            bottom: None,
            left_width: 280.,
            right_width: 400.,
            bottom_height: 300.,
        }
    }
}

impl Layout {
    fn path() -> std::path::PathBuf {
        crate::config::dir().join("layout.json")
    }

    pub fn load() -> Self {
        let mut layout: Self = std::fs::read_to_string(Self::path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        // Panels that need a running session start closed.
        for tool in [ToolWindow::Ai, ToolWindow::Terminal, ToolWindow::Git, ToolWindow::Database, ToolWindow::Laravel] {
            layout.hide(tool);
        }
        layout
    }

    pub fn save(&self) {
        let _ = std::fs::create_dir_all(crate::config::dir());
        let _ = std::fs::write(Self::path(), serde_json::to_string_pretty(self).unwrap_or_default());
    }

    pub fn side(&self, tool: ToolWindow) -> Side {
        let default = match tool {
            ToolWindow::Terminal | ToolWindow::Git | ToolWindow::Laravel => Side::Bottom,
            ToolWindow::Ai | ToolWindow::Database => Side::Right,
            _ => Side::Left,
        };
        self.docks.get(&tool).copied().unwrap_or(default)
    }

    fn slot(&mut self, side: Side) -> &mut Option<ToolWindow> {
        match side {
            Side::Left => &mut self.left,
            Side::Right => &mut self.right,
            Side::Bottom => &mut self.bottom,
        }
    }

    pub fn active(&self, side: Side) -> Option<ToolWindow> {
        match side {
            Side::Left => self.left,
            Side::Right => self.right,
            Side::Bottom => self.bottom,
        }
    }

    pub fn is_active(&self, tool: ToolWindow) -> bool {
        self.left == Some(tool) || self.right == Some(tool) || self.bottom == Some(tool)
    }

    /// Shows a tool on its edge (replacing what was open there).
    pub fn show(&mut self, tool: ToolWindow) {
        let side = self.side(tool);
        *self.slot(side) = Some(tool);
    }

    pub fn hide(&mut self, tool: ToolWindow) {
        for slot in [&mut self.left, &mut self.right, &mut self.bottom] {
            if *slot == Some(tool) {
                *slot = None;
            }
        }
    }

    /// Moves a tool to another edge, keeping it open if it was.
    fn dock(&mut self, tool: ToolWindow, side: Side) {
        if self.side(tool) == side {
            return;
        }
        let was_open = self.is_active(tool);
        self.hide(tool);
        self.docks.insert(tool, side);
        if was_open {
            self.show(tool);
        }
    }
}

/// A tool icon being dragged to another stripe.
#[derive(Clone)]
pub(super) struct DraggedTool(ToolWindow);

/// The edge of a panel being dragged to resize it.
#[derive(Clone)]
pub(super) struct ResizeHandle(Side);

struct DragPreview(ToolWindow);

impl Render for DragPreview {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_2()
            .py_1()
            .rounded_md()
            .bg(theme::panel_bg())
            .border_1()
            .border_color(theme::accent())
            .text_size(px(12.))
            .text_color(theme::ui_text())
            .child(format!("{} {}", self.0.icon(), self.0.title()))
    }
}

struct Invisible;

impl Render for Invisible {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

pub(super) struct Tooltip(SharedString);

impl Render for Tooltip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_2()
            .py_1()
            .rounded_md()
            .bg(theme::panel_bg())
            .border_1()
            .border_color(theme::border_strong())
            .shadow_md()
            .text_size(px(12.))
            .text_color(theme::ui_text())
            .child(self.0.clone())
    }
}

/// `.tooltip(tooltip("…"))`
pub(super) fn tooltip(text: impl Into<SharedString>) -> impl Fn(&mut Window, &mut gpui::App) -> gpui::AnyView + 'static {
    let text = text.into();
    move |_, cx| cx.new(|_| Tooltip(text.clone())).into()
}

impl Workspace {
    pub(super) fn tool_open(&self, tool: ToolWindow) -> bool {
        self.layout.is_active(tool)
    }

    /// Opens or closes a tool window (stripe click).
    pub(super) fn toggle_tool(&mut self, tool: ToolWindow, window: &mut Window, cx: &mut Context<Self>) {
        match tool {
            // A stripe click always shows or hides (⌥F12 also moves focus).
            ToolWindow::Terminal if self.layout.is_active(tool) => {
                self.layout.hide(tool);
                self.focus_active(window, cx);
            }
            ToolWindow::Terminal => self.toggle_terminal(&super::terminal_panel::ToggleTerminal, window, cx),
            ToolWindow::Git => self.toggle_git_log(&super::git_panel::ToggleGitLog, window, cx),
            ToolWindow::Ai => self.toggle_agent(&super::agent_panel::ToggleAgent, window, cx),
            ToolWindow::Project | ToolWindow::Commit | ToolWindow::Structure | ToolWindow::Modules | ToolWindow::Database | ToolWindow::Laravel => {
                if tool == ToolWindow::Database {
                    self.ensure_db(cx);
                }
                if tool == ToolWindow::Laravel {
                    self.ensure_laravel_panel(cx);
                }
                if self.layout.is_active(tool) {
                    self.layout.hide(tool);
                    self.focus_active(window, cx);
                } else {
                    self.layout.show(tool);
                    if tool == ToolWindow::Commit {
                        self.show_commit(&super::git_panel::ShowCommit, window, cx);
                    }
                }
                self.layout.save();
            }
        }
        cx.notify();
    }

    fn stripe_button(&self, tool: ToolWindow, cx: &mut Context<Self>) -> AnyElement {
        let open = self.tool_open(tool);
        let changes = if tool == ToolWindow::Commit { self.git.status.len() } else { 0 };
        let mut button = div()
            .id(SharedString::from(format!("stripe-{tool:?}")))
            .relative()
            .w(px(26.))
            .h(px(26.))
            .flex()
            .items_center()
            .justify_center()
            .rounded_md()
            .cursor_pointer()
            .text_size(px(14.))
            .when(open, |d| d.bg(theme::selection()).text_color(theme::ui_text()))
            .when(!open, |d| d.text_color(theme::muted()).hover(|d| d.bg(theme::hover()).text_color(theme::ui_text())))
            .child(tool.icon())
            .when(changes > 0, |d| {
                d.child(div().absolute().top(px(2.)).right(px(2.)).size(px(6.)).rounded_full().bg(theme::accent()))
            })
            .tooltip(tooltip(format!("{}  {}  ·  drag to another edge", tool.title(), tool.shortcut())))
            .on_click(cx.listener(move |this, _, window, cx| this.toggle_tool(tool, window, cx)));
        button = button.on_drag(DraggedTool(tool), |drag, _, _, cx| cx.new(|_| DragPreview(drag.0)));
        button.into_any_element()
    }

    fn drop_zone(&self, side: Side, id: &'static str, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let has_modules = crate::modules::has_modules(self.project.root());
        let buttons: Vec<AnyElement> = ToolWindow::ALL
            .iter()
            .filter(|t| self.layout.side(**t) == side && (**t != ToolWindow::Modules || has_modules) && (!matches!(**t, ToolWindow::Database | ToolWindow::Laravel) || self.intel.is_laravel()))
            .map(|t| self.stripe_button(*t, cx))
            .collect();
        div()
            .id(id)
            .flex()
            .flex_col()
            .items_center()
            .gap_1()
            .min_h(px(60.))
            .w_full()
            .rounded_md()
            .drag_over::<DraggedTool>(|style, _, _, _| style.bg(theme::selection()))
            .on_drop(cx.listener(move |this, dragged: &DraggedTool, _, cx| {
                this.layout.dock(dragged.0, side);
                this.layout.save();
                cx.notify();
            }))
            .children(buttons)
    }

    /// The icon column on one edge. The left column's lower part holds the bottom panels;
    /// dropping a tool icon on a zone docks it there.
    pub(super) fn render_stripe(&self, side: Side, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let stripe = div()
            .w(px(STRIPE_WIDTH))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_between()
            .p_1()
            .bg(theme::panel_bg())
            .when(side == Side::Left, |d| d.border_r_1())
            .when(side == Side::Right, |d| d.border_l_1())
            .border_color(theme::border());
        match side {
            Side::Left => stripe
                .child(self.drop_zone(Side::Left, "zone-left", cx).flex_1())
                .child(self.drop_zone(Side::Bottom, "zone-bottom", cx).border_t_1().border_color(theme::border()).pt_1()),
            _ => stripe.child(self.drop_zone(Side::Right, "zone-right", cx).flex_1()),
        }
    }

    /// A thin handle on a panel edge; drag it to resize.
    pub(super) fn resize_handle(&self, side: Side) -> impl IntoElement + use<> {
        let handle = div()
            .id(match side {
                Side::Left => "resize-left",
                Side::Right => "resize-right",
                Side::Bottom => "resize-bottom",
            })
            .absolute()
            .occlude()
            .hover(|d| d.bg(theme::accent().opacity(0.5)))
            .on_drag(ResizeHandle(side), |_, _, _, cx| cx.new(|_| Invisible));
        match side {
            Side::Left => handle.top_0().bottom_0().right(px(-2.)).w(px(4.)).cursor_col_resize(),
            Side::Right => handle.top_0().bottom_0().left(px(-2.)).w(px(4.)).cursor_col_resize(),
            Side::Bottom => handle.left_0().right_0().top(px(-2.)).h(px(4.)).cursor_row_resize(),
        }
    }

    /// Root listener while a resize handle is dragged.
    pub(super) fn on_resize(&mut self, event: &DragMoveEvent<ResizeHandle>, _: &mut Window, cx: &mut Context<Self>) {
        let position = event.event.position;
        let bounds = event.bounds;
        let to_f32 = |p: Pixels| f32::from(p);
        match event.drag(cx).0 {
            Side::Left => self.layout.left_width = (to_f32(position.x - bounds.left()) - STRIPE_WIDTH).clamp(160., 900.),
            Side::Right => self.layout.right_width = (to_f32(bounds.right() - position.x) - STRIPE_WIDTH).clamp(220., 1000.),
            Side::Bottom => self.layout.bottom_height = (to_f32(bounds.bottom() - position.y) - STATUS_HEIGHT).clamp(120., 900.),
        }
        cx.notify();
    }

    /// The open tool window on one edge, with its title bar and resize handle.
    pub(super) fn render_side(&self, side: Side, cx: &mut Context<Self>) -> Option<AnyElement> {
        let tool = self.layout.active(side)?;
        let content: AnyElement = match tool {
            ToolWindow::Project => self.render_project_tree(cx).into_any_element(),
            ToolWindow::Structure => self.render_structure(cx).into_any_element(),
            ToolWindow::Modules => self.render_modules(cx).into_any_element(),
            ToolWindow::Database => self.render_database(cx).into_any_element(),
            ToolWindow::Laravel => self.render_laravel_panel(cx).into_any_element(),
            ToolWindow::Commit => self.render_commit_panel(cx).into_any_element(),
            ToolWindow::Ai => self.render_agent_panel(cx)?.into_any_element(),
            ToolWindow::Terminal => self.render_terminal_panel(cx)?.into_any_element(),
            ToolWindow::Git => self.render_git_log(cx)?.into_any_element(),
        };
        // Terminal and Git draw their own tab bars.
        let own_header = matches!(tool, ToolWindow::Terminal | ToolWindow::Git | ToolWindow::Laravel);
        let header = (!own_header).then(|| {
            div()
                .flex()
                .flex_none()
                .items_center()
                .h(px(30.))
                .px_3()
                .text_size(px(12.))
                .text_color(theme::muted())
                .child(div().flex_1().child(tool.title().to_uppercase()))
                .child(
                    div()
                        .id("side-hide")
                        .px_1()
                        .rounded_sm()
                        .cursor_pointer()
                        .hover(|d| d.bg(theme::hover()))
                        .child("—")
                        .tooltip(tooltip("Hide"))
                        .on_click(cx.listener(move |this, _, window, cx| this.toggle_tool(tool, window, cx))),
                )
        });
        let panel = div()
            .relative()
            .flex_none()
            .flex()
            .flex_col()
            .bg(theme::panel_bg())
            .border_color(theme::border_strong())
            .text_size(px(13.))
            .children(header)
            .child(div().flex_1().min_h(px(0.)).flex().flex_col().child(content))
            .child(self.resize_handle(side));
        Some(
            match side {
                Side::Left => panel.w(px(self.layout.left_width)).h_full().border_r_1(),
                Side::Right => panel.w(px(self.layout.right_width)).h_full().border_l_1(),
                Side::Bottom => panel.w_full().h(px(self.layout.bottom_height)).border_t_1(),
            }
            .into_any_element(),
        )
    }
}

impl Workspace {
    pub(super) fn toggle_structure(&mut self, _: &ToggleStructure, window: &mut Window, cx: &mut Context<Self>) {
        self.toggle_tool(ToolWindow::Structure, window, cx);
    }
}
