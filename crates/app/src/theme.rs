//! Darcula-like color palette.

use gpui::{Hsla, rgb};
use php_syntax::Highlight;

pub const EDITOR_FONT: &str = "Menlo";
pub const UI_FONT: &str = ".SystemUIFont";

pub fn editor_bg() -> Hsla {
    rgb(0x2b2b2b).into()
}
pub fn panel_bg() -> Hsla {
    rgb(0x3c3f41).into()
}
pub fn border() -> Hsla {
    rgb(0x323232).into()
}
pub fn gutter_bg() -> Hsla {
    rgb(0x313335).into()
}
pub fn line_number() -> Hsla {
    rgb(0x606366).into()
}
pub fn current_line() -> Hsla {
    rgb(0x323232).into()
}
pub fn selection() -> Hsla {
    rgb(0x214283).into()
}
pub fn caret() -> Hsla {
    rgb(0xbbbbbb).into()
}
pub fn text() -> Hsla {
    rgb(0xa9b7c6).into()
}
pub fn ui_text() -> Hsla {
    rgb(0xbbbbbb).into()
}
pub fn muted() -> Hsla {
    rgb(0x808080).into()
}
pub fn hover() -> Hsla {
    rgb(0x4b4d4f).into()
}
pub fn active_row() -> Hsla {
    rgb(0x0d293e).into()
}
pub fn tab_active() -> Hsla {
    rgb(0x4e5254).into()
}
pub fn border_strong() -> Hsla {
    rgb(0x5e6060).into()
}
pub fn success() -> Hsla {
    rgb(0x499c54).into()
}
pub fn overlay() -> Hsla {
    gpui::hsla(0., 0., 0., 0.45)
}
pub fn error() -> Hsla {
    rgb(0xbc3f3c).into()
}
pub fn link() -> Hsla {
    rgb(0x589df6).into()
}
pub fn accent() -> Hsla {
    rgb(0x4a88c7).into()
}

pub fn syntax(highlight: Highlight) -> Hsla {
    let color = match highlight {
        Highlight::Keyword => 0xcc7832,
        Highlight::String => 0x6a8759,
        Highlight::Comment => 0x808080,
        Highlight::Number => 0x6897bb,
        Highlight::Function | Highlight::Method => 0xffc66d,
        Highlight::Type | Highlight::Namespace => 0xa9b7c6,
        Highlight::Variable | Highlight::Property | Highlight::Constant => 0x9876aa,
        Highlight::Operator => 0xa9b7c6,
        Highlight::Tag => 0xe8bf6a,
    };
    rgb(color).into()
}
