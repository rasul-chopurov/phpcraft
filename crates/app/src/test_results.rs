//! Test results from a JUnit XML report (`--log-junit`, written by PHPUnit, Pest and
//! `artisan test`), and line coverage from a Clover report (`--coverage-clover`).

use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Outcome {
    Failed,
    Error,
    Skipped,
    Passed,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TestCase {
    pub name: String,
    pub class: String,
    pub file: Option<PathBuf>,
    pub line: Option<usize>,
    pub seconds: f64,
    pub outcome: Outcome,
    /// The failure or error message (first lines).
    pub message: String,
}

/// `name="value"` attributes of one XML tag.
fn attribute(tag: &str, name: &str) -> Option<String> {
    let needle = format!(" {name}=\"");
    let start = tag.find(&needle)? + needle.len();
    let end = tag[start..].find('"')? + start;
    Some(unescape(&tag[start..end]))
}

fn unescape(s: &str) -> String {
    s.replace("&quot;", "\"").replace("&apos;", "'").replace("&lt;", "<").replace("&gt;", ">").replace("&#10;", "\n").replace("&amp;", "&")
}

/// Every `<testcase>` of the report.
pub fn parse_junit(xml: &str) -> Vec<TestCase> {
    let mut cases = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find("<testcase ") {
        rest = &rest[start..];
        let tag_end = rest.find('>').unwrap_or(rest.len());
        let tag = &rest[..tag_end];
        let self_closing = tag.ends_with('/');
        let body_end = if self_closing { tag_end } else { rest.find("</testcase>").unwrap_or(tag_end) };
        let body = &rest[tag_end.min(body_end)..body_end];
        let (outcome, message) = if let Some(i) = body.find("<failure") {
            (Outcome::Failed, failure_text(&body[i..]))
        } else if let Some(i) = body.find("<error") {
            (Outcome::Error, failure_text(&body[i..]))
        } else if body.contains("<skipped") {
            (Outcome::Skipped, String::new())
        } else {
            (Outcome::Passed, String::new())
        };
        let file_attr = attribute(tag, "file");
        // Pest writes `file="tests/Feature/X.php::it works"`.
        let file = file_attr.as_deref().map(|f| PathBuf::from(f.split("::").next().unwrap_or(f)));
        cases.push(TestCase {
            name: attribute(tag, "name").unwrap_or_default(),
            class: attribute(tag, "class").or_else(|| attribute(tag, "classname")).unwrap_or_default(),
            file,
            line: attribute(tag, "line").and_then(|l| l.parse().ok()),
            seconds: attribute(tag, "time").and_then(|t| t.parse().ok()).unwrap_or(0.0),
            outcome,
            message,
        });
        rest = &rest[body_end.max(1)..];
    }
    cases
}

fn failure_text(element: &str) -> String {
    let open_end = element.find('>').unwrap_or(element.len());
    let tag = &element[..open_end];
    let close = element.find("</").unwrap_or(element.len());
    let text = if close > open_end { unescape(&element[open_end + 1..close]) } else { attribute(tag, "message").unwrap_or_default() };
    text.lines().filter(|l| !l.trim().is_empty()).take(6).collect::<Vec<_>>().join("\n")
}

/// Line coverage per file: (0-based line, covered).
pub type Coverage = std::collections::HashMap<PathBuf, Vec<(usize, bool)>>;

static COVERAGE: std::sync::Mutex<Option<Coverage>> = std::sync::Mutex::new(None);

/// Statement lines of each `<file>` in a Clover report.
pub fn parse_clover(xml: &str) -> Coverage {
    let mut coverage = Coverage::new();
    let mut rest = xml;
    while let Some(start) = rest.find("<file ") {
        rest = &rest[start..];
        let tag_end = rest.find('>').unwrap_or(rest.len());
        let name = attribute(&rest[..tag_end], "name");
        let end = rest.find("</file>").unwrap_or(rest.len());
        let body = &rest[tag_end..end];
        if let Some(name) = name {
            let lines = body
                .split("<line ")
                .skip(1)
                .filter_map(|l| {
                    let tag = format!(" {}", &l[..l.find('>').unwrap_or(l.len())]);
                    if attribute(&tag, "type").as_deref() != Some("stmt") {
                        return None;
                    }
                    let num: usize = attribute(&tag, "num")?.parse().ok()?;
                    let count: u64 = attribute(&tag, "count")?.parse().ok()?;
                    Some((num.saturating_sub(1), count > 0))
                })
                .collect();
            coverage.insert(PathBuf::from(name), lines);
        }
        rest = &rest[end.max(1)..];
    }
    coverage
}

/// Replaces the coverage shown in editors.
pub fn set_coverage(coverage: Option<Coverage>) {
    *COVERAGE.lock().unwrap() = coverage;
}

/// The covered / uncovered lines of `path` from the last coverage run.
pub fn coverage_for(path: &std::path::Path) -> Vec<(usize, bool)> {
    COVERAGE.lock().unwrap().as_ref().and_then(|c| c.get(path).cloned()).unwrap_or_default()
}

/// Covered statements / all statements, in percent.
pub fn percent(coverage: &Coverage) -> f64 {
    let (covered, total) = coverage.values().flatten().fold((0usize, 0usize), |(c, t), (_, hit)| (c + *hit as usize, t + 1));
    if total == 0 { 0.0 } else { covered as f64 * 100.0 / total as f64 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_phpunit_and_pest_reports() {
        let xml = r#"<?xml version="1.0"?>
<testsuites><testsuite name="Unit">
  <testcase name="test_adds" class="Tests\Unit\MathTest" file="/p/tests/Unit/MathTest.php" line="12" assertions="1" time="0.002"/>
  <testcase name="test_fails" class="Tests\Unit\MathTest" file="/p/tests/Unit/MathTest.php" line="18" time="0.01">
    <failure type="PHPUnit\Framework\ExpectationFailedException">Tests\Unit\MathTest::test_fails
Failed asserting that 3 is identical to 4.

/p/tests/Unit/MathTest.php:20</failure>
  </testcase>
  <testcase name="it works" file="tests/Feature/HomeTest.php::it works" class="Tests\Feature\HomeTest" time="0.1"><skipped/></testcase>
</testsuite></testsuites>"#;
        let cases = parse_junit(xml);
        assert_eq!(cases.len(), 3);
        assert_eq!(cases[0].outcome, Outcome::Passed);
        assert_eq!(cases[0].line, Some(12));
        assert_eq!(cases[1].outcome, Outcome::Failed);
        assert!(cases[1].message.contains("Failed asserting that 3 is identical to 4."));
        assert_eq!(cases[2].outcome, Outcome::Skipped);
        assert_eq!(cases[2].file.as_deref(), Some(std::path::Path::new("tests/Feature/HomeTest.php")));
    }

    #[test]
    fn parses_clover() {
        let xml = r#"<coverage><project><file name="/p/app/Cart.php"><class name="Cart"/><line num="5" type="method" count="1"/><line num="7" type="stmt" count="3"/><line num="9" type="stmt" count="0"/></file></project></coverage>"#;
        let coverage = parse_clover(xml);
        assert_eq!(coverage[&PathBuf::from("/p/app/Cart.php")], [(6, true), (8, false)]);
        assert_eq!(percent(&coverage), 50.0);
    }
}
