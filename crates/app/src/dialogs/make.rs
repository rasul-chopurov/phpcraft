//! "New Laravel…" chooser and the per-kind generator dialog (with the Eloquent model editor).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use gpui::{
    AnyElement, App, Context, Entity, EventEmitter, FocusHandle, Focusable, SharedString,
    Subscription, Window, div, prelude::*, px,
};
use laravel::make::{
    COLUMN_TYPES, Extra, Field, IdType, IndexSpec, MakeKind, Maker, ModelSpec, PlannedFile, Relation, RelationKind,
};
use laravel::settings::{ModelSettings, RelationColumns};

use super::{
    Cancel, Confirm, DeleteRow, DialogEvent, FocusNext, FocusPrev, MoveRowDown, MoveRowUp, SelectNext, SelectPrev, button,
    card, checkbox,
};
use crate::text_field::{FieldEvent, TextField};
use crate::theme;

// --- chooser ------------------------------------------------------------------

/// Small list of generators, like PhpStorm's "New…" popup.
pub struct MakeChooser {
    kinds: Vec<MakeKind>,
    query: Entity<TextField>,
    selected: usize,
    focus_handle: FocusHandle,
    _subscription: Subscription,
}

impl EventEmitter<DialogEvent> for MakeChooser {}

impl MakeChooser {
    pub fn new(root: &Path, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let query = cx.new(|cx| TextField::new("New Laravel…", cx));
        let subscription = cx.subscribe(&query, |this: &mut Self, _, _: &FieldEvent, cx| {
            this.selected = 0;
            cx.notify();
        });
        window.focus(&query.focus_handle(cx));
        Self {
            kinds: Maker::new(root).kinds(),
            query,
            selected: 0,
            focus_handle: cx.focus_handle(),
            _subscription: subscription,
        }
    }

    fn matches(&self, cx: &App) -> Vec<MakeKind> {
        let query = self.query.read(cx).text().to_ascii_lowercase();
        self.kinds
            .iter()
            .copied()
            .filter(|k| k.label().to_ascii_lowercase().contains(query.trim()))
            .collect()
    }

    fn select_prev(&mut self, _: &SelectPrev, _: &mut Window, cx: &mut Context<Self>) {
        self.selected = self.selected.saturating_sub(1);
        cx.notify();
    }

    fn select_next(&mut self, _: &SelectNext, _: &mut Window, cx: &mut Context<Self>) {
        let count = self.matches(cx).len();
        self.selected = (self.selected + 1).min(count.saturating_sub(1));
        cx.notify();
    }

    fn confirm(&mut self, _: &Confirm, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(kind) = self.matches(cx).get(self.selected) {
            cx.emit(DialogEvent::OpenMakeKind(*kind));
        }
    }

    fn cancel(&mut self, _: &Cancel, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(DialogEvent::Dismiss);
    }
}

impl Focusable for MakeChooser {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for MakeChooser {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let matches = self.matches(cx);
        let selected = self.selected.min(matches.len().saturating_sub(1));
        let rows = matches.into_iter().enumerate().map(|(ix, kind)| {
            div()
                .id(("make-kind", ix))
                .px_3()
                .h(px(24.))
                .flex()
                .items_center()
                .gap_2()
                .cursor_pointer()
                .when(ix == selected, |d| d.bg(theme::selection()))
                .hover(|d| d.bg(theme::hover()))
                .child(div().w(px(16.)).text_color(theme::syntax(php_syntax::Highlight::Keyword)).child(if kind.is_filament() { "◆" } else { "C" }))
                .child(kind.label())
                .on_click(cx.listener(move |_, _, _, cx| cx.emit(DialogEvent::OpenMakeKind(kind))))
        });
        card(340.)
            .key_context("Dialog ArtisanDialog")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::cancel))
            .on_action(cx.listener(Self::select_prev))
            .on_action(cx.listener(Self::select_next))
            .child(div().p_2().border_b_1().border_color(theme::border()).child(self.query.clone()))
            .child(div().id("make-kinds").max_h(px(460.)).overflow_y_scroll().py_1().children(rows))
    }
}

// --- generator dialog -----------------------------------------------------------

/// One row of the fields table.
struct ColumnRow {
    name: Entity<TextField>,
    ty: usize,
    nullable: bool,
    unique: bool,
    index: bool,
    default: Entity<TextField>,
}

/// One relation row in the fields table.
struct RelationRow {
    kind: usize,
    model: Entity<TextField>,
    /// Through model, or morph name for morph relations.
    extra: Entity<TextField>,
}

