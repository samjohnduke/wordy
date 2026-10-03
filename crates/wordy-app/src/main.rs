//! Wordy: a private writing desk. Entry point.

mod app;
mod panels;
mod workspace;



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
            gpui_kit::init(cx);
            app::init(cx);
            app::open_main_window(cx);
        });
}
