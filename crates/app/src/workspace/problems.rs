//! Problems (⌘6): the current file's inspection problems; Code ▸ Inspect Code… checks every
//! PHP file of the project in the background; TODO lists `TODO` / `FIXME` comments. All open
//! as a list with a preview.

use gpui::{Context, Window, actions};

use super::Workspace;
use crate::dialogs::{NavItem, NavMode, NavTarget};

actions!(workspace, [ShowProblems, InspectCode, ShowTodo]);

fn icon(severity: php_analysis::Severity) -> &'static str {
    match severity {
        php_analysis::Severity::Error => "⛔",
        php_analysis::Severity::Warning => "⚠",
        php_analysis::Severity::Weak => "·",
    }
}

impl Workspace {
    fn problem_items(&self, found: Vec<(std::path::PathBuf, String, php_analysis::Problem)>) -> Vec<NavItem> {
        let root = self.project.root().to_path_buf();
        let mut found = found;
        found.sort_by(|a, b| a.2.severity.cmp(&b.2.severity).then(a.0.cmp(&b.0)).then(a.2.range.start.cmp(&b.2.range.start)));
        found
            .into_iter()
            .map(|(path, text, problem)| {
                let line = text[..problem.range.start.min(text.len())].matches('\n').count() + 1;
                let file = path.strip_prefix(&root).unwrap_or(&path).display().to_string();
                NavItem {
                    label: problem.message,
                    detail: format!("{file}:{line}"),
                    icon: icon(problem.severity),
                    target: NavTarget::Location(php_index::Location { path, range: problem.range }),
                }
            })
            .collect()
    }

    pub(super) fn show_problems(&mut self, _: &ShowProblems, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get(self.active).cloned() else { return };
        let (path, text) = {
            let buffer = &tab.read(cx).buffer;
            let Some(path) = buffer.path().map(|p| p.to_path_buf()) else { return };
            (path, buffer.text())
        };
        // The editor's list includes PHPStan / Psalm / PHPCS findings from the last save.
        let found = tab.update(cx, |editor, _| editor.all_problems()).into_iter().map(|p| (path.clone(), text.clone(), p)).collect();
        let items = self.problem_items(found);
        self.open_list(NavMode::Problems, items, window, cx);
    }

    pub(super) fn inspect_code(&mut self, _: &InspectCode, window: &mut Window, cx: &mut Context<Self>) {
        let intel = self.intel.clone();
        let root = self.project.root().to_path_buf();
        let task = self.begin_task("Inspecting code…", cx);
        cx.spawn_in(window, async move |this, cx| {
            let found = cx
                .background_executor()
                .spawn(async move {
                    let mut found = Vec::new();
                    for path in search::list_files(&root, false) {
                        if !path.extension().is_some_and(|e| e == "php") || path.to_string_lossy().ends_with(".blade.php") {
                            continue;
                        }
                        let Ok(text) = std::fs::read_to_string(&path) else { continue };
                        for problem in intel.problems(&path, &text) {
                            found.push((path.clone(), text.clone(), problem));
                        }
                    }
                    found
                })
                .await;
            this.update_in(cx, |this, window, cx| {
                this.end_task(task, cx);
                let items = this.problem_items(found);
                this.open_list(NavMode::Problems, items, window, cx);
            })
            .ok();
        })
        .detach();
    }

    /// View ▸ TODO: `TODO` and `FIXME` comments in the project (vendor/ excluded).
    pub(super) fn show_todo(&mut self, _: &ShowTodo, window: &mut Window, cx: &mut Context<Self>) {
        let root = self.project.root().to_path_buf();
        cx.spawn_in(window, async move |this, cx| {
            let found = cx
                .background_executor()
                .spawn(async move {
                    let mut query = search::TextQuery::new(r"\b(TODO|FIXME)\b");
                    query.regex = true;
                    query.case_sensitive = true;
                    let options = search::FileSearch { max_results: 2000, ..Default::default() };
                    search::search_files(&root, &query, &options).unwrap_or_default().into_iter().map(|m| (root.clone(), m)).collect::<Vec<_>>()
                })
                .await;
            this.update_in(cx, |this, window, cx| {
                let items = found
                    .into_iter()
                    .map(|(root, m)| {
                        // From the TODO / FIXME word to the end of the line.
                        let at = m.ranges.first().map_or(0, |r| r.start);
                        let label = m.text.get(at..).unwrap_or(&m.text).trim().to_string();
                        let file = m.path.strip_prefix(&root).unwrap_or(&m.path).display().to_string();
                        NavItem {
                            label: label.trim_end_matches(['*', '/', ' ']).to_string(),
                            detail: format!("{file}:{}", m.line + 1),
                            icon: "☐",
                            target: NavTarget::Location(php_index::Location { path: m.path, range: m.offset..m.offset }),
                        }
                    })
                    .collect();
                this.open_list(NavMode::Todo, items, window, cx);
            })
            .ok();
        })
        .detach();
    }
}
