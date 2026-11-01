//! Markdown preview for `.md` files: CommonMark (+ tables, task lists, strikethrough) rendered
//! as GPUI blocks next to the editor.

use std::ops::Range;

use gpui::{AnyElement, FontStyle, FontWeight, HighlightStyle, SharedString, StyledText, div, prelude::*, px};
use pulldown_cmark::{Alignment, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

use crate::theme;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
struct SpanStyle {
    bold: bool,
    italic: bool,
    code: bool,
    strike: bool,
    link: bool,
}

/// Inline text with styled ranges.
#[derive(Default, Clone)]
pub struct Inline {
    pub text: String,
    runs: Vec<(Range<usize>, SpanStyle)>,
}

impl Inline {
    fn push(&mut self, text: &str, style: SpanStyle) {
        let start = self.text.len();
        self.text.push_str(text);
        if style != SpanStyle::default() {
            self.runs.push((start..self.text.len(), style));
        }
    }

    fn is_empty(&self) -> bool {
        self.text.trim().is_empty()
    }
}

#[derive(Clone)]
pub enum Block {
    Heading(u8, Inline),
    Paragraph(Inline),
    Code { lang: String, text: String },
    /// Nesting depth, marker (`•`, `1.`, `☐`, `☑`), content.
    Item { depth: usize, marker: String, content: Inline },
    Quote(Inline),
    Rule,
    Table { header: Vec<Inline>, rows: Vec<Vec<Inline>>, align: Vec<Alignment> },
}

/// Parses Markdown into display blocks.
pub fn parse(source: &str) -> Vec<Block> {
    let options = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let mut blocks = Vec::new();
    let mut inline = Inline::default();
    let mut style = SpanStyle::default();
    // (ordered next number) per list level
    let mut lists: Vec<Option<u64>> = Vec::new();
    let mut item_marker: Option<String> = None;
    let mut quote = 0usize;
    let mut code: Option<(String, String)> = None;
    let mut heading: Option<u8> = None;
    let mut table: Option<(Vec<Inline>, Vec<Vec<Inline>>, Vec<Alignment>, bool)> = None;
    let mut row: Vec<Inline> = Vec::new();

    let flush = |blocks: &mut Vec<Block>, inline: &mut Inline, lists: &[Option<u64>], marker: &mut Option<String>, quote: usize, heading: Option<u8>| {
        let content = std::mem::take(inline);
        if let Some(level) = heading {
            blocks.push(Block::Heading(level, content));
        } else if let Some(m) = marker.take() {
            blocks.push(Block::Item { depth: lists.len().max(1), marker: m, content });
        } else if content.is_empty() {
        } else if quote > 0 {
            blocks.push(Block::Quote(content));
        } else {
            blocks.push(Block::Paragraph(content));
        }
    };

    for event in Parser::new_ext(source, options) {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                heading = Some(match level {
                    HeadingLevel::H1 => 1,
                    HeadingLevel::H2 => 2,
                    HeadingLevel::H3 => 3,
                    HeadingLevel::H4 => 4,
                    HeadingLevel::H5 => 5,
                    HeadingLevel::H6 => 6,
                })
            }
            Event::End(TagEnd::Heading(_)) => {
                flush(&mut blocks, &mut inline, &lists, &mut item_marker, quote, heading);
                heading = None;
            }
            Event::Start(Tag::Paragraph) => {}
            Event::End(TagEnd::Paragraph) => flush(&mut blocks, &mut inline, &lists, &mut item_marker, quote, None),
            Event::Start(Tag::BlockQuote(_)) => quote += 1,
            Event::End(TagEnd::BlockQuote(_)) => {
                flush(&mut blocks, &mut inline, &lists, &mut item_marker, quote, None);
                quote = quote.saturating_sub(1);
            }
            Event::Start(Tag::CodeBlock(kind)) => {
                let lang = match kind {
                    CodeBlockKind::Fenced(lang) => lang.to_string(),
                    CodeBlockKind::Indented => String::new(),
                };
                code = Some((lang, String::new()));
            }
            Event::End(TagEnd::CodeBlock) => {
                if let Some((lang, text)) = code.take() {
                    blocks.push(Block::Code { lang, text: text.trim_end_matches('\n').to_string() });
                }
            }
            Event::Start(Tag::List(start)) => {
                // A nested list ends the parent item's text.
                if item_marker.is_some() {
                    flush(&mut blocks, &mut inline, &lists, &mut item_marker, quote, None);
                }
                lists.push(start);
            }
            Event::End(TagEnd::List(_)) => {
                lists.pop();
            }
            Event::Start(Tag::Item) => {
                let marker = match lists.last_mut() {
                    Some(Some(n)) => {
                        let m = format!("{n}.");
                        *n += 1;
                        m
                    }
                    _ => "•".to_string(),
                };
                item_marker = Some(marker);
            }
            Event::End(TagEnd::Item) => {
                if item_marker.is_some() {
                    flush(&mut blocks, &mut inline, &lists, &mut item_marker, quote, None);
                }
            }
            Event::TaskListMarker(done) => item_marker = Some(if done { "☑".into() } else { "☐".into() }),
            Event::Start(Tag::Table(align)) => table = Some((Vec::new(), Vec::new(), align, false)),
            Event::End(TagEnd::Table) => {
                if let Some((header, rows, align, _)) = table.take() {
                    blocks.push(Block::Table { header, rows, align });
                }
            }
            Event::Start(Tag::TableHead) => {}
            Event::End(TagEnd::TableHead) => {
                if let Some(t) = table.as_mut() {
                    t.0 = std::mem::take(&mut row);
                    t.3 = true;
                }
            }
            Event::Start(Tag::TableRow) => {}
            Event::End(TagEnd::TableRow) => {
                if let Some(t) = table.as_mut() {
                    t.1.push(std::mem::take(&mut row));
                }
            }
            Event::Start(Tag::TableCell) => {}
            Event::End(TagEnd::TableCell) => row.push(std::mem::take(&mut inline)),
            Event::Start(Tag::Emphasis) => style.italic = true,
            Event::End(TagEnd::Emphasis) => style.italic = false,
            Event::Start(Tag::Strong) => style.bold = true,
            Event::End(TagEnd::Strong) => style.bold = false,
            Event::Start(Tag::Strikethrough) => style.strike = true,
            Event::End(TagEnd::Strikethrough) => style.strike = false,
            Event::Start(Tag::Link { .. }) => style.link = true,
            Event::End(TagEnd::Link) => style.link = false,
            Event::Start(Tag::Image { .. }) => inline.push("🖼 ", style),
            Event::Text(text) => match code.as_mut() {
                Some((_, buf)) => buf.push_str(&text),
                None => inline.push(&text, style),
            },
            Event::Code(text) => inline.push(&text, SpanStyle { code: true, ..style }),
            Event::Html(html) | Event::InlineHtml(html) => {
                let text = html.trim();
                if !text.starts_with('<') && !text.is_empty() {
                    inline.push(text, style);
                }
            }
            Event::SoftBreak => inline.push(" ", style),
            Event::HardBreak => inline.push("\n", style),
            Event::Rule => blocks.push(Block::Rule),
            _ => {}
        }
    }
    flush(&mut blocks, &mut inline, &lists, &mut item_marker, quote, None);
    blocks
}

