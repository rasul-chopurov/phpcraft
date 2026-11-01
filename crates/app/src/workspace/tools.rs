//! Laravel tooling in the workspace: Make / Artisan dialogs, the toolbar and the output panel.

use std::path::PathBuf;
use std::sync::Arc;

use gpui::{AnyView, Context, Entity, SharedString, Window, actions, div, prelude::*, px};

use super::Workspace;
use crate::dialogs::{ArtisanDialog, DialogEvent, MakeDialog};
use crate::theme;

actions!(workspace, [OpenMake, OpenArtisan, CloseOutput]);

/// Bottom panel showing Artisan output and generation results.
pub struct Output {
    pub title: String,
    pub text: String,
    pub running: bool,
    pub ok: bool,
}

const MAX_OUTPUT_LINES: usize = 2000;

impl Workspace {
    pub(super) fn open_make(&mut self, _: &OpenMake, window: &mut Window, cx: &mut Context<Self>) {
        let root = self.project.root().to_path_buf();
        let dialog = cx.new(|cx| MakeDialog::new(&root, window, cx));
        self.show_modal(dialog, window, cx);
    }

    pub(super) fn open_artisan(&mut self, _: &OpenArtisan, window: &mut Window, cx: &mut Context<Self>) {
        let root = self.project.root().to_path_buf();
        let cached = self.artisan_commands.clone();
        let dialog = cx.new(|cx| ArtisanDialog::new(&root, cached, window, cx));
        self.show_modal(dialog, window, cx);
    }

    fn show_modal<D: gpui::Render + gpui::EventEmitter<DialogEvent>>(
        &mut self,
        dialog: Entity<D>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.modal_subscription = Some(cx.subscribe_in(&dialog, window, |this, _, event, window, cx| {
            this.on_dialog_event(event, window, cx)
        }));
        self.modal = Some(AnyView::from(dialog));
        cx.notify();
    }

