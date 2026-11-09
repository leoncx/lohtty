//! Turns the terminal grid into gpui quads and shaped text.

use std::cell::Cell;
use std::rc::Rc;

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point as GridPoint};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::vte::ansi::CursorShape;
use gpui::{
    App, BorderStyle, Bounds, Font, PaintQuad, Pixels, Point, Rgba, ShapedLine, StrikethroughStyle,
    TextRun, UnderlineStyle, Window, fill, outline, point, px, size,
};
use lohtty_term::search::{self, Match, RegexSearch};
use lohtty_term::{Session, TermSize};
use lohtty_theme::Theme;

use super::{CellMetrics, GridGeometry, PaintState};
use crate::colors::{self, to_rgba};

/// Everything needed to lay out one frame, captured by the canvas.
pub(super) struct FramePainter {
    pub session: Rc<Session>,
    pub theme: Theme,
    /// Regular, bold, italic, bold-italic.
    pub fonts: [Font; 4],
    pub metrics: CellMetrics,
    pub focused: bool,
    pub painted: Rc<Cell<PaintState>>,
    /// IME composition text to draw at the cursor.
    pub marked_text: Option<String>,
    /// Active search: the compiled query and the current match.
    pub search: Option<(RegexSearch, Option<Match>)>,
}

/// A run of adjacent cells shaped together (so ligatures can form).
struct Segment {
    row: usize,
    col: usize,
    cells: usize,
    /// Snap glyphs to the cell grid (off when combining marks are present).
    snap: bool,
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

pub(super) struct Frame {
    rects: Vec<PaintQuad>,
    cursor_below: Option<PaintQuad>,
    lines: Vec<(Point<Pixels>, ShapedLine)>,
    cursor_above: Option<PaintQuad>,
    composition: Option<(PaintQuad, Point<Pixels>, ShapedLine)>,
    line_height: Pixels,
}

impl FramePainter {
    pub fn prepaint(self, bounds: Bounds<Pixels>, window: &mut Window) -> Frame {
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
        let selection_bg = to_rgba(theme.selection);
        // Search highlights derive from the theme's yellow.
        let current_match_bg = to_rgba(theme.ansi[3]);
        let match_bg = to_rgba(colors::mix(theme.background, theme.ansi[3], 0.35));
        let mut rects = Vec::new();
        let mut segments = Vec::new();
        let mut cursor_below = None;
        let mut cursor_above = None;
        let mut cursor_bounds = None;

        {
            let term = self.session.term().lock();

            // Search matches intersecting the viewport.
            let (visible_matches, current_match) = match self.search {
                Some((mut regex, current)) => {
                    let top = -(term.grid().display_offset() as i32);
                    let first = GridPoint::new(Line(top), Column(0));
                    let last = GridPoint::new(Line(top + rows as i32 - 1), term.last_column());
                    let found = search::matches_in_lines(&term, &mut regex, first, last);
                    (found, current)
                }
                None => (Vec::new(), None),
            };

            let content = term.renderable_content();
            let overrides = content.colors;
            let selection = content.selection;
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
                // Highlight priority: current search match, selection, other matches.
                let in_current = current_match
                    .as_ref()
                    .is_some_and(|m| m.contains(&item.point));
                if in_current {
                    fg = theme.background;
                }
                let fg = to_rgba(fg);
                let bg_c = if in_current {
                    current_match_bg
                } else if selection.is_some_and(|s| s.contains(item.point)) {
                    selection_bg
                } else if visible_matches.iter().any(|m| m.contains(&item.point)) {
                    match_bg
                } else {
                    to_rgba(bg_c)
                };

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
                let hyperlink = cell.hyperlink().is_some();
                let underlined = flags.intersects(Flags::ALL_UNDERLINES) || hyperlink;
                let decorated = underlined || flags.contains(Flags::STRIKEOUT);
                if flags.contains(Flags::HIDDEN) || (cell.c == ' ' && !decorated) {
                    segments.extend(seg.take());
                    continue;
                }

                // Wide chars and the cursor cell get their own segment: wide
                // glyphs must not shift their neighbors, and ligatures are
                // broken under the cursor so the edited character stays legible.
                let alone = width == 2 || is_cursor;
                let contiguous = matches!(&seg, Some(s) if s.row == row && s.col + s.cells == col);
                if !contiguous || alone {
                    segments.extend(seg.take());
                }
                let s = seg.get_or_insert_with(|| Segment {
                    row,
                    col,
                    cells: 0,
                    snap: true,
                    text: String::new(),
                    runs: Vec::new(),
                });

                let start = s.text.len();
                s.text.push(cell.c);
                if let Some(zw) = cell.zerowidth() {
                    s.text.extend(zw);
                    // Combining marks add glyphs: per-glyph snapping would misplace them.
                    s.snap = false;
                }
                let len = s.text.len() - start;
                s.cells += width;

                let font_idx = flags.contains(Flags::BOLD) as usize
                    + 2 * flags.contains(Flags::ITALIC) as usize;
                let underline_color = cell
                    .underline_color()
                    .map(|c| to_rgba(colors::resolve(c, overrides, theme)).into());
                let run = TextRun {
                    len,
                    font: self.fonts[font_idx].clone(),
                    color: fg.into(),
                    background_color: None,
                    underline: underlined.then(|| UnderlineStyle {
                        thickness: px(1.0),
                        color: underline_color,
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

                if alone {
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
                cursor_bounds = Some(b);
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

        self.painted.set(PaintState {
            grid: Some(GridGeometry {
                origin: bounds.origin,
                cell_width: m.cell_width,
                line_height: m.line_height,
                cols,
                rows,
            }),
            cursor: cursor_bounds,
        });

        let text_system = window.text_system().clone();
        let lines = segments
            .into_iter()
            .map(|s| {
                let origin = cell_bounds(s.row, s.col, s.cells).origin;
                // Pin each glyph to its cell, so fallback glyphs (icons, symbols)
                // with a different advance can't push the rest of the line off-grid.
                let force_width = s.snap.then_some(m.cell_width);
                let shaped =
                    text_system.shape_line(s.text.into(), m.font_size, &s.runs, force_width);
                (origin, shaped)
            })
            .collect();

        // IME composition: drawn over the grid at the cursor, underlined.
        let composition = self
            .marked_text
            .filter(|t| !t.is_empty())
            .zip(cursor_bounds)
            .map(|(text, at)| {
                let run = TextRun {
                    len: text.len(),
                    font: self.fonts[0].clone(),
                    color: to_rgba(theme.foreground).into(),
                    background_color: None,
                    underline: Some(UnderlineStyle {
                        thickness: px(1.0),
                        color: None,
                        wavy: false,
                    }),
                    strikethrough: None,
                };
                let shaped = text_system.shape_line(text.into(), m.font_size, &[run], None);
                let backdrop = fill(
                    Bounds::new(at.origin, size(shaped.width, m.line_height)),
                    to_rgba(theme.background),
                );
                (backdrop, at.origin, shaped)
            });

        Frame {
            rects,
            cursor_below,
            lines,
            cursor_above,
            composition,
            line_height: m.line_height,
        }
    }
}

impl Frame {
    pub fn paint(self, window: &mut Window, cx: &mut App) {
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
        if let Some((backdrop, origin, line)) = self.composition {
            window.paint_quad(backdrop);
            let _ = line.paint(origin, self.line_height, window, cx);
        }
    }
}
