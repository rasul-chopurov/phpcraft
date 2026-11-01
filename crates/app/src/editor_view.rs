//! Code editor view: renders a [`Buffer`] with gutter, highlighting, caret and selection.

mod find_bar;
mod intel_ui;

use std::ops::Range;
use std::sync::Arc;

use editor::{Buffer, Selection};
use gpui::{
    App, Bounds, ClipboardItem, Context, CursorStyle, ElementId, ElementInputHandler, Entity,
    EntityInputHandler, FocusHandle, Focusable, GlobalElementId, Hsla, KeyContext, LayoutId,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, Point,
    ScrollWheelEvent, ShapedLine, SharedString, Style, TextRun, UTF16Selection, Window, actions,
    fill, font, point, prelude::*, px, relative, size,
};

use crate::intel::Intel;
use crate::theme;
use crate::workspace::Workspace;
pub use find_bar::{ShowFind, ShowReplace};
use intel_ui::Menu;
pub use intel_ui::OptimizeImports;

actions!(
    editor,
    [
        ToggleMarkdownPreview,
        MoveLeft,
        MoveRight,
        MoveUp,
        MoveDown,
        SelectLeft,
        SelectRight,
        SelectUp,
        SelectDown,
        MoveHome,
        MoveEnd,
        SelectHome,
        SelectEnd,
        PageUp,
        PageDown,
        DocStart,
        DocEnd,
        Backspace,
        Delete,
        Newline,
        Tab,
        SelectAll,
        Copy,
        Cut,
        Paste,
        Undo,
        Redo,
        Save,
        ToggleComment,
        DuplicateLine,
        DeleteLine,
        MoveLineUp,
        MoveLineDown,
        ToggleBlame,
    ]
);

const GUTTER_PADDING: f32 = 12.;
const TEXT_PADDING: f32 = 8.;
/// Width of the git blame column, in digits.
const BLAME_CHARS: usize = 25;

/// Geometry of the last paint, used for mouse hit-testing and IME.
struct LayoutInfo {
    bounds: Bounds<Pixels>,
    text_x: Pixels,
    first_line: usize,
    lines: Vec<(String, ShapedLine)>,
}

pub struct EditorView {
    pub buffer: Buffer,
    focus_handle: FocusHandle,
    scroll_y: Pixels,
    autoscroll: bool,
    /// Scroll so the caret sits near the middle (after navigation).
    center_on_caret: bool,
    is_selecting: bool,
    marked_range: Option<Range<usize>>,
    layout: Option<LayoutInfo>,
    intel: Arc<Intel>,
    workspace: gpui::WeakEntity<Workspace>,
    menu: Option<Menu>,
    find: Option<find_bar::FindBar>,
    autosave: Option<gpui::Task<()>>,
    /// Compact mode without gutter (commit message box).
    plain: bool,
    /// Markdown files: editor, split or preview, plus the parsed preview for a buffer version.
    md_mode: MdMode,
    md_cache: Option<(u64, std::rc::Rc<Vec<crate::markdown_preview::Block>>)>,
    /// File contents at HEAD for change markers (`None`: not in a repository / untracked).
    git_base: Option<String>,
    git_markers: Option<(u64, Vec<(usize, git::LineChange)>)>,
    /// Git blame annotations shown in the gutter.
    blame: Option<Vec<git::BlameLine>>,
    /// Buffer version the auto-save timer was last scheduled for.
    autosaved_version: u64,
    /// Byte range underlined while ⌘ is held over a navigable symbol.
    hover_link: Option<Range<usize>>,
    /// Unresolved class references for the buffer version they were computed for.
    diagnostics: Option<(u64, Vec<php_analysis::Unresolved>)>,
    /// Window position just below the caret, for anchoring popups.
    caret_point: Option<Point<Pixels>>,
}

