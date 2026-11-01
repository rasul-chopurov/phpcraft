//! Integrated terminal: a shell (or a run configuration's command) on a PTY, emulated by
//! `alacritty_terminal`. UI-agnostic: the app paints [`Snapshot`]s and feeds key bytes back.

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, EventLoopSender, Msg};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{Config, Term, TermMode};
use alacritty_terminal::tty;
use alacritty_terminal::vte::ansi::{Color, NamedColor};
pub use alacritty_terminal::vte::ansi::Rgb;

/// Notifications for the UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalEvent {
    /// New output; repaint.
    Wakeup,
    Title(String),
    /// The process exited with this code (or -1 when unknown).
    Exited(i32),
    Bell,
}

#[derive(Clone)]
struct Listener {
    tx: async_channel::Sender<TerminalEvent>,
    pty_writes: async_channel::Sender<String>,
    exited: Arc<AtomicBool>,
    code: Arc<AtomicI32>,
}

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        let event = match event {
            Event::Wakeup => TerminalEvent::Wakeup,
            Event::Title(title) => TerminalEvent::Title(title),
            Event::Bell => TerminalEvent::Bell,
            Event::PtyWrite(text) => {
                let _ = self.pty_writes.try_send(text);
                return;
            }
            Event::ChildExit(status) => {
                let code = status.code().unwrap_or(-1);
                self.code.store(code, Ordering::SeqCst);
                self.exited.store(true, Ordering::SeqCst);
                TerminalEvent::Exited(code)
            }
            Event::Exit => {
                if self.exited.swap(true, Ordering::SeqCst) {
                    return;
                }
                TerminalEvent::Exited(self.code.load(Ordering::SeqCst))
            }
            _ => return,
        };
        let _ = self.tx.try_send(event);
    }
}

#[derive(Clone, Copy)]
struct Size {
    columns: usize,
    lines: usize,
}

impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.lines
    }
    fn screen_lines(&self) -> usize {
        self.lines
    }
    fn columns(&self) -> usize {
        self.columns
    }
}

fn window_size(columns: usize, lines: usize) -> WindowSize {
    WindowSize { num_lines: lines as u16, num_cols: columns as u16, cell_width: 8, cell_height: 16 }
}

/// What to run: `None` starts the user's login shell.
#[derive(Debug, Clone, Default)]
pub struct Command {
    pub program: String,
    pub args: Vec<String>,
}

pub struct Terminal {
    term: Arc<FairMutex<Term<Listener>>>,
    sender: EventLoopSender,
    size: Size,
    exited: Arc<AtomicBool>,
    code: Arc<AtomicI32>,
    pty_writes: async_channel::Receiver<String>,
}

/// One painted cell.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CellView {
    pub ch: char,
    pub fg: Rgb,
    pub bg: Option<Rgb>,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    /// The second half of a wide character: paint nothing.
    pub spacer: bool,
}

pub struct Snapshot {
    pub lines: Vec<Vec<CellView>>,
    /// Row and column, when the cursor is on screen.
    pub cursor: Option<(usize, usize)>,
    pub display_offset: usize,
    pub history: usize,
    pub app_cursor: bool,
    pub bracketed_paste: bool,
}

impl Terminal {
    /// Starts `command` (or the default shell) in `cwd` with the given grid size.
    /// Returns the terminal and a stream of [`TerminalEvent`]s.
    pub fn spawn(
        cwd: &Path,
        command: Option<Command>,
        env: HashMap<String, String>,
        columns: usize,
        lines: usize,
    ) -> std::io::Result<(Self, async_channel::Receiver<TerminalEvent>)> {
        let size = Size { columns: columns.max(2), lines: lines.max(1) };
        let (tx, rx) = async_channel::unbounded();
        let (pty_tx, pty_rx) = async_channel::unbounded();
        let exited = Arc::new(AtomicBool::new(false));
        let code = Arc::new(AtomicI32::new(-1));
        let listener = Listener { tx, pty_writes: pty_tx, exited: exited.clone(), code: code.clone() };
        let config = Config { scrolling_history: 10_000, ..Config::default() };
        let term = Arc::new(FairMutex::new(Term::new(config, &size, listener.clone())));
        let mut env = env;
        env.entry("TERM".into()).or_insert_with(|| "xterm-256color".into());
        env.entry("COLORTERM".into()).or_insert_with(|| "truecolor".into());
        env.insert("TERM_PROGRAM".into(), "PhpCraft".into());
        let options = tty::Options {
            shell: command.map(|c| tty::Shell::new(c.program, c.args)),
            working_directory: Some(cwd.to_path_buf()),
            drain_on_exit: true,
            env,
            #[cfg(target_os = "windows")]
            escape_args: true,
        };
        let pty = tty::new(&options, window_size(size.columns, size.lines), 0)?;
        let event_loop = EventLoop::new(term.clone(), listener, pty, true, false)?;
        let sender = event_loop.channel();
        event_loop.spawn();
        Ok((Self { term, sender, size, exited, code, pty_writes: pty_rx }, rx))
    }

