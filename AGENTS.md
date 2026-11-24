# AGENTS.md — PhpCraft

Guide for agents (and people) continuing work on this repository.

## Goal

PhpCraft is a native, offline PHP / Laravel IDE written in Rust on **gpui** (Zed's UI
framework): the feature set of a modern PHP / Laravel IDE without the JVM. Open source
(Apache-2.0), macOS / Windows / Linux. Author: Rasul Chopurov (path.az). The user speaks Russian — reply in
Russian.

Other IDEs are used only as a **feature checklist**. Never copy their code, icons or texts;
everything is written from scratch. (Exception: the phpstorm-stubs package, Apache-2.0,
bundled as data — its name appears only where that license requires attribution; see
`crates/php-index/STUBS-LICENSE`.)

## Workspace layout (`crates/`)

| Crate | What it is |
|---|---|
| `app` | The application (binary `phpcraft`): windows, editor view, workspace, dialogs, menus, keymap |
| `editor` | Text buffer: rope, multi-caret selections, undo/redo groups, anchors, smart pairs, indentation, extend selection |
| `php-syntax` | tree-sitter grammars + highlighting (PHP, HTML, JS/TS, CSS, JSON, YAML), hand-written highlighters (Blade overlay, Twig, SQL, Markdown, .env…), fold ranges, node ranges |
| `php-index` | Project symbol index (classes, members, functions), binary cache, bundled PHP stubs (`stubs.bin.gz`, `stubs.rs`, Apache-2.0) |
| `js-analysis` | JS/TS intelligence: declarations, imports and re-exports, module resolution (tsconfig `paths`/`baseUrl`, Vite `resolve.alias`, `@/`), Vue/Svelte `<script>` blocks (goto, usages, rename, completion, import rewriting on rename) |
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
  `config.rs` (settings.json), `updates.rs` + `self_update.rs` (update check, What's New,
  Update and Restart in place), `run_configs.rs` (run/test configs, PHP interpreter local /
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
- Per-project files live in `.phpcraft/` (session.json, run.json, laravel.json, debug.json,
  deployment.json, shelf/, http/, junit.xml…); app-wide in `config::dir()` (settings, keymap,
  bookmarks, breakpoints, index caches, unpacked stubs). The per-project folder is created
  through `config::project_dir()`, which also writes a `.gitignore` of `*` inside it, so the
  user's repository never sees it.
- Verify with `cargo test --workspace` (≈150 tests) and `cargo build -p phpcraft` without
  warnings. For UI checks run `./target/debug/phpcraft <file>` and screenshot the window by id
  (a Swift `CGWindowListCopyWindowInfo` helper); background windows don't repaint, so state that
  appears after the first frame can't be screenshotted that way.
- `INSPECT_ROOT=/path cargo test -p php-analysis survey -- --ignored --nocapture` surveys
  inspections on a real project (false-positive check).
- Releases: bump `version` in `Cargo.toml` and `VERSION` in `site/src/home.html` +
  `site/phpcraft/index.html`; push; tag `vX.Y.Z-alpha` → GitHub Actions `Release` builds 8
  installers (macOS + Windows). CI (`ci.yml`) must be green first. The site deploys with `./site/deploy.sh`
  (FTP password asked interactively).

## What exists (high level)

Editor (multi-caret, folding, soft wrap, smart pairs, bookmarks, extend selection, templates,
font zoom with ⌘+ / ⌘− / ⌘+scroll, draggable scrollbar with problem marks), search (Find and
Replace in Files, structural search with `$A$` holes, ⌥⌘F),
PHP intelligence (completion with auto-import, goto, find usages, rename, move class, extract / introduce /
inline / safe delete, generate, quick doc, parameter info, hierarchy, call hierarchy,
inspections + PHPStan/Psalm/PHPCS, spell checking of comments and strings (`typo`, ⌥↵ suggestions / save to dictionary), intentions, bundled PHP stubs), Laravel (string navigation,
Blade sections/stacks/props, routes in the gutter, generators incl. DTO / Nova, extract blade
component/view, facade→DI / rules→array / scope→#[Scope] intentions, the Modules window for
laravel-modules with enable/disable and `module:make`), Twig, Git (commit, log
with graph, branches, diff, blame, three-way merge window (BASE pane, line-level auto-merge), compare with revision / clipboard,
interactive rebase, cherry-pick, revert, undo commit, shelve, worktrees, remotes),
Pull Requests (Git ▸ Pull Requests…: list, details, checkout, diff, comment via gh / glab),
Xdebug debugger (conditional breakpoints, evaluate), Docker services, test runner with results and coverage, Composer menu, HTTP
client (+ cURL / Postman / OpenAPI, history), database tools, SFTP deployment (auto upload, per-server picker, drag and drop in both
 directions, an upload chip in the tab bar),
terminal (Run Anything with ⌃⌃: configurations, Artisan, typed shell lines), AI agents, settings
sync and import (VS Code / Zed).

## State at the last handoff (2026-11-01)

- Releases: **0.1.0-alpha**, **0.2.0-alpha**, **0.4.0-alpha** (the feature waves,
  Pull Requests UI, spell checker). A `v0.3.0-alpha` tag exists on GitHub without a release
  (its build was cancelled) — the user may delete it.
- **0.4.1-alpha**: git status polled only while the window is active (Windows users saw cmd
  windows flashing). **0.4.2-alpha**: windows and dialogs fit small screens.
  **0.4.3-alpha**: Update and Restart (self-update in place). **0.5.0-alpha**: three-way merge
  window, JS/TS module resolution with import rewriting, Move Class companions, Import Class,
  session restore with scroll and folds, external-change prompts, saved window layouts,
  Run Anything, structural search, full Russian UI. **0.5.1-alpha**: conditional breakpoints in
  the Debug window, white labels on every dialog button in the dark themes, the per-project
  `.phpcraft` folder kept out of git.
- Releases are built for **macOS and Windows only** (the user paused Linux builds; the site
  doesn't list Linux). `Cargo.toml` version is `0.5.1-alpha`.

### Next steps: known gaps / ideas

coverage needs Xdebug or PCOV in the user's project,
UI localization (`i18n.rs`, Russian: menus, tool windows, dialogs and panels are translated;
inspection messages and intention labels travel as templates so the app translates them, About
is translated), Mermaid in Markdown preview,
DataGrip-level DB features (ER diagrams, explain, import/export), HTTP response preview,
PR review comments inline in the diff, JS type information / hover docs.