impl EditorView {
    pub fn new(
        buffer: Buffer,
        intel: Arc<Intel>,
        workspace: gpui::WeakEntity<Workspace>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            intel,
            workspace,
            menu: None,
            find: None,
            autosave: None,
            git_base: None,
            git_markers: None,
            blame: None,
            autosaved_version: 0,
            hover_link: None,
            diagnostics: None,
            caret_point: None,
            buffer,
            focus_handle: cx.focus_handle(),
            scroll_y: px(0.),
            autoscroll: false,
            center_on_caret: false,
            is_selecting: false,
            marked_range: None,
            layout: None,
            plain: false,
            md_mode: MdMode::Split,
            md_cache: None,
        }
    }

    /// A compact text area (commit message): no gutter, no code intelligence.
    pub fn plain(intel: Arc<Intel>, workspace: gpui::WeakEntity<Workspace>, cx: &mut Context<Self>) -> Self {
        let mut view = Self::new(Buffer::new(""), intel, workspace, cx);
        view.plain = true;
        view
    }

    fn line_height() -> Pixels {
        px(theme::editor_line_height())
    }

    fn changed(&mut self, cx: &mut Context<Self>) {
        self.autoscroll = true;
        if self.buffer.version() != self.autosaved_version {
            self.autosaved_version = self.buffer.version();
            self.schedule_autosave(cx);
        }
        cx.notify();
    }

    /// Moves the caret to a byte offset and scrolls it into view.
    pub fn reveal_byte(&mut self, byte: usize, cx: &mut Context<Self>) {
        let ix = self.buffer.byte_to_char(byte);
        self.reveal_char(ix, cx);
    }

    pub fn reveal_char(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.buffer.move_to(ix, false);
        self.close_menu();
        self.center_on_caret = true;
        self.changed(cx);
    }

    /// Replaces the whole text (Local History revert), as one undoable edit.
    pub fn replace_all_text(&mut self, text: &str, cx: &mut Context<Self>) {
        let end = self.buffer.text().len();
        let caret = self.buffer.char_to_byte(self.buffer.selection.head).min(text.len());
        self.buffer.apply_edits(vec![(0..end, text.to_string())], 0);
        let caret = self.buffer.byte_to_char(caret);
        self.buffer.move_to(caret, false);
        self.changed(cx);
    }

    /// Reloads the buffer from disk after an external change, unless it has unsaved edits.
    pub fn reload_if_clean(&mut self, path: &std::path::Path, cx: &mut Context<Self>) {
        if self.buffer.is_dirty() {
            return;
        }
        if let Ok(buffer) = Buffer::open(path) {
            let caret = self.buffer.selection.head.min(buffer.len_chars());
            self.buffer = buffer;
            self.buffer.move_to(caret, false);
            cx.notify();
        }
    }

    /// Caret movement closes popups.
    fn moved(&mut self, cx: &mut Context<Self>) {
        self.close_menu();
        self.changed(cx);
    }

    fn visible_lines(&self) -> isize {
        self.layout
            .as_ref()
            .map(|l| (l.bounds.size.height / Self::line_height()) as isize)
            .unwrap_or(30)
            .max(1)
    }

    fn move_left(&mut self, _: &MoveLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.move_left(false);
        self.moved(cx);
    }
    fn move_right(&mut self, _: &MoveRight, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.move_right(false);
        self.moved(cx);
    }
    fn move_up(&mut self, _: &MoveUp, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.move_vertical(-1, false);
        self.moved(cx);
    }
    fn move_down(&mut self, _: &MoveDown, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.move_vertical(1, false);
        self.moved(cx);
    }
    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.move_left(true);
        self.moved(cx);
    }
    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.move_right(true);
        self.moved(cx);
    }
    fn select_up(&mut self, _: &SelectUp, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.move_vertical(-1, true);
        self.moved(cx);
    }
    fn select_down(&mut self, _: &SelectDown, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.move_vertical(1, true);
        self.moved(cx);
    }
    fn move_home(&mut self, _: &MoveHome, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.move_home(false);
        self.moved(cx);
    }
    fn move_end(&mut self, _: &MoveEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.move_end(false);
        self.moved(cx);
    }
    fn select_home(&mut self, _: &SelectHome, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.move_home(true);
        self.moved(cx);
    }
    fn select_end(&mut self, _: &SelectEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.move_end(true);
        self.moved(cx);
    }
    fn page_up(&mut self, _: &PageUp, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.move_vertical(-self.visible_lines(), false);
        self.moved(cx);
    }
    fn page_down(&mut self, _: &PageDown, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.move_vertical(self.visible_lines(), false);
        self.moved(cx);
    }
    fn doc_start(&mut self, _: &DocStart, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.move_to(0, false);
        self.moved(cx);
    }
    fn doc_end(&mut self, _: &DocEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.move_to(self.buffer.len_chars(), false);
        self.moved(cx);
    }
    fn backspace(&mut self, _: &Backspace, _: &mut Window, cx: &mut Context<Self>) {
        let completing = self.menu_is_completion();
        self.buffer.backspace();
        self.changed(cx);
        if completing {
            self.refresh_completion(false, cx);
        }
    }
    fn delete(&mut self, _: &Delete, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.delete();
        self.moved(cx);
    }
    fn newline(&mut self, _: &Newline, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.newline();
        self.moved(cx);
    }
    fn tab(&mut self, _: &Tab, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.tab();
        self.moved(cx);
    }
    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.select_all();
        cx.notify();
    }
    fn undo(&mut self, _: &Undo, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.undo();
        self.moved(cx);
    }
    fn redo(&mut self, _: &Redo, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.redo();
        self.moved(cx);
    }
    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.buffer.selection.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(self.buffer.selected_text()));
        }
    }
    fn cut(&mut self, _: &Cut, _: &mut Window, cx: &mut Context<Self>) {
        if !self.buffer.selection.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(self.buffer.selected_text()));
            self.buffer.insert("");
            self.changed(cx);
        }
    }
    fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.buffer.insert(&text);
            self.changed(cx);
        }
    }
    fn save(&mut self, _: &Save, _: &mut Window, cx: &mut Context<Self>) {
        self.save_now(cx);
    }

    /// Writes the buffer if it has unsaved changes and reindexes it.
    pub fn save_now(&mut self, cx: &mut Context<Self>) {
        self.autosave = None;
        if !self.buffer.is_dirty() || self.buffer.path().is_none() {
            return;
        }
        if let Err(err) = self.buffer.save() {
            eprintln!("save failed: {err:#}");
            return;
        }
        if let Some(path) = self.buffer.path() {
            let text = self.buffer.text();
            crate::local_history::record(path, &text, "Save");
            self.intel.update_file(path, &text);
        }
        cx.notify();
    }

    /// Restarts the auto-save timer after an edit (dropping the old task cancels it).
    fn schedule_autosave(&mut self, cx: &mut Context<Self>) {
        let settings = crate::config::Settings::load_cached();
        if settings.auto_save != crate::config::AutoSave::AfterDelay || !self.buffer.is_dirty() {
            return;
        }
        let delay = std::time::Duration::from_millis(settings.auto_save_delay_ms.max(200));
        self.autosave = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;
            this.update(cx, |editor, cx| editor.save_now(cx)).ok();
        }));
    }

    // --- mouse ---------------------------------------------------------

    fn char_for_position(&self, position: Point<Pixels>) -> Option<usize> {
        let layout = self.layout.as_ref()?;
        let y = position.y - layout.bounds.top() + self.scroll_y;
        let line = (y / Self::line_height()).floor().max(0.) as usize;
        if line >= self.buffer.len_lines() {
            return Some(self.buffer.len_chars());
        }
        let col = match layout.lines.get(line.wrapping_sub(layout.first_line)) {
            Some((text, shaped)) => {
                let byte = shaped.closest_index_for_x(position.x - layout.text_x);
                text[..byte.min(text.len())].chars().count()
            }
            None => 0,
        };
        Some(self.buffer.line_col_to_char(line, col))
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus_handle);
        self.close_menu();
        if self.cmd_click(event, window, cx) {
            return;
        }
        if let Some(ix) = self.char_for_position(event.position) {
            match event.click_count {
                // Triple click: the whole line.
                n if n >= 3 => {
                    let line = self.buffer.line_range_at(ix);
                    self.buffer.selection = Selection { anchor: line.start, head: line.end };
                    self.is_selecting = false;
                }
                // Double click: the word under the cursor.
                2 => {
                    let word = self.buffer.word_range_at(ix);
                    self.buffer.selection = Selection { anchor: word.start, head: word.end };
                    self.is_selecting = false;
                }
                _ => {
                    self.buffer.move_to(ix, event.modifiers.shift);
                    self.is_selecting = true;
                }
            }
            cx.stop_propagation();
            cx.notify();
        }
    }

    /// Reloads the HEAD version used for gutter change markers.
    pub fn refresh_git_base(&mut self, cx: &mut Context<Self>) {
        self.git_base = self.buffer.path().and_then(|p| self.intel.git_base(p));
        self.git_markers = None;
        if self.blame.is_some() {
            self.blame = self.buffer.path().and_then(|p| self.intel.git_blame(p));
        }
        cx.notify();
    }

    fn toggle_blame(&mut self, _: &ToggleBlame, _: &mut Window, cx: &mut Context<Self>) {
        self.blame = match self.blame {
            Some(_) => None,
            None => self.buffer.path().and_then(|p| self.intel.git_blame(p)),
        };
        cx.notify();
    }

    fn git_markers(&mut self) -> Vec<(usize, git::LineChange)> {
        let version = self.buffer.version();
        let Some(base) = &self.git_base else { return Vec::new() };
        if self.git_markers.as_ref().is_none_or(|(v, _)| *v != version) {
            self.git_markers = Some((version, git::line_changes(base, &self.buffer.text())));
        }
        self.git_markers.as_ref().map(|(_, m)| m.clone()).unwrap_or_default()
    }

    fn toggle_comment(&mut self, _: &ToggleComment, _: &mut Window, cx: &mut Context<Self>) {
        let (prefix, suffix) = editor::comment_tokens(self.buffer.lang());
        self.buffer.toggle_comment(prefix, suffix);
        self.moved(cx);
    }

    fn duplicate_line(&mut self, _: &DuplicateLine, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.duplicate();
        self.moved(cx);
    }

    fn delete_line(&mut self, _: &DeleteLine, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.delete_lines();
        self.moved(cx);
    }

    fn move_line_up(&mut self, _: &MoveLineUp, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.move_lines(-1);
        self.moved(cx);
    }

    fn move_line_down(&mut self, _: &MoveLineDown, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.move_lines(1);
        self.moved(cx);
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.is_selecting = false;
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.update_hover_link(event.position, event.modifiers.platform, cx);
        if self.is_selecting
            && let Some(ix) = self.char_for_position(event.position)
        {
            self.buffer.move_to(ix, true);
            self.changed(cx);
        }
    }

    fn on_scroll(&mut self, event: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        let delta = event.delta.pixel_delta(Self::line_height()).y;
        let max = Self::line_height() * self.buffer.len_lines().saturating_sub(1) as f32;
        self.scroll_y = (self.scroll_y - delta).clamp(px(0.), max);
        cx.notify();
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.buffer.char_to_utf16(range.start)..self.buffer.char_to_utf16(range.end)
    }

    fn range_from_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.buffer.utf16_to_char(range.start)..self.buffer.utf16_to_char(range.end)
    }
}

