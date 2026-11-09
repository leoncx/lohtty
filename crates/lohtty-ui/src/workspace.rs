use gpui::{
    AnyElement, Context, Entity, Focusable, IntoElement, Render, Window, div, prelude::*, px,
};
use lohtty_config::Config;
use lohtty_theme::Theme;

use crate::TerminalView;
use crate::colors::to_rgba;
use crate::terminal_view::spawn_session;

/// Root view of a window. Will own the tab bar and tabs (M3).
pub struct Workspace {
    config: Config,
    theme: Theme,
    /// The terminal, or the error that prevented the shell from starting.
    content: Result<Entity<TerminalView>, String>,
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
        Self {
            config,
            theme,
            content,
        }
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
            .bg(bg)
            .p(px(self.config.window.padding))
            .text_color(to_rgba(self.theme.foreground))
            .font_family(self.config.font.family.clone())
            .text_size(px(self.config.font.size))
            .child(content)
    }
}
