//! Keyboard, text (IME / dead keys), mouse, scroll and clipboard handling.

use std::ops::Range;
use std::time::Duration;

use alacritty_terminal::index::{Column, Line, Point as GridPoint};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::TermMode;
use gpui::{
    Bounds, ClipboardItem, Context, EntityInputHandler, KeyDownEvent, Modifiers, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, ScrollDelta, ScrollWheelEvent,
    UTF16Selection, Window,
};
use lohtty_term::input::{KeyInput, encode_key, encode_paste, encode_win32, is_text_key};
use lohtty_term::mouse::{self, MouseAction, MouseReport, encode_mouse, wants_mouse};

use super::{Copy, Paste, TerminalView};

fn report_button(button: MouseButton) -> Option<mouse::MouseButton> {
    match button {
        MouseButton::Left => Some(mouse::MouseButton::Left),
        MouseButton::Middle => Some(mouse::MouseButton::Middle),
        MouseButton::Right => Some(mouse::MouseButton::Right),
        _ => None,
    }
}

impl TerminalView {
    // --- Keyboard -----------------------------------------------------------

    pub(super) fn on_key_down(
        &mut self,
        ev: &KeyDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ks = &ev.keystroke;
        tracing::trace!(key = %ks.key, key_char = ?ks.key_char, mods = ?ks.modifiers, "key down");
        if ks.modifiers.platform {
            return;
        }
        let input = KeyInput {
            key: &ks.key,
            key_char: ks.key_char.as_deref(),
            ctrl: ks.modifiers.control,
            alt: ks.modifiers.alt,
            shift: ks.modifiers.shift,
        };
        // Plain text is delivered through the input handler instead.
        if is_text_key(&input) {
            return;
        }
        let mode = *self.session.term().lock().mode();
        // A program that asked for the kitty keyboard protocol gets its
        // encoding (ConPTY passes it through). Otherwise, under ConPTY's
        // win32 input mode, keys VT can't express (Shift+Enter) go as key
        // records, as Windows Terminal sends them, for programs that read
        // console key events.
        let win32 = (!mode.contains(TermMode::DISAMBIGUATE_ESC_CODES)
            && self.session.win32_input_mode())
        .then(|| encode_win32(&input))
        .flatten();
        if let Some(bytes) = win32.or_else(|| encode_key(&input, mode)) {
            if self.pending_dead_key() {
                // Let Windows resolve the dead key: propagating lets gpui call
                // TranslateMessage, which emits the accent as text and clears the
                // keyboard state. This key's bytes are sent right after it.
                self.marked_text = None;
                self.deferred_input = Some(bytes);
                cx.spawn(async move |this, cx| {
                    // Fallback if no text arrives (e.g. dead key + arrow).
                    cx.background_executor()
                        .timer(Duration::from_millis(50))
                        .await;
                    let _ = this.update(cx, |this, cx| this.flush_deferred(cx));
                })
                .detach();
                return;
            }
            self.send_input(bytes, cx);
            cx.stop_propagation();
        }
    }

    /// A single-character composition on Windows is a dead key (e.g. `^` or `"`
    /// on international layouts), not an IME session.
    pub(super) fn pending_dead_key(&self) -> bool {
        cfg!(windows)
            && self
                .marked_text
                .as_ref()
                .is_some_and(|t| t.chars().count() == 1)
    }

    fn flush_deferred(&mut self, cx: &mut Context<Self>) {
        if let Some(bytes) = self.deferred_input.take() {
            self.send_input(bytes, cx);
        }
    }

    /// Sends user input: jumps back to the bottom and drops the selection.
    fn send_input(&mut self, bytes: impl Into<Vec<u8>>, cx: &mut Context<Self>) {
        self.session.term().lock().selection = None;
        self.session.scroll_to_bottom();
        self.session.write(bytes);
        self.last_input = Some(std::time::Instant::now());
        crate::perf::input_sent();
        cx.notify();
    }

    // --- Clipboard ----------------------------------------------------------

