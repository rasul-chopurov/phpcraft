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
        if !self.buffer.is_php() {
            return;
        }
        let settings = crate::config::Settings::load_cached();
        let root = self.intel.root().to_path_buf();
        let Some(path) = self.path() else { return };
        if settings.reformat_on_save
            && let Some(formatter) = crate::quality::formatter(&root)
        {
            self.run_formatter(root.clone(), formatter, path.clone(), cx);
        }
        self.run_analysers(root, path, cx);
    }

    /// PHPStan / Psalm / PHPCS on the saved file, in the background; their findings show with
    /// the inspections until the text changes.
    fn run_analysers(&mut self, root: std::path::PathBuf, path: std::path::PathBuf, cx: &mut Context<Self>) {
        let settings = crate::config::Settings::load_cached();
        if !crate::quality::Analyser::ALL.iter().any(|a| a.enabled(&settings)) {
            return;
        }
        let version = self.buffer.version();
        cx.spawn(async move |this, cx| {
            let found = cx.background_executor().spawn(async move { crate::quality::analyse(&root, &path, &settings) }).await;
            this.update(cx, |this, cx| {
                if this.buffer.version() != version {
                    return;
                }
                let problems = found
                    .into_iter()
                    .filter(|(_, f)| f.line >= 1 && f.line <= this.buffer.len_lines())
                    .map(|(tool, f)| {
                        let line = f.line - 1;
                        let text = this.buffer.line(line);
                        let start = this.buffer.line_to_byte(line) + (text.len() - text.trim_start().len());
                        let end = this.buffer.line_to_byte(line) + text.trim_end().len();
                        php_analysis::Problem {
                            range: start..end.max(start),
                            severity: if f.minor { php_analysis::Severity::Weak } else { php_analysis::Severity::Warning },
                            message: format!("{}: {}", tool.name(), f.message),
                            id: "external",
                        }
                    })
                    .collect();
                this.external_problems = Some((version, problems));
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}
