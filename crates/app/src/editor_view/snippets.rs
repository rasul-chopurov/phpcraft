//! Templates expanded with Tab in PHP files: postfix templates after an expression
//! (`$user.if`, `$items.foreach`, `$x.return`…), live templates for a short word at the start
//! of a statement (`pubf`, `fore`, `ctor`…), and Surround With (⌘⌥T) for selected lines.

use gpui::{Context, Window};

use super::intel_ui::{MenuAction, MenuItem};
use super::{EditorView, SurroundWith};

/// `$END$` marks where the caret goes; `\t` is one indentation level.
const POSTFIX: &[(&str, &str)] = &[
    ("if", "if (EXPR) {\n\t$END$\n}"),
    ("notnull", "if (EXPR !== null) {\n\t$END$\n}"),
    ("null", "if (EXPR === null) {\n\t$END$\n}"),
    ("isset", "if (isset(EXPR)) {\n\t$END$\n}"),
    ("empty", "if (empty(EXPR)) {\n\t$END$\n}"),
    ("foreach", "foreach (EXPR as ITEM) {\n\t$END$\n}"),
    ("return", "return EXPR;$END$"),
    ("var", "$value = EXPR;$END$"),
    ("echo", "echo EXPR;$END$"),
    ("dd", "dd(EXPR);$END$"),
    ("dump", "dump(EXPR);$END$"),
    ("throw", "throw EXPR;$END$"),
    ("not", "!EXPR$END$"),
    ("par", "(EXPR)$END$"),
];

const LIVE: &[(&str, &str)] = &[
    ("fore", "foreach ($END$ as $value) {\n}"),
    ("forek", "foreach ($END$ as $key => $value) {\n}"),
    ("fori", "for ($i = 0; $i < $END$; $i++) {\n}"),
    ("pubf", "public function $END$()\n{\n}"),
    ("prof", "protected function $END$()\n{\n}"),
    ("prif", "private function $END$()\n{\n}"),
    ("pubsf", "public static function $END$()\n{\n}"),
    ("fun", "function $END$()\n{\n}"),
    ("ctor", "public function __construct($END$)\n{\n}"),
    ("thr", "throw new $END$;"),
    ("eco", "echo $END$;"),
    ("ife", "if ($END$) {\n} else {\n}"),
    ("tryc", "try {\n\t$END$\n} catch (\\Throwable $e) {\n}"),
];

const SURROUND: &[(&str, &str, &str)] = &[
    ("if", "if ($END$) {\n", "}"),
    ("if / else", "if ($END$) {\n", "} else {\n}"),
    ("foreach", "foreach ($END$ as $value) {\n", "}"),
    ("while", "while ($END$) {\n", "}"),
    ("try / catch", "try {\n", "} catch (\\Throwable $e) {\n\t$END$\n}"),
    ("function", "function $END$()\n{\n", "}"),
];

/// The expression ending at the end of `text`: variables, calls, `->`, `::`, `[...]`.
fn expression_before(text: &str) -> Option<usize> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut i = chars.len();
    let mut depth = 0i32;
    while i > 0 {
        let (_, c) = chars[i - 1];
        match c {
            ')' | ']' => depth += 1,
            '(' | '[' if depth > 0 => depth -= 1,
            _ if depth > 0 => {}
            c if c.is_alphanumeric() || c == '_' || c == '$' || c == '\\' => {}
            '>' if i >= 2 && chars[i - 2].1 == '-' => i -= 1,
            ':' if i >= 2 && chars[i - 2].1 == ':' => i -= 1,
            _ => break,
        }
        i -= 1;
    }
    let start = chars.get(i).map_or(text.len(), |(b, _)| *b);
    (start < text.len() && depth == 0).then_some(start)
}

/// A loop variable for `$items` → `$item`.
fn singular(expr: &str) -> String {
    let name = expr.rsplit(['$', '>', ':']).next().unwrap_or("item").trim_end_matches(')').trim_end_matches('(');
    let name = name.strip_suffix("ies").map(|s| format!("{s}y")).or_else(|| name.strip_suffix('s').map(str::to_string)).filter(|s| !s.is_empty() && s.chars().all(|c| c.is_alphanumeric() || c == '_'));
    format!("${}", name.unwrap_or_else(|| "item".into()))
}

