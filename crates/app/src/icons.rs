//! Built-in SVG icon set (24×24, 1.75 stroke). GPUI renders an SVG as a mask, so each icon
//! takes the colour of the element's text colour.

use std::borrow::Cow;

use gpui::{AssetSource, Hsla, Result, SharedString, Styled, Svg, px, svg};

macro_rules! icons {
    ($($name:ident = $body:expr;)*) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        #[allow(non_camel_case_types, dead_code)]
        pub enum Icon { $($name),* }

        impl Icon {
            pub fn path(self) -> &'static str {
                match self { $(Icon::$name => concat!("icons/", stringify!($name), ".svg")),* }
            }
        }

        fn body(path: &str) -> Option<&'static str> {
            match path {
                $(concat!("icons/", stringify!($name), ".svg") => Some($body),)*
                _ => None,
            }
        }

        fn all() -> Vec<SharedString> {
            vec![$(SharedString::new_static(concat!("icons/", stringify!($name), ".svg"))),*]
        }
    };
}

icons! {
    folder = r#"<path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/>"#;
    folder_open = r#"<path d="M3 17V7a2 2 0 0 1 2-2h4l2 2h6a2 2 0 0 1 2 2v1"/><path d="M3 17l2.4-6.2A1.5 1.5 0 0 1 6.8 10H21l-2.6 7.1a1.5 1.5 0 0 1-1.4 .9H4a1 1 0 0 1-1-1z"/>"#;
    structure = r#"<path d="M4 5h9M4 12h6M4 19h6"/><path d="M14 12h6M14 19h6M17 5h3"/><circle cx="15" cy="5" r=".6"/>"#;
    modules = r#"<rect x="3" y="3" width="7.5" height="7.5" rx="1.5"/><rect x="13.5" y="3" width="7.5" height="7.5" rx="1.5"/><rect x="3" y="13.5" width="7.5" height="7.5" rx="1.5"/><rect x="13.5" y="13.5" width="7.5" height="7.5" rx="1.5"/>"#;
    database = r#"<ellipse cx="12" cy="5.5" rx="8" ry="2.8"/><path d="M4 5.5v13c0 1.5 3.6 2.8 8 2.8s8-1.3 8-2.8v-13"/><path d="M4 12c0 1.5 3.6 2.8 8 2.8s8-1.3 8-2.8"/>"#;
    laravel = r#"<path d="M4 6l4-2 4 2v9l4 2 4-2"/><path d="M8 4v13l4 2 4-2"/><path d="M12 6l4-2 4 2v5l-4 2"/><path d="M4 6v9l4 2"/>"#;
    commit = r#"<circle cx="12" cy="12" r="3.5"/><path d="M3 12h5.5M15.5 12H21"/>"#;
    check = r#"<path d="M5 12.5l4.5 4.5L19 7.5"/>"#;
    sparkles = r#"<path d="M12 3l1.8 5.2L19 10l-5.2 1.8L12 17l-1.8-5.2L5 10l5.2-1.8z"/><path d="M19 15l.8 2.2L22 18l-2.2.8L19 21l-.8-2.2L16 18l2.2-.8z"/>"#;
    terminal = r#"<rect x="2.5" y="4" width="19" height="16" rx="2"/><path d="M7 9.5l3 2.5-3 2.5M12.5 15H17"/>"#;
    branch = r#"<circle cx="6" cy="5.5" r="2.2"/><circle cx="6" cy="18.5" r="2.2"/><circle cx="18" cy="7.5" r="2.2"/><path d="M6 7.7v8.6"/><path d="M18 9.7c0 4-6 3.3-11 7"/>"#;
    search = r#"<circle cx="11" cy="11" r="6.5"/><path d="M20 20l-4.4-4.4"/>"#;
    settings = r#"<circle cx="12" cy="12" r="3"/><path d="M12 2.8v2.4M12 18.8v2.4M2.8 12h2.4M18.8 12h2.4M5.5 5.5l1.7 1.7M16.8 16.8l1.7 1.7M5.5 18.5l1.7-1.7M16.8 7.2l1.7-1.7"/>"#;
    play = r#"<path d="M7 4.5v15l12.5-7.5z"/>"#;
    stop = r#"<rect x="6" y="6" width="12" height="12" rx="1.5"/>"#;
    container = r#"<path d="M3.5 7.5L12 3.5l8.5 4v9L12 20.5l-8.5-4z"/><path d="M3.5 7.5L12 11.5l8.5-4M12 11.5v9"/>"#;
    bug = r#"<rect x="7" y="8" width="10" height="12" rx="5"/><path d="M12 8v12M4 13h3M17 13h3M5 8l2.5 2M19 8l-2.5 2M5 19l2.5-2M19 19l-2.5-2M9 5l1.5 3M15 5l-1.5 3"/>"#;
    bookmark = r#"<path d="M6 3.5h12v17l-6-4-6 4z"/>"#;
    todo = r#"<rect x="3.5" y="3.5" width="17" height="17" rx="2.5"/><path d="M8 12l3 3 5-6"/>"#;
    history = r#"<path d="M3.5 12a8.5 8.5 0 1 0 2.6-6.1"/><path d="M3.5 4v4.5H8"/><path d="M12 7.5V12l3 2"/>"#;
    file = r#"<path d="M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8z"/><path d="M14 3v5h5"/>"#;
    file_code = r#"<path d="M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8z"/><path d="M14 3v5h5"/><path d="M10 12l-2 2.2 2 2.3M14 12l2 2.2-2 2.3"/>"#;
    file_text = r#"<path d="M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8z"/><path d="M14 3v5h5"/><path d="M8.5 13h7M8.5 16.5h5"/>"#;
    braces = r#"<path d="M8 4c-2 0-2.5 1-2.5 3v2c0 1.5-.8 2.5-2 3 1.2.5 2 1.5 2 3v2c0 2 .5 3 2.5 3M16 4c2 0 2.5 1 2.5 3v2c0 1.5.8 2.5 2 3-1.2.5-2 1.5-2 3v2c0 2-.5 3-2.5 3"/>"#;
    image = r#"<rect x="3.5" y="4.5" width="17" height="15" rx="2"/><circle cx="9" cy="10" r="1.8"/><path d="M20.5 16l-5-5-9 8.5"/>"#;
    plus = r#"<path d="M12 5v14M5 12h14"/>"#;
    minus = r#"<path d="M5 12h14"/>"#;
    close = r#"<path d="M6 6l12 12M18 6L6 18"/>"#;
    refresh = r#"<path d="M20 11a8 8 0 0 0-14.5-4.5L4 8"/><path d="M4 4v4h4"/><path d="M4 13a8 8 0 0 0 14.5 4.5L20 16"/><path d="M20 20v-4h-4"/>"#;
    chevron_right = r#"<path d="M9.5 6l6 6-6 6"/>"#;
    chevron_down = r#"<path d="M6 9.5l6 6 6-6"/>"#;
    diff = r#"<path d="M12 3v18"/><path d="M5 8h4M7 6v4M15 16h4"/><rect x="3" y="3" width="18" height="18" rx="2.5"/>"#;
    globe = r#"<circle cx="12" cy="12" r="8.5"/><path d="M3.5 12h17M12 3.5c2.5 2.6 3.5 5.4 3.5 8.5s-1 5.9-3.5 8.5c-2.5-2.6-3.5-5.4-3.5-8.5s1-5.9 3.5-8.5z"/>"#;
    split = r#"<rect x="3" y="4" width="18" height="16" rx="2"/><path d="M12 4v16"/>"#;
    upload = r#"<path d="M12 15V4M7.5 8.5L12 4l4.5 4.5"/><path d="M4 15v3a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2v-3"/>"#;
    download = r#"<path d="M12 4v11M7.5 10.5L12 15l4.5-4.5"/><path d="M4 15v3a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2v-3"/>"#;
    warning = r#"<path d="M12 4l9 16H3z"/><path d="M12 10v4.5M12 17.2v.3"/>"#;
}

