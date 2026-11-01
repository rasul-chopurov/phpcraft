//! ▶ marks in the gutter of test files: clicking one runs that test (or the whole class).

use gpui::Context;

use super::EditorView;

impl EditorView {
    /// Lines with a ▶ (tests and test classes), cached per buffer version.
    pub(super) fn test_lines(&mut self) -> Vec<usize> {
        let version = self.buffer.version();
        if self.test_marks.as_ref().is_none_or(|(v, _)| *v != version) {
            let lines = match self.path() {
                Some(path) if self.buffer.is_php() && !self.plain => crate::run_configs::test_lines(&path, &self.buffer.text()),
                _ => Vec::new(),
            };
            self.test_marks = Some((version, lines));
        }
        self.test_marks.as_ref().map(|(_, l)| l.clone()).unwrap_or_default()
    }

    /// Runs the test on `line` (from its ▶ mark).
    pub(super) fn run_test_on_line(&mut self, line: usize, cx: &mut Context<Self>) {
        let Some(path) = self.path() else { return };
        let root = self.intel.root().to_path_buf();
        let text = self.buffer.text();
        // Just past the declaration's start, so the method or `it(` above counts.
        let start = self.buffer.line_to_byte(line);
        let offset = start + self.buffer.line(line).len();
        if let Some(config) = crate::run_configs::for_context(&root, &path, &text, offset) {
            self.save_now(cx);
            self.workspace.update(cx, |ws, cx| ws.run_config(config, cx)).ok();
        }
    }
}
