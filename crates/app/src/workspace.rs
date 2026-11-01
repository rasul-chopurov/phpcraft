//! Main window: project tree, editor tabs and status bar.

mod git_panel;
mod terminal_panel;
mod layout;
mod context_menu;
mod agent_panel;
mod db_panel;
mod laravel_panel;
mod panes;
mod git_window;
pub use git_panel::ShowCommit;
pub(crate) use agent_panel::{load_config as load_agent_config, save_config as save_agent_config};
pub use tools::{OpenKeymap, OpenLaravelSettings, SyncPull, SyncPush};
mod navigation;
mod tools;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use editor::Buffer;
use gpui::{
    Context, Entity, FocusHandle, Focusable, MouseButton, SharedString, Window, actions, div,
    prelude::*, px,
};
use project::{Project, Row};

use crate::editor_view::EditorView;
use crate::intel::Intel;
use crate::theme;
use php_index::Location;

actions!(workspace, [CloseTab, NextTab, PrevTab, NavigateBack]);

pub struct Workspace {
    project: Project,
    rows: Vec<Row>,
    tabs: Vec<Entity<EditorView>>,
    active: usize,
    selected_path: Option<PathBuf>,
    focus_handle: FocusHandle,
    intel: Arc<Intel>,
    /// Positions to return to with ⌘[ (file, caret char index).
    history: Vec<(PathBuf, usize)>,
    modal: Option<gpui::AnyView>,
    modal_subscription: Option<gpui::Subscription>,
    /// Left offset of a title bar dropdown; `None` centres the modal.
    modal_anchor: Option<f32>,
    /// Anchor the dropdown above the status bar instead of below the title bar.
    modal_at_bottom: bool,
    terminals: terminal_panel::TerminalState,
    agent: agent_panel::AgentState,
    db: db_panel::DbState,
    laravel: laravel_panel::LaravelState,
    /// Split editor panes; the focused one's tabs live in `tabs`/`active`.
    panes: Vec<Option<panes::Pane>>,
    focused_pane: usize,
    output: Option<tools::Output>,
    artisan_commands: tools::CommandCache,
    laravel_menu_open: bool,
    prefs_menu_open: bool,
    layout: layout::Layout,
    context_menu: Option<context_menu::ContextMenu>,
    /// Outline of the active file for (path, buffer version).
    structure: std::cell::RefCell<Option<(PathBuf, u64, std::rc::Rc<Vec<crate::structure::StructItem>>)>>,
    /// The diff tab (one at a time, like PhpStorm's preview diff) and whether it is shown.
    diff: Option<Entity<crate::diff_view::DiffView>>,
    diff_active: bool,
    _diff_subscription: Option<gpui::Subscription>,
    git: git_panel::GitState,
    _git_poll: Option<gpui::Task<()>>,
    /// Running background work shown in the status bar: (id, label).
    tasks: Vec<(u64, String)>,
    next_task: u64,
    recent: Vec<PathBuf>,
    files_cache: Option<Arc<Vec<crate::dialogs::NavItem>>>,
    shift_clean: bool,
    last_shift_tap: Option<std::time::Instant>,
}

