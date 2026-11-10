//! The search bar overlaid on a terminal pane: a one-line text field with
//! case / regex toggles, a match counter and navigation buttons.

use std::ops::Range;

use gpui::{
    App, Bounds, Context, ElementInputHandler, EntityInputHandler, EventEmitter, FocusHandle,
    Focusable, IntoElement, KeyDownEvent, MouseButton, Pixels, Point, Render, SharedString,
    UTF16Selection, Window, canvas, div, prelude::*, px,
};
use lohtty_term::search::SearchQuery;
use lohtty_theme::Theme;

use crate::colors::{mix, to_rgba};

pub const KEY_CONTEXT: &str = "SearchBar";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchEvent {
    /// The query text or an option changed.
    Changed,
    /// Go to the match above (older output).
    Up,
    /// Go to the match below.
    Down,
    Close,
}

pub struct SearchBar {
    focus: FocusHandle,
    query: SearchQuery,
    /// Match counter ("3/12"), "No results", or a regex error.
    status: SharedString,
    theme: Theme,
    /// A dead key (e.g. `"` on US-International) waiting for the next key.
    dead_key: bool,
    /// An action held back until Windows resolves the pending dead key.
    deferred: Option<SearchEvent>,
}

impl EventEmitter<SearchEvent> for SearchBar {}

impl SearchBar {
    pub fn new(theme: Theme, initial_text: String, cx: &mut Context<Self>) -> Self {
        Self {
            focus: cx.focus_handle(),
            query: SearchQuery {
                text: initial_text,
                ..Default::default()
            },
            status: SharedString::default(),
            theme,
            dead_key: false,
            deferred: None,
        }
    }

    pub fn query(&self) -> &SearchQuery {
        &self.query
    }

    pub fn set_status(&mut self, status: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.status = status.into();
        cx.notify();
    }

    pub fn set_theme(&mut self, theme: Theme, cx: &mut Context<Self>) {
        self.theme = theme;
        cx.notify();
    }

    fn edit(&mut self, f: impl FnOnce(&mut String), cx: &mut Context<Self>) {
        f(&mut self.query.text);
        cx.emit(SearchEvent::Changed);
        cx.notify();
    }

    fn toggle(&mut self, f: impl FnOnce(&mut SearchQuery), cx: &mut Context<Self>) {
        f(&mut self.query);
        cx.emit(SearchEvent::Changed);
        cx.notify();
    }

    fn flush_deferred(&mut self, cx: &mut Context<Self>) {
        if let Some(event) = self.deferred.take() {
            cx.emit(event);
        }
    }

    fn on_key_down(&mut self, ev: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let mods = ev.keystroke.modifiers;
        let action = match ev.keystroke.key.as_str() {
            "escape" => Some(SearchEvent::Close),
            "enter" if mods.shift => Some(SearchEvent::Down),
            "enter" => Some(SearchEvent::Up),
            _ => None,
        };
        if let Some(action) = action
            && self.dead_key
        {
            // Same as `TextField`: let Windows commit the pending dead key as
            // text first (propagating lets gpui call TranslateMessage), then
            // act. A timer covers keys that produce no text.
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
            "escape" => cx.emit(SearchEvent::Close),
            "enter" if mods.shift => cx.emit(SearchEvent::Down),
            "enter" | "up" => cx.emit(SearchEvent::Up),
            "down" => cx.emit(SearchEvent::Down),
            "backspace" if mods.control => self.edit(String::clear, cx),
            "backspace" => self.edit(
                |t| {
                    t.pop();
                },
                cx,
            ),
            "v" if mods.control && !mods.alt => {
                // Paste the first line only: the field is single-line.
                let text = cx.read_from_clipboard().and_then(|i| i.text());
                if let Some(line) = text.as_deref().and_then(|t| t.lines().next()) {
                    let line = line.to_owned();
                    self.edit(|t| t.push_str(&line), cx);
                }
            }
            // Everything else either types text (delivered through the input
            // handler) or is ignored; never let it reach the terminal.
            _ => return,
        }
        cx.stop_propagation();
    }
}

impl Focusable for SearchBar {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for SearchBar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = &self.theme;
        let bg = to_rgba(mix(t.background, t.foreground, 0.08));
        let field = to_rgba(t.background);
        let fg = to_rgba(t.foreground);
        let muted = to_rgba(mix(t.foreground, t.background, 0.45));
        let accent = to_rgba(t.ansi[4]);
        let hover = to_rgba(mix(t.background, t.foreground, 0.18));
        let focused = self.focus.is_focused(window);

        let toggle = |id: &'static str, label: &'static str, on: bool| {
            div()
                .id(id)
                .px_1()
                .rounded_sm()
                .border_1()
                .border_color(if on { accent } else { bg })
                .text_color(if on { fg } else { muted })
                .hover(|s| s.bg(hover))
                .child(label)
        };
        let button = |id: &'static str, label: &'static str| {
            div()
                .id(id)
                .px_1()
                .rounded_sm()
                .text_color(muted)
                .hover(|s| s.bg(hover).text_color(fg))
                .child(label)
        };

        let entity = cx.entity();
        let focus = self.focus.clone();

        div()
            .track_focus(&self.focus)
            .key_context(KEY_CONTEXT)
            // Clicks on the bar must not reach the terminal underneath
            // (it would take focus or start a selection).
            .occlude()
            .on_key_down(cx.listener(Self::on_key_down))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, _| window.focus(&this.focus)),
            )
            .relative()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .px_2()
            .py_1()
            .rounded_md()
            .border_1()
            .border_color(if focused { accent } else { hover })
            .bg(bg)
            .text_color(fg)
            .text_sm()
            .shadow_md()
            .child(
                // Receives typed text while the bar is focused.
                canvas(
                    |_, _, _| {},
                    move |bounds, _, window, cx| {
                        window.handle_input(&focus, ElementInputHandler::new(bounds, entity), cx)
                    },
                )
                .absolute()
                .size_full(),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .min_w(px(180.0))
                    .max_w(px(320.0))
                    .px_1()
                    .rounded_sm()
                    .bg(field)
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .when(self.query.text.is_empty(), |el| {
                        el.text_color(muted).child("Search")
                    })
                    .child(self.query.text.clone())
                    .when(focused, |el| el.child(div().text_color(accent).child("▏"))),
            )
            .child(
                toggle("search-case", "Aa", self.query.case_sensitive).on_click(cx.listener(
                    |this, _, _, cx| this.toggle(|q| q.case_sensitive = !q.case_sensitive, cx),
                )),
            )
            .child(
                toggle("search-regex", ".*", self.query.regex).on_click(
                    cx.listener(|this, _, _, cx| this.toggle(|q| q.regex = !q.regex, cx)),
                ),
            )
            .child(
                div()
                    .min_w(px(64.0))
                    .text_color(muted)
                    .child(self.status.clone()),
            )
            .child(
                button("search-up", "↑")
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(SearchEvent::Up))),
            )
            .child(
                button("search-down", "↓")
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(SearchEvent::Down))),
            )
            .child(
                button("search-close", "×")
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(SearchEvent::Close))),
            )
    }
}

/// Typed text arrives here (WM_CHAR / IME), like in the terminal.
impl EntityInputHandler for SearchBar {
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
        let end = self.query.text.encode_utf16().count();
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
        // Compositions (dead keys, IME) are committed through
        // `replace_text_in_range`; nothing is shown while composing. A
        // one-character composition on Windows is a dead key.
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
