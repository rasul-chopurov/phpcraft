//! A terminal tab: paints a [`terminal::Terminal`] grid and sends keys to its PTY.

use std::collections::HashMap;
use std::path::Path;

use gpui::{
    App, AppContext, Bounds, ClipboardItem, Context, Entity, EventEmitter, FocusHandle, Focusable, FontStyle, FontWeight, Hsla, KeyDownEvent,
    MouseButton, MouseDownEvent, MouseMoveEvent, Pixels, Point, ScrollWheelEvent, SharedString, StrikethroughStyle,
    TextRun, UnderlineStyle, Window, canvas, div, fill, font, point, prelude::*, px, size,
};
use terminal::{CellView, Palette, Snapshot, Terminal, TerminalEvent};

use crate::theme;


fn line_height() -> f32 {
    (theme::editor_font_size() * 1.36).round()
}

pub enum TerminalViewEvent {
    TitleChanged,
    Exited,
}

impl EventEmitter<TerminalViewEvent> for TerminalView {}

pub struct TerminalView {
    terminal: Terminal,
    pub title: String,
    pub exit_code: Option<i32>,
    /// Shown above the output of a run configuration.
    command_line: Option<String>,
    focus_handle: FocusHandle,
    cell_width: Pixels,
    origin: Point<Pixels>,
    /// Selection in viewport cells: anchor and head (row, column).
    selection: Option<((usize, usize), (usize, usize))>,
    selecting: bool,
    _events: gpui::Task<()>,
}

fn rgb(c: terminal::Rgb) -> Hsla {
    gpui::Rgba { r: c.r as f32 / 255., g: c.g as f32 / 255., b: c.b as f32 / 255., a: 1. }.into()
}

fn palette() -> Palette {
    let bg = theme::editor_bg();
    let mut palette = if bg.l > 0.5 { Palette::light() } else { Palette::dark() };
    let to_rgb = |c: Hsla| {
        let c: gpui::Rgba = c.into();
        terminal::Rgb { r: (c.r * 255.) as u8, g: (c.g * 255.) as u8, b: (c.b * 255.) as u8 }
    };
    palette.foreground = to_rgb(theme::text());
    palette.background = to_rgb(bg);
    palette
}

impl TerminalView {
    /// Starts a shell in `cwd`, or `command` when running a configuration.
    pub fn spawn(
        cwd: &Path,
        command: Option<terminal::Command>,
        title: String,
        env: HashMap<String, String>,
        cx: &mut App,
    ) -> std::io::Result<Entity<Self>> {
        let command_line = command.as_ref().map(|c| {
            std::iter::once(c.program.as_str()).chain(c.args.iter().map(String::as_str)).collect::<Vec<_>>().join(" ")
        });
        let (terminal, events) = Terminal::spawn(cwd, command, env, 120, 24)?;
        Ok(cx.new(|cx| Self::new(terminal, events, title, command_line, cx)))
    }

    fn new(
        terminal: Terminal,
        events: async_channel::Receiver<TerminalEvent>,
        title: String,
        command_line: Option<String>,
        cx: &mut Context<Self>,
    ) -> Self {
        let task = cx.spawn(async move |this, cx| {
            while let Ok(event) = events.recv().await {
                // Coalesce bursts of output into one repaint.
                let mut batch = vec![event];
                while let Ok(more) = events.try_recv() {
                    batch.push(more);
                }
                let alive = this
                    .update(cx, |view, cx| {
                        for event in batch {
                            match event {
                                TerminalEvent::Wakeup | TerminalEvent::Bell => view.terminal.flush_replies(),
                                TerminalEvent::Title(title) => {
                                    if view.command_line.is_none() {
                                        view.title = title;
                                        cx.emit(TerminalViewEvent::TitleChanged);
                                    }
                                }
                                TerminalEvent::Exited(code) => {
                                    view.exit_code = Some(code);
                                    cx.emit(TerminalViewEvent::Exited);
                                }
                            }
                        }
                        cx.notify();
                    })
                    .is_ok();
                if !alive {
                    break;
                }
            }
        });
        Self {
            terminal,
            title,
            exit_code: None,
            command_line,
            focus_handle: cx.focus_handle(),
            cell_width: px(8.),
            origin: Point::default(),
            selection: None,
            selecting: false,
            _events: task,
        }
    }

    pub fn is_running(&self) -> bool {
        self.exit_code.is_none()
    }

    /// Ctrl-C to the process.
    pub fn interrupt(&self) {
        self.terminal.write(vec![3u8]);
    }

    fn cell_at(&self, position: Point<Pixels>) -> (usize, usize) {
        let x = (position.x - self.origin.x).max(px(0.));
        let y = (position.y - self.origin.y).max(px(0.));
        ((y / px(line_height())) as usize, (x / self.cell_width) as usize)
    }

