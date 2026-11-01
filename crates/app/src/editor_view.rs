//! Code editor view: renders a [`Buffer`] with gutter, highlighting, caret and selection.

use std::ops::Range;

use editor::{Buffer, Selection};
use gpui::{
    App, Bounds, ClipboardItem, Context, CursorStyle, ElementId, ElementInputHandler, Entity,
    EntityInputHandler, FocusHandle, Focusable, GlobalElementId, Hsla, LayoutId, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, Point, ScrollWheelEvent,
    ShapedLine, SharedString, Style, TextRun, UTF16Selection, Window, actions, fill, font, point,
    prelude::*, px, relative, size,
};

use crate::theme;

actions!(
    editor,
    [
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
    ]
);

const FONT_SIZE: f32 = 13.5;
const LINE_HEIGHT: f32 = 20.;
const GUTTER_PADDING: f32 = 12.;
const TEXT_PADDING: f32 = 8.;

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
    is_selecting: bool,
    marked_range: Option<Range<usize>>,
    layout: Option<LayoutInfo>,
}

impl EditorView {
    pub fn new(buffer: Buffer, cx: &mut Context<Self>) -> Self {
        Self {
            buffer,
            focus_handle: cx.focus_handle(),
            scroll_y: px(0.),
            autoscroll: false,
            is_selecting: false,
            marked_range: None,
            layout: None,
        }
    }

    fn line_height() -> Pixels {
        px(LINE_HEIGHT)
    }

    fn changed(&mut self, cx: &mut Context<Self>) {
        self.autoscroll = true;
        cx.notify();
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
        self.changed(cx);
    }
    fn move_right(&mut self, _: &MoveRight, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.move_right(false);
        self.changed(cx);
    }
    fn move_up(&mut self, _: &MoveUp, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.move_vertical(-1, false);
        self.changed(cx);
    }
    fn move_down(&mut self, _: &MoveDown, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.move_vertical(1, false);
        self.changed(cx);
    }
    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.move_left(true);
        self.changed(cx);
    }
    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.move_right(true);
        self.changed(cx);
    }
    fn select_up(&mut self, _: &SelectUp, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.move_vertical(-1, true);
        self.changed(cx);
    }
    fn select_down(&mut self, _: &SelectDown, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.move_vertical(1, true);
        self.changed(cx);
    }
    fn move_home(&mut self, _: &MoveHome, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.move_home(false);
        self.changed(cx);
    }
    fn move_end(&mut self, _: &MoveEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.move_end(false);
        self.changed(cx);
    }
    fn select_home(&mut self, _: &SelectHome, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.move_home(true);
        self.changed(cx);
    }
    fn select_end(&mut self, _: &SelectEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.move_end(true);
        self.changed(cx);
    }
    fn page_up(&mut self, _: &PageUp, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.move_vertical(-self.visible_lines(), false);
        self.changed(cx);
    }
    fn page_down(&mut self, _: &PageDown, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.move_vertical(self.visible_lines(), false);
        self.changed(cx);
    }
    fn doc_start(&mut self, _: &DocStart, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.move_to(0, false);
        self.changed(cx);
    }
    fn doc_end(&mut self, _: &DocEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.move_to(self.buffer.len_chars(), false);
        self.changed(cx);
    }
    fn backspace(&mut self, _: &Backspace, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.backspace();
        self.changed(cx);
    }
    fn delete(&mut self, _: &Delete, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.delete();
        self.changed(cx);
    }
    fn newline(&mut self, _: &Newline, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.newline();
        self.changed(cx);
    }
    fn tab(&mut self, _: &Tab, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.tab();
        self.changed(cx);
    }
    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.select_all();
        cx.notify();
    }
    fn undo(&mut self, _: &Undo, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.undo();
        self.changed(cx);
    }
    fn redo(&mut self, _: &Redo, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.redo();
        self.changed(cx);
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
        if let Err(err) = self.buffer.save() {
            eprintln!("save failed: {err:#}");
        }
        cx.notify();
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

    fn on_mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus_handle);
        if let Some(ix) = self.char_for_position(event.position) {
            if event.click_count >= 2 {
                self.select_word_at(ix);
            } else {
                self.buffer.move_to(ix, event.modifiers.shift);
                self.is_selecting = true;
            }
            cx.notify();
        }
    }

    fn select_word_at(&mut self, ix: usize) {
        let rope = self.buffer.rope();
        let is_word = |c: char| c.is_alphanumeric() || c == '_' || c == '$';
        let mut start = ix;
        while start > 0 && is_word(rope.char(start - 1)) {
            start -= 1;
        }
        let mut end = ix;
        while end < rope.len_chars() && is_word(rope.char(end)) {
            end += 1;
        }
        self.buffer.selection = Selection { anchor: start, head: end };
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.is_selecting = false;
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
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
        self.buffer.replace(range, new_text);
        self.changed(cx);
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
    text.char_indices().nth(col).map(|(b, _)| b).unwrap_or(text.len())
}

impl Focusable for EditorView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for EditorView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        gpui::div()
            .key_context("Editor")
            .track_focus(&self.focus_handle)
            .size_full()
            .overflow_hidden()
            .cursor(CursorStyle::IBeam)
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
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_scroll_wheel(cx.listener(Self::on_scroll))
            .child(EditorElement { view: cx.entity() })
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
    caret: Option<PaintQuad>,
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
        let font_size = px(FONT_SIZE);
        let run = |len: usize, color: Hsla| TextRun {
            len,
            font: font(theme::EDITOR_FONT),
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
            let gutter_width = digit_width * digits as f32 + px(GUTTER_PADDING * 2.);
            let text_x = bounds.left() + gutter_width + px(TEXT_PADDING);

            // Keep the caret visible after keyboard edits and movement.
            let (caret_line, caret_col) = view.buffer.char_to_line_col(view.buffer.selection.head);
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
                let number_color = if line == caret_line { theme::ui_text() } else { theme::line_number() };
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
                        Bounds::from_corners(point(text_x + x0, y), point(text_x + x1, y + line_height)),
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
                lines.push(PaintLine { number, text, shaped, y });
            }

            let current_line_y = (caret_line >= first_line && caret_line < last_line)
                .then(|| offset_y + line_height * (caret_line - first_line) as f32);
            EditorPrepaint {
                gutter_width,
                lines,
                first_line,
                current_line_y,
                selection: selection_quads,
                caret,
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
        window.handle_input(&focus_handle, ElementInputHandler::new(bounds, self.view.clone()), cx);

        let gutter = prepaint.gutter_width;
        let text_x = bounds.left() + gutter + px(TEXT_PADDING);
        window.paint_quad(fill(
            Bounds::new(bounds.origin, size(gutter, bounds.size.height)),
            theme::gutter_bg(),
        ));
        if let Some(y) = prepaint.current_line_y {
            window.paint_quad(fill(
                Bounds::from_corners(point(bounds.left() + gutter, y), point(bounds.right(), y + line_height)),
                theme::current_line(),
            ));
        }
        for quad in prepaint.selection.drain(..) {
            window.paint_quad(quad);
        }
        for line in &prepaint.lines {
            let number_x = bounds.left() + gutter - px(GUTTER_PADDING) - line.number.width;
            line.number.paint(point(number_x, line.y), line_height, window, cx).ok();
            line.shaped.paint(point(text_x, line.y), line_height, window, cx).ok();
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
            view.layout = Some(LayoutInfo { bounds, text_x, first_line, lines });
        });
    }
}
