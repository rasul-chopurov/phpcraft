//! Framework-aware string completion and navigation: validation rules, middleware aliases,
//! Gate/policy abilities, Filament field names (model columns and relations) and Blade
//! directives.

use std::collections::HashMap;
use std::fs;
use std::ops::Range;
use std::path::Path;

use php_analysis::{Callee, CompletionItem, CompletionKind, StringArg, StringProvider};
use php_index::{Index, Location, MemberKind, short_name};

use crate::helper::ModelHelp;

/// Laravel validation rules: (name, takes parameters, description).
pub const RULES: &[(&str, bool, &str)] = &[
    ("accepted", false, "must be yes, on, 1 or true"), ("accepted_if", true, "accepted if another field equals a value"),
    ("active_url", false, "valid A/AAAA DNS record"), ("after", true, "date after the given date"),
    ("after_or_equal", true, "date after or equal"), ("alpha", false, "alphabetic characters"),
    ("alpha_dash", false, "letters, numbers, dashes, underscores"), ("alpha_num", false, "letters and numbers"),
    ("array", false, "PHP array"), ("ascii", false, "7-bit ASCII"), ("bail", false, "stop on first failure"),
    ("before", true, "date before the given date"), ("before_or_equal", true, "date before or equal"),
    ("between", true, "size between min and max"), ("boolean", false, "castable to boolean"),
    ("confirmed", false, "matching {field}_confirmation"), ("contains", true, "array contains values"),
    ("current_password", false, "authenticated user's password"), ("date", false, "valid date"),
    ("date_equals", true, "date equal to"), ("date_format", true, "matches the format"), ("decimal", true, "decimal places"),
    ("declined", false, "no, off, 0 or false"), ("different", true, "different from field"),
    ("digits", true, "exact number of digits"), ("digits_between", true, "digits between min and max"),
    ("dimensions", true, "image dimensions"), ("distinct", false, "no duplicates in array"),
    ("doesnt_start_with", true, "must not start with"), ("doesnt_end_with", true, "must not end with"),
    ("email", false, "e-mail address"), ("ends_with", true, "must end with"), ("enum", true, "valid enum value"),
    ("exclude", false, "exclude from validated data"), ("exclude_if", true, "exclude if"), ("exclude_unless", true, "exclude unless"),
    ("exists", true, "exists in a database table"), ("extensions", true, "file extensions"), ("file", false, "uploaded file"),
    ("filled", false, "not empty when present"), ("gt", true, "greater than field"), ("gte", true, "greater than or equal"),
    ("hex_color", false, "hexadecimal color"), ("image", false, "image file"), ("in", true, "in the list of values"),
    ("in_array", true, "exists in another field's values"), ("integer", false, "integer"), ("ip", false, "IP address"),
    ("ipv4", false, "IPv4 address"), ("ipv6", false, "IPv6 address"), ("json", false, "valid JSON string"),
    ("list", false, "list array"), ("lowercase", false, "lowercase"), ("lt", true, "less than field"),
    ("lte", true, "less than or equal"), ("mac_address", false, "MAC address"), ("max", true, "maximum size"),
    ("max_digits", true, "maximum digits"), ("mimetypes", true, "MIME types"), ("mimes", true, "file extensions by MIME"),
    ("min", true, "minimum size"), ("min_digits", true, "minimum digits"), ("missing", false, "must not be present"),
    ("multiple_of", true, "multiple of a value"), ("not_in", true, "not in the list"), ("not_regex", true, "must not match"),
    ("nullable", false, "may be null"), ("numeric", false, "numeric"), ("present", false, "must be present"),
    ("prohibited", false, "must be missing or empty"), ("prohibited_if", true, "prohibited if"),
    ("regex", true, "matches the pattern"), ("required", false, "must be present and not empty"),
    ("required_if", true, "required if another field equals"), ("required_unless", true, "required unless"),
    ("required_with", true, "required with other fields"), ("required_with_all", true, "required with all fields"),
    ("required_without", true, "required without other fields"), ("required_array_keys", true, "array must contain keys"),
    ("same", true, "same as field"), ("size", true, "exact size"), ("sometimes", false, "validate only when present"),
    ("starts_with", true, "must start with"), ("string", false, "string"), ("timezone", false, "valid timezone"),
    ("unique", true, "unique in a database table"), ("uppercase", false, "uppercase"), ("url", false, "valid URL"),
    ("ulid", false, "ULID"), ("uuid", false, "UUID"),
];