impl EntityInputHandler for EditorView {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range_utf16);
        actual_range.replace(self.range_to_utf16(&range));
        Some(self.buffer.slice(range))
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let selection = self.buffer.selection;
        Some(UTF16Selection {
            range: self.range_to_utf16(&selection.range()),
            reversed: selection.head < selection.anchor,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked_range.as_ref().map(|r| self.range_to_utf16(r))
    }

    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {
        self.marked_range = None;
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .map(|r| self.range_from_utf16(&r))
            .or(self.marked_range.take())
            .unwrap_or(self.buffer.selection.range());
        // Like PhpStorm's lookup char filter: punctuation accepts the selected completion
        // (with its auto-import) before being typed, so `TextInput` + `::` imports the class.
        if range.is_empty() && self.commit_completion_with(new_text, cx) {
            if new_text == "(" && self.buffer.slice(self.buffer.selection.head.saturating_sub(1)..self.buffer.selection.head + 1) == "()" {
                // The completion already inserted `()`.
                self.changed(cx);
                return;
            }
            let at = self.buffer.selection.range();
            self.buffer.replace(at, new_text);
            self.changed(cx);
            self.after_typing(new_text, cx);
            return;
        }
        self.buffer.replace(range, new_text);
        self.changed(cx);
        self.after_typing(new_text, cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _new_selected_range_utf16: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .map(|r| self.range_from_utf16(&r))
            .or(self.marked_range.clone())
            .unwrap_or(self.buffer.selection.range());
        self.buffer.replace(range.clone(), new_text);
        self.marked_range =
            (!new_text.is_empty()).then(|| range.start..range.start + new_text.chars().count());
        self.changed(cx);
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        _bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let layout = self.layout.as_ref()?;
        let start = self.buffer.utf16_to_char(range_utf16.start);
        let (line, col) = self.buffer.char_to_line_col(start);
        let (text, shaped) = layout.lines.get(line.checked_sub(layout.first_line)?)?;
        let byte = byte_of_col(text, col);
        let top = layout.bounds.top() + Self::line_height() * line as f32 - self.scroll_y;
        Some(Bounds::new(
            point(layout.text_x + shaped.x_for_index(byte), top),
            size(px(2.), Self::line_height()),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        let ix = self.char_for_position(point)?;
        Some(self.buffer.char_to_utf16(ix))
    }
}

fn byte_of_col(text: &str, col: usize) -> usize {
    text.char_indices()
        .nth(col)
        .map(|(b, _)| b)
        .unwrap_or(text.len())
}

impl Focusable for EditorView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum MdMode {
    Editor,
    Split,
    Preview,
}

impl EditorView {
    fn is_markdown(&self) -> bool {
        !self.plain && self.buffer.lang() == php_syntax::Lang::Markdown
    }

    fn toggle_markdown_preview(&mut self, _: &ToggleMarkdownPreview, _: &mut Window, cx: &mut Context<Self>) {
        self.md_mode = match self.md_mode {
            MdMode::Editor => MdMode::Split,
            MdMode::Split => MdMode::Preview,
            MdMode::Preview => MdMode::Editor,
        };
        cx.notify();
    }

    fn markdown_blocks(&mut self) -> std::rc::Rc<Vec<crate::markdown_preview::Block>> {
        let version = self.buffer.version();
        if let Some((v, blocks)) = &self.md_cache
            && *v == version
        {
            return blocks.clone();
        }
        let blocks = std::rc::Rc::new(crate::markdown_preview::parse(&self.buffer.text()));
        self.md_cache = Some((version, blocks.clone()));
        blocks
    }

    fn render_markdown_switch(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let mode = self.md_mode;
        let button = |id: &'static str, label: &'static str, target: MdMode| {
            gpui::div()
                .id(id)
                .px_2()
                .h(px(20.))
                .flex()
                .items_center()
                .rounded_sm()
                .cursor_pointer()
                .when(mode == target, |d| d.bg(theme::selection()))
                .hover(|d| d.bg(theme::hover()))
                .child(label)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.md_mode = target;
                    cx.notify();
                }))
        };
        gpui::div()
            .flex()
            .flex_none()
            .justify_end()
            .gap_1()
            .px_2()
            .h(px(26.))
            .items_center()
            .border_b_1()
            .border_color(theme::border())
            .bg(theme::panel_bg())
            .text_size(px(12.))
            .text_color(theme::ui_text())
            .child(button("md-editor", "Editor", MdMode::Editor))
            .child(button("md-split", "Editor and Preview", MdMode::Split))
            .child(button("md-preview", "Preview", MdMode::Preview))
    }
}

