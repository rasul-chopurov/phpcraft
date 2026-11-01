//! Project switcher (title bar): Open…, Clone Repository…, open and recent projects.

use std::path::{Path, PathBuf};

use gpui::{App, Context, Entity, EventEmitter, FocusHandle, Focusable, Hsla, SharedString, Window, div, prelude::*, px};

use super::{Cancel, Confirm, DialogEvent, SelectNext, SelectPrev, button, card};
use crate::text_field::TextField;
use crate::theme;

/// `erp-server` -> `ES`, `akta2026` -> `A`.
pub fn initials(name: &str) -> String {
    let parts: Vec<&str> = name.split(['-', '_', '.', ' ']).filter(|p| !p.is_empty()).collect();
    let letters: String = parts.iter().take(2).filter_map(|p| p.chars().next()).collect();
    letters.to_uppercase()
}

/// A stable colour per project name.
pub fn avatar_color(name: &str) -> Hsla {
    let hash = name.bytes().fold(0u32, |h, b| h.wrapping_mul(31).wrapping_add(u32::from(b)));
    gpui::hsla((hash % 360) as f32 / 360., 0.45, 0.45, 1.)
}

pub fn avatar(name: &str, size: f32) -> gpui::Div {
    div()
        .size(px(size))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded_md()
        .bg(avatar_color(name))
        .text_color(gpui::white())
        .text_size(px(size * 0.45))
        .child(initials(name))
}

pub fn project_name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// `~/qorug/erp-server`
pub fn tilde(path: &Path) -> String {
    match dirs::home_dir().and_then(|home| path.strip_prefix(&home).ok().map(Path::to_path_buf)) {
        Some(rel) => format!("~/{}", rel.display()),
        None => path.display().to_string(),
    }
}

enum Row {
    Open,
    Clone,
    Header(&'static str),
    Project(PathBuf, bool),
}

pub struct ProjectDialog {
    current: PathBuf,
    recent: Vec<PathBuf>,
    selected: usize,
    /// Clone Repository mode: URL and parent directory fields.
    clone: Option<(Entity<TextField>, Entity<TextField>)>,
    focus_handle: FocusHandle,
}

impl EventEmitter<DialogEvent> for ProjectDialog {}

impl ProjectDialog {
    pub fn new(current: &Path, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        window.focus(&focus_handle);
        let recent = crate::config::recent_projects().into_iter().filter(|p| p != current).collect();
        Self { current: current.to_path_buf(), recent, selected: 0, clone: None, focus_handle }
    }

    /// Opens directly in Clone Repository mode (welcome screen).
    pub fn clone_repository(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut dialog = Self::new(Path::new(""), window, cx);
        dialog.activate(1, window, cx);
        dialog
    }

    fn rows(&self) -> Vec<Row> {
        let mut rows = vec![Row::Open, Row::Clone, Row::Header("Open Projects"), Row::Project(self.current.clone(), true)];
        if !self.recent.is_empty() {
            rows.push(Row::Header("Recent Projects"));
            rows.extend(self.recent.iter().map(|p| Row::Project(p.clone(), false)));
        }
        rows
    }

    fn selectable(&self) -> Vec<usize> {
        self.rows().iter().enumerate().filter(|(_, r)| !matches!(r, Row::Header(_))).map(|(i, _)| i).collect()
    }

    fn activate(&mut self, row: usize, window: &mut Window, cx: &mut Context<Self>) {
        match self.rows().into_iter().nth(row) {
            Some(Row::Open) => cx.emit(DialogEvent::PickProjectFolder),
            Some(Row::Clone) => {
                let parent = dirs::home_dir().map(|h| h.join("Projects")).unwrap_or_default().display().to_string();
                let url = cx.new(|cx| TextField::new("https://github.com/owner/repo.git", cx));
                let dir = cx.new(|cx| {
                    let mut f = TextField::new("Directory", cx);
                    f.set_text(parent, cx);
                    f
                });
                window.focus(&url.focus_handle(cx));
                self.clone = Some((url, dir));
                cx.notify();
            }
            Some(Row::Project(path, false)) => cx.emit(DialogEvent::OpenProject(path, super::OpenMode::Replace)),
            Some(Row::Project(_, true)) => cx.emit(DialogEvent::Dismiss),
            _ => {}
        }
    }

    fn select_prev(&mut self, _: &SelectPrev, _: &mut Window, cx: &mut Context<Self>) {
        let selectable = self.selectable();
        if let Some(pos) = selectable.iter().position(|&i| i == self.selected) {
            self.selected = selectable[pos.saturating_sub(1)];
        }
        cx.notify();
    }

    fn select_next(&mut self, _: &SelectNext, _: &mut Window, cx: &mut Context<Self>) {
        let selectable = self.selectable();
        if let Some(pos) = selectable.iter().position(|&i| i == self.selected) {
            self.selected = selectable[(pos + 1).min(selectable.len() - 1)];
        }
        cx.notify();
    }

