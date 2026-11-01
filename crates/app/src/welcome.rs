//! Welcome window (no project open): recent projects, Open / Clone / New Laravel project,
//! Customize (theme, font, settings import) and the ⚙ menu.

use std::path::{Path, PathBuf};

use gpui::{
    AnyView, App, AppContext, Context, Entity, FocusHandle, Focusable, SharedString, Subscription, Window, div, prelude::*, px,
};

use crate::config::{self, Settings};
use crate::dialogs::{DialogEvent, KeymapDialog, ProjectDialog, SettingsDialog, avatar, button, project_name, tilde};
use crate::settings_import::{self, Source};
use crate::terminal_view::{TerminalView, TerminalViewEvent};
use crate::text_field::{FieldEvent, TextField};
use crate::theme;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Projects,
    Customize,
}

struct NewProject {
    name: Entity<TextField>,
    location: Entity<TextField>,
}

pub struct Welcome {
    page: Page,
    query: Entity<TextField>,
    projects: Vec<(PathBuf, Option<String>)>,
    gear_open: bool,
    new_project: Option<NewProject>,
    /// `composer create-project` running for this folder.
    creating: Option<(Entity<TerminalView>, PathBuf, Subscription)>,
    modal: Option<AnyView>,
    message: Option<String>,
    focus_handle: FocusHandle,
    _subscriptions: Vec<Subscription>,
    _modal_subscription: Option<Subscription>,
}

/// The checked-out branch, read from `.git/HEAD`.
fn branch(root: &Path) -> Option<String> {
    let head = std::fs::read_to_string(root.join(".git/HEAD")).ok()?;
    head.trim().strip_prefix("ref: refs/heads/").map(str::to_string)
}

/// Opens the welcome window.
pub fn open(cx: &mut App) {
    let bounds = gpui::Bounds::centered(None, gpui::size(px(900.), px(620.)), cx);
    let options = gpui::WindowOptions {
        window_bounds: Some(gpui::WindowBounds::Windowed(bounds)),
        titlebar: Some(gpui::TitlebarOptions { title: Some("Welcome to PhpCraft".into()), ..Default::default() }),
        ..Default::default()
    };
    let _ = cx.open_window(options, |window, cx| cx.new(|cx| Welcome::new(window, cx)));
}

