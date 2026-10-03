//! Wordy: a private writing desk. Entry point.

mod app;
mod panels;
mod workspace;

use std::borrow::Cow;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("wordy=info")),
        )
        .init();

    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(|cx| {
            load_fonts(cx);
            gpui_kit::init(cx);
            wordy_editor::init(cx);
            app::init(cx);
            app::open_main_window(cx);
        });
}

/// The one bundled serif, used by the editor and (later) the PDF export.
fn load_fonts(cx: &mut gpui_kit::App) {
    let fonts: Vec<Cow<'static, [u8]>> = vec![
        Cow::Borrowed(include_bytes!("../assets/fonts/LibertinusSerif-Regular.otf")),
        Cow::Borrowed(include_bytes!("../assets/fonts/LibertinusSerif-Italic.otf")),
        Cow::Borrowed(include_bytes!("../assets/fonts/LibertinusSerif-Bold.otf")),
        Cow::Borrowed(include_bytes!("../assets/fonts/LibertinusSerif-BoldItalic.otf")),
    ];
    if let Err(e) = cx.text_system().add_fonts(fonts) {
        tracing::error!("could not load bundled fonts: {e:#}");
    }
}
