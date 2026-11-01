//! AI inline completion ("ghost text"): after a pause in typing at the end of a line, asks
//! Claude for the rest and shows it greyed out after the caret. Tab accepts, Esc dismisses;
//! any edit or caret move hides it.

use std::time::Duration;

use gpui::{AppContext, Context, Window};

use super::{AcceptGhost, DismissGhost, EditorView};

const DELAY: Duration = Duration::from_millis(450);

pub(super) struct Ghost {
    /// Valid only for this buffer version and caret position.
    version: u64,
    caret: usize,
    pub text: String,
}

impl EditorView {
    /// The suggestion, if it still matches the buffer and caret.
    pub(super) fn ghost(&self) -> Option<&Ghost> {
        let ghost = self.ghost.as_ref()?;
        let selection = &self.buffer.selection;
        (ghost.version == self.buffer.version() && selection.head == ghost.caret && selection.anchor == selection.head && !ghost.text.is_empty())
            .then_some(ghost)
    }

    /// Restarts the pause timer after typing.
    pub(super) fn schedule_ghost(&mut self, cx: &mut Context<Self>) {
        self.ghost = None;
        self.ghost_task = None;
        if self.plain || !crate::config::Settings::load_cached().ai_inline_completion {
            return;
        }
        let config = crate::workspace::load_agent_config();
        if config.resolved_key().is_none() {
            return;
        }
        self.ghost_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(DELAY).await;
            let Ok(Some((version, caret, path, prefix, suffix))) = this.update(cx, |editor, _| editor.ghost_context()) else { return };
            let result = cx
                .background_spawn(async move {
                    let request = agent::complete::CompletionRequest { path: &path, prefix: &prefix, suffix: &suffix };
                    agent::complete::complete(&config, &request)
                })
                .await;
            let _ = this.update(cx, |editor, cx| match result {
                Ok(text) if editor.buffer.version() == version && editor.buffer.selection.head == caret => {
                    editor.ghost = Some(Ghost { version, caret, text });
                    cx.notify();
                }
                Ok(_) => {}
                Err(err) => eprintln!("inline completion: {err}"),
            });
        }));
    }

    /// `(version, caret, path, prefix, suffix)` when the caret is at the end of a line.
    fn ghost_context(&self) -> Option<(u64, usize, String, String, String)> {
        let selection = &self.buffer.selection;
        if selection.anchor != selection.head || self.menu_open() {
            return None;
        }
        let caret = selection.head;
        let (line, _) = self.buffer.char_to_line_col(caret);
        let line_end = self.buffer.line_to_char(line + 1).min(self.buffer.len_chars());
        if !self.buffer.slice(caret..line_end).trim().is_empty() {
            return None;
        }
        let text = self.buffer.text();
        let byte = self.buffer.char_to_byte(caret);
        let path = self.buffer.path().map(|p| p.display().to_string()).unwrap_or_else(|| "untitled".into());
        Some((self.buffer.version(), caret, path, text[..byte].to_string(), text[byte..].to_string()))
    }

    pub(super) fn accept_ghost(&mut self, _: &AcceptGhost, _: &mut Window, cx: &mut Context<Self>) {
        let Some(text) = self.ghost().map(|g| g.text.clone()) else {
            cx.propagate();
            return;
        };
        let caret = self.buffer.selection.head;
        self.buffer.replace(caret..caret, &text);
        self.ghost = None;
        self.changed(cx);
    }

    pub(super) fn dismiss_ghost(&mut self, _: &DismissGhost, _: &mut Window, cx: &mut Context<Self>) {
        if self.ghost.take().is_none() {
            cx.propagate();
        }
        cx.notify();
    }
}
