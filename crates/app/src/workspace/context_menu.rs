//! Right-click menus for the project tree and editor tabs, like PhpStorm's.

use std::path::{Path, PathBuf};

use gpui::{AppContext, ClipboardItem, Context, Focusable, MouseButton, Pixels, Point, SharedString, Window, div, prelude::*, px};

use super::Workspace;
use crate::dialogs::{ConfirmDialog, DialogEvent, FileOp, GitCommand, InputDialog};
use crate::theme;

#[derive(Clone)]
pub(super) enum MenuCommand {
    File(FileOp),
    CopyPath(PathBuf),
    CopyRelativePath(PathBuf),
    Reveal(PathBuf),
    OpenInTerminal(PathBuf),
    LocalHistory(PathBuf),
    ShowDiff(PathBuf),
    Rollback(PathBuf),
    AddToAgent(PathBuf),
    ReloadFromDisk(PathBuf),
    SelectInProject(PathBuf),
    OpenInSplit(PathBuf),
    CloseTab(usize),
    CloseOthers(usize),
    CloseAll,
    CloseLeft(usize),
    CloseRight(usize),
    TogglePin(usize),
    /// Terminal tab commands (by tab index).
    TerminalRename(usize),
    TerminalClose(usize),
    TerminalCloseOthers(usize),
    TerminalCloseAll,
    TerminalSelect(usize),
    /// Moves a tool window to another edge.
    Dock(super::layout::ToolWindow, super::layout::Side),
    HideTool(super::layout::ToolWindow),
    /// Opens a submenu next to the item at this index.
    Submenu(usize, Vec<Option<(&'static str, &'static str, MenuCommand, bool)>>),
    Action(&'static str),
}

type Items = Vec<Option<(&'static str, &'static str, MenuCommand, bool)>>;

pub(super) struct ContextMenu {
    position: Point<Pixels>,
    /// `None` is a separator.
    items: Items,
    /// Open submenu: parent item index and its items.
    submenu: Option<(usize, Items)>,
}

fn new_items(dir: &Path) -> Items {
    use crate::templates::Template;
    let mut items: Items = vec![
        Some(("File…", "", MenuCommand::File(FileOp::NewFile(dir.to_path_buf())), true)),
        Some(("Directory…", "", MenuCommand::File(FileOp::NewDirectory(dir.to_path_buf())), true)),
        None,
    ];
    for template in Template::ALL {
        items.push(Some((template.label(), "", MenuCommand::File(FileOp::FromTemplate(dir.to_path_buf(), template)), true)));
        if matches!(template, Template::PestTest | Template::Vue) {
            items.push(None);
        }
    }
    items.push(None);
    items.push(Some(("Laravel: Code Generation…", "⇧⌘,", MenuCommand::Action("workspace::OpenMake"), true)));
    items.push(Some(("Laravel: Eloquent Model…", "", MenuCommand::Action("workspace::NewModel"), true)));
    items
}

impl Workspace {
    fn git_changed(&self, path: &Path) -> bool {
        let relative = path.strip_prefix(self.project.root()).unwrap_or(path).to_string_lossy().replace('\\', "/");
        self.git.status.iter().any(|f| f.path == relative)
    }

    fn file_items(&self, path: &Path, is_dir: bool) -> Vec<Option<(&'static str, &'static str, MenuCommand, bool)>> {
        let dir = if is_dir { path.to_path_buf() } else { path.parent().unwrap_or(path).to_path_buf() };
        let p = path.to_path_buf();
        let changed = self.git_changed(path);
        vec![
            Some(("New", "›", MenuCommand::Submenu(0, new_items(&dir)), true)),
            None,
            Some(("Open in Right Split", "", MenuCommand::OpenInSplit(p.clone()), !is_dir)),
            None,
            Some(("Copy Path", "", MenuCommand::CopyPath(p.clone()), true)),
            Some(("Copy Relative Path", "", MenuCommand::CopyRelativePath(p.clone()), true)),
            None,
            Some(("Rename…", "⇧F6", MenuCommand::File(FileOp::Rename(p.clone())), true)),
            Some(("Delete…", "⌫", MenuCommand::File(FileOp::Delete(p.clone())), true)),
            None,
            Some(("Add to Agent Context", "", MenuCommand::AddToAgent(p.clone()), true)),
            Some(("Open in Terminal", "", MenuCommand::OpenInTerminal(dir), true)),
            Some((if cfg!(target_os = "macos") { "Reveal in Finder" } else { "Show in Folder" }, "", MenuCommand::Reveal(p.clone()), true)),
            None,
            Some(("Local History", "⌥⇧H", MenuCommand::LocalHistory(p.clone()), !is_dir)),
            Some(("Git: Show Diff", "", MenuCommand::ShowDiff(p.clone()), changed)),
            Some(("Git: Rollback…", "", MenuCommand::Rollback(p.clone()), changed)),
            Some(("Reload from Disk", "", MenuCommand::ReloadFromDisk(p), !is_dir)),
        ]
    }