    pub(super) fn copy(&mut self, _: &Copy, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = self.selection_text() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    pub(super) fn paste(&mut self, _: &Paste, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return;
        };
        let mode = *self.session.term().lock().mode();
        self.send_input(encode_paste(&text, mode), cx);
    }

    // --- Mouse --------------------------------------------------------------

    fn mode(&self) -> TermMode {
        *self.session.term().lock().mode()
    }

    fn report(
        &mut self,
        button: Option<mouse::MouseButton>,
        action: MouseAction,
        (col, row): (usize, usize),
        modifiers: &Modifiers,
        mode: TermMode,
    ) {
        let report = MouseReport {
            button,
            action,
            col,
            row,
            shift: modifiers.shift,
            alt: modifiers.alt,
            ctrl: modifiers.control,
        };
        if let Some(bytes) = encode_mouse(&report, mode) {
            self.session.write(bytes);
        }
        self.last_reported_cell = Some((col, row));
    }

    /// The OSC 8 hyperlink under a viewport cell, if it's a URL we can open.
    fn hyperlink_at(&self, col: usize, row: usize) -> Option<String> {
        let point = self.grid_point(col, row);
        let uri = self.session.term().lock().grid()[point]
            .hyperlink()?
            .uri()
            .to_owned();
        // Only hand well-known schemes to the OS: a link's target is chosen by
        // whatever program printed it.
        let allowed = ["http://", "https://", "mailto:", "file://"];
        allowed
            .iter()
            .any(|scheme| uri.to_ascii_lowercase().starts_with(scheme))
            .then_some(uri)
    }

    /// Converts a viewport cell to a grid point (accounting for scrollback).
    fn grid_point(&self, col: usize, row: usize) -> GridPoint {
        let offset = self.session.term().lock().grid().display_offset() as i32;
        GridPoint::new(Line(row as i32 - offset), Column(col))
    }

    pub(super) fn on_mouse_down(
        &mut self,
        ev: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus);
        let Some(grid) = self.painted.get().grid else {
            return;
        };
        let (col, row, side) = grid.cell_at(ev.position);

        // Ctrl+click (Cmd+click on macOS) opens an OSC 8 hyperlink.
        let open_modifier = if cfg!(target_os = "macos") {
            ev.modifiers.platform
        } else {
            ev.modifiers.control
        };
        if ev.button == MouseButton::Left
            && open_modifier
            && let Some(uri) = self.hyperlink_at(col, row)
        {
            cx.open_url(&uri);
            return;
        }

        let mode = self.mode();

        if wants_mouse(mode, ev.modifiers.shift) {
            if let Some(button) = report_button(ev.button) {
                self.reported_button = Some(button);
                self.report(
                    Some(button),
                    MouseAction::Press,
                    (col, row),
                    &ev.modifiers,
                    mode,
                );
            }
            return;
        }

