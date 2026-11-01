//! Modal dialogs: Laravel Make (code generation) and Artisan command runner.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::{
    AnyElement, App, Context, Entity, EventEmitter, FocusHandle, Focusable, MouseButton,
    SharedString, Subscription, Window, actions, div, prelude::*, px,
};
use laravel::artisan::{self, ArtisanCommand};
use laravel::make::{Extra, IdType, MakeKind, Maker, ModelSpec, PlannedFile, parse_fields, parse_relations};

use crate::text_field::{FieldEvent, TextField};
use crate::theme;

actions!(dialog, [Confirm, Cancel, FocusNext, FocusPrev, SelectPrev, SelectNext, Complete]);

pub enum DialogEvent {
    Dismiss,
    Created(Vec<PathBuf>),
    RunArtisan(String),
    CommandsLoaded(Arc<Vec<ArtisanCommand>>),
}

fn chip(id: impl Into<SharedString>, label: impl Into<SharedString>, active: bool) -> gpui::Stateful<gpui::Div> {
    div()
        .id(gpui::ElementId::Name(id.into()))
        .px_2()
        .py(px(3.))
        .rounded_sm()
        .border_1()
        .border_color(if active { theme::accent() } else { theme::border_strong() })
        .when(active, |d| d.bg(theme::active_row()))
        .hover(|d| d.bg(theme::hover()))
        .cursor_pointer()
        .child(label.into())
}

fn checkbox(id: impl Into<SharedString>, label: impl Into<SharedString>, checked: bool) -> gpui::Stateful<gpui::Div> {
    div()
        .id(gpui::ElementId::Name(id.into()))
        .flex()
        .items_center()
        .gap_1()
        .cursor_pointer()
        .child(
            div()
                .size(px(14.))
                .rounded_sm()
                .border_1()
                .border_color(if checked { theme::accent() } else { theme::border_strong() })
                .when(checked, |d| d.bg(theme::accent()))
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(10.))
                .text_color(theme::editor_bg())
                .child(if checked { "✓" } else { "" }),
        )
        .child(label.into())
}

fn label(text: &'static str) -> impl IntoElement {
    div().w(px(96.)).flex_none().text_color(theme::muted()).child(text)
}

fn row(name: &'static str, content: impl IntoElement) -> impl IntoElement {
    div().flex().items_center().gap_2().child(label(name)).child(div().flex_1().child(content))
}

fn button(id: &'static str, text: &'static str, primary: bool) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .px_3()
        .py_1()
        .rounded_sm()
        .cursor_pointer()
        .when(primary, |d| d.bg(theme::accent()).text_color(gpui::white()))
        .when(!primary, |d| d.border_1().border_color(theme::border_strong()))
        .hover(|d| d.opacity(0.85))
        .child(text)
}

fn card(width: f32) -> gpui::Div {
    div()
        .w(px(width))
        .bg(theme::panel_bg())
        .border_1()
        .border_color(theme::border_strong())
        .rounded_md()
        .shadow_lg()
        .text_size(px(13.))
        .text_color(theme::ui_text())
        // Clicks inside the card must not reach the dismissing overlay.
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
}

// --- Laravel Make -------------------------------------------------------------

pub struct MakeDialog {
    root: PathBuf,
    maker: Maker,
    kinds: Vec<MakeKind>,
    kind: usize,
    modules: Vec<String>,
    module: Option<usize>,
    name: Entity<TextField>,
    columns: Entity<TextField>,
    relations: Entity<TextField>,
    extras: HashSet<Extra>,
    id_type: IdType,
    timestamps: bool,
    soft_deletes: bool,
    error: Option<String>,
    focus_handle: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<DialogEvent> for MakeDialog {}

impl MakeDialog {
    pub fn new(root: &Path, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let maker = Maker::new(root);
        let kinds = maker.kinds();
        let name = cx.new(|cx| TextField::new(kinds[0].placeholder(), cx));
        let columns = cx.new(|cx| TextField::new("title:string, body:text:nullable, price:decimal", cx));
        let relations = cx.new(|cx| TextField::new("belongsTo:User, hasMany:Comment, belongsToMany:Tag", cx));
        let subscriptions = [&name, &columns, &relations]
            .into_iter()
            .map(|field| {
                cx.subscribe(field, |this: &mut Self, _, _: &FieldEvent, cx| {
                    this.error = None;
                    cx.notify();
                })
            })
            .collect();
        window.focus(&name.focus_handle(cx));
        let modules = maker.modules();
        let mut dialog = Self {
            root: root.to_path_buf(),
            maker,
            kinds,
            kind: 0,
            modules,
            module: None,
            name,
            columns,
            relations,
            extras: HashSet::new(),
            id_type: IdType::Id,
            timestamps: true,
            soft_deletes: false,
            error: None,
            focus_handle: cx.focus_handle(),
            _subscriptions: subscriptions,
        };
        dialog.reset_extras();
        dialog
    }

