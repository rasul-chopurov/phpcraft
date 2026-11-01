//! `.sql` files: a bar above the editor picks the data source; ⌘↵ (or Run) runs the selection,
//! or the statement under the caret, in a console tab.

use gpui::{Context, SharedString, Window, actions, div, prelude::*, px};

use super::Workspace;
use crate::icons::{Icon, icon};
use crate::theme;

actions!(workspace, [RunSqlFile]);

pub fn is_sql_file(path: &std::path::Path) -> bool {
    path.extension().is_some_and(|e| e.eq_ignore_ascii_case("sql"))
}

/// The statement around byte `at`: from the previous `;` to the next one. A caret at the end
/// of a line just after `;` means the statement that `;` ends.
pub fn statement_at(text: &str, at: usize) -> &str {
    let mut at = at.min(text.len());
    while !text.is_char_boundary(at) {
        at -= 1;
    }
    let line_rest = text[at..].split('\n').next().unwrap_or("");
    let before = text[..at].trim_end_matches([' ', '\t']);
    if line_rest.trim().is_empty() && before.ends_with(';') {
        at = before.len() - 1;
    }
    let start = text[..at].rfind(';').map_or(0, |i| i + 1);
    let end = text[at..].find(';').map_or(text.len(), |i| at + i);
    text[start..end].trim()
}

impl Workspace {
    fn active_sql_file(&self, cx: &gpui::App) -> bool {
        self.tabs.get(self.active).and_then(|t| t.read(cx).buffer.path().map(is_sql_file)).unwrap_or(false)
    }

    pub(super) fn run_sql_file(&mut self, _: &RunSqlFile, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tabs.get(self.active) else { return };
        let buffer = &tab.read(cx).buffer;
        let selected = buffer.selected_text();
        let sql = if selected.trim().is_empty() {
            let text = buffer.text();
            let at = buffer.char_to_byte(buffer.selection.head);
            statement_at(&text, at).to_string()
        } else {
            selected
        };
        if !sql.trim().is_empty() {
            self.run_in_console(sql.trim().trim_end_matches(';').to_string(), window, cx);
        }
    }

    pub(super) fn render_sql_bar(&mut self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        // Only while the file's own editor is showing (not a console/table/diff tab).
        if !self.active_sql_file(cx) || self.table_active.is_some() || self.diff_active {
            return None;
        }
        self.ensure_db(cx);
        let (sources, selected) = self.db_sources();
        let chips: Vec<_> = sources
            .iter()
            .map(|source| {
                let on = selected == Some(source.id.as_str());
                let id = source.id.clone();
                div()
                    .id(SharedString::from(format!("sql-source-{}", source.id)))
                    .px_2()
                    .py_0p5()
                    .rounded_md()
                    .cursor_pointer()
                    .border_1()
                    .border_color(if on { theme::accent() } else { theme::border() })
                    .text_color(if on { theme::ui_text() } else { theme::muted() })
                    .child(source.name.clone())
                    .on_click(cx.listener(move |this, _, _, cx| this.select_source(id.clone(), cx)))
            })
            .collect();
        let empty = chips.is_empty();
        Some(
            div()
                .flex()
                .items_center()
                .gap_2()
                .px_3()
                .h(px(30.))
                .border_b_1()
                .border_color(theme::border())
                .bg(theme::panel_bg())
                .text_size(px(12.))
                .child(
                    div()
                        .id("sql-run")
                        .flex()
                        .items_center()
                        .gap_1()
                        .px_2()
                        .py_0p5()
                        .rounded_md()
                        .cursor_pointer()
                        .bg(theme::accent())
                        .text_color(gpui::white())
                        .child(icon(Icon::play, 11., gpui::white()))
                        .child(format!("Run  {}", crate::keymap::display("secondary-enter")))
                        .on_click(cx.listener(|this, _, window, cx| this.run_sql_file(&RunSqlFile, window, cx))),
                )
                .child(div().text_color(theme::muted()).child("selection or statement at the caret"))
                .child(div().flex_1())
                .child(icon(Icon::database, 13., theme::muted()))
                .children(chips)
                .when(empty, |d| d.child(div().text_color(theme::muted()).child("no data sources — add one in the Database window"))),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statements() {
        let sql = "SELECT 1;\nSELECT * FROM users WHERE id = 2;\n\nUPDATE x SET y = 1";
        assert_eq!(statement_at(sql, 3), "SELECT 1");
        assert_eq!(statement_at(sql, 20), "SELECT * FROM users WHERE id = 2");
        assert_eq!(statement_at(sql, sql.len()), "UPDATE x SET y = 1");
        assert_eq!(statement_at(sql, 9), "SELECT 1");
    }
}