/// Middleware aliases registered by the framework itself.
const DEFAULT_MIDDLEWARE: &[(&str, &str)] = &[
    ("auth", "Illuminate\\Auth\\Middleware\\Authenticate"),
    ("auth.basic", "Illuminate\\Auth\\Middleware\\AuthenticateWithBasicAuth"),
    ("auth.session", "Illuminate\\Session\\Middleware\\AuthenticateSession"),
    ("cache.headers", "Illuminate\\Http\\Middleware\\SetCacheHeaders"),
    ("can", "Illuminate\\Auth\\Middleware\\Authorize"),
    ("guest", "Illuminate\\Auth\\Middleware\\RedirectIfAuthenticated"),
    ("password.confirm", "Illuminate\\Auth\\Middleware\\RequirePassword"),
    ("precognitive", "Illuminate\\Foundation\\Http\\Middleware\\HandlePrecognitiveRequests"),
    ("signed", "Illuminate\\Routing\\Middleware\\ValidateSignature"),
    ("throttle", "Illuminate\\Routing\\Middleware\\ThrottleRequests"),
    ("verified", "Illuminate\\Auth\\Middleware\\EnsureEmailIsVerified"),
    ("web", ""),
    ("api", ""),
];

/// Filament components whose `make('name')` names a model attribute.
const FILAMENT_PREFIXES: &[&str] = &["Filament\\Forms\\Components\\", "Filament\\Tables\\Columns\\", "Filament\\Infolists\\Components\\", "Filament\\Tables\\Filters\\"];

pub const BLADE_DIRECTIVES: &[(&str, &str)] = &[
    ("if", "@if()\n@endif"), ("elseif", "@elseif()"), ("else", "@else"), ("endif", "@endif"), ("unless", "@unless()\n@endunless"),
    ("isset", "@isset()\n@endisset"), ("empty", "@empty()\n@endempty"), ("auth", "@auth\n@endauth"), ("guest", "@guest\n@endguest"),
    ("production", "@production\n@endproduction"), ("env", "@env('')\n@endenv"), ("switch", "@switch()\n@endswitch"),
    ("case", "@case()"), ("break", "@break"), ("for", "@for()\n@endfor"), ("foreach", "@foreach( as )\n@endforeach"),
    ("forelse", "@forelse( as )\n@empty\n@endforelse"), ("while", "@while()\n@endwhile"), ("continue", "@continue"),
    ("php", "@php\n@endphp"), ("include", "@include('')"), ("includeIf", "@includeIf('')"), ("includeWhen", "@includeWhen(, '')"),
    ("includeFirst", "@includeFirst([])"), ("each", "@each('', , '')"), ("extends", "@extends('')"), ("section", "@section('')\n@endsection"),
    ("yield", "@yield('')"), ("show", "@show"), ("parent", "@parent"), ("stack", "@stack('')"), ("push", "@push('')\n@endpush"),
    ("prepend", "@prepend('')\n@endprepend"), ("once", "@once\n@endonce"), ("csrf", "@csrf"), ("method", "@method('')"),
    ("error", "@error('')\n@enderror"), ("can", "@can('')\n@endcan"), ("cannot", "@cannot('')\n@endcannot"),
    ("canany", "@canany([])\n@endcanany"), ("json", "@json()"), ("lang", "@lang('')"), ("verbatim", "@verbatim\n@endverbatim"),
    ("props", "@props([])"), ("aware", "@aware([])"), ("class", "@class([])"), ("style", "@style([])"), ("checked", "@checked()"),
    ("selected", "@selected()"), ("disabled", "@disabled()"), ("readonly", "@readonly()"), ("required", "@required()"),
    ("vite", "@vite([])"), ("livewireStyles", "@livewireStyles"), ("livewireScripts", "@livewireScripts"), ("livewire", "@livewire('')"),
    ("inertia", "@inertia"), ("inertiaHead", "@inertiaHead"), ("session", "@session('')\n@endsession"), ("fragment", "@fragment('')\n@endfragment"),
    ("use", "@use('')"), ("dump", "@dump()"), ("dd", "@dd()"), ("hasSection", "@hasSection('')\n@endif"),
];

