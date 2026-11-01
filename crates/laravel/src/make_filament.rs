//! Filament generators: resources (with pages, and schema/table classes on v4+), pages, widgets.

use super::{ClassName, PlannedFile, Target, plural, snake};

fn class_at(t: &Target, sub: &str, name: &str, contents: String) -> PlannedFile {
    let path = t.root.join("app").join(sub.replace('\\', "/")).join(format!("{name}.php"));
    t.file(path, contents)
}

fn uses(list: &[&str]) -> String {
    let mut list: Vec<&str> = list.to_vec();
    list.sort_by_key(|u| u.to_ascii_lowercase());
    list.iter().map(|u| format!("use {u};\n")).collect()
}

pub fn resource(t: &Target, model: &str, model_fqn: &str, v3: bool) -> Vec<PlannedFile> {
    let plural_model = {
        let words = snake(model);
        let (head, last) = words.rsplit_once('_').map_or(("", words.as_str()), |(h, l)| (h, l));
        super::studly(&if head.is_empty() { plural(last) } else { format!("{head}_{}", plural(last)) })
    };
    let resource = format!("{model}Resource");
    let list = format!("List{plural_model}");
    let create = format!("Create{model}");
    let edit = format!("Edit{model}");
    let base = t.app("Filament\\Resources");

    let (resource_ns, pages_ns) = if v3 {
        (base.clone(), format!("{base}\\{resource}\\Pages"))
    } else {
        let ns = format!("{base}\\{plural_model}");
        (ns.clone(), format!("{ns}\\Pages"))
    };
    let resource_fqn = format!("{resource_ns}\\{resource}");
    let mut files = Vec::new();

    let pages = format!(
        "    public static function getPages(): array\n    {{\n        return [\n            'index' => {list}::route('/'),\n            'create' => {create}::route('/create'),\n            'edit' => {edit}::route('/{{record}}/edit'),\n        ];\n    }}\n"
    );
    let relations = "    public static function getRelations(): array\n    {\n        return [\n            //\n        ];\n    }\n";
    let resource_src = if v3 {
        let header = uses(&[
            &format!("{pages_ns}"),
            model_fqn,
            "Filament\\Forms",
            "Filament\\Forms\\Form",
            "Filament\\Resources\\Resource",
            "Filament\\Tables",
            "Filament\\Tables\\Table",
        ]);
        format!(
            "<?php\n\nnamespace {resource_ns};\n\n{header}\nclass {resource} extends Resource\n{{\n    protected static ?string $model = {model}::class;\n\n    protected static ?string $navigationIcon = 'heroicon-o-rectangle-stack';\n\n    public static function form(Form $form): Form\n    {{\n        return $form\n            ->schema([\n                //\n            ]);\n    }}\n\n    public static function table(Table $table): Table\n    {{\n        return $table\n            ->columns([\n                //\n            ])\n            ->filters([\n                //\n            ])\n            ->actions([\n                Tables\\Actions\\EditAction::make(),\n            ])\n            ->bulkActions([\n                Tables\\Actions\\BulkActionGroup::make([\n                    Tables\\Actions\\DeleteBulkAction::make(),\n                ]),\n            ]);\n    }}\n\n{relations}\n{pages}}}\n"
        )
        .replace(&format!("{list}::"), &format!("Pages\\{list}::"))
        .replace(&format!("{create}::"), &format!("Pages\\{create}::"))
        .replace(&format!("{edit}::"), &format!("Pages\\{edit}::"))
    } else {
        let form = format!("{model}Form");
        let table = format!("{plural_model}Table");
        let header = uses(&[
            &format!("{pages_ns}\\{create}"),
            &format!("{pages_ns}\\{edit}"),
            &format!("{pages_ns}\\{list}"),
            &format!("{resource_ns}\\Schemas\\{form}"),
            &format!("{resource_ns}\\Tables\\{table}"),
            model_fqn,
            "BackedEnum",
            "Filament\\Resources\\Resource",
            "Filament\\Schemas\\Schema",
            "Filament\\Support\\Icons\\Heroicon",
            "Filament\\Tables\\Table",
        ]);
        files.push(class_at(
            t,
            &format!("Filament\\Resources\\{plural_model}\\Schemas"),
            &form,
            format!(
                "<?php\n\nnamespace {resource_ns}\\Schemas;\n\nuse Filament\\Schemas\\Schema;\n\nclass {form}\n{{\n    public static function configure(Schema $schema): Schema\n    {{\n        return $schema\n            ->components([\n                //\n            ]);\n    }}\n}}\n"
            ),
        ));
        files.push(class_at(
            t,
            &format!("Filament\\Resources\\{plural_model}\\Tables"),
            &table,
            format!(
                "<?php\n\nnamespace {resource_ns}\\Tables;\n\n{}\nclass {table}\n{{\n    public static function configure(Table $table): Table\n    {{\n        return $table\n            ->columns([\n                //\n            ])\n            ->filters([\n                //\n            ])\n            ->recordActions([\n                EditAction::make(),\n            ])\n            ->toolbarActions([\n                BulkActionGroup::make([\n                    DeleteBulkAction::make(),\n                ]),\n            ]);\n    }}\n}}\n",
                uses(&["Filament\\Actions\\BulkActionGroup", "Filament\\Actions\\DeleteBulkAction", "Filament\\Actions\\EditAction", "Filament\\Tables\\Table"])
            ),
        ));
        format!(
            "<?php\n\nnamespace {resource_ns};\n\n{header}\nclass {resource} extends Resource\n{{\n    protected static ?string $model = {model}::class;\n\n    protected static string|BackedEnum|null $navigationIcon = Heroicon::OutlinedRectangleStack;\n\n    public static function form(Schema $schema): Schema\n    {{\n        return {form}::configure($schema);\n    }}\n\n    public static function table(Table $table): Table\n    {{\n        return {table}::configure($table);\n    }}\n\n{relations}\n{pages}}}\n"
        )
    };
    let resource_dir = if v3 { "Filament\\Resources".to_string() } else { format!("Filament\\Resources\\{plural_model}") };
    files.insert(0, class_at(t, &resource_dir, &resource, resource_src));

    let pages_dir = format!("{resource_dir}\\{}", if v3 { format!("{resource}\\Pages") } else { "Pages".into() });
    let page = |name: &str, base: &str, actions: Option<&str>| {
        let (action_use, body) = match actions {
            Some(action) => (
                format!("use Filament\\Actions\\{action};\n"),
                format!("\n    protected function getHeaderActions(): array\n    {{\n        return [\n            {action}::make(),\n        ];\n    }}\n"),
            ),
            None => (String::new(), String::new()),
        };
        let base_short = base.rsplit('\\').next().unwrap();
        format!(
            "<?php\n\nnamespace {pages_ns};\n\nuse {resource_fqn};\n{action_use}use {base};\n\nclass {name} extends {base_short}\n{{\n    protected static string $resource = {resource}::class;\n{body}}}\n"
        )
    };
    files.push(class_at(t, &pages_dir, &list, page(&list, "Filament\\Resources\\Pages\\ListRecords", Some("CreateAction"))));
    files.push(class_at(t, &pages_dir, &create, page(&create, "Filament\\Resources\\Pages\\CreateRecord", None)));
    files.push(class_at(t, &pages_dir, &edit, page(&edit, "Filament\\Resources\\Pages\\EditRecord", Some("DeleteAction"))));
    files
}

