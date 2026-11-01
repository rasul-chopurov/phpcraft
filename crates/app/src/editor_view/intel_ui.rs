//! Editor features backed by code intelligence: navigation, completion popup, quick-fixes.

use gpui::{
    AnyElement, Context, MouseButton, MouseDownEvent, Pixels, Point, SharedString, Window, actions,
    anchored, deferred, div, prelude::*, px,
};
use php_analysis::{CompletionItem, CompletionKind};
use php_index::{Location, short_name};

use super::EditorView;
use crate::theme;

actions!(
    editor,
    [
        GoToDeclaration,
        ShowCompletions,
        ShowIntentions,
        OptimizeImports,
        MenuUp,
        MenuDown,
        MenuConfirm,
        MenuCancel,
    ]
);

const MENU_ROWS: usize = 12;

pub enum MenuAction {
    Complete(CompletionItem),
    Navigate(Location),
    Import(String),
    OptimizeImports,
    Nothing,
}

pub struct MenuItem {
    label: SharedString,
    detail: SharedString,
    icon: &'static str,
    action: MenuAction,
}

pub struct Menu {
    items: Vec<MenuItem>,
    selected: usize,
    /// Completion menus refresh as you type; others close on edit.
    is_completion: bool,
}

fn completion_icon(kind: CompletionKind) -> &'static str {
    match kind {
        CompletionKind::Class => "C",
        CompletionKind::Method => "m",
        CompletionKind::Property => "f",
        CompletionKind::Constant => "k",
        CompletionKind::Function => "ƒ",
        CompletionKind::Variable => "$",
        CompletionKind::Keyword => "◆",
        CompletionKind::Value => "\"",
    }
}

impl EditorView {
    fn path(&self) -> Option<std::path::PathBuf> {
        self.buffer.path().map(|p| p.to_path_buf())
    }

    fn caret_byte(&self) -> usize {
        self.buffer.char_to_byte(self.buffer.selection.head)
    }

    // --- navigation ----------------------------------------------------

    pub(super) fn goto_offset(
        &mut self,
        offset: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(path) = self.path() else { return };
        let targets = self.intel.goto(&path, &self.buffer.text(), offset);
        match targets.len() {
            0 => {}
            1 => self.navigate(targets.into_iter().next().unwrap(), window, cx),
            _ => {
                let root = self.intel_root();
                let items = targets
                    .into_iter()
                    .map(|loc| MenuItem {
                        label: relative(&root, &loc.path).into(),
                        detail: "".into(),
                        icon: "→",
                        action: MenuAction::Navigate(loc),
                    })
                    .collect();
                self.show_menu(items, false, cx);
            }
        }
    }

    fn intel_root(&self) -> std::path::PathBuf {
        self.intel.root().to_path_buf()
    }

    fn navigate(&mut self, location: Location, window: &mut Window, cx: &mut Context<Self>) {
        let workspace = self.workspace.clone();
        let from = self.path().map(|p| (p, self.buffer.selection.head));
        // Deferred: the workspace may need to update this editor while we're borrowed.
        window.defer(cx, move |window, cx| {
            workspace
                .update(cx, |ws, cx| ws.open_location(&location, from, window, cx))
                .ok();
        });
    }

    pub(super) fn go_to_declaration(
        &mut self,
        _: &GoToDeclaration,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.goto_offset(self.caret_byte(), window, cx);
    }

    /// ⌘+click: navigate instead of moving the caret.
    pub(super) fn cmd_click(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !event.modifiers.platform {
            return false;
        }
        if let Some(ix) = self.char_for_position(event.position) {
            self.hover_link = None;
            self.goto_offset(self.buffer.char_to_byte(ix), window, cx);
        }
        true
    }

    pub(super) fn update_hover_link(
        &mut self,
        position: Point<Pixels>,
        cmd: bool,
        cx: &mut Context<Self>,
    ) {
        let link = if cmd {
            let current = self.hover_link.clone();
            let offset = self
                .char_for_position(position)
                .map(|ix| self.buffer.char_to_byte(ix));
            match (offset, current) {
                (Some(offset), Some(range)) if range.contains(&offset) => Some(range),
                (Some(offset), _) => self
                    .path()
                    .and_then(|path| self.intel.link_at(&path, &self.buffer.text(), offset)),
                _ => None,
            }
        } else {
            None
        };
        if link != self.hover_link {
            self.hover_link = link;
            cx.notify();
        }
    }

