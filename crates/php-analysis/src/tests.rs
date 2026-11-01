use std::path::Path;

use php_index::{Index, Location};

use crate::*;

const MODEL: &str = r#"<?php
namespace Illuminate\Database\Eloquent;

abstract class Model
{
    public static function query(): Builder {}
    public function save(): bool {}
}
"#;

const BUILDER: &str = r#"<?php
namespace Illuminate\Database\Eloquent;

class Builder
{
    /** @return static */
    public function where($column, $value = null) {}
    public function first(): ?Model {}
}
"#;

const POST: &str = r#"<?php
namespace App\Models;

class Post extends \Illuminate\Database\Eloquent\Model
{
    public string $title;
}
"#;

const USER: &str = r#"<?php
namespace App\Models;

use Illuminate\Database\Eloquent\Model;

class User extends Model
{
    const ADMIN = 'admin';
    public ?Post $latest = null;

    public function posts(): Post {}

    public function demo(Post $post)
    {
        $builder = static::query();
        $builder->where('a')->first();
        $this->posts()->title;
        $post->save();
        $other = new Post();
        $other->title;
        return [User::class, 'posts'];
    }
}
"#;

fn index() -> Index {
    let mut index = Index::new();
    for (path, src) in [
        ("/p/Model.php", MODEL),
        ("/p/Builder.php", BUILDER),
        ("/p/Post.php", POST),
        ("/p/User.php", USER),
    ] {
        index.update_file(Path::new(path), src);
    }
    index
}

/// Offset of the `nth` occurrence of `needle` plus `delta`.
fn at(src: &str, needle: &str, delta: usize) -> usize {
    src.find(needle).unwrap_or_else(|| panic!("{needle} not found")) + delta
}

fn goto(src: &str, offset: usize) -> Vec<(String, String)> {
    let index = index();
    let doc = Document::parse(Path::new("/p/User.php"), src).unwrap();
    goto_definition(&doc, &index, offset, &[])
        .into_iter()
        .map(|Location { path, range }| {
            let file = std::fs::read_to_string(&path).ok();
            let source = match path.to_str().unwrap() {
                "/p/Model.php" => MODEL,
                "/p/Builder.php" => BUILDER,
                "/p/Post.php" => POST,
                "/p/User.php" => USER,
                _ => file.as_deref().unwrap_or(""),
            };
            (path.display().to_string(), source[range].to_string())
        })
        .collect()
}

#[test]
fn goto_classes_methods_and_properties() {
    let p = |path: &str, text: &str| vec![(path.to_string(), text.to_string())];
    assert_eq!(goto(USER, at(USER, "Model;", 1)), p("/p/Model.php", "Model"));
    assert_eq!(goto(USER, at(USER, "extends Model", 9)), p("/p/Model.php", "Model"));
    assert_eq!(goto(USER, at(USER, "query()", 1)), p("/p/Model.php", "query"));
    assert_eq!(goto(USER, at(USER, "where('a')", 1)), p("/p/Builder.php", "where"));
    assert_eq!(goto(USER, at(USER, "first()", 1)), p("/p/Builder.php", "first"));
    assert_eq!(goto(USER, at(USER, "->title;", 3)), p("/p/Post.php", "$title"));
    assert_eq!(goto(USER, at(USER, "$post->save", 7)), p("/p/Model.php", "save"));
    assert_eq!(goto(USER, at(USER, "$other->title", 9)), p("/p/Post.php", "$title"));
    assert_eq!(goto(USER, at(USER, "new Post", 5)), p("/p/Post.php", "Post"));
    assert_eq!(goto(USER, at(USER, "'posts']", 2)), p("/p/User.php", "posts"));
    // Caret right after the identifier still resolves.
    assert_eq!(goto(USER, at(USER, "first()", 5)), p("/p/Builder.php", "first"));
    // Local variable -> its first assignment.
    assert_eq!(
        goto(USER, at(USER, "$builder->", 1)),
        p("/p/User.php", "$builder")
    );
}

fn labels(src: &str, offset: usize) -> Vec<String> {
    let index = index();
    let doc = Document::parse(Path::new("/p/User.php"), src).unwrap();
    complete(&doc, &index, offset, &[])
        .into_iter()
        .map(|i| i.label)
        .collect()
}

