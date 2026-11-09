use std::time::Duration;

use gpui::{
    AnyElement, Context, Entity, Focusable, IntoElement, Render, Task, Window, div, prelude::*, px,
};
use lohtty_config::{Config, ConfigWatcher};
use lohtty_theme::Theme;

use crate::colors::to_rgba;
use crate::terminal_view::spawn_session;
use crate::{TerminalView, load_theme, window_background};

/// Root view of a window. Owns the configuration and reloads it live;
/// will own the tab bar and tabs (M3).
pub struct Workspace {
    config: Config,
    theme: Theme,
    /// The terminal, or the error that prevented the shell from starting.
    content: Result<Entity<TerminalView>, String>,
    /// Last config/theme error, shown until the next successful reload.
    config_error: Option<String>,
    _watcher: Option<ConfigWatcher>,
    _reload: Task<()>,
}

impl Workspace {
    pub fn new(config: Config, theme: Theme, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let content = match spawn_session(&config) {
            Ok((session, events)) => {
                let view = cx.new(|cx| {
                    TerminalView::new(session, events, &config, theme.clone(), window, cx)
                });
                window.focus(&view.focus_handle(cx));
                Ok(view)
            }
            Err(e) => {
                tracing::error!("failed to start shell: {e:#}");
                Err(format!("{e:#}"))
            }
        };

        let (tx, rx) = async_channel::unbounded::<()>();
        let watcher = ConfigWatcher::spawn(move || {
            let _ = tx.try_send(());
        })
        .inspect_err(|e| tracing::warn!("config live reload disabled: {e}"))
        .ok();
        let reload = cx.spawn_in(window, async move |this, cx| {
            while rx.recv().await.is_ok() {
                // Editors emit several events per save: wait, then coalesce.
                cx.background_executor()
                    .timer(Duration::from_millis(100))
                    .await;
                while rx.try_recv().is_ok() {}
                if this
                    .update_in(cx, |this, window, cx| this.reload(window, cx))
                    .is_err()
                {
                    break;
                }
            }
        });

        Self {
            config,
            theme,
            content,
            config_error: None,
            _watcher: watcher,
            _reload: reload,
        }
    }

    /// Reloads config and theme from disk. On error, keeps the current ones.
    fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let result = Config::load()
            .map_err(|e| e.to_string())
            .and_then(|config| {
                let theme = load_theme(&config).map_err(|e| e.to_string())?;
                Ok((config, theme))
            });
        let (config, theme) = match result {
            Ok(loaded) => loaded,
            Err(e) => {
                tracing::error!("config not reloaded: {e}");
                self.config_error = Some(e);
                cx.notify();
                return;
            }
        };

        tracing::info!(theme = %theme.name, "config reloaded");
        window.set_background_appearance(window_background(&config.window));
        if let Ok(view) = &self.content {
            view.update(cx, |view, cx| view.apply_config(&config, theme.clone(), cx));
        }
        self.config = config;
        self.theme = theme;
        self.config_error = None;
        cx.notify();
    }
}

impl Render for Workspace {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let mut bg = to_rgba(self.theme.background);
        bg.a = self.config.window.opacity.clamp(0.0, 1.0);

        let content: AnyElement = match &self.content {
            Ok(view) => view.clone().into_any_element(),
            Err(e) => format!("Failed to start the shell: {e}").into_any_element(),
        };

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(bg)
            .text_color(to_rgba(self.theme.foreground))
            .font_family(self.config.font.family.clone())
            .text_size(px(self.config.font.size))
            .children(self.config_error.as_ref().map(|e| {
                div()
                    .px_2()
                    .py_1()
                    .bg(to_rgba(self.theme.ansi[1]))
                    .text_color(to_rgba(self.theme.background))
                    .child(format!("Config error (previous settings kept): {e}"))
            }))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .p(px(self.config.window.padding))
                    .child(content),
            )
    }
}
