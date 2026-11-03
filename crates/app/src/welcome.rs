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

#[derive(Clone, Copy, PartialEq, Eq)]
enum ProjectKind {
    Empty,
    Laravel,
    StarterKit,
    Html5,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum StarterKit {
    React,
    Vue,
    Livewire,
}

impl StarterKit {
    fn package(self) -> &'static str {
        match self {
            StarterKit::React => "laravel/react-starter-kit",
            StarterKit::Vue => "laravel/vue-starter-kit",
            StarterKit::Livewire => "laravel/livewire-starter-kit",
        }
    }
}

struct NewProject {
    kind: ProjectKind,
    kit: StarterKit,
    composer_json: bool,
    name: Entity<TextField>,
    location: Entity<TextField>,
}

const HTML5_INDEX: &str = "<!doctype html>\n<html lang=\"en\">\n<head>\n  <meta charset=\"utf-8\">\n  <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n  <title>NAME</title>\n  <link rel=\"stylesheet\" href=\"css/style.css\">\n</head>\n<body>\n  <h1>NAME</h1>\n  <script src=\"js/app.js\"></script>\n</body>\n</html>\n";

pub struct Welcome {
    page: Page,
    query: Entity<TextField>,
    projects: Vec<(PathBuf, Option<String>)>,
    groups: Vec<config::ProjectGroup>,
    /// The project whose "move to group" menu is open.
    group_menu: Option<PathBuf>,
    /// Name field while creating a group.
    new_group: Option<Entity<TextField>>,
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
    let bounds = crate::window_state::centered(900., 620., cx);
    let options = gpui::WindowOptions {
        window_bounds: Some(gpui::WindowBounds::Windowed(bounds)),
        window_min_size: Some(crate::window_state::min_size()),
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
            groups: config::project_groups(),
            group_menu: None,
            new_group: None,
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
        // Show the list at once; folders that are gone and git branches are filled in later.
        let previous: std::collections::HashMap<PathBuf, Option<String>> = self.projects.drain(..).collect();
        self.projects = config::recent_projects_unchecked().into_iter().map(|p| {
            let branch = previous.get(&p).cloned().flatten();
            (p, branch)
        }).collect();
        cx.notify();
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
                DialogEvent::OpenProject(path, _) => {
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

    /// Creates the chosen kind of project: composer ones run in a terminal, the rest are
    /// written here and opened right away.
    fn create_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(new) = &self.new_project else { return };
        let name = new.name.read(cx).text().trim().to_string();
        let location = PathBuf::from(new.location.read(cx).text().trim());
        if name.is_empty() {
            return;
        }
        let target = location.join(&name);
        if target.exists() && std::fs::read_dir(&target).is_ok_and(|mut d| d.next().is_some()) {
            self.message = Some(format!("{} already exists and isn't empty", target.display()));
            cx.notify();
            return;
        }
        let _ = std::fs::create_dir_all(&location);
        let composer = |package: &str| terminal::Command {
            program: "composer".into(),
            args: vec!["create-project".into(), package.into(), name.clone()],
        };
        match new.kind {
            ProjectKind::Laravel => self.run_creation(composer("laravel/laravel"), &location.clone(), target, window, cx),
            ProjectKind::StarterKit => {
                let package = new.kit.package();
                self.run_creation(composer(package), &location.clone(), target, window, cx)
            }
            ProjectKind::Empty | ProjectKind::Html5 => {
                let result = (|| -> std::io::Result<()> {
                    std::fs::create_dir_all(&target)?;
                    if new.kind == ProjectKind::Html5 {
                        std::fs::create_dir_all(target.join("css"))?;
                        std::fs::create_dir_all(target.join("js"))?;
                        std::fs::write(target.join("index.html"), HTML5_INDEX.replace("NAME", &name))?;
                        std::fs::write(target.join("css/style.css"), "*,\n*::before,\n*::after {\n  box-sizing: border-box;\n}\n\nbody {\n  margin: 0;\n  font-family: system-ui, sans-serif;\n}\n")?;
                        std::fs::write(target.join("js/app.js"), "'use strict';\n")?;
                    } else if new.composer_json {
                        let package = name.to_lowercase().replace(' ', "-");
                        std::fs::write(
                            target.join("composer.json"),
                            format!("{{\n    \"name\": \"app/{package}\",\n    \"type\": \"project\",\n    \"require\": {{\n        \"php\": \"^8.2\"\n    }},\n    \"autoload\": {{\n        \"psr-4\": {{\n            \"App\\\\\": \"src/\"\n        }}\n    }}\n}}\n"),
                        )?;
                        std::fs::create_dir_all(target.join("src"))?;
                    }
                    Ok(())
                })();
                match result {
                    Ok(()) => self.open_project(&target, window, cx),
                    Err(err) => self.message = Some(err.to_string()),
                }
            }
        }
        cx.notify();
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

    fn save_groups(&mut self, cx: &mut Context<Self>) {
        config::save_project_groups(&self.groups);
        cx.notify();
    }

    /// Moves `path` into the group at `target` (or out of every group with `None`).
    fn move_to_group(&mut self, path: &Path, target: Option<usize>, cx: &mut Context<Self>) {
        for group in &mut self.groups {
            group.projects.retain(|p| p != path);
        }
        if let Some(group) = target.and_then(|ix| self.groups.get_mut(ix)) {
            group.projects.push(path.to_path_buf());
        }
        self.group_menu = None;
        self.save_groups(cx);
    }

    fn start_new_group(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let field = cx.new(|cx| TextField::new("Group name, e.g. Clients", cx));
        window.focus(&field.focus_handle(cx));
        self.new_group = Some(field);
        cx.notify();
    }

    fn create_group(&mut self, cx: &mut Context<Self>) {
        let Some(field) = self.new_group.take() else { return };
        let name = field.read(cx).text().trim().to_string();
        if !name.is_empty() && !self.groups.iter().any(|g| g.name == name) {
            self.groups.push(config::ProjectGroup { name, ..Default::default() });
            // A project whose menu was open goes straight into the new group.
            if let Some(path) = self.group_menu.clone() {
                let ix = self.groups.len() - 1;
                self.move_to_group(&path, Some(ix), cx);
            }
        }
        self.save_groups(cx);
    }

    fn project_row(&self, ix: usize, path: &Path, branch: Option<&String>, indent: bool, cx: &mut Context<Self>) -> gpui::AnyElement {
        let name = project_name(path);
        let open_path = path.to_path_buf();
        let forget = path.to_path_buf();
        let menu_path = path.to_path_buf();
        let menu_open = self.group_menu.as_deref() == Some(path);
        let hover_icon = |id: (&'static str, usize), glyph: crate::icons::Icon, tip: &'static str| {
            div()
                .id(id)
                .invisible()
                .group_hover("project-row", |d| d.visible())
                .p_1()
                .rounded_sm()
                .hover(|d| d.bg(theme::hover()))
                .child(crate::icons::icon(glyph, 14., theme::muted()))
                .tooltip(move |_, cx| cx.new(|_| crate::dialogs::TextTooltip(tip.into())).into())
        };
        let row = div()
            .id(("welcome-project", ix))
            .group("project-row")
            .flex()
            .items_center()
            .gap_3()
            .when(indent, |d| d.ml_5())
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
                    .when_some(branch.cloned(), |d, b| d.child(div().text_size(px(12.)).text_color(theme::muted()).child(format!("⎇ {b}")))),
            )
            .child(hover_icon(("welcome-group", ix), crate::icons::Icon::folder, "Move to Group").on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.group_menu = if this.group_menu.as_ref() == Some(&menu_path) { None } else { Some(menu_path.clone()) };
                cx.notify();
            })))
            .child({
                let new_window = path.to_path_buf();
                hover_icon(("welcome-new-window", ix), crate::icons::Icon::split, "Open in New Window").on_click(cx.listener(move |this, _, _, cx| {
                    // Keep the welcome window open for more projects.
                    cx.stop_propagation();
                    if let Err(err) = crate::open_project_window(&new_window, cx) {
                        this.message = Some(err.to_string());
                    }
                    cx.notify();
                }))
            })
            .child(hover_icon(("welcome-forget", ix), crate::icons::Icon::close, "Remove from Recent").on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                config::forget_project(&forget);
                this.reload_projects(cx);
                cx.notify();
            })))
            .on_click(cx.listener(move |this, _, window, cx| this.open_project(&open_path, window, cx)));
        if !menu_open {
            return row.into_any_element();
        }
        let path = path.to_path_buf();
        let in_group = self.groups.iter().any(|g| g.projects.contains(&path));
        let chip = |id: SharedString, label: String| {
            div().id(id).px_2().py_0p5().rounded_md().cursor_pointer().border_1().border_color(theme::border()).text_size(px(12.)).hover(|d| d.bg(theme::hover())).child(label)
        };
        let mut menu = div().flex().flex_wrap().items_center().gap_1().ml_12().mb_2().child(div().text_size(px(12.)).text_color(theme::muted()).child("Move to:"));
        for (gix, group) in self.groups.iter().enumerate() {
            let p = path.clone();
            menu = menu.child(chip(SharedString::from(format!("move-{ix}-{gix}")), group.name.clone()).on_click(cx.listener(move |this, _, _, cx| this.move_to_group(&p, Some(gix), cx))));
        }
        if in_group {
            let p = path.clone();
            menu = menu.child(chip(SharedString::from(format!("move-{ix}-none")), "No group".into()).on_click(cx.listener(move |this, _, _, cx| this.move_to_group(&p, None, cx))));
        }
        menu = menu.child(chip(SharedString::from(format!("move-{ix}-new")), "+ New group".into()).on_click(cx.listener(|this, _, window, cx| this.start_new_group(window, cx))));
        div().flex().flex_col().child(row).child(menu).into_any_element()
    }

    fn render_projects(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let query = self.query.read(cx).text().to_ascii_lowercase();
        let matches = |p: &Path| query.is_empty() || p.to_string_lossy().to_ascii_lowercase().contains(&query);
        let branch_of = |p: &Path| self.projects.iter().find(|(q, _)| q == p).and_then(|(_, b)| b.clone());
        let known: std::collections::HashSet<&PathBuf> = self.projects.iter().map(|(p, _)| p).collect();
        let mut children: Vec<gpui::AnyElement> = Vec::new();
        let mut ix = 0;
        if let Some(field) = &self.new_group {
            children.push(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .key_context("NewGroup")
                    .on_action(cx.listener(|this, _: &crate::dialogs::Confirm, _, cx| this.create_group(cx)))
                    .on_action(cx.listener(|this, _: &crate::dialogs::Cancel, _, cx| {
                        this.new_group = None;
                        cx.notify();
                    }))
                    .child(crate::icons::icon(crate::icons::Icon::folder, 16., theme::accent()))
                    .child(div().flex_1().flex().child(field.clone()))
                    .child(crate::dialogs::button("group-create", "Create", true).on_click(cx.listener(|this, _, _, cx| this.create_group(cx))))
                    .into_any_element(),
            );
        }
        for (gix, group) in self.groups.iter().enumerate() {
            let members: Vec<&PathBuf> = group.projects.iter().filter(|p| known.contains(p) && matches(p)).collect();
            if !query.is_empty() && members.is_empty() {
                continue;
            }
            let collapsed = group.collapsed && query.is_empty();
            children.push(
                div()
                    .id(("welcome-group-row", gix))
                    .group("group-row")
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_1p5()
                    .rounded_md()
                    .cursor_pointer()
                    .hover(|d| d.bg(theme::hover()))
                    .child(crate::icons::icon(if collapsed { crate::icons::Icon::chevron_right } else { crate::icons::Icon::chevron_down }, 12., theme::muted()))
                    .child(crate::icons::icon(if collapsed { crate::icons::Icon::folder } else { crate::icons::Icon::folder_open }, 16., theme::accent()))
                    .child(div().text_color(theme::ui_text()).font_weight(gpui::FontWeight::SEMIBOLD).child(group.name.clone()))
                    .child(div().text_size(px(12.)).text_color(theme::muted()).child(members.len().to_string()))
                    .child(div().flex_1())
                    .child(
                        div()
                            .id(("welcome-group-delete", gix))
                            .invisible()
                            .group_hover("group-row", |d| d.visible())
                            .p_1()
                            .rounded_sm()
                            .hover(|d| d.bg(theme::hover()))
                            .child(crate::icons::icon(crate::icons::Icon::close, 13., theme::muted()))
                            .tooltip(|_, cx| cx.new(|_| crate::dialogs::TextTooltip("Delete group (its projects stay in Recent)".into())).into())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.groups.remove(gix);
                                this.save_groups(cx);
                            })),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(group) = this.groups.get_mut(gix) {
                            group.collapsed = !group.collapsed;
                        }
                        this.save_groups(cx);
                    }))
                    .into_any_element(),
            );
            if !collapsed {
                for path in members {
                    children.push(self.project_row(ix, path, branch_of(path).as_ref(), true, cx));
                    ix += 1;
                }
            }
        }
        let grouped: std::collections::HashSet<&PathBuf> = self.groups.iter().flat_map(|g| g.projects.iter()).collect();
        let loose: Vec<(PathBuf, Option<String>)> = self.projects.iter().filter(|(p, _)| !grouped.contains(p) && matches(p)).cloned().collect();
        if !self.groups.is_empty() && !loose.is_empty() {
            children.push(div().px_3().pt_3().pb_1().text_size(px(11.)).text_color(theme::muted()).child("RECENT").into_any_element());
        }
        let empty = self.projects.is_empty();
        for (path, branch) in &loose {
            children.push(self.project_row(ix, path, branch.as_ref(), false, cx));
            ix += 1;
        }
        div()
            .id("welcome-projects")
            .flex_1()
            .overflow_y_scroll()
            .px_2()
            .children(children)
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
            let kinds = [
                (ProjectKind::Empty, "PHP Empty Project", "▢"),
                (ProjectKind::Laravel, "Laravel", "◈"),
                (ProjectKind::StarterKit, "Laravel Starter Kits", "◆"),
                (ProjectKind::Html5, "HTML5 Boilerplate", "5"),
            ];
            let list = kinds.into_iter().map(|(kind, label, icon)| {
                let active = new.kind == kind;
                div()
                    .id(SharedString::from(format!("np-{label}")))
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .h(px(30.))
                    .rounded_md()
                    .cursor_pointer()
                    .text_color(theme::ui_text())
                    .when(active, |d| d.bg(theme::selection()))
                    .when(!active, |d| d.hover(|d| d.bg(theme::hover())))
                    .child(div().w(px(18.)).text_color(theme::syntax(php_syntax::Highlight::Keyword)).child(icon))
                    .child(label)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(new) = this.new_project.as_mut() {
                            new.kind = kind;
                        }
                        cx.notify();
                    }))
            });
            let field_row = |label: &'static str| div().flex().items_center().gap_2().child(div().w(px(90.)).text_color(theme::muted()).child(label));
            let (title, hint) = match new.kind {
                ProjectKind::Empty => ("PHP Empty Project", "A folder for your code."),
                ProjectKind::Laravel => ("Laravel", "Runs composer create-project laravel/laravel <name>."),
                ProjectKind::StarterKit => ("Laravel Starter Kits", "Runs composer create-project with the official starter kit."),
                ProjectKind::Html5 => ("HTML5 Boilerplate", "index.html, css/style.css and js/app.js."),
            };
            let kits = [(StarterKit::React, "React"), (StarterKit::Vue, "Vue"), (StarterKit::Livewire, "Livewire")].into_iter().map(|(kit, label)| {
                let active = new.kit == kit;
                div()
                    .id(SharedString::from(format!("np-kit-{label}")))
                    .px_3()
                    .py_1()
                    .rounded_md()
                    .border_1()
                    .cursor_pointer()
                    .border_color(if active { theme::accent() } else { theme::border_strong() })
                    .when(active, |d| d.bg(theme::active_row()))
                    .child(label)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(new) = this.new_project.as_mut() {
                            new.kit = kit;
                        }
                        cx.notify();
                    }))
            });
            let composer_json = new.composer_json;
            div()
                .flex_1()
                .flex()
                .min_h(px(0.))
                .child(div().w(px(220.)).flex_none().p_2().flex().flex_col().gap_1().border_r_1().border_color(theme::border()).children(list))
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .p_6()
                        .child(div().text_size(px(18.)).text_color(theme::ui_text()).child(title))
                        .child(field_row("Name").child(div().flex_1().child(new.name.clone())))
                        .child(field_row("Location").child(div().flex_1().child(new.location.clone())))
                        .when(new.kind == ProjectKind::StarterKit, |d| d.child(field_row("Stack").child(div().flex().gap_1().children(kits))))
                        .when(new.kind == ProjectKind::Empty, |d| {
                            d.child(crate::dialogs::checkbox("np-composer", "Add 'composer.json'", composer_json).on_click(cx.listener(|this, _, _, cx| {
                                if let Some(new) = this.new_project.as_mut() {
                                    new.composer_json = !new.composer_json;
                                }
                                cx.notify();
                            })))
                        })
                        .child(div().text_size(px(12.)).text_color(theme::muted()).child(hint))
                        .child(div().flex_1())
                        .child(
                            div()
                                .flex()
                                .gap_2()
                                .justify_end()
                                .child(button("w-new-cancel", "Cancel", false).on_click(cx.listener(|this, _, _, cx| {
                                    this.new_project = None;
                                    cx.notify();
                                })))
                                .child(button("w-new-create", "Create", true).on_click(cx.listener(|this, _, window, cx| this.create_project(window, cx)))),
                        ),
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
                            .child(div().flex_1().flex().child(self.query.clone()))
                            .child(button("w-new-group", "New Group", false).on_click(cx.listener(|this, _, window, cx| {
                                this.group_menu = None;
                                this.start_new_group(window, cx);
                            })))
                            .child(button("w-new", "New Project", false).on_click(cx.listener(|this, _, window, cx| {
                                let parent = dirs::home_dir().map(|h| h.join("Projects")).unwrap_or_default().display().to_string();
                                let name = cx.new(|cx| TextField::new("example-app", cx));
                                let location = cx.new(|cx| {
                                    let mut f = TextField::new("Location", cx);
                                    f.set_text(parent, cx);
                                    f
                                });
                                window.focus(&name.focus_handle(cx));
                                this.new_project = Some(NewProject { kind: ProjectKind::Laravel, kit: StarterKit::Livewire, composer_json: false, name, location });
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
                    this.message = Some(format!("PhpCraft {} · Made with ❤️ in Azerbaijan · path.az", env!("CARGO_PKG_VERSION")));
                    this.gear_open = false;
                    cx.notify();
                })))
        });
        div()
            .track_focus(&self.focus_handle)
            // Drop a project folder (or files) here to open it.
            .on_drop(cx.listener(|_, paths: &gpui::ExternalPaths, _, cx| {
                let paths = paths.paths().to_vec();
                cx.defer(move |cx| crate::open_paths::open(paths, cx));
            }))
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
            .when_some(self.modal.clone(), |d, modal| d.child(crate::dialogs::modal_overlay(modal)))
    }
}