/// One composite index in the Index Editor.
struct IndexRow {
    columns: Entity<TextField>,
    unique: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    Main,
    Settings,
    Indexes,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Dropdown {
    Module,
    IdType,
    SettingsIdType,
    RelationColumns,
    ColumnType(usize),
    RelationKind(usize),
}

pub struct MakeDialog {
    root: PathBuf,
    /// Project models (FQNs) offered while typing a relation's model.
    models: Vec<String>,
    maker: Maker,
    kind: MakeKind,
    modules: Vec<String>,
    module: Option<usize>,
    name: Entity<TextField>,
    table: Entity<TextField>,
    columns: Vec<ColumnRow>,
    relations: Vec<RelationRow>,
    dropdown: Option<Dropdown>,
    extras: HashSet<Extra>,
    /// Name fields for post-generations (empty = derived name).
    extra_names: HashMap<Extra, Entity<TextField>>,
    id_type: IdType,
    view: View,
    /// Working copy edited on the Settings screen.
    draft: ModelSettings,
    id_name: Entity<TextField>,
    templates: HashMap<Extra, Entity<TextField>>,
    settings_extra: Extra,
    indexes: Vec<IndexRow>,
    with_id: bool,
    timestamps: bool,
    soft_deletes: bool,
    fillable: bool,
    error: Option<String>,
    focus_handle: FocusHandle,
    subscriptions: Vec<Subscription>,
}

impl EventEmitter<DialogEvent> for MakeDialog {}

impl MakeDialog {
    pub fn new(root: &Path, kind: MakeKind, models: Vec<String>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let maker = Maker::new(root);
        let modules = maker.modules();
        let extras_available = maker.extras(kind);
        let mut dialog = Self {
            root: root.to_path_buf(),
            models,
            kind,
            modules,
            module: None,
            name: cx.new(|cx| TextField::new(kind.placeholder(), cx)),
            table: cx.new(|cx| TextField::new("", cx)),
            columns: Vec::new(),
            relations: Vec::new(),
            dropdown: None,
            extras: if kind == MakeKind::Model {
                extras_available.iter().copied().filter(|e| maker.settings.model.enabled.contains(e)).collect()
            } else {
                extras_available.iter().copied().filter(|e| e.default_on()).collect()
            },
            extra_names: HashMap::new(),
            id_type: maker.settings.model.id_type,
            view: View::Main,
            draft: maker.settings.model.clone(),
            id_name: cx.new(|cx| TextField::new("id", cx)),
            templates: HashMap::new(),
            settings_extra: Extra::Migration,
            indexes: Vec::new(),
            with_id: true,
            timestamps: true,
            soft_deletes: false,
            fillable: true,
            error: None,
            focus_handle: cx.focus_handle(),
            subscriptions: Vec::new(),
            maker,
        };
        for field in [dialog.name.clone(), dialog.table.clone()] {
            dialog.watch(&field, cx);
        }
        if kind == MakeKind::Model {
            for extra in extras_available {
                let field = dialog.new_field("", cx);
                dialog.extra_names.insert(extra, field);
                let template = cx.new(|cx| TextField::new(ModelSettings::default_template(extra), cx));
                dialog.templates.insert(extra, template);
            }
            dialog.add_column(cx);
            dialog.sync_placeholders(cx);
        }
        window.focus(&dialog.name.focus_handle(cx));
        dialog
    }

    fn watch(&mut self, field: &Entity<TextField>, cx: &mut Context<Self>) {
        self.subscriptions.push(cx.subscribe(field, |this: &mut Self, _, _: &FieldEvent, cx| {
            this.error = None;
            this.sync_placeholders(cx);
            cx.notify();
        }));
    }

    fn new_field(&mut self, placeholder: &str, cx: &mut Context<Self>) -> Entity<TextField> {
        let placeholder = placeholder.to_string();
        let field = cx.new(|cx| TextField::new(placeholder, cx));
        self.watch(&field, cx);
        field
    }

    /// Derived names (table, migration, factory…) follow the model name as placeholders.
    fn sync_placeholders(&mut self, cx: &mut Context<Self>) {
        if self.kind != MakeKind::Model {
            return;
        }
        let model = self.name.read(cx).text().trim().to_string();
        let model = if model.is_empty() { "Model".to_string() } else { model };
        let model = model.rsplit(['/', '\\']).next().unwrap_or(&model).to_string();
        let typed_table = self.table.read(cx).text().trim().to_string();
        let table = if typed_table.is_empty() { laravel::make::table_name(&model) } else { typed_table };
        self.table.update(cx, |f, cx| f.set_placeholder(laravel::make::table_name(&model), cx));
        for (extra, field) in &self.extra_names {
            let name = self.maker.settings.model.name_for(*extra, &model, &table);
            field.update(cx, |f, cx| f.set_placeholder(name, cx));
        }
    }

    fn add_column(&mut self, cx: &mut Context<Self>) {
        let name = self.new_field("name", cx);
        let default = self.new_field("default", cx);
        self.columns.push(ColumnRow { name, ty: 0, nullable: false, unique: false, index: false, default });
        cx.notify();
    }

