//! Laravel tooling in the workspace: Make / Artisan dialogs, the toolbar and the output panel.

use std::path::PathBuf;
use std::sync::Arc;

use gpui::{AnyView, Context, Entity, SharedString, Window, actions, div, prelude::*, px};

use super::Workspace;
use crate::dialogs::{
    ArtisanDialog, ArtisanRunDialog, DialogEvent, HelperCodeDialog, KeymapDialog,
    LaravelSettingsDialog, MakeChooser, MakeDialog, RouteSearchDialog, SettingsDialog, ThemeDialog,
};
use crate::theme;
use laravel::make::MakeKind;

actions!(
    workspace,
    [
        OpenMake,
        OpenArtisan,
        CloseOutput,
        OpenRouteSearch,
        NewModel,
        NewPivotTable,
        ToggleLaravelMenu,
        OpenLaravelSettings,
        GenerateHelperCode,
        RemoveModelPhpDocs,
        ModelFromTable,
        FindUnusedViews,
        OpenKeymap,
        OpenThemes,
        TogglePrefsMenu,
        OpenSettings,
        SyncPush,
        SyncPull,
        OpenProjects
    ]
);

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

    pub(super) fn new_pivot_table(
        &mut self,
        _: &NewPivotTable,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_make_kind(MakeKind::PivotTable, window, cx);
    }

    fn open_make_kind(&mut self, kind: MakeKind, window: &mut Window, cx: &mut Context<Self>) {
        let root = self.project.root().to_path_buf();
        let models = self.intel.models();
        let dialog = cx.new(|cx| MakeDialog::new(&root, kind, models, window, cx));
        self.show_modal(dialog, window, cx);
    }

    pub(super) fn open_route_search(
        &mut self,
        _: &OpenRouteSearch,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let root = self.artisan_root();
        let dialog = cx.new(|cx| RouteSearchDialog::new(&root, window, cx));
        self.show_modal(dialog, window, cx);
    }

    pub(super) fn open_keymap(
        &mut self,
        _: &OpenKeymap,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let dialog = cx.new(|cx| KeymapDialog::new(window, cx));
        self.show_modal(dialog, window, cx);
    }

    pub(super) fn sync_push(&mut self, _: &SyncPush, _: &mut Window, cx: &mut Context<Self>) {
        self.run_sync(true, cx);
    }

    pub(super) fn sync_pull(&mut self, _: &SyncPull, _: &mut Window, cx: &mut Context<Self>) {
        self.run_sync(false, cx);
    }

    /// Settings Sync through a secret GitHub gist (via the `gh` CLI), in the background.
    fn run_sync(&mut self, push: bool, cx: &mut Context<Self>) {
        let task = self.begin_task(if push { "Pushing settings to GitHub…" } else { "Pulling settings from GitHub…" }, cx);
        cx.spawn(async move |this, cx| {
            let result = cx.background_executor().spawn(async move { if push { crate::sync::push() } else { crate::sync::pull() } }).await;
            this.update(cx, |this, cx| {
                this.end_task(task, cx);
                let ok = result.is_ok();
                if ok && !push {
                    // Apply what was downloaded.
                    crate::theme::init(cx);
                    crate::keymap::apply(cx);
                }
                this.output = Some(Output {
                    title: "Settings Sync".into(),
                    text: result.unwrap_or_else(|e| e),
                    running: false,
                    ok,
                });
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn open_settings(&mut self, _: &OpenSettings, window: &mut Window, cx: &mut Context<Self>) {
        let dialog = cx.new(|cx| SettingsDialog::new(window, cx));
        self.show_modal(dialog, window, cx);
    }

    pub(super) fn open_themes(
        &mut self,
        _: &OpenThemes,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let dialog = cx.new(|cx| ThemeDialog::new(window, cx));
        self.show_modal(dialog, window, cx);
    }

    pub(super) fn toggle_prefs_menu(
        &mut self,
        _: &TogglePrefsMenu,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.prefs_menu_open = !self.prefs_menu_open;
        self.laravel_menu_open = false;
        cx.notify();
    }

    pub(super) fn open_laravel_settings(
        &mut self,
        _: &OpenLaravelSettings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let root = self.project.root().to_path_buf();
        let dialog = cx.new(|cx| LaravelSettingsDialog::new(&root, window, cx));
        self.show_modal(dialog, window, cx);
    }

    pub(super) fn generate_helper_code(
        &mut self,
        _: &GenerateHelperCode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let root = self.project.root().to_path_buf();
        let dialog = cx.new(|cx| HelperCodeDialog::new(&root, window, cx));
        self.show_modal(dialog, window, cx);
    }

    /// New Eloquent Model from Database Table: list tables, then prefill the model dialog.
    pub(super) fn model_from_table(
        &mut self,
        _: &ModelFromTable,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let root = self.artisan_root();
        let task = self.begin_task("Reading database tables…", cx);
        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { laravel::database::list_tables(&root) })
                .await;
            this.update_in(cx, |this, window, cx| {
                this.end_task(task, cx);
                match result {
                    Ok(tables) => {
                        let items = tables
                            .into_iter()
                            .map(|table| crate::dialogs::NavItem {
                                label: table.clone(),
                                detail: laravel::database::model_name(&table),
                                icon: "▦",
                                target: crate::dialogs::NavTarget::Table(table),
                            })
                            .collect();
                        this.open_list(crate::dialogs::NavMode::Tables, items, window, cx);
                    }
                    Err(err) => {
                        this.output = Some(Output {
                            title: "php artisan db:show".into(),
                            text: err,
                            running: false,
                            ok: false,
                        });
                        cx.notify();
                    }
                }
            })
            .ok();
        })
        .detach();
    }

    fn open_model_for_table(&mut self, table: String, window: &mut Window, cx: &mut Context<Self>) {
        let root = self.artisan_root();
        let project_root = self.project.root().to_path_buf();
        let models = self.intel.models();
        self.dismiss_modal(window, cx);
        let task = self.begin_task(format!("Reading table {table}…"), cx);
        cx.spawn_in(window, async move |this, cx| {
            let fetch_table = table.clone();
            let result = cx
                .background_executor()
                .spawn(async move { laravel::database::table_columns(&root, &fetch_table) })
                .await;
            this.update_in(cx, |this, window, cx| {
                this.end_task(task, cx);
                match result {
                    Ok(columns) => {
                        let (model, spec) = laravel::database::spec_from_table(&table, &columns);
                        let dialog = cx.new(|cx| {
                            let mut dialog =
                                MakeDialog::new(&project_root, MakeKind::Model, models, window, cx);
                            dialog.prefill(&model, &spec, cx);
                            dialog
                        });
                        this.show_modal(dialog, window, cx);
                    }
                    Err(err) => {
                        this.output = Some(Output {
                            title: format!("php artisan db:table {table}"),
                            text: err,
                            running: false,
                            ok: false,
                        });
                        cx.notify();
                    }
                }
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn find_unused_views(
        &mut self,
        _: &FindUnusedViews,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(root) = self.intel.laravel_root() else {
            return;
        };
        let views = laravel::Laravel::for_project(self.project.root()).unused_views();
        if views.is_empty() {
            self.output = Some(Output {
                title: "Find Unused Views".into(),
                text: "No unused views found.".into(),
                running: false,
                ok: true,
            });
            cx.notify();
            return;
        }
        let items = views
            .into_iter()
            .map(|(name, path)| crate::dialogs::NavItem {
                label: name,
                detail: path
                    .strip_prefix(&root)
                    .unwrap_or(&path)
                    .display()
                    .to_string(),
                icon: "▤",
                target: crate::dialogs::NavTarget::Location(php_index::Location {
                    path,
                    range: 0..0,
                }),
            })
            .collect();
        self.open_list(crate::dialogs::NavMode::UnusedViews, items, window, cx);
    }

    /// Opens a single-list navigation dialog with the given items.
    fn open_list(
        &mut self,
        mode: crate::dialogs::NavMode,
        items: Vec<crate::dialogs::NavItem>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let root = self.project.root().to_path_buf();
        let sources = crate::dialogs::NavSources {
            classes: Default::default(),
            symbols: Default::default(),
            actions: Default::default(),
            recent: items,
            files: None,
        };
        let dialog =
            cx.new(|cx| crate::dialogs::NavigateDialog::new(&root, mode, sources, window, cx));
        self.show_modal(dialog, window, cx);
    }

    /// Recomputes helper code in the background and reports the result.
    pub(super) fn run_helpers(&mut self, cx: &mut Context<Self>) {
        let intel = self.intel.clone();
        let task = self.begin_task("Generating helper code…", cx);
        cx.spawn(async move |this, cx| {
            let (models, members) = cx.background_executor().spawn(async move { intel.generate_helpers() }).await;
            this.update(cx, |this, cx| {
                this.end_task(task, cx);
                this.output = Some(Output {
                    title: "Generate Helper Code".into(),
                    text: format!("Helper code generated for {models} model(s): {members} properties and methods."),
                    running: false,
                    ok: true,
                });
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn remove_model_phpdocs(
        &mut self,
        _: &RemoveModelPhpDocs,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let changed = self.intel.remove_model_phpdocs();
        let root = self.project.root().to_path_buf();
        let list = changed
            .iter()
            .map(|p| format!("  - {}", p.strip_prefix(&root).unwrap_or(p).display()))
            .collect::<Vec<_>>()
            .join("\n");
        self.reload_tabs(&changed, cx);
        self.output = Some(Output {
            title: "Remove Model PhpDocs".into(),
            text: if changed.is_empty() {
                "No model phpdocs to remove.".into()
            } else {
                format!("Cleaned {} model(s):\n{list}", changed.len())
            },
            running: false,
            ok: true,
        });
        cx.notify();
    }

    /// Where Artisan runs: the configured Laravel directory.
    pub(super) fn artisan_root(&self) -> PathBuf {
        self.intel
            .laravel_root()
            .unwrap_or_else(|| self.project.root().to_path_buf())
    }

    pub(super) fn toggle_laravel_menu(
        &mut self,
        _: &ToggleLaravelMenu,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.laravel_menu_open = !self.laravel_menu_open;
        cx.notify();
    }

    pub(super) fn open_artisan(
        &mut self,
        _: &OpenArtisan,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let root = self.artisan_root();
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
        self.modal_subscription = Some(cx.subscribe_in(
            &dialog,
            window,
            |this, _, event, window, cx| this.on_dialog_event(event, window, cx),
        ));
        self.modal = Some(AnyView::from(dialog));
        self.modal_anchor = None;
        self.laravel_menu_open = false;
        self.prefs_menu_open = false;
        cx.notify();
    }

    /// Shows a dialog as a dropdown under a title bar button at `left`.
    pub(super) fn show_dropdown<D: gpui::Render + gpui::EventEmitter<DialogEvent>>(
        &mut self,
        dialog: Entity<D>,
        left: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.show_modal(dialog, window, cx);
        self.modal_anchor = Some(left);
    }

    pub(super) fn open_projects(&mut self, _: &OpenProjects, window: &mut Window, cx: &mut Context<Self>) {
        let root = self.project.root().to_path_buf();
        let dialog = cx.new(|cx| crate::dialogs::ProjectDialog::new(&root, window, cx));
        self.show_dropdown(dialog, 6., window, cx);
    }

    fn pick_project_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.dismiss_modal(window, cx);
        let paths = cx.prompt_for_paths(gpui::PathPromptOptions { files: false, directories: true, multiple: false, prompt: None });
        cx.spawn(async move |_, cx| {
            if let Ok(Ok(Some(paths))) = paths.await
                && let Some(path) = paths.into_iter().next()
            {
                let _ = cx.update(|cx| crate::open_project_window(&path, cx));
            }
        })
        .detach();
    }

    fn clone_repository(&mut self, url: String, parent: std::path::PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.dismiss_modal(window, cx);
        let name = url.trim_end_matches('/').trim_end_matches(".git").rsplit(['/', ':']).next().unwrap_or("repo").to_string();
        let target = parent.join(&name);
        let task = self.begin_task(format!("Cloning {name}"), cx);
        self.output = Some(Output { title: "Clone".into(), text: format!("git clone {url} {}", target.display()), running: true, ok: true });
        cx.notify();
        cx.spawn(async move |this, cx| {
            let dir = target.clone();
            let result = cx.background_spawn(async move {
                let _ = std::fs::create_dir_all(dir.parent().unwrap_or(&dir));
                git::Repo::clone_into(&url, &dir)
            }).await;
            let _ = this.update(cx, |this, cx| {
                this.end_task(task, cx);
                let ok = result.is_ok();
                this.output = Some(Output {
                    title: "Clone".into(),
                    text: result.unwrap_or_else(|e| e),
                    running: false,
                    ok,
                });
                if ok {
                    let _ = crate::open_project_window(&target, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn dismiss_modal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.modal = None;
        self.modal_subscription = None;
        self.focus_active(window, cx);
        cx.notify();
    }

    fn on_dialog_event(
        &mut self,
        event: &DialogEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.on_run_dialog_event(event, window, cx) {
            return;
        }
        match event {
            DialogEvent::Run { .. } | DialogEvent::EditRunConfigurations => {}
            DialogEvent::Dismiss => self.dismiss_modal(window, cx),
            DialogEvent::CommandsLoaded(list) => self.artisan_commands = Some(list.clone()),
            DialogEvent::Created(paths) => {
                self.dismiss_modal(window, cx);
                self.files_created(paths, window, cx);
                if laravel::settings::LaravelSettings::load(self.project.root())
                    .model
                    .generate_helper_after
                {
                    self.run_helpers(cx);
                }
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
            DialogEvent::ModelFromTable(table) => {
                self.open_model_for_table(table.clone(), window, cx)
            }
            DialogEvent::RevertFile { path, content } => {
                let (path, content) = (path.clone(), content.clone());
                self.dismiss_modal(window, cx);
                self.revert_file(&path, &content, cx);
            }
            DialogEvent::PickProjectFolder => self.pick_project_folder(window, cx),
            DialogEvent::OpenProject(path) => {
                let path = path.clone();
                self.dismiss_modal(window, cx);
                if let Err(err) = crate::open_project_window(&path, cx) {
                    eprintln!("open project: {err}");
                }
            }
            DialogEvent::CloneRepository { url, parent } => self.clone_repository(url.clone(), parent.clone(), window, cx),
            DialogEvent::Git(command) => {
                let command = command.clone();
                self.dismiss_modal(window, cx);
                self.run_git(command, cx);
            }
            DialogEvent::GenerateHelpers => {
                self.dismiss_modal(window, cx);
                self.run_helpers(cx);
            }
            DialogEvent::SettingsSaved => {
                self.dismiss_modal(window, cx);
                self.intel.reload_settings();
                self.artisan_commands = None;
            }
            DialogEvent::OpenLocation(location) => {
                let location = location.clone();
                self.dismiss_modal(window, cx);
                let from = self.tabs.get(self.active).and_then(|t| {
                    t.read(cx)
                        .buffer
                        .path()
                        .map(|p| (p.to_path_buf(), t.read(cx).buffer.selection.head))
                });
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
            DialogEvent::ReplaceInFiles {
                query,
                replacement,
                paths,
            } => {
                let (query, replacement, paths) =
                    (query.clone(), replacement.clone(), paths.clone());
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
        // Open every generated file (like Laravel Idea), keeping the first one active.
        const MAX_OPENED: usize = 15;
        for path in paths.iter().take(MAX_OPENED).skip(1) {
            self.open_file(path, window, cx);
        }
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
        let root = self.artisan_root();
        let task = self.begin_task(format!("php artisan {line}"), cx);
        cx.spawn(async move |this, cx| {
            let run_line = line.clone();
            let (text, ok) = cx
                .background_executor()
                .spawn(async move { laravel::artisan::run(&root, &run_line) })
                .await;
            this.update(cx, |this, cx| {
                this.end_task(task, cx);
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
            .child({
                let name = self.project.name();
                div()
                    .id("tool-project")
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .px_1p5()
                    .h(px(24.))
                    .rounded_md()
                    .cursor_pointer()
                    .hover(|d| d.bg(theme::hover()))
                    .child(crate::dialogs::avatar(&name, 18.))
                    .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child(SharedString::from(name)))
                    .child(div().text_color(theme::muted()).child("▾"))
                    .on_click(cx.listener(|this, _, window, cx| this.open_projects(&OpenProjects, window, cx)))
            })
            .when_some(self.git.branch.clone(), |d, branch| {
                d.child(tool("tool-branch", "", "").child(format!("⎇ {branch}")).child(div().text_color(theme::muted()).child("▾")).on_click(cx.listener(
                    |this, _, window, cx| this.open_branches_dropdown(window, cx),
                )))
            })
            .child(div().flex_1())
            .child(self.render_run_widget(cx))
            .when(laravel, |d| {
                d.child(
                    div()
                        .flex()
                        .flex_col()
                        .child(tool("tool-laravel", "◈ Laravel", "▾").on_click(cx.listener(
                            |this, _, window, cx| {
                                this.toggle_laravel_menu(&ToggleLaravelMenu, window, cx)
                            },
                        )))
                        .children(self.render_laravel_menu(cx)),
                )
                .child(tool("tool-artisan", "▶ Artisan", "⌃⇧X").on_click(
                    cx.listener(|this, _, window, cx| this.open_artisan(&OpenArtisan, window, cx)),
                ))
            })
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(tool("tool-prefs", "⚙", "").on_click(cx.listener(
                        |this, _, window, cx| this.toggle_prefs_menu(&TogglePrefsMenu, window, cx),
                    )))
                    .children(self.render_prefs_menu(cx)),
            )
    }

    fn render_prefs_menu(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        if !self.prefs_menu_open {
            return None;
        }
        let entries: &[Option<(&'static str, &'static str, Option<Handler>)>] = &[
            Some((
                "Settings…",
                "⌘,",
                Some(|w, win, cx| w.open_settings(&OpenSettings, win, cx)),
            )),
            Some((
                "Keymap…",
                "⌘K ⌘S",
                Some(|w, win, cx| w.open_keymap(&OpenKeymap, win, cx)),
            )),
            Some((
                "Themes…",
                "⌘K ⌘T",
                Some(|w, win, cx| w.open_themes(&OpenThemes, win, cx)),
            )),
            None,
            Some((
                "Laravel Settings…",
                "",
                Some(|w, win, cx| w.open_laravel_settings(&OpenLaravelSettings, win, cx)),
            )),
        ];
        Some(self.popup_menu("prefs-menu", entries, cx))
    }

    fn render_laravel_menu(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        if !self.laravel_menu_open {
            return None;
        }
        let entries: &[Option<(&'static str, &'static str, Option<Handler>)>] = &[
            Some((
                "Code Generation",
                "⇧⌘,",
                Some(|w, win, cx| w.open_make(&OpenMake, win, cx)),
            )),
            Some((
                "New Eloquent Model",
                "",
                Some(|w, win, cx| w.new_model(&NewModel, win, cx)),
            )),
            Some((
                "New Eloquent Model from Database Table",
                "",
                Some(|w, win, cx| w.model_from_table(&ModelFromTable, win, cx)),
            )),
            Some((
                "New Class",
                "",
                Some(|w, win, cx| w.open_make(&OpenMake, win, cx)),
            )),
            Some((
                "New Pivot Table",
                "",
                Some(|w, win, cx| w.new_pivot_table(&NewPivotTable, win, cx)),
            )),
            None,
            Some((
                "Route Search",
                "⌃⇧R",
                Some(|w, win, cx| w.open_route_search(&OpenRouteSearch, win, cx)),
            )),
            None,
            Some((
                "Run Artisan Command",
                "⌃⇧X",
                Some(|w, win, cx| w.open_artisan(&OpenArtisan, win, cx)),
            )),
            None,
            Some((
                "Generate Helper Code",
                "⇧⌘.",
                Some(|w, win, cx| w.generate_helper_code(&GenerateHelperCode, win, cx)),
            )),
            Some((
                "Remove Model PhpDocs…",
                "",
                Some(|w, win, cx| w.remove_model_phpdocs(&RemoveModelPhpDocs, win, cx)),
            )),
            Some((
                "Find Unused Views",
                "",
                Some(|w, win, cx| w.find_unused_views(&FindUnusedViews, win, cx)),
            )),
            None,
            Some((
                "Laravel Settings…",
                "",
                Some(|w, win, cx| w.open_laravel_settings(&OpenLaravelSettings, win, cx)),
            )),
        ];
        Some(self.popup_menu("laravel-menu", entries, cx))
    }

    /// A dropdown menu anchored under its toolbar button.
    fn popup_menu(
        &self,
        id: &'static str,
        entries: &[Option<(&'static str, &'static str, Option<Handler>)>],
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let items = entries.iter().enumerate().map(|(ix, entry)| match entry {
            None => div()
                .my_1()
                .h(px(1.))
                .bg(theme::border_strong())
                .into_any_element(),
            Some((label, hint, handler)) => {
                let enabled = handler.is_some();
                let handler = *handler;
                div()
                    .id((id, ix))
                    .flex()
                    .justify_between()
                    .gap_6()
                    .px_3()
                    .h(px(24.))
                    .items_center()
                    .when(enabled, |d| {
                        d.cursor_pointer().hover(|d| d.bg(theme::selection()))
                    })
                    .when(!enabled, |d| d.text_color(theme::muted()))
                    .child(*label)
                    .child(div().text_color(theme::muted()).child(*hint))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.laravel_menu_open = false;
                        this.prefs_menu_open = false;
                        if let Some(handler) = handler {
                            handler(this, window, cx);
                        }
                        cx.notify();
                    }))
                    .into_any_element()
            }
        });
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
        .with_priority(1)
        .into_any_element()
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
        let lines = lines[skip..]
            .iter()
            .map(|line| div().child(SharedString::from(line.to_string())));
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
                        .child(
                            div()
                                .text_color(theme::ui_text())
                                .child(SharedString::from(output.title.clone())),
                        )
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
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.close_output(&CloseOutput, window, cx)
                                })),
                        ),
                )
                .child(
                    div()
                        .id("output-text")
                        .flex_1()
                        .overflow_y_scroll()
                        .p_2()
                        .font_family(theme::editor_font())
                        .text_size(px(12.))
                        .text_color(theme::text())
                        .children(lines),
                ),
        )
    }

    pub(super) fn render_modal(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let modal = self.modal.clone()?;
        if let Some(left) = self.modal_anchor {
            return Some(
                div()
                    .absolute()
                    .inset_0()
                    .on_mouse_down(gpui::MouseButton::Left, cx.listener(|this, _, window, cx| this.dismiss_modal(window, cx)))
                    .child(div().absolute().top(px(30.)).left(px(left)).child(modal))
                    .into_any_element(),
            );
        }
        Some(
            div()
                .absolute()
                .inset_0()
                .bg(theme::overlay())
                .flex()
                .justify_center()
                .items_start()
                .pt(px(70.))
                .on_mouse_down(
                    gpui::MouseButton::Left,
                    cx.listener(|this, _, window, cx| this.dismiss_modal(window, cx)),
                )
                .child(modal)
                .into_any_element(),
        )
    }
}

type Handler = fn(&mut Workspace, &mut Window, &mut Context<Workspace>);

pub type CommandCache = Option<Arc<Vec<laravel::artisan::ArtisanCommand>>>;
