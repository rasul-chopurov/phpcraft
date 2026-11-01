//! Search and navigation in the workspace: Find/Replace in Files, Search Everywhere (⇧⇧),
//! Go to File / Class / Symbol, Recent Files and Go to Line.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{Action, AppContext, Context, ModifiersChangedEvent, Window, actions};
use php_index::Location;

use super::Workspace;
use super::tools::{NewModel, OpenArtisan, OpenMake, OpenRouteSearch};
use crate::dialogs::{
    FindInFilesDialog, GoToLineDialog, NavItem, NavMode, NavSources, NavTarget, NavigateDialog,
};
use crate::editor_view::{OptimizeImports, ShowFind, ShowReplace};

actions!(
    workspace,
    [
        FindInFiles,
        ReplaceInFiles,
        SearchEverywhere,
        GoToFile,
        GoToClass,
        GoToSymbol,
        RecentFiles,
        GoToLine,
        ShowLocalHistory
    ]
);

const DOUBLE_SHIFT: Duration = Duration::from_millis(350);
const RECENT_LIMIT: usize = 30;

impl Workspace {
    pub(super) fn remember_recent(&mut self, path: &std::path::Path) {
        self.recent.retain(|p| p != path);
        self.recent.insert(0, path.to_path_buf());
        self.recent.truncate(RECENT_LIMIT);
    }

    fn selected_text(&self, cx: &gpui::App) -> Option<String> {
        let text = self.tabs.get(self.active)?.read(cx).buffer.selected_text();
        (!text.is_empty() && !text.contains('\n')).then_some(text)
    }

    pub(super) fn find_in_files(
        &mut self,
        _: &FindInFiles,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_find_in_files(false, window, cx);
    }

    pub(super) fn replace_in_files(
        &mut self,
        _: &ReplaceInFiles,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_find_in_files(true, window, cx);
    }

    fn open_find_in_files(&mut self, replace: bool, window: &mut Window, cx: &mut Context<Self>) {
        let root = self.project.root().to_path_buf();
        let seed = self.selected_text(cx);
        let dialog = cx.new(|cx| FindInFilesDialog::new(&root, replace, seed, window, cx));
        self.show_modal(dialog, window, cx);
    }

    fn actions_list() -> Vec<NavItem> {
        let action = |label: &str, shortcut: &str, action: Box<dyn Action>| NavItem {
            label: label.to_string(),
            detail: shortcut.to_string(),
            icon: "▶",
            target: NavTarget::Action(action),
        };
        vec![
            action("Find in Files", "⇧⌘F", Box::new(FindInFiles)),
            action("Replace in Files", "⇧⌘R", Box::new(ReplaceInFiles)),
            action("Find", "⌘F", Box::new(ShowFind)),
            action("Replace", "⌘R", Box::new(ShowReplace)),
            action("Go to File", "⇧⌘O", Box::new(GoToFile)),
            action("Go to Class", "⌘O", Box::new(GoToClass)),
            action("Go to Symbol", "⌥⌘O", Box::new(GoToSymbol)),
            action("Recent Files", "⌘E", Box::new(RecentFiles)),
            action("Local History: Show History", "⌥⇧H", Box::new(ShowLocalHistory)),
            action("Go to Line", "⌘L", Box::new(GoToLine)),
            action("Optimize Imports", "⌃⌥O", Box::new(OptimizeImports)),
            action("Code Generation (Laravel)", "⇧⌘,", Box::new(OpenMake)),
            action("New Eloquent Model", "", Box::new(NewModel)),
            action("Route Search", "⌃⇧R", Box::new(OpenRouteSearch)),
            action("Run Artisan Command", "⌃⇧X", Box::new(OpenArtisan)),
            action("Settings", "⌘,", Box::new(super::tools::OpenSettings)),
            action("Settings Sync: Push to GitHub", "", Box::new(super::tools::SyncPush)),
            action("Settings Sync: Pull from GitHub", "", Box::new(super::tools::SyncPull)),
            action("Commit…", "⌘0", Box::new(super::git_panel::ShowCommit)),
            action("Git Log", "⌘9", Box::new(super::git_panel::ToggleGitLog)),
            action("Git Branches…", "", Box::new(super::git_panel::OpenBranches)),
            action("Git Pull", "⌘T", Box::new(super::git_panel::GitPull)),
            action("Git Push", "⇧⌘K", Box::new(super::git_panel::GitPush)),
            action("Annotate with Git Blame", "⌥⌘A", Box::new(crate::editor_view::ToggleBlame)),
            action("Keymap", "⌘K ⌘S", Box::new(super::tools::OpenKeymap)),
            action("Select Theme", "⌘K ⌘T", Box::new(super::tools::OpenThemes)),
            action(
                "Generate Helper Code",
                "⇧⌘.",
                Box::new(super::tools::GenerateHelperCode),
            ),
            action(
                "Find Unused Views",
                "",
                Box::new(super::tools::FindUnusedViews),
            ),
            action(
                "New Eloquent Model from Database Table",
                "",
                Box::new(super::tools::ModelFromTable),
            ),
            action(
                "Laravel Settings",
                "",
                Box::new(super::tools::OpenLaravelSettings),
            ),
        ]
    }