    fn selected_text(&self, snapshot: &Snapshot) -> Option<String> {
        let (a, b) = self.selection?;
        let (start, end) = if a <= b { (a, b) } else { (b, a) };
        if start == end {
            return None;
        }
        let mut out = String::new();
        for row in start.0..=end.0.min(snapshot.lines.len().saturating_sub(1)) {
            let line = &snapshot.lines[row];
            let from = if row == start.0 { start.1 } else { 0 };
            let to = if row == end.0 { end.1.min(line.len()) } else { line.len() };
            let text: String = line.get(from..to).unwrap_or(&[]).iter().filter(|c| !c.spacer).map(|c| c.ch).collect();
            out.push_str(text.trim_end());
            if row != end.0 {
                out.push('\n');
            }
        }
        Some(out)
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        let m = &keystroke.modifiers;
        let key = keystroke.key.as_str();
        // ⌘ shortcuts (and Ctrl+Shift on Windows/Linux) stay with the IDE, except copy/paste/clear.
        let shortcut = m.platform || (cfg!(not(target_os = "macos")) && m.control && m.shift);
        if shortcut {
            match key {
                "c" => {
                    let snapshot = self.terminal.snapshot(&palette());
                    if let Some(text) = self.selected_text(&snapshot) {
                        cx.write_to_clipboard(ClipboardItem::new_string(text));
                    }
                    cx.stop_propagation();
                }
                "v" => {
                    if let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) {
                        self.terminal.scroll_to_bottom();
                        self.terminal.paste(&text);
                    }
                    cx.stop_propagation();
                }
                "k" if m.platform => {
                    // Clear screen and scrollback.
                    self.terminal.write(b"\x0c".to_vec());
                    cx.stop_propagation();
                }
                _ => {}
            }
            return;
        }
        if m.alt && key.starts_with('f') && key.len() > 1 {
            return; // ⌥F12 and friends toggle tool windows.
        }
        let snapshot_app_cursor = self.terminal.snapshot(&palette()).app_cursor;
        if let Some(bytes) = terminal::encode_key(key, keystroke.key_char.as_deref(), m.control, m.alt, m.shift, snapshot_app_cursor) {
            self.selection = None;
            self.terminal.scroll_to_bottom();
            self.terminal.write(bytes);
            cx.stop_propagation();
            cx.notify();
        }
    }

    fn on_scroll(&mut self, event: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        let delta = event.delta.pixel_delta(px(line_height())).y;
        let lines = (delta / px(line_height())).round() as i32;
        if lines != 0 {
            self.terminal.scroll(lines);
            cx.notify();
        }
    }

    fn on_mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus_handle);
        let cell = self.cell_at(event.position);
        self.selection = Some((cell, cell));
        self.selecting = true;
        cx.notify();
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.selecting && event.pressed_button == Some(MouseButton::Left) {
            let cell = self.cell_at(event.position);
            if let Some((anchor, _)) = self.selection {
                self.selection = Some((anchor, cell));
                cx.notify();
            }
        }
    }
}

impl Focusable for TerminalView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

struct Row {
    text: String,
    runs: Vec<TextRun>,
    backgrounds: Vec<(usize, usize, Hsla)>,
}

