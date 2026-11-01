//! A small dialog asking for a name (New File, New Directory, Rename) or a confirmation
//! (Delete).

use std::path::PathBuf;

use gpui::{App, Context, Entity, EventEmitter, FocusHandle, Focusable, Window, div, prelude::*, px};

use super::{Cancel, Confirm, DialogEvent, button, card};
use crate::text_field::TextField;
use crate::theme;

#[derive(Debug, Clone)]
pub enum FileOp {
    NewFile(PathBuf),
    NewDirectory(PathBuf),
    Rename(PathBuf),
    /// Move to the Trash.
    Delete(PathBuf),
    /// New ▸ <template> in a folder.
    FromTemplate(PathBuf, crate::templates::Template),
    /// Refactor ▸ Rename of a PHP symbol.
    RenameSymbol(php_analysis::Symbol),
    /// Tools ▸ Composer ▸ Require… / Remove… (the name is `vendor/package [constraint]`).
    ComposerRequire,
    ComposerRemove,
    /// Run ▸ PHP Interpreter… (the current setting).
    PhpInterpreter(String),
    /// Refactor ▸ Extract Blade Component / Extract View of a selection (byte range).
    ExtractBlade { component: bool, path: PathBuf, range: std::ops::Range<usize>, text: String },
    /// Git ▸ Compare with Branch / Revision… for a file.
    CompareRevision(PathBuf),
}

pub struct InputDialog {
    op: FileOp,
    field: Option<Entity<TextField>>,
    focus_handle: FocusHandle,
}

impl EventEmitter<DialogEvent> for InputDialog {}

impl InputDialog {
    pub fn new(op: FileOp, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let initial = match &op {
            FileOp::Rename(path) => Some(path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()),
            FileOp::NewFile(_) | FileOp::NewDirectory(_) | FileOp::FromTemplate(..) => Some(String::new()),
            FileOp::Delete(_) => None,
            FileOp::RenameSymbol(symbol) => Some(symbol.name.clone()),
            FileOp::ComposerRequire | FileOp::ComposerRemove => Some(String::new()),
            FileOp::PhpInterpreter(current) => Some(current.clone()),
            FileOp::ExtractBlade { .. } => Some(String::new()),
            FileOp::CompareRevision(_) => Some("main".into()),
        };
        let focus_handle = cx.focus_handle();
        let field = initial.map(|text| {
            cx.new(|cx| {
                let mut f = TextField::new("Name", cx);
                f.set_text(text, cx);
                f
            })
        });
        match &field {
            Some(field) => window.focus(&field.focus_handle(cx)),
            None => window.focus(&focus_handle),
        }
        Self { op, field, focus_handle }
    }

    fn confirm(&mut self, _: &Confirm, _: &mut Window, cx: &mut Context<Self>) {
        let text = self.field.as_ref().map(|f| f.read(cx).text().trim().to_string()).unwrap_or_default();
        if self.field.is_some() && (text.is_empty() || text.contains("..")) {
            return;
        }
        cx.emit(DialogEvent::FileOp { op: self.op.clone(), name: text });
    }

    fn cancel(&mut self, _: &Cancel, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(DialogEvent::Dismiss);
    }
}

impl Focusable for InputDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for InputDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let name = |p: &PathBuf| p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let (title, ok) = match &self.op {
            FileOp::NewFile(dir) => (format!("New File in {}", name(dir)), "Create"),
            FileOp::NewDirectory(dir) => (format!("New Directory in {}", name(dir)), "Create"),
            FileOp::Rename(path) => (format!("Rename {}", name(path)), "Rename"),
            FileOp::Delete(path) => (format!("Move {} to the Trash?", name(path)), "Delete"),
            FileOp::FromTemplate(dir, template) => (format!("New {} in {}", template.label(), name(dir)), "Create"),
            FileOp::RenameSymbol(symbol) => (format!("Rename '{}' and its usages to", symbol.name), "Refactor"),
            FileOp::ComposerRequire => ("Require package (vendor/package, optionally a version)".to_string(), "Require"),
            FileOp::ComposerRemove => ("Remove package (vendor/package)".to_string(), "Remove"),
            FileOp::PhpInterpreter(_) => ("PHP interpreter for runs, tests and Artisan".to_string(), "Save"),
            FileOp::ExtractBlade { component: true, .. } => ("Extract Blade Component — name (e.g. user-card or cards.user)".to_string(), "Extract"),
            FileOp::ExtractBlade { component: false, .. } => ("Extract View — name (e.g. partials.header)".to_string(), "Extract"),
            FileOp::CompareRevision(path) => (format!("Compare {} with branch, tag or commit", name(path)), "Compare"),
        };
        card(420.)
            .key_context("Dialog")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::cancel))
            .child(div().px_3().py_2().border_b_1().border_color(theme::border()).child(title))
            .children(self.field.clone().map(|f| div().p_3().child(f)))
            .when(matches!(self.op, FileOp::PhpInterpreter(_)), |d| {
                d.child(div().px_3().text_size(px(12.)).text_color(theme::muted()).child("php or a path · docker:<container>[:<workdir>] · sail · ssh:<host>[:<workdir>]"))
            })
            .when(matches!(self.op, FileOp::NewFile(_)), |d| {
                d.child(div().px_3().text_size(px(12.)).text_color(theme::muted()).child("Use / to create folders too, e.g. Http/Controllers/HomeController.php"))
            })
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .p_3()
                    .child(button("input-cancel", "Cancel", false).on_click(cx.listener(|this, _, window, cx| this.cancel(&Cancel, window, cx))))
                    .child(button("input-ok", ok, true).on_click(cx.listener(|this, _, window, cx| this.confirm(&Confirm, window, cx)))),
            )
    }
}
