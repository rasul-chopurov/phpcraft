use std::fs;
use std::path::{Path, PathBuf};

use php_analysis::{Document, complete, goto_definition};
use php_index::Index;

use crate::*;

struct Fixture(PathBuf);

impl Fixture {
    fn new(name: &str, files: &[(&str, &str)]) -> Self {
        let root = std::env::temp_dir().join(format!("phpcraft-laravel-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        for (path, content) in files {
            let path = root.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, content).unwrap();
        }
        Self(root)
    }

    fn rel(&self, loc: &Location) -> String {
        loc.path.strip_prefix(&self.0).unwrap().display().to_string().replace('\\', "/")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

const CONTROLLER: &str = "<?php\nnamespace App\\Http\\Controllers;\n\nclass HomeController\n{\n    public function index()\n    {\n        $url = route('profile.edit');\n        $name = config('app.name');\n        $hi = __('messages.welcome');\n        $key = env('APP_KEY');\n        return view('pages.home');\n    }\n}\n";

fn fixture(name: &str) -> Fixture {
    Fixture::new(
        name,
        &[
            ("artisan", ""),
            (".env", "APP_NAME=Demo\nAPP_KEY=base64:x\n"),
            ("routes/web.php", "<?php\nRoute::get('/profile', [ProfileController::class, 'edit'])->name('profile.edit');\nRoute::name('admin.')->group(fn () => null);\n"),
            ("config/app.php", "<?php\nreturn [\n    'name' => env('APP_NAME'),\n];\n"),
            ("lang/en/messages.php", "<?php\nreturn ['welcome' => 'Welcome'];\n"),
            ("resources/views/pages/home.blade.php", "@extends('layouts.app')\n<x-alert type=\"x\" />\n<livewire:user-list />\n{{ route('profile.edit') }}\n"),
            ("resources/views/layouts/app.blade.php", "<html></html>"),
            ("resources/views/components/alert.blade.php", "<div></div>"),
            ("app/Livewire/UserList.php", "<?php\n"),
            ("app/Http/Controllers/HomeController.php", CONTROLLER),
        ],
    )
}

fn goto_in_controller(fx: &Fixture, needle: &str) -> Vec<String> {
    let laravel = Laravel::new(&fx.0);
    let path = fx.0.join("app/Http/Controllers/HomeController.php");
    let doc = Document::parse(&path, CONTROLLER).unwrap();
    let offset = CONTROLLER.find(needle).unwrap() + 2;
    goto_definition(&doc, &Index::new(), offset, &[&laravel])
        .iter()
        .map(|l| fx.rel(l))
        .collect()
}

#[test]
fn navigates_php_strings() {
    let fx = fixture("php");
    assert_eq!(goto_in_controller(&fx, "profile.edit"), ["routes/web.php"]);
    assert_eq!(goto_in_controller(&fx, "app.name"), ["config/app.php"]);
    assert_eq!(goto_in_controller(&fx, "messages.welcome"), ["lang/en/messages.php"]);
    assert_eq!(goto_in_controller(&fx, "APP_KEY"), [".env"]);
    assert_eq!(goto_in_controller(&fx, "pages.home"), ["resources/views/pages/home.blade.php"]);

    let laravel = Laravel::new(&fx.0);
    let config = laravel.config_key("app.name").unwrap();
    let source = fs::read_to_string(&config.path).unwrap();
    assert_eq!(&source[config.range], "name");
    assert_eq!(laravel.route_names().len(), 1, "group prefixes are not routes");
}

#[test]
fn navigates_blade() {
    let fx = fixture("blade");
    let laravel = Laravel::new(&fx.0);
    let blade = fs::read_to_string(fx.0.join("resources/views/pages/home.blade.php")).unwrap();
    let goto = |needle: &str, delta: usize| -> Vec<String> {
        laravel
            .text_goto(&blade, blade.find(needle).unwrap() + delta)
            .iter()
            .map(|l| fx.rel(l))
            .collect()
    };
    assert_eq!(goto("layouts.app", 2), ["resources/views/layouts/app.blade.php"]);
    assert_eq!(goto("x-alert", 3), ["resources/views/components/alert.blade.php"]);
    assert_eq!(goto("livewire:user", 10), ["app/Livewire/UserList.php"]);
    assert_eq!(goto("profile.edit') }}", 1), ["routes/web.php"]);
}

#[test]
fn completes_route_and_view_names() {
    let fx = fixture("complete");
    let laravel = Laravel::new(&fx.0);
    let path: &Path = &fx.0.join("x.php");
    let src = "<?php\nview('pa');\nroute('');\n";
    let doc = Document::parse(path, src).unwrap();
    let views: Vec<_> = complete(&doc, &Index::new(), src.find("pa'").unwrap() + 2, &[&laravel])
        .into_iter()
        .map(|i| i.label)
        .collect();
    assert_eq!(views, ["pages.home"]);
    let routes: Vec<_> = complete(&doc, &Index::new(), src.find("route('").unwrap() + 7, &[&laravel])
        .into_iter()
        .map(|i| i.label)
        .collect();
    assert_eq!(routes, ["profile.edit"]);
}