        match ev.button {
            MouseButton::Left => {
                let ty = match ev.click_count {
                    2 => SelectionType::Semantic,
                    n if n >= 3 => SelectionType::Lines,
                    _ => SelectionType::Simple,
                };
                let point = self.grid_point(col, row);
                self.session.term().lock().selection = Some(Selection::new(ty, point, side));
                self.selecting = true;
                cx.notify();
            }
            // Like Windows Terminal: right click copies the selection, or pastes.
            MouseButton::Right => match self.selection_text() {
                Some(text) => {
                    cx.write_to_clipboard(ClipboardItem::new_string(text));
                    self.session.term().lock().selection = None;
                    cx.notify();
                }
                None => self.paste(&Paste, window, cx),
            },
            _ => {}
        }
    }

    pub(super) fn on_mouse_move(&mut self, ev: &MouseMoveEvent, cx: &mut Context<Self>) {
        let Some(grid) = self.painted.get().grid else {
            return;
        };
        let (col, row, side) = grid.cell_at(ev.position);

        if self.selecting && ev.pressed_button == Some(MouseButton::Left) {
            // TODO: auto-scroll when dragging above or below the grid.
            let point = self.grid_point(col, row);
            if let Some(selection) = self.session.term().lock().selection.as_mut() {
                selection.update(point, side);
            }
            cx.notify();
            return;
        }

        let mode = self.mode();
        if !wants_mouse(mode, ev.modifiers.shift) || self.last_reported_cell == Some((col, row)) {
            return;
        }
        // Hover motion is only reported inside the grid; drags anywhere.
        if self.reported_button.is_some() || grid.contains(ev.position) {
            self.report(
                self.reported_button,
                MouseAction::Motion,
                (col, row),
                &ev.modifiers,
                mode,
            );
        }
    }

    pub(super) fn on_mouse_up(&mut self, ev: &MouseUpEvent, cx: &mut Context<Self>) {
        if self.selecting && ev.button == MouseButton::Left {
            self.selecting = false;
            let mut term = self.session.term().lock();
            if term.selection.as_ref().is_some_and(Selection::is_empty) {
                term.selection = None;
            }
            cx.notify();
        }

        if let Some(button) = self.reported_button
            && report_button(ev.button) == Some(button)
        {
            self.reported_button = None;
            if let Some(grid) = self.painted.get().grid {
                let (col, row, _) = grid.cell_at(ev.position);
                let mode = self.mode();
                self.report(
                    Some(button),
                    MouseAction::Release,
                    (col, row),
                    &ev.modifiers,
                    mode,
                );
            }
        }
    }

    pub(super) fn on_scroll(
        &mut self,
        ev: &ScrollWheelEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let line_height = self.line_height();
        self.scroll_accum += match ev.delta {
            ScrollDelta::Lines(p) => p.y,
            ScrollDelta::Pixels(p) => p.y / line_height,
        };
        let lines = self.scroll_accum.trunc() as i32;
        if lines == 0 {
            return;
        }
        self.scroll_accum -= lines as f32;

        let mode = self.mode();
        if wants_mouse(mode, ev.modifiers.shift) {
            let Some(grid) = self.painted.get().grid else {
                return;
            };
            let (col, row, _) = grid.cell_at(ev.position);
            let button = if lines > 0 {
                mouse::MouseButton::WheelUp
            } else {
                mouse::MouseButton::WheelDown
            };
            for _ in 0..lines.unsigned_abs() {
                self.report(
                    Some(button),
                    MouseAction::Press,
                    (col, row),
                    &ev.modifiers,
                    mode,
                );
            }
        } else if mode.contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL) {
            // Full-screen apps (less, vim...) get arrow keys instead of scrollback.
            let arrow: &[u8] = match (lines > 0, mode.contains(TermMode::APP_CURSOR)) {
                (true, true) => b"\x1bOA",
                (true, false) => b"\x1b[A",
                (false, true) => b"\x1bOB",
                (false, false) => b"\x1b[B",
            };
            self.session
                .write(arrow.repeat(lines.unsigned_abs() as usize));
        } else {
            self.session.scroll(lines);
            cx.notify();
        }
    }
}

/// Text input from the platform: typed characters, dead keys, AltGr and IME.
/// The terminal has no editable document, so only composition state is kept.
impl EntityInputHandler for TerminalView {
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
        Some(UTF16Selection {
            range: 0..0,
            reversed: false,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        // A pending dead key (one character) is not reported as a composition:
        // gpui would otherwise stop delivering key-down events, and control
        // keys (Enter, Backspace...) typed after it would be lost.
        self.marked_text
            .as_ref()
            .filter(|_| !self.pending_dead_key())
            .map(|t| 0..t.encode_utf16().count())
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.marked_text = None;
    }

    fn replace_text_in_range(
        &mut self,
        _range: Option<Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.marked_text = None;
        if !text.is_empty() {
            self.send_input(text, cx);
        }
        self.flush_deferred(cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _range: Option<Range<usize>>,
        new_text: &str,
        _new_selected_range: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // TODO(M4): draw the composition string at the cursor.
        self.marked_text = (!new_text.is_empty()).then(|| new_text.to_owned());
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        _range_utf16: Range<usize>,
        _element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        // Places the IME candidate window at the cursor.
        self.painted.get().cursor
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
