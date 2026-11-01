//! Code ▸ Generate (⌘N / Alt+Insert in PHP files): constructor, getters and setters,
//! Implement / Override Methods, PHPDoc, and the Laravel generators.

use gpui::{Context, Window};

use super::intel_ui::{MenuAction, MenuItem};
use super::{EditorView, Generate};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GenerateKind {
    Constructor,
    Getters,
    Setters,
    GettersAndSetters,
    Implement,
    Override,
    PhpDoc,
    Laravel,
}

impl EditorView {
    pub(super) fn generate(&mut self, _: &Generate, window: &mut Window, cx: &mut Context<Self>) {
        if !self.buffer.is_php() {
            window.dispatch_action(Box::new(crate::workspace::OpenMake), cx);
            return;
        }
        use GenerateKind::*;
        let items = [
            ("Constructor", Constructor),
            ("Getters", Getters),
            ("Setters", Setters),
            ("Getters and Setters", GettersAndSetters),
            ("Implement Methods", Implement),
            ("Override Methods…", Override),
            ("PHPDoc Comment", PhpDoc),
            ("Laravel Code Generation…", Laravel),
        ]
        .into_iter()
        .map(|(label, kind)| MenuItem::new(label, "", "+", MenuAction::Generate(kind)))
        .collect();
        self.show_menu(items, false, cx);
    }

    pub(super) fn run_generate(&mut self, kind: GenerateKind, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.path() else { return };
        let text = self.buffer.text();
        let at = self.caret_byte();
        use GenerateKind::*;
        let edit = match kind {
            Laravel => {
                window.dispatch_action(Box::new(crate::workspace::OpenMake), cx);
                return;
            }
            Override => {
                let methods = self.intel.with_doc(&path, &text, |doc, index| php_analysis::overridable_methods(doc, index, at)).unwrap_or_default();
                let items = methods.into_iter().map(|m| MenuItem::new(m.clone(), "", "m", MenuAction::OverrideMethod(m))).collect();
                self.show_menu(items, false, cx);
                return;
            }
            _ => self.intel.with_doc(&path, &text, |doc, index| match kind {
                Constructor => php_analysis::generate_constructor(doc, index, at),
                Getters => php_analysis::generate_accessors(doc, index, at, true, false),
                Setters => php_analysis::generate_accessors(doc, index, at, false, true),
                GettersAndSetters => php_analysis::generate_accessors(doc, index, at, true, true),
                Implement => php_analysis::implement_methods(doc, index, at),
                _ => php_analysis::generate_phpdoc(doc, at),
            }),
        };
        if let Some(Some(edit)) = edit {
            self.apply_generated(edit, cx);
        }
    }

    pub(super) fn override_method(&mut self, name: &str, cx: &mut Context<Self>) {
        let Some(path) = self.path() else { return };
        let text = self.buffer.text();
        let at = self.caret_byte();
        if let Some(Some(edit)) = self.intel.with_doc(&path, &text, |doc, index| php_analysis::override_method(doc, index, at, name)) {
            self.apply_generated(edit, cx);
        }
    }

    /// Inserts generated code and puts the caret at its start.
    fn apply_generated(&mut self, edit: php_analysis::TextEdit, cx: &mut Context<Self>) {
        let at = edit.range.start + edit.text.len() - edit.text.trim_start().len();
        self.buffer.apply_edits(vec![(edit.range, edit.text)], at);
        let caret = self.buffer.byte_to_char(at);
        self.buffer.move_to(caret, false);
        self.changed(cx);
    }
}