fn view_file(t: &Target, view: &str, contents: &str) -> PlannedFile {
    let path = t.root.join("resources/views").join(format!("{}.blade.php", view.replace('.', "/")));
    t.file(path, contents.to_string())
}

fn kebab(name: &str) -> String {
    snake(name).replace('_', "-")
}

pub fn page(t: &Target, class: &ClassName, v3: bool) -> Vec<PlannedFile> {
    let ns = class.namespace(&t.app("Filament\\Pages"));
    let name = &class.name;
    let view = format!("filament.pages.{}", kebab(name));
    let modifier = if v3 { "static " } else { "" };
    vec![
        t.file(
            class.path(&t.root, "app/Filament/Pages"),
            format!("<?php\n\nnamespace {ns};\n\nuse Filament\\Pages\\Page;\n\nclass {name} extends Page\n{{\n    protected {modifier}string $view = '{view}';\n}}\n"),
        ),
        view_file(t, &view, "<x-filament-panels::page>\n    {{-- --}}\n</x-filament-panels::page>\n"),
    ]
}

pub fn widget(t: &Target, class: &ClassName, v3: bool) -> Vec<PlannedFile> {
    let ns = class.namespace(&t.app("Filament\\Widgets"));
    let name = &class.name;
    let view = format!("filament.widgets.{}", kebab(name));
    let modifier = if v3 { "static " } else { "" };
    vec![
        t.file(
            class.path(&t.root, "app/Filament/Widgets"),
            format!("<?php\n\nnamespace {ns};\n\nuse Filament\\Widgets\\Widget;\n\nclass {name} extends Widget\n{{\n    protected {modifier}string $view = '{view}';\n}}\n"),
        ),
        view_file(t, &view, "<x-filament-widgets::widget>\n    <x-filament::section>\n        {{-- --}}\n    </x-filament::section>\n</x-filament-widgets::widget>\n"),
    ]
}