    fn kind(&self) -> MakeKind {
        self.kinds[self.kind]
    }

    fn reset_extras(&mut self) {
        self.extras = self.kind().extras().iter().copied().filter(|e| e.default_on()).collect();
    }

    fn select_kind(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.kind = ix;
        self.reset_extras();
        let placeholder = self.kind().placeholder();
        self.name.update(cx, |f, cx| f.set_placeholder(placeholder, cx));
        window.focus(&self.name.focus_handle(cx));
        cx.notify();
    }

    fn spec(&self, cx: &App) -> ModelSpec {
        ModelSpec {
            id: self.id_type,
            timestamps: self.timestamps,
            soft_deletes: self.soft_deletes,
            fields: parse_fields(self.columns.read(cx).text()),
            relations: parse_relations(self.relations.read(cx).text()),
        }
    }

    fn plan(&self, cx: &App) -> Vec<PlannedFile> {
        let extras: Vec<Extra> = self.extras.iter().copied().collect();
        let module = self.module.map(|ix| self.modules[ix].as_str());
        self.maker.plan_with(self.kind(), self.name.read(cx).text(), &extras, module, &self.spec(cx))
    }

    fn fields(&self) -> Vec<&Entity<TextField>> {
        if self.kind() == MakeKind::Model {
            vec![&self.name, &self.columns, &self.relations]
        } else {
            vec![&self.name]
        }
    }

    fn cycle_focus(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let fields = self.fields();
        let current = fields.iter().position(|f| f.focus_handle(cx).is_focused(window)).unwrap_or(0);
        let next = (current as isize + delta).rem_euclid(fields.len() as isize) as usize;
        window.focus(&fields[next].focus_handle(cx));
    }

    fn focus_next(&mut self, _: &FocusNext, window: &mut Window, cx: &mut Context<Self>) {
        self.cycle_focus(1, window, cx);
    }

    fn focus_prev(&mut self, _: &FocusPrev, window: &mut Window, cx: &mut Context<Self>) {
        self.cycle_focus(-1, window, cx);
    }

    fn cancel(&mut self, _: &Cancel, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(DialogEvent::Dismiss);
    }

    fn confirm(&mut self, _: &Confirm, _: &mut Window, cx: &mut Context<Self>) {
        self.create(cx);
    }

    fn create(&mut self, cx: &mut Context<Self>) {
        let plan = self.plan(cx);
        if plan.is_empty() {
            self.error = Some(match self.kind() {
                MakeKind::PivotTable => "Enter two models, e.g. Post, Tag".into(),
                _ => "Enter a class name".into(),
            });
        } else if plan.iter().all(|f| f.exists) {
            self.error = Some("All files already exist".into());
        } else {
            match laravel::make::write(&plan) {
                Ok(written) => cx.emit(DialogEvent::Created(written)),
                Err(err) => self.error = Some(format!("Failed to write files: {err}")),
            }
        }
        cx.notify();
    }

    fn render_kinds(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut last_filament = false;
        let items: Vec<AnyElement> = self
            .kinds
            .iter()
            .enumerate()
            .flat_map(|(ix, kind)| {
                let mut out = Vec::new();
                if kind.is_filament() && !last_filament {
                    out.push(div().px_2().pt_2().pb_1().text_size(px(11.)).text_color(theme::muted()).child("FILAMENT").into_any_element());
                }
                last_filament = kind.is_filament();
                out.push(
                    div()
                        .id(("kind", ix))
                        .px_2()
                        .h(px(24.))
                        .flex()
                        .items_center()
                        .rounded_sm()
                        .cursor_pointer()
                        .when(ix == self.kind, |d| d.bg(theme::selection()))
                        .hover(|d| d.bg(theme::hover()))
                        .child(kind.label())
                        .on_click(cx.listener(move |this, _, window, cx| this.select_kind(ix, window, cx)))
                        .into_any_element(),
                );
                out
            })
            .collect();
        div()
            .id("kinds")
            .w(px(190.))
            .flex_none()
            .h(px(460.))
            .overflow_y_scroll()
            .p_1()
            .border_r_1()
            .border_color(theme::border())
            .children(items)
    }