    pub(super) fn clear_hover_link(&mut self, cx: &mut Context<Self>) {
        if self.hover_link.take().is_some() {
            cx.notify();
        }
    }

    // --- completion ----------------------------------------------------

    pub(super) fn show_completions(
        &mut self,
        _: &ShowCompletions,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.refresh_completion(true, cx);
    }

    /// Recomputes completion items at the caret. `explicit` shows the menu even for an empty prefix.
    pub(super) fn refresh_completion(&mut self, explicit: bool, cx: &mut Context<Self>) {
        let Some(path) = self.path() else { return };
        if !self.buffer.is_php() {
            return;
        }
        let items = self
            .intel
            .complete(&path, &self.buffer.text(), self.caret_byte());
        if items.is_empty() && !explicit {
            self.menu = None;
            cx.notify();
            return;
        }
        let items = items
            .into_iter()
            .map(|item| MenuItem {
                label: item.label.clone().into(),
                detail: item.detail.clone().into(),
                icon: completion_icon(item.kind),
                action: MenuAction::Complete(item),
            })
            .collect::<Vec<_>>();
        if items.is_empty() {
            self.menu = None;
        } else {
            self.show_menu(items, true, cx);
        }
        cx.notify();
    }

    /// Opens completion automatically after typing identifier characters, `->`, `::` or `$`.
    pub(super) fn after_typing(&mut self, typed: &str, cx: &mut Context<Self>) {
        let before = self
            .buffer
            .slice(self.buffer.selection.head.saturating_sub(2)..self.buffer.selection.head);
        let trigger = typed.chars().count() == 1
            && (typed
                .chars()
                .all(|c| c.is_alphanumeric() || matches!(c, '_' | '$' | '\\' | '@' | '|'))
                || before == "->"
                || before == "::"
                || (self.menu.is_some() && typed != " "));
        if trigger {
            self.refresh_completion(false, cx);
        } else {
            self.menu = None;
        }
    }

    fn apply_completion(&mut self, item: CompletionItem, cx: &mut Context<Self>) {
        let mut caret = item.replace.end;
        let mut edits = vec![(item.replace.clone(), item.insert.clone())];
        // Put the caret inside `()` when the callable takes arguments.
        let takes_args = item.insert.ends_with("()") && !item.detail.starts_with("()");
        if let Some(edit) = item.additional_edit {
            edits.push((edit.range, edit.text));
        }
        self.buffer.apply_edits(edits, caret);
        if takes_args {
            caret = self.buffer.selection.head - 1;
            self.buffer.move_to(caret, false);
        }
        self.changed(cx);
    }

    // --- quick fixes ---------------------------------------------------

    pub(super) fn show_intentions(
        &mut self,
        _: &ShowIntentions,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let offset = self.caret_byte();
        let mut items = Vec::new();
        let unresolved = self.current_diagnostics().to_vec();
        for problem in unresolved
            .iter()
            .filter(|u| u.range.start <= offset && offset <= u.range.end)
        {
            for fqn in &problem.candidates {
                items.push(MenuItem {
                    label: format!("Import class '{}'", short_name(fqn)).into(),
                    detail: fqn.clone().into(),
                    icon: "💡",
                    action: MenuAction::Import(fqn.clone()),
                });
            }
            if problem.candidates.is_empty() {
                items.push(MenuItem {
                    label: format!("Undefined class '{}'", problem.name).into(),
                    detail: "no candidates in index".into(),
                    icon: "⚠",
                    action: MenuAction::Nothing,
                });
            }
        }
        if self.buffer.is_php() {
            items.push(MenuItem {
                label: "Optimize imports".into(),
                detail: "⌃⌥O".into(),
                icon: "✦",
                action: MenuAction::OptimizeImports,
            });
        }
        self.show_menu(items, false, cx);
    }

    pub(super) fn optimize_imports(
        &mut self,
        _: &OptimizeImports,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.run_optimize_imports(cx);
    }

    fn run_optimize_imports(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.path() else { return };
        if let Some(edit) = self.intel.optimize_imports(&path, &self.buffer.text()) {
            let caret = self.caret_byte();
            self.buffer
                .apply_edits(vec![(edit.range, edit.text)], caret);
            self.changed(cx);
        }
    }

    fn import_class(&mut self, fqn: &str, cx: &mut Context<Self>) {
        let Some(path) = self.path() else { return };
        if let Some(edit) = self.intel.import(&path, &self.buffer.text(), fqn) {
            let caret = self.caret_byte();
            self.buffer
                .apply_edits(vec![(edit.range, edit.text)], caret);
            self.changed(cx);
        }
    }

