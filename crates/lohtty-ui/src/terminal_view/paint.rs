//! Turns the terminal grid into gpui quads and shaped text.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Instant;

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point as GridPoint};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::vte::ansi::CursorShape;
use gpui::{
    App, BorderStyle, Bounds, Font, FontId, GlyphId, Hsla, PaintQuad, Path, PathBuilder, Pixels,
    Point, Rgba, ShapedLine, StrikethroughStyle, TextRun, UnderlineStyle, Window, WindowTextSystem,
    fill, outline, point, px, size,
};
use lohtty_term::search::{self, Match, RegexSearch};
use lohtty_term::{Session, TermSize};
use lohtty_theme::Theme;

use super::box_drawing::{self, Shape};
use super::{CellMetrics, GridGeometry, PaintState};
use crate::colors::{self, to_rgba};
use crate::perf;

/// Everything needed to lay out one frame, captured by the canvas.
pub(super) struct FramePainter {
    pub session: Rc<Session>,
    pub theme: Theme,
    /// Regular, bold, italic, bold-italic.
    pub fonts: [Font; 4],
    /// Whether the fonts request ligatures (`calt`, `liga`).
    pub ligatures: bool,
    /// Kitty graphics of this session, if enabled.
    #[cfg(feature = "kitty-graphics")]
    pub images: Option<super::images::ImageLayer>,
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

/// A glyph painted straight at its cell, without shaping its row.
struct CellGlyph {
    /// Top-left of the cell.
    origin: Point<Pixels>,
    font_id: FontId,
    id: GlyphId,
    color: Hsla,
    emoji: bool,
}

/// A box-drawing or block character's shapes, ready to paint.
enum BoxShape {
    Quad(PaintQuad),
    Path(Path<Pixels>, Hsla),
}

/// The glyph a character renders as, alone, in a given font (possibly a
/// fallback font). `None` when it takes several glyphs.
type CachedGlyph = Option<(FontId, GlyphId, bool)>;

thread_local! {
    /// Glyph lookups by (requested font, character). Glyph ids don't depend on
    /// the font size, so this survives zooming; it's only bounded in size.
    static GLYPHS: RefCell<HashMap<(FontId, char), CachedGlyph>> = RefCell::new(HashMap::new());
}

/// Characters that can take part in a ligature or contextual alternate
/// (`->`, `!=`, `::`, `/*`...). Runs of two or more are shaped by DirectWrite;
/// everything else is painted glyph by glyph.
fn ligature_candidate(c: char) -> bool {
    c.is_ascii_punctuation()
}

pub(super) struct Frame {
    rects: Vec<PaintQuad>,
    cursor_below: Option<PaintQuad>,
    lines: Vec<(Point<Pixels>, ShapedLine)>,
    glyphs: Vec<CellGlyph>,
    /// Box-drawing and block characters, painted with the glyphs.
    boxes: Vec<BoxShape>,
    #[cfg(feature = "kitty-graphics")]
    images: super::images::ImageFrame,
    /// Distance from the top of a cell to the text baseline.
    baseline: Pixels,
    font_size: Pixels,
    /// The terminal's bounds; `glyphs` are painted as one layer within them.
    bounds: Bounds<Pixels>,
    cursor_above: Option<PaintQuad>,
    composition: Option<(PaintQuad, Point<Pixels>, ShapedLine)>,
    line_height: Pixels,
}

impl FramePainter {
    pub fn prepaint(mut self, bounds: Bounds<Pixels>, window: &mut Window) -> Frame {
        let started = Instant::now();
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
        let mut boxes = Vec::new();
        let scale = window.scale_factor();
        let box_weights = box_drawing::Weights::for_font(f32::from(m.font_size) * scale);

        #[cfg(feature = "kitty-graphics")]
        let mut markers = Vec::new();
        {
            let term = self.session.term().lock();

            // Search matches intersecting the viewport.
            let (visible_matches, current_match) = match self.search.take() {
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
            // Blank cells seen after the current segment, not yet added to it.
            let mut pending_spaces = 0usize;
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

                // Text. Plain spaces are not drawn on their own: they're held
                // back and only added to the current segment if more text
                // follows on the same row, so a row shapes as one segment
                // (glyphs are snapped to cells) without trailing blanks.
                let link = cell.hyperlink();
                // Kitty image markers are blank cells, not links.
                #[cfg(feature = "kitty-graphics")]
                let link = link.filter(|h| {
                    let marker = super::images::marker(h.uri(), row, col);
                    let is_marker = marker.is_some();
                    markers.extend(marker);
                    !is_marker
                });
                let hyperlink = link.is_some();
                let underlined = flags.intersects(Flags::ALL_UNDERLINES) || hyperlink;
                let decorated = underlined || flags.contains(Flags::STRIKEOUT);
                if flags.contains(Flags::HIDDEN) {
                    segments.extend(seg.take());
                    pending_spaces = 0;
                    continue;
                }
                // Box-drawing and block characters are drawn as shapes so they
                // join across cells; decorated or combined ones keep the font.
                if box_drawing::supported(cell.c)
                    && width == 1
                    && !decorated
                    && cell.zerowidth().is_none()
                {
                    segments.extend(seg.take());
                    pending_spaces = 0;
                    box_shapes(
                        cell.c,
                        cell_bounds(row, col, 1),
                        scale,
                        box_weights,
                        fg,
                        &mut boxes,
                    );
                    continue;
                }
                if cell.c == ' ' && !decorated && !is_cursor {
                    if seg.as_ref().is_some_and(|s| s.row == row) {
                        pending_spaces += 1;
                    }
                    continue;
                }

                // Wide chars and the cursor cell get their own segment: wide
                // glyphs must not shift their neighbors, and ligatures are
                // broken under the cursor so the edited character stays legible.
                let alone = width == 2 || is_cursor;
                let contiguous = matches!(&seg, Some(s)
                    if s.row == row && s.col + s.cells + pending_spaces == col);
                if !contiguous || alone {
                    segments.extend(seg.take());
                } else if let Some(s) = seg.as_mut()
                    && pending_spaces > 0
                {
                    // Spaces only need advance: extend the previous run unless
                    // it's decorated (spaces must not be underlined/struck).
                    s.text.extend(std::iter::repeat_n(' ', pending_spaces));
                    if let Some(last) = s.runs.last_mut() {
                        if last.underline.is_none() && last.strikethrough.is_none() {
                            last.len += pending_spaces;
                        } else {
                            let plain = TextRun {
                                len: pending_spaces,
                                underline: None,
                                strikethrough: None,
                                ..last.clone()
                            };
                            s.runs.push(plain);
                        }
                    }
                    s.cells += pending_spaces;
                }
                pending_spaces = 0;
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

        #[cfg(feature = "kitty-graphics")]
        let images = self
            .images
            .as_ref()
            .map(|layer| layer.layout(markers, bounds, m))
            .unwrap_or_default();

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

        let grid_time = started.elapsed();
        let shape_start = Instant::now();
        let segment_count = segments.len();
        let text_system = window.text_system().clone();
        let mut lines = Vec::new();
        let mut glyphs = Vec::new();
        for s in segments {
            let origin = cell_bounds(s.row, s.col, s.cells).origin;
            self.layout_segment(s, origin, &text_system, &mut lines, &mut glyphs);
        }
        // The baseline of the regular font, as a shaped line would place it.
        let reference = text_system.shape_line(
            "M".into(),
            m.font_size,
            &[plain_run(1, &self.fonts[0], Hsla::default())],
            None,
        );
        let baseline =
            (m.line_height - reference.ascent - reference.descent) / 2. + reference.ascent;
        perf::record_prepaint(perf::FrameTimings {
            grid: grid_time,
            shape: shape_start.elapsed(),
            segments: segment_count,
        });

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
            glyphs,
            boxes,
            #[cfg(feature = "kitty-graphics")]
            images,
            baseline,
            font_size: m.font_size,
            bounds,
            cursor_above,
            composition,
            line_height: m.line_height,
        }
    }
}

impl FramePainter {
    /// Lays out one segment. Shaping a whole row through DirectWrite is the
    /// main cost while output streams, so plain cells are painted straight
    /// from cached glyph ids, snapped to the grid like `force_width` does.
    /// Only spans that can form ligatures, and segments that need the full
    /// layout (combining marks, underline or strikethrough), are shaped.
    fn layout_segment(
        &self,
        s: Segment,
        origin: Point<Pixels>,
        text_system: &WindowTextSystem,
        lines: &mut Vec<(Point<Pixels>, ShapedLine)>,
        glyphs: &mut Vec<CellGlyph>,
    ) {
        let m = self.metrics;
        let decorated = s
            .runs
            .iter()
            .any(|r| r.underline.is_some() || r.strikethrough.is_some());
        if !s.snap || decorated {
            // Pin each glyph to its cell, so fallback glyphs (icons, symbols)
            // with a different advance can't push the rest of the line off-grid.
            let force_width = s.snap.then_some(m.cell_width);
            let shaped = text_system.shape_line(s.text.into(), m.font_size, &s.runs, force_width);
            lines.push((origin, shaped));
            return;
        }

        let cell_origin = |cell: usize| point(origin.x + m.cell_width * cell as f32, origin.y);
        // (byte offset, char, run index) for every character. Every character
        // is one cell here: wide characters are segments of their own, and
        // combining marks were excluded above.
        let mut chars = Vec::with_capacity(s.text.len());
        let mut start = 0;
        for (r, run) in s.runs.iter().enumerate() {
            chars.extend(
                s.text[start..start + run.len]
                    .char_indices()
                    .map(|(i, c)| (start + i, c, r)),
            );
            start += run.len;
        }
        let byte_end = |k: usize| chars.get(k).map_or(s.text.len(), |&(i, _, _)| i);
        // Shapes cells `from..to`, keeping the colors and fonts of their runs
        // (a ligature may span a color change, as in a whole-row layout).
        let shape_span = |from: usize, to: usize, lines: &mut Vec<_>| {
            let mut runs: Vec<TextRun> = Vec::new();
            for k in from..to {
                let len = byte_end(k + 1) - byte_end(k);
                let run = &s.runs[chars[k].2];
                match runs.last_mut() {
                    Some(last) if k > from && chars[k - 1].2 == chars[k].2 => last.len += len,
                    _ => runs.push(plain_run(len, &run.font, run.color)),
                }
            }
            let text = &s.text[byte_end(from)..byte_end(to)];
            let shaped = text_system.shape_line(
                text.to_owned().into(),
                m.font_size,
                &runs,
                Some(m.cell_width),
            );
            lines.push((cell_origin(from), shaped));
        };

        let font_ids: Vec<FontId> = s
            .runs
            .iter()
            .map(|r| text_system.resolve_font(&r.font))
            .collect();
        let mut k = 0;
        while k < chars.len() {
            let (_, c, r) = chars[k];
            if c == ' ' {
                k += 1;
                continue;
            }
            if self.ligatures && ligature_candidate(c) {
                let end = (k + 1..chars.len())
                    .find(|&j| !ligature_candidate(chars[j].1))
                    .unwrap_or(chars.len());
                if end - k >= 2 {
                    shape_span(k, end, lines);
                    k = end;
                    continue;
                }
            }
            let run = &s.runs[r];
            match glyph_for(text_system, font_ids[r], &run.font, c, m.font_size) {
                Some((font_id, id, emoji)) => glyphs.push(CellGlyph {
                    origin: cell_origin(k),
                    font_id,
                    id,
                    color: run.color,
                    emoji,
                }),
                None => shape_span(k, k + 1, lines),
            }
            k += 1;
        }
    }
}

/// Adds the shapes of box-drawing character `c` in `cell`, converted from
/// device pixels back to the window's logical pixels.
fn box_shapes(
    c: char,
    cell: Bounds<Pixels>,
    scale: f32,
    weights: box_drawing::Weights,
    color: Rgba,
    out: &mut Vec<BoxShape>,
) {
    let device = box_drawing::Cell {
        x: f32::from(cell.origin.x) * scale,
        y: f32::from(cell.origin.y) * scale,
        w: f32::from(cell.size.width) * scale,
        h: f32::from(cell.size.height) * scale,
    };
    let at = |(x, y): (f32, f32)| point(px(x / scale), px(y / scale));
    for shape in box_drawing::shapes(c, device, weights).unwrap_or_default() {
        match shape {
            Shape::Rect {
                x0,
                y0,
                x1,
                y1,
                alpha,
            } => {
                let color = Rgba {
                    a: color.a * alpha,
                    ..color
                };
                out.push(BoxShape::Quad(fill(
                    Bounds::from_corners(at((x0, y0)), at((x1, y1))),
                    color,
                )));
            }
            Shape::Stroke {
                start,
                segments,
                width,
            } => {
                let mut path = PathBuilder::stroke(px(width / scale));
                path.move_to(at(start));
                for [c1, c2, to] in segments {
                    if c1 == c2 && c2 == to {
                        path.line_to(at(to));
                    } else {
                        path.cubic_bezier_to(at(to), at(c1), at(c2));
                    }
                }
                if let Ok(path) = path.build() {
                    out.push(BoxShape::Path(path, color.into()));
                }
            }
            Shape::Fill { start, segments } => {
                let mut path = PathBuilder::fill();
                path.move_to(at(start));
                for [c1, c2, to] in segments {
                    if c1 == c2 && c2 == to {
                        path.line_to(at(to));
                    } else {
                        path.cubic_bezier_to(at(to), at(c1), at(c2));
                    }
                }
                path.close();
                if let Ok(path) = path.build() {
                    out.push(BoxShape::Path(path, color.into()));
                }
            }
        }
    }
}

fn plain_run(len: usize, font: &Font, color: Hsla) -> TextRun {
    TextRun {
        len,
        font: font.clone(),
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    }
}

/// The single glyph `c` renders as in `font`, found once by shaping it alone.
fn glyph_for(
    text_system: &WindowTextSystem,
    font_id: FontId,
    font: &Font,
    c: char,
    font_size: Pixels,
) -> CachedGlyph {
    GLYPHS.with_borrow_mut(|cache| {
        if let Some(&found) = cache.get(&(font_id, c)) {
            return found;
        }
        if cache.len() >= 1 << 16 {
            cache.clear();
        }
        let text = c.to_string();
        let shaped = text_system.shape_line(
            text.clone().into(),
            font_size,
            &[plain_run(text.len(), font, Hsla::default())],
            None,
        );
        let found = match shaped.runs.as_slice() {
            [run] => match run.glyphs.as_slice() {
                [g] => Some((run.font_id, g.id, g.is_emoji)),
                _ => None,
            },
            _ => None,
        };
        cache.insert((font_id, c), found);
        found
    })
}

impl Frame {
    pub fn paint(self, window: &mut Window, cx: &mut App) {
        let started = Instant::now();
        self.paint_inner(window, cx);
        perf::record_paint(started.elapsed());
        perf::frame_painted();
    }

