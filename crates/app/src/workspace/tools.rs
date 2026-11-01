//! Laravel tooling in the workspace: Make / Artisan dialogs, the toolbar and the output panel.

use std::path::PathBuf;
use std::sync::Arc;

use gpui::{AnyView, Context, Entity, SharedString, Window, actions, div, prelude::*, px};

use super::Workspace;
use crate::dialogs::{ArtisanDialog, ArtisanRunDialog, DialogEvent, MakeChooser, MakeDialog, RouteSearchDialog};
use laravel::make::MakeKind;
use crate::theme;

actions!(workspace, [OpenMake, OpenArtisan, CloseOutput, OpenRouteSearch, NewModel, NewPivotTable, ToggleLaravelMenu]);

/// Bottom panel showing Artisan output and generation results.
pub(super) struct Output {
    pub title: String,
    pub text: String,
    pub running: bool,
    pub ok: bool,
}

const MAX_OUTPUT_LINES: usize = 2000;

impl Workspace {
    pub(super) fn open_make(&mut self, _: &OpenMake, window: &mut Window, cx: &mut Context<Self>) {
        let root = self.project.root().to_path_buf();
        let dialog = cx.new(|cx| MakeChooser::new(&root, window, cx));
        self.show_modal(dialog, window, cx);
    }

    pub(super) fn new_model(&mut self, _: &NewModel, window: &mut Window, cx: &mut Context<Self>) {
        self.open_make_kind(MakeKind::Model, window, cx);
    }

    pub(super) fn new_pivot_table(&mut self, _: &NewPivotTable, window: &mut Window, cx: &mut Context<Self>) {
        self.open_make_kind(MakeKind::PivotTable, window, cx);
    }

    fn open_make_kind(&mut self, kind: MakeKind, window: &mut Window, cx: &mut Context<Self>) {
        let root = self.project.root().to_path_buf();
        let models = self.intel.models();
        let dialog = cx.new(|cx| MakeDialog::new(&root, kind, models, window, cx));
        self.show_modal(dialog, window, cx);
    }

    pub(super) fn open_route_search(&mut self, _: &OpenRouteSearch, window: &mut Window, cx: &mut Context<Self>) {
        let root = self.project.root().to_path_buf();
        let dialog = cx.new(|cx| RouteSearchDialog::new(&root, window, cx));
        self.show_modal(dialog, window, cx);
    }

    pub(super) fn toggle_laravel_menu(&mut self, _: &ToggleLaravelMenu, _: &mut Window, cx: &mut Context<Self>) {
        self.laravel_menu_open = !self.laravel_menu_open;
        cx.notify();
    }

    pub(super) fn open_artisan(&mut self, _: &OpenArtisan, window: &mut Window, cx: &mut Context<Self>) {
        let root = self.project.root().to_path_buf();
        let cached = self.artisan_commands.clone();
        let dialog = cx.new(|cx| ArtisanDialog::new(&root, cached, window, cx));
        self.show_modal(dialog, window, cx);
    }

