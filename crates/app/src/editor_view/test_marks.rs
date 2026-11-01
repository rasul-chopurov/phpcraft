//! Gutter marks left of the line numbers: ▶ in test files runs that test (or the class);
//! ↑ / ↓ on methods that override or are overridden jump through the hierarchy.

use gpui::{Context, Window};

use super::{EditorView, GoToImplementation, GoToSuperMethod};

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

    /// Methods with an inheritance mark: (line, name byte offset, overrides, overridden).
    pub(super) fn inheritance_lines(&mut self) -> Vec<(usize, usize, bool, bool)> {
        let version = self.buffer.version();
        if self.inheritance.as_ref().is_none_or(|(v, _)| *v != version) {
            let marks = match self.path() {
                Some(path) if self.buffer.is_php() && !self.plain && self.intel.is_ready() => {
                    self.intel.with_doc(&path, &self.buffer.text(), |doc, index| php_analysis::inheritance_marks(doc, index)).unwrap_or_default()
                }
                _ => Vec::new(),
            };
            let lines = marks
                .into_iter()
                .map(|m| (self.buffer.char_to_line_col(self.buffer.byte_to_char(m.range.start)).0, m.range.start, m.overrides, m.overridden))
                .collect();
            let version = if self.intel.is_ready() { version } else { u64::MAX };
            self.inheritance = Some((version, lines));
        }
        self.inheritance.as_ref().map(|(_, l)| l.clone()).unwrap_or_default()
    }

    /// A click on ↑ / ↓: the overridden method, or the overriding ones.
    pub(super) fn follow_inheritance(&mut self, line: usize, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some((_, at, overrides, _)) = self.inheritance_lines().into_iter().find(|(l, ..)| *l == line) else { return false };
        let caret = self.buffer.byte_to_char(at);
        self.buffer.move_to(caret, false);
        if overrides {
            self.go_to_super_method(&GoToSuperMethod, window, cx);
        } else {
            self.go_to_implementation(&GoToImplementation, window, cx);
        }
        true
    }
}