    fn paint_inner(self, window: &mut Window, cx: &mut App) {
        for quad in self.rects {
            window.paint_quad(quad);
        }
        if let Some(c) = self.cursor_below {
            window.paint_quad(c);
        }
        #[cfg(feature = "kitty-graphics")]
        self.images.paint(true, window);
        for (origin, line) in &self.lines {
            let _ = line.paint(*origin, self.line_height, window, cx);
        }
        // One layer for all glyphs: outside a layer, gpui's scene computes a
        // draw order per primitive, which costs more than painting them.
        window.paint_layer(self.bounds, |window| {
            for g in &self.glyphs {
                let at = point(g.origin.x, g.origin.y + self.baseline);
                let _ = if g.emoji {
                    window.paint_emoji(at, g.font_id, g.id, self.font_size)
                } else {
                    window.paint_glyph(at, g.font_id, g.id, self.font_size, g.color)
                };
            }
            for shape in self.boxes {
                match shape {
                    BoxShape::Quad(quad) => window.paint_quad(quad),
                    BoxShape::Path(path, color) => window.paint_path(path, color),
                }
            }
        });
        #[cfg(feature = "kitty-graphics")]
        self.images.paint(false, window);
        if let Some(c) = self.cursor_above {
            window.paint_quad(c);
        }
        if let Some((backdrop, origin, line)) = self.composition {
            window.paint_quad(backdrop);
            let _ = line.paint(origin, self.line_height, window, cx);
        }
        #[cfg(feature = "kitty-graphics")]
        self.images.release(window);
    }
}
