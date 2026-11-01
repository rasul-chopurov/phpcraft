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