impl Workspace {
    pub fn new(project: Project, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let rows = project.rows();
        let intel = Intel::new(project.root());
        let scanning = intel.clone();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .spawn(async move { scanning.scan() })
                .await;
            this.update(cx, |this, cx| this.end_task(0, cx)).ok();
        })
        .detach();
        // Save everything when the window loses focus (PhpStorm's "save on frame deactivation").
        cx.observe_window_activation(window, |this, window, cx| {
            if !window.is_window_active()
                && crate::config::Settings::load_cached().auto_save != crate::config::AutoSave::Off
            {
                this.save_all(cx);
            }
        })
        .detach();
        Self {
            layout: layout::Layout::load(),
            context_menu: None,
            structure: Default::default(),
            diff: None,
            diff_active: false,
            _diff_subscription: None,
            git: {
                let mut git = git_panel::GitState::new(intel.clone(), cx);
                git.watch_filters(cx);
                git
            },
            _git_poll: None,
            tasks: vec![(0, "Indexing…".into())],
            next_task: 1,
            intel,
            history: Vec::new(),
            modal: None,
            modal_subscription: None,
            modal_anchor: None,
            modal_at_bottom: false,
            terminals: Default::default(),
            agent: Default::default(),
            db: Default::default(),
            laravel: Default::default(),
            panes: vec![None],
            focused_pane: 0,
            output: None,
            artisan_commands: None,
            laravel_menu_open: false,
            prefs_menu_open: false,
            recent: Vec::new(),
            files_cache: None,
            shift_clean: false,
            last_shift_tap: None,
            project,
            rows,
            tabs: Vec::new(),
            active: 0,
            selected_path: None,
            focus_handle: cx.focus_handle(),
        }
    }

    pub fn title(&self) -> String {
        format!("{} – PhpCraft", self.project.name())
    }

    fn click_row(&mut self, ix: usize, clicks: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self.rows.get(ix) else { return };
        let path = row.entry.path.clone();
        self.selected_path = Some(path.clone());
        if row.entry.is_dir {
            if clicks == 1 {
                self.project.toggle(&path);
                self.rows = self.project.rows();
            }
        } else if clicks >= 2 || crate::config::Settings::load_cached().open_on_single_click {
            self.open_file(&path, window, cx);
        }
        cx.notify();
    }

    pub fn open_file(&mut self, path: &Path, window: &mut Window, cx: &mut Context<Self>) {
        self.remember_recent(path);
        // A file lives in one pane: jump there if it's open anywhere.
        if let Some((pane, ix)) = self.pane_with(path, cx) {
            self.focus_pane(pane, window, cx);
            self.activate(ix, window, cx);
            return;
        }
        let buffer = match Buffer::open(path) {
            Ok(buffer) => {
                crate::local_history::record(path, &buffer.text(), "Opened");
                buffer
            }
            Err(err) => {
                eprintln!("cannot open {}: {err:#}", path.display());
                return;
            }
        };
        let intel = self.intel.clone();
        let workspace = cx.weak_entity();
        let editor = cx.new(|cx| {
            let mut editor = EditorView::new(buffer, intel, workspace, cx);
            editor.refresh_git_base(cx);
            editor
        });
        cx.observe(&editor, |_, _, cx| cx.notify()).detach();
        self.tabs.push(editor);
        self.activate(self.tabs.len() - 1, window, cx);
    }

    /// Opens `location` (a byte range), remembering `from` for ⌘[.
    pub fn open_location(
        &mut self,
        location: &Location,
        from: Option<(PathBuf, usize)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(from) = from {
            self.history.push(from);
        }
        self.open_file(&location.path, window, cx);
        let start = location.range.start;
        if let Some(tab) = self.active_tab_for(&location.path, cx) {
            tab.update(cx, |editor, cx| editor.reveal_byte(start, cx));
        }
        self.selected_path = Some(location.path.clone());
    }

    fn active_tab_for(&self, path: &Path, cx: &gpui::App) -> Option<Entity<EditorView>> {
        self.tabs
            .iter()
            .find(|tab| tab.read(cx).buffer.path() == Some(path))
            .cloned()
    }

    fn navigate_back(&mut self, _: &NavigateBack, window: &mut Window, cx: &mut Context<Self>) {
        let Some((path, char_ix)) = self.history.pop() else {
            return;
        };
        self.open_file(&path, window, cx);
        if let Some(tab) = self.active_tab_for(&path, cx) {
            tab.update(cx, |editor, cx| editor.reveal_char(char_ix, cx));
        }
    }

    fn focus_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.tabs.get(self.active) {
            Some(tab) => window.focus(&tab.focus_handle(cx)),
            None => window.focus(&self.focus_handle),
        }
    }

    pub(crate) fn begin_task(&mut self, label: impl Into<String>, cx: &mut Context<Self>) -> u64 {
        let id = self.next_task;
        self.next_task += 1;
        self.tasks.push((id, label.into()));
        cx.notify();
        id
    }

    pub(crate) fn end_task(&mut self, id: u64, cx: &mut Context<Self>) {
        self.tasks.retain(|(t, _)| *t != id);
        cx.notify();
    }

    pub(crate) fn save_all(&mut self, cx: &mut Context<Self>) {
        for tab in self.tabs.clone() {
            tab.update(cx, |editor, cx| editor.save_now(cx));
        }
    }

    fn render_diff_tab(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let diff = self.diff.as_ref()?;
        let title = diff.read(cx).tab_title();
        let active = self.diff_active;
        Some(
            div()
                .id("diff-tab")
                .flex()
                .items_center()
                .gap_1p5()
                .pl_3()
                .pr_1p5()
                .h_full()
                .border_r_1()
                .border_color(theme::border())
                .when(active, |d| d.bg(theme::tab_active()).text_color(theme::accent()))
                .when(!active, |d| d.text_color(theme::muted()).hover(|d| d.bg(theme::hover())))
                .child("⇄")
                .child(title)
                .child(
                    div()
                        .id("diff-tab-close")
                        .px_1()
                        .rounded_sm()
                        .text_color(theme::muted())
                        .hover(|d| d.bg(theme::hover()))
                        .child("×")
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.diff = None;
                            this.diff_active = false;
                            this.focus_active(window, cx);
                            cx.notify();
                        })),
                )
                .on_click(cx.listener(|this, _, window, cx| {
                    this.diff_active = true;
                    if let Some(diff) = &this.diff {
                        window.focus(&diff.focus_handle(cx));
                    }
                    cx.notify();
                })),
        )
    }

    fn activate(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.diff_active = false;
        if ix != self.active
            && crate::config::Settings::load_cached().auto_save != crate::config::AutoSave::Off
            && let Some(previous) = self.tabs.get(self.active).cloned()
        {
            previous.update(cx, |editor, cx| editor.save_now(cx));
        }
        self.active = ix;
        if let Some(tab) = self.tabs.get(ix) {
            window.focus(&tab.focus_handle(cx));
            // Follow the active editor in the project tree.
            if crate::config::Settings::load_cached().select_opened_file
                && let Some(path) = tab.read(cx).buffer.path().map(|p| p.to_path_buf())
            {
                self.project.reveal(&path);
                self.rows = self.project.rows();
                self.selected_path = Some(path);
            }
        }
        cx.notify();
    }

    fn close_tab_at(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if ix >= self.tabs.len() {
            return;
        }
        self.tabs.remove(ix);
        let next = self.active.min(self.tabs.len().saturating_sub(1));
        if self.tabs.is_empty() && self.pane_count() > 1 {
            self.drop_empty_pane(window, cx);
        } else if self.tabs.is_empty() {
            window.focus(&self.focus_handle);
            cx.notify();
        } else {
            self.activate(next, window, cx);
        }
    }

    fn close_tab(&mut self, _: &CloseTab, window: &mut Window, cx: &mut Context<Self>) {
        self.close_tab_at(self.active, window, cx);
    }

    fn next_tab(&mut self, _: &NextTab, window: &mut Window, cx: &mut Context<Self>) {
        if !self.tabs.is_empty() {
            self.activate((self.active + 1) % self.tabs.len(), window, cx);
        }
    }

    fn prev_tab(&mut self, _: &PrevTab, window: &mut Window, cx: &mut Context<Self>) {
        if !self.tabs.is_empty() {
            let len = self.tabs.len();
            self.activate((self.active + len - 1) % len, window, cx);
        }
    }

    fn render_project_tree(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let rows = self.rows.iter().enumerate().map(|(ix, row)| {
            let selected = self.selected_path.as_deref() == Some(row.entry.path.as_path());
            let (icon, icon_color) = file_icon(&row.entry.name, row.entry.is_dir, row.expanded);
            div()
                .id(ix)
                .mx_1()
                .pl(px(6. + 12. * row.depth as f32))
                .pr_2()
                .h(px(24.))
                .flex()
                .items_center()
                .gap_1p5()
                .rounded_md()
                .text_color(theme::ui_text())
                .when(selected, |d| d.bg(theme::active_row()))
                .when(!selected, |d| d.hover(|d| d.bg(theme::hover())))
                .child(
                    div()
                        .w(px(10.))
                        .flex_none()
                        .text_size(px(9.))
                        .text_color(theme::muted())
                        .child(match (row.entry.is_dir, row.expanded) {
                            (true, true) => "▼",
                            (true, false) => "▶",
                            _ => "",
                        }),
                )
                .child(div().w(px(16.)).flex_none().flex().justify_center().text_size(px(11.)).text_color(icon_color).child(icon))
                .child(div().overflow_hidden().whitespace_nowrap().child(SharedString::from(row.entry.name.clone())))
                .on_click(cx.listener(move |this, event: &gpui::ClickEvent, window, cx| {
                    this.click_row(ix, event.click_count(), window, cx)
                }))
                .on_mouse_down(MouseButton::Right, {
                    let path = row.entry.path.clone();
                    let is_dir = row.entry.is_dir;
                    cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                        this.selected_path = Some(path.clone());
                        this.open_tree_menu(path.clone(), is_dir, event.position, cx);
                    })
                })
        });
        let rows: Vec<_> = rows.collect();
        div().id("project-tree").flex_1().overflow_y_scroll().pb_2().children(rows)
    }

    fn render_tabs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let tabs = self.tabs.iter().enumerate().map(|(ix, tab)| {
            let buffer = &tab.read(cx).buffer;
            let title: SharedString = buffer.title().into();
            let dirty = buffer.is_dirty();
            let (icon, icon_color) = file_icon(&buffer.title(), false, false);
            let active = ix == self.active;
            div()
                .id(("tab", ix))
                .group("tab")
                .flex()
                .items_center()
                .gap_1p5()
                .pl_3()
                .pr_1p5()
                .h_full()
                .border_r_1()
                .border_color(theme::border())
                .when(active, |d| d.bg(theme::tab_active()).text_color(theme::ui_text()))
                .when(!active, |d| d.text_color(theme::muted()).hover(|d| d.bg(theme::hover())))
                .child(div().text_size(px(11.)).text_color(icon_color).child(icon))
                .child(title)
                .child(
                    div()
                        .id(("close", ix))
                        .w(px(16.))
                        .h(px(16.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_sm()
                        .text_color(theme::muted())
                        .hover(|d| d.bg(theme::hover()).text_color(theme::ui_text()))
                        // A dot for unsaved changes; the close button shows on hover.
                        .when(dirty, |d| d.child(div().group_hover("tab", |d| d.hidden()).child("●")))
                        .child(
                            div()
                                .when(dirty, |d| d.hidden().group_hover("tab", |d| d.block()))
                                .when(!dirty && !active, |d| d.invisible().group_hover("tab", |d| d.visible()))
                                .child("×"),
                        )
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(cx.listener(move |this, _, window, cx| this.close_tab_at(ix, window, cx))),
                )
                .on_click(cx.listener(move |this, _, window, cx| this.activate(ix, window, cx)))
                .on_mouse_down(MouseButton::Right, cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| this.open_tab_menu(ix, event.position, cx)))
        });
        div()
            .id("tabs")
            .flex()
            .flex_none()
            .h(px(32.))
            .w_full()
            .overflow_x_scroll()
            .bg(theme::panel_bg())
            .border_b_1()
            .border_color(theme::border())
            .text_size(px(13.))
            .children(tabs)
            .children(self.render_diff_tab(cx))
    }

    fn render_status(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let info: SharedString = match self.tabs.get(self.active) {
            Some(tab) => {
                let buffer = &tab.read(cx).buffer;
                let (line, col) = buffer.char_to_line_col(buffer.selection.head);
                let lang = buffer.lang().name();
                format!("{}:{}    LF    UTF-8    {lang}", line + 1, col + 1).into()
            }
            None => "".into(),
        };
        div()
            .flex()
            .flex_none()
            .justify_between()
            .items_center()
            .px_3()
            .h(px(26.))
            .bg(theme::panel_bg())
            .border_t_1()
            .border_color(theme::border())
            .text_size(px(12.))
            .text_color(theme::muted())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .children(self.render_branch_widget(cx))
                    .child(SharedString::from(self.breadcrumb(cx).unwrap_or_else(|| self.project.name())))
                    .child(
                        div()
                            .id("status-terminal")
                            .px_1()
                            .rounded_sm()
                            .cursor_pointer()
                            .hover(|d| d.bg(theme::hover()).text_color(theme::ui_text()))
                            .child("› Terminal")
                            .on_click(cx.listener(|this, _, window, cx| this.toggle_terminal(&terminal_panel::ToggleTerminal, window, cx))),
                    ),
            )
            .child(
                div()
                    .flex()
                    .gap_3()
                    .children(self.tasks.first().map(|(_, label)| {
                        let more = if self.tasks.len() > 1 {
                            format!(" (+{})", self.tasks.len() - 1)
                        } else {
                            String::new()
                        };
                        div()
                            .text_color(theme::accent())
                            .child(format!("⟳ {label}{more}"))
                    }))
                    .when(self.tasks.is_empty(), |d| {
                        d.child(SharedString::from(self.index_status()))
                    }),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(info)
                    .children(self.project_facts(cx).into_iter().map(|fact| div().child(fact)))
                    .child(
                        div()
                            .id("reindex")
                            .px_1()
                            .rounded_sm()
                            .cursor_pointer()
                            .hover(|d| d.bg(theme::hover()).text_color(theme::ui_text()))
                            .child("⟳ Reindex")
                            .on_click(cx.listener(|this, _, _, cx| {
                                // Rebuild the code index and the cached file list.
                                this.files_cache = None;
                                this.rescan(cx);
                            })),
                    ),
            )
    }
}