    /// Fills the dialog from an existing table (New Eloquent Model from Database Table).
    pub fn prefill(&mut self, model: &str, spec: &ModelSpec, cx: &mut Context<Self>) {
        let model = model.to_string();
        self.name.update(cx, |f, cx| f.set_text(model, cx));
        if let Some(table) = spec.table.clone() {
            self.table.update(cx, |f, cx| f.set_text(table, cx));
        }
        self.id_type = spec.id;
        self.with_id = spec.with_id;
        self.timestamps = spec.timestamps;
        self.soft_deletes = spec.soft_deletes;
        self.columns.clear();
        for field in &spec.fields {
            self.add_column(cx);
            let row = self.columns.last_mut().unwrap();
            row.ty = COLUMN_TYPES.iter().position(|t| *t == field.column).unwrap_or(0);
            row.nullable = field.nullable;
            row.unique = field.unique;
            row.index = field.index;
            let (name, default) = (row.name.clone(), row.default.clone());
            let (field_name, field_default) = (field.name.clone(), field.default.clone().unwrap_or_default());
            name.update(cx, |f, cx| f.set_text(field_name, cx));
            default.update(cx, |f, cx| f.set_text(field_default, cx));
        }
        self.relations.clear();
        for relation in &spec.relations {
            self.add_relation(cx);
            let row = self.relations.last_mut().unwrap();
            row.kind = RelationKind::ALL.iter().position(|k| *k == relation.kind).unwrap_or(0);
            let model = row.model.clone();
            let target = relation.target.clone();
            model.update(cx, |f, cx| f.set_text(target, cx));
        }
        // The table already exists.
        self.extras.remove(&Extra::Migration);
        self.sync_placeholders(cx);
        cx.notify();
    }

    fn add_relation(&mut self, cx: &mut Context<Self>) {
        let model = self.new_field("Model", cx);
        let extra = self.new_field("through / morph name", cx);
        self.relations.push(RelationRow { kind: 0, model, extra });
        cx.notify();
    }

    fn spec(&self, cx: &App) -> ModelSpec {
        let text = |f: &Entity<TextField>| f.read(cx).text().trim().to_string();
        ModelSpec {
            id: self.id_type,
            with_id: self.with_id,
            timestamps: self.timestamps,
            soft_deletes: self.soft_deletes,
            fillable: self.fillable,
            table: Some(text(&self.table)).filter(|t| !t.is_empty()),
            fields: self
                .columns
                .iter()
                .filter_map(|row| {
                    let name = laravel::make::snake(&text(&row.name));
                    let default = text(&row.default);
                    (!name.is_empty()).then(|| Field {
                        name,
                        column: COLUMN_TYPES[row.ty].to_string(),
                        nullable: row.nullable,
                        unique: row.unique,
                        index: row.index,
                        default: (!default.is_empty()).then_some(default),
                    })
                })
                .collect(),
            relations: self
                .relations
                .iter()
                .filter_map(|row| {
                    let target = text(&row.model);
                    let extra = text(&row.extra);
                    let target_fqn = self
                        .models
                        .iter()
                        .find(|fqn| php_index::short_name(fqn) == target || **fqn == target)
                        .cloned();
                    let target = php_index::short_name(&target).to_string();
                    (!target.is_empty()).then(|| Relation {
                        kind: RelationKind::ALL[row.kind],
                        target,
                        extra: (!extra.is_empty()).then_some(extra),
                        target_fqn,
                    })
                })
                .collect(),
            class_names: self
                .extra_names
                .iter()
                .map(|(extra, field)| (*extra, text(field)))
                .filter(|(_, name)| !name.is_empty())
                .collect(),
            indexes: self
                .indexes
                .iter()
                .map(|row| IndexSpec {
                    columns: text(&row.columns).split([',', ' ']).filter(|c| !c.is_empty()).map(laravel::make::snake).collect(),
                    unique: row.unique,
                })
                .filter(|i| !i.columns.is_empty())
                .collect(),
            settings: self.maker.settings.model.clone(),
        }
    }

    fn plan(&self, cx: &App) -> Vec<PlannedFile> {
        let extras: Vec<Extra> = self.extras.iter().copied().collect();
        let module = self.module.map(|ix| self.modules[ix].as_str());
        self.maker.plan_with(self.kind, self.name.read(cx).text(), &extras, module, &self.spec(cx))
    }

    fn focusable_fields(&self) -> Vec<Entity<TextField>> {
        let mut fields = vec![self.name.clone()];
        if self.kind == MakeKind::Model {
            fields.push(self.table.clone());
            for row in &self.columns {
                fields.push(row.name.clone());
                fields.push(row.default.clone());
            }
            for row in &self.relations {
                fields.push(row.model.clone());
                fields.push(row.extra.clone());
            }
            for extra in self.maker.extras(self.kind) {
                if let Some(field) = self.extra_names.get(&extra) {
                    fields.push(field.clone());
                }
            }
        }
        fields
    }

    fn cycle_focus(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let fields = self.focusable_fields();
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

    fn cancel(&mut self, _: &Cancel, window: &mut Window, cx: &mut Context<Self>) {
        if self.dropdown.take().is_some() {
            cx.notify();
        } else if self.view != View::Main {
            self.show_main(window, cx);
        } else {
            cx.emit(DialogEvent::Dismiss);
        }
    }

    fn confirm(&mut self, _: &Confirm, window: &mut Window, cx: &mut Context<Self>) {
        match self.view {
            View::Main => self.create(cx),
            View::Settings => self.save_settings(window, cx),
            View::Indexes => self.show_main(window, cx),
        }
    }

    fn show_main(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.view = View::Main;
        self.dropdown = None;
        window.focus(&self.name.focus_handle(cx));
        cx.notify();
    }

    fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.draft = self.maker.settings.model.clone();
        let id_name = self.draft.id_name.clone();
        self.id_name.update(cx, |f, cx| f.set_text(id_name, cx));
        for (extra, field) in &self.templates {
            let custom = self.draft.templates.get(extra).cloned().unwrap_or_default();
            field.update(cx, |f, cx| f.set_text(custom, cx));
        }
        self.view = View::Settings;
        window.focus(&self.id_name.focus_handle(cx));
        cx.notify();
    }