    pub(super) fn dismiss_modal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.modal = None;
        self.modal_subscription = None;
        self.focus_active(window, cx);
        cx.notify();
    }

    fn on_dialog_event(&mut self, event: &DialogEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            DialogEvent::Dismiss => self.dismiss_modal(window, cx),
            DialogEvent::CommandsLoaded(list) => self.artisan_commands = Some(list.clone()),
            DialogEvent::Created(paths) => {
                self.dismiss_modal(window, cx);
                self.files_created(paths, window, cx);
                let root = self.project.root().to_path_buf();
                let list = paths
                    .iter()
                    .map(|p| format!("  + {}", p.strip_prefix(&root).unwrap_or(p).display()))
                    .collect::<Vec<_>>()
                    .join("\n");
                self.output = Some(Output {
                    title: "Laravel Make".into(),
                    text: format!("Created {} file(s):\n{list}", paths.len()),
                    running: false,
                    ok: true,
                });
            }
            DialogEvent::RunArtisan(line) => {
                self.dismiss_modal(window, cx);
                self.run_artisan(line.clone(), cx);
            }
        }
    }

    /// Reindexes new files, reveals them in the tree and opens the first one.
    fn files_created(&mut self, paths: &[PathBuf], window: &mut Window, cx: &mut Context<Self>) {
        for path in paths {
            if let Ok(source) = std::fs::read_to_string(path) {
                self.intel.update_file(path, &source);
            }
            self.project.reveal(path);
        }
        self.rows = self.project.rows();
        if let Some(first) = paths.first() {
            self.selected_path = Some(first.clone());
            self.open_file(first, window, cx);
        }
    }

    pub(super) fn run_artisan(&mut self, line: String, cx: &mut Context<Self>) {
        self.output = Some(Output {
            title: format!("php artisan {line}"),
            text: String::new(),
            running: true,
            ok: true,
        });
        cx.notify();
        let root = self.project.root().to_path_buf();
        cx.spawn(async move |this, cx| {
            let run_line = line.clone();
            let (text, ok) = cx
                .background_executor()
                .spawn(async move { laravel::artisan::run(&root, &run_line) })
                .await;
            this.update(cx, |this, cx| {
                this.output = Some(Output {
                    title: format!("php artisan {line}"),
                    text,
                    running: false,
                    ok,
                });
                // Generators and migrations may have created files.
                this.rows = this.project.rows();
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn close_output(&mut self, _: &CloseOutput, _: &mut Window, cx: &mut Context<Self>) {
        self.output = None;
        cx.notify();
    }

    pub(super) fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let tool = |id: &'static str, label: &'static str, shortcut: &'static str| {
            div()
                .id(id)
                .flex()
                .items_center()
                .gap_1()
                .px_2()
                .h(px(22.))
                .rounded_sm()
                .cursor_pointer()
                .hover(|d| d.bg(theme::hover()))
                .child(label)
                .child(div().text_color(theme::muted()).child(shortcut))
        };
        let laravel = self.intel.is_laravel();
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap_2()
            .px_2()
            .h(px(30.))
            .bg(theme::panel_bg())
            .border_b_1()
            .border_color(theme::border())
            .text_size(px(12.))
            .text_color(theme::ui_text())
            .child(div().px_1().child(SharedString::from(self.project.name())))
            .child(div().flex_1())
            .when(laravel, |d| {
                d.child(tool("tool-make", "⚡ Laravel Make", "⌘N").on_click(cx.listener(|this, _, window, cx| {
                    this.open_make(&OpenMake, window, cx)
                })))
                .child(tool("tool-artisan", "▶ Artisan", "⌃⇧X").on_click(cx.listener(|this, _, window, cx| {
                    this.open_artisan(&OpenArtisan, window, cx)
                })))
            })
    }

    pub(super) fn render_output(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let output = self.output.as_ref()?;
        let (status, color) = match (output.running, output.ok) {
            (true, _) => ("running…", theme::muted()),
            (false, true) => ("done", theme::success()),
            (false, false) => ("failed", theme::error()),
        };
        let lines: Vec<&str> = output.text.lines().collect();
        let skip = lines.len().saturating_sub(MAX_OUTPUT_LINES);
        let lines = lines[skip..].iter().map(|line| div().child(SharedString::from(line.to_string())));
        Some(
            div()
                .flex()
                .flex_col()
                .flex_none()
                .h(px(220.))
                .border_t_1()
                .border_color(theme::border_strong())
                .bg(theme::editor_bg())
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .px_2()
                        .h(px(26.))
                        .bg(theme::panel_bg())
                        .text_size(px(12.))
                        .child(div().text_color(theme::ui_text()).child(SharedString::from(output.title.clone())))
                        .child(div().text_color(color).child(status))
                        .child(div().flex_1())
                        .child(
                            div()
                                .id("close-output")
                                .px_1()
                                .cursor_pointer()
                                .text_color(theme::muted())
                                .hover(|d| d.text_color(theme::ui_text()))
                                .child("×")
                                .on_click(cx.listener(|this, _, window, cx| this.close_output(&CloseOutput, window, cx))),
                        ),
                )
                .child(
                    div()
                        .id("output-text")
                        .flex_1()
                        .overflow_y_scroll()
                        .p_2()
                        .font_family(theme::EDITOR_FONT)
                        .text_size(px(12.))
                        .text_color(theme::text())
                        .children(lines),
                ),
        )
    }

    pub(super) fn render_modal(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let modal = self.modal.clone()?;
        Some(
            div()
                .absolute()
                .inset_0()
                .bg(theme::overlay())
                .flex()
                .justify_center()
                .items_start()
                .pt(px(70.))
                .on_mouse_down(gpui::MouseButton::Left, cx.listener(|this, _, window, cx| this.dismiss_modal(window, cx)))
                .child(modal),
        )
    }
}

pub type CommandCache = Option<Arc<Vec<laravel::artisan::ArtisanCommand>>>;