fn build_row(cells: &[CellView], background: Hsla) -> Row {
    let mut text = String::new();
    let mut runs: Vec<TextRun> = Vec::new();
    let mut backgrounds = Vec::new();
    let mut col = 0;
    for cell in cells {
        // The right half of a wide character: a space keeps later cells aligned while the
        // glyph itself overflows into it.
        let ch = if cell.spacer || cell.ch == '\0' { ' ' } else { cell.ch };
        if let Some(bg) = cell.bg {
            let color = rgb(bg);
            if color != background {
                match backgrounds.last_mut() {
                    Some((_, end, c)) if *end == col && *c == color => *end += 1,
                    _ => backgrounds.push((col, col + 1, color)),
                }
            }
        }
        let len = ch.len_utf8();
        text.push(ch);
        let mut f = font(theme::editor_font());
        if cell.bold {
            f.weight = FontWeight::BOLD;
        }
        if cell.italic {
            f.style = FontStyle::Italic;
        }
        let color = rgb(cell.fg);
        let underline = cell.underline.then(|| UnderlineStyle { thickness: px(1.), color: Some(color), wavy: false });
        let strikethrough = cell.strike.then(|| StrikethroughStyle { thickness: px(1.), color: Some(color) });
        match runs.last_mut() {
            Some(run) if run.color == color && run.font == f && run.underline == underline && run.strikethrough == strikethrough => run.len += len,
            _ => runs.push(TextRun { len, font: f, color, background_color: None, underline, strikethrough }),
        }
        col += 1;
    }
    Row { text, runs, backgrounds }
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = palette();
        let background = rgb(palette.background);
        let snapshot = self.terminal.snapshot(&palette);
        let rows: Vec<Row> = snapshot.lines.iter().map(|cells| build_row(cells, background)).collect();
        let cursor = snapshot.cursor.filter(|_| self.exit_code.is_none());
        let focused = self.focus_handle.is_focused(window);
        let selection = self.selection.map(|(a, b)| if a <= b { (a, b) } else { (b, a) }).filter(|(a, b)| a != b);
        let view = cx.entity();
        let status = self.exit_code.map(|code| {
            if code == 0 { "Process finished with exit code 0".to_string() } else { format!("Process finished with exit code {code}") }
        });
        let ok = self.exit_code == Some(0);
        div()
            .key_context("Terminal")
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::on_key_down))
            .on_scroll_wheel(cx.listener(Self::on_scroll))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, _| this.selecting = false))
            .size_full()
            .flex()
            .flex_col()
            .bg(background)
            .cursor_text()
            .when_some(self.command_line.clone(), |d, line| {
                d.child(div().px_2().pt_1().text_size(px(12.)).text_color(theme::muted()).font_family(theme::editor_font()).child(SharedString::from(line)))
            })
            .child(
                canvas(
                    move |bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App| {
                        let probe = TextRun { len: 1, font: font(theme::editor_font()), color: theme::text(), background_color: None, underline: None, strikethrough: None };
                        let cell_width = window.text_system().shape_line("M".into(), px(theme::editor_font_size() - 0.5), &[probe], None).width;
                        let columns = ((bounds.size.width - px(8.)) / cell_width).floor().max(2.) as usize;
                        let lines = (bounds.size.height / px(line_height())).floor().max(1.) as usize;
                        view.update(cx, |view, _| {
                            view.cell_width = cell_width;
                            view.origin = bounds.origin + point(px(4.), px(0.));
                            // The first frames of a new window can lay out at zero size.
                            if bounds.size.width > px(80.) && bounds.size.height > px(line_height() * 2.) {
                                view.terminal.resize(columns, lines);
                            }
                        });
                        let shaped: Vec<_> = rows
                            .into_iter()
                            .map(|row| {
                                let line = window.text_system().shape_line(row.text.into(), px(theme::editor_font_size() - 0.5), &row.runs, Some(cell_width));
                                (line, row.backgrounds)
                            })
                            .collect();
                        (cell_width, shaped)
                    },
                    move |bounds, (cell_width, shaped), window, cx| {
                        let origin = bounds.origin + point(px(4.), px(0.));
                        window.paint_quad(fill(bounds, background));
                        for (row, (line, backgrounds)) in shaped.iter().enumerate() {
                            let y = origin.y + px(line_height()) * row as f32;
                            if y + px(line_height()) > bounds.bottom() + px(1.) {
                                break;
                            }
                            for (start, end, color) in backgrounds {
                                window.paint_quad(fill(
                                    Bounds::new(point(origin.x + cell_width * *start as f32, y), size(cell_width * (end - start) as f32, px(line_height()))),
                                    *color,
                                ));
                            }
                            if let Some(((sr, sc), (er, ec))) = selection
                                && row >= sr
                                && row <= er
                            {
                                let from = if row == sr { sc } else { 0 };
                                let to = if row == er { ec } else { (bounds.size.width / cell_width) as usize };
                                window.paint_quad(fill(
                                    Bounds::new(point(origin.x + cell_width * from as f32, y), size(cell_width * to.saturating_sub(from) as f32, px(line_height()))),
                                    theme::selection(),
                                ));
                            }
                            let _ = line.paint(point(origin.x, y), px(line_height()), window, cx);
                        }
                        if let Some((row, col)) = cursor {
                            let at = point(origin.x + cell_width * col as f32, origin.y + px(line_height()) * row as f32);
                            let caret = Bounds::new(at, size(cell_width, px(line_height())));
                            if focused {
                                window.paint_quad(fill(caret, theme::text().opacity(0.6)));
                            } else {
                                window.paint_quad(gpui::outline(caret, theme::text().opacity(0.6), gpui::BorderStyle::Solid));
                            }
                        }
                    },
                )
                .flex_1()
                .w_full(),
            )
            .when_some(status, |d, status| {
                d.child(
                    div()
                        .px_2()
                        .py_1()
                        .text_size(px(12.))
                        .font_family(theme::editor_font())
                        .text_color(if ok { theme::success() } else { theme::error() })
                        .child(status),
                )
            })
    }
}