    /// Shows a context menu with `items` at `position`.
    pub(super) fn open_menu_at(&mut self, position: Point<Pixels>, items: Items, cx: &mut Context<Self>) {
        self.context_menu = Some(ContextMenu { position, items, submenu: None });
        cx.notify();
    }

    pub(super) fn open_tree_menu(&mut self, path: PathBuf, is_dir: bool, position: Point<Pixels>, cx: &mut Context<Self>) {
        self.context_menu = Some(ContextMenu { position, items: self.file_items(&path, is_dir), submenu: None });
        cx.notify();
    }

    pub(super) fn open_tab_menu(&mut self, ix: usize, position: Point<Pixels>, cx: &mut Context<Self>) {
        let mut items = vec![
            Some(("Close", "⌘W", MenuCommand::CloseTab(ix), true)),
            Some(("Close Other Tabs", "", MenuCommand::CloseOthers(ix), self.tabs.len() > 1)),
            Some(("Close All Tabs", "", MenuCommand::CloseAll, true)),
            Some(("Close Tabs to the Left", "", MenuCommand::CloseLeft(ix), ix > 0)),
            Some(("Close Tabs to the Right", "", MenuCommand::CloseRight(ix), ix + 1 < self.tabs.len())),
            Some(("Close Unmodified Tabs", "", MenuCommand::Action("workspace::CloseUnmodifiedTabs"), true)),
            Some((if self.is_pinned(ix, cx) { "Unpin Tab" } else { "Pin Tab" }, "", MenuCommand::TogglePin(ix), true)),
            Some(("Reopen Closed Tab", "", MenuCommand::Action("workspace::ReopenClosedTab"), !self.closed_tabs.is_empty())),
            Some(("Split Right", "⌘\\", MenuCommand::Action("workspace::SplitRight"), self.pane_count() < super::panes::MAX_PANES)),
            None,
        ];
        if let Some(path) = self.tabs.get(ix).and_then(|t| t.read(cx).buffer.path().map(Path::to_path_buf)) {
            items.push(Some(("Select in Project", "", MenuCommand::SelectInProject(path.clone()), true)));
            items.extend(self.file_items(&path, false).into_iter().skip(2));
        }
        self.context_menu = Some(ContextMenu { position, items, submenu: None });
        cx.notify();
    }

