//! Laravel Idea's "All Code Generations": every generator by name. Rich ones (model,
//! controller, migration…) open the Make dialog; the rest create files from a name here.

use std::path::{Path, PathBuf};

use crate::make::MakeKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Generator {
    /// Handled by the Make dialog.
    Make(MakeKind),
    Simple(Simple),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Simple {
    BladeComponent,
    BroadcastingChannel,
    Class,
    ConfigFile,
    Cast,
    Dto,
    NovaResource,
    Scope,
    Exception,
    Facade,
    HttpException,
    JsonResourceCollection,
    LivewireClass,
    LivewireForm,
    LivewireMultiFile,
    LivewireSingleFile,
    Mailable,
    Notification,
    Service,
    ServiceProvider,
    Trait,
    ValidationRule,
    View,
    ViewComposer,
}

/// `(label, generator)` in Laravel Idea's order.
pub fn all() -> Vec<(&'static str, Generator)> {
    use Generator::{Make, Simple as S};
    use Simple::*;
    vec![
        ("Create Blade Component", S(BladeComponent)),
        ("Create Broadcasting Channel", S(BroadcastingChannel)),
        ("Create Class", S(Class)),
        ("Create Config File", S(ConfigFile)),
        ("Create Console Command", Make(MakeKind::Command)),
        ("Create Controller", Make(MakeKind::Controller)),
        ("Create Database Class Factory", Make(MakeKind::Factory)),
        ("Create Database Migration", Make(MakeKind::Migration)),
        ("Create Database Seeder", Make(MakeKind::Seeder)),
        ("Create DTO", S(Dto)),
        ("Create Eloquent Cast", S(Cast)),
        ("Create Eloquent Scope", S(Scope)),
        ("Create Enum", Make(MakeKind::Enum)),
        ("Create Event", Make(MakeKind::Event)),
        ("Create Event Listener", Make(MakeKind::Listener)),
        ("Create Exception", S(Exception)),
        ("Create Facade", S(Facade)),
        ("Create Feature Test", Make(MakeKind::Test)),
        ("Create Filament Page", Make(MakeKind::FilamentPage)),
        ("Create Filament Resource", Make(MakeKind::FilamentResource)),
        ("Create Filament Widget", Make(MakeKind::FilamentWidget)),
        ("Create Form Request", Make(MakeKind::Request)),
        ("Create HTTP Exception", S(HttpException)),
        ("Create Job", Make(MakeKind::Job)),
        ("Create Json Resource", Make(MakeKind::Resource)),
        ("Create Json Resource Collection", S(JsonResourceCollection)),
        ("Create Livewire Class Component", S(LivewireClass)),
        ("Create Livewire Form", S(LivewireForm)),
        ("Create Livewire Multi File Component", S(LivewireMultiFile)),
        ("Create Livewire Single File Component", S(LivewireSingleFile)),
        ("Create Mailable", S(Mailable)),
        ("Create Middleware", Make(MakeKind::Middleware)),
        ("Create Model", Make(MakeKind::Model)),
        ("Create Notification", S(Notification)),
        ("Create Nova Resource", S(NovaResource)),
        ("Create Observer", Make(MakeKind::Observer)),
        ("Create Pivot Table", Make(MakeKind::PivotTable)),
        ("Create Policy", Make(MakeKind::Policy)),
        ("Create Resource Controller", Make(MakeKind::Controller)),
        ("Create Service", S(Service)),
        ("Create Service Provider", S(ServiceProvider)),
        ("Create Trait", S(Trait)),
        ("Create Unit Test", Make(MakeKind::Test)),
        ("Create Validation Rule", S(ValidationRule)),
        ("Create View", S(View)),
        ("Create View Composer", S(ViewComposer)),
    ]
}

fn studly(s: &str) -> String {
    crate::make::studly(s)
}

fn kebab(name: &str) -> String {
    let mut out = String::new();
    for (i, c) in name.chars().enumerate() {
        if c.is_uppercase() && i > 0 && !out.ends_with(['-', '.', '/']) {
            out.push('-');
        }
        out.extend(c.to_lowercase());
    }
    out.replace('_', "-")
}

/// Where code goes: the app or a laravel-modules module (`Modules/Blog[/app]`).
struct Base {
    /// Folder that maps to the base namespace (`app/`, `Modules/Blog/app/`).
    code: PathBuf,
    namespace: String,
    /// Folder holding `resources/` and `config/`.
    root: PathBuf,
}

fn base(root: &Path, module: Option<&str>) -> Base {
    match module {
        Some(module) => {
            let dir = root.join("Modules").join(module);
            let code = if dir.join("app").is_dir() { dir.join("app") } else { dir.clone() };
            Base { code, namespace: format!("Modules\\{module}"), root: dir }
        }
        None => Base { code: root.join("app"), namespace: "App".into(), root: root.to_path_buf() },
    }
}

/// `Admin/Report` -> (`Admin`, `Report`).
fn split(name: &str) -> (String, String) {
    let parts: Vec<String> = name.trim().trim_end_matches(".php").split(['/', '\\']).filter(|p| !p.is_empty()).map(studly).collect();
    let class = parts.last().cloned().unwrap_or_default();
    (parts[..parts.len().saturating_sub(1)].join("\\"), class)
}

fn class_file(base: &Base, folder: &str, name: &str, body: impl Fn(&str, &str) -> String) -> (PathBuf, String) {
    let (sub, class) = split(name);
    let mut namespace = format!("{}\\{}", base.namespace, folder.replace('/', "\\"));
    let mut dir = base.code.join(folder);
    if !sub.is_empty() {
        namespace = format!("{namespace}\\{sub}");
        dir = dir.join(sub.replace('\\', "/"));
    }
    let namespace = namespace.trim_end_matches('\\').replace("\\\\", "\\");
    (dir.join(format!("{class}.php")), body(&namespace, &class))
}

/// Files for a simple generator named `name` (first is opened).
pub fn plan(kind: Simple, name: &str, root: &Path, module: Option<&str>) -> Vec<(PathBuf, String)> {
    let b = base(root, module);
    let php = |ns: &str, uses: &str, decl: &str, body: &str| format!("<?php\n\nnamespace {ns};\n\n{uses}{decl}\n{{\n{body}}}\n");
    let uses = |list: &[&str]| if list.is_empty() { String::new() } else { format!("{}\n\n", list.iter().map(|u| format!("use {u};")).collect::<Vec<_>>().join("\n")) };
    let (_, class) = split(name);
    match kind {
        Simple::Class => vec![class_file(&b, "", name, |ns, c| php(ns, "", &format!("class {c}"), ""))],
        Simple::Service => vec![class_file(&b, "Services", name, |ns, c| php(ns, "", &format!("class {c}"), ""))],
        Simple::Dto => vec![class_file(&b, "Data", name, |ns, c| {
            php(
                ns,
                "",
                &format!("final readonly class {c}"),
                &format!("    public function __construct(\n        //\n    ) {{}}\n\n    public static function fromArray(array $data): self\n    {{\n        return new self(...$data);\n    }}\n"),
            )
        })],
        Simple::NovaResource => vec![class_file(&b, "Nova", name, |ns, c| {
            php(
                ns,
                &uses(&["Laravel\\Nova\\Fields\\ID", "Laravel\\Nova\\Http\\Requests\\NovaRequest"]),
                &format!("class {c} extends Resource"),
                &format!("    public static $model = \\App\\Models\\{c}::class;\n\n    public static $title = 'id';\n\n    public static $search = ['id'];\n\n    public function fields(NovaRequest $request): array\n    {{\n        return [\n            ID::make()->sortable(),\n        ];\n    }}\n"),
            )
        })],
        Simple::Trait => vec![class_file(&b, "Traits", name, |ns, c| php(ns, "", &format!("trait {c}"), ""))],
        Simple::Exception => vec![class_file(&b, "Exceptions", name, |ns, c| php(ns, &uses(&["Exception"]), &format!("class {c} extends Exception"), ""))],
        Simple::HttpException => vec![class_file(&b, "Exceptions", name, |ns, c| {
            php(ns, &uses(&["Symfony\\Component\\HttpKernel\\Exception\\HttpException"]), &format!("class {c} extends HttpException"), "    public function __construct(string $message = '')\n    {\n        parent::__construct(400, $message);\n    }\n")
        })],
        Simple::Facade => vec![class_file(&b, "Facades", name, |ns, c| {
            php(ns, &uses(&["Illuminate\\Support\\Facades\\Facade"]), &format!("class {c} extends Facade"), &format!("    protected static function getFacadeAccessor(): string\n    {{\n        return '{}';\n    }}\n", kebab(c)))
        })],
        Simple::Cast => vec![class_file(&b, "Casts", name, |ns, c| {
            php(
                ns,
                &uses(&["Illuminate\\Contracts\\Database\\Eloquent\\CastsAttributes", "Illuminate\\Database\\Eloquent\\Model"]),
                &format!("class {c} implements CastsAttributes"),
                "    public function get(Model $model, string $key, mixed $value, array $attributes): mixed\n    {\n        return $value;\n    }\n\n    public function set(Model $model, string $key, mixed $value, array $attributes): mixed\n    {\n        return $value;\n    }\n",
            )
        })],
        Simple::Scope => vec![class_file(&b, "Models/Scopes", name, |ns, c| {
            php(
                ns,
                &uses(&["Illuminate\\Database\\Eloquent\\Builder", "Illuminate\\Database\\Eloquent\\Model", "Illuminate\\Database\\Eloquent\\Scope"]),
                &format!("class {c} implements Scope"),
                "    public function apply(Builder $builder, Model $model): void\n    {\n        //\n    }\n",
            )
        })],
        Simple::BroadcastingChannel => vec![class_file(&b, "Broadcasting", name, |ns, c| {
            php(ns, &uses(&["App\\Models\\User"]), &format!("class {c}"), "    public function join(User $user): array|bool\n    {\n        return true;\n    }\n")
        })],
        Simple::JsonResourceCollection => vec![class_file(&b, "Http/Resources", name, |ns, c| {
            php(
                ns,
                &uses(&["Illuminate\\Http\\Request", "Illuminate\\Http\\Resources\\Json\\ResourceCollection"]),
                &format!("class {c} extends ResourceCollection"),
                "    public function toArray(Request $request): array\n    {\n        return parent::toArray($request);\n    }\n",
            )
        })],
        Simple::Mailable => {
            let view = format!("mail.{}", kebab(&class));
            vec![
                class_file(&b, "Mail", name, |ns, c| {
                    php(
                        ns,
                        &uses(&["Illuminate\\Bus\\Queueable", "Illuminate\\Mail\\Mailable", "Illuminate\\Mail\\Mailables\\Content", "Illuminate\\Mail\\Mailables\\Envelope", "Illuminate\\Queue\\SerializesModels"]),
                        &format!("class {c} extends Mailable"),
                        &format!("    use Queueable, SerializesModels;\n\n    public function envelope(): Envelope\n    {{\n        return new Envelope(subject: '{c}');\n    }}\n\n    public function content(): Content\n    {{\n        return new Content(view: '{view}');\n    }}\n"),
                    )
                }),
                (b.root.join("resources/views/mail").join(format!("{}.blade.php", kebab(&class))), "<div>\n    \n</div>\n".into()),
            ]
        }
        Simple::Notification => vec![class_file(&b, "Notifications", name, |ns, c| {
            php(
                ns,
                &uses(&["Illuminate\\Bus\\Queueable", "Illuminate\\Notifications\\Messages\\MailMessage", "Illuminate\\Notifications\\Notification"]),
                &format!("class {c} extends Notification"),
                "    use Queueable;\n\n    public function via(object $notifiable): array\n    {\n        return ['mail'];\n    }\n\n    public function toMail(object $notifiable): MailMessage\n    {\n        return (new MailMessage)->line('…');\n    }\n",
            )
        })],
        Simple::ServiceProvider => vec![class_file(&b, "Providers", name, |ns, c| {
            php(ns, &uses(&["Illuminate\\Support\\ServiceProvider"]), &format!("class {c} extends ServiceProvider"), "    public function register(): void\n    {\n        //\n    }\n\n    public function boot(): void\n    {\n        //\n    }\n")
        })],
        Simple::ValidationRule => vec![class_file(&b, "Rules", name, |ns, c| {
            php(
                ns,
                &uses(&["Closure", "Illuminate\\Contracts\\Validation\\ValidationRule"]),
                &format!("class {c} implements ValidationRule"),
                "    public function validate(string $attribute, mixed $value, Closure $fail): void\n    {\n        //\n    }\n",
            )
        })],
        Simple::ViewComposer => vec![class_file(&b, "View/Composers", name, |ns, c| {
            php(ns, &uses(&["Illuminate\\View\\View"]), &format!("class {c}"), "    public function compose(View $view): void\n    {\n        //\n    }\n")
        })],
        Simple::BladeComponent => {
            let view = kebab(&name.replace('\\', "/")).replace('/', ".");
            vec![
                class_file(&b, "View/Components", name, |ns, c| {
                    php(
                        ns,
                        &uses(&["Closure", "Illuminate\\Contracts\\View\\View", "Illuminate\\View\\Component"]),
                        &format!("class {c} extends Component"),
                        &format!("    public function render(): View|Closure|string\n    {{\n        return view('components.{view}');\n    }}\n"),
                    )
                }),
                (b.root.join("resources/views/components").join(format!("{}.blade.php", view.replace('.', "/"))), "<div>\n    {{ $slot }}\n</div>\n".into()),
            ]
        }
        Simple::LivewireClass => {
            let view = kebab(&name.replace('\\', "/")).replace('/', ".");
            vec![
                class_file(&b, "Livewire", name, |ns, c| {
                    php(ns, &uses(&["Livewire\\Component"]), &format!("class {c} extends Component"), &format!("    public function render()\n    {{\n        return view('livewire.{view}');\n    }}\n"))
                }),
                (b.root.join("resources/views/livewire").join(format!("{}.blade.php", view.replace('.', "/"))), "<div>\n    {{-- … --}}\n</div>\n".into()),
            ]
        }
        Simple::LivewireForm => vec![class_file(&b, "Livewire/Forms", name, |ns, c| php(ns, &uses(&["Livewire\\Form"]), &format!("class {c} extends Form"), "    //\n"))],
        Simple::LivewireSingleFile => {
            let view = kebab(&name.replace('\\', "/"));
            vec![(
                b.root.join("resources/views/livewire").join(format!("{view}.blade.php")),
                "<?php\n\nuse Livewire\\Component;\n\nnew class extends Component {\n    //\n};\n?>\n\n<div>\n    {{-- … --}}\n</div>\n".into(),
            )]
        }
        Simple::LivewireMultiFile => {
            let view = kebab(&name.replace('\\', "/"));
            let short = view.rsplit('/').next().unwrap_or(&view).to_string();
            let dir = b.root.join("resources/views/livewire").join(&view);
            vec![
                (dir.join(format!("{short}.php")), "<?php\n\nuse Livewire\\Component;\n\nnew class extends Component {\n    //\n};\n".into()),
                (dir.join(format!("{short}.blade.php")), "<div>\n    {{-- … --}}\n</div>\n".into()),
            ]
        }
        Simple::View => {
            let view = name.trim().trim_end_matches(".blade.php").replace('.', "/");
            vec![(b.root.join("resources/views").join(format!("{view}.blade.php")), "<div>\n    \n</div>\n".into())]
        }
        Simple::ConfigFile => {
            let file = name.trim().trim_end_matches(".php").to_lowercase().replace(' ', "_");
            vec![(b.root.join("config").join(format!("{file}.php")), "<?php\n\nreturn [\n    //\n];\n".into())]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plans_files() {
        let root = Path::new("/p");
        let files = plan(Simple::Cast, "Money", root, None);
        assert_eq!(files[0].0, Path::new("/p/app/Casts/Money.php"));
        assert!(files[0].1.contains("namespace App\\Casts;") && files[0].1.contains("class Money implements CastsAttributes"));
        let files = plan(Simple::BladeComponent, "Forms/TextInput", root, None);
        assert_eq!(files[0].0, Path::new("/p/app/View/Components/Forms/TextInput.php"));
        assert!(files[0].1.contains("namespace App\\View\\Components\\Forms;"));
        assert!(files[0].1.contains("view('components.forms.text-input')"));
        assert_eq!(files[1].0, Path::new("/p/resources/views/components/forms/text-input.blade.php"));
        let files = plan(Simple::Class, "Support/Money", root, None);
        assert!(files[0].1.contains("namespace App\\Support;"), "{}", files[0].1);
        let files = plan(Simple::Class, "Money", root, None);
        assert!(files[0].1.contains("namespace App;"), "{}", files[0].1);
        let files = plan(Simple::Service, "Billing", root, Some("Shop"));
        assert_eq!(files[0].0, Path::new("/p/Modules/Shop/Services/Billing.php"));
        assert!(files[0].1.contains("namespace Modules\\Shop\\Services;"));
        let files = plan(Simple::View, "admin.users.index", root, None);
        assert_eq!(files[0].0, Path::new("/p/resources/views/admin/users/index.blade.php"));
        let files = plan(Simple::Dto, "OrderData", root, None);
        assert_eq!(files[0].0, Path::new("/p/app/Data/OrderData.php"));
        assert!(files[0].1.contains("final readonly class OrderData") && files[0].1.contains("fromArray"));
        let files = plan(Simple::NovaResource, "Post", root, None);
        assert_eq!(files[0].0, Path::new("/p/app/Nova/Post.php"));
        assert!(files[0].1.contains("use Laravel\\Nova\\Fields\\ID;") && files[0].1.contains("public static $model = \\App\\Models\\Post::class;"), "{}", files[0].1);
        assert_eq!(all().len(), 46);
    }
}