#[derive(Default)]
struct ModelData {
    fqn: String,
    /// (column, type, declaration)
    columns: Vec<(String, String, Option<Location>)>,
    relations: Vec<(String, Location)>,
}

#[derive(Default)]
pub struct FrameworkProvider {
    middleware: Vec<(String, Option<Location>)>,
    abilities: Vec<(String, Option<Location>)>,
    /// Lowercased short model name -> data.
    models: HashMap<String, ModelData>,
}

fn string_after(source: &str, from: usize) -> Option<(String, Range<usize>)> {
    let rest = &source[from..];
    let start = from + rest.find(['\'', '"'])?;
    let quote = source[start..].chars().next()?;
    let end = source[start + 1..].find(quote)? + start + 1;
    Some((source[start + 1..end].to_string(), start + 1..end))
}

impl FrameworkProvider {
    pub fn new(root: &Path, index: &Index, helpers: &[ModelHelp]) -> Self {
        let mut provider = Self::default();
        provider.load_middleware(root, index);
        provider.load_abilities(root, index);
        for help in helpers {
            let Some(class) = index.class(&help.fqn) else { continue };
            let mut data = ModelData { fqn: help.fqn.clone(), ..Default::default() };
            for member in help.members.iter().filter(|m| m.kind == MemberKind::Property) {
                match &member.source {
                    Some(path) => data.columns.push((
                        member.name.clone(),
                        member.detail.clone(),
                        Some(Location { path: path.to_path_buf(), range: member.range.clone() }),
                    )),
                    None if !member.name.ends_with("_count") => data
                        .relations
                        .push((member.name.clone(), Location { path: class.path.to_path_buf(), range: member.range.clone() })),
                    None => {}
                }
            }
            provider.models.insert(class.name.to_ascii_lowercase(), data);
        }
        provider
    }

    fn load_middleware(&mut self, root: &Path, index: &Index) {
        let mut found: Vec<(String, Option<Location>)> = DEFAULT_MIDDLEWARE
            .iter()
            .map(|(alias, class)| (alias.to_string(), (!class.is_empty()).then(|| index.class_location(class)).flatten()))
            .collect();
        // Laravel 11+: ->withMiddleware(fn ($m) => $m->alias([...])); older: Kernel::$middlewareAliases.
        for file in [root.join("bootstrap/app.php"), root.join("app/Http/Kernel.php")] {
            let Ok(source) = fs::read_to_string(&file) else { continue };
            for marker in ["->alias([", "$middlewareAliases = [", "$routeMiddleware = [", "->group('", "->appendToGroup('"] {
                let mut search = 0;
                while let Some(at) = source[search..].find(marker).map(|i| search + i) {
                    search = at + marker.len();
                    if marker.ends_with("('") {
                        if let Some((name, range)) = string_after(&source, at + marker.len() - 1) {
                            found.push((name, Some(Location { path: file.clone(), range })));
                        }
                        continue;
                    }
                    let end = source[search..].find(']').map_or(source.len(), |i| search + i);
                    for line in source[search..end].lines() {
                        let Some((_, value)) = line.split_once("=>") else { continue };
                        let Some((alias, range)) = string_after(line, 0) else { continue };
                        let class = value.trim().trim_end_matches(',').trim_end_matches("::class").trim_start_matches('\\');
                        let line_start = source[search..end].find(line).map_or(search, |i| search + i);
                        let location = index
                            .class_location(class)
                            .or(Some(Location { path: file.clone(), range: line_start + range.start..line_start + range.end }));
                        found.push((alias, location));
                    }
                }
            }
        }
        found.sort_by(|a, b| a.0.cmp(&b.0));
        found.dedup_by(|a, b| a.0 == b.0);
        self.middleware = found;
    }