    fn save_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut draft = self.draft.clone();
        draft.id_name = self.id_name.read(cx).text().trim().to_string();
        if draft.id_name.is_empty() {
            draft.id_name = "id".into();
        }
        draft.templates = self
            .templates
            .iter()
            .map(|(extra, field)| (*extra, field.read(cx).text().trim().to_string()))
            .filter(|(extra, t)| !t.is_empty() && t != ModelSettings::default_template(*extra))
            .collect();
        self.maker.settings.model = draft;
        if let Err(err) = self.maker.settings.save(&self.root) {
            self.error = Some(format!("Failed to save settings: {err}"));
        }
        self.id_type = self.maker.settings.model.id_type;
        self.sync_placeholders(cx);
        self.show_main(window, cx);
    }

    fn open_indexes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.indexes.is_empty() {
            self.add_index(cx);
        }
        self.view = View::Indexes;
        if let Some(row) = self.indexes.last() {
            window.focus(&row.columns.focus_handle(cx));
        }
        cx.notify();
    }

    fn add_index(&mut self, cx: &mut Context<Self>) {
        let columns = self.new_field("column_a, column_b", cx);
        self.indexes.push(IndexRow { columns, unique: false });
        cx.notify();
    }

    /// Row of the Columns/Relations table containing the focused field.
    fn focused_row(&self, window: &Window, cx: &App) -> Option<(bool, usize)> {
        let focused = |f: &Entity<TextField>| f.focus_handle(cx).is_focused(window);
        if let Some(ix) = self.columns.iter().position(|r| focused(&r.name) || focused(&r.default)) {
            return Some((true, ix));
        }
        self.relations.iter().position(|r| focused(&r.model) || focused(&r.extra)).map(|ix| (false, ix))
    }

    fn move_row(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let Some((is_column, ix)) = self.focused_row(window, cx) else { return };
        let len = if is_column { self.columns.len() } else { self.relations.len() };
        let target = ix as isize + delta;
        if target < 0 || target as usize >= len {
            return;
        }
        if is_column {
            self.columns.swap(ix, target as usize);
        } else {
            self.relations.swap(ix, target as usize);
        }
        self.dropdown = None;
        cx.notify();
    }

    fn move_row_up(&mut self, _: &MoveRowUp, window: &mut Window, cx: &mut Context<Self>) {
        self.move_row(-1, window, cx);
    }

    fn move_row_down(&mut self, _: &MoveRowDown, window: &mut Window, cx: &mut Context<Self>) {
        self.move_row(1, window, cx);
    }

    fn delete_row(&mut self, _: &DeleteRow, window: &mut Window, cx: &mut Context<Self>) {
        let Some((is_column, ix)) = self.focused_row(window, cx) else { return };
        if is_column {
            self.columns.remove(ix);
        } else {
            self.relations.remove(ix);
        }
        self.dropdown = None;
        window.focus(&self.name.focus_handle(cx));
        cx.notify();
    }

    fn render_settings(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let d = &self.draft;
        let label = |text: &'static str| div().w(px(260.)).flex_none().child(text);
        let ids: Vec<SharedString> = IdType::ALL.iter().map(|i| SharedString::from(i.label())).collect();
        let relation_methods: Vec<SharedString> = vec!["foreignId/foreignUuid/foreignUlid".into(), "unsignedBigInteger".into()];
        let relation_value: SharedString = relation_methods[(d.relation_columns == RelationColumns::UnsignedBigInteger) as usize].clone();
        let id_select = self.select("set-id-type".into(), d.id_type.label().into(), None, Dropdown::SettingsIdType, ids, cx);
        let rel_select = self.select("set-rel".into(), relation_value, None, Dropdown::RelationColumns, relation_methods, cx);
        let toggle = |id: &'static str, text: &'static str, on: bool, f: fn(&mut ModelSettings)| {
            checkbox(id, text, on).on_click(cx.listener(move |this, _, _, cx| {
                f(&mut this.draft);
                cx.notify();
            }))
        };
        let extras = self.maker.extras(MakeKind::Model);
        let selected = self.settings_extra;
        let list = extras.iter().map(|&extra| {
            div()
                .id(SharedString::from(format!("set-extra-{extra:?}")))
                .px_2()
                .h(px(24.))
                .flex()
                .items_center()
                .cursor_pointer()
                .when(extra == selected, |d| d.bg(theme::selection()))
                .hover(|d| d.bg(theme::hover()))
                .child(extra.label())
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.settings_extra = extra;
                    cx.notify();
                }))
        });
        let enabled = d.enabled.contains(&selected);
        div()
            .flex()
            .flex_col()
            .gap_3()
            .p_3()
            .child(div().flex().items_center().gap_2().child(label("\"id\" field name:")).child(div().flex_1().child(self.id_name.clone())))
            .child(div().flex().items_center().gap_2().child(label("\"id\" field type:")).child(id_select))
            .child(div().flex().items_center().gap_2().child(label("Migration method for relation fields:")).child(rel_select))
            .child(toggle("set-constrained", "Add 'constrained()' calls", d.constrained, |d| d.constrained ^= true))
            .child(toggle("set-casts", "Use 'casts()' method instead of $casts field", d.casts_method, |d| d.casts_method ^= true))
            .child(toggle("set-tz", "Add timezones for time fields", d.timezones, |d| d.timezones ^= true))
            .child(toggle("set-down", "Omit the \"down\" method in migrations", d.omit_down, |d| d.omit_down ^= true))
            .child(toggle("set-helper", "Generate Helper Code after model creation", d.generate_helper_after, |d| d.generate_helper_after ^= true))
            .child(div().mt_2().text_color(theme::muted()).child("Post Generations"))
            .child(
                div()
                    .flex()
                    .gap_3()
                    .child(div().w(px(260.)).flex_none().border_1().border_color(theme::border()).rounded_sm().py_1().children(list))
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(checkbox("set-enabled", "Enabled by default", enabled).on_click(cx.listener(move |this, _, _, cx| {
                                if !this.draft.enabled.contains(&selected) {
                                    this.draft.enabled.push(selected);
                                } else {
                                    this.draft.enabled.retain(|e| *e != selected);
                                }
                                cx.notify();
                            })))
                            .child(div().text_color(theme::muted()).child("Class name template (${MODEL}, ${TABLE_NAME}):"))
                            .children(self.templates.get(&selected).cloned()),
                    ),
            )
            .child(div().text_size(px(11.)).text_color(theme::muted()).child("Saved to .phpcraft/laravel.json for this project."))
    }

    fn render_indexes(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let columns: Vec<String> = self
            .columns
            .iter()
            .map(|r| r.name.read(cx).text().trim().to_string())
            .filter(|n| !n.is_empty())
            .collect();
        let rows: Vec<_> = self
            .indexes
            .iter()
            .enumerate()
            .map(|(ix, row)| {
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().flex_1().child(row.columns.clone()))
                    .child(checkbox(format!("idx-unique-{ix}"), "Unique", row.unique).on_click(cx.listener(move |this, _, _, cx| {
                        this.indexes[ix].unique ^= true;
                        cx.notify();
                    })))
                    .child(remove_button(format!("idx-remove-{ix}")).on_click(cx.listener(move |this, _, _, cx| {
                        this.indexes.remove(ix);
                        cx.notify();
                    })))
            })
            .collect();
        div()
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .child(div().text_color(theme::muted()).child(format!(
                "Composite indexes. Columns: {}",
                if columns.is_empty() { "(none yet)".to_string() } else { columns.join(", ") }
            )))
            .children(rows)
            .child(div().flex().child(button("add-index", "Add Index", false).on_click(cx.listener(|this, _, window, cx| {
                this.add_index(cx);
                if let Some(row) = this.indexes.last() {
                    window.focus(&row.columns.focus_handle(cx));
                }
            }))))
    }

    fn create(&mut self, cx: &mut Context<Self>) {
        let plan = self.plan(cx);
        if plan.is_empty() {
            self.error = Some(match self.kind {
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

    fn toggle_dropdown(&mut self, dropdown: Dropdown, cx: &mut Context<Self>) {
        self.dropdown = if self.dropdown == Some(dropdown) { None } else { Some(dropdown) };
        cx.notify();
    }

    fn choose(&mut self, dropdown: Dropdown, ix: usize, cx: &mut Context<Self>) {
        match dropdown {
            Dropdown::Module => self.module = ix.checked_sub(1),
            Dropdown::IdType => self.id_type = IdType::ALL[ix],
            Dropdown::SettingsIdType => self.draft.id_type = IdType::ALL[ix],
            Dropdown::RelationColumns => {
                self.draft.relation_columns = [RelationColumns::ForeignId, RelationColumns::UnsignedBigInteger][ix];
            }
            Dropdown::ColumnType(row) => self.columns[row].ty = ix,
            Dropdown::RelationKind(row) => self.relations[row].kind = ix,
        }
        self.dropdown = None;
        cx.notify();
    }

    /// A select box; when open, its options float below it.
    fn select(
        &self,
        id: String,
        value: SharedString,
        width: Option<f32>,
        dropdown: Dropdown,
        options: Vec<SharedString>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let open = self.dropdown == Some(dropdown);
        let list = if open {
            let items: Vec<_> = options
                .into_iter()
                .enumerate()
                .map(|(ix, option)| {
                    let current = option == value;
                    div()
                        .id(("option", ix))
                        .px_2()
                        .h(px(22.))
                        .flex()
                        .items_center()
                        .cursor_pointer()
                        .when(current, |d| d.bg(theme::selection()))
                        .hover(|d| d.bg(theme::hover()))
                        .child(option)
                        .on_click(cx.listener(move |this, _, _, cx| this.choose(dropdown, ix, cx)))
                })
                .collect();
            Some(
                gpui::deferred(
                    gpui::anchored().child(
                        div()
                            .id("select-list")
                            .occlude()
                            .min_w(px(width.unwrap_or(200.)))
                            .max_h(px(260.))
                            .overflow_y_scroll()
                            .py_1()
                            .bg(theme::panel_bg())
                            .border_1()
                            .border_color(theme::border_strong())
                            .rounded_sm()
                            .shadow_lg()
                            .text_size(px(12.))
                            .children(items),
                    ),
                )
                .with_priority(2),
            )
        } else {
            None
        };
        div()
            .flex()
            .flex_col()
            .map(|d| match width {
                Some(w) => d.w(px(w)).flex_none(),
                None => d.flex_1(),
            })
            .child(
                div()
                    .id(gpui::ElementId::Name(id.into()))
                    .h(px(28.))
                    .px_2()
                    .flex()
                    .items_center()
                    .justify_between()
                    .rounded_sm()
                    .border_1()
                    .border_color(if open { theme::accent() } else { theme::border_strong() })
                    .bg(theme::editor_bg())
                    .cursor_pointer()
                    .text_size(px(12.))
                    .child(value)
                    .child(div().text_color(theme::muted()).child("⌄"))
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle_dropdown(dropdown, cx))),
            )
            .children(list)
    }

    fn module_row(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        if self.modules.is_empty() {
            return None;
        }
        let mut options: Vec<SharedString> = vec!["(no module)".into()];
        options.extend(self.modules.iter().map(|m| SharedString::from(m.clone())));
        let value = options[self.module.map_or(0, |m| m + 1)].clone();
        Some(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(div().w(px(90.)).flex_none().child("Module:"))
                .child(self.select("module".into(), value, None, Dropdown::Module, options, cx)),
        )
    }

    /// Project models matching what is typed in relation row `ix`, shown while it has focus.
    fn model_suggestions(&self, ix: usize, window: &Window, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let field = &self.relations[ix].model;
        if !field.focus_handle(cx).is_focused(window) {
            return None;
        }
        let typed = field.read(cx).text().trim().to_ascii_lowercase();
        let matches: Vec<String> = self
            .models
            .iter()
            .filter(|fqn| {
                let short = php_index::short_name(fqn).to_ascii_lowercase();
                short.contains(&typed) && short != typed
            })
            .take(8)
            .cloned()
            .collect();
        if matches.is_empty() {
            return None;
        }
        let items: Vec<_> = matches
            .into_iter()
            .enumerate()
            .map(|(i, fqn)| {
                let short = php_index::short_name(&fqn).to_string();
                let namespace = php_index::namespace_of(&fqn).to_string();
                div()
                    .id(("model-suggestion", i))
                    .flex()
                    .gap_2()
                    .px_2()
                    .h(px(22.))
                    .items_center()
                    .cursor_pointer()
                    .hover(|d| d.bg(theme::selection()))
                    .child(div().text_color(theme::syntax(php_syntax::Highlight::Keyword)).child("C"))
                    .child(short.clone())
                    .child(div().text_color(theme::muted()).child(namespace))
                    .on_mouse_down(gpui::MouseButton::Left, cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        let model = this.relations[ix].model.clone();
                        model.update(cx, |f, cx| f.set_text(short.clone(), cx));
                        // Move on so the list closes.
                        let next = this.relations[ix].extra.focus_handle(cx);
                        window.focus(&next);
                        cx.notify();
                    }))
            })
            .collect();
        Some(
            gpui::deferred(
                gpui::anchored().child(
                    div()
                        .occlude()
                        .min_w(px(260.))
                        .py_1()
                        .bg(theme::panel_bg())
                        .border_1()
                        .border_color(theme::border_strong())
                        .rounded_sm()
                        .shadow_lg()
                        .text_size(px(12.))
                        .children(items),
                ),
            )
            .with_priority(3),
        )
    }

    fn render_fields_table(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let header = |text: &'static str, width: Option<f32>| {
            div()
                .map(|d| match width {
                    Some(w) => d.w(px(w)).flex_none(),
                    None => d.flex_1(),
                })
                .text_color(theme::muted())
                .text_size(px(12.))
                .child(text)
        };
        let types: Vec<SharedString> = COLUMN_TYPES.iter().map(|t| SharedString::from(*t)).collect();
        let mut rows: Vec<AnyElement> = Vec::new();
        for (ix, row) in self.columns.iter().enumerate() {
            let check = |id: &str, on: bool| {
                div()
                    .w(px(58.))
                    .flex_none()
                    .flex()
                    .justify_center()
                    .child(checkbox(format!("{id}-{ix}"), "", on))
            };
            rows.push(
                div()
                    .flex()
                    .items_start()
                    .gap_2()
                    .child(div().flex_1().child(row.name.clone()))
                    .child(self.select(format!("type-{ix}"), COLUMN_TYPES[row.ty].into(), Some(140.), Dropdown::ColumnType(ix), types.clone(), cx))
                    .child(check("null", row.nullable).id(("null-cell", ix)).on_click(cx.listener(move |this, _, _, cx| {
                        this.columns[ix].nullable ^= true;
                        cx.notify();
                    })))
                    .child(check("unique", row.unique).id(("unique-cell", ix)).on_click(cx.listener(move |this, _, _, cx| {
                        this.columns[ix].unique ^= true;
                        cx.notify();
                    })))
                    .child(check("index", row.index).id(("index-cell", ix)).on_click(cx.listener(move |this, _, _, cx| {
                        this.columns[ix].index ^= true;
                        cx.notify();
                    })))
                    .child(div().w(px(90.)).flex_none().child(row.default.clone()))
                    .child(remove_button(format!("remove-col-{ix}")).on_click(cx.listener(move |this, _, _, cx| {
                        this.columns.remove(ix);
                        this.dropdown = None;
                        cx.notify();
                    })))
                    .into_any_element(),
            );
        }
        let kinds: Vec<SharedString> = RelationKind::ALL.iter().map(|k| SharedString::from(k.method())).collect();
        for (ix, row) in self.relations.iter().enumerate() {
            let kind = RelationKind::ALL[row.kind];
            let needs_extra = matches!(
                kind,
                RelationKind::HasOneThrough
                    | RelationKind::HasManyThrough
                    | RelationKind::MorphOne
                    | RelationKind::MorphMany
                    | RelationKind::MorphToMany
                    | RelationKind::MorphedByMany
            );
            rows.push(
                div()
                    .flex()
                    .items_start()
                    .gap_2()
                    .child(self.select(format!("rel-{ix}"), kind.method().into(), Some(150.), Dropdown::RelationKind(ix), kinds.clone(), cx))
                    .child(div().flex_1().flex().flex_col().child(row.model.clone()).children(self.model_suggestions(ix, window, cx)))
                    .child(div().w(px(200.)).flex_none().when(needs_extra, |d| d.child(row.extra.clone())))
                    .child(remove_button(format!("remove-rel-{ix}")).on_click(cx.listener(move |this, _, _, cx| {
                        this.relations.remove(ix);
                        this.dropdown = None;
                        cx.notify();
                    })))
                    .into_any_element(),
            );
        }
        div()
            .id("fields-table")
            .flex_1()
            .flex()
            .flex_col()
            .gap_1()
            .p_2()
            .h(px(300.))
            .overflow_y_scroll()
            .border_1()
            .border_color(theme::border())
            .rounded_sm()
            .child(
                div()
                    .flex()
                    .gap_2()
                    .mb_1()
                    .child(header("Name", None))
                    .child(header("Type", Some(140.)))
                    .child(header("Nullable", Some(58.)))
                    .child(header("Unique", Some(58.)))
                    .child(header("Index", Some(58.)))
                    .child(header("Default", Some(90.)))
                    .child(div().w(px(18.))),
            )
            .children(rows)
            .child(
                div()
                    .flex()
                    .gap_2()
                    .mt_2()
                    .child(button("add-field", "Add New Field", false).on_click(cx.listener(|this, _, window, cx| {
                        this.add_column(cx);
                        if let Some(row) = this.columns.last() {
                            window.focus(&row.name.focus_handle(cx));
                        }
                    })))
                    .child(button("add-relation", "Add New Relation", false).on_click(cx.listener(|this, _, window, cx| {
                        this.add_relation(cx);
                        if let Some(row) = this.relations.last() {
                            window.focus(&row.model.focus_handle(cx));
                        }
                    }))),
            )
    }

    fn render_model_options(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let ids: Vec<SharedString> = IdType::ALL.iter().map(|i| SharedString::from(i.label())).collect();
        let id_select = self.select("id-type".into(), self.id_type.label().into(), None, Dropdown::IdType, ids, cx);
        let toggle = |id: &'static str, label: &'static str, on: bool, f: fn(&mut Self)| {
            checkbox(id, label, on).on_click(cx.listener(move |this, _, _, cx| {
                f(this);
                cx.notify();
            }))
        };
        div()
            .w(px(290.))
            .flex_none()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(toggle("with-id", "id field", self.with_id, |d| d.with_id ^= true))
                    .child(id_select),
            )
            .child(toggle("timestamps", "timestamps (created_at, updated_at)", self.timestamps, |d| d.timestamps ^= true))
            .child(toggle("soft-deletes", "soft deletes (deleted_at)", self.soft_deletes, |d| d.soft_deletes ^= true))
            .child(toggle("fillable", "add $fillable array to the model class", self.fillable, |d| d.fillable ^= true))
            .child(
                div().flex().child(
                    button("index-editor", "Index Editor", false)
                        .on_click(cx.listener(|this, _, window, cx| this.open_indexes(window, cx))),
                ),
            )
            .when(!self.indexes.is_empty(), |d| {
                d.child(div().text_size(px(12.)).text_color(theme::muted()).child(format!("{} composite index(es)", self.indexes.len())))
            })
            .child(div().flex_1())
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(theme::muted())
                    .child("Tab / Shift-Tab to move between fields, ↵ to create, Esc to cancel. Ctrl-[ / Ctrl-] move rows, Ctrl-Backspace deletes."),
            )
    }

    fn render_post_generations(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let items: Vec<AnyElement> = self
            .maker
            .extras(self.kind)
            .into_iter()
            .map(|extra| {
                let on = self.extras.contains(&extra);
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .w(px(420.))
                    .child(
                        div().w(px(170.)).flex_none().child(
                            checkbox(format!("extra-{extra:?}"), extra.label(), on).on_click(cx.listener(move |this, _, _, cx| {
                                if !this.extras.remove(&extra) {
                                    this.extras.insert(extra);
                                }
                                cx.notify();
                            })),
                        ),
                    )
                    .children(self.extra_names.get(&extra).map(|f| div().flex_1().child(f.clone())))
                    .into_any_element()
            })
            .collect();
        div().flex().flex_wrap().gap_x_6().gap_y_2().children(items)
    }

    fn render_files(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let plan = self.plan(cx);
        let files = plan.iter().map(|file| {
            let rel = file.path.strip_prefix(&self.root).unwrap_or(&file.path).display().to_string();
            div()
                .text_color(if file.exists { theme::error() } else { theme::success() })
                .child(if file.exists { format!("{rel}  (exists, skipped)") } else { rel })
        });
        div()
            .id("files-preview")
            .max_h(px(110.))
            .overflow_y_scroll()
            .p_2()
            .rounded_sm()
            .bg(theme::editor_bg())
            .font_family(theme::EDITOR_FONT)
            .text_size(px(11.))
            .child(div().text_color(theme::muted()).child(format!("{} file(s)", plan.len())))
            .children(files)
    }
}

