// No console window in release builds on Windows.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use gpui::{
    App, Application, Bounds, TitlebarOptions, WindowBounds, WindowOptions, prelude::*, px, size,
};
use lohtty_config::Config;
use lohtty_theme::Theme;
use lohtty_ui::{Workspace, load_theme, window_background};
use tracing_subscriber::EnvFilter;

const DEFAULT_LOG_FILTER: &str =
    "warn,lohtty=info,lohtty_ui=info,lohtty_term=info,lohtty_config=info,lohtty_theme=info";

fn main() {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            // By default lohtty's own messages, and only warnings from the
            // libraries (gpui's renderer logs every pipeline at info).
            EnvFilter::try_from_env("LOHTTY_LOG")
                .unwrap_or_else(|_| EnvFilter::new(DEFAULT_LOG_FILTER)),
        )
        .init();

    let config = Config::load().unwrap_or_else(|e| {
        tracing::error!("{e}; falling back to defaults");
        Config::default()
    });
    let theme = load_theme(&config).unwrap_or_else(|e| {
        tracing::error!("{e}; falling back to the default theme");
        Theme::default()
    });

    Application::new().run(move |cx: &mut App| {
        lohtty_ui::init(cx);

        let win = &config.window;
        let bounds = Bounds::centered(None, size(px(win.width), px(win.height)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions {
                title: Some("lohtty".into()),
                // The tab bar doubles as the title bar (see lohtty_ui::CUSTOM_TITLEBAR).
                appears_transparent: lohtty_ui::CUSTOM_TITLEBAR,
                ..Default::default()
            }),
            window_background: window_background(win),
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