impl Render for EditorView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut key_context = KeyContext::new_with_defaults();
        key_context.add("Editor");
        if self.menu_open() {
            key_context.add("menu");
        }
        let cursor = if self.hover_link.is_some() {
            CursorStyle::PointingHand
        } else {
            CursorStyle::IBeam
        };
        let find_bar = self.render_find_bar(cx);
        let editor = gpui::div()
            .key_context(key_context)
            .track_focus(&self.focus_handle)
            .flex_1()
            .w_full()
            .min_h(px(0.))
            .overflow_hidden()
            .cursor(cursor)
            .on_action(cx.listener(Self::go_to_declaration))
            .on_action(cx.listener(Self::show_completions))
            .on_action(cx.listener(Self::show_intentions))
            .on_action(cx.listener(Self::optimize_imports))
            .on_action(cx.listener(Self::menu_up))
            .on_action(cx.listener(Self::menu_down))
            .on_action(cx.listener(Self::menu_confirm))
            .on_action(cx.listener(Self::menu_cancel))
            .on_modifiers_changed(cx.listener(
                |this, event: &gpui::ModifiersChangedEvent, _, cx| {
                    if !event.modifiers.platform {
                        this.clear_hover_link(cx);
                    }
                },
            ))
            .bg(theme::editor_bg())
            .on_action(cx.listener(Self::move_left))
            .on_action(cx.listener(Self::move_right))
            .on_action(cx.listener(Self::move_up))
            .on_action(cx.listener(Self::move_down))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_up))
            .on_action(cx.listener(Self::select_down))
            .on_action(cx.listener(Self::move_home))
            .on_action(cx.listener(Self::move_end))
            .on_action(cx.listener(Self::select_home))
            .on_action(cx.listener(Self::select_end))
            .on_action(cx.listener(Self::page_up))
            .on_action(cx.listener(Self::page_down))
            .on_action(cx.listener(Self::doc_start))
            .on_action(cx.listener(Self::doc_end))
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::newline))
            .on_action(cx.listener(Self::tab))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::undo))
            .on_action(cx.listener(Self::redo))
            .on_action(cx.listener(Self::save))
            .on_action(cx.listener(Self::toggle_comment))
            .on_action(cx.listener(Self::duplicate_line))
            .on_action(cx.listener(Self::delete_line))
            .on_action(cx.listener(Self::move_line_up))
            .on_action(cx.listener(Self::move_line_down))
            .on_action(cx.listener(Self::toggle_blame))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_scroll_wheel(cx.listener(Self::on_scroll))
            .child(EditorElement { view: cx.entity() })
            .children(self.render_menu(cx));
        gpui::div()
            .key_context("EditorPane")
            .size_full()
            .flex()
            .flex_col()
            .on_action(cx.listener(Self::show_find))
            .on_action(cx.listener(Self::show_replace))
            .on_action(cx.listener(Self::find_next))
            .on_action(cx.listener(Self::find_prev))
            .on_action(cx.listener(Self::toggle_markdown_preview))
            .children(find_bar)
            .map(|pane| {
                if !self.is_markdown() {
                    return pane.child(editor);
                }
                let blocks = self.markdown_blocks();
                let mode = self.md_mode;
                pane.child(self.render_markdown_switch(cx)).child(
                    gpui::div()
                        .flex_1()
                        .min_h(px(0.))
                        .flex()
                        .when(mode != MdMode::Preview, |d| {
                            d.child(gpui::div().flex_1().min_w(px(0.)).h_full().flex().flex_col().child(editor))
                        })
                        .when(mode == MdMode::Split, |d| d.child(gpui::div().w(px(1.)).h_full().bg(theme::border())))
                        .when(mode != MdMode::Editor, |d| {
                            d.child(gpui::div().flex_1().min_w(px(0.)).h_full().child(crate::markdown_preview::render(&blocks)))
                        }),
                )
            })
    }
}

