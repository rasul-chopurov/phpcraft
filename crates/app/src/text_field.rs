//! Single-line text input used in dialogs.

use std::ops::Range;

use gpui::{
    App, Bounds, ClipboardItem, Context, CursorStyle, ElementId, ElementInputHandler, Entity,
    EntityInputHandler, EventEmitter, FocusHandle, Focusable, GlobalElementId, LayoutId,
    MouseButton, MouseDownEvent, PaintQuad, Pixels, Point, ShapedLine, SharedString, Style,
    TextRun, UTF16Selection, Window, actions, div, fill, font, point, prelude::*, px, relative,
    size,
};

use crate::theme;

actions!(
    text_field,
    [
        Backspace, Delete, Left, Right, Home, End, SelectAll, Paste, Copy
    ]
);

pub enum FieldEvent {
    Changed,
}

pub struct TextField {
    focus_handle: FocusHandle,
    text: String,
    placeholder: SharedString,
    /// Byte offsets; `selected` is a caret when empty.
    selected: Range<usize>,
    marked: Option<Range<usize>>,
    last_line: Option<ShapedLine>,
    last_bounds: Option<Bounds<Pixels>>,
}

impl EventEmitter<FieldEvent> for TextField {}

impl TextField {
    pub fn new(placeholder: impl Into<SharedString>, cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            text: String::new(),
            placeholder: placeholder.into(),
            selected: 0..0,
            marked: None,
            last_line: None,
            last_bounds: None,
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn set_text(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        self.text = text.into();
        self.selected = self.text.len()..self.text.len();
        cx.emit(FieldEvent::Changed);
        cx.notify();
    }

    pub fn set_placeholder(
        &mut self,
        placeholder: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) {
        self.placeholder = placeholder.into();
        cx.notify();
    }

    fn caret(&self) -> usize {
        self.selected.end
    }

    fn prev_boundary(&self, at: usize) -> usize {
        self.text[..at]
            .char_indices()
            .next_back()
            .map_or(0, |(i, _)| i)
    }

    fn next_boundary(&self, at: usize) -> usize {
        self.text[at..]
            .chars()
            .next()
            .map_or(at, |c| at + c.len_utf8())
    }

    fn move_to(&mut self, at: usize, cx: &mut Context<Self>) {
        self.selected = at..at;
        cx.notify();
    }

    fn replace(&mut self, range: Range<usize>, new: &str, cx: &mut Context<Self>) {
        self.text.replace_range(range.clone(), new);
        let caret = range.start + new.len();
        self.selected = caret..caret;
        cx.emit(FieldEvent::Changed);
        cx.notify();
    }

    fn backspace(&mut self, _: &Backspace, _: &mut Window, cx: &mut Context<Self>) {
        let range = if self.selected.is_empty() {
            self.prev_boundary(self.caret())..self.caret()
        } else {
            self.selected.clone()
        };
        self.replace(range, "", cx);
    }

    fn delete(&mut self, _: &Delete, _: &mut Window, cx: &mut Context<Self>) {
        let range = if self.selected.is_empty() {
            self.caret()..self.next_boundary(self.caret())
        } else {
            self.selected.clone()
        };
        self.replace(range, "", cx);
    }

    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        let at = if self.selected.is_empty() {
            self.prev_boundary(self.caret())
        } else {
            self.selected.start
        };
        self.move_to(at, cx);
    }

    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        let at = if self.selected.is_empty() {
            self.next_boundary(self.caret())
        } else {
            self.selected.end
        };
        self.move_to(at, cx);
    }

    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
    }

    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.text.len(), cx);
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.selected = 0..self.text.len();
        cx.notify();
    }

    fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.replace(self.selected.clone(), &text.replace('\n', " "), cx);
        }
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selected.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.text[self.selected.clone()].to_string(),
            ));
        }
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus_handle);
        let at = match (&self.last_line, &self.last_bounds) {
            (Some(line), Some(bounds)) => {
                line.closest_index_for_x(event.position.x - bounds.left())
            }
            _ => self.text.len(),
        };
        self.move_to(at.min(self.text.len()), cx);
    }

    fn to_utf16(&self, offset: usize) -> usize {
        self.text[..offset.min(self.text.len())]
            .encode_utf16()
            .count()
    }

    fn from_utf16(&self, offset: usize) -> usize {
        let mut count = 0;
        for (i, c) in self.text.char_indices() {
            if count >= offset {
                return i;
            }
            count += c.len_utf16();
        }
        self.text.len()
    }

    fn range_from_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.from_utf16(range.start)..self.from_utf16(range.end)
    }
}