    fn run_menu_command(&mut self, command: MenuCommand, window: &mut Window, cx: &mut Context<Self>) {
        if let MenuCommand::Submenu(ix, items) = command {
            if let Some(menu) = self.context_menu.as_mut() {
                menu.submenu = Some((ix, items));
            }
            cx.notify();
            return;
        }
        self.context_menu = None;
        let root = self.project.root().to_path_buf();
        let relative = |p: &Path| p.strip_prefix(&root).unwrap_or(p).to_string_lossy().replace('\\', "/");
        match command {
            MenuCommand::File(FileOp::FromTemplate(dir, template)) if !template.needs_name() => {
                self.apply_file_op(FileOp::FromTemplate(dir, template), "", window, cx);
            }
            MenuCommand::File(op) => {
                let dialog = cx.new(|cx| InputDialog::new(op, window, cx));
                self.show_modal(dialog, window, cx);
            }
            MenuCommand::Submenu(..) => {}
            MenuCommand::Action(name) => {
                if let Ok(action) = cx.build_action(name, None) {
                    window.dispatch_action(action, cx);
                }
            }
            MenuCommand::CopyPath(p) => cx.write_to_clipboard(ClipboardItem::new_string(p.display().to_string())),
            MenuCommand::CopyRelativePath(p) => cx.write_to_clipboard(ClipboardItem::new_string(relative(&p))),
            MenuCommand::Reveal(p) => cx.reveal_path(&p),
            MenuCommand::OpenInTerminal(dir) => {
                if let Some(view) = self.add_terminal_at(None, dir, cx) {
                    window.focus(&view.focus_handle(cx));
                }
            }
            MenuCommand::LocalHistory(p) => {
                let current = self
                    .tabs
                    .iter()
                    .find(|t| t.read(cx).buffer.path() == Some(p.as_path()))
                    .map(|t| t.read(cx).buffer.text())
                    .or_else(|| std::fs::read_to_string(&p).ok())
                    .unwrap_or_default();
                let dialog = cx.new(|cx| crate::dialogs::LocalHistoryDialog::new(p, current, window, cx));
                self.show_modal(dialog, window, cx);
            }
            MenuCommand::ShowDiff(p) => self.show_file_diff(&relative(&p), window, cx),
            MenuCommand::Rollback(p) => {
                let rel = relative(&p);
                let files: Vec<_> = self.git.status.iter().filter(|f| f.path == rel).cloned().collect();
                let dialog = cx.new(|cx| ConfirmDialog::new(format!("Roll back changes in {rel}?"), GitCommand::Rollback(files), window, cx));
                self.show_modal(dialog, window, cx);
            }
            MenuCommand::AddToAgent(p) => {
                let mention = format!("`{}` ", relative(&p));
                if !self.layout.is_active(super::layout::ToolWindow::Ai) {
                    self.toggle_agent(&super::agent_panel::ToggleAgent, window, cx);
                }
                self.ensure_agent_input(cx);
                if let Some(input) = self.agent.input.clone() {
                    input.update(cx, |editor, cx| {
                        let text = editor.buffer.text();
                        editor.replace_all_text(&format!("{text}{mention}"), cx);
                    });
                    window.focus(&input.focus_handle(cx));
                }
            }
            MenuCommand::ReloadFromDisk(p) => {
                if let Some(tab) = self.tabs.iter().find(|t| t.read(cx).buffer.path() == Some(p.as_path())).cloned()
                    && let Ok(text) = std::fs::read_to_string(&p)
                {
                    tab.update(cx, |editor, cx| editor.replace_all_text(&text, cx));
                }
            }
            MenuCommand::SelectInProject(p) => {
                self.project.reveal(&p);
                self.rows = self.project.rows();
                self.selected_path = Some(p);
                self.layout.show(super::layout::ToolWindow::Project);
            }
            MenuCommand::OpenInSplit(p) => self.open_in_right_split(&p, window, cx),
            MenuCommand::CloseTab(ix) => self.close_tab_at(ix, window, cx),
            MenuCommand::CloseOthers(ix) => self.close_tabs_where(false, move |i, _, _| i != ix, window, cx),
            MenuCommand::CloseAll => self.close_tabs_where(false, |_, _, _| true, window, cx),
            MenuCommand::CloseLeft(ix) => self.close_tabs_where(false, move |i, _, _| i < ix, window, cx),
            MenuCommand::CloseRight(ix) => self.close_tabs_where(false, move |i, _, _| i > ix, window, cx),
            MenuCommand::TogglePin(ix) => self.toggle_pin_at(ix, cx),
            MenuCommand::TerminalRename(ix) => self.start_terminal_rename(ix, window, cx),
            MenuCommand::TerminalClose(ix) => self.close_terminal_tab(ix, window, cx),
            MenuCommand::TerminalCloseOthers(ix) => self.close_other_terminals(ix, cx),
            MenuCommand::TerminalCloseAll => {
                for i in (0..self.terminal_count()).rev() {
                    self.close_terminal_tab(i, window, cx);
                }
            }
            MenuCommand::TerminalSelect(ix) => self.select_terminal(ix, window, cx),
            MenuCommand::Dock(tool, side) => {
                self.layout.dock(tool, side);
                self.layout.show(tool);
                self.layout.save();
            }
            MenuCommand::HideTool(tool) => {
                self.layout.hide(tool);
                self.focus_active(window, cx);
            }
        }
        cx.notify();
    }