    fn render_form(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let kind = self.kind();
        let mut form = div().flex_1().flex().flex_col().gap_3().p_3();

        if !self.modules.is_empty() {
            let mut chips = vec![chip("module-none", "No module", self.module.is_none())
                .on_click(cx.listener(|this, _, _, cx| {
                    this.module = None;
                    cx.notify();
                }))
                .into_any_element()];
            for (ix, module) in self.modules.iter().enumerate() {
                chips.push(
                    chip(format!("module-{ix}"), module.clone(), self.module == Some(ix))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.module = Some(ix);
                            cx.notify();
                        }))
                        .into_any_element(),
                );
            }
            form = form.child(row("Module", div().flex().flex_wrap().gap_1().children(chips)));
        }

        let name_label = if kind == MakeKind::PivotTable { "Models" } else { "Name" };
        form = form.child(div().flex().items_center().gap_2().child(div().w(px(96.)).flex_none().text_color(theme::muted()).child(name_label)).child(div().flex_1().child(self.name.clone())));

        if kind == MakeKind::Model {
            let ids = IdType::ALL.into_iter().map(|id| {
                chip(format!("id-{id:?}"), id.label(), self.id_type == id)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.id_type = id;
                        cx.notify();
                    }))
            });
            form = form
                .child(row("Primary key", div().flex().gap_1().children(ids)))
                .child(row(
                    "Options",
                    div()
                        .flex()
                        .gap_4()
                        .child(checkbox("timestamps", "Timestamps", self.timestamps).on_click(cx.listener(|this, _, _, cx| {
                            this.timestamps = !this.timestamps;
                            cx.notify();
                        })))
                        .child(checkbox("soft-deletes", "Soft deletes", self.soft_deletes).on_click(cx.listener(|this, _, _, cx| {
                            this.soft_deletes = !this.soft_deletes;
                            cx.notify();
                        }))),
                ))
                .child(row("Columns", self.columns.clone()))
                .child(row("Relations", self.relations.clone()));
        }

        if !kind.extras().is_empty() {
            let extras = kind.extras().iter().map(|&extra| {
                checkbox(format!("extra-{extra:?}"), extra.label(), self.extras.contains(&extra)).on_click(cx.listener(move |this, _, _, cx| {
                    if !this.extras.remove(&extra) {
                        this.extras.insert(extra);
                    }
                    cx.notify();
                }))
            });
            form = form.child(row("Also create", div().flex().flex_wrap().gap_x_4().gap_y_2().children(extras)));
        }

        let plan = self.plan(cx);
        let files = plan.iter().map(|file| {
            let rel = file.path.strip_prefix(&self.root).unwrap_or(&file.path).display().to_string();
            div()
                .font_family(theme::EDITOR_FONT)
                .text_size(px(12.))
                .text_color(if file.exists { theme::error() } else { theme::success() })
                .child(if file.exists { format!("{rel}  (exists, skipped)") } else { rel })
        });
        form.child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .mt_1()
                .p_2()
                .rounded_sm()
                .bg(theme::editor_bg())
                .min_h(px(60.))
                .child(div().text_size(px(11.)).text_color(theme::muted()).child("FILES"))
                .children(files),
        )
        .children(self.error.clone().map(|e| div().text_color(theme::error()).child(e)))
    }
}

impl Focusable for MakeDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for MakeDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let title: SharedString = format!("Laravel Make · {}", self.kind().label()).into();
        card(820.)
            .key_context("Dialog")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::cancel))
            .on_action(cx.listener(Self::focus_next))
            .on_action(cx.listener(Self::focus_prev))
            .child(div().px_3().py_2().border_b_1().border_color(theme::border()).child(title))
            .child(div().flex().child(self.render_kinds(cx)).child(self.render_form(cx)))
            .child(
                div()
                    .flex()
                    .justify_between()
                    .items_center()
                    .px_3()
                    .py_2()
                    .border_t_1()
                    .border_color(theme::border())
                    .child(div().text_size(px(11.)).text_color(theme::muted()).child("Tab — next field · ↵ create · Esc cancel"))
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(button("cancel", "Cancel", false).on_click(cx.listener(|_, _, _, cx| cx.emit(DialogEvent::Dismiss))))
                            .child(button("create", "Create", true).on_click(cx.listener(|this, _, _, cx| this.create(cx)))),
                    ),
            )
    }
}

// --- Artisan ------------------------------------------------------------------

pub struct ArtisanDialog {
    query: Entity<TextField>,
    commands: Option<Arc<Vec<ArtisanCommand>>>,
    error: Option<String>,
    selected: usize,
    focus_handle: FocusHandle,
    _subscription: Subscription,
}

impl EventEmitter<DialogEvent> for ArtisanDialog {}