impl EntityInputHandler for TextField {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        actual: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range);
        actual.replace(self.to_utf16(range.start)..self.to_utf16(range.end));
        Some(self.text[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.to_utf16(self.selected.start)..self.to_utf16(self.selected.end),
            reversed: false,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked
            .as_ref()
            .map(|r| self.to_utf16(r.start)..self.to_utf16(r.end))
    }

    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {
        self.marked = None;
    }

    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range
            .map(|r| self.range_from_utf16(&r))
            .or(self.marked.take())
            .unwrap_or(self.selected.clone());
        self.replace(range, text, cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        _: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range
            .map(|r| self.range_from_utf16(&r))
            .or(self.marked.clone())
            .unwrap_or(self.selected.clone());
        self.replace(range.clone(), text, cx);
        self.marked = (!text.is_empty()).then(|| range.start..range.start + text.len());
    }

    fn bounds_for_range(
        &mut self,
        range: Range<usize>,
        bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let line = self.last_line.as_ref()?;
        let start = self.from_utf16(range.start);
        Some(Bounds::new(
            point(bounds.left() + line.x_for_index(start), bounds.top()),
            size(px(2.), bounds.size.height),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        let bounds = self.last_bounds?;
        let line = self.last_line.as_ref()?;
        Some(self.to_utf16(line.closest_index_for_x(point.x - bounds.left())))
    }
}

impl Focusable for TextField {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for TextField {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = self.focus_handle.is_focused(window);
        div()
            .key_context("TextField")
            .track_focus(&self.focus_handle)
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::copy))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .w_full()
            .h(px(28.))
            .px_2()
            .flex()
            .items_center()
            .bg(theme::editor_bg())
            .border_1()
            .rounded_sm()
            .border_color(if focused {
                theme::accent()
            } else {
                theme::border_strong()
            })
            .child(FieldElement { field: cx.entity() })
    }
}

struct FieldElement {
    field: Entity<TextField>,
}

impl IntoElement for FieldElement {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

pub struct FieldPrepaint {
    line: ShapedLine,
    caret: Option<PaintQuad>,
    selection: Option<PaintQuad>,
}

impl Element for FieldElement {
    type RequestLayoutState = ();
    type PrepaintState = FieldPrepaint;

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
        style.size.height = px(18.).into();
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
    ) -> FieldPrepaint {
        let field = self.field.read(cx);
        let (text, color): (SharedString, _) = if field.text.is_empty() {
            (field.placeholder.clone(), theme::muted())
        } else {
            (field.text.clone().into(), theme::text())
        };
        let run = TextRun {
            len: text.len(),
            font: font(theme::editor_font()),
            color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let line = window.text_system().shape_line(text, px(13.), &[run], None);
        let x = |i: usize| {
            if field.text.is_empty() {
                px(0.)
            } else {
                line.x_for_index(i)
            }
        };
        let caret = field.selected.is_empty().then(|| {
            fill(
                Bounds::new(
                    point(bounds.left() + x(field.caret()), bounds.top()),
                    size(px(1.5), bounds.size.height),
                ),
                theme::caret(),
            )
        });
        let selection = (!field.selected.is_empty()).then(|| {
            fill(
                Bounds::from_corners(
                    point(bounds.left() + x(field.selected.start), bounds.top()),
                    point(bounds.left() + x(field.selected.end), bounds.bottom()),
                ),
                theme::selection(),
            )
        });
        FieldPrepaint {
            line,
            caret,
            selection,
        }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        prepaint: &mut FieldPrepaint,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus = self.field.read(cx).focus_handle.clone();
        window.handle_input(
            &focus,
            ElementInputHandler::new(bounds, self.field.clone()),
            cx,
        );
        if let Some(selection) = prepaint.selection.take() {
            window.paint_quad(selection);
        }
        prepaint
            .line
            .paint(bounds.origin, bounds.size.height, window, cx)
            .ok();
        if focus.is_focused(window)
            && let Some(caret) = prepaint.caret.take()
        {
            window.paint_quad(caret);
        }
        let line = prepaint.line.clone();
        let empty = self.field.read(cx).text.is_empty();
        self.field.update(cx, |field, _| {
            field.last_line = (!empty).then_some(line);
            field.last_bounds = Some(bounds);
        });
    }
}
