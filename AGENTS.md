# AGENTS.md — PhpCraft

Guide for agents (and people) continuing work on this repository.

## Goal

PhpCraft is a native, offline PHP / Laravel IDE written in Rust on **gpui** (Zed's UI
framework): PhpStorm + Laravel Idea's feature set without the JVM. Open source (Apache-2.0),
macOS / Windows / Linux. Author: Rasul Chopurov (path.az). The user speaks Russian — reply in
Russian.

PhpStorm is used only as a **feature checklist**. Never copy JetBrains code, icons or texts;
everything is written from scratch. (Exception: JetBrains phpstorm-stubs, Apache-2.0, bundled
as data — see `crates/php-index/STUBS-LICENSE`.)

## Workspace layout (`crates/`)

| Crate | What it is |
|---|---|
| `app` | The application (binary `phpcraft`): windows, editor view, workspace, dialogs, menus, keymap |
| `editor` | Text buffer: rope, multi-caret selections, undo/redo groups, anchors, smart pairs, indentation, extend selection |
| `php-syntax` | tree-sitter grammars + highlighting (PHP, HTML, JS/TS, CSS, JSON, YAML), hand-written highlighters (Blade overlay, Twig, SQL, Markdown, .env…), fold ranges, node ranges |
| `php-index` | Project symbol index (classes, members, functions), binary cache, phpstorm-stubs (`stubs.bin.gz`, `stubs.rs`) |
| `php-analysis` | Code intelligence on one parsed `Document` + `Index`: completion, goto, usages, rename symbol, generate, docs/parameter info, hierarchy, refactorings, inspections, intentions |
| `laravel` | Laravel awareness: string navigation (views, routes, config, lang, env), Blade (sections/stacks/props, extract), route refs from routes/*.php, generators, artisan, DB, Laravel intentions |
| `git` | git CLI wrapper: status, log, diff, blame, rebase (interactive), shelve, conflicts parsing, worktrees, remotes |
| `dbgp` | Xdebug DBGp protocol (sessions, breakpoints, stack, variables, eval) |
| `terminal` | PTY terminal (alacritty_terminal) |
| `agent` | AI assistant: Claude API agent loop + ACP client (Claude Code, OpenCode, Gemini, Codex) |
| `spell` | Spell checker: bundled word list (web2, public domain) + `extra.txt`, inflection rules, camelCase split, suggestions, project dictionary `.phpcraft/dictionary.txt` |
| `history` | Local History store |
| `project` | File tree model |
| `search` | Find in files, fuzzy matching |
| `process` | `process::command()` — `Command::new` that doesn't open console windows on Windows. **Always use it to spawn processes.** |

### Inside `crates/app/src`

- `main.rs` — startup (login-shell PATH, splash, windows). Module list lives here.
- `intel.rs` — `Intel`: the bridge between the UI and analysis (index lock, Laravel providers,
  `with_doc(path, text, |doc, index| …)`, goto, usages, problems, route marks, stubs indexing).
- `editor_view.rs` + `editor_view/*` — the code editor: rendering (visual **rows** in
  `rows.rs` model folding + soft wrap), mouse, IME, and feature modules: `multicaret`,
  `text_ops`, `folding`, `bookmarks`, `breakpoints`, `usages`, `generate`, `info` (quick doc /
  parameter info), `hierarchy`, `refactor`, `problems`, `snippets` (postfix/live templates),
  `format`, `test_marks` (▶ tests, ↑↓ inheritance, ⇄ routes), `intel_ui` (completion menu,
  intentions ⌥↵, goto).
- `workspace.rs` + `workspace/*` — the project window: tabs, panes, tool windows
  (`layout.rs` lists them: Project, Structure, Commit, Git, Terminal, AI, Database, Laravel,
  Remote, Debug, Services…), and feature modules (`git_*`, `debug_panel`, `services_panel`,
  `tests_view`, `problems`, `refactor`, `tabs`, `composer`, `http_panel`…).
- `dialogs/*` — modal dialogs. `InputDialog` + `FileOp` is the generic "ask for a name" dialog
  (rename symbol, composer require, extract blade, compare revision…); results go through
  `DialogEvent` → `Workspace::on_dialog_event` (`workspace/tools.rs`).
  `navigate.rs` is the list/search popup reused for many result lists (`NavMode::*`).
- `menus.rs` (menu bar), `keymap.rs` (default shortcuts, mac + Windows/Linux),
  `config.rs` (settings.json), `run_configs.rs` (run/test configs, PHP interpreter local /
  docker / sail / ssh), `quality.rs` (Pint, PHP CS Fixer, PHPStan, Psalm, PHPCS), `debugger.rs`
  (Xdebug controller thread), `docker.rs`, `test_results.rs` (JUnit, Clover), `http_client.rs`
  + `http_convert.rs`, `pull_requests.rs`, `twig.rs`, `bookmarks.rs`, `breakpoints.rs`.

### Conventions

- Match the surrounding code: short doc comments explaining *why*, no noise. Commit messages
  are one descriptive paragraph in English ending with
  `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Analysis logic goes into the library crates with unit tests; the app wires UI.
- Blocking work (git, php, network, indexing) runs on `cx.background_executor()`.
- Spawned processes: `process::command`. PHP: `laravel::php_binary()`.
- Per-project files live in `.phpcraft/` (run.json, laravel.json, debug.json, deployment.json,
  shelf/, http/, junit.xml…); app-wide in `config::dir()` (settings, keymap, bookmarks,
  breakpoints, index caches, unpacked stubs).
- Verify with `cargo test --workspace` (≈150 tests) and `cargo build -p phpcraft` without
  warnings. For UI checks run `./target/debug/phpcraft <file>` and screenshot the window by id
  (a Swift `CGWindowListCopyWindowInfo` helper); background windows don't repaint, so state that
  appears after the first frame can't be screenshotted that way.
- `INSPECT_ROOT=/path cargo test -p php-analysis survey -- --ignored --nocapture` surveys
  inspections on a real project (false-positive check).
- Releases: bump `version` in `Cargo.toml` and `VERSION` in `site/src/home.html` +
  `site/phpcraft/index.html`; push; tag `vX.Y.Z-alpha` → GitHub Actions `Release` builds 12
  installers. CI (`ci.yml`) must be green first. The site deploys with `./site/deploy.sh`
  (FTP password asked interactively).

## What exists (high level)

Editor (multi-caret, folding, soft wrap, smart pairs, bookmarks, extend selection, templates),
PHP intelligence (completion with auto-import, goto, find usages, rename, extract / introduce /
inline / safe delete, generate, quick doc, parameter info, hierarchy, call hierarchy,
inspections + PHPStan/Psalm/PHPCS, spell checking of comments and strings (`typo`, ⌥↵ suggestions / save to dictionary), intentions, phpstorm-stubs), Laravel (string navigation,
Blade sections/stacks/props, routes in the gutter, generators incl. DTO / Nova, extract blade
component/view, facade→DI / rules→array / scope→#[Scope] intentions), Twig, Git (commit, log
with graph, branches, diff, blame, merge conflicts in the editor, compare with revision /
clipboard, interactive rebase, cherry-pick, revert, undo commit, shelve, worktrees, remotes),
Pull Requests (Git ▸ Pull Requests…: list, details, checkout, diff, comment via gh / glab),
Xdebug debugger, Docker services, test runner with results and coverage, Composer menu, HTTP
client (+ cURL / Postman / OpenAPI, history), database tools, SFTP deployment (auto upload),
terminal, AI agents, settings sync and import (PhpStorm / VS Code / Zed).

## State at the last handoff (2026-11-01)

- Published releases: **0.1.0-alpha**, **0.2.0-alpha**. A `v0.3.0-alpha` tag exists on GitHub
  but its release build was cancelled (no release) — the user may delete the tag.
- `Cargo.toml` version is `0.3.0-alpha`; **~20 local commits are not pushed**.
- The user asked for **no intermediate builds**: finish the work, then one final release.
  Open question to the user: final version number (recommended **0.4.0-alpha**; plain `0.1.0`
  would sort below the published 0.2.0-alpha, so the in-app updater wouldn't offer it).

### In progress / next steps (in this order)

1. Final release (version per the user's answer), then push and deploy the site.

### Known gaps / ideas after that

JS/TS intelligence (goto/usages/completion for Vue/Inertia), Structural Search & Replace,
three-pane merge window (conflicts are resolved in the editor today), coverage needs Xdebug or
PCOV in the user's project, Run Anything, saved window layouts, UI localization (`i18n.rs`
exists but strings aren't translated), Mermaid in Markdown preview, file watchers, DataGrip-level
DB features (ER diagrams, explain, import/export), HTTP response preview, PR review comments
inline in the diff.
