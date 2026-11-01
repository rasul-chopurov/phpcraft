//! Code ▸ Reformat Code (⌘⌥L) with the project's formatter (Laravel Pint, PHP CS Fixer), or
//! by re-indenting when there is none; Auto-Indent Lines (⌃⌥I); and the actions on ⌘S.

use gpui::{Context, Window};

use super::{AutoIndentLines, EditorView, ReformatCode};

impl EditorView {
    pub(super) fn reformat_code(&mut self, _: &ReformatCode, _: &mut Window, cx: &mut Context<Self>) {
        let root = self.intel.root().to_path_buf();
        match (self.path(), crate::quality::formatter(&root)) {
            (Some(path), Some(formatter)) if self.buffer.is_php() => {
                self.save_now(cx);
                self.run_formatter(root, formatter, path, cx);
            }
            _ => {
                let whole = self.buffer.selection.is_empty();
                self.buffer.reindent_lines(whole);
                self.changed(cx);
            }
        }
    }

    pub(super) fn auto_indent_lines(&mut self, _: &AutoIndentLines, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.reindent_lines(false);
        self.changed(cx);
    }

    /// Formats the saved file in the background, then reloads it (unless edited meanwhile).
    fn run_formatter(&mut self, root: std::path::PathBuf, formatter: crate::quality::Formatter, path: std::path::PathBuf, cx: &mut Context<Self>) {
        let name = formatter.name();
        cx.spawn(async move |this, cx| {
            let file = path.clone();
            let result = cx.background_executor().spawn(async move { crate::quality::format_file(&root, &formatter, &file) }).await;
            this.update(cx, |this, cx| match result {
                Ok(()) => this.reload_if_clean(&path, cx),
                Err(err) => {
                    let first = err.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim().to_string();
                    this.info = Some(super::info::Info::Message(format!("{name} failed: {first}")));
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Settings ▸ Actions on save, before writing (optimize imports) and after (formatter).
    pub(super) fn before_save(&mut self) {
        if self.buffer.is_php() && crate::config::Settings::load_cached().optimize_imports_on_save {
            let Some(path) = self.path() else { return };
            let text = self.buffer.text();
            if let Some(Some(edit)) = self.intel.with_doc(&path, &text, |doc, _| php_analysis::optimize_imports(doc)) {
                let caret = self.caret_byte();
                self.buffer.apply_edits(vec![(edit.range, edit.text)], caret);
            }
        }
    }

    pub(super) fn after_save(&mut self, cx: &mut Context<Self>) {
        if !self.buffer.is_php() || !crate::config::Settings::load_cached().reformat_on_save {
            return;
        }
        let root = self.intel.root().to_path_buf();
        if let (Some(path), Some(formatter)) = (self.path(), crate::quality::formatter(&root)) {
            self.run_formatter(root, formatter, path, cx);
        }
    }
}
