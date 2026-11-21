//! Breakpoints in the editor (⌘F8 / Ctrl+F8, or a click on the line number) and the
//! execution point while debugging. Lines are anchors, so breakpoints follow edits.

use gpui::{Context, Window};

use super::{EditorView, ToggleBreakpoint};

impl EditorView {
    pub(super) fn load_breakpoints(&mut self) {
        let Some(path) = self.buffer.path().map(|p| p.to_path_buf()) else { return };
        for (line, condition) in crate::breakpoints::for_file(&path) {
            if line < self.buffer.len_lines() {
                let id = self.buffer.add_anchor(self.buffer.line_to_char(line));
                self.breakpoints.push((id, condition));
            }
        }
    }

    /// Re-reads the stored breakpoints after another surface (the Debug window) changed
    /// them; the anchors of the old ones go away, so lines follow the same store again.
    pub fn refresh_breakpoints(&mut self, cx: &mut Context<Self>) {
        for (id, _) in std::mem::take(&mut self.breakpoints) {
            self.buffer.remove_anchor(id);
        }
        self.load_breakpoints();
        cx.notify();
    }

    /// Breakpoint lines, sorted.
    pub(super) fn breakpoint_lines(&self) -> Vec<usize> {
        let mut lines: Vec<usize> = self.breakpoints.iter().filter_map(|(id, _)| Some(self.buffer.char_to_line_col(self.buffer.anchor(*id)?).0)).collect();
        lines.sort();
        lines.dedup();
        lines
    }

    /// Lines whose breakpoint carries a condition: drawn as a ring instead of a dot.
    pub(super) fn conditional_breakpoint_lines(&self) -> Vec<usize> {
        self.breakpoints
            .iter()
            .filter(|(_, condition)| condition.is_some())
            .filter_map(|(id, _)| self.buffer.anchor(*id).map(|a| self.buffer.char_to_line_col(a).0))
            .collect()
    }

    fn save_breakpoints(&self) {
        let Some(path) = self.buffer.path() else { return };
        let lines: Vec<(usize, Option<String>)> = self
            .breakpoints
            .iter()
            .filter_map(|(id, c)| Some((self.buffer.char_to_line_col(self.buffer.anchor(*id)?).0, c.clone())))
            .collect();
        crate::breakpoints::set_file(path, &lines);
    }

    pub(super) fn toggle_breakpoint_on_line(&mut self, line: usize, cx: &mut Context<Self>) {
        let existing = self.breakpoints.iter().position(|(id, _)| self.buffer.anchor(*id).is_some_and(|c| self.buffer.char_to_line_col(c).0 == line));
        match existing {
            Some(ix) => {
                let (id, _) = self.breakpoints.remove(ix);
                self.buffer.remove_anchor(id);
            }
            None => {
                let id = self.buffer.add_anchor(self.buffer.line_to_char(line));
                self.breakpoints.push((id, None));
            }
        }
        self.save_breakpoints();
        // A running debug session picks the change up.
        self.workspace.update(cx, |ws, _| ws.breakpoints_changed()).ok();
        cx.notify();
    }

    pub(super) fn toggle_breakpoint(&mut self, _: &ToggleBreakpoint, _: &mut Window, cx: &mut Context<Self>) {
        let line = self.buffer.char_to_line_col(self.buffer.selection.head).0;
        self.toggle_breakpoint_on_line(line, cx);
    }

    /// Shows (or clears) where the debugger stopped.
    pub fn set_execution_line(&mut self, line: Option<usize>, cx: &mut Context<Self>) {
        self.execution_line = line;
        if let Some(line) = line {
            let at = self.buffer.line_to_char(line.min(self.buffer.len_lines().saturating_sub(1)));
            self.reveal_char(at, cx);
        }
        cx.notify();
    }
}
