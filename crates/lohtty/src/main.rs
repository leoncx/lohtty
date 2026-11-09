// No console window in release builds on Windows.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use gpui::{
    App, Application, Bounds, TitlebarOptions, WindowBackgroundAppearance, WindowBounds,
    WindowOptions, prelude::*, px, size,
};
use lohtty_config::Config;
use lohtty_theme::Theme;
use lohtty_ui::Workspace;
use tracing_subscriber::EnvFilter;

fn main() {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            EnvFilter::try_from_env("LOHTTY_LOG").unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let config = Config::load().unwrap_or_else(|e| {
        tracing::error!("{e}; falling back to defaults");
        Config::default()
    });
    let theme = Theme::default();

    Application::new().run(move |cx: &mut App| {
        let win = &config.window;
        let bounds = Bounds::centered(None, size(px(win.width), px(win.height)), cx);
        let window_background = match (win.opacity < 1.0, win.blur) {
            (false, _) => WindowBackgroundAppearance::Opaque,
            (true, false) => WindowBackgroundAppearance::Transparent,
            (true, true) => WindowBackgroundAppearance::Blurred,
        };

        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions {
                title: Some("lohtty".into()),
                ..Default::default()
            }),
            window_background,
            app_id: Some("lohtty".into()),
            ..Default::default()
        };

        let config = config.clone();
        cx.open_window(options, |window, cx| {
            cx.new(|cx| Workspace::new(config, theme, window, cx))
        })
        .expect("failed to open window");
        cx.on_window_closed(|cx| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        cx.activate(true);
    });
}