    fn nav_sources(&self) -> NavSources {
        let classes = self
            .intel
            .class_entries()
            .into_iter()
            .map(|(name, fqn, location)| NavItem {
                label: name,
                detail: php_index::namespace_of(&fqn).to_string(),
                icon: "C",
                target: NavTarget::Location(location),
            })
            .collect();
        let symbols = self
            .intel
            .symbol_entries()
            .into_iter()
            .map(|(label, container, icon, location)| NavItem {
                label,
                detail: container,
                icon,
                target: NavTarget::Location(location),
            })
            .collect();
        let root = self.project.root();
        let recent = self
            .recent
            .iter()
            .map(|path| NavItem {
                label: path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                detail: path
                    .strip_prefix(root)
                    .unwrap_or(path)
                    .display()
                    .to_string(),
                icon: "▤",
                target: NavTarget::Location(Location {
                    path: path.clone(),
                    range: 0..0,
                }),
            })
            .collect();
        NavSources {
            classes: Arc::new(classes),
            symbols: Arc::new(symbols),
            actions: Arc::new(Self::actions_list()),
            recent,
            files: self.files_cache.clone(),
        }
    }

    pub(super) fn open_navigate(
        &mut self,
        mode: NavMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let root = self.project.root().to_path_buf();
        let sources = self.nav_sources();
        let dialog = cx.new(|cx| NavigateDialog::new(&root, mode, sources, window, cx));
        self.show_modal(dialog, window, cx);
    }

    pub(super) fn search_everywhere(
        &mut self,
        _: &SearchEverywhere,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_navigate(NavMode::All, window, cx);
    }

    pub(super) fn go_to_file(&mut self, _: &GoToFile, window: &mut Window, cx: &mut Context<Self>) {
        self.open_navigate(NavMode::Files, window, cx);
    }

    pub(super) fn go_to_class(
        &mut self,
        _: &GoToClass,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_navigate(NavMode::Classes, window, cx);
    }

    pub(super) fn go_to_symbol(
        &mut self,
        _: &GoToSymbol,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_navigate(NavMode::Symbols, window, cx);
    }

    pub(super) fn recent_files(
        &mut self,
        _: &RecentFiles,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_navigate(NavMode::Recent, window, cx);
    }

    pub(super) fn go_to_line(&mut self, _: &GoToLine, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get(self.active) else {
            return;
        };
        let buffer = &tab.read(cx).buffer;
        let current = buffer.char_to_line_col(buffer.selection.head);
        let dialog = cx.new(|cx| GoToLineDialog::new(current, window, cx));
        self.show_modal(dialog, window, cx);
    }

