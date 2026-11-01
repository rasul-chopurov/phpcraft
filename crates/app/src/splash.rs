//! Start-up splash: a borderless window with the artwork, name and version, shown for a
//! moment before the first project or welcome window.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use gpui::{
    App, Bounds, Context, FontWeight, Image, ImageFormat, ObjectFit, WindowBackgroundAppearance, Window, WindowBounds, WindowKind, WindowOptions, div, img, prelude::*, px, rgb,
    size,
};

const SHOW_FOR: Duration = Duration::from_millis(1600);

/// The artwork, rendered from `assets/splash/splash.svg` (re-render the PNG after editing it).
fn artwork() -> Arc<Image> {
    static IMAGE: OnceLock<Arc<Image>> = OnceLock::new();
    IMAGE.get_or_init(|| Arc::new(Image::from_bytes(ImageFormat::Png, include_bytes!("../../../assets/splash/splash.png").to_vec()))).clone()
}

struct Splash;

impl Splash {
    /// Logo square, then "Path.az IDEs".
    fn brand(&self) -> impl IntoElement {
        let white = rgb(0xffffff);
        div()
            .flex()
            .items_center()
            .gap_2()
            .child(
                div()
                    .size(px(24.))
                    .rounded(px(5.))
                    .bg(gpui::linear_gradient(
                        135.,
                        gpui::linear_color_stop(rgb(0x8b6cf6), 0.),
                        gpui::linear_color_stop(rgb(0x2dd4bf), 1.),
                    ))
                    .flex()
                    .items_end()
                    .p(px(4.))
                    .child(div().w(px(9.)).h(px(2.)).bg(white)),
            )
            .child(div().text_size(px(15.)).font_weight(FontWeight::BOLD).child("Path.az"))
            .child(div().text_size(px(15.)).child("IDEs"))
    }
}

impl Render for Splash {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let white = rgb(0xffffff);
        div()
            .size_full()
            .relative()
            .overflow_hidden()
            .bg(rgb(0x0a0820))
            .text_color(white)
            .child(img(artwork()).absolute().top_0().left_0().size_full().object_fit(ObjectFit::Cover))
            .child(
                div()
                    .absolute()
                    .left(px(40.))
                    .top(px(44.))
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(div().text_size(px(56.)).font_weight(FontWeight::BOLD).child("PhpCraft"))
                    .child(div().text_size(px(17.)).text_color(rgb(0xe6e0ff)).child("2026.0.1")),
            )
            .child(
                div()
                    .absolute()
                    .left(px(40.))
                    .bottom(px(22.))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(self.brand())
                    .child(div().pl(px(32.)).text_size(px(12.)).text_color(rgb(0x22d3ee)).child("#opensource")),
            )
            .child(
                div()
                    .absolute()
                    .right(px(26.))
                    .bottom(px(32.))
                    .text_size(px(12.))
                    .text_color(rgb(0xd9d2f5))
                    .child("made with ❤️ Azerbaijan · 11.2026"),
            )
    }
}

/// Shows the splash, then runs `then` and closes it.
pub fn show(cx: &mut App, then: impl FnOnce(&mut App) + 'static) {
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, size(px(640.), px(380.)), cx))),
        titlebar: None,
        kind: WindowKind::PopUp,
        is_movable: false,
        is_resizable: false,
        focus: false,
        window_background: WindowBackgroundAppearance::Opaque,
        ..Default::default()
    };
    let Ok(handle) = cx.open_window(options, |_, cx| cx.new(|_| Splash)) else {
        then(cx);
        return;
    };
    cx.spawn(async move |cx| {
        cx.background_executor().timer(SHOW_FOR).await;
        let _ = cx.update(|cx| {
            then(cx);
            let _ = handle.update(cx, |_, window, _| window.remove_window());
        });
    })
    .detach();
}
