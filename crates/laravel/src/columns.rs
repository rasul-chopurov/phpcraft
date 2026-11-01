//! Column names in query-builder strings: `User::where('email', ...)`, `orderBy('name')`,
//! `pluck('id')` — completion and navigation to the migration column.

use std::collections::HashMap;
use std::path::PathBuf;
use std::ops::Range;

use php_analysis::{Callee, CompletionItem, CompletionKind, StringArg, StringProvider};
use php_index::{Location, Member, MemberKind};

use crate::helper::ModelHelp;

const COLUMN_METHODS: &[&str] = &[
    "where", "orWhere", "whereNot", "orWhereNot", "whereIn", "whereNotIn", "orWhereIn", "whereNull", "whereNotNull",
    "orWhereNull", "whereBetween", "whereNotBetween", "whereDate", "whereMonth", "whereYear", "whereDay", "whereTime",
    "whereColumn", "whereLike", "orderBy", "orderByDesc", "latest", "oldest", "groupBy", "pluck", "value", "firstWhere",
    "select", "addSelect", "increment", "decrement", "sum", "avg", "min", "max", "having", "distinct", "whereJsonContains",
];

#[derive(Default)]
pub struct ColumnProvider {
    /// Lowercased model FQN -> (column, declaration).
    columns: HashMap<String, Vec<(String, String, Option<(PathBuf, Range<usize>)>)>>,
}

impl ColumnProvider {
    pub fn new(helpers: &[ModelHelp]) -> Self {
        let mut columns = HashMap::new();
        for help in helpers {
            let list = help
                .members
                .iter()
                .filter(|m: &&Member| m.kind == MemberKind::Property && m.source.is_some())
                .map(|m| (m.name.clone(), m.detail.clone(), m.source.as_ref().map(|p| (p.to_path_buf(), m.range.clone()))))
                .collect();
            columns.insert(help.fqn.to_ascii_lowercase(), list);
        }
        Self { columns }
    }

    fn for_arg(&self, arg: &StringArg) -> Option<&Vec<(String, String, Option<(PathBuf, Range<usize>)>)>> {
        let (class, method) = match &arg.callee {
            Callee::Static { class: Some(class), name } | Callee::Method { class: Some(class), name } => (class, name),
            _ => return None,
        };
        if !COLUMN_METHODS.iter().any(|m| m.eq_ignore_ascii_case(method)) {
            return None;
        }
        self.columns
            .get(&class.to_ascii_lowercase())
            .or_else(|| self.columns.get(&arg.chain_root.as_deref()?.to_ascii_lowercase()))
    }
}

impl StringProvider for ColumnProvider {
    fn goto(&self, arg: &StringArg) -> Vec<Location> {
        self.for_arg(arg)
            .into_iter()
            .flatten()
            .filter(|(name, _, _)| *name == arg.value)
            .filter_map(|(_, _, at)| at.clone())
            .map(|(path, range)| Location { path, range })
            .collect()
    }

    fn complete(&self, arg: &StringArg) -> Vec<CompletionItem> {
        self.for_arg(arg)
            .into_iter()
            .flatten()
            .map(|(name, ty, _)| CompletionItem::simple(name.clone(), ty.clone(), CompletionKind::Property, arg.range.clone()))
            .collect()
    }
}
