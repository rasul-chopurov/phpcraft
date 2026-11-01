//! Test results after a PHPUnit / Pest run: a summary in the output panel, failures listed
//! with their messages (Run ▸ Show Test Results), and Run ▸ Rerun Failed Tests.

use gpui::{Context, Window, actions};

use super::Workspace;
use super::tools::Output;
use crate::dialogs::{NavItem, NavMode, NavTarget};
use crate::run_configs::{self, Kind, RunConfig};
use crate::test_results::{Outcome, TestCase};

actions!(workspace, [ShowTestResults, RerunFailedTests, RunWithCoverage, HideCoverage]);

impl Workspace {
    /// Reads the JUnit report of the run that just finished.
    pub(super) fn read_test_results(&mut self, cx: &mut Context<Self>) {
        let root = self.project.root().to_path_buf();
        let Ok(xml) = std::fs::read_to_string(root.join(run_configs::JUNIT_REPORT)) else { return };
        let cases = crate::test_results::parse_junit(&xml);
        if cases.is_empty() {
            return;
        }
        let count = |o: Outcome| cases.iter().filter(|c| c.outcome == o).count();
        let failed = count(Outcome::Failed) + count(Outcome::Error);
        let seconds: f64 = cases.iter().map(|c| c.seconds).sum();
        let mut text = format!("{} passed, {failed} failed, {} skipped in {seconds:.2}s", count(Outcome::Passed), count(Outcome::Skipped));
        for case in cases.iter().filter(|c| matches!(c.outcome, Outcome::Failed | Outcome::Error)) {
            text.push_str(&format!("\n\n✗ {}::{}\n{}", short(&case.class), case.name, case.message));
        }
        if let Ok(xml) = std::fs::read_to_string(root.join(run_configs::CLOVER_REPORT)) {
            let coverage = crate::test_results::parse_clover(&xml);
            text = format!("{text}\nLine coverage: {:.1}% (shown in the editor gutter)", crate::test_results::percent(&coverage));
            crate::test_results::set_coverage(Some(coverage));
        }
        self.output = Some(Output { title: "Tests".into(), text, running: false, ok: failed == 0 });
        self.test_results = cases;
        cx.notify();
    }

    fn test_location(&self, case: &TestCase) -> Option<php_index::Location> {
        let file = case.file.as_ref()?;
        let path = if file.is_absolute() { file.clone() } else { self.project.root().join(file) };
        let text = std::fs::read_to_string(&path).ok()?;
        let offset = match case.line {
            Some(line) => text.split_inclusive('\n').take(line.saturating_sub(1)).map(str::len).sum(),
            None => text.find(&case.name).unwrap_or(0),
        };
        Some(php_index::Location { path, range: offset..offset })
    }

    pub(super) fn show_test_results(&mut self, _: &ShowTestResults, window: &mut Window, cx: &mut Context<Self>) {
        let mut cases = self.test_results.clone();
        cases.sort_by_key(|c| c.outcome);
        let items = cases
            .iter()
            .filter_map(|case| {
                let icon = match case.outcome {
                    Outcome::Passed => "✓",
                    Outcome::Skipped => "○",
                    Outcome::Failed | Outcome::Error => "✗",
                };
                Some(NavItem {
                    label: format!("{}::{}", short(&case.class), case.name),
                    detail: if case.message.is_empty() { format!("{:.0} ms", case.seconds * 1000.0) } else { case.message.lines().next().unwrap_or("").to_string() },
                    icon,
                    target: NavTarget::Location(self.test_location(case)?),
                })
            })
            .collect();
        self.open_list(NavMode::Tests, items, window, cx);
    }

    pub(super) fn rerun_failed_tests(&mut self, _: &RerunFailedTests, _: &mut Window, cx: &mut Context<Self>) {
        let failed: Vec<String> = self
            .test_results
            .iter()
            .filter(|c| matches!(c.outcome, Outcome::Failed | Outcome::Error))
            .map(|c| run_configs::regex_escape(&c.name))
            .collect();
        let Some(runner) = run_configs::test_runner(self.project.root()) else { return };
        if failed.is_empty() {
            return;
        }
        let config = RunConfig {
            name: "Failed tests".into(),
            kind: Kind::Test { runner, path: String::new(), filter: failed.join("|") },
            env: Default::default(),
        };
        self.start_run(config, false, cx);
    }
}

fn short(class: &str) -> &str {
    class.rsplit('\\').next().unwrap_or(class)
}

impl Workspace {
    /// Run ▸ Run with Coverage: the context test (or all tests) with Xdebug / PCOV coverage.
    pub(super) fn run_with_coverage(&mut self, _: &RunWithCoverage, _: &mut Window, cx: &mut Context<Self>) {
        let root = self.project.root().to_path_buf();
        let config = self
            .context_run_config(cx)
            .filter(|c| matches!(c.kind, Kind::Test { .. }))
            .or_else(|| run_configs::test_runner(&root).map(|runner| RunConfig { name: "All tests".into(), kind: Kind::Test { runner, path: String::new(), filter: String::new() }, env: Default::default() }));
        let Some(mut config) = config else { return };
        config.name = format!("{} with Coverage", config.name);
        config.env.insert("XDEBUG_MODE".into(), "coverage".into());
        self.start_run(config, false, cx);
    }

    pub(super) fn hide_coverage(&mut self, _: &HideCoverage, _: &mut Window, cx: &mut Context<Self>) {
        crate::test_results::set_coverage(None);
        cx.notify();
    }
}