struct EditorElement {
    view: Entity<EditorView>,
}

struct PaintLine {
    number: ShapedLine,
    text: String,
    shaped: ShapedLine,
    y: Pixels,
}

pub struct EditorPrepaint {
    gutter_width: Pixels,
    lines: Vec<PaintLine>,
    first_line: usize,
    current_line_y: Option<Pixels>,
    selection: Vec<PaintQuad>,
    underlines: Vec<PaintQuad>,
    caret: Option<PaintQuad>,
    gutter_quads: Vec<PaintQuad>,
    blame_lines: Vec<(ShapedLine, Pixels)>,
    plain: bool,
}

impl IntoElement for EditorElement {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for EditorElement {
    type RequestLayoutState = ();
    type PrepaintState = EditorPrepaint;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> EditorPrepaint {
        let line_height = EditorView::line_height();
        let font_size = px(theme::editor_font_size());
        let run = |len: usize, color: Hsla| TextRun {
            len,
            font: font(theme::editor_font()),
            color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };

        self.view.update(cx, |view, _| {
            let digits = view.buffer.len_lines().to_string().len().max(3);
            let digit_width = window
                .text_system()
                .shape_line("0".into(), font_size, &[run(1, theme::line_number())], None)
                .width;
            let blame_width = if view.blame.is_some() { digit_width * BLAME_CHARS as f32 } else { px(0.) };
            let gutter_width = if view.plain { px(0.) } else { blame_width + digit_width * digits as f32 + px(GUTTER_PADDING * 2.) };
            let markers = view.git_markers();
            let mut gutter_quads = Vec::new();
            let mut blame_lines = Vec::new();
            let text_x = bounds.left() + gutter_width + px(TEXT_PADDING);

            // Keep the caret visible after keyboard edits and movement.
            let (caret_line, caret_col) = view.buffer.char_to_line_col(view.buffer.selection.head);
            if std::mem::take(&mut view.center_on_caret) {
                view.autoscroll = false;
                let caret_top = line_height * caret_line as f32;
                view.scroll_y = (caret_top - bounds.size.height / 3.).max(px(0.));
            }
            if std::mem::take(&mut view.autoscroll) {
                let caret_top = line_height * caret_line as f32;
                let viewport = bounds.size.height - line_height;
                if caret_top < view.scroll_y {
                    view.scroll_y = caret_top;
                } else if caret_top > view.scroll_y + viewport {
                    view.scroll_y = caret_top - viewport;
                }
            }

            let first_line = (view.scroll_y / line_height).floor() as usize;
            let line_count = (bounds.size.height / line_height).ceil() as usize + 1;
            let last_line = (first_line + line_count).min(view.buffer.len_lines());
            let offset_y = bounds.top() - (view.scroll_y - line_height * first_line as f32);
            let selection = view.buffer.selection.range();
            let spans = view.buffer.highlights().to_vec();
            let problems: Vec<Range<usize>> = view
                .current_diagnostics()
                .iter()
                .map(|u| u.range.clone())
                .collect();
            let link = view.hover_link.clone();
            let mut underlines = Vec::new();
            let (find_matches, find_current) = match &view.find {
                Some(find) => (find.matches.clone(), find.current),
                None => (Vec::new(), None),
            };

            let mut lines = Vec::new();
            let mut selection_quads = Vec::new();
            let mut caret = None;
            for line in first_line..last_line {
                let text = view.buffer.line(line);
                let line_start = view.buffer.line_to_byte(line);
                let line_end = line_start + text.len();

                let mut runs = Vec::new();
                let mut cursor = line_start;
                let first_span = spans.partition_point(|s| s.range.end <= line_start);
                for span in &spans[first_span..] {
                    if span.range.start >= line_end {
                        break;
                    }
                    let start = span.range.start.max(line_start);
                    let end = span.range.end.min(line_end);
                    if start > cursor {
                        runs.push(run(start - cursor, theme::text()));
                    }
                    runs.push(run(end - start, theme::syntax(span.highlight)));
                    cursor = end;
                }
                if line_end > cursor {
                    runs.push(run(line_end - cursor, theme::text()));
                }

                let shaped =
                    window
                        .text_system()
                        .shape_line(text.clone().into(), font_size, &runs, None);
                let number: SharedString = (line + 1).to_string().into();
                let number_len = number.len();
                let number_color = if line == caret_line {
                    theme::ui_text()
                } else {
                    theme::line_number()
                };
                let number = window.text_system().shape_line(
                    number,
                    font_size,
                    &[run(number_len, number_color)],
                    None,
                );
                let y = offset_y + line_height * (line - first_line) as f32;

                let line_char_start = view.buffer.line_to_char(line);
                let line_char_end = line_char_start + text.chars().count();
                if !selection.is_empty()
                    && selection.start <= line_char_end
                    && selection.end > line_char_start
                {
                    let start_col = selection.start.saturating_sub(line_char_start);
                    let end_col = selection.end.min(line_char_end) - line_char_start;
                    let x0 = shaped.x_for_index(byte_of_col(&text, start_col));
                    let mut x1 = shaped.x_for_index(byte_of_col(&text, end_col));
                    if selection.end > line_char_end {
                        x1 += digit_width; // show the selected line break
                    }
                    selection_quads.push(fill(
                        Bounds::from_corners(
                            point(text_x + x0, y),
                            point(text_x + x1, y + line_height),
                        ),
                        theme::selection(),
                    ));
                }
                if line == caret_line {
                    let x = shaped.x_for_index(byte_of_col(&text, caret_col));
                    caret = Some(fill(
                        Bounds::new(point(text_x + x, y), size(px(2.), line_height)),
                        theme::caret(),
                    ));
                }
                let underline = |range: &Range<usize>, color: Hsla, out: &mut Vec<PaintQuad>| {
                    if range.end <= line_start || range.start > line_end || range.is_empty() {
                        return;
                    }
                    let x0 = shaped.x_for_index(range.start.max(line_start) - line_start);
                    let x1 = shaped.x_for_index(range.end.min(line_end) - line_start);
                    out.push(fill(
                        Bounds::from_corners(
                            point(text_x + x0, y + line_height - px(3.)),
                            point(text_x + x1, y + line_height - px(1.5)),
                        ),
                        color,
                    ));
                };
                for problem in &problems {
                    underline(problem, theme::error(), &mut underlines);
                }
                let first_match = find_matches.partition_point(|m| m.end <= line_start);
                for (ix, found) in find_matches.iter().enumerate().skip(first_match) {
                    if found.start > line_end {
                        break;
                    }
                    let x0 = shaped.x_for_index(found.start.max(line_start) - line_start);
                    let x1 = shaped.x_for_index(found.end.min(line_end) - line_start);
                    let color = if Some(ix) == find_current {
                        theme::find_current()
                    } else {
                        theme::find_match()
                    };
                    selection_quads.insert(
                        0,
                        fill(
                            Bounds::from_corners(
                                point(text_x + x0, y),
                                point(text_x + x1, y + line_height),
                            ),
                            color,
                        ),
                    );
                }
                if let Some(link) = &link {
                    underline(link, theme::link(), &mut underlines);
                }
                if line == caret_line {
                    let x = shaped.x_for_index(byte_of_col(&text, caret_col));
                    view.caret_point = Some(point(text_x + x, y + line_height));
                }
                let marker_x = bounds.left() + gutter_width - px(4.);
                let first_marker = markers.partition_point(|(l, _)| *l < line);
                for (_, change) in markers[first_marker..].iter().take_while(|(l, _)| *l == line) {
                    gutter_quads.push(match change {
                        git::LineChange::Added => fill(Bounds::new(point(marker_x, y), size(px(3.), line_height)), theme::success()),
                        git::LineChange::Modified => fill(Bounds::new(point(marker_x, y), size(px(3.), line_height)), theme::accent()),
                        git::LineChange::DeletedAbove => fill(Bounds::new(point(marker_x - px(2.), y - px(2.)), size(px(7.), px(4.))), theme::muted()),
                    });
                }
                if let Some(entry) = view.blame.as_ref().and_then(|b| b.get(line)) {
                    let author: String = entry.author.chars().take(12).collect();
                    let label = format!("{:<12} {}", author, entry.date);
                    let len = label.len();
                    blame_lines.push((
                        window.text_system().shape_line(label.into(), px(theme::editor_font_size() - 1.5), &[run(len, theme::muted())], None),
                        y,
                    ));
                }
                lines.push(PaintLine {
                    number,
                    text,
                    shaped,
                    y,
                });
            }

            let current_line_y = (caret_line >= first_line && caret_line < last_line)
                .then(|| offset_y + line_height * (caret_line - first_line) as f32);
            EditorPrepaint {
                gutter_width,
                lines,
                first_line,
                current_line_y,
                selection: selection_quads,
                underlines,
                caret,
                gutter_quads,
                blame_lines,
                plain: view.plain,
            }
        })
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        prepaint: &mut EditorPrepaint,
        window: &mut Window,
        cx: &mut App,
    ) {
        let line_height = EditorView::line_height();
        let focus_handle = self.view.read(cx).focus_handle.clone();
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.view.clone()),
            cx,
        );