fn document(body: &str) -> String {
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#000" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round">{body}</svg>"##
    )
}

/// Serves `icons/*.svg` from the table above.
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(body(path).map(|b| Cow::Owned(document(b).into_bytes())))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(all().into_iter().filter(|p| p.starts_with(path)).collect())
    }
}

/// An icon element of `size` pixels in `color`.
pub fn icon(icon: Icon, size: f32, color: impl Into<Hsla>) -> Svg {
    svg().path(icon.path()).size(px(size)).flex_none().text_color(color.into())
}

/// The icon for a file in the project tree, by extension.
pub fn for_file(name: &str) -> (Icon, Hsla) {
    use gpui::rgb;
    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    let lower = name.to_ascii_lowercase();
    let (icon, color) = if lower.ends_with(".blade.php") {
        (Icon::file_code, 0xf97316)
    } else {
        match ext.as_str() {
            "php" => (Icon::file_code, 0x8b8cf6),
            "js" | "mjs" | "cjs" | "jsx" => (Icon::file_code, 0xe5c07b),
            "ts" | "tsx" => (Icon::file_code, 0x4f9cf0),
            "vue" => (Icon::file_code, 0x42b883),
            "css" | "scss" | "sass" | "less" => (Icon::file_code, 0x56b6c2),
            "html" | "htm" => (Icon::file_code, 0xe06c75),
            "json" | "lock" => (Icon::braces, 0xe5c07b),
            "yml" | "yaml" | "toml" | "xml" | "neon" => (Icon::braces, 0xa0a6b4),
            "md" | "txt" | "log" => (Icon::file_text, 0xa0a6b4),
            "sql" | "sqlite" => (Icon::database, 0xc0a060),
            "png" | "jpg" | "jpeg" | "gif" | "svg" | "webp" | "ico" => (Icon::image, 0x98c379),
            "rs" => (Icon::file_code, 0xdea584),
            "http" | "rest" => (Icon::globe, 0x22d3ee),
            "sh" | "zsh" | "bash" => (Icon::terminal, 0x98c379),
            _ if lower.starts_with(".env") => (Icon::settings, 0xa0a6b4),
            _ => (Icon::file, 0x8b90a0),
        }
    };
    (icon, rgb(color).into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_loads_as_svg() {
        for path in all() {
            let bytes = Assets.load(&path).unwrap().unwrap();
            assert!(std::str::from_utf8(&bytes).unwrap().starts_with("<svg"));
        }
        assert_eq!(for_file("welcome.blade.php").0, Icon::file_code);
    }
}