    pub(super) fn show_modal<D: gpui::Render + gpui::EventEmitter<DialogEvent>>(
        &mut self,
        dialog: Entity<D>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.modal_subscription = Some(cx.subscribe_in(&dialog, window, |this, _, event, window, cx| {
            this.on_dialog_event(event, window, cx)
        }));
        self.modal = Some(AnyView::from(dialog));
        self.laravel_menu_open = false;
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
            DialogEvent::OpenMakeKind(kind) => self.open_make_kind(*kind, window, cx),
            DialogEvent::FilesListed(files) => self.files_cache = Some(files.clone()),
            DialogEvent::OpenLocation(location) => {
                let location = location.clone();
                self.dismiss_modal(window, cx);
                let from = self
                    .tabs
                    .get(self.active)
                    .and_then(|t| t.read(cx).buffer.path().map(|p| (p.to_path_buf(), t.read(cx).buffer.selection.head)));
                self.open_location(&location, from, window, cx);
            }
            DialogEvent::RunAction(action) => {
                let action = action.boxed_clone();
                self.dismiss_modal(window, cx);
                // After the modal closes, so editor actions reach the focused editor.
                window.defer(cx, move |window, cx| window.dispatch_action(action, cx));
            }
            DialogEvent::GoToLine(line, column) => {
                let (line, column) = (*line, *column);
                self.dismiss_modal(window, cx);
                self.jump_to_line(line, column, cx);
            }
            DialogEvent::ReplaceInFiles { query, replacement, paths } => {
                let (query, replacement, paths) = (query.clone(), replacement.clone(), paths.clone());
                self.dismiss_modal(window, cx);
                self.run_replace_in_files(query, replacement, paths, cx);
            }
            DialogEvent::OpenRoute(route) => {
                let location = self.intel.route_location(route);
                self.dismiss_modal(window, cx);
                if let Some(location) = location {
                    self.open_location(&location, None, window, cx);
                }
            }
            DialogEvent::ConfigureArtisan(command) => {
                let command = command.clone();
                let dialog = cx.new(|cx| ArtisanRunDialog::new(command, window, cx));
                self.show_modal(dialog, window, cx);
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
                d.child(
                    div()
                        .flex()
                        .flex_col()
                        .child(tool("tool-laravel", "◈ Laravel", "▾").on_click(cx.listener(|this, _, window, cx| {
                            this.toggle_laravel_menu(&ToggleLaravelMenu, window, cx)
                        })))
                        .children(self.render_laravel_menu(cx)),
                )
                .child(tool("tool-artisan", "▶ Artisan", "⌃⇧X").on_click(cx.listener(|this, _, window, cx| {
                    this.open_artisan(&OpenArtisan, window, cx)
                })))
            })
    }

    fn render_laravel_menu(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        if !self.laravel_menu_open {
            return None;
        }
        type Handler = fn(&mut Workspace, &mut Window, &mut Context<Workspace>);
        let entries: &[Option<(&'static str, &'static str, Option<Handler>)>] = &[
            Some(("Code Generation", "⇧⌘,", Some(|w, win, cx| w.open_make(&OpenMake, win, cx)))),
            Some(("New Eloquent Model", "", Some(|w, win, cx| w.new_model(&NewModel, win, cx)))),
            Some(("New Eloquent Model from Database Table", "soon", None)),
            Some(("New Class", "", Some(|w, win, cx| w.open_make(&OpenMake, win, cx)))),
            Some(("New Pivot Table", "", Some(|w, win, cx| w.new_pivot_table(&NewPivotTable, win, cx)))),
            None,
            Some(("Route Search", "⌃⇧R", Some(|w, win, cx| w.open_route_search(&OpenRouteSearch, win, cx)))),
            None,
            Some(("Run Artisan Command", "⌃⇧X", Some(|w, win, cx| w.open_artisan(&OpenArtisan, win, cx)))),
            None,
            Some(("Generate Helper Code", "soon", None)),
            Some(("Find Unused Views", "soon", None)),
            None,
            Some(("Laravel Settings…", "soon", None)),
        ];
        let items = entries.iter().enumerate().map(|(ix, entry)| match entry {
            None => div().my_1().h(px(1.)).bg(theme::border_strong()).into_any_element(),
            Some((label, hint, handler)) => {
                let enabled = handler.is_some();
                let handler = *handler;
                div()
                    .id(("laravel-menu", ix))
                    .flex()
                    .justify_between()
                    .gap_6()
                    .px_3()
                    .h(px(24.))
                    .items_center()
                    .when(enabled, |d| d.cursor_pointer().hover(|d| d.bg(theme::selection())))
                    .when(!enabled, |d| d.text_color(theme::muted()))
                    .child(*label)
                    .child(div().text_color(theme::muted()).child(*hint))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.laravel_menu_open = false;
                        if let Some(handler) = handler {
                            handler(this, window, cx);
                        }
                        cx.notify();
                    }))
                    .into_any_element()
            }
        });
        Some(
            gpui::deferred(
                gpui::anchored().child(
                    div()
                        .occlude()
                        .min_w(px(340.))
                        .py_1()
                        .bg(theme::panel_bg())
                        .border_1()
                        .border_color(theme::border_strong())
                        .rounded_md()
                        .shadow_lg()
                        .text_size(px(13.))
                        .children(items),
                ),
            )
            .with_priority(1),
        )
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