    fn load_abilities(&mut self, root: &Path, index: &Index) {
        let mut found = Vec::new();
        let mut files = Vec::new();
        crate::walk_dir(&root.join("app/Providers"), &mut files);
        for file in files {
            let Ok(source) = fs::read_to_string(&file) else { continue };
            let mut search = 0;
            while let Some(at) = source[search..].find("Gate::define(").map(|i| search + i) {
                search = at + 13;
                if let Some((name, range)) = string_after(&source, at + 12) {
                    found.push((name, Some(Location { path: file.clone(), range })));
                }
            }
        }
        // Policy methods (viewAny, update, ...) are abilities too.
        let policies = root.join("app/Policies");
        for class in index.classes().filter(|c| c.path.starts_with(&policies)) {
            for member in class.members.iter().filter(|m| m.kind == MemberKind::Method && !m.name.starts_with("__")) {
                if !found.iter().any(|(n, _)| *n == member.name) {
                    found.push((member.name.clone(), Some(Location { path: class.path.to_path_buf(), range: member.range.clone() })));
                }
            }
        }
        found.sort_by(|a, b| a.0.cmp(&b.0));
        self.abilities = found;
    }

    fn is_rules_context(arg: &StringArg) -> bool {
        let in_rules_method = arg.enclosing_method.as_deref() == Some("rules");
        let validating_call = arg.is_method(&["validate", "validateWithBag", "sometimes"])
            || arg.is_static(&["Validator"], &["make", "validate"])
            || arg.is_function(&["validator"]);
        arg.in_array && arg.array_key.is_some() && (in_rules_method || validating_call)
    }

    fn is_middleware_context(arg: &StringArg) -> bool {
        arg.is_method(&["middleware", "withoutMiddleware"]) || arg.is_static(&["Route"], &["middleware", "withoutMiddleware"])
    }

    fn is_ability_context(arg: &StringArg) -> bool {
        arg.arg_index == 0
            && !arg.in_array
            && (matches!(&arg.callee, Callee::Method { .. })
                && arg.is_method(&["can", "cannot", "cant", "authorize", "authorizeForUser"])
                || arg.is_static(&["Gate"], &["allows", "denies", "check", "any", "none", "authorize", "inspect", "has"]))
    }

    /// The model a Filament resource/schema/table class works with.
    fn filament_model(&self, arg: &StringArg) -> Option<&ModelData> {
        if let Some(model) = &arg.class_model {
            return self.models.get(&short_name(model).to_ascii_lowercase());
        }
        let class = short_name(arg.enclosing_class.as_deref()?);
        for suffix in ["Resource", "Form", "Table", "Infolist", "Schema", "RelationManager"] {
            if let Some(base) = class.strip_suffix(suffix).filter(|b| !b.is_empty()) {
                let singular = crate::database::model_name(&crate::make::snake(base));
                for name in [base.to_string(), singular] {
                    if let Some(model) = self.models.get(&name.to_ascii_lowercase()) {
                        return Some(model);
                    }
                }
            }
        }
        None
    }

    fn is_filament_make(arg: &StringArg) -> bool {
        matches!(&arg.callee, Callee::Static { class: Some(class), name }
            if name == "make" && arg.arg_index == 0 && FILAMENT_PREFIXES.iter().any(|p| class.starts_with(p)))
    }

    fn is_filament_relationship(arg: &StringArg) -> bool {
        arg.arg_index == 0 && arg.is_method(&["relationship"]) && arg.enclosing_class.is_some()
    }

    /// The `|`-separated rule segment at the caret (`required|em|max:255`).
    fn rule_segment(arg: &StringArg) -> Range<usize> {
        let caret = arg.offset.clamp(arg.range.start, arg.range.end) - arg.range.start;
        let start = arg.value[..caret].rfind('|').map_or(0, |i| i + 1);
        arg.range.start + start..arg.offset
    }
}