/// A small glyph and color per file type, Zed-style.
fn file_icon(name: &str, is_dir: bool, expanded: bool) -> (&'static str, gpui::Hsla) {
    use php_syntax::Highlight as H;
    if is_dir {
        return (if expanded { "▿" } else { "▸" }, theme::muted());
    }
    let lower = name.to_ascii_lowercase();
    let (icon, color) = match () {
        _ if lower.ends_with(".blade.php") => ("◆", theme::syntax(H::Tag)),
        _ if lower.ends_with(".php") => ("◉", theme::syntax(H::Keyword)),
        _ if lower.ends_with(".js") || lower.ends_with(".mjs") || lower.ends_with(".cjs") => ("◼", theme::syntax(H::Type)),
        _ if lower.ends_with(".ts") || lower.ends_with(".tsx") => ("◼", theme::syntax(H::Function)),
        _ if lower.ends_with(".vue") => ("▲", theme::syntax(H::String)),
        _ if lower.ends_with(".css") || lower.ends_with(".scss") => ("#", theme::syntax(H::Function)),
        _ if lower.ends_with(".json") || lower.ends_with(".lock") => ("{}", theme::syntax(H::Number)),
        _ if lower.ends_with(".md") => ("¶", theme::muted()),
        _ if lower.ends_with(".yml") || lower.ends_with(".yaml") || lower.ends_with(".xml") => ("≡", theme::syntax(H::Attribute)),
        _ if lower.starts_with(".env") => ("⚙", theme::syntax(H::Constant)),
        _ if lower.ends_with(".log") => ("☰", theme::muted()),
        _ if lower == "artisan" || lower.ends_with(".sh") => ("›", theme::success()),
        _ => ("•", theme::muted()),
    };
    (icon, color)
}