    pub fn write(&self, bytes: impl Into<Cow<'static, [u8]>>) {
        if !self.has_exited() {
            let _ = self.sender.send(Msg::Input(bytes.into()));
        }
    }

    /// Answers terminal queries (device attributes, cursor position) the emulator produced.
    pub fn flush_replies(&self) {
        while let Ok(text) = self.pty_writes.try_recv() {
            self.write(text.into_bytes());
        }
    }

    /// Pastes text, honouring bracketed paste mode.
    pub fn paste(&self, text: &str) {
        let bracketed = self.term.lock().mode().contains(TermMode::BRACKETED_PASTE);
        let text = text.replace("\r\n", "\r").replace('\n', "\r");
        if bracketed {
            self.write(format!("\x1b[200~{text}\x1b[201~").into_bytes());
        } else {
            self.write(text.into_bytes());
        }
    }

    pub fn resize(&mut self, columns: usize, lines: usize) {
        let size = Size { columns: columns.max(2), lines: lines.max(1) };
        if size.columns == self.size.columns && size.lines == self.size.lines {
            return;
        }
        self.size = size;
        let _ = self.sender.send(Msg::Resize(window_size(size.columns, size.lines)));
        self.term.lock().resize(size);
    }

    pub fn size(&self) -> (usize, usize) {
        (self.size.columns, self.size.lines)
    }

    /// Scrolls the viewport by `lines` (positive = back in history).
    pub fn scroll(&self, lines: i32) {
        self.term.lock().scroll_display(Scroll::Delta(lines));
    }

    pub fn scroll_to_bottom(&self) {
        self.term.lock().scroll_display(Scroll::Bottom);
    }

    pub fn has_exited(&self) -> bool {
        self.exited.load(Ordering::SeqCst)
    }

    pub fn exit_code(&self) -> Option<i32> {
        self.has_exited().then(|| self.code.load(Ordering::SeqCst))
    }

    pub fn shutdown(&self) {
        let _ = self.sender.send(Msg::Shutdown);
    }

    /// All text in the scrollback and on screen.
    pub fn text(&self) -> String {
        let term = self.term.lock();
        let grid = term.grid();
        let top = Point::new(Line(-(grid.history_size() as i32)), Column(0));
        let bottom = Point::new(Line(grid.screen_lines() as i32 - 1), Column(grid.columns() - 1));
        term.bounds_to_string(top, bottom).trim_end().to_string()
    }