    fn confirm(&mut self, _: &Confirm, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((url, dir)) = &self.clone {
            let url = url.read(cx).text().trim().to_string();
            let parent = PathBuf::from(dir.read(cx).text().trim());
            if !url.is_empty() {
                cx.emit(DialogEvent::CloneRepository { url, parent });
            }
            return;
        }
        self.activate(self.selected, window, cx);
    }

    fn cancel(&mut self, _: &Cancel, _: &mut Window, cx: &mut Context<Self>) {
        if self.clone.take().is_some() {
            cx.notify();
        } else {
            cx.emit(DialogEvent::Dismiss);
        }
    }
}

impl Focusable for ProjectDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ProjectDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some((url, dir)) = &self.clone {
            return card(520.)
                .key_context("Dialog")
                .track_focus(&self.focus_handle)
                .on_action(cx.listener(Self::confirm))
                .on_action(cx.listener(Self::cancel))
                .child(div().px_3().py_2().border_b_1().border_color(theme::border()).child("Clone Repository"))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .p_3()
                        .child(div().flex().items_center().gap_2().child(div().w(px(80.)).text_color(theme::muted()).child("URL")).child(div().flex_1().child(url.clone())))
                        .child(div().flex().items_center().gap_2().child(div().w(px(80.)).text_color(theme::muted()).child("Directory")).child(div().flex_1().child(dir.clone()))),
                )
                .child(
                    div()
                        .flex()
                        .justify_end()
                        .gap_2()
                        .p_3()
                        .child(button("clone-cancel", "Cancel", false).on_click(cx.listener(|this, _, window, cx| this.cancel(&Cancel, window, cx))))
                        .child(button("clone-ok", "Clone", true).on_click(cx.listener(|this, _, window, cx| this.confirm(&Confirm, window, cx)))),
                );
        }
        let rows: Vec<gpui::AnyElement> = self
            .rows()
            .into_iter()
            .enumerate()
            .map(|(ix, row)| {
                let base = div()
                    .id(("project-row", ix))
                    .mx_1()
                    .px_2()
                    .flex()
                    .items_center()
                    .gap_2()
                    .rounded_md()
                    .cursor_pointer()
                    .when(ix == self.selected, |d| d.bg(theme::selection()))
                    .hover(|d| d.bg(theme::hover()))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.selected = ix;
                        this.activate(ix, window, cx);
                    }));
                match row {
                    Row::Open => base.h(px(28.)).child("▢").child("Open…").into_any_element(),
                    Row::Clone => base.h(px(28.)).child("⎇").child("Clone Repository…").into_any_element(),
                    Row::Header(title) => div()
                        .px_3()
                        .pt_2()
                        .pb_1()
                        .mt_1()
                        .border_t_1()
                        .border_color(theme::border())
                        .text_size(px(12.))
                        .text_color(theme::muted())
                        .child(title)
                        .into_any_element(),
                    Row::Project(path, current) => {
                        let name = project_name(&path);
                        let new_window = path.clone();
                        base.py_1()
                            .group("project-row")
                            .child(avatar(&name, 26.))
                            .child(
                                div()
                                    .flex_1()
                                    .flex()
                                    .flex_col()
                                    .child(div().text_color(theme::ui_text()).child(SharedString::from(name.clone())))
                                    .child(div().text_size(px(11.)).text_color(theme::muted()).child(tilde(&path))),
                            )
                            .when(!current, |d| {
                                d.child(
                                    div()
                                        .id(("project-new-window", ix))
                                        .invisible()
                                        .group_hover("project-row", |d| d.visible())
                                        .px_1()
                                        .rounded_sm()
                                        .text_color(theme::muted())
                                        .hover(|d| d.bg(theme::hover()).text_color(theme::ui_text()))
                                        .child("↗")
                                        .tooltip(|_, cx| cx.new(|_| crate::dialogs::TextTooltip("Open in New Window".into())).into())
                                        .on_click(cx.listener(move |_, _, _, cx| {
                                            cx.stop_propagation();
                                            cx.emit(DialogEvent::OpenProject(new_window.clone(), super::OpenMode::NewWindow));
                                        })),
                                )
                                .child({
                                    let new_tab = path.clone();
                                    div()
                                        .id(("project-new-tab", ix))
                                        .invisible()
                                        .group_hover("project-row", |d| d.visible())
                                        .px_1()
                                        .rounded_sm()
                                        .text_color(theme::muted())
                                        .hover(|d| d.bg(theme::hover()).text_color(theme::ui_text()))
                                        .child("⊞")
                                        .tooltip(|_, cx| cx.new(|_| crate::dialogs::TextTooltip("Open in New Tab".into())).into())
                                        .on_click(cx.listener(move |_, _, _, cx| {
                                            cx.stop_propagation();
                                            cx.emit(DialogEvent::OpenProject(new_tab.clone(), super::OpenMode::NewTab));
                                        }))
                                })
                            })
                            .into_any_element()
                    }
                }
            })
            .collect();
        card(360.)
            .key_context("ListDialog")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::cancel))
            .on_action(cx.listener(Self::select_prev))
            .on_action(cx.listener(Self::select_next))
            .child(div().id("projects").max_h(px(560.)).overflow_y_scroll().py_1().text_size(px(13.)).children(rows))
    }
}