impl Workspace {
    fn index_status(&self) -> String {
        let framework = if self.intel.is_laravel() {
            "Laravel · "
        } else {
            ""
        };
        if self.intel.is_ready() {
            format!("{framework}{} classes indexed", self.intel.class_count())
        } else {
            format!("{framework}Indexing…")
        }
    }
}

impl Workspace {
    /// Structure of the active PHP file, cached per buffer version.
    fn active_structure(&self, cx: &gpui::App) -> Option<std::rc::Rc<Vec<crate::structure::StructItem>>> {
        let buffer = &self.tabs.get(self.active)?.read(cx).buffer;
        let path = buffer.path()?.to_path_buf();
        if !path.to_string_lossy().ends_with(".php") || path.to_string_lossy().ends_with(".blade.php") {
            return None;
        }
        let version = buffer.version();
        if let Some((p, v, items)) = self.structure.borrow().as_ref()
            && *p == path
            && *v == version
        {
            return Some(items.clone());
        }
        let items = std::rc::Rc::new(crate::structure::outline(&buffer.text()));
        *self.structure.borrow_mut() = Some((path, version, items.clone()));
        Some(items)
    }

    fn render_structure(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let items = self.active_structure(cx).unwrap_or_default();
        let caret = self.tabs.get(self.active).map(|t| {
            let buffer = &t.read(cx).buffer;
            buffer.char_to_byte(buffer.selection.head)
        });
        // The innermost item at or before the caret is highlighted.
        let current = caret.and_then(|c| items.iter().rposition(|i| i.range.start <= c));
        let empty = items.is_empty();
        let rows = items.iter().enumerate().map(|(ix, item)| {
            let start = item.range.start;
            div()
                .id(("structure", ix))
                .mx_1()
                .pl(px(8. + 14. * item.depth as f32))
                .pr_2()
                .h(px(24.))
                .flex()
                .items_center()
                .gap_1p5()
                .rounded_md()
                .cursor_pointer()
                .text_color(theme::ui_text())
                .when(Some(ix) == current, |d| d.bg(theme::active_row()))
                .hover(|d| d.bg(theme::hover()))
                .child(div().w(px(16.)).flex_none().text_color(theme::syntax(php_syntax::Highlight::Keyword)).child(item.icon))
                .child(div().overflow_hidden().whitespace_nowrap().child(SharedString::from(item.name.clone())))
                .child(div().flex_1())
                .child(div().text_size(px(11.)).text_color(theme::muted()).child(SharedString::from(item.detail.clone())))
                .on_click(cx.listener(move |this, _, window, cx| {
                    if let Some(tab) = this.tabs.get(this.active).cloned() {
                        tab.update(cx, |editor, cx| editor.reveal_byte(start, cx));
                        window.focus(&tab.focus_handle(cx));
                    }
                }))
        });
        div()
            .id("structure-list")
            .flex_1()
            .overflow_y_scroll()
            .pb_2()
            .text_size(px(13.))
            .when(empty, |d| d.child(div().p_3().text_color(theme::muted()).child("Open a PHP file to see its structure.")))
            .children(rows.collect::<Vec<_>>())
    }