fn remove_button(id: String) -> gpui::Stateful<gpui::Div> {
    div()
        .id(gpui::ElementId::Name(id.into()))
        .w(px(18.))
        .h(px(28.))
        .flex()
        .items_center()
        .text_color(theme::muted())
        .cursor_pointer()
        .hover(|d| d.text_color(theme::error()))
        .child("✕")
}

impl Focusable for MakeDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for MakeDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let is_model = self.kind == MakeKind::Model;
        let title: SharedString = match (is_model, self.view) {
            (true, View::Settings) => "New Eloquent Model Settings".into(),
            (true, View::Indexes) => "Index Editor".into(),
            (true, View::Main) => "New Eloquent Model".into(),
            _ => format!("New {}", self.kind.label()).into(),
        };
        let name_label = match self.kind {
            MakeKind::Model => "Model name",
            MakeKind::PivotTable => "Models",
            _ => "Name",
        };
        let mut body = div().flex().flex_col().gap_3().p_3();
        body = body.children(self.module_row(cx));
        body = body.child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(div().w(px(90.)).flex_none().child(name_label))
                .child(div().flex_1().child(self.name.clone()))
                .when(is_model, |d| {
                    d.child(div().ml_2().text_color(theme::muted()).child("Table name"))
                        .child(div().w(px(260.)).flex_none().child(self.table.clone()))
                }),
        );
        if is_model && self.view == View::Settings {
            body = div().child(self.render_settings(cx));
        } else if is_model && self.view == View::Indexes {
            body = div().child(self.render_indexes(cx));
        } else if is_model {
            body = body
                .child(div().flex().gap_3().child(self.render_fields_table(window, cx)).child(self.render_model_options(cx)))
                .child(self.render_post_generations(cx));
        } else if !self.maker.extras(self.kind).is_empty() {
            let extras = self.maker.extras(self.kind).into_iter().map(|extra| {
                checkbox(format!("extra-{extra:?}"), extra.label(), self.extras.contains(&extra)).on_click(cx.listener(move |this, _, _, cx| {
                    if !this.extras.remove(&extra) {
                        this.extras.insert(extra);
                    }
                    cx.notify();
                }))
            });
            body = body.child(div().flex().gap_4().children(extras));
        }
        if self.view == View::Main {
            body = body.child(self.render_files(cx));
        }
        body = body
            .children(self.error.clone().map(|e| div().text_color(theme::error()).child(e)));

        card(if is_model { 1040. } else { 560. })
            .key_context("Dialog")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::cancel))
            .on_action(cx.listener(Self::focus_next))
            .on_action(cx.listener(Self::focus_prev))
            .on_action(cx.listener(Self::move_row_up))
            .on_action(cx.listener(Self::move_row_down))
            .on_action(cx.listener(Self::delete_row))
            .child(div().px_3().py_2().border_b_1().border_color(theme::border()).child(title))
            .child(body)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .border_t_1()
                    .border_color(theme::border())
                    .when(is_model && self.view == View::Main, |d| {
                        d.child(button("settings", "Settings", false).on_click(cx.listener(|this, _, window, cx| this.open_settings(window, cx))))
                    })
                    .child(div().flex_1())
                    .child(button("cancel", "Cancel", false).on_click(cx.listener(|this, _, window, cx| this.cancel(&Cancel, window, cx))))
                    .child(button("create", "OK", true).on_click(cx.listener(|this, _, window, cx| this.confirm(&Confirm, window, cx)))),
            )
    }
}