    pub(super) fn show_local_history(&mut self, _: &ShowLocalHistory, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get(self.active) else { return };
        let buffer = &tab.read(cx).buffer;
        let Some(path) = buffer.path().map(|p| p.to_path_buf()) else { return };
        let current = buffer.text();
        let dialog = cx.new(|cx| crate::dialogs::LocalHistoryDialog::new(path, current, window, cx));
        self.show_modal(dialog, window, cx);
    }

    pub(super) fn revert_file(&mut self, path: &std::path::Path, content: &str, cx: &mut Context<Self>) {
        match self.tabs.iter().find(|t| t.read(cx).buffer.path() == Some(path)).cloned() {
            Some(tab) => tab.update(cx, |editor, cx| editor.replace_all_text(content, cx)),
            None => {
                crate::local_history::record_disk(&[path.to_path_buf()], "Before Revert");
                let _ = std::fs::write(path, content);
            }
        }
    }

    pub(super) fn jump_to_line(&mut self, line: usize, column: usize, cx: &mut Context<Self>) {
        if let Some(tab) = self.tabs.get(self.active) {
            tab.update(cx, |editor, cx| {
                let ix = editor.buffer.line_col_to_char(line, column);
                editor.reveal_char(ix, cx);
            });
        }
    }

    /// Runs Replace in Files off the UI thread, then reloads affected tabs without edits.
    pub(super) fn run_replace_in_files(
        &mut self,
        query: search::TextQuery,
        replacement: String,
        paths: Vec<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        crate::local_history::record_disk(&paths, "Before Replace in Files");
        let task = self.begin_task("Replacing in files…", cx);
        cx.spawn(async move |this, cx| {
            let changed = paths.clone();
            let result = cx
                .background_executor()
                .spawn(async move { search::replace_in_files(&paths, &query, &replacement) })
                .await;
            this.update(cx, |this, cx| {
                this.end_task(task, cx);
                let text = match result {
                    Ok((files, count)) => {
                        format!("Replaced {count} occurrence(s) in {files} file(s).")
                    }
                    Err(err) => format!("Replace failed: {err}"),
                };
                for path in &changed {
                    if let Ok(source) = std::fs::read_to_string(path) {
                        this.intel.update_file(path, &source);
                    }
                }
                this.reload_clean_tabs(&changed, cx);
                this.output = Some(super::tools::Output {
                    title: "Replace in Files".into(),
                    text,
                    running: false,
                    ok: true,
                });
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn reload_tabs(&mut self, paths: &[PathBuf], cx: &mut Context<Self>) {
        self.reload_clean_tabs(paths, cx);
    }

    fn reload_clean_tabs(&mut self, paths: &[PathBuf], cx: &mut Context<Self>) {
        for tab in &self.tabs {
            let path = tab.read(cx).buffer.path().map(|p| p.to_path_buf());
            if let Some(path) = path.filter(|p| paths.contains(p)) {
                tab.update(cx, |editor, cx| editor.reload_if_clean(&path, cx));
            }
        }
    }

    /// ⇧⇧: two taps of Shift alone in quick succession.
    pub(super) fn on_modifiers(
        &mut self,
        event: &ModifiersChangedEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let m = event.modifiers;
        let others = m.control || m.alt || m.platform || m.function;
        if m.shift && !others {
            self.shift_clean = true;
            return;
        }
        if !m.shift && !others && std::mem::take(&mut self.shift_clean) {
            let now = Instant::now();
            if self
                .last_shift_tap
                .is_some_and(|t| now.duration_since(t) < DOUBLE_SHIFT)
                && self.modal.is_none()
            {
                self.last_shift_tap = None;
                self.search_everywhere(&SearchEverywhere, window, cx);
            } else {
                self.last_shift_tap = Some(now);
            }
        } else {
            self.shift_clean = false;
        }
    }

    pub(super) fn on_any_key(&mut self) {
        self.shift_clean = false;
        self.last_shift_tap = None;
    }
}