    fn render_modules(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let root = self.project.root().to_path_buf();
        let modules = crate::modules::list(&root);
        let empty = modules.is_empty();
        let rows = modules.into_iter().enumerate().map(|(ix, module)| {
            let (dot, label) = match module.enabled {
                Some(true) => (theme::success(), "Enabled"),
                Some(false) => (theme::error(), "Disabled"),
                None => (theme::muted(), "Not set"),
            };
            let name = module.name.clone();
            let path = module.path.clone();
            let enabled = module.enabled == Some(true);
            let root = root.clone();
            div()
                .id(("module", ix))
                .mx_1()
                .px_2()
                .py_1()
                .flex()
                .items_center()
                .gap_2()
                .rounded_md()
                .hover(|d| d.bg(theme::hover()))
                .child(div().size(px(8.)).flex_none().rounded_full().bg(dot))
                .child(
                    div()
                        .id(("module-name", ix))
                        .flex_1()
                        .min_w(px(0.))
                        .cursor_pointer()
                        .flex()
                        .flex_col()
                        .child(div().text_color(theme::ui_text()).child(SharedString::from(module.name.clone())))
                        .when(!module.description.is_empty(), |d| {
                            d.child(div().text_size(px(11.)).text_color(theme::muted()).overflow_hidden().whitespace_nowrap().child(SharedString::from(module.description.clone())))
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            // Show the module in the project tree.
                            this.project.reveal(&path.join("module.json"));
                            if !this.project.is_expanded(&path) {
                                this.project.toggle(&path);
                            }
                            this.rows = this.project.rows();
                            this.selected_path = Some(path.clone());
                            this.layout.show(layout::ToolWindow::Project);
                            cx.notify();
                        })),
                )
                .child(
                    div()
                        .id(("module-toggle", ix))
                        .px_2()
                        .h(px(20.))
                        .flex()
                        .items_center()
                        .rounded_md()
                        .cursor_pointer()
                        .text_size(px(11.))
                        .text_color(gpui::white())
                        .bg(dot)
                        .child(label)
                        .tooltip(layout::tooltip(if enabled { "Click to disable (module:disable)" } else { "Click to enable (module:enable)" }))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Err(err) = crate::modules::set_enabled(&root, &name, !enabled) {
                                this.output = Some(tools::Output { title: "Modules".into(), text: err.to_string(), running: false, ok: false });
                            }
                            cx.notify();
                        })),
                )
        });
        div()
            .id("modules-list")
            .flex_1()
            .overflow_y_scroll()
            .py_1()
            .text_size(px(13.))
            .when(empty, |d| d.child(div().p_3().text_color(theme::muted()).child("No modules in Modules/.")))
            .children(rows.collect::<Vec<_>>())
    }

    /// `Laravel 11.31.0`, `PHP 8.4.1`, `4 spaces` for the status bar.
    fn project_facts(&self, cx: &gpui::App) -> Vec<String> {
        let root = self.project.root();
        let mut facts = Vec::new();
        if self.intel.is_laravel()
            && let Some(version) = crate::project_info::laravel_version(root)
        {
            facts.push(format!("Laravel {version}"));
        }
        if let Some(version) = crate::project_info::php_version() {
            facts.push(format!("PHP {version}"));
        }
        if let Some(path) = self.tabs.get(self.active).and_then(|t| t.read(cx).buffer.path().map(|p| p.to_path_buf()))
            && let Some(indent) = crate::project_info::editorconfig_indent(root, &path)
        {
            facts.push(format!("EditorConfig: {indent}"));
        }
        facts
    }

    /// `app › Models › User.php › User › posts()` for the status bar.
    fn breadcrumb(&self, cx: &gpui::App) -> Option<String> {
        let buffer = &self.tabs.get(self.active)?.read(cx).buffer;
        let path = buffer.path()?;
        let relative = path.strip_prefix(self.project.root()).unwrap_or(path);
        let mut parts: Vec<String> = relative.iter().map(|p| p.to_string_lossy().into_owned()).collect();
        if buffer.is_php() {
            let offset = buffer.char_to_byte(buffer.selection.head);
            parts.extend(crate::structure::scope_at(&buffer.text(), offset));
        }
        Some(parts.join(" › "))
    }
}