#[test]
fn completes_members_statics_and_variables() {
    let src = USER.replace("$this->posts()->title;", "$this->po");
    let items = labels(&src, at(&src, "$this->po", 9));
    assert_eq!(items[0], "posts");

    let src = USER.replace("$this->posts()->title;", "$post->");
    let items = labels(&src, at(&src, "$post->", 7));
    assert!(items.contains(&"save".to_string()) && items.contains(&"title".to_string()));

    let src = USER.replace("$this->posts()->title;", "User::");
    let items = labels(&src, at(&src, "User::", 6));
    assert!(items.contains(&"ADMIN".to_string()) && items.contains(&"query".to_string()));
    assert!(!items.contains(&"posts".to_string()));

    let src = USER.replace("$this->posts()->title;", "$bu");
    let items = labels(&src, at(&src, "$bu\n", 3));
    assert_eq!(items, ["$builder"]);
}

#[test]
fn class_completion_adds_import() {
    let index = index();
    let src = "<?php\nnamespace App\\Http;\n\nuse Illuminate\\Database\\Eloquent\\Model;\n\nclass X { function f() { Pos } }\n";
    let doc = Document::parse(Path::new("/p/X.php"), src).unwrap();
    let items = complete(&doc, &index, at(src, "Pos }", 3), &[]);
    let post = items.iter().find(|i| i.label == "Post").unwrap();
    assert_eq!(post.detail, "App\\Models\\Post");
    let edit = post.additional_edit.clone().unwrap();
    let mut out = src.to_string();
    out.replace_range(edit.range, &edit.text);
    assert!(out.contains("use App\\Models\\Post;\nuse Illuminate\\Database\\Eloquent\\Model;"));
}

#[test]
fn unresolved_and_optimize_imports() {
    let index = index();
    let src = "<?php\nnamespace App\\Http;\n\nuse Zeta\\Unused;\nuse Illuminate\\Database\\Eloquent\\Model;\nuse App\\Models\\User;\n\n/** @var User $u */\nclass X extends Model { function f(): Post {} }\n";
    let doc = Document::parse(Path::new("/p/X.php"), src).unwrap();
    let unresolved = unresolved_classes(&doc, &index);
    assert_eq!(unresolved.len(), 1);
    assert_eq!(unresolved[0].name, "Post");
    assert_eq!(unresolved[0].candidates, ["App\\Models\\Post"]);

    let edit = optimize_imports(&doc).unwrap();
    let mut out = src.to_string();
    out.replace_range(edit.range, &edit.text);
    assert!(out.contains("use App\\Models\\User;\nuse Illuminate\\Database\\Eloquent\\Model;\n\n/**"));
    assert!(!out.contains("Unused"));
}

#[test]
fn string_args_are_detected() {
    let index = index();
    let src = "<?php\nreturn view('users.index', ['a' => 1]);\nRoute::view('/x', 'welcome');\n";
    let doc = Document::parse(Path::new("/p/r.php"), src).unwrap();
    let arg = string_arg_at(&doc, &index, at(src, "users", 2)).unwrap();
    assert_eq!(arg.value, "users.index");
    assert!(arg.is_function(&["view"]));
    let arg = string_arg_at(&doc, &index, at(src, "welcome", 1)).unwrap();
    assert_eq!(arg.arg_index, 1);
    assert!(arg.is_method(&["view"]));
}

#[test]
fn string_paths_resolve_to_files() {
    let root = std::env::temp_dir().join(format!("phpcraft-paths-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("public")).unwrap();
    std::fs::create_dir_all(root.join("bootstrap")).unwrap();
    std::fs::create_dir_all(root.join("routes")).unwrap();
    std::fs::write(root.join("bootstrap/app.php"), "<?php").unwrap();
    std::fs::write(root.join("routes/web.php"), "<?php").unwrap();
    let index_php = root.join("public/index.php");
    let src = "<?php\n$app = require_once __DIR__.'/../bootstrap/app.php';\n$r = base_path('routes/web.php');\n$x = dirname(__DIR__) . '/routes/web.php';\n$n = 'not a path';\n";
    let doc = Document::parse(&index_php, src).unwrap();
    let helpers = [("base_path", root.clone())];
    let at = |needle: &str| src.find(needle).unwrap() + 2;
    let canon = |p: &str| std::fs::canonicalize(root.join(p)).unwrap();
    assert_eq!(file_path_at(&doc, at("/../bootstrap"), &helpers, &[]), Some(canon("bootstrap/app.php")));
    assert_eq!(file_path_at(&doc, at("routes/web.php')"), &helpers, &[]), Some(canon("routes/web.php")));
    assert_eq!(file_path_at(&doc, at("/routes/web.php';"), &helpers, &[]), Some(canon("routes/web.php")));
    assert_eq!(file_path_at(&doc, at("not a path"), &helpers, &[root.clone()]), None);
    std::fs::remove_dir_all(&root).unwrap();
}