    /// Unresolved class references, recomputed when the buffer changes.
    pub(super) fn current_diagnostics(&mut self) -> &[php_analysis::Unresolved] {
        let version = self.buffer.version();
        let stale = self.diagnostics.as_ref().is_none_or(|(v, _)| *v != version);
        if stale {
            let found = match self.path() {
                Some(path) if self.buffer.is_php() => {
                    self.intel.unresolved(&path, &self.buffer.text())
                }
                _ => Vec::new(),
            };
            // Keep stale results until the index is ready, so nothing flickers.
            self.diagnostics = Some((
                if self.intel.is_ready() {
                    version
                } else {
                    u64::MAX
                },
                found,
            ));
        }
        &self.diagnostics.as_ref().unwrap().1
    }

    // --- menu ----------------------------------------------------------

    fn show_menu(&mut self, items: Vec<MenuItem>, is_completion: bool, cx: &mut Context<Self>) {
        self.menu = (!items.is_empty()).then_some(Menu {
            items,
            selected: 0,
            is_completion,
        });
        cx.notify();
    }

    pub(super) fn menu_open(&self) -> bool {
        self.menu.is_some()
    }

    pub(super) fn menu_is_completion(&self) -> bool {
        self.menu.as_ref().is_some_and(|m| m.is_completion)
    }

    pub(super) fn close_menu(&mut self) {
        self.menu = None;
    }

    pub(super) fn menu_up(&mut self, _: &MenuUp, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(menu) = &mut self.menu {
            menu.selected = menu.selected.checked_sub(1).unwrap_or(menu.items.len() - 1);
            cx.notify();
        }
    }

    pub(super) fn menu_down(&mut self, _: &MenuDown, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(menu) = &mut self.menu {
            menu.selected = (menu.selected + 1) % menu.items.len();
            cx.notify();
        }
    }

    pub(super) fn menu_cancel(&mut self, _: &MenuCancel, _: &mut Window, cx: &mut Context<Self>) {
        self.menu = None;
        cx.notify();
    }

    pub(super) fn menu_confirm(
        &mut self,
        _: &MenuConfirm,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.confirm_item(None, window, cx);
    }

    fn confirm_item(&mut self, ix: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(menu) = self.menu.take() else { return };
        let ix = ix.unwrap_or(menu.selected);
        let Some(item) = menu.items.into_iter().nth(ix) else {
            return;
        };
        match item.action {
            MenuAction::Complete(completion) => self.apply_completion(completion, cx),
            MenuAction::Navigate(location) => self.navigate(location, window, cx),
            MenuAction::Import(fqn) => self.import_class(&fqn, cx),
            MenuAction::OptimizeImports => self.run_optimize_imports(cx),
            MenuAction::Nothing => {}
        }
        cx.notify();
    }

    pub(super) fn render_menu(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let menu = self.menu.as_ref()?;
        let position = self.caret_point?;
        let first = menu.selected.saturating_sub(MENU_ROWS - 1);
        let rows = menu
            .items
            .iter()
            .enumerate()
            .skip(first)
            .take(MENU_ROWS)
            .map(|(ix, item)| {
                let selected = ix == menu.selected;
                div()
                    .id(ix)
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .h(px(22.))
                    .when(selected, |d| d.bg(theme::selection()))
                    .hover(|d| d.bg(theme::hover()))
                    .child(
                        div()
                            .w(px(16.))
                            .text_color(theme::syntax(php_syntax::Highlight::Keyword))
                            .child(item.icon),
                    )
                    .child(div().text_color(theme::text()).child(item.label.clone()))
                    .child(
                        div()
                            .flex_1()
                            .ml_4()
                            .text_right()
                            .overflow_hidden()
                            .text_color(theme::muted())
                            .child(item.detail.clone()),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.confirm_item(Some(ix), window, cx);
                        }),
                    )
            });
        Some(
            deferred(
                anchored().position(position).child(
                    div()
                        .id("editor-menu")
                        .occlude()
                        .min_w(px(320.))
                        .max_w(px(640.))
                        .py_1()
                        .bg(theme::panel_bg())
                        .border_1()
                        .border_color(theme::border())
                        .rounded_md()
                        .shadow_lg()
                        .font_family(theme::editor_font())
                        .text_size(px(13.))
                        .children(rows),
                ),
            )
            .with_priority(1)
            .into_any_element(),
        )
    }
}

fn relative(root: &std::path::Path, path: &std::path::Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string()
}
