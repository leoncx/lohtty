//! A small themed text tooltip.

use gpui::{
    AnyView, App, AppContext, Context, IntoElement, Render, Rgba, SharedString, Window, div,
    prelude::*,
};
use lohtty_theme::Theme;

use crate::colors::{mix, to_rgba};

pub struct Tooltip {
    text: SharedString,
    bg: Rgba,
    fg: Rgba,
    border: Rgba,
}

impl Tooltip {
    /// A tooltip builder for [`gpui::StatefulInteractiveElement::tooltip`].
    pub fn text(
        text: impl Into<SharedString>,
        theme: &Theme,
    ) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
        let text = text.into();
        let bg = to_rgba(mix(theme.background, theme.foreground, 0.1));
        let fg = to_rgba(theme.foreground);
        let border = to_rgba(mix(theme.background, theme.foreground, 0.25));
        move |_, cx| {
            let text = text.clone();
            cx.new(|_| Tooltip {
                text,
                bg,
                fg,
                border,
            })
            .into()
        }
    }
}

impl Render for Tooltip {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_2()
            .py_1()
            .rounded_md()
            .border_1()
            .border_color(self.border)
            .bg(self.bg)
            .text_color(self.fg)
            .text_xs()
            .shadow_md()
            .child(self.text.clone())
    }
}
