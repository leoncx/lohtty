//! The terminal grid view: paints a [`Session`]'s screen and forwards input.

use std::cell::Cell;
use std::ops::Range;
use std::rc::Rc;
use std::time::Duration;

use alacritty_terminal::term::TermMode;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::vte::ansi::CursorShape;
use gpui::{
    App, BorderStyle, Bounds, ClipboardItem, Context, ElementInputHandler, EntityInputHandler,
    FocusHandle, Focusable, Font, FontFeatures, FontStyle, FontWeight, IntoElement, KeyDownEvent,
    MouseButton, PaintQuad, Pixels, Point, Render, Rgba, ScrollDelta, ScrollWheelEvent, ShapedLine,
    StrikethroughStyle, Task, TextRun, UTF16Selection, UnderlineStyle, Window, canvas, div, fill,
    outline, point, prelude::*, px, size,
};
use lohtty_config::{Config, FontConfig};
use lohtty_term::input::{KeyInput, encode_key, is_text_key};
use lohtty_term::{PassThrough, Session, SessionEvent, TermSize, resolve_shell};
use lohtty_theme::Theme;

use crate::colors::{self, to_rgba};

const DEFAULT_TITLE: &str = "lohtty";

#[derive(Debug, Clone, Copy)]
struct CellMetrics {
    font_size: Pixels,
    cell_width: Pixels,
    line_height: Pixels,
}

pub struct TerminalView {
    session: Rc<Session>,
    theme: Theme,
    font: FontConfig,
    focus: FocusHandle,
    metrics: Option<CellMetrics>,
    scroll_accum: f32,
    /// IME composition in progress (dead key or IME), if any.
    marked_text: Option<String>,
    /// Key bytes held back until a pending dead key is resolved.
    deferred_input: Option<Vec<u8>>,
    /// Last painted cursor cell, for positioning the IME window.
    cursor_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    _events: Task<()>,
}

/// Starts the configured shell. Separate from [`TerminalView::new`] so that
/// failures surface before a view is created.
pub fn spawn_session(
    config: &Config,
) -> anyhow::Result<(Session, async_channel::Receiver<SessionEvent>)> {
    let shell = resolve_shell(config.shell.program.as_deref(), &config.shell.args);
    // The real size is applied on first paint, once the layout is known.
    let initial = TermSize {
        cols: 80,
        rows: 24,
        cell_width: 8,
        cell_height: 16,
    };
    Session::spawn(&shell, initial, config.scrollback, Box::new(PassThrough))
}

impl TerminalView {
    pub fn new(
        session: Session,
        events: async_channel::Receiver<SessionEvent>,
        config: &Config,
        theme: Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let task = cx.spawn_in(window, async move |this, cx| {
            while let Ok(event) = events.recv().await {
                let alive =
                    this.update_in(cx, |this, window, cx| this.handle_event(event, window, cx));
                if alive.is_err() {
                    break;
                }
            }
        });

        Self {
            session: Rc::new(session),
            theme,
            font: config.font.clone(),
            focus: cx.focus_handle(),
            metrics: None,
            scroll_accum: 0.0,
            marked_text: None,
            deferred_input: None,
            cursor_bounds: Rc::default(),
            _events: task,
        }
    }