impl Welcome {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let query = cx.new(|cx| TextField::new("Search projects", cx));
        let subscription = cx.subscribe(&query, |_, _, _: &FieldEvent, cx| cx.notify());
        let focus_handle = cx.focus_handle();
        window.focus(&query.focus_handle(cx));
        let mut welcome = Self {
            page: Page::Projects,
            query,
            projects: Vec::new(),
            gear_open: false,
            new_project: None,
            creating: None,
            modal: None,
            message: None,
            focus_handle,
            _subscriptions: vec![subscription],
            _modal_subscription: None,
        };
        welcome.reload_projects(cx);
        welcome
    }

    /// Lists recent projects in the background: touching folders like ~/Desktop can wait on
    /// a macOS privacy prompt, which must not keep the window from appearing.
    fn reload_projects(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let projects = cx
                .background_spawn(async move {
                    config::recent_projects()
                        .into_iter()
                        .map(|p| {
                            let b = branch(&p);
                            (p, b)
                        })
                        .collect::<Vec<_>>()
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.projects = projects;
                cx.notify();
            });
        })
        .detach();
    }

    /// Opens a project window and closes this one.
    fn open_project(&mut self, path: &Path, window: &mut Window, cx: &mut Context<Self>) {
        match crate::open_project_window(path, cx) {
            Ok(()) => window.remove_window(),
            Err(err) => {
                self.message = Some(format!("Cannot open {}: {err}", path.display()));
                cx.notify();
            }
        }
    }

    fn pick_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(gpui::PathPromptOptions { files: false, directories: true, multiple: false, prompt: None });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = paths.await
                && let Some(path) = paths.into_iter().next()
            {
                let _ = this.update_in(cx, |this, window, cx| this.open_project(&path, window, cx));
            }
        })
        .detach();
    }

    fn show_modal<D: Render + gpui::EventEmitter<DialogEvent>>(&mut self, dialog: Entity<D>, window: &mut Window, cx: &mut Context<Self>) {
        self._modal_subscription = Some(cx.subscribe_in(&dialog, window, |this, _, event: &DialogEvent, window, cx| {
            this.modal = None;
            match event {
                DialogEvent::CloneRepository { url, parent } => this.clone_repository(url.clone(), parent.clone(), window, cx),
                DialogEvent::OpenProject(path) => {
                    let path = path.clone();
                    this.open_project(&path, window, cx);
                }
                _ => {}
            }
            window.focus(&this.query.focus_handle(cx));
            cx.notify();
        }));
        self.modal = Some(dialog.into());
        self.gear_open = false;
        cx.notify();
    }

    fn clone_repository(&mut self, url: String, parent: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let name = url.trim_end_matches('/').trim_end_matches(".git").rsplit(['/', ':']).next().unwrap_or("repo").to_string();
        let target = parent.join(name);
        let _ = std::fs::create_dir_all(&parent);
        let command = terminal::Command { program: "git".into(), args: vec!["clone".into(), "--progress".into(), url, target.display().to_string()] };
        self.run_creation(command, &parent, target, window, cx);
    }

    fn create_laravel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(new) = &self.new_project else { return };
        let name = new.name.read(cx).text().trim().to_string();
        let location = PathBuf::from(new.location.read(cx).text().trim());
        if name.is_empty() {
            return;
        }
        let _ = std::fs::create_dir_all(&location);
        let command = terminal::Command {
            program: "composer".into(),
            args: vec!["create-project".into(), "laravel/laravel".into(), name.clone()],
        };
        self.run_creation(command, &location.clone(), location.join(name), window, cx);
    }

    /// Runs a project-creating command in a terminal; opens the project when it succeeds.
    fn run_creation(&mut self, command: terminal::Command, cwd: &Path, target: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        match TerminalView::spawn(cwd, Some(command), "Create".into(), Default::default(), cx) {
            Ok(view) => {
                let path = target.clone();
                let subscription = cx.subscribe_in(&view, window, move |this, view, event: &TerminalViewEvent, window, cx| {
                    if matches!(event, TerminalViewEvent::Exited) && view.read(cx).exit_code == Some(0) {
                        this.open_project(&path, window, cx);
                    }
                    cx.notify();
                });
                self.creating = Some((view, target, subscription));
                self.new_project = None;
                self.message = None;
            }
            Err(err) => self.message = Some(err.to_string()),
        }
        cx.notify();
    }

    fn import(&mut self, source: Source, cx: &mut Context<Self>) {
        self.message = Some(match settings_import::import(source) {
            Ok(report) => {
                theme::init(cx);
                crate::keymap::apply(cx);
                format!(
                    "Imported from {}: {}{} keybinding(s).",
                    source.name(),
                    report.settings.iter().map(|s| format!("{s}; ")).collect::<String>(),
                    report.bindings.len()
                )
            }
            Err(err) => err,
        });
        cx.notify();
    }

    fn render_projects(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let query = self.query.read(cx).text().to_ascii_lowercase();
        let rows = self
            .projects
            .iter()
            .filter(|(p, _)| query.is_empty() || p.to_string_lossy().to_ascii_lowercase().contains(&query))
            .enumerate()
            .map(|(ix, (path, branch))| {
                let name = project_name(path);
                let open_path = path.clone();
                let forget = path.clone();
                div()
                    .id(("welcome-project", ix))
                    .group("project-row")
                    .flex()
                    .gap_3()
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .cursor_pointer()
                    .hover(|d| d.bg(theme::hover()))
                    .child(avatar(&name, 30.))
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .child(div().text_size(px(14.)).text_color(theme::ui_text()).child(SharedString::from(name)))
                            .child(div().text_size(px(12.)).text_color(theme::muted()).child(tilde(path)))
                            .when_some(branch.clone(), |d, b| d.child(div().text_size(px(12.)).text_color(theme::muted()).child(format!("⎇ {b}")))),
                    )
                    .child(
                        div()
                            .id(("welcome-forget", ix))
                            .invisible()
                            .group_hover("project-row", |d| d.visible())
                            .px_1()
                            .text_color(theme::muted())
                            .hover(|d| d.text_color(theme::ui_text()))
                            .child("×")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                config::forget_project(&forget);
                                this.reload_projects(cx);
                                cx.notify();
                            })),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| this.open_project(&open_path, window, cx)))
            })
            .collect::<Vec<_>>();
        let empty = rows.is_empty();
        div()
            .id("welcome-projects")
            .flex_1()
            .overflow_y_scroll()
            .px_2()
            .children(rows)
            .when(empty, |d| d.child(div().p_6().text_color(theme::muted()).child("No recent projects. Open a folder or clone a repository.")))
    }

    fn render_customize(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let settings = Settings::load();
        let label = |text: &'static str| div().w(px(150.)).flex_none().text_color(theme::muted()).child(text);
        let chip = |id: SharedString, text: String, active: bool| {
            div()
                .id(id)
                .px_2()
                .py(px(3.))
                .rounded_md()
                .border_1()
                .cursor_pointer()
                .border_color(if active { theme::accent() } else { theme::border_strong() })
                .when(active, |d| d.bg(theme::active_row()))
                .hover(|d| d.bg(theme::hover()))
                .child(text)
        };
        let themes = theme::available().into_iter().map(|t| {
            let name = t.name.clone();
            chip(SharedString::from(format!("w-theme-{name}")), name.clone(), settings.theme == name).on_click(cx.listener(move |_, _, _, cx| {
                let mut s = Settings::load();
                s.theme = name.clone();
                s.save();
                theme::set(&name, cx);
                cx.notify();
            }))
        });
        let sizes = [12.0f32, 13., 13.5, 14., 15., 16.].into_iter().map(|size| {
            chip(SharedString::from(format!("w-size-{size}")), format!("{size}"), (settings.editor_font_size - size).abs() < 0.01).on_click(cx.listener(
                move |_, _, _, cx| {
                    let mut s = Settings::load();
                    s.editor_font_size = size;
                    s.save();
                    theme::init(cx);
                    cx.notify();
                },
            ))
        });
        let imports = Source::ALL.into_iter().map(|source| {
            let found = source.locate().is_some();
            chip(SharedString::from(format!("w-import-{}", source.name())), source.name().to_string(), false)
                .when(!found, |d| d.opacity(0.45))
                .on_click(cx.listener(move |this, _, _, cx| this.import(source, cx)))
        });
        div()
            .flex()
            .flex_col()
            .gap_4()
            .p_6()
            .text_size(px(13.))
            .child(div().text_size(px(18.)).text_color(theme::ui_text()).child("Customize"))
            .child(div().flex().items_start().gap_2().child(label("Color theme")).child(div().flex().flex_wrap().gap_1().children(themes)))
            .child(div().flex().items_center().gap_2().child(label("Editor font size")).child(div().flex().gap_1().children(sizes)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(label("Keymap"))
                    .child(button("w-keymap", "Configure Keymap…", false).on_click(cx.listener(|this, _, window, cx| {
                        let dialog = cx.new(|cx| KeymapDialog::new(window, cx));
                        this.show_modal(dialog, window, cx);
                    }))),
            )
            .child(div().flex().items_center().gap_2().child(label("Import settings from")).child(div().flex().gap_1().children(imports)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(label(""))
                    .child(button("w-all-settings", "All Settings…", false).on_click(cx.listener(|this, _, window, cx| {
                        let dialog = cx.new(|cx| SettingsDialog::new(window, cx));
                        this.show_modal(dialog, window, cx);
                    }))),
            )
    }
}

impl Focusable for Welcome {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for Welcome {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let nav = |id: &'static str, label: &'static str, page: Page, current: Page| {
            div()
                .id(id)
                .px_3()
                .h(px(32.))
                .flex()
                .items_center()
                .rounded_md()
                .cursor_pointer()
                .text_color(theme::ui_text())
                .when(page == current, |d| d.bg(theme::selection()))
                .when(page != current, |d| d.hover(|d| d.bg(theme::hover())))
                .child(label)
        };
        let page = self.page;
        let main: gpui::AnyElement = if let Some((view, target, _)) = &self.creating {
            let running = view.read(cx).is_running();
            div()
                .flex_1()
                .flex()
                .flex_col()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .p_3()
                        .child(div().flex_1().text_color(theme::ui_text()).child(format!("Creating {}", tilde(target))))
                        .when(!running, |d| {
                            d.child(button("w-back", "Back", false).on_click(cx.listener(|this, _, _, cx| {
                                this.creating = None;
                                this.reload_projects(cx);
                                cx.notify();
                            })))
                        }),
                )
                .child(div().flex_1().min_h(px(0.)).child(view.clone()))
                .into_any_element()
        } else if let Some(new) = &self.new_project {
            div()
                .flex_1()
                .flex()
                .flex_col()
                .gap_3()
                .p_6()
                .child(div().text_size(px(18.)).text_color(theme::ui_text()).child("New Laravel Project"))
                .child(div().flex().items_center().gap_2().child(div().w(px(90.)).text_color(theme::muted()).child("Name")).child(div().flex_1().child(new.name.clone())))
                .child(div().flex().items_center().gap_2().child(div().w(px(90.)).text_color(theme::muted()).child("Location")).child(div().flex_1().child(new.location.clone())))
                .child(div().text_size(px(12.)).text_color(theme::muted()).child("Runs composer create-project laravel/laravel <name> in the location."))
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .justify_end()
                        .child(button("w-new-cancel", "Cancel", false).on_click(cx.listener(|this, _, _, cx| {
                            this.new_project = None;
                            cx.notify();
                        })))
                        .child(button("w-new-create", "Create", true).on_click(cx.listener(|this, _, window, cx| this.create_laravel(window, cx)))),
                )
                .into_any_element()
        } else {
            match page {
                Page::Projects => div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .min_h(px(0.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .p_3()
                            .border_b_1()
                            .border_color(theme::border())
                            .child(div().flex_1().child(self.query.clone()))
                            .child(button("w-new", "New Project", false).on_click(cx.listener(|this, _, window, cx| {
                                let parent = dirs::home_dir().map(|h| h.join("Projects")).unwrap_or_default().display().to_string();
                                let name = cx.new(|cx| TextField::new("example-app", cx));
                                let location = cx.new(|cx| {
                                    let mut f = TextField::new("Location", cx);
                                    f.set_text(parent, cx);
                                    f
                                });
                                window.focus(&name.focus_handle(cx));
                                this.new_project = Some(NewProject { name, location });
                                cx.notify();
                            })))
                            .child(button("w-open", "Open", false).on_click(cx.listener(|this, _, window, cx| this.pick_folder(window, cx))))
                            .child(button("w-clone", "Clone Repository", false).on_click(cx.listener(|this, _, window, cx| {
                                let dialog = cx.new(|cx| ProjectDialog::clone_repository(window, cx));
                                this.show_modal(dialog, window, cx);
                            }))),
                    )
                    .child(self.render_projects(cx))
                    .into_any_element(),
                Page::Customize => self.render_customize(cx).into_any_element(),
            }
        };
        let gear_menu = self.gear_open.then(|| {
            let item = |id: &'static str, label: &'static str, hint: &'static str| {
                div()
                    .id(id)
                    .flex()
                    .justify_between()
                    .gap_6()
                    .px_3()
                    .h(px(26.))
                    .items_center()
                    .rounded_md()
                    .cursor_pointer()
                    .hover(|d| d.bg(theme::selection()))
                    .child(label)
                    .child(div().text_color(theme::muted()).child(hint))
            };
            div()
                .absolute()
                .bottom(px(44.))
                .left(px(10.))
                .w(px(240.))
                .p_1()
                .bg(theme::panel_bg())
                .border_1()
                .border_color(theme::border_strong())
                .rounded_md()
                .shadow_lg()
                .child(item("g-settings", "Settings", "⌘,").on_click(cx.listener(|this, _, window, cx| {
                    let dialog = cx.new(|cx| SettingsDialog::new(window, cx));
                    this.show_modal(dialog, window, cx);
                })))
                .child(item("g-keymap", "Keymap", "").on_click(cx.listener(|this, _, window, cx| {
                    let dialog = cx.new(|cx| KeymapDialog::new(window, cx));
                    this.show_modal(dialog, window, cx);
                })))
                .child(item("g-import", "Import Settings…", "").on_click(cx.listener(|this, _, _, cx| {
                    this.page = Page::Customize;
                    this.gear_open = false;
                    cx.notify();
                })))
                .child(item("g-about", "About", "").on_click(cx.listener(|this, _, _, cx| {
                    this.message = Some(format!("PhpCraft {} — PhpStorm in pure Rust. github.com/rasul-chopurov/phpcraft", env!("CARGO_PKG_VERSION")));
                    this.gear_open = false;
                    cx.notify();
                })))
        });
        div()
            .track_focus(&self.focus_handle)
            .relative()
            .size_full()
            .flex()
            .bg(theme::editor_bg())
            .font_family(theme::ui_font())
            .text_size(px(13.))
            .text_color(theme::text())
            .child(
                div()
                    .w(px(220.))
                    .flex_none()
                    .flex()
                    .flex_col()
                    .p_2()
                    .gap_1()
                    .bg(theme::panel_bg())
                    .border_r_1()
                    .border_color(theme::border())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .px_2()
                            .py_3()
                            .child(avatar("Php Craft", 34.))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .child(div().text_size(px(15.)).text_color(theme::ui_text()).child("PhpCraft"))
                                    .child(div().text_size(px(12.)).text_color(theme::muted()).child(env!("CARGO_PKG_VERSION"))),
                            ),
                    )
                    .child(nav("w-nav-projects", "Projects", Page::Projects, page).on_click(cx.listener(|this, _, _, cx| {
                        this.page = Page::Projects;
                        cx.notify();
                    })))
                    .child(nav("w-nav-customize", "Customize", Page::Customize, page).on_click(cx.listener(|this, _, _, cx| {
                        this.page = Page::Customize;
                        cx.notify();
                    })))
                    .child(div().flex_1())
                    .child(
                        div()
                            .id("w-gear")
                            .w(px(30.))
                            .h(px(30.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_md()
                            .cursor_pointer()
                            .text_size(px(16.))
                            .text_color(theme::muted())
                            .hover(|d| d.bg(theme::hover()))
                            .child("⚙")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.gear_open = !this.gear_open;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .min_w(px(0.))
                    .child(main)
                    .when_some(self.message.clone(), |d, message| {
                        d.child(
                            div()
                                .id("w-message")
                                .p_3()
                                .border_t_1()
                                .border_color(theme::border())
                                .text_size(px(12.))
                                .text_color(theme::muted())
                                .child(message)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.message = None;
                                    cx.notify();
                                })),
                        )
                    }),
            )
            .children(gear_menu)
            .when_some(self.modal.clone(), |d, modal| {
                d.child(
                    div()
                        .absolute()
                        .inset_0()
                        .bg(theme::overlay())
                        .flex()
                        .justify_center()
                        .items_start()
                        .pt(px(60.))
                        .child(modal),
                )
            })
    }
}