    pub fn snapshot(&self, palette: &Palette) -> Snapshot {
        let term = self.term.lock();
        let content = term.renderable_content();
        let colors = content.colors;
        let offset = content.display_offset;
        let (columns, lines) = (term.columns(), term.screen_lines());
        let blank = CellView { ch: ' ', fg: palette.foreground, bg: None, bold: false, italic: false, underline: false, strike: false, spacer: false };
        let mut grid = vec![vec![blank; columns]; lines];
        let resolve = |color: Color| -> Rgb {
            match color {
                Color::Spec(rgb) => rgb,
                Color::Indexed(i) => colors[i as usize].unwrap_or_else(|| palette.indexed(i)),
                Color::Named(named) => colors[named].unwrap_or_else(|| palette.named(named)),
            }
        };
        for indexed in content.display_iter {
            let row = indexed.point.line.0 + offset as i32;
            let col = indexed.point.column.0;
            if row < 0 || row as usize >= lines || col >= columns {
                continue;
            }
            let cell = indexed.cell;
            let mut fg = resolve(cell.fg);
            let bg_color = resolve(cell.bg);
            let mut bg = (cell.bg != Color::Named(NamedColor::Background)).then_some(bg_color);
            if cell.flags.contains(Flags::INVERSE) {
                let new_fg = bg.unwrap_or(palette.background);
                bg = Some(fg);
                fg = new_fg;
            }
            if cell.flags.contains(Flags::DIM) {
                fg = Rgb { r: fg.r / 3 * 2, g: fg.g / 3 * 2, b: fg.b / 3 * 2 };
            }
            grid[row as usize][col] = CellView {
                ch: if cell.flags.contains(Flags::HIDDEN) { ' ' } else { cell.c },
                fg,
                bg,
                bold: cell.flags.contains(Flags::BOLD),
                italic: cell.flags.contains(Flags::ITALIC),
                underline: cell.flags.intersects(Flags::ALL_UNDERLINES),
                strike: cell.flags.contains(Flags::STRIKEOUT),
                spacer: cell.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER),
            };
        }
        let mode = *term.mode();
        let cursor_point = content.cursor.point;
        let cursor_row = cursor_point.line.0 + offset as i32;
        let cursor = (mode.contains(TermMode::SHOW_CURSOR) && cursor_row >= 0 && (cursor_row as usize) < lines)
            .then_some((cursor_row as usize, cursor_point.column.0));
        Snapshot {
            lines: grid,
            cursor,
            display_offset: offset,
            history: term.grid().history_size(),
            app_cursor: mode.contains(TermMode::APP_CURSOR),
            bracketed_paste: mode.contains(TermMode::BRACKETED_PASTE),
        }
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// The 16 ANSI colours plus default foreground/background.
#[derive(Debug, Clone)]
pub struct Palette {
    pub foreground: Rgb,
    pub background: Rgb,
    pub ansi: [Rgb; 16],
}

const fn rgb(hex: u32) -> Rgb {
    Rgb { r: (hex >> 16) as u8, g: (hex >> 8) as u8, b: hex as u8 }
}

impl Palette {
    pub fn dark() -> Self {
        Self {
            foreground: rgb(0xd4d7dd),
            background: rgb(0x1e1f22),
            ansi: [
                rgb(0x1e1f22), rgb(0xf0524f), rgb(0x5c962c), rgb(0xa68a0d), rgb(0x3993d4), rgb(0xa771bf), rgb(0x00a3a3), rgb(0xbbbbbb),
                rgb(0x595b5d), rgb(0xff4050), rgb(0x4fc414), rgb(0xe5bf00), rgb(0x1fb0ff), rgb(0xed7eed), rgb(0x00e5e5), rgb(0xffffff),
            ],
        }
    }

    pub fn light() -> Self {
        Self {
            foreground: rgb(0x1f2328),
            background: rgb(0xffffff),
            ansi: [
                rgb(0x000000), rgb(0xc4262e), rgb(0x2a7f2a), rgb(0x8c6c00), rgb(0x2463c8), rgb(0x8f3fa8), rgb(0x0b7f8a), rgb(0x9a9a9a),
                rgb(0x5c5c5c), rgb(0xe0353d), rgb(0x3a9a3a), rgb(0xa98500), rgb(0x3b7ae0), rgb(0xa555c0), rgb(0x1797a3), rgb(0x1f2328),
            ],
        }
    }

    fn named(&self, named: NamedColor) -> Rgb {
        let ix = named as usize;
        match named {
            NamedColor::Foreground | NamedColor::BrightForeground | NamedColor::Cursor => self.foreground,
            NamedColor::Background => self.background,
            NamedColor::DimForeground => self.foreground,
            _ if ix < 16 => self.ansi[ix],
            // Dim colours follow the normal ones.
            _ if (259..267).contains(&ix) => self.ansi[ix - 259],
            _ => self.foreground,
        }
    }

