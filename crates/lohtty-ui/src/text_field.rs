//! A minimal single-line text field: typing (through the platform input
//! handler, so dead keys and IME work), Backspace, Ctrl+Backspace (clear),
//! Ctrl+V (paste first line), Enter (submit) and Esc (cancel).

use std::ops::Range;

use gpui::{
    App, Bounds, Context, ElementInputHandler, EntityInputHandler, EventEmitter, FocusHandle,
    Focusable, IntoElement, KeyDownEvent, MouseButton, Pixels, Point, Render, Rgba, SharedString,
    UTF16Selection, Window, canvas, div, prelude::*, px,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextFieldEvent {
    Changed,
    Submit,
    Cancel,
}

/// Colors, set by the owner from its theme.
#[derive(Debug, Clone, Copy)]
pub struct TextFieldStyle {
    pub bg: Rgba,
    pub fg: Rgba,
    pub muted: Rgba,
    pub border: Rgba,
    pub accent: Rgba,
}

pub struct TextField {
    focus: FocusHandle,
    text: String,
    placeholder: SharedString,
    style: TextFieldStyle,
    /// A dead key (e.g. `"` on US-International) waiting for the next key.
    dead_key: bool,
    /// An action held back until Windows resolves the pending dead key.
    deferred: Option<TextFieldEvent>,
}

impl EventEmitter<TextFieldEvent> for TextField {}

impl TextField {
    pub fn new(
        text: impl Into<String>,
        placeholder: impl Into<SharedString>,
        style: TextFieldStyle,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            focus: cx.focus_handle(),
            text: text.into(),
            placeholder: placeholder.into(),
            style,
            dead_key: false,
            deferred: None,
        }
    }

    fn flush_deferred(&mut self, cx: &mut Context<Self>) {
        if let Some(event) = self.deferred.take() {
            cx.emit(event);
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    fn edit(&mut self, f: impl FnOnce(&mut String), cx: &mut Context<Self>) {
        f(&mut self.text);
        cx.emit(TextFieldEvent::Changed);
        cx.notify();
    }

    fn on_key_down(&mut self, ev: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let mods = ev.keystroke.modifiers;
        let action = match ev.keystroke.key.as_str() {
            "escape" => Some(TextFieldEvent::Cancel),
            "enter" => Some(TextFieldEvent::Submit),
            _ => None,
        };
        if let Some(action) = action
            && self.dead_key
        {
            // Let Windows resolve the dead key first (propagating lets gpui call
            // TranslateMessage, which commits the accent as text and clears the
            // keyboard state), then act. A timer covers keys that produce no text.
            self.dead_key = false;
            self.deferred = Some(action);
            cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(50))
                    .await;
                let _ = this.update(cx, |this, cx| this.flush_deferred(cx));
            })
            .detach();
            return;
        }
        match ev.keystroke.key.as_str() {
            "escape" => cx.emit(TextFieldEvent::Cancel),
            "enter" => cx.emit(TextFieldEvent::Submit),
            "backspace" if mods.control => self.edit(String::clear, cx),
            "backspace" => self.edit(
                |t| {
                    t.pop();
                },
                cx,
            ),
            "v" if mods.control && !mods.alt => {
                let text = cx.read_from_clipboard().and_then(|i| i.text());
                if let Some(line) = text.as_deref().and_then(|t| t.lines().next()) {
                    let line = line.to_owned();
                    self.edit(|t| t.push_str(&line), cx);
                }
            }
            // Other keys type text through the input handler, or are ignored.
            _ => return,
        }
        cx.stop_propagation();
    }
}

impl Focusable for TextField {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for TextField {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let s = self.style;
        let focused = self.focus.is_focused(window);
        let entity = cx.entity();
        let focus = self.focus.clone();

        div()
            .track_focus(&self.focus)
            .key_context("TextField")
            .on_key_down(cx.listener(Self::on_key_down))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    window.focus(&this.focus);
                    // Containers that focus themselves on click must not take
                    // the focus back.
                    cx.stop_propagation();
                }),
            )
            .relative()
            .flex()
            .flex_row()
            .items_center()
            .w_full()
            .h(px(26.0))
            .px_2()
            .rounded_sm()
            .border_1()
            .border_color(if focused { s.accent } else { s.border })
            .bg(s.bg)
            .text_color(s.fg)
            .overflow_hidden()
            .whitespace_nowrap()
            .child(
                canvas(
                    |_, _, _| {},
                    move |bounds, _, window, cx| {
                        window.handle_input(&focus, ElementInputHandler::new(bounds, entity), cx)
                    },
                )
                .absolute()
                .size_full(),
            )
            .when(self.text.is_empty() && !focused, |el| {
                el.text_color(s.muted).child(self.placeholder.clone())
            })
            .child(self.text.clone())
            .when(focused, |el| {
                el.child(div().text_color(s.accent).child("▏"))
            })
    }
}

impl EntityInputHandler for TextField {
    fn text_for_range(
        &mut self,
        _range: Range<usize>,
        _adjusted_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        None
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let end = self.text.encode_utf16().count();
        Some(UTF16Selection {
            range: end..end,
            reversed: false,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        None
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {}

    fn replace_text_in_range(
        &mut self,
        _range: Option<Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.dead_key = false;
        let text: String = text.chars().filter(|c| !c.is_control()).collect();
        if !text.is_empty() {
            self.edit(|t| t.push_str(&text), cx);
        }
        self.flush_deferred(cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _range: Option<Range<usize>>,
        new_text: &str,
        _new_selected_range: Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        // A one-character composition on Windows is a dead key. It's not
        // reported as marked text, so gpui keeps delivering key events.
        self.dead_key = cfg!(windows) && new_text.chars().count() == 1;
    }

    fn bounds_for_range(
        &mut self,
        _range_utf16: Range<usize>,
        element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        Some(element_bounds)
    }

    fn character_index_for_point(
        &mut self,
        _point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }
}