fn styled(inline: &Inline) -> StyledText {
    let code_bg = theme::hover();
    let highlights = inline
        .runs
        .iter()
        .map(|(range, s)| {
            let mut h = HighlightStyle::default();
            if s.bold {
                h.font_weight = Some(FontWeight::BOLD);
            }
            if s.italic {
                h.font_style = Some(FontStyle::Italic);
            }
            if s.code {
                h.background_color = Some(code_bg);
                h.color = Some(theme::accent());
            }
            if s.link {
                h.color = Some(theme::accent());
                h.underline = Some(gpui::UnderlineStyle { thickness: px(1.), color: Some(theme::accent()), wavy: false });
            }
            if s.strike {
                h.strikethrough = Some(gpui::StrikethroughStyle { thickness: px(1.), color: Some(theme::muted()) });
            }
            (range.clone(), h)
        })
        .collect::<Vec<_>>();
    StyledText::new(SharedString::from(inline.text.clone())).with_highlights(highlights)
}

fn render_block(block: &Block) -> AnyElement {
    match block {
        Block::Heading(level, text) => {
            let size = match level {
                1 => 26.,
                2 => 21.,
                3 => 17.,
                _ => 15.,
            };
            div()
                .mt(px(if *level <= 2 { 14. } else { 8. }))
                .pb(px(if *level <= 2 { 6. } else { 0. }))
                .when(*level <= 2, |d| d.border_b_1().border_color(theme::border()))
                .text_size(px(size))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme::text())
                .child(styled(text))
                .into_any_element()
        }
        Block::Paragraph(text) => div().child(styled(text)).into_any_element(),
        Block::Quote(text) => div()
            .pl_3()
            .border_l_4()
            .border_color(theme::border_strong())
            .text_color(theme::muted())
            .child(styled(text))
            .into_any_element(),
        Block::Item { depth, marker, content } => div()
            .flex()
            .gap_2()
            .pl(px(*depth as f32 * 18. - 8.))
            .child(div().flex_none().min_w(px(14.)).text_color(theme::muted()).child(marker.clone()))
            .child(div().flex_1().child(styled(content)))
            .into_any_element(),
        Block::Code { lang, text } => div()
            .relative()
            .p_3()
            .rounded_md()
            .bg(theme::panel_bg())
            .border_1()
            .border_color(theme::border())
            .font_family(theme::editor_font())
            .text_size(px(12.5))
            .text_color(theme::text())
            .when(!lang.is_empty(), |d| {
                d.child(div().absolute().top(px(4.)).right(px(8.)).text_size(px(11.)).text_color(theme::muted()).child(lang.clone()))
            })
            .children(text.lines().map(|line| div().whitespace_nowrap().child(SharedString::from(if line.is_empty() { " ".to_string() } else { line.replace('\t', "    ") }))))
            .into_any_element(),
        Block::Rule => div().h(px(1.)).my_2().bg(theme::border_strong()).into_any_element(),
        Block::Table { header, rows, align } => {
            let cell = |inline: &Inline, ix: usize, head: bool| {
                div()
                    .flex_1()
                    .px_2()
                    .py_1()
                    .border_r_1()
                    .border_color(theme::border())
                    .flex()
                    .when(matches!(align.get(ix), Some(Alignment::Center)), |d| d.justify_center())
                    .when(matches!(align.get(ix), Some(Alignment::Right)), |d| d.justify_end())
                    .when(head, |d| d.font_weight(FontWeight::SEMIBOLD))
                    .child(styled(inline))
            };
            div()
                .border_1()
                .border_color(theme::border())
                .rounded_md()
                .overflow_hidden()
                .child(div().flex().bg(theme::panel_bg()).children(header.iter().enumerate().map(|(i, c)| cell(c, i, true))))
                .children(rows.iter().map(|r| {
                    div().flex().border_t_1().border_color(theme::border()).children(r.iter().enumerate().map(|(i, c)| cell(c, i, false)))
                }))
                .into_any_element()
        }
    }
}

