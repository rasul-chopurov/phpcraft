//! Inspection problems in the editor: underlined by severity (error red, warning yellow, unused
//! code grey), F2 / ⇧F2 jump between them and show the message.

use gpui::{Context, Window};

use super::info::Info;
use super::{EditorView, NextProblem, PrevProblem};

impl EditorView {
    /// Problems for the current text (recomputed after edits, kept while the index loads).
    pub(super) fn current_problems(&mut self) -> Vec<php_analysis::Problem> {
        let version = self.buffer.version();
        if self.problems.as_ref().is_none_or(|(v, _)| *v != version) {
            let found = match self.path() {
                Some(path) if self.buffer.is_php() && !self.plain => self.intel.problems(&path, &self.buffer.text()),
                _ => Vec::new(),
            };
            let version = if self.intel.is_ready() { version } else { u64::MAX };
            self.problems = Some((version, found));
        }
        let mut problems = self.problems.as_ref().map(|(_, p)| p.clone()).unwrap_or_default();
        // Merge conflicts, in any file: the `<<<<<<<` line is the problem. Until they are
        // resolved the code doesn't parse as intended, so other problems are noise.
        let text = self.buffer.text();
        let conflicts = self.conflicts();
        if !conflicts.is_empty() {
            problems.clear();
        }
        for conflict in conflicts {
            let start = conflict.whole.start;
            let end = text[start..].find('\n').map_or(text.len(), |i| start + i);
            problems.push(php_analysis::Problem {
                range: start..end,
                severity: php_analysis::Severity::Error,
                message: format!("Merge conflict: {} ↔ {} (⌥↵ to resolve)", conflict.ours_label, conflict.theirs_label),
                id: "conflict",
            });
        }
        if let Some((v, external)) = &self.external_problems
            && *v == self.buffer.version()
        {
            problems.extend(external.iter().cloned());
            problems.sort_by_key(|p| p.range.start);
        }
        problems
    }

    /// Every problem of the file (inspections and external analysers), for the Problems list.
    pub fn all_problems(&mut self) -> Vec<php_analysis::Problem> {
        self.current_problems()
    }

    /// Merge conflict blocks in the text, cached per buffer version.
    pub(super) fn conflicts(&mut self) -> Vec<git::Conflict> {
        let version = self.buffer.version();
        if self.conflict_cache.as_ref().is_none_or(|(v, _)| *v != version) {
            let text = self.buffer.text();
            let found = if text.contains("<<<<<<<") { git::conflicts(&text) } else { Vec::new() };
            self.conflict_cache = Some((version, found));
        }
        self.conflict_cache.as_ref().map(|(_, c)| c.clone()).unwrap_or_default()
    }

    /// Accept Yours / Theirs / Both for the conflict at the caret.
    pub(super) fn conflict_resolutions(&mut self) -> Vec<(String, Vec<(std::ops::Range<usize>, String)>)> {
        let at = self.caret_byte();
        let text = self.buffer.text();
        let Some(conflict) = self.conflicts().into_iter().find(|c| c.whole.start <= at && at <= c.whole.end) else { return Vec::new() };
        let (ours, theirs) = (conflict.ours_label.clone(), conflict.theirs_label.clone());
        [
            (format!("Accept Yours ({ours})"), true, false, false),
            (format!("Accept Theirs ({theirs})"), false, true, false),
            ("Accept Both (yours first)".to_string(), true, true, false),
            ("Accept Both (theirs first)".to_string(), true, true, true),
        ]
        .into_iter()
        .map(|(label, o, t, tf)| (label, vec![(conflict.whole.clone(), git::resolve(&text, &conflict, o, t, tf))]))
        .collect()
    }

    /// Problems at the caret, for the intentions menu.
    pub(super) fn problems_at_caret(&mut self) -> Vec<php_analysis::Problem> {
        let at = self.caret_byte();
        self.current_problems().into_iter().filter(|p| p.range.start <= at && at <= p.range.end).collect()
    }

    fn jump_problem(&mut self, forward: bool, cx: &mut Context<Self>) {
        let at = self.caret_byte();
        let problems = self.current_problems();
        let target = if forward {
            problems.iter().find(|p| p.range.start > at).or(problems.first())
        } else {
            problems.iter().rev().find(|p| p.range.start < at).or(problems.last())
        };
        if let Some(problem) = target.cloned() {
            self.reveal_byte(problem.range.start, cx);
            self.info = Some(Info::Message(problem.message));
        }
    }

    pub(super) fn next_problem(&mut self, _: &NextProblem, _: &mut Window, cx: &mut Context<Self>) {
        self.jump_problem(true, cx);
    }

    pub(super) fn prev_problem(&mut self, _: &PrevProblem, _: &mut Window, cx: &mut Context<Self>) {
        self.jump_problem(false, cx);
    }
}