impl StringProvider for FrameworkProvider {
    fn goto(&self, arg: &StringArg) -> Vec<Location> {
        let pick = |list: &[(String, Option<Location>)], name: &str| -> Vec<Location> {
            list.iter().filter(|(n, _)| n == name).filter_map(|(_, l)| l.clone()).collect()
        };
        if Self::is_middleware_context(arg) {
            let alias = arg.value.split(':').next().unwrap_or("");
            return pick(&self.middleware, alias);
        }
        if Self::is_ability_context(arg) {
            return pick(&self.abilities, &arg.value);
        }
        if (Self::is_filament_make(arg) || Self::is_filament_relationship(arg))
            && let Some(model) = self.filament_model(arg)
        {
            let first = arg.value.split('.').next().unwrap_or("");
            let column = model.columns.iter().find(|(n, _, _)| n == first).and_then(|(_, _, l)| l.clone());
            let relation = model.relations.iter().find(|(n, _)| n == first).map(|(_, l)| l.clone());
            return column.or(relation).into_iter().collect();
        }
        Vec::new()
    }

    fn complete(&self, arg: &StringArg) -> Vec<CompletionItem> {
        let mut items = Vec::new();
        if Self::is_rules_context(arg) {
            let replace = Self::rule_segment(arg);
            for (rule, params, doc) in RULES {
                let mut item = CompletionItem::simple(*rule, *doc, CompletionKind::Value, replace.clone());
                if *params {
                    item.insert = format!("{rule}:");
                }
                items.push(item);
            }
            return items;
        }
        if Self::is_middleware_context(arg) {
            for (alias, location) in &self.middleware {
                let detail = location.as_ref().map(|l| l.path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default());
                items.push(CompletionItem::simple(alias.clone(), detail.unwrap_or_else(|| "middleware".into()), CompletionKind::Value, arg.range.clone()));
            }
            return items;
        }
        if Self::is_ability_context(arg) {
            for (ability, _) in &self.abilities {
                items.push(CompletionItem::simple(ability.clone(), "ability", CompletionKind::Value, arg.range.clone()));
            }
            return items;
        }
        if let Some(model) = self.filament_model(arg) {
            let model_name = short_name(&model.fqn).to_string();
            if Self::is_filament_make(arg) {
                for (column, ty, _) in &model.columns {
                    items.push(CompletionItem::simple(column.clone(), format!("{ty} · {model_name}"), CompletionKind::Property, arg.range.clone()));
                }
                // `author.name` style paths into relations.
                for (relation, _) in &model.relations {
                    items.push(CompletionItem::simple(format!("{relation}."), format!("relation · {model_name}"), CompletionKind::Method, arg.range.clone()));
                }
            } else if Self::is_filament_relationship(arg) {
                for (relation, _) in &model.relations {
                    items.push(CompletionItem::simple(relation.clone(), format!("relation · {model_name}"), CompletionKind::Method, arg.range.clone()));
                }
            }
        }
        items
    }
}