impl EditorView {
    /// Expands a template at the caret; false when there is none (Tab inserts indentation).
    pub(super) fn expand_template(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.buffer.is_php() || self.buffer.has_extra_carets() || !self.buffer.selection.is_empty() {
            return false;
        }
        let head = self.buffer.selection.head;
        let (line, _) = self.buffer.char_to_line_col(head);
        let line_start = self.buffer.line_to_char(line);
        let before = self.buffer.slice(line_start..head);
        let indent: String = before.chars().take_while(|c| *c == ' ' || *c == '\t').collect();
        // Postfix: `<expr>.<key>`.
        if let Some(dot) = before.rfind('.') {
            let key = &before[dot + 1..];
            if let Some((_, template)) = POSTFIX.iter().find(|(k, _)| *k == key)
                && let Some(start) = expression_before(&before[..dot])
            {
                let expr = &before[start..dot];
                let body = template.replace("ITEM", &singular(expr)).replace("EXPR", expr);
                let from = line_start + before[..start].chars().count();
                self.insert_template(from..head, &body, &indent, cx);
                return true;
            }
        }
        // Live template: a lone word at the start of a statement.
        let word_start = before.trim_end_matches(|c: char| c.is_alphanumeric()).len();
        let word = &before[word_start..];
        if before[..word_start].trim().is_empty()
            && let Some((_, template)) = LIVE.iter().find(|(k, _)| *k == word)
        {
            let from = line_start + before[..word_start].chars().count();
            self.insert_template(from..head, template, &indent, cx);
            return true;
        }
        false
    }

    /// Replaces `range` with `template`, indenting its lines and placing the caret at `$END$`.
    fn insert_template(&mut self, range: std::ops::Range<usize>, template: &str, indent: &str, cx: &mut Context<Self>) {
        let unit = self.buffer.indent_unit().to_string();
        let text = template.replace('\t', &unit).replace('\n', &format!("\n{indent}"));
        let caret = text.find("$END$").unwrap_or(text.len());
        let text = text.replace("$END$", "");
        let start_byte = self.buffer.char_to_byte(range.start);
        let end_byte = self.buffer.char_to_byte(range.end);
        self.buffer.apply_edits(vec![(start_byte..end_byte, text)], start_byte);
        let at = self.buffer.byte_to_char(start_byte + caret);
        self.buffer.move_to(at, false);
        self.changed(cx);
    }

    pub(super) fn surround_with(&mut self, _: &SurroundWith, _: &mut Window, cx: &mut Context<Self>) {
        let items = SURROUND.iter().enumerate().map(|(ix, (label, ..))| MenuItem::new(*label, "", "{", MenuAction::Surround(ix))).collect();
        self.show_menu(items, false, cx);
    }

    /// Wraps the selected lines (or the caret line) in the chosen block.
    pub(super) fn apply_surround(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some((_, open, close)) = SURROUND.get(ix) else { return };
        let range = self.buffer.selection.range();
        let first = self.buffer.char_to_line_col(range.start).0;
        let mut last = self.buffer.char_to_line_col(range.end).0;
        if last > first && self.buffer.char_to_line_col(range.end).1 == 0 {
            last -= 1;
        }
        let indent: String = self.buffer.line(first).chars().take_while(|c| *c == ' ' || *c == '\t').collect();
        let unit = self.buffer.indent_unit().to_string();
        let body: Vec<String> = (first..=last).map(|l| {
            let text = self.buffer.line(l);
            if text.trim().is_empty() { String::new() } else { format!("{unit}{text}") }
        }).collect();
        let block = |s: &str| s.replace('\t', &unit).lines().map(|l| format!("{indent}{l}")).collect::<Vec<_>>().join("\n");
        let template = format!("{}\n{}\n{}", block(open).trim_end(), body.join("\n"), block(close));
        let caret = template.find("$END$").unwrap_or(0);
        let text = template.replace("$END$", "");
        let start = self.buffer.line_to_byte(first);
        let end = self.buffer.line_to_byte(last) + self.buffer.line(last).len();
        self.buffer.apply_edits(vec![(start..end, text)], start);
        let at = self.buffer.byte_to_char(start + caret);
        self.buffer.move_to(at, false);
        self.changed(cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_expressions_and_loop_names() {
        let text = "    $repo->find($id)['x']";
        assert_eq!(&text[expression_before(text).unwrap()..], "$repo->find($id)['x']");
        assert_eq!(&"return User::query()"[expression_before("return User::query()").unwrap()..], "User::query()");
        assert_eq!(singular("$this->categories"), "$category");
        assert_eq!(singular("$items"), "$item");
        assert_eq!(singular("getAll()"), "$item");
    }
}