        let gutter = prepaint.gutter_width;
        let text_x = bounds.left() + gutter + px(TEXT_PADDING);
        window.paint_quad(fill(
            Bounds::new(bounds.origin, size(gutter, bounds.size.height)),
            theme::gutter_bg(),
        ));
        if let Some(y) = prepaint.current_line_y {
            window.paint_quad(fill(
                Bounds::from_corners(
                    point(bounds.left() + gutter, y),
                    point(bounds.right(), y + line_height),
                ),
                theme::current_line(),
            ));
        }
        for quad in prepaint
            .selection
            .drain(..)
            .chain(prepaint.underlines.drain(..))
        {
            window.paint_quad(quad);
        }
        for quad in prepaint.gutter_quads.drain(..) {
            window.paint_quad(quad);
        }
        for (label, y) in &prepaint.blame_lines {
            label.paint(point(bounds.left() + px(6.), *y), line_height, window, cx).ok();
        }
        for line in &prepaint.lines {
            if !prepaint.plain {
                let number_x = bounds.left() + gutter - px(GUTTER_PADDING) - line.number.width;
                line.number.paint(point(number_x, line.y), line_height, window, cx).ok();
            }
            line.shaped
                .paint(point(text_x, line.y), line_height, window, cx)
                .ok();
        }
        if focus_handle.is_focused(window)
            && let Some(caret) = prepaint.caret.take()
        {
            window.paint_quad(caret);
        }

        let lines = std::mem::take(&mut prepaint.lines)
            .into_iter()
            .map(|l| (l.text, l.shaped))
            .collect();
        let first_line = prepaint.first_line;
        self.view.update(cx, |view, _| {
            view.layout = Some(LayoutInfo {
                bounds,
                text_x,
                first_line,
                lines,
            });
        });
    }
}
