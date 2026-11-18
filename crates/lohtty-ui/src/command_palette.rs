//! The command palette (Ctrl+Shift+P): every action by name, with its
//! shortcut, filtered as you type.

use gpui::{
    Action, App, Context, Entity, EventEmitter, FocusHandle, Focusable, IntoElement, KeyDownEvent,
    MouseButton, Render, ScrollHandle, SharedString, Subscription, Window, div, prelude::*, px,
};
use lohtty_theme::Theme;

use crate::settings::{Palette, field_style};
use crate::text_field::{TextField, TextFieldEvent};

/// A command: its label, the action it runs and the shortcut shown.
pub struct Command {
    pub label: &'static str,
    pub action: Box<dyn Action>,
    pub shortcut: Option<SharedString>,
}

pub enum PaletteEvent {
    /// Run this action (on the active pane), then close.
    Run(Box<dyn Action>),
    Close,
}

pub struct CommandPalette {
    commands: Vec<Command>,
    /// Indexes into `commands` matching the query, in order.
    shown: Vec<usize>,
    /// Position in `shown` of the highlighted command.
    selected: usize,
    query: Entity<TextField>,
    scroll: ScrollHandle,
    theme: Theme,
    _subscription: Subscription,
}

impl EventEmitter<PaletteEvent> for CommandPalette {}

impl CommandPalette {
    pub fn new(
        commands: Vec<Command>,
        theme: Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let query = cx.new(|cx| TextField::new("", "Type a command", field_style(&theme), cx));
        let subscription = cx.subscribe_in(&query, window, Self::on_query_event);
        let shown = (0..commands.len()).collect();
        Self {
            commands,
            shown,
            selected: 0,
            query,
            scroll: ScrollHandle::new(),
            theme,
            _subscription: subscription,
        }
    }

    fn on_query_event(
        &mut self,
        _: &Entity<TextField>,
        event: &TextFieldEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            TextFieldEvent::Changed => {
                let query = self.query.read(cx).text().to_owned();
                self.shown = filter(&self.commands, &query);
                self.select(0, cx);
            }
            TextFieldEvent::Submit => self.run(self.selected, cx),
            TextFieldEvent::Cancel => cx.emit(PaletteEvent::Close),
        }
    }

    fn select(&mut self, index: usize, cx: &mut Context<Self>) {
        self.selected = index.min(self.shown.len().saturating_sub(1));
        self.scroll.scroll_to_item(self.selected);
        cx.notify();
    }

    fn run(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(&command) = self.shown.get(index) {
            cx.emit(PaletteEvent::Run(
                self.commands[command].action.boxed_clone(),
            ));
        }
    }

    fn on_key_down(&mut self, ev: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let last = self.shown.len().saturating_sub(1);
        let index = match ev.keystroke.key.as_str() {
            "up" => self.selected.checked_sub(1).unwrap_or(last),
            "down" if self.selected >= last => 0,
            "down" => self.selected + 1,
            "pageup" => self.selected.saturating_sub(8),
            "pagedown" => (self.selected + 8).min(last),
            _ => return,
        };
        self.select(index, cx);
        cx.stop_propagation();
    }
}

/// The commands whose label contains every word of `query` (any case), in
/// their original order.
pub fn filter(commands: &[Command], query: &str) -> Vec<usize> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    commands
        .iter()
        .enumerate()
        .filter(|(_, c)| {
            let label = c.label.to_lowercase();
            words.iter().all(|w| label.contains(w.as_str()))
        })
        .map(|(i, _)| i)
        .collect()
}

/// A shortcut as people write it: `Ctrl+Shift+T` (macOS keeps gpui's
/// symbols, `⇧⌘T`).
pub fn shortcut_text(binding: &gpui::KeyBinding) -> SharedString {
    let strokes = binding.keystrokes().iter().map(|k| {
        if cfg!(target_os = "macos") {
            return k.to_string();
        }
        let k = k.inner();
        let m = k.modifiers;
        let mut parts = Vec::new();
        for (on, name) in [
            (m.control, "Ctrl"),
            (m.alt, "Alt"),
            (m.shift, "Shift"),
            (m.platform, "Win"),
        ] {
            if on {
                parts.push(name.to_owned());
            }
        }
        let key = match k.key.as_str() {
            "up" => "Up".to_owned(),
            "down" => "Down".to_owned(),
            "left" => "Left".to_owned(),
            "right" => "Right".to_owned(),
            "enter" => "Enter".to_owned(),
            "tab" => "Tab".to_owned(),
            "escape" => "Esc".to_owned(),
            other if other.chars().count() == 1 => other.to_uppercase(),
            other => {
                let mut c = other.chars();
                c.next()
                    .map(|f| f.to_uppercase().chain(c).collect())
                    .unwrap_or_default()
            }
        };
        parts.push(key);
        parts.join("+")
    });
    strokes.collect::<Vec<_>>().join(" ").into()
}

impl Focusable for CommandPalette {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.query.focus_handle(cx)
    }
}

impl Render for CommandPalette {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = Palette::new(&self.theme);
        let rows = self.shown.iter().enumerate().map(|(i, &command)| {
            let c = &self.commands[command];
            let selected = i == self.selected;
            div()
                .id(("command", i))
                .flex()
                .flex_row()
                .justify_between()
                .gap_4()
                .px_3()
                .py_1()
                .rounded_md()
                .when(selected, |el| el.bg(p.accent).text_color(p.on_accent))
                .when(!selected, |el| el.hover(|s| s.bg(p.hover)))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| this.run(i, cx)),
                )
                .child(c.label)
                .children(c.shortcut.clone().map(|s| {
                    div()
                        .text_color(if selected { p.on_accent } else { p.muted })
                        .child(s)
                }))
        });
        div()
            .key_context("CommandPalette")
            .on_key_down(cx.listener(Self::on_key_down))
            // Clicks inside don't reach the backdrop, which closes it.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .occlude()
            .w(px(520.0))
            .flex()
            .flex_col()
            .gap_1()
            .p_2()
            .rounded_lg()
            .border_1()
            .border_color(p.border)
            .bg(p.panel)
            .text_color(p.fg)
            .shadow_lg()
            .child(self.query.clone())
            .child(
                div()
                    .id("commands")
                    .track_scroll(&self.scroll)
                    .overflow_y_scroll()
                    .max_h(px(360.0))
                    .flex()
                    .flex_col()
                    .children(rows)
                    .when(self.shown.is_empty(), |el| {
                        el.child(div().px_3().py_1().text_color(p.muted).child("No command"))
                    }),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    gpui::actions!(test, [A]);

    fn commands(labels: &[&'static str]) -> Vec<Command> {
        labels
            .iter()
            .map(|&label| Command {
                label,
                action: Box::new(A),
                shortcut: None,
            })
            .collect()
    }

    #[test]
    fn every_word_must_match_in_any_case() {
        let c = commands(&["New tab", "Close tab", "Split right", "Next prompt"]);
        assert_eq!(filter(&c, ""), vec![0, 1, 2, 3]);
        assert_eq!(filter(&c, "TAB"), vec![0, 1]);
        assert_eq!(filter(&c, "ne ta"), vec![0]);
        assert_eq!(filter(&c, "  pro  next "), vec![3]);
        assert!(filter(&c, "zoom").is_empty());
    }
}
