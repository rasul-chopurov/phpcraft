//! Navigate ▸ Implementation(s) (⌘⌥B), Super Method (⌘U), Test (⌘⇧T) and Type Hierarchy
//! (⌃H): one target opens directly, several show a menu at the caret.

use gpui::{Context, Window};
use php_index::Location;

use super::intel_ui::{MenuAction, MenuItem};
use super::{EditorView, GoToImplementation, GoToSuperMethod, GoToTest, TypeHierarchy};

impl EditorView {
    fn show_targets(&mut self, targets: Vec<(String, Location)>, window: &mut Window, cx: &mut Context<Self>) {
        match targets.len() {
            0 => {}
            1 => self.navigate(targets.into_iter().next().unwrap().1, window, cx),
            _ => {
                let items = targets.into_iter().map(|(label, loc)| MenuItem::new(label, "", "→", MenuAction::Navigate(loc))).collect();
                self.show_menu(items, false, cx);
            }
        }
    }

    fn with_hierarchy<T: Default>(&self, f: impl FnOnce(&php_analysis::Document, &php_index::Index, usize) -> T) -> T {
        let Some(path) = self.path() else { return T::default() };
        let at = self.caret_byte();
        self.intel.with_doc(&path, &self.buffer.text(), |doc, index| f(doc, index, at)).unwrap_or_default()
    }

    pub(super) fn go_to_implementation(&mut self, _: &GoToImplementation, window: &mut Window, cx: &mut Context<Self>) {
        let targets = self.with_hierarchy(php_analysis::implementations);
        self.show_targets(targets, window, cx);
    }

    pub(super) fn go_to_super_method(&mut self, _: &GoToSuperMethod, window: &mut Window, cx: &mut Context<Self>) {
        let target = self.with_hierarchy(|d, i, a| php_analysis::super_declaration(d, i, a));
        if let Some(location) = target {
            self.navigate(location, window, cx);
        }
    }

    pub(super) fn go_to_test(&mut self, _: &GoToTest, window: &mut Window, cx: &mut Context<Self>) {
        let targets = self.with_hierarchy(php_analysis::related_tests);
        self.show_targets(targets, window, cx);
    }

    pub(super) fn type_hierarchy(&mut self, _: &TypeHierarchy, _: &mut Window, cx: &mut Context<Self>) {
        let rows = self.with_hierarchy(php_analysis::type_hierarchy);
        let items = rows
            .into_iter()
            .map(|(depth, name, loc)| MenuItem::new(format!("{}{name}", "  ".repeat(depth)), "", "C", MenuAction::Navigate(loc)))
            .collect();
        self.show_menu(items, false, cx);
    }
}