    fn handle_event(&mut self, event: SessionEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            SessionEvent::Wakeup => cx.notify(),
            SessionEvent::Title(title) => window.set_window_title(&title),
            SessionEvent::ResetTitle => window.set_window_title(DEFAULT_TITLE),
            SessionEvent::Bell => {}
            SessionEvent::ClipboardStore(text) => {
                cx.write_to_clipboard(ClipboardItem::new_string(text))
            }
            SessionEvent::ColorRequest(index, format) => {
                let rgb = {
                    let term = self.session.term().lock();
                    colors::palette(index, term.colors(), &self.theme)
                };
                self.session.write(format(colors::to_term(rgb)));
            }
            SessionEvent::Exit => window.remove_window(),
        }
    }

    fn font(&self, bold: bool, italic: bool) -> Font {
        Font {
            family: self.font.family.clone().into(),
            features: if self.font.ligatures {
                FontFeatures::default()
            } else {
                FontFeatures::disable_ligatures()
            },
            fallbacks: None,
            weight: if bold {
                FontWeight::BOLD
            } else {
                FontWeight::NORMAL
            },
            style: if italic {
                FontStyle::Italic
            } else {
                FontStyle::Normal
            },
        }
    }

    fn metrics(&mut self, window: &mut Window) -> CellMetrics {
        if let Some(m) = self.metrics {
            return m;
        }
        let font_size = px(self.font.size);
        let text_system = window.text_system();
        let font_id = text_system.resolve_font(&self.font(false, false));
        let cell_width = text_system
            .advance(font_id, font_size, 'm')
            .map(|s| s.width)
            .unwrap_or(font_size * 0.6);
        let m = CellMetrics {
            font_size,
            cell_width,
            line_height: (font_size * 1.25).round(),
        };
        self.metrics = Some(m);
        m
    }

    fn on_key_down(&mut self, ev: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let ks = &ev.keystroke;
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
        if let Some(bytes) = encode_key(&input, mode) {
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
    fn pending_dead_key(&self) -> bool {
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

    fn send_input(&mut self, bytes: impl Into<Vec<u8>>, cx: &mut Context<Self>) {
        self.session.scroll_to_bottom();
        self.session.write(bytes);
        cx.notify();
    }

    fn on_scroll(&mut self, ev: &ScrollWheelEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let line_height = self.metrics.map_or(px(16.0), |m| m.line_height);
        self.scroll_accum += match ev.delta {
            ScrollDelta::Lines(p) => p.y,
            ScrollDelta::Pixels(p) => p.y / line_height,
        };
        let lines = self.scroll_accum.trunc() as i32;
        if lines == 0 {
            return;
        }
        self.scroll_accum -= lines as f32;

        let mode = *self.session.term().lock().mode();
        if mode.contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL) {
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

impl Focusable for TerminalView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
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
        self.cursor_bounds.get()
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

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let metrics = self.metrics(window);
        let painter = FramePainter {
            session: self.session.clone(),
            theme: self.theme.clone(),
            fonts: [
                self.font(false, false),
                self.font(true, false),
                self.font(false, true),
                self.font(true, true),
            ],
            metrics,
            focused: self.focus.is_focused(window),
            cursor_bounds: self.cursor_bounds.clone(),
        };
        let view = cx.entity();
        let focus = self.focus.clone();

        div()
            .id("terminal")
            .track_focus(&self.focus)
            .key_context("Terminal")
            .size_full()
            .on_key_down(cx.listener(Self::on_key_down))
            .on_scroll_wheel(cx.listener(Self::on_scroll))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, _| window.focus(&this.focus)),
            )
            .child(
                canvas(
                    move |bounds, window, _cx| painter.prepaint(bounds, window),
                    move |bounds, frame, window, cx| {
                        window.handle_input(&focus, ElementInputHandler::new(bounds, view), cx);
                        frame.paint(window, cx)
                    },
                )
                .size_full(),
            )
    }
}

/// Everything needed to lay out one frame, captured by the canvas.
struct FramePainter {
    session: Rc<Session>,
    theme: Theme,
    /// Regular, bold, italic, bold-italic.
    fonts: [Font; 4],
    metrics: CellMetrics,
    focused: bool,
    cursor_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
}

/// A run of adjacent cells shaped together (so ligatures can form).
struct Segment {
    row: usize,
    col: usize,
    cells: usize,
    text: String,
    runs: Vec<TextRun>,
}

#[derive(Clone, Copy, PartialEq)]
struct BgRect {
    row: usize,
    col: usize,
    cells: usize,
    color: Rgba,
}

