//! A small dialog asking for a name (New File, New Directory, Rename) or a confirmation
//! (Delete).

use std::path::PathBuf;

use gpui::{App, Context, Entity, EventEmitter, FocusHandle, Focusable, SharedString, Window, div, prelude::*, px};

use super::{Cancel, Confirm, DialogEvent, button, card};
use crate::i18n::{tr, trf};
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
    /// Refactor ▸ Move Class (F6): the declaration, which leaves its namespace.
    MoveClass(php_analysis::Moving),
    /// Tools ▸ Composer ▸ Require… / Remove… (the name is `vendor/package [constraint]`).
    ComposerRequire,
    ComposerRemove,
    /// Run ▸ PHP Interpreter… (the current setting).
    PhpInterpreter(String),
    /// Refactor ▸ Extract Blade Component / Extract View of a selection (byte range).
    ExtractBlade { component: bool, path: PathBuf, range: std::ops::Range<usize>, text: String },
    /// Git ▸ Compare with Branch / Revision… for a file.
    CompareRevision(PathBuf),
    /// Debug ▸ the PHP condition of a breakpoint (0-based line); empty clears it.
    BreakpointCondition { path: PathBuf, line: usize },
    /// Laravel ▸ New Module: the name laravel-modules turns into `Modules/<StudlyName>`, plus
    /// the module type its `config/modules.php` otherwise picks.
    NewModule { kind: crate::modules::ModuleKind },
    /// Remote ▸ Rename on the server: the entry keeps its folder, only its name changes.
    RenameRemote { server: String, from: String },
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
            FileOp::MoveClass(class) => Some(class.namespace.clone()),
            FileOp::ComposerRequire | FileOp::ComposerRemove => Some(String::new()),
            FileOp::PhpInterpreter(current) => Some(current.clone()),
            FileOp::ExtractBlade { .. } => Some(String::new()),
            FileOp::CompareRevision(_) => Some("main".into()),
            FileOp::NewModule { .. } => Some(String::new()),
            FileOp::RenameRemote { from, .. } => Some(from.rsplit('/').next().unwrap_or(from).to_string()),
            FileOp::BreakpointCondition { path, line } => Some(
                crate::breakpoints::for_file(path).into_iter().find(|(l, _)| l == line).and_then(|(_, c)| c).unwrap_or_default(),
            ),
        };
        let focus_handle = cx.focus_handle();
        let field = initial.map(|text| {
            cx.new(|cx| {
                let mut f = TextField::new(tr("Name"), cx);
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
        // An empty condition is how a breakpoint loses its condition.
        let may_be_empty = matches!(self.op, FileOp::BreakpointCondition { .. });
        if self.field.is_some() && ((text.is_empty() && !may_be_empty) || text.contains("..")) {
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
            FileOp::NewFile(dir) => (trf("New File in {}", &[&name(dir)]), "Create"),
            FileOp::NewDirectory(dir) => (trf("New Directory in {}", &[&name(dir)]), "Create"),
            FileOp::Rename(path) => (trf("Rename {}", &[&name(path)]), "Rename"),
            FileOp::Delete(path) => (trf("Move {} to the Trash?", &[&name(path)]), "Delete"),
            FileOp::FromTemplate(dir, template) => (trf("New {} in {}", &[template.label(), &name(dir)]), "Create"),
            FileOp::RenameSymbol(symbol) => (trf("Rename '{}' and its usages to", &[&symbol.name]), "Refactor"),
            FileOp::MoveClass(class) => {
                (trf("Move '{}' to namespace (empty for the global namespace)", &[&class.name]), "Move")
            }
            FileOp::ComposerRequire => (tr("Require package (vendor/package, optionally a version)").to_string(), "Require"),
            FileOp::ComposerRemove => (tr("Remove package (vendor/package)").to_string(), "Remove"),
            FileOp::PhpInterpreter(_) => (tr("PHP interpreter for runs, tests and Artisan").to_string(), "Save"),
            FileOp::ExtractBlade { component: true, .. } => {
                (tr("Extract Blade Component — name (e.g. user-card or cards.user)").to_string(), "Extract")
            }
            FileOp::ExtractBlade { component: false, .. } => {
                (tr("Extract View — name (e.g. partials.header)").to_string(), "Extract")
            }
            FileOp::CompareRevision(path) => (trf("Compare {} with branch, tag or commit", &[&name(path)]), "Compare"),
            FileOp::BreakpointCondition { path, line } => {
                (trf("Condition for the breakpoint in {} line {}", &[&name(path), &(line + 1).to_string()]), "Save")
            }
            FileOp::NewModule { .. } => (tr("New Module — name (e.g. Blog)").to_string(), "Create"),
            FileOp::RenameRemote { from, .. } => {
                (trf("Rename '{}' on the server to", &[from.rsplit('/').next().unwrap_or(from)]), "Rename")
            }
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
            .when(matches!(self.op, FileOp::BreakpointCondition { .. }), |d| {
                d.child(div().px_3().text_size(px(12.)).text_color(theme::muted()).child(tr("PHP expression — the breakpoint stops only when it is true. An empty condition removes it.")))
            })
            .when(matches!(self.op, FileOp::NewModule { .. }), |d| {
                let current = match &self.op {
                    FileOp::NewModule { kind } => *kind,
                    _ => crate::modules::ModuleKind::Standard,
                };
                let chips = crate::modules::ModuleKind::ALL.map(|kind| {
                    let wanted = kind;
                    let on = kind == current;
                    div()
                        .id(SharedString::from(format!("module-kind-{}", kind.label())))
                        .px_2()
                        .py_0p5()
                        .rounded_md()
                        .cursor_pointer()
                        .border_1()
                        .border_color(if on { theme::accent() } else { theme::border() })
                        .when(on, |d| d.bg(theme::selection()))
                        .text_color(if on { theme::ui_text() } else { theme::muted() })
                        .child(tr(kind.label()))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let FileOp::NewModule { kind } = &mut this.op {
                                *kind = wanted;
                            }
                            cx.notify();
                        }))
                });
                d.child(div().px_3().pb_2().flex().gap_1().children(chips))
                    .child(div().px_3().text_size(px(12.)).text_color(theme::muted()).child(tr("laravel-modules runs `php artisan module:make` and creates Modules/<Name> with its provider, routes and config.")))
            })
            .when(matches!(self.op, FileOp::NewFile(_)), |d| {
                d.child(div().px_3().text_size(px(12.)).text_color(theme::muted()).child(tr("Use / to create folders too, e.g. Http/Controllers/HomeController.php")))
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