    /// Applies a confirmed New File / New Directory / Rename / Delete.
    pub(super) fn apply_file_op(&mut self, op: FileOp, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let result: Result<Option<PathBuf>, String> = match &op {
            FileOp::NewFile(dir) => {
                let path = dir.join(name);
                (|| {
                    if path.exists() {
                        return Err(format!("{name} already exists"));
                    }
                    std::fs::create_dir_all(path.parent().unwrap_or(dir)).map_err(|e| e.to_string())?;
                    let initial = if name.ends_with(".php") && !name.ends_with(".blade.php") { "<?php\n\n" } else { "" };
                    std::fs::write(&path, initial).map_err(|e| e.to_string())?;
                    Ok(Some(path.clone()))
                })()
            }
            FileOp::NewDirectory(dir) => std::fs::create_dir_all(dir.join(name)).map(|_| None).map_err(|e| e.to_string()),
            FileOp::Rename(path) => {
                let target = path.with_file_name(name);
                if target.exists() {
                    Err(format!("{name} already exists"))
                } else {
                    std::fs::rename(path, &target).map(|_| {
                        // Keep open tabs pointing at the file.
                        for tab in &self.tabs {
                            tab.update(cx, |editor, _| editor.buffer.rename_path(path, &target));
                        }
                        None
                    }).map_err(|e| e.to_string())
                }
            }
            FileOp::FromTemplate(dir, template) => {
                let files = crate::templates::render(*template, name, dir, self.project.root());
                (|| {
                    if let Some((existing, _)) = files.iter().find(|(p, _)| p.exists()) {
                        return Err(format!("{} already exists", existing.display()));
                    }
                    for (path, content) in &files {
                        std::fs::create_dir_all(path.parent().unwrap_or(dir)).map_err(|e| e.to_string())?;
                        std::fs::write(path, content).map_err(|e| e.to_string())?;
                    }
                    Ok(files.first().map(|(p, _)| p.clone()))
                })()
            }
            FileOp::Delete(path) => {
                crate::local_history::record_disk(&[path.clone()], "Before Delete");
                trash::delete(path).map(|_| None).map_err(|e| e.to_string())
            }
            FileOp::RenameSymbol(symbol) => self.rename_symbol(symbol, name, cx),
        };
        match result {
            Ok(created) => {
                self.dismiss_modal(window, cx);
                if let FileOp::Delete(path) = &op {
                    let gone: Vec<usize> = (0..self.tabs.len()).rev().filter(|&i| self.tabs[i].read(cx).buffer.path().is_some_and(|p| p.starts_with(path))).collect();
                    for i in gone {
                        self.close_tab_at(i, window, cx);
                    }
                }
                self.rows = self.project.rows();
                self.files_cache = None;
                self.rescan(cx);
                if let Some(path) = created {
                    self.open_file(&path, window, cx);
                }
            }
            Err(err) => {
                self.output = Some(super::tools::Output { title: "Files".into(), text: err, running: false, ok: false });
                self.dismiss_modal(window, cx);
            }
        }
        cx.notify();
    }

    pub(super) fn render_context_menu(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let menu = self.context_menu.as_ref()?;
        let column = |id: &'static str, items: &Items, cx: &mut Context<Self>| {
            let rows = items.iter().enumerate().map(|(ix, item)| match item {
                None => div().my_1().h(px(1.)).bg(theme::border_strong()).into_any_element(),
                Some((label, hint, command, enabled)) => {
                    let command = command.clone();
                    let enabled = *enabled;
                    let open_sub = matches!(&command, MenuCommand::Submenu(..));
                    div()
                        .id((id, ix))
                        .flex()
                        .justify_between()
                        .gap_6()
                        .mx_1()
                        .px_2()
                        .h(px(24.))
                        .items_center()
                        .rounded_md()
                        .when(enabled, |d| d.cursor_pointer().text_color(theme::ui_text()).hover(|d| d.bg(theme::selection())))
                        .when(!enabled, |d| d.text_color(theme::muted()))
                        .child(SharedString::from(*label))
                        .child(div().text_color(theme::muted()).child(SharedString::from(*hint)))
                        .when(enabled, |d| {
                            let hover_command = command.clone();
                            d.on_click(cx.listener(move |this, _, window, cx| this.run_menu_command(command.clone(), window, cx)))
                                // Submenus also open on hover, like native menus.
                                .when(open_sub, |d| {
                                    d.on_hover(cx.listener(move |this, hovered: &bool, window, cx| {
                                        if *hovered {
                                            this.run_menu_command(hover_command.clone(), window, cx);
                                        }
                                    }))
                                })
                        })
                        .into_any_element()
                }
            });
            div()
                .id(id)
                .occlude()
                .min_w(px(240.))
                .py_1()
                .bg(theme::panel_bg())
                .border_1()
                .border_color(theme::border_strong())
                .rounded_md()
                .shadow_lg()
                .text_size(px(13.))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .children(rows.collect::<Vec<_>>())
        };
        let main = column("ctx-main", &menu.items, cx);
        let sub = menu.submenu.as_ref().map(|(ix, items)| {
            // Line the submenu up with its parent row.
            let offset = menu.items[..*ix].iter().map(|i| if i.is_some() { 24. } else { 9. }).sum::<f32>();
            div().pt(px(offset)).child(column("ctx-sub", items, cx))
        });
        Some(
            div()
                .absolute()
                .inset_0()
                .occlude()
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                    this.context_menu = None;
                    cx.notify();
                }))
                .on_mouse_down(MouseButton::Right, cx.listener(|this, _, _, cx| {
                    this.context_menu = None;
                    cx.notify();
                }))
                .child(gpui::deferred(
                    gpui::anchored().position(menu.position).snap_to_window().child(div().flex().items_start().gap_1().child(main).children(sub)),
                )),
        )
    }
}

impl DialogEvent {
    pub(super) fn file_op(&self) -> Option<(FileOp, String)> {
        match self {
            DialogEvent::FileOp { op, name } => Some((op.clone(), name.clone())),
            _ => None,
        }
    }
}
