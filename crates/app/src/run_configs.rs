//! Run configurations: saved in `.phpcraft/run.json`, plus ones detected from the project
//! (composer / npm scripts, artisan, test runners) and the test under the caret.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Kind {
    /// `php <script> <args>`
    Php { script: String, #[serde(default)] args: String },
    /// `php artisan <command>`
    Artisan { command: String },
    /// PHPUnit or Pest, optionally limited to a file and a `--filter`.
    Test {
        runner: Runner,
        #[serde(default)]
        path: String,
        #[serde(default)]
        filter: String,
    },
    Composer { script: String },
    Npm { script: String },
    /// Any command line, run through the shell.
    Shell { command: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Runner {
    Phpunit,
    Pest,
    /// `php artisan test` (wraps PHPUnit/Pest).
    Artisan,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunConfig {
    pub name: String,
    #[serde(flatten)]
    pub kind: Kind,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct File {
    #[serde(default)]
    configurations: Vec<RunConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    php: Option<String>,
}

pub fn file_path(root: &Path) -> PathBuf {
    root.join(".phpcraft").join("run.json")
}

/// Saved configurations.
pub fn load(root: &Path) -> Vec<RunConfig> {
    read(root).configurations
}

fn read(root: &Path) -> File {
    std::fs::read_to_string(file_path(root)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

/// The PHP interpreter setting (`"php"` unless `.phpcraft/run.json` names another).
pub fn php(root: &Path) -> String {
    read(root).php.unwrap_or_else(|| "php".into())
}

/// Saves the PHP interpreter setting.
pub fn set_php(root: &Path, php: &str) -> std::io::Result<()> {
    let mut file = read(root);
    file.php = (!php.trim().is_empty() && php.trim() != "php").then(|| php.trim().to_string());
    write(root, &file)
}

/// Where PHP runs: on this machine, in a Docker container, through Sail, or over SSH.
#[derive(Debug, Clone, PartialEq)]
pub enum Interpreter {
    Local(String),
    /// `docker:<container>[:<workdir>]` (workdir defaults to /var/www/html).
    Docker { container: String, workdir: String },
    Sail,
    /// `ssh:<host>[:<workdir>]`.
    Ssh { host: String, workdir: String },
}

impl Interpreter {
    pub fn parse(setting: &str) -> Self {
        let setting = setting.trim();
        let split = |rest: &str, default: &str| -> (String, String) {
            match rest.split_once(':') {
                Some((a, b)) if !b.is_empty() => (a.to_string(), b.to_string()),
                _ => (rest.trim_end_matches(':').to_string(), default.to_string()),
            }
        };
        if let Some(rest) = setting.strip_prefix("docker:") {
            let (container, workdir) = split(rest, "/var/www/html");
            Interpreter::Docker { container, workdir }
        } else if let Some(rest) = setting.strip_prefix("ssh:") {
            let (host, workdir) = split(rest, ".");
            Interpreter::Ssh { host, workdir }
        } else if setting == "sail" {
            Interpreter::Sail
        } else {
            Interpreter::Local(if setting.is_empty() { "php".into() } else { setting.to_string() })
        }
    }

    /// `php <args>` through this interpreter. `env` reaches PHP inside containers too.
    pub fn command(&self, root: &Path, args: Vec<String>, env: &BTreeMap<String, String>) -> terminal::Command {
        let quote = |a: &String| if a.chars().all(|c| c.is_alphanumeric() || "-_./=:@".contains(c)) { a.clone() } else { format!("'{}'", a.replace('\'', "'\\''")) };
        match self {
            Interpreter::Local(php) => terminal::Command { program: php.clone(), args },
            Interpreter::Docker { container, workdir } => {
                let mut full = vec!["exec".to_string(), "-it".to_string(), "-w".to_string(), workdir.clone()];
                for (key, value) in env {
                    // Xdebug in a container reaches the IDE through the Docker host.
                    let value = value.replace("client_host=127.0.0.1", "client_host=host.docker.internal");
                    full.push("-e".into());
                    full.push(format!("{key}={value}"));
                }
                full.push(container.clone());
                full.push("php".into());
                full.extend(args);
                terminal::Command { program: "docker".into(), args: full }
            }
            Interpreter::Sail => terminal::Command { program: root.join("vendor/bin/sail").to_string_lossy().into_owned(), args: [vec!["php".to_string()], args].concat() },
            Interpreter::Ssh { host, workdir } => {
                let exports: String = env.iter().map(|(k, v)| format!("{k}={} ", quote(v))).collect();
                let line = format!("cd {} && {exports}php {}", quote(workdir), args.iter().map(quote).collect::<Vec<_>>().join(" "));
                terminal::Command { program: "ssh".into(), args: vec!["-t".into(), host.clone(), line] }
            }
        }
    }
}

/// Adds or replaces (by name) a saved configuration.
pub fn save(root: &Path, config: &RunConfig) -> std::io::Result<()> {
    let mut file = read(root);
    file.configurations.retain(|c| c.name != config.name);
    file.configurations.push(config.clone());
    write(root, &file)
}

fn write(root: &Path, file: &File) -> std::io::Result<()> {
    let path = file_path(root);
    std::fs::create_dir_all(path.parent().unwrap())?;
    std::fs::write(path, serde_json::to_string_pretty(file).unwrap_or_default())
}

/// Creates `.phpcraft/run.json` with the detected configurations if it doesn't exist.
pub fn ensure_file(root: &Path) -> std::io::Result<PathBuf> {
    let path = file_path(root);
    if !path.exists() {
        let file = File { configurations: detect(root).into_iter().take(3).collect(), php: None };
        write(root, &file)?;
    }
    Ok(path)
}

pub fn test_runner(root: &Path) -> Option<Runner> {
    if root.join("vendor/bin/pest").exists() {
        Some(Runner::Pest)
    } else if root.join("vendor/bin/phpunit").exists() || root.join("phpunit.xml").exists() || root.join("phpunit.xml.dist").exists() {
        Some(if root.join("artisan").exists() { Runner::Artisan } else { Runner::Phpunit })
    } else {
        None
    }
}

/// Configurations offered without saving anything.
pub fn detect(root: &Path) -> Vec<RunConfig> {
    let config = |name: String, kind: Kind| RunConfig { name, kind, env: BTreeMap::new() };
    let mut out = Vec::new();
    if root.join("artisan").exists() {
        out.push(config("artisan serve".into(), Kind::Artisan { command: "serve".into() }));
    }
    if let Some(runner) = test_runner(root) {
        out.push(config("All tests".into(), Kind::Test { runner, path: String::new(), filter: String::new() }));
    }
    if root.join("artisan").exists() {
        for command in ["queue:work", "schedule:work", "migrate"] {
            out.push(config(format!("artisan {command}"), Kind::Artisan { command: command.into() }));
        }
    }
    let scripts = |file: &str| -> Vec<String> {
        std::fs::read_to_string(root.join(file))
            .ok()
            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
            .and_then(|v| v.get("scripts")?.as_object().map(|o| o.keys().cloned().collect()))
            .unwrap_or_default()
    };
    for script in scripts("composer.json") {
        if !script.starts_with("pre-") && !script.starts_with("post-") {
            out.push(config(format!("composer {script}"), Kind::Composer { script }));
        }
    }
    for script in scripts("package.json") {
        out.push(config(format!("npm run {script}"), Kind::Npm { script }));
    }
    out
}

/// The program and arguments for a configuration.
/// Where test runs write their JUnit report, relative to the project.
pub const JUNIT_REPORT: &str = ".phpcraft/junit.xml";

pub fn command(config: &RunConfig, root: &Path) -> terminal::Command {
    let interpreter = Interpreter::parse(&php(root));
    let php_cmd = |_: &str, args: Vec<String>| interpreter.command(root, args, &config.env);
    let cmd = |program: &str, args: Vec<String>| terminal::Command { program: program.into(), args };
    let split = |s: &str| s.split_whitespace().map(str::to_string).collect::<Vec<_>>();
    match &config.kind {
        Kind::Php { script, args } => php_cmd("", [vec![script.clone()], split(args)].concat()),
        Kind::Artisan { command } => php_cmd("", [vec!["artisan".into()], split(command)].concat()),
        Kind::Test { runner, path, filter } => {
            let mut args = match runner {
                Runner::Phpunit => vec!["vendor/bin/phpunit".to_string()],
                Runner::Pest => vec!["vendor/bin/pest".to_string()],
                Runner::Artisan => vec!["artisan".to_string(), "test".to_string()],
            };
            if !path.is_empty() {
                args.push(path.clone());
            }
            if !filter.is_empty() {
                args.push(format!("--filter={filter}"));
            }
            // A JUnit report for the results list (see `test_results`).
            args.push("--log-junit".into());
            args.push(JUNIT_REPORT.into());
            php_cmd("", args)
        }
        Kind::Composer { script } => cmd("composer", vec!["run-script".into(), script.clone()]),
        Kind::Npm { script } => cmd(if cfg!(windows) { "npm.cmd" } else { "npm" }, vec!["run".into(), script.clone()]),
        Kind::Shell { command } => {
            if cfg!(windows) {
                cmd("cmd", vec!["/C".into(), command.clone()])
            } else {
                cmd("/bin/sh", vec!["-c".into(), command.clone()])
            }
        }
    }
}

/// Lines of a test file that start a test (method `test…` / `#[Test]`, Pest `it(` / `test(`)
/// or the test class, for the ▶ marks in the gutter.
pub fn test_lines(file: &Path, text: &str) -> Vec<usize> {
    let name = file.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    if !name.ends_with("Test") && !file.components().any(|c| c.as_os_str() == "tests") {
        return Vec::new();
    }
    let lines: Vec<&str> = text.lines().collect();
    let mut found = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let t = line.trim_start();
        let attributed = i > 0 && lines[i - 1].trim() == "#[Test]";
        let is_test = (t.contains("function test") && t.contains("public"))
            || (attributed && t.contains("function "))
            || t.starts_with("it(")
            || t.starts_with("test(")
            || (t.starts_with("class ") && t.split_whitespace().nth(1).is_some_and(|c| c.ends_with("Test")))
            || (t.starts_with("final class ") && t.split_whitespace().nth(2).is_some_and(|c| c.ends_with("Test")));
        if is_test {
            found.push(i);
        }
    }
    found
}

/// The run configuration for the caret position: a test method / Pest test, a test class,
/// or the PHP script itself.
pub fn for_context(root: &Path, file: &Path, text: &str, offset: usize) -> Option<RunConfig> {
    let relative = file.strip_prefix(root).unwrap_or(file).to_string_lossy().replace('\\', "/");
    let name = file.file_stem()?.to_string_lossy().to_string();
    let is_test = name.ends_with("Test") || relative.starts_with("tests/");
    let config = |name: String, kind| Some(RunConfig { name, kind, env: BTreeMap::new() });
    if is_test && let Some(runner) = test_runner(root) {
        let before = &text[..offset.min(text.len())];
        // Pest: the nearest `it('…'` / `test('…'` above the caret.
        let pest = ["it(", "test("]
            .iter()
            .filter_map(|call| {
                let at = before.rfind(call)?;
                let preceded_ok = at == 0 || !before.as_bytes()[at - 1].is_ascii_alphanumeric() && before.as_bytes()[at - 1] != b'>';
                preceded_ok.then_some((at, &before[at + call.len()..]))
            })
            .max_by_key(|(at, _)| *at)
            .and_then(|(_, rest)| {
                let rest = rest.trim_start();
                let quote = rest.chars().next().filter(|c| *c == '\'' || *c == '"')?;
                let body = &rest[1..];
                Some(body[..body.find(quote)?].to_string())
            });
        let method = before.rfind("function ").and_then(|at| {
            let rest = &before[at + 9..];
            let end = rest.find(|c: char| !(c.is_alphanumeric() || c == '_'))?;
            let method = &rest[..end];
            (method.starts_with("test") || before[..at].trim_end().ends_with("*/") || before[..at].contains("#[Test]"))
                .then(|| method.to_string())
        });
        let filter = match (pest, method) {
            (Some(desc), _) => desc,
            (None, Some(method)) if method != "setUp" && method != "tearDown" => method,
            _ => String::new(),
        };
        let label = if filter.is_empty() { name } else { format!("{name}::{filter}") };
        let filter = regex_escape(&filter);
        return config(label, Kind::Test { runner, path: relative, filter });
    }
    if file.extension().is_some_and(|e| e == "php") {
        return config(name, Kind::Php { script: relative, args: String::new() });
    }
    None
}

pub(crate) fn regex_escape(s: &str) -> String {
    s.chars().fold(String::new(), |mut out, c| {
        if "\\.^$|?*+()[]{}/'\"".contains(c) {
            out.push('\\');
        }
        out.push(c);
        out
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(files: &[(&str, &str)]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("phpcraft-run-{}-{}", std::process::id(), files.len()));
        let _ = std::fs::remove_dir_all(&dir);
        for (path, content) in files {
            let path = dir.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, content).unwrap();
        }
        dir
    }

    #[test]
    fn detects_and_builds_commands() {
        let root = project(&[
            ("artisan", ""),
            ("vendor/bin/phpunit", ""),
            ("composer.json", r#"{"scripts":{"dev":"x","post-install-cmd":"y"}}"#),
            ("package.json", r#"{"scripts":{"build":"vite build"}}"#),
        ]);
        let names: Vec<String> = detect(&root).into_iter().map(|c| c.name).collect();
        assert!(names.contains(&"artisan serve".to_string()));
        assert!(names.contains(&"All tests".to_string()));
        assert!(names.contains(&"composer dev".to_string()));
        assert!(!names.iter().any(|n| n.contains("post-install")));
        assert!(names.contains(&"npm run build".to_string()));

        let all = detect(&root).into_iter().find(|c| c.name == "All tests").unwrap();
        let c = command(&all, &root);
        assert_eq!((c.program.as_str(), c.args.clone()), ("php", ["artisan", "test", "--log-junit", JUNIT_REPORT].map(String::from).to_vec()));

        save(&root, &RunConfig { name: "serve".into(), kind: Kind::Artisan { command: "serve --port=8080".into() }, env: BTreeMap::new() }).unwrap();
        let saved = load(&root);
        assert_eq!(saved.len(), 1);
        assert_eq!(command(&saved[0], &root).args, ["artisan", "serve", "--port=8080"]);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn interpreters() {
        let env = BTreeMap::from([("XDEBUG_CONFIG".to_string(), "client_host=127.0.0.1 client_port=9003".to_string())]);
        let root = Path::new("/p");
        let args = vec!["artisan".to_string(), "migrate".to_string()];
        let local = Interpreter::parse("php").command(root, args.clone(), &env);
        assert_eq!((local.program.as_str(), local.args.clone()), ("php", args.clone()));
        let docker = Interpreter::parse("docker:app").command(root, args.clone(), &env);
        assert_eq!(docker.program, "docker");
        assert_eq!(docker.args, ["exec", "-it", "-w", "/var/www/html", "-e", "XDEBUG_CONFIG=client_host=host.docker.internal client_port=9003", "app", "php", "artisan", "migrate"]);
        assert_eq!(Interpreter::parse("docker:app:/srv"), Interpreter::Docker { container: "app".into(), workdir: "/srv".into() });
        let ssh = Interpreter::parse("ssh:deploy@web:/var/www/shop").command(root, args.clone(), &BTreeMap::new());
        assert_eq!(ssh.args, ["-t", "deploy@web", "cd /var/www/shop && php artisan migrate"]);
        assert_eq!(Interpreter::parse("sail"), Interpreter::Sail);
    }

    #[test]
    fn test_lines_in_files() {
        let text = "<?php\nclass UserTest extends TestCase\n{\n    public function test_login(): void {}\n    #[Test]\n    public function logs_out(): void {}\n    private function helper() {}\n}\n";
        assert_eq!(test_lines(Path::new("tests/UserTest.php"), text), [1, 3, 5]);
        assert_eq!(test_lines(Path::new("tests/Feature/HomeTest.php"), "<?php\nit('works', function () {});\ntest('x', fn () => 1);\n"), [1, 2]);
        assert!(test_lines(Path::new("app/User.php"), text).is_empty());
    }

    #[test]
    fn test_under_caret() {
        let root = project(&[("vendor/bin/pest", "")]);
        let phpunit = "<?php\nclass UserTest extends TestCase {\n    public function test_login(): void {\n        $this->get('/');\n    }\n}\n";
        let file = root.join("tests/Feature/UserTest.php");
        let caret = phpunit.find("$this").unwrap();
        let c = for_context(&root, &file, phpunit, caret).unwrap();
        assert_eq!(c.name, "UserTest::test_login");
        assert_eq!(c.kind, Kind::Test { runner: Runner::Pest, path: "tests/Feature/UserTest.php".into(), filter: "test_login".into() });

        let pest = "<?php\nit('logs in (fast)', function () {\n    expect(true)->toBeTrue();\n});\n";
        let caret = pest.find("expect").unwrap();
        let c = for_context(&root, &root.join("tests/Unit/AuthTest.php"), pest, caret).unwrap();
        assert_eq!(c.kind, Kind::Test { runner: Runner::Pest, path: "tests/Unit/AuthTest.php".into(), filter: "logs in \\(fast\\)".into() });

        let script = for_context(&root, &root.join("bin/tool.php"), "<?php echo 1;", 3).unwrap();
        assert_eq!(script.kind, Kind::Php { script: "bin/tool.php".into(), args: String::new() });
        std::fs::remove_dir_all(root).unwrap();
    }
}