struct Frame {
    rects: Vec<PaintQuad>,
    cursor_below: Option<PaintQuad>,
    lines: Vec<(Point<Pixels>, ShapedLine)>,
    cursor_above: Option<PaintQuad>,
    line_height: Pixels,
}

impl FramePainter {
    fn prepaint(self, bounds: Bounds<Pixels>, window: &mut Window) -> Frame {
        let m = self.metrics;
        let cols = ((bounds.size.width / m.cell_width) as usize).clamp(2, u16::MAX as usize);
        let rows = ((bounds.size.height / m.line_height) as usize).clamp(1, u16::MAX as usize);
        self.session.resize(TermSize {
            cols: cols as u16,
            rows: rows as u16,
            cell_width: f32::from(m.cell_width).round() as u16,
            cell_height: f32::from(m.line_height).round() as u16,
        });
        self.session.clear_dirty();

        let cell_bounds = |row: usize, col: usize, cells: usize| {
            Bounds::new(
                point(
                    bounds.origin.x + m.cell_width * col as f32,
                    bounds.origin.y + m.line_height * row as f32,
                ),
                size(m.cell_width * cells as f32, m.line_height),
            )
        };

        let theme = &self.theme;
        let default_bg = to_rgba(theme.background);
        let mut rects = Vec::new();
        let mut segments = Vec::new();
        let mut cursor_below = None;
        let mut cursor_above = None;

        {
            let term = self.session.term().lock();
            let content = term.renderable_content();
            let overrides = content.colors;
            let offset = content.display_offset as i32;
            let cursor = content.cursor;
            let cursor_row = cursor.point.line.0 + offset;
            let cursor_col = cursor.point.column.0;
            let block_cursor = self.focused && cursor.shape == CursorShape::Block;
            let mut cursor_wide = false;

            let mut seg: Option<Segment> = None;
            let mut bg: Option<BgRect> = None;

            for item in content.display_iter {
                let row_i = item.point.line.0 + offset;
                if row_i < 0 || row_i as usize >= rows {
                    continue;
                }
                let (row, col) = (row_i as usize, item.point.column.0);
                let cell = item.cell;
                let flags = cell.flags;
                if flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
                    continue;
                }
                let width = if flags.contains(Flags::WIDE_CHAR) {
                    2
                } else {
                    1
                };

                let mut fg = colors::resolve(cell.fg, overrides, theme);
                let mut bg_c = colors::resolve(cell.bg, overrides, theme);
                if flags.contains(Flags::INVERSE) {
                    std::mem::swap(&mut fg, &mut bg_c);
                }
                if flags.contains(Flags::DIM) {
                    fg = colors::dim(fg, bg_c);
                }
                let is_cursor = row_i == cursor_row && col == cursor_col;
                if is_cursor {
                    cursor_wide = width == 2;
                    if block_cursor {
                        fg = theme.background;
                    }
                }
                let (fg, bg_c) = (to_rgba(fg), to_rgba(bg_c));

                // Backgrounds: merge adjacent cells of the same color.
                if bg_c != default_bg {
                    match &mut bg {
                        Some(r) if r.row == row && r.col + r.cells == col && r.color == bg_c => {
                            r.cells += width
                        }
                        _ => {
                            if let Some(r) = bg.take() {
                                rects.push(fill(cell_bounds(r.row, r.col, r.cells), r.color));
                            }
                            bg = Some(BgRect {
                                row,
                                col,
                                cells: width,
                                color: bg_c,
                            });
                        }
                    }
                }

                // Text: spaces end a segment unless decorated.
                let decorated = flags.intersects(Flags::ALL_UNDERLINES | Flags::STRIKEOUT);
                if flags.contains(Flags::HIDDEN) || (cell.c == ' ' && !decorated) {
                    segments.extend(seg.take());
                    continue;
                }

                let contiguous = matches!(&seg, Some(s) if s.row == row && s.col + s.cells == col);
                if !contiguous || width == 2 {
                    segments.extend(seg.take());
                }
                let s = seg.get_or_insert_with(|| Segment {
                    row,
                    col,
                    cells: 0,
                    text: String::new(),
                    runs: Vec::new(),
                });

                let start = s.text.len();
                s.text.push(cell.c);
                if let Some(zw) = cell.zerowidth() {
                    s.text.extend(zw);
                }
                let len = s.text.len() - start;
                s.cells += width;

                let font_idx = flags.contains(Flags::BOLD) as usize
                    + 2 * flags.contains(Flags::ITALIC) as usize;
                let run = TextRun {
                    len,
                    font: self.fonts[font_idx].clone(),
                    color: fg.into(),
                    background_color: None,
                    underline: flags
                        .intersects(Flags::ALL_UNDERLINES)
                        .then(|| UnderlineStyle {
                            thickness: px(1.0),
                            color: None,
                            wavy: flags.contains(Flags::UNDERCURL),
                        }),
                    strikethrough: flags
                        .contains(Flags::STRIKEOUT)
                        .then(|| StrikethroughStyle {
                            thickness: px(1.0),
                            color: None,
                        }),
                };
                match s.runs.last_mut() {
                    Some(last)
                        if last.font == run.font
                            && last.color == run.color
                            && last.underline == run.underline
                            && last.strikethrough == run.strikethrough =>
                    {
                        last.len += len
                    }
                    _ => s.runs.push(run),
                }

                if width == 2 {
                    segments.extend(seg.take());
                }
            }
            segments.extend(seg.take());
            if let Some(r) = bg.take() {
                rects.push(fill(cell_bounds(r.row, r.col, r.cells), r.color));
            }

            // Cursor.
            if cursor.shape != CursorShape::Hidden
                && cursor_row >= 0
                && (cursor_row as usize) < rows
            {
                let cells = if cursor_wide { 2 } else { 1 };
                let b = cell_bounds(cursor_row as usize, cursor_col, cells);
                self.cursor_bounds.set(Some(b));
                let color = to_rgba(theme.cursor);
                let thin = px(2.0);
                match cursor.shape {
                    _ if !self.focused => {
                        cursor_above = Some(outline(b, color, BorderStyle::Solid))
                    }
                    CursorShape::Block => cursor_below = Some(fill(b, color)),
                    CursorShape::HollowBlock => {
                        cursor_above = Some(outline(b, color, BorderStyle::Solid))
                    }
                    CursorShape::Beam => {
                        cursor_above = Some(fill(
                            Bounds::new(b.origin, size(thin, b.size.height)),
                            color,
                        ))
                    }
                    CursorShape::Underline => {
                        cursor_above = Some(fill(
                            Bounds::new(
                                point(b.origin.x, b.origin.y + b.size.height - thin),
                                size(b.size.width, thin),
                            ),
                            color,
                        ))
                    }
                    CursorShape::Hidden => {}
                }
            }
        }

        let text_system = window.text_system().clone();
        let lines = segments
            .into_iter()
            .map(|s| {
                let origin = cell_bounds(s.row, s.col, s.cells).origin;
                let shaped = text_system.shape_line(s.text.into(), m.font_size, &s.runs, None);
                (origin, shaped)
            })
            .collect();

        Frame {
            rects,
            cursor_below,
            lines,
            cursor_above,
            line_height: m.line_height,
        }
    }
}

impl Frame {
    fn paint(self, window: &mut Window, cx: &mut App) {
        for quad in self.rects {
            window.paint_quad(quad);
        }
        if let Some(c) = self.cursor_below {
            window.paint_quad(c);
        }
        for (origin, line) in &self.lines {
            let _ = line.paint(*origin, self.line_height, window, cx);
        }
        if let Some(c) = self.cursor_above {
            window.paint_quad(c);
        }
    }
}
