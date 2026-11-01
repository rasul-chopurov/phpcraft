//! Tool window layout like PhpStorm: icon stripes on the left and right edges (with
//! tooltips), side panels you can drag to the other edge, and resizable side and bottom
//! panels. Saved in `<config dir>/layout.json`.

use std::collections::HashMap;

use gpui::{
    AnyElement, AppContext, Context, DragMoveEvent, Pixels, Render, SharedString, Window, div, prelude::*, px,
};
use serde::{Deserialize, Serialize};

use super::Workspace;
use crate::theme;

pub(super) const STRIPE_WIDTH: f32 = 34.;
const STATUS_HEIGHT: f32 = 26.;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ToolWindow {
    Project,
    Commit,
    Ai,
    Terminal,
    Git,
}

impl ToolWindow {
    const SIDE_TOOLS: [ToolWindow; 3] = [ToolWindow::Project, ToolWindow::Commit, ToolWindow::Ai];

    fn icon(self) -> &'static str {
        match self {
            ToolWindow::Project => "▤",
            ToolWindow::Commit => "✓",
            ToolWindow::Ai => "✦",
            ToolWindow::Terminal => "›_",
            ToolWindow::Git => "⎇",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            ToolWindow::Project => "Project",
            ToolWindow::Commit => "Commit",
            ToolWindow::Ai => "AI Assistant",
            ToolWindow::Terminal => "Terminal",
            ToolWindow::Git => "Git",
        }
    }

    fn shortcut(self) -> &'static str {
        match self {
            ToolWindow::Project => "⌘1",
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
    pub left_width: f32,
    pub right_width: f32,
    pub bottom_height: f32,
}

impl Default for Layout {
    fn default() -> Self {
        Self {
            docks: HashMap::from([(ToolWindow::Project, Side::Left), (ToolWindow::Commit, Side::Left), (ToolWindow::Ai, Side::Right)]),
            left: Some(ToolWindow::Project),
            right: None,
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
        // The AI panel starts closed: it needs setup and a session.
        if layout.left == Some(ToolWindow::Ai) {
            layout.left = None;
        }
        if layout.right == Some(ToolWindow::Ai) {
            layout.right = None;
        }
        layout
    }

    pub fn save(&self) {
        let _ = std::fs::create_dir_all(crate::config::dir());
        let _ = std::fs::write(Self::path(), serde_json::to_string_pretty(self).unwrap_or_default());
    }

    pub fn side(&self, tool: ToolWindow) -> Side {
        match tool {
            ToolWindow::Terminal | ToolWindow::Git => Side::Bottom,
            _ => self.docks.get(&tool).copied().unwrap_or(Side::Left),
        }
    }

    fn slot(&mut self, side: Side) -> Option<&mut Option<ToolWindow>> {
        match side {
            Side::Left => Some(&mut self.left),
            Side::Right => Some(&mut self.right),
            Side::Bottom => None,
        }
    }

    pub fn is_active(&self, tool: ToolWindow) -> bool {
        self.left == Some(tool) || self.right == Some(tool)
    }

    /// Shows a side tool (replacing what was open on its edge).
    pub fn show(&mut self, tool: ToolWindow) {
        let side = self.side(tool);
        if let Some(slot) = self.slot(side) {
            *slot = Some(tool);
        }
    }

    pub fn hide(&mut self, tool: ToolWindow) {
        if self.left == Some(tool) {
            self.left = None;
        }
        if self.right == Some(tool) {
            self.right = None;
        }
    }

    /// Moves a side tool to another edge, keeping it open if it was.
    fn dock(&mut self, tool: ToolWindow, side: Side) {
        if side == Side::Bottom || self.side(tool) == side {
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
        match tool {
            ToolWindow::Terminal => self.terminals.open,
            ToolWindow::Git => self.git.log_open,
            _ => self.layout.is_active(tool),
        }
    }

    /// Opens or closes a tool window (stripe click).
    pub(super) fn toggle_tool(&mut self, tool: ToolWindow, window: &mut Window, cx: &mut Context<Self>) {
        match tool {
            ToolWindow::Terminal => self.toggle_terminal(&super::terminal_panel::ToggleTerminal, window, cx),
            ToolWindow::Git => self.toggle_git_log(&super::git_panel::ToggleGitLog, window, cx),
            ToolWindow::Ai => self.toggle_agent(&super::agent_panel::ToggleAgent, window, cx),
            ToolWindow::Project | ToolWindow::Commit => {
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
            .tooltip(tooltip(format!("{}  {}{}", tool.title(), tool.shortcut(), if self.layout.side(tool) == Side::Bottom { "" } else { "  ·  drag to the other edge" })))
            .on_click(cx.listener(move |this, _, window, cx| this.toggle_tool(tool, window, cx)));
        if self.layout.side(tool) != Side::Bottom {
            button = button.on_drag(DraggedTool(tool), |drag, _, _, cx| cx.new(|_| DragPreview(drag.0)));
        }
        button.into_any_element()
    }

    /// The icon column on one edge; dropping a tool icon here docks it on this edge.
    pub(super) fn render_stripe(&self, side: Side, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let top: Vec<AnyElement> =
            ToolWindow::SIDE_TOOLS.iter().filter(|t| self.layout.side(**t) == side).map(|t| self.stripe_button(*t, cx)).collect();
        let bottom: Vec<AnyElement> = if side == Side::Left {
            [ToolWindow::Terminal, ToolWindow::Git].iter().map(|t| self.stripe_button(*t, cx)).collect()
        } else {
            Vec::new()
        };
        div()
            .id(if side == Side::Left { "stripe-left" } else { "stripe-right" })
            .w(px(STRIPE_WIDTH))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_between()
            .py_1()
            .bg(theme::panel_bg())
            .when(side == Side::Left, |d| d.border_r_1())
            .when(side == Side::Right, |d| d.border_l_1())
            .border_color(theme::border())
            .drag_over::<DraggedTool>(|style, _, _, _| style.bg(theme::selection()))
            .on_drop(cx.listener(move |this, dragged: &DraggedTool, _, cx| {
                this.layout.dock(dragged.0, side);
                this.layout.save();
                cx.notify();
            }))
            .child(div().flex().flex_col().gap_1().children(top))
            .child(div().flex().flex_col().gap_1().children(bottom))
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
        let tool = match side {
            Side::Left => self.layout.left?,
            Side::Right => self.layout.right?,
            Side::Bottom => return None,
        };
        let content: AnyElement = match tool {
            ToolWindow::Project => self.render_project_tree(cx).into_any_element(),
            ToolWindow::Commit => self.render_commit_panel(cx).into_any_element(),
            ToolWindow::Ai => self.render_agent_panel(cx)?.into_any_element(),
            _ => return None,
        };
        let width = if side == Side::Left { self.layout.left_width } else { self.layout.right_width };
        Some(
            div()
                .relative()
                .w(px(width))
                .h_full()
                .flex_none()
                .flex()
                .flex_col()
                .bg(theme::panel_bg())
                .when(side == Side::Left, |d| d.border_r_1())
                .when(side == Side::Right, |d| d.border_l_1())
                .border_color(theme::border())
                .text_size(px(13.))
                .child(
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
                        ),
                )
                .child(div().flex_1().min_h(px(0.)).flex().flex_col().child(content))
                .child(self.resize_handle(side))
                .into_any_element(),
        )
    }
}
