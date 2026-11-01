//! Code editor view: renders a [`Buffer`] with gutter, highlighting, caret and selection.

mod bookmarks;
mod find_bar;
mod folding;
mod generate;
mod hierarchy;
mod info;
mod rows;
mod ghost;
mod intel_ui;
mod multicaret;
mod text_ops;
mod usages;

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

use rows::{Row, RowText};

use crate::intel::Intel;
use crate::theme;
use crate::workspace::Workspace;
pub use find_bar::{ShowFind, ShowReplace};
use intel_ui::Menu;
pub use intel_ui::OptimizeImports;

actions!(
    editor,
    [
        AcceptGhost,
        DismissGhost,
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
        AddNextOccurrence,
        UnselectOccurrence,
        SelectAllOccurrences,
        CaretsAtLineEnds,
        CloneCaretAbove,
        CloneCaretBelow,
        ClearCarets,
        CollapseRegion,
        ExpandRegion,
        CollapseAll,
        ExpandAll,
        FoldSelection,
        ToggleSoftWrap,
        ToggleBookmark,
        ToggleBookmarkWithMnemonic,
        NextBookmark,
        PrevBookmark,
        ToggleWhitespace,
        FindUsages,
        HighlightUsages,
        ClearUsageHighlights,
        RenameSymbol,
        Generate,
        QuickDocumentation,
        ParameterInfo,
        CloseInfo,
        GoToImplementation,
        GoToSuperMethod,
        GoToTest,
        TypeHierarchy,
        ExtendSelection,
        ShrinkSelection,
        JoinLines,
        SortLines,
        ReverseLines,
        ToggleCharCase,
        Unindent,
        GoToMatchingBrace,
        PasteFromHistory,
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
    /// Left edge of the fold-arrow column in the gutter.
    fold_x: Pixels,
    first_row: usize,
    rows: Vec<(Row, RowText, ShapedLine)>,
}

pub struct EditorView {
    pub buffer: Buffer,
    focus_handle: FocusHandle,
    scroll_y: Pixels,
    autoscroll: bool,
    /// Scroll so the caret sits near the middle (after navigation).
    center_on_caret: bool,
    is_selecting: bool,
    /// ⌥-drag: where the column selection started.
    column_anchor: Option<usize>,
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
    /// AI inline completion and the request waiting for a pause in typing.
    ghost: Option<ghost::Ghost>,
    ghost_task: Option<gpui::Task<()>>,
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
    folding: folding::Folding,
    /// Soft wrap: `None` follows the file type (on for Markdown and text files).
    soft_wrap: Option<bool>,
    /// Visual rows for (version, collapsed regions, wrap width).
    rows_cache: Option<(u64, Vec<(usize, usize)>, Option<usize>, std::rc::Rc<Vec<Row>>)>,
    /// Number of visual rows at the last paint (scroll limit).
    row_count: usize,
    /// Bookmarks in this file: (line anchor, mnemonic).
    bookmarks: Vec<(usize, Option<char>)>,
    /// View ▸ Show Whitespaces.
    show_whitespace: bool,
    /// Highlight Usages in File: byte ranges for a buffer version.
    usage_highlights: Option<(u64, Vec<Range<usize>>)>,
    /// Quick Documentation / Parameter Info popup.
    info: Option<info::Info>,
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
            folding: Default::default(),
            soft_wrap: None,
            rows_cache: None,
            row_count: 0,
            bookmarks: Vec::new(),
            show_whitespace: false,
            usage_highlights: None,
            info: None,
            buffer,
            focus_handle: cx.focus_handle(),
            scroll_y: px(0.),
            autoscroll: false,
            center_on_caret: false,
            is_selecting: false,
            column_anchor: None,
            marked_range: None,
            layout: None,
            plain: false,
            md_mode: MdMode::Split,
            ghost: None,
            ghost_task: None,
            md_cache: None,
        }
        .with_bookmarks()
    }

    fn with_bookmarks(mut self) -> Self {
        self.load_bookmarks();
        self
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
        self.info = None;
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
        self.skip_folds(false);
        self.moved(cx);
    }
    fn move_down(&mut self, _: &MoveDown, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.move_vertical(1, false);
        self.skip_folds(true);
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
        self.skip_folds(false);
        self.moved(cx);
    }
    fn select_down(&mut self, _: &SelectDown, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.move_vertical(1, true);
        self.skip_folds(true);
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
    fn has_selected_text(&self) -> bool {
        self.buffer.selections().iter().any(|s| !s.is_empty())
    }
    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if self.has_selected_text() {
            let text = self.buffer.selections_text();
            text_ops::remember_copy(&text);
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }
    fn cut(&mut self, _: &Cut, _: &mut Window, cx: &mut Context<Self>) {
        if self.has_selected_text() {
            let text = self.buffer.selections_text();
            text_ops::remember_copy(&text);
            cx.write_to_clipboard(ClipboardItem::new_string(text));
            self.buffer.insert("");
            self.changed(cx);
        }
    }
    fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.buffer.paste(&text);
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
        self.save_bookmarks();
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

    /// The visual row at a window position, with its layout when it is on screen.
    fn row_at(&self, position: Point<Pixels>) -> Option<(usize, Option<&(Row, RowText, ShapedLine)>)> {
        let layout = self.layout.as_ref()?;
        let y = position.y - layout.bounds.top() + self.scroll_y;
        let row = (y / Self::line_height()).floor().max(0.) as usize;
        Some((row, layout.rows.get(row.wrapping_sub(layout.first_row))))
    }

    fn char_for_position(&self, position: Point<Pixels>) -> Option<usize> {
        let layout = self.layout.as_ref()?;
        let (row, shown) = self.row_at(position)?;
        if row >= self.row_count {
            return Some(self.buffer.len_chars());
        }
        let Some((row, text, shaped)) = shown else {
            return self.rows_cache.as_ref().and_then(|(.., rows)| rows.get(row)).map(Row::start);
        };
        let display = shaped.closest_index_for_x(position.x - layout.text_x);
        Some(match text.char_at(&self.buffer, row, display) {
            Ok(ix) => ix,
            Err(seg) => row.segments[seg].start,
        })
    }

    /// The collapsed region whose placeholder is under the mouse (its open position).
    fn placeholder_at(&self, position: Point<Pixels>) -> Option<usize> {
        let layout = self.layout.as_ref()?;
        let (_, Some((row, text, shaped))) = self.row_at(position)? else { return None };
        let display = shaped.index_for_x(position.x - layout.text_x)?;
        let seg = text.placeholder_at(display)?;
        Some(row.segments[seg - 1].end - 1)
    }

    /// A click in the fold-arrow column: the buffer line of that row.
    fn fold_arrow_at(&self, position: Point<Pixels>) -> Option<usize> {
        let layout = self.layout.as_ref()?;
        if self.plain || position.x < layout.fold_x || position.x >= layout.text_x - px(TEXT_PADDING) {
            return None;
        }
        let (_, Some((row, ..))) = self.row_at(position)? else { return None };
        row.first.then_some(row.line)
    }

    /// Rows for the current text, collapsed regions and wrap width (cached).
    fn rows(&mut self, collapsed: &[(usize, usize)], wrap: Option<usize>) -> std::rc::Rc<Vec<Row>> {
        let version = self.buffer.version();
        match &self.rows_cache {
            Some((v, c, w, rows)) if *v == version && c == collapsed && *w == wrap => rows.clone(),
            _ => {
                let rows = std::rc::Rc::new(rows::build(&self.buffer, collapsed, wrap));
                self.rows_cache = Some((version, collapsed.to_vec(), wrap, rows.clone()));
                rows
            }
        }
    }

    fn soft_wrap_enabled(&self) -> bool {
        self.soft_wrap.unwrap_or_else(|| {
            self.buffer.path().and_then(|p| p.extension()).is_some_and(|e| {
                let e = e.to_string_lossy().to_lowercase();
                matches!(e.as_str(), "md" | "markdown" | "txt" | "log")
            })
        })
    }

    fn toggle_soft_wrap(&mut self, _: &ToggleSoftWrap, _: &mut Window, cx: &mut Context<Self>) {
        self.soft_wrap = Some(!self.soft_wrap_enabled());
        cx.notify();
    }

    fn toggle_whitespace(&mut self, _: &ToggleWhitespace, _: &mut Window, cx: &mut Context<Self>) {
        self.show_whitespace = !self.show_whitespace;
        cx.notify();
    }

    fn extend_selection(&mut self, _: &ExtendSelection, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.extend_selection();
        self.moved(cx);
    }

    fn shrink_selection(&mut self, _: &ShrinkSelection, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.shrink_selection();
        self.moved(cx);
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
        if let Some(line) = self.fold_arrow_at(event.position) {
            self.toggle_fold_on_line(line, cx);
            cx.stop_propagation();
            return;
        }
        if let Some(open) = self.placeholder_at(event.position) {
            self.expand_fold(open, cx);
            cx.stop_propagation();
            return;
        }
        if let Some(ix) = self.char_for_position(event.position) {
            // ⌥⇧-click adds a caret; ⌥-drag selects a column.
            if event.modifiers.alt && event.click_count == 1 {
                if event.modifiers.shift {
                    self.buffer.add_selection(Selection::caret(ix));
                } else {
                    self.buffer.select_column(ix, ix);
                    self.column_anchor = Some(ix);
                }
                cx.stop_propagation();
                cx.notify();
                return;
            }
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
    /// Reloads the HEAD version (gutter change markers) and blame in the background, so opening
    /// a file doesn't wait for `git`.
    pub fn refresh_git_base(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.buffer.path().map(std::path::Path::to_path_buf) else { return };
        let intel = self.intel.clone();
        let with_blame = self.blame.is_some();
        cx.spawn(async move |this, cx| {
            let (base, blame) = cx
                .background_executor()
                .spawn(async move { (intel.git_base(&path), with_blame.then(|| intel.git_blame(&path)).flatten()) })
                .await;
            this.update(cx, |this, cx| {
                this.git_base = base;
                this.git_markers = None;
                if this.blame.is_some() {
                    this.blame = blame;
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
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
        self.column_anchor = None;
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.update_hover_link(event.position, event.modifiers.platform, cx);
        if let Some(anchor) = self.column_anchor
            && let Some(ix) = self.char_for_position(event.position)
        {
            self.buffer.select_column(anchor, ix);
            self.changed(cx);
            return;
        }
        if self.is_selecting
            && let Some(ix) = self.char_for_position(event.position)
        {
            self.buffer.move_to(ix, true);
            self.changed(cx);
        }
    }

    fn on_scroll(&mut self, event: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        let delta = event.delta.pixel_delta(Self::line_height()).y;
        let max = Self::line_height() * self.row_count.max(1).saturating_sub(1) as f32;
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
        // Typing with several carets goes to all of them.
        if self.buffer.has_extra_carets() && range == self.buffer.selection.range() {
            self.close_menu();
            self.buffer.type_text(new_text);
            self.changed(cx);
            return;
        }
        // Like PhpStorm's lookup char filter: punctuation accepts the selected completion
        // (with its auto-import) before being typed, so `TextInput` + `::` imports the class.
        if range.is_empty() && self.commit_completion_with(new_text, cx) {
            if new_text == "(" && self.buffer.slice(self.buffer.selection.head.saturating_sub(1)..self.buffer.selection.head + 1) == "()" {
                // The completion already inserted `()`.
                self.changed(cx);
                return;
            }
            self.buffer.type_text(new_text);
            self.changed(cx);
            self.after_typing(new_text, cx);
            return;
        }
        if range == self.buffer.selection.range() {
            self.buffer.type_text(new_text);
        } else {
            self.buffer.replace(range, new_text);
        }
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
        let (ix, (row, text, shaped)) = layout.rows.iter().enumerate().find(|(_, (row, ..))| row.contains(start))?;
        let display = text.display_char(&self.buffer, row, start)?;
        let top = layout.bounds.top() + Self::line_height() * (layout.first_row + ix) as f32 - self.scroll_y;
        Some(Bounds::new(
            point(layout.text_x + shaped.x_for_index(display), top),
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
        if self.buffer.path().is_some_and(crate::http_client::is_http_file) {
            key_context.add("HttpFile");
        }
        if self.buffer.path().is_some_and(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("sql"))) {
            key_context.add("SqlFile");
        }
        if self.buffer.has_extra_carets() {
            key_context.add("multicaret");
        }
        if !self.current_usage_highlights().is_empty() {
            key_context.add("usages");
        }
        if self.info.is_some() {
            key_context.add("info");
        }
        if self.menu_open() {
            key_context.add("menu");
        } else if self.ghost().is_some() {
            key_context.add("ghost");
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
            .on_action(cx.listener(Self::add_next_occurrence))
            .on_action(cx.listener(Self::unselect_occurrence))
            .on_action(cx.listener(Self::select_all_occurrences))
            .on_action(cx.listener(Self::carets_at_line_ends))
            .on_action(cx.listener(Self::clone_caret_above))
            .on_action(cx.listener(Self::clone_caret_below))
            .on_action(cx.listener(Self::clear_carets))
            .on_action(cx.listener(Self::join_lines))
            .on_action(cx.listener(Self::sort_lines))
            .on_action(cx.listener(Self::reverse_lines))
            .on_action(cx.listener(Self::toggle_char_case))
            .on_action(cx.listener(Self::unindent))
            .on_action(cx.listener(Self::go_to_matching_brace))
            .on_action(cx.listener(Self::paste_from_history))
            .on_action(cx.listener(Self::collapse_region))
            .on_action(cx.listener(Self::expand_region))
            .on_action(cx.listener(Self::collapse_all))
            .on_action(cx.listener(Self::expand_all))
            .on_action(cx.listener(Self::fold_selection))
            .on_action(cx.listener(Self::toggle_soft_wrap))
            .on_action(cx.listener(Self::extend_selection))
            .on_action(cx.listener(Self::shrink_selection))
            .on_action(cx.listener(Self::toggle_bookmark))
            .on_action(cx.listener(Self::toggle_bookmark_with_mnemonic))
            .on_action(cx.listener(Self::next_bookmark))
            .on_action(cx.listener(Self::prev_bookmark))
            .on_action(cx.listener(Self::toggle_whitespace))
            .on_action(cx.listener(Self::find_usages))
            .on_action(cx.listener(Self::highlight_usages))
            .on_action(cx.listener(Self::clear_usage_highlights))
            .on_action(cx.listener(Self::rename_symbol))
            .on_action(cx.listener(Self::generate))
            .on_action(cx.listener(Self::quick_documentation))
            .on_action(cx.listener(Self::parameter_info))
            .on_action(cx.listener(Self::close_info))
            .on_action(cx.listener(Self::go_to_implementation))
            .on_action(cx.listener(Self::go_to_super_method))
            .on_action(cx.listener(Self::go_to_test))
            .on_action(cx.listener(Self::type_hierarchy))
            .on_action(cx.listener(Self::accept_ghost))
            .on_action(cx.listener(Self::dismiss_ghost))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_scroll_wheel(cx.listener(Self::on_scroll))
            .child(EditorElement { view: cx.entity() })
            .children(self.render_menu(cx))
            .children(self.render_info());
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
    /// Line number (first row of a line only).
    number: Option<ShapedLine>,
    row: Row,
    row_text: RowText,
    shaped: ShapedLine,
    y: Pixels,
}

pub struct EditorPrepaint {
    gutter_width: Pixels,
    fold_width: Pixels,
    lines: Vec<PaintLine>,
    first_row: usize,
    current_line_y: Option<Pixels>,
    selection: Vec<PaintQuad>,
    underlines: Vec<PaintQuad>,
    caret: Option<PaintQuad>,
    extra_carets: Vec<PaintQuad>,
    gutter_quads: Vec<PaintQuad>,
    blame_lines: Vec<(ShapedLine, Pixels)>,
    fold_marks: Vec<(ShapedLine, Point<Pixels>)>,
    plain: bool,
    /// AI suggestion: first line shaped, painted at this origin.
    ghost: Option<(ShapedLine, Point<Pixels>)>,
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
            let fold_width = if view.plain { px(0.) } else { digit_width * 1.8 };
            let gutter_width = if view.plain { px(0.) } else { blame_width + digit_width * digits as f32 + fold_width + px(GUTTER_PADDING * 2.) };
            let markers = view.git_markers();
            let bookmark_lines = view.bookmark_lines();
            let indent_cols = match view.buffer.indent_unit() {
                "\t" => 4,
                unit => unit.len().max(1),
            };
            let show_whitespace = view.show_whitespace;
            let mut gutter_quads = Vec::new();
            let mut blame_lines = Vec::new();
            let mut fold_marks = Vec::new();
            let text_x = bounds.left() + gutter_width + px(TEXT_PADDING);

            // Visual rows: collapsed regions take one row, long lines may wrap.
            let candidates = if view.plain { Vec::new() } else { view.fold_candidates() };
            let collapsed = view.collapsed_folds();
            let wrap = view.soft_wrap_enabled().then(|| {
                let width = bounds.size.width - gutter_width - px(TEXT_PADDING * 2.);
                ((width / digit_width).floor() as usize).max(20)
            });
            let rows = view.rows(&collapsed, wrap);
            view.row_count = rows.len();

            // Keep the caret visible after keyboard edits and movement.
            let head = view.buffer.selection.head;
            let caret_line = view.buffer.char_to_line_col(head).0;
            let caret_row = rows::row_of(&rows, head);
            if std::mem::take(&mut view.center_on_caret) {
                view.autoscroll = false;
                let caret_top = line_height * caret_row as f32;
                view.scroll_y = (caret_top - bounds.size.height / 3.).max(px(0.));
            }
            if std::mem::take(&mut view.autoscroll) {
                let caret_top = line_height * caret_row as f32;
                let viewport = bounds.size.height - line_height;
                if caret_top < view.scroll_y {
                    view.scroll_y = caret_top;
                } else if caret_top > view.scroll_y + viewport {
                    view.scroll_y = caret_top - viewport;
                }
            }

            let first_row = ((view.scroll_y / line_height).floor() as usize).min(rows.len().saturating_sub(1));
            let row_count = (bounds.size.height / line_height).ceil() as usize + 1;
            let last_row = (first_row + row_count).min(rows.len());
            let offset_y = bounds.top() - (view.scroll_y - line_height * first_row as f32);
            let byte_range = |b: &Buffer, r: &Range<usize>| b.char_to_byte(r.start)..b.char_to_byte(r.end);
            let selections: Vec<Range<usize>> = view.buffer.selections().iter().map(|s| s.range()).filter(|r| !r.is_empty()).collect();
            let extra_carets: Vec<usize> = view.buffer.extra.iter().map(|s| s.head).collect();
            let mut extra_caret_quads = Vec::new();
            let brackets: Vec<usize> = view.buffer.matching_bracket(head)
                .filter(|_| view.buffer.selection.is_empty())
                .map(|(a, b)| vec![a, b]).unwrap_or_default();
            let spans = view.buffer.highlights().to_vec();
            let problems: Vec<Range<usize>> = view
                .current_diagnostics()
                .iter()
                .map(|u| u.range.clone())
                .collect();
            let link = view.hover_link.clone();
            let mut underlines = Vec::new();
            let (mut find_matches, find_current) = match &view.find {
                Some(find) => (find.matches.clone(), find.current),
                None => (Vec::new(), None),
            };
            if find_matches.is_empty() {
                find_matches = view.current_usage_highlights();
            }

            let mut lines = Vec::new();
            let mut selection_quads = Vec::new();
            let mut caret = None;
            let mut ghost = None;
            for row_ix in first_row..last_row {
                let row = &rows[row_ix];
                let row_text = RowText::new(&view.buffer, row);
                let text = row_text.text.clone();

                let mut runs = Vec::new();
                for (seg_ix, seg) in row.segments.iter().enumerate() {
                    if seg_ix > 0 {
                        runs.push(TextRun { background_color: Some(theme::border()), ..run(rows::PLACEHOLDER.len(), theme::muted()) });
                    }
                    let seg_start = view.buffer.char_to_byte(seg.start);
                    let seg_end = view.buffer.char_to_byte(seg.end);
                    let mut cursor = seg_start;
                    let first_span = spans.partition_point(|s| s.range.end <= seg_start);
                    for span in &spans[first_span..] {
                        if span.range.start >= seg_end {
                            break;
                        }
                        let start = span.range.start.max(cursor);
                        let end = span.range.end.min(seg_end);
                        if start > cursor {
                            runs.push(run(start - cursor, theme::text()));
                        }
                        if end > start {
                            runs.push(run(end - start, theme::syntax(span.highlight)));
                            cursor = end;
                        }
                    }
                    if seg_end > cursor {
                        runs.push(run(seg_end - cursor, theme::text()));
                    }
                }

                let shaped = window.text_system().shape_line(text.clone().into(), font_size, &runs, None);
                let line = row.line;
                let number = row.first.then(|| {
                    let number: SharedString = (line + 1).to_string().into();
                    let len = number.len();
                    let color = if line == caret_line { theme::ui_text() } else { theme::line_number() };
                    window.text_system().shape_line(number, font_size, &[run(len, color)], None)
                });
                let y = offset_y + line_height * (row_ix - first_row) as f32;
                let quad = |range: Range<usize>, color: Hsla| {
                    fill(Bounds::from_corners(point(text_x + shaped.x_for_index(range.start), y), point(text_x + shaped.x_for_index(range.end), y + line_height)), color)
                };

                for selection in &selections {
                    for r in row_text.display_ranges(&view.buffer, row, &byte_range(&view.buffer, selection)) {
                        selection_quads.push(quad(r, theme::selection()));
                    }
                    // Show the selected line break.
                    if selection.start <= row.end() && selection.end > row.end() {
                        let x = shaped.x_for_index(text.len());
                        selection_quads.push(fill(Bounds::from_corners(point(text_x + x, y), point(text_x + x + digit_width, y + line_height)), theme::selection()));
                    }
                }
                for &bracket in &brackets {
                    let b = view.buffer.char_to_byte(bracket);
                    for r in row_text.display_ranges(&view.buffer, row, &(b..b + 1)) {
                        selection_quads.push(quad(r, theme::find_match()));
                    }
                }
                for &c in &extra_carets {
                    if rows::row_of(&rows, c) == row_ix
                        && let Some(d) = row_text.display_char(&view.buffer, row, c)
                    {
                        let x = shaped.x_for_index(d);
                        extra_caret_quads.push(fill(Bounds::new(point(text_x + x, y), size(px(2.), line_height)), theme::caret()));
                    }
                }
                if row_ix == caret_row
                    && let Some(d) = row_text.display_char(&view.buffer, row, head)
                {
                    let x = shaped.x_for_index(d);
                    caret = Some(fill(Bounds::new(point(text_x + x, y), size(px(2.), line_height)), theme::caret()));
                    view.caret_point = Some(point(text_x + x, y + line_height));
                    if let Some(suggestion) = view.ghost() {
                        let mut lines = suggestion.text.lines();
                        let first = lines.next().unwrap_or("").replace('\t', "    ");
                        let more = lines.count();
                        let label = if more > 0 { format!("{first}  ⋯ +{more} lines (Tab)") } else { first };
                        let ghost_run = TextRun {
                            len: label.len(),
                            font: gpui::Font { style: gpui::FontStyle::Italic, ..font(theme::editor_font()) },
                            color: theme::muted().opacity(0.8),
                            background_color: None,
                            underline: None,
                            strikethrough: None,
                        };
                        let ghost_line = window.text_system().shape_line(label.into(), font_size, &[ghost_run], None);
                        ghost = Some((ghost_line, point(text_x + x + px(1.), y)));
                    }
                }
                let underline = |range: &Range<usize>, color: Hsla, out: &mut Vec<PaintQuad>| {
                    for r in row_text.display_ranges(&view.buffer, row, range) {
                        out.push(fill(
                            Bounds::from_corners(
                                point(text_x + shaped.x_for_index(r.start), y + line_height - px(3.)),
                                point(text_x + shaped.x_for_index(r.end), y + line_height - px(1.5)),
                            ),
                            color,
                        ));
                    }
                };
                for problem in &problems {
                    underline(problem, theme::error(), &mut underlines);
                }
                let row_start = view.buffer.char_to_byte(row.start());
                let row_end = view.buffer.char_to_byte(row.end());
                let first_match = find_matches.partition_point(|m| m.end <= row_start);
                for (ix, found) in find_matches.iter().enumerate().skip(first_match) {
                    if found.start > row_end {
                        break;
                    }
                    let color = if Some(ix) == find_current { theme::find_current() } else { theme::find_match() };
                    for r in row_text.display_ranges(&view.buffer, row, found) {
                        selection_quads.insert(0, quad(r, color));
                    }
                }
                if let Some(link) = &link {
                    underline(link, theme::link(), &mut underlines);
                }
                // Indent guides at each indentation level of the line's leading whitespace.
                if row.first && !view.plain {
                    let lead: usize = text.chars().take_while(|c| *c == ' ' || *c == '\t').map(|c| if c == '\t' { 4 } else { 1 }).sum();
                    let lead = if text.trim().is_empty() { 0 } else { lead };
                    for level in 1..lead.div_ceil(indent_cols) {
                        let x = text_x + digit_width * (level * indent_cols) as f32;
                        if level * indent_cols < lead {
                            gutter_quads.push(fill(Bounds::new(point(x, y), size(px(1.), line_height)), theme::border().opacity(0.6)));
                        }
                    }
                }
                if show_whitespace {
                    for (byte, c) in text.char_indices().filter(|(_, c)| *c == ' ' || *c == '\t') {
                        let x0 = shaped.x_for_index(byte);
                        let x1 = shaped.x_for_index(byte + 1);
                        let mid = (x0 + x1) / 2.;
                        let quad = if c == ' ' {
                            fill(Bounds::new(point(text_x + mid - px(1.), y + line_height / 2. - px(1.)), size(px(2.), px(2.))), theme::muted().opacity(0.6))
                        } else {
                            fill(Bounds::new(point(text_x + x0 + px(2.), y + line_height / 2.), size(x1 - x0 - px(4.), px(1.))), theme::muted().opacity(0.6))
                        };
                        gutter_quads.push(quad);
                    }
                }
                if row.first {
                    let marker_x = bounds.left() + gutter_width - fold_width - px(4.);
                    if let Some((_, mnemonic)) = bookmark_lines.iter().find(|(l, _)| *l == line) {
                        let label: SharedString = mnemonic.map_or("◆".to_string(), |m| m.to_string()).into();
                        let len = label.len();
                        let mark = window.text_system().shape_line(label, px(theme::editor_font_size() - 2.), &[run(len, theme::accent())], None);
                        fold_marks.push((mark, point(bounds.left() + px(4.), y)));
                    }
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
                    // Fold arrow: ▸ collapsed, ▾ expanded.
                    if let Some(&(open, _)) = candidates.iter().find(|(o, _)| view.buffer.char_to_line_col(*o).0 == line) {
                        let folded = collapsed.iter().any(|(o, _)| *o == open);
                        let arrow: SharedString = if folded { "▸".into() } else { "▾".into() };
                        let shaped_arrow = window.text_system().shape_line(arrow, font_size, &[run("▸".len(), theme::muted())], None);
                        fold_marks.push((shaped_arrow, point(bounds.left() + gutter_width - fold_width + px(2.), y)));
                    }
                }
                lines.push(PaintLine { number, row: row.clone(), row_text, shaped, y });
            }

            let current_line_y = (caret_row >= first_row && caret_row < last_row)
                .then(|| offset_y + line_height * (caret_row - first_row) as f32);
            EditorPrepaint {
                gutter_width,
                fold_width,
                lines,
                first_row,
                current_line_y,
                selection: selection_quads,
                underlines,
                caret,
                extra_carets: extra_caret_quads,
                gutter_quads,
                blame_lines,
                fold_marks,
                plain: view.plain,
                ghost,
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
        for (arrow, origin) in &prepaint.fold_marks {
            arrow.paint(*origin, line_height, window, cx).ok();
        }
        for line in &prepaint.lines {
            if !prepaint.plain
                && let Some(number) = &line.number
            {
                let number_x = bounds.left() + gutter - prepaint.fold_width - px(GUTTER_PADDING) - number.width;
                number.paint(point(number_x, line.y), line_height, window, cx).ok();
            }
            line.shaped
                .paint(point(text_x, line.y), line_height, window, cx)
                .ok();
        }
        if let Some((line, origin)) = prepaint.ghost.take() {
            line.paint(origin, line_height, window, cx).ok();
        }
        if focus_handle.is_focused(window) {
            for caret in prepaint.caret.take().into_iter().chain(prepaint.extra_carets.drain(..)) {
                window.paint_quad(caret);
            }
        }

        let rows = std::mem::take(&mut prepaint.lines)
            .into_iter()
            .map(|l| (l.row, l.row_text, l.shaped))
            .collect();
        let first_row = prepaint.first_row;
        let fold_x = bounds.left() + gutter - prepaint.fold_width;
        self.view.update(cx, |view, _| {
            view.layout = Some(LayoutInfo {
                bounds,
                text_x,
                fold_x,
                first_row,
                rows,
            });
        });
    }
}