impl Focusable for Workspace {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for Workspace {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let editor = match (self.diff.as_ref().filter(|_| self.diff_active), self.tabs.get(self.active)) {
            (Some(diff), _) => div().flex_1().overflow_hidden().child(diff.clone()),
            (None, Some(tab)) => div().flex_1().overflow_hidden().child(tab.clone()),
            (None, None) => div()
                .flex_1()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_2()
                .bg(theme::editor_bg())
                .text_color(theme::muted())
                .child("Откройте файл в дереве проекта")
                .child("⌘W закрыть вкладку · ⌃Tab следующая · ⌘S сохранить"),
        };
        div()
            .key_context("Workspace")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::close_tab))
            .on_action(cx.listener(Self::next_tab))
            .on_action(cx.listener(Self::prev_tab))
            .on_action(cx.listener(Self::navigate_back))
            .on_action(cx.listener(Self::open_make))
            .on_action(cx.listener(Self::open_artisan))
            .on_action(cx.listener(Self::open_route_search))
            .on_action(cx.listener(Self::new_model))
            .on_action(cx.listener(Self::new_pivot_table))
            .on_action(cx.listener(Self::toggle_laravel_menu))
            .on_action(cx.listener(Self::open_laravel_settings))
            .on_action(cx.listener(Self::generate_helper_code))
            .on_action(cx.listener(Self::remove_model_phpdocs))
            .on_action(cx.listener(Self::model_from_table))
            .on_action(cx.listener(Self::find_unused_views))
            .on_action(cx.listener(Self::open_keymap))
            .on_action(cx.listener(Self::open_themes))
            .on_action(cx.listener(Self::open_settings))
            .on_action(cx.listener(Self::show_project))
            .on_action(cx.listener(Self::show_local_history))
            .on_action(cx.listener(Self::sync_push))
            .on_action(cx.listener(Self::sync_pull))
            .on_action(cx.listener(Self::show_commit))
            .on_action(cx.listener(Self::toggle_git_log))
            .on_action(cx.listener(Self::open_branches))
            .on_action(cx.listener(Self::open_projects))
            .on_action(cx.listener(Self::about))
            .on_action(cx.listener(Self::new_project))
            .on_action(cx.listener(Self::open_folder))
            .on_action(cx.listener(Self::close_project))
            .on_action(cx.listener(Self::reload_all))
            .on_action(cx.listener(Self::reindex))
            .on_action(cx.listener(Self::save_all_action))
            .on_action(cx.listener(Self::toggle_terminal))
            .on_action(cx.listener(Self::toggle_agent))
            .on_action(cx.listener(Self::toggle_structure))
            .on_action(cx.listener(Self::split_right))
            .on_action(cx.listener(Self::unsplit_pane))
            .on_action(cx.listener(Self::focus_next_pane))
            .on_action(cx.listener(Self::run_sql))
            .on_action(cx.listener(Self::new_agent_chat))
            .on_action(cx.listener(Self::new_terminal))
            .on_action(cx.listener(Self::close_terminal))
            .on_action(cx.listener(Self::run_popup))
            .on_action(cx.listener(Self::run_last))
            .on_action(cx.listener(Self::run_context))
            .on_action(cx.listener(Self::stop_run))
            .on_action(cx.listener(Self::edit_run_configurations))
            .on_action(cx.listener(Self::commit_changes))
            .on_action(cx.listener(Self::commit_and_push))
            .on_action(cx.listener(Self::git_pull))
            .on_action(cx.listener(Self::git_push))
            .on_action(cx.listener(Self::toggle_prefs_menu))
            .on_action(cx.listener(Self::find_in_files))
            .on_action(cx.listener(Self::replace_in_files))
            .on_action(cx.listener(Self::search_everywhere))
            .on_action(cx.listener(Self::go_to_file))
            .on_action(cx.listener(Self::go_to_class))
            .on_action(cx.listener(Self::go_to_symbol))
            .on_action(cx.listener(Self::recent_files))
            .on_action(cx.listener(Self::go_to_line))
            .on_modifiers_changed(cx.listener(Self::on_modifiers))
            .capture_key_down(cx.listener(|this, _: &gpui::KeyDownEvent, _, _| this.on_any_key()))
            .on_action(cx.listener(Self::close_output))
            .on_drag_move(cx.listener(Self::on_resize))
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .font_family(theme::ui_font())
            .bg(theme::editor_bg())
            .child(self.render_toolbar(cx))
            .child(
                div()
                    .flex_1()
                    .flex()
                    .overflow_hidden()
                    .child(self.render_stripe(layout::Side::Left, cx))
                    .children(self.render_side(layout::Side::Left, cx))
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .overflow_hidden()
                            .child({
                                // Split editor: the focused pane plus the others, left to right.
                                let focused = self.focused_pane;
                                let focused_pane = div()
                                    .flex_1()
                                    .min_w(px(0.))
                                    .flex()
                                    .flex_col()
                                    .when(focused > 0, |d| d.border_l_1().border_color(theme::border_strong()))
                                    .child(self.render_tabs(cx))
                                    .child(editor)
                                    .into_any_element();
                                let mut row: Vec<gpui::AnyElement> = (0..self.pane_count()).filter(|i| *i != focused).map(|i| (i, self.render_other_pane(i, cx))).collect::<Vec<_>>().into_iter().map(|(_, e)| e).collect();
                                row.insert(focused, focused_pane);
                                div().flex_1().min_h(px(0.)).flex().children(row)
                            })
                            .children(self.render_output(cx))
                            .children(self.render_side(layout::Side::Bottom, cx)),
                    )
                    .children(self.render_side(layout::Side::Right, cx))
                    .child(self.render_stripe(layout::Side::Right, cx)),
            )
            .child(self.render_status(cx))
            .children(self.render_context_menu(cx))
            .children(self.render_modal(cx))
    }
}