/// Blade directive completions for a prefix typed after `@` (`replace` covers the `@`).
pub fn blade_directive_items(replace: Range<usize>) -> Vec<CompletionItem> {
    BLADE_DIRECTIVES
        .iter()
        .map(|(name, snippet)| {
            let mut item = CompletionItem::simple(format!("@{name}"), snippet.replace('\n', " … "), CompletionKind::Keyword, replace.clone());
            item.insert = snippet.to_string();
            item
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use php_analysis::{Document, complete, goto_definition};
    use php_index::Member;

    use super::*;

    fn labels(provider: &FrameworkProvider, index: &Index, path: &Path, src: &str, needle: &str, delta: usize) -> Vec<String> {
        let doc = Document::parse(path, src).unwrap();
        let offset = src.find(needle).unwrap_or_else(|| panic!("{needle}")) + delta;
        complete(&doc, index, offset, &[provider]).into_iter().map(|i| i.label).collect()
    }

    #[test]
    fn framework_completions() {
        let root = std::env::temp_dir().join(format!("phpcraft-framework-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        for (path, text) in [
            ("bootstrap/app.php", "<?php\nreturn Application::configure()->withMiddleware(function ($m) {\n    $m->alias([\n        'admin' => \\App\\Http\\Middleware\\EnsureAdmin::class,\n    ]);\n});\n"),
            ("app/Providers/AppServiceProvider.php", "<?php\nGate::define('publish-posts', fn () => true);\n"),
            ("app/Policies/PostPolicy.php", "<?php\nnamespace App\\Policies;\nclass PostPolicy { public function update($u, $p) {} }\n"),
        ] {
            fs::create_dir_all(root.join(path).parent().unwrap()).unwrap();
            fs::write(root.join(path), text).unwrap();
        }
        let mut index = Index::new();
        index.update_file(&root.join("app/Policies/PostPolicy.php"), &fs::read_to_string(root.join("app/Policies/PostPolicy.php")).unwrap());
        index.update_file(Path::new("/m/Post.php"), "<?php\nnamespace App\\Models;\nclass Post {}\n");
        let column = |name: &str| Member {
            name: name.into(),
            kind: MemberKind::Property,
            is_static: false,
            range: 0..0,
            type_fqn: None,
            detail: "string".into(),
            virtual_member: true,
            source: Some(Arc::from(Path::new("/m/migration.php"))),
        };
        let mut author = column("author");
        author.source = None;
        let helpers = vec![ModelHelp { fqn: "App\\Models\\Post".into(), members: vec![column("title"), column("slug"), author] }];
        let provider = FrameworkProvider::new(&root, &index, &helpers);

        let request = "<?php\nclass StorePostRequest {\n  public function rules(): array {\n    return [\n      'title' => 'required|ma',\n      'tags' => ['array', 'nu'],\n    ];\n  }\n}\n";
        let path = root.join("app/Http/Requests/StorePostRequest.php");
        let rules = labels(&provider, &index, &path, request, "|ma'", 3);
        assert_eq!(rules.first().map(String::as_str), Some("max"), "{rules:?}");
        assert!(labels(&provider, &index, &path, request, "'nu'", 3).contains(&"nullable".to_string()));
        assert!(labels(&provider, &index, &path, request, "'title'", 2).is_empty(), "keys are not rules");

        let routes = "<?php\nRoute::get('/', fn () => 1)->middleware(['auth', 'ad']);\nGate::allows('pu');\n$this->authorize('up');\nGate::denies('publish-posts');\n";
        let path = root.join("routes/web.php");
        assert_eq!(labels(&provider, &index, &path, routes, "'ad'", 3), ["admin"]);
        assert_eq!(labels(&provider, &index, &path, routes, "'pu'", 3), ["publish-posts"]);
        assert_eq!(labels(&provider, &index, &path, routes, "'up'", 3), ["update"]);
        let doc = Document::parse(&path, routes).unwrap();
        let found = goto_definition(&doc, &index, routes.find("'publish-posts'").unwrap() + 2, &[&provider]);
        assert!(found[0].path.ends_with("app/Providers/AppServiceProvider.php"));

        let form = "<?php\nnamespace App\\Filament\\Resources\\Posts\\Schemas;\nuse Filament\\Forms\\Components\\TextInput;\nuse Filament\\Forms\\Components\\Select;\nclass PostForm {\n  public static function configure($schema) {\n    return $schema->components([\n      TextInput::make('ti'),\n      Select::make('author_id')->relationship('au', 'name'),\n    ]);\n  }\n}\n";
        let path = root.join("app/Filament/Resources/Posts/Schemas/PostForm.php");
        assert_eq!(labels(&provider, &index, &path, form, "'ti'", 3), ["title"]);
        assert_eq!(labels(&provider, &index, &path, form, "'au'", 3), ["author"]);
        fs::remove_dir_all(&root).unwrap();
    }
}