    /// xterm 256-colour table.
    pub fn indexed(&self, i: u8) -> Rgb {
        match i {
            0..16 => self.ansi[i as usize],
            16..232 => {
                let i = i - 16;
                let level = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
                Rgb { r: level(i / 36), g: level((i / 6) % 6), b: level(i % 6) }
            }
            _ => {
                let v = 8 + (i - 232) * 10;
                Rgb { r: v, g: v, b: v }
            }
        }
    }
}

/// Encodes a key press for the PTY. `key` uses GPUI's names (`enter`, `up`, `a`…);
/// `text` is the character the key produced, if any.
pub fn encode_key(key: &str, text: Option<&str>, ctrl: bool, alt: bool, shift: bool, app_cursor: bool) -> Option<Vec<u8>> {
    let modifier = 1 + u8::from(shift) + 2 * u8::from(alt) + 4 * u8::from(ctrl);
    let csi = |code: &str, letter: char| -> Vec<u8> {
        if modifier > 1 {
            format!("\x1b[1;{modifier}{letter}").into_bytes()
        } else if app_cursor && code.is_empty() {
            format!("\x1bO{letter}").into_bytes()
        } else {
            format!("\x1b[{code}{letter}").into_bytes()
        }
    };
    let tilde = |n: u8| -> Vec<u8> {
        if modifier > 1 { format!("\x1b[{n};{modifier}~").into_bytes() } else { format!("\x1b[{n}~").into_bytes() }
    };
    let bytes = match key {
        "enter" => b"\r".to_vec(),
        "backspace" => if ctrl { vec![0x08] } else if alt { b"\x1b\x7f".to_vec() } else { vec![0x7f] },
        "tab" => if shift { b"\x1b[Z".to_vec() } else { b"\t".to_vec() },
        "escape" => vec![0x1b],
        "up" => csi("", 'A'),
        "down" => csi("", 'B'),
        "right" => csi("", 'C'),
        "left" => csi("", 'D'),
        "home" => csi("", 'H'),
        "end" => csi("", 'F'),
        "insert" => tilde(2),
        "delete" => tilde(3),
        "pageup" => tilde(5),
        "pagedown" => tilde(6),
        "f1" => b"\x1bOP".to_vec(),
        "f2" => b"\x1bOQ".to_vec(),
        "f3" => b"\x1bOR".to_vec(),
        "f4" => b"\x1bOS".to_vec(),
        "f5" => tilde(15),
        "f6" => tilde(17),
        "f7" => tilde(18),
        "f8" => tilde(19),
        "f9" => tilde(20),
        "f10" => tilde(21),
        "f11" => tilde(23),
        "f12" => tilde(24),
        "space" if ctrl => vec![0],
        _ if ctrl && key.len() == 1 => {
            let c = key.as_bytes()[0].to_ascii_lowercase();
            let byte = match c {
                b'a'..=b'z' => c - b'a' + 1,
                b'[' => 0x1b,
                b'\\' => 0x1c,
                b']' => 0x1d,
                b'^' | b'6' => 0x1e,
                b'_' | b'-' => 0x1f,
                b'@' | b'2' => 0,
                _ => return None,
            };
            if alt { vec![0x1b, byte] } else { vec![byte] }
        }
        _ => {
            let text = text.or((key == "space").then_some(" "))?;
            if text.is_empty() {
                return None;
            }
            let mut bytes = if alt { vec![0x1b] } else { Vec::new() };
            bytes.extend_from_slice(text.as_bytes());
            bytes
        }
    };
    Some(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_keys() {
        assert_eq!(encode_key("enter", None, false, false, false, false).unwrap(), b"\r");
        assert_eq!(encode_key("c", Some("c"), true, false, false, false).unwrap(), [3]);
        assert_eq!(encode_key("up", None, false, false, false, false).unwrap(), b"\x1b[A");
        assert_eq!(encode_key("up", None, false, false, false, true).unwrap(), b"\x1bOA");
        assert_eq!(encode_key("left", None, true, false, false, false).unwrap(), b"\x1b[1;5D");
        assert_eq!(encode_key("delete", None, false, false, false, false).unwrap(), b"\x1b[3~");
        assert_eq!(encode_key("b", Some("b"), false, true, false, false).unwrap(), b"\x1bb");
        assert_eq!(encode_key("a", Some("ä"), false, false, false, false).unwrap(), "ä".as_bytes());
        assert!(encode_key("shift", None, false, false, true, false).is_none());
    }

    #[test]
    fn palette_256() {
        let p = Palette::dark();
        assert_eq!(p.indexed(16), Rgb { r: 0, g: 0, b: 0 });
        assert_eq!(p.indexed(231), Rgb { r: 255, g: 255, b: 255 });
        assert_eq!(p.indexed(232), Rgb { r: 8, g: 8, b: 8 });
    }

    #[cfg(unix)]
    #[test]
    fn runs_a_command_and_captures_output() {
        let (terminal, events) = Terminal::spawn(
            Path::new("."),
            Some(Command { program: "/bin/sh".into(), args: vec!["-c".into(), "printf 'hello\\033[31mred'; exit 3".into()] }),
            HashMap::new(),
            40,
            5,
        )
        .unwrap();
        let mut code = None;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while code.is_none() && std::time::Instant::now() < deadline {
            match events.recv_blocking() {
                Ok(TerminalEvent::Exited(c)) => code = Some(c),
                Ok(_) => {}
                Err(_) => break,
            }
        }
        assert_eq!(code, Some(3));
        assert!(terminal.text().contains("hellored"), "{}", terminal.text());
        let snap = terminal.snapshot(&Palette::dark());
        let row = &snap.lines[0];
        assert_eq!(row[5].ch, 'r');
        assert_eq!(row[5].fg, Palette::dark().ansi[1]);
    }
}
