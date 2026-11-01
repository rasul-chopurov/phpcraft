//! Quick Documentation (F1 / Ctrl+Q) and Parameter Info (⌘P / Ctrl+P): a popup under the
//! caret with the declaration's signature and doc comment, or the call's parameters with the
//! current one in bold. Moving the caret or Esc closes it; Parameter Info follows typing.

use gpui::{AnyElement, Context, FontWeight, HighlightStyle, StyledText, Window, anchored, deferred, div, prelude::*, px};

use super::{CloseInfo, EditorView, ParameterInfo, QuickDocumentation};
use crate::theme;

pub(super) enum Info {
    Doc { signature: String, comment: String, file: String },
    Params { params: String, active: usize },
}

impl EditorView {
    pub(super) fn quick_documentation(&mut self, _: &QuickDocumentation, _: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.path() else { return };
        let at = self.caret_byte();
        let root = self.intel.root().to_path_buf();
        let doc = self.intel.with_doc(&path, &self.buffer.text(), |doc, index| php_analysis::documentation(doc, index, at)).flatten();
        self.info = doc.map(|d| Info::Doc {
            file: d.location.path.strip_prefix(&root).unwrap_or(&d.location.path).display().to_string(),
            signature: d.signature,
            comment: d.comment,
        });
        cx.notify();
    }

    pub(super) fn parameter_info(&mut self, _: &ParameterInfo, _: &mut Window, cx: &mut Context<Self>) {
        self.refresh_parameter_info(true, cx);
    }

    /// Recomputes Parameter Info (after typing, when it is open).
    pub(super) fn refresh_parameter_info(&mut self, open: bool, cx: &mut Context<Self>) {
        if !open && !matches!(self.info, Some(Info::Params { .. })) {
            return;
        }
        let Some(path) = self.path() else { return };
        let at = self.caret_byte();
        let found = self.intel.with_doc(&path, &self.buffer.text(), |doc, index| php_analysis::parameter_info(doc, index, at)).flatten();
        self.info = found.map(|(params, active)| Info::Params { params: if params.is_empty() { "<no parameters>".into() } else { params }, active });
        cx.notify();
    }

    pub(super) fn close_info(&mut self, _: &CloseInfo, _: &mut Window, cx: &mut Context<Self>) {
        self.info = None;
        cx.notify();
    }

    pub(super) fn render_info(&self) -> Option<AnyElement> {
        let info = self.info.as_ref()?;
        let position = self.caret_point?;
        let body = match info {
            Info::Doc { signature, comment, file } => div()
                .flex()
                .flex_col()
                .gap_2()
                .child(div().text_color(theme::muted()).text_size(px(11.)).child(file.clone()))
                .child(div().font_family(theme::editor_font()).text_color(theme::text()).child(signature.clone()))
                .when(!comment.is_empty(), |d| {
                    d.child(div().text_color(theme::ui_text()).children(comment.lines().map(|l| div().child(l.to_string()))))
                }),
            Info::Params { params, active } => {
                // Bold the parameter being typed.
                let mut start = 0;
                let mut ranges = Vec::new();
                let mut depth = 0;
                for (i, c) in params.char_indices() {
                    match c {
                        '(' | '[' => depth += 1,
                        ')' | ']' => depth -= 1,
                        ',' if depth == 0 => {
                            ranges.push(start..i);
                            start = i + 1;
                        }
                        _ => {}
                    }
                }
                ranges.push(start..params.len());
                let highlight = ranges.get(*active).cloned().map(|r| {
                    let trimmed = r.start + (params[r.clone()].len() - params[r.clone()].trim_start().len());
                    vec![(trimmed..r.end, HighlightStyle { font_weight: Some(FontWeight::BOLD), color: Some(theme::accent()), ..Default::default() })]
                });
                div().font_family(theme::editor_font()).text_color(theme::text()).child(StyledText::new(params.clone()).with_highlights(highlight.unwrap_or_default()))
            }
        };
        Some(
            deferred(
                anchored().position(position).child(
                    div()
                        .occlude()
                        .max_w(px(640.))
                        .px_3()
                        .py_2()
                        .bg(theme::panel_bg())
                        .border_1()
                        .border_color(theme::border())
                        .rounded_md()
                        .shadow_lg()
                        .text_size(px(13.))
                        .child(body),
                ),
            )
            .with_priority(1)
            .into_any_element(),
        )
    }
}