/// The preview pane for already parsed blocks.
pub fn render(blocks: &[Block]) -> impl IntoElement + use<> {
    div()
        .id("markdown-preview")
        .size_full()
        .overflow_y_scroll()
        .bg(theme::editor_bg())
        .px_6()
        .py_4()
        .text_size(px(14.))
        .line_height(px(22.))
        .text_color(theme::text())
        .child(div().max_w(px(860.)).flex().flex_col().gap_2().children(blocks.iter().map(render_block)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_blocks() {
        let md = "# Title\n\nSome **bold** and `code`.\n\n- one\n- [x] done\n  1. nested\n\n```php\necho 1;\n```\n\n| a | b |\n|---|--:|\n| 1 | 2 |\n\n> quote\n\n---\n";
        let blocks = parse(md);
        let kinds: Vec<&str> = blocks
            .iter()
            .map(|b| match b {
                Block::Heading(..) => "h",
                Block::Paragraph(_) => "p",
                Block::Code { .. } => "code",
                Block::Item { .. } => "li",
                Block::Quote(_) => "quote",
                Block::Rule => "hr",
                Block::Table { .. } => "table",
            })
            .collect();
        assert_eq!(kinds, ["h", "p", "li", "li", "li", "code", "table", "quote", "hr"]);
        let Block::Paragraph(p) = &blocks[1] else { panic!() };
        assert_eq!(p.text, "Some bold and code.");
        assert_eq!(p.runs.len(), 2);
        let Block::Item { marker, depth, .. } = &blocks[3] else { panic!() };
        assert_eq!((marker.as_str(), *depth), ("☑", 1));
        let Block::Item { marker, depth, .. } = &blocks[4] else { panic!() };
        assert_eq!((marker.as_str(), *depth), ("1.", 2));
        let Block::Code { lang, text } = &blocks[5] else { panic!() };
        assert_eq!((lang.as_str(), text.as_str()), ("php", "echo 1;"));
    }
}