impl ArtisanDialog {
    pub fn new(root: &Path, cached: Option<Arc<Vec<ArtisanCommand>>>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let query = cx.new(|cx| TextField::new("Type a command, e.g. make:model Post -mf, migrate, route:list", cx));
        let subscription = cx.subscribe(&query, |this: &mut Self, _, _: &FieldEvent, cx| {
            this.selected = 0;
            cx.notify();
        });
        window.focus(&query.focus_handle(cx));
        if cached.is_none() {
            let root = root.to_path_buf();
            cx.spawn(async move |this, cx| {
                let result = cx.background_executor().spawn(async move { artisan::list_commands(&root) }).await;
                this.update(cx, |this, cx| {
                    match result {
                        Ok(list) => {
                            let list = Arc::new(list);
                            this.commands = Some(list.clone());
                            cx.emit(DialogEvent::CommandsLoaded(list));
                        }
                        Err(err) => this.error = Some(err),
                    }
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
        Self {
            query,
            commands: cached,
            error: None,
            selected: 0,
            focus_handle: cx.focus_handle(),
            _subscription: subscription,
        }
    }

    fn matches(&self, cx: &App) -> Vec<ArtisanCommand> {
        let Some(commands) = &self.commands else { return Vec::new() };
        let query = self.query.read(cx).text();
        let word = query.split_whitespace().next().unwrap_or("").to_ascii_lowercase();
        let mut found: Vec<_> = commands
            .iter()
            .filter(|c| {
                let name = c.name.to_ascii_lowercase();
                let mut chars = name.chars();
                word.chars().all(|w| chars.any(|c| c == w))
            })
            .cloned()
            .collect();
        found.sort_by_key(|c| (!c.name.starts_with(&word), !c.name.contains(&word), c.name.len()));
        found
    }

    fn select_prev(&mut self, _: &SelectPrev, _: &mut Window, cx: &mut Context<Self>) {
        self.selected = self.selected.saturating_sub(1);
        cx.notify();
    }

    fn select_next(&mut self, _: &SelectNext, _: &mut Window, cx: &mut Context<Self>) {
        let count = self.matches(cx).len();
        if count > 0 {
            self.selected = (self.selected + 1).min(count - 1);
        }
        cx.notify();
    }

    /// Tab: put the selected command name into the field so arguments can be typed.
    fn complete(&mut self, _: &Complete, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(command) = self.matches(cx).get(self.selected) {
            let text = format!("{} ", command.name);
            self.query.update(cx, |q, cx| q.set_text(text, cx));
        }
    }

    fn confirm(&mut self, _: &Confirm, _: &mut Window, cx: &mut Context<Self>) {
        let query = self.query.read(cx).text().trim().to_string();
        let first = query.split_whitespace().next().unwrap_or("");
        let exact = self.commands.as_ref().is_some_and(|c| c.iter().any(|c| c.name == first));
        let line = if exact || (query.contains(' ') && !query.is_empty()) {
            query
        } else if let Some(command) = self.matches(cx).get(self.selected) {
            command.name.clone()
        } else {
            query
        };
        if !line.is_empty() {
            cx.emit(DialogEvent::RunArtisan(line));
        }
    }

    fn cancel(&mut self, _: &Cancel, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(DialogEvent::Dismiss);
    }
}

impl Focusable for ArtisanDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ArtisanDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let matches = self.matches(cx);
        let selected = self.selected.min(matches.len().saturating_sub(1));
        let first = selected.saturating_sub(13);
        let rows = matches.iter().enumerate().skip(first).take(14).map(|(ix, command)| {
            div()
                .id(("cmd", ix))
                .flex()
                .gap_3()
                .px_3()
                .h(px(24.))
                .items_center()
                .when(ix == selected, |d| d.bg(theme::selection()))
                .hover(|d| d.bg(theme::hover()))
                .cursor_pointer()
                .child(div().w(px(220.)).flex_none().font_family(theme::EDITOR_FONT).text_color(theme::syntax(php_syntax::Highlight::Function)).child(command.name.clone()))
                .child(div().flex_1().overflow_hidden().text_color(theme::muted()).child(command.description.clone()))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.selected = ix;
                    cx.notify();
                }))
        });
        let status = match (&self.commands, &self.error) {
            (_, Some(err)) => Some(div().px_3().py_2().text_color(theme::error()).child(err.clone())),
            (None, None) => Some(div().px_3().py_2().text_color(theme::muted()).child("Loading artisan commands…")),
            _ => None,
        };
        card(720.)
            .key_context("Dialog ArtisanDialog")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::cancel))
            .on_action(cx.listener(Self::select_prev))
            .on_action(cx.listener(Self::select_next))
            .on_action(cx.listener(Self::complete))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .p_2()
                    .border_b_1()
                    .border_color(theme::border())
                    .child(div().font_family(theme::EDITOR_FONT).text_color(theme::muted()).child("php artisan"))
                    .child(div().flex_1().child(self.query.clone())),
            )
            .children(status)
            .child(div().py_1().children(rows))
            .child(
                div()
                    .px_3()
                    .py_1()
                    .border_t_1()
                    .border_color(theme::border())
                    .text_size(px(11.))
                    .text_color(theme::muted())
                    .child("↑↓ select · Tab complete · ↵ run · Esc close"),
            )
    }
}
