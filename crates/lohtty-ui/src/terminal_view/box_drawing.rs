//! Box-drawing (U+2500–U+257F) and block-element (U+2580–U+259F) characters,
//! and the Powerline symbols (U+E0B0–U+E0BF, from Nerd Fonts), drawn as
//! shapes instead of font glyphs. Font glyphs are designed for the
//! font's own line height, so at lohtty's 1.25 line spacing (or with a font
//! that lacks them) lines don't join across cells. Here every line runs to
//! the cell edges, and edges are snapped to device pixels so neighbors meet
//! exactly.
//!
//! Geometry is computed in device pixels: the caller passes the cell in
//! device pixels and converts the shapes back.

/// A cell, in device pixels.
#[derive(Clone, Copy, Debug)]
pub struct Cell {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// A shape to fill with the cell's foreground color, in device pixels.
#[derive(Clone, Debug, PartialEq)]
pub enum Shape {
    /// `x0..x1` by `y0..y1`, at `alpha` times the color's opacity.
    Rect {
        x0: f32,
        y0: f32,
        x1: f32,
        y1: f32,
        alpha: f32,
    },
    /// A stroked path `width` wide: a start point, then cubic segments
    /// `(control 1, control 2, end)`; straight segments repeat their ends as
    /// controls.
    Stroke {
        start: (f32, f32),
        segments: Vec<[(f32, f32); 3]>,
        width: f32,
    },
    /// A filled outline, in the same form as `Stroke`, closed back to
    /// `start`.
    Fill {
        start: (f32, f32),
        segments: Vec<[(f32, f32); 3]>,
    },
}

/// Whether `c` is drawn here rather than from the font.
pub fn supported(c: char) -> bool {
    matches!(c, '\u{2500}'..='\u{259F}' | '\u{E0B0}'..='\u{E0BF}')
}

/// Line weights, from the light line thickness in device pixels (scaled with
/// the font size, at least one pixel).
#[derive(Clone, Copy, Debug)]
pub struct Weights {
    light: f32,
    heavy: f32,
}

impl Weights {
    pub fn for_font(font_size_device_px: f32) -> Self {
        let light = (font_size_device_px / 12.0).round().max(1.0);
        Self {
            light,
            heavy: light * 2.0,
        }
    }
}

/// The shapes `c` is drawn with, or `None` if it isn't [`supported`].
pub fn shapes(c: char, cell: Cell, weights: Weights) -> Option<Vec<Shape>> {
    let mut out = Vec::new();
    let d = Draw {
        cell,
        weights,
        out: &mut out,
    };
    match c {
        '\u{2504}'..='\u{250B}' | '\u{254C}'..='\u{254F}' => d.dashed(c),
        '\u{256D}'..='\u{2570}' => d.arc(c),
        '\u{2571}'..='\u{2573}' => d.diagonal(c),
        '\u{2580}'..='\u{259F}' => d.block(c),
        '\u{E0B0}'..='\u{E0BF}' => d.powerline(c),
        _ => d.lines(arms(c)?),
    }
    Some(out)
}

/// The weight of one arm of a box-drawing character.
#[derive(Clone, Copy, Debug, PartialEq)]
enum W {
    None,
    Light,
    Heavy,
    Double,
}

impl W {
    fn single(self) -> bool {
        matches!(self, W::Light | W::Heavy)
    }
}

/// Left, right, up, down.
type Arms = [W; 4];

/// The arms of the line characters (everything but dashes, arcs and
/// diagonals), from the weights in their Unicode names.
fn arms(c: char) -> Option<Arms> {
    // Digits per arm, in the order left, right, up, down:
    // 0 none, 1 light, 2 heavy, 3 double.
    let code = match c {
        '─' => "1100",
        '━' => "2200",
        '│' => "0011",
        '┃' => "0022",
        '┌' => "0101",
        '┍' => "0201",
        '┎' => "0102",
        '┏' => "0202",
        '┐' => "1001",
        '┑' => "2001",
        '┒' => "1002",
        '┓' => "2002",
        '└' => "0110",
        '┕' => "0210",
        '┖' => "0120",
        '┗' => "0220",
        '┘' => "1010",
        '┙' => "2010",
        '┚' => "1020",
        '┛' => "2020",
        '├' => "0111",
        '┝' => "0211",
        '┞' => "0121",
        '┟' => "0112",
        '┠' => "0122",
        '┡' => "0221",
        '┢' => "0212",
        '┣' => "0222",
        '┤' => "1011",
        '┥' => "2011",
        '┦' => "1021",
        '┧' => "1012",
        '┨' => "1022",
        '┩' => "2021",
        '┪' => "2012",
        '┫' => "2022",
        '┬' => "1101",
        '┭' => "2101",
        '┮' => "1201",
        '┯' => "2201",
        '┰' => "1102",
        '┱' => "2102",
        '┲' => "1202",
        '┳' => "2202",
        '┴' => "1110",
        '┵' => "2110",
        '┶' => "1210",
        '┷' => "2210",
        '┸' => "1120",
        '┹' => "2120",
        '┺' => "1220",
        '┻' => "2220",
        '┼' => "1111",
        '┽' => "2111",
        '┾' => "1211",
        '┿' => "2211",
        '╀' => "1121",
        '╁' => "1112",
        '╂' => "1122",
        '╃' => "2121",
        '╄' => "1221",
        '╅' => "2112",
        '╆' => "1212",
        '╇' => "2221",
        '╈' => "2212",
        '╉' => "2122",
        '╊' => "1222",
        '╋' => "2222",
        '═' => "3300",
        '║' => "0033",
        '╒' => "0301",
        '╓' => "0103",
        '╔' => "0303",
        '╕' => "3001",
        '╖' => "1003",
        '╗' => "3003",
        '╘' => "0310",
        '╙' => "0130",
        '╚' => "0330",
        '╛' => "3010",
        '╜' => "1030",
        '╝' => "3030",
        '╞' => "0311",
        '╟' => "0133",
        '╠' => "0333",
        '╡' => "3011",
        '╢' => "1033",
        '╣' => "3033",
        '╤' => "3301",
        '╥' => "1103",
        '╦' => "3303",
        '╧' => "3310",
        '╨' => "1130",
        '╩' => "3330",
        '╪' => "3311",
        '╫' => "1133",
        '╬' => "3333",
        '╴' => "1000",
        '╵' => "0010",
        '╶' => "0100",
        '╷' => "0001",
        '╸' => "2000",
        '╹' => "0020",
        '╺' => "0200",
        '╻' => "0002",
        '╼' => "1200",
        '╽' => "0012",
        '╾' => "2100",
        '╿' => "0021",
        _ => return None,
    };
    let mut arms = [W::None; 4];
    for (arm, digit) in arms.iter_mut().zip(code.bytes()) {
        *arm = match digit {
            b'1' => W::Light,
            b'2' => W::Heavy,
            b'3' => W::Double,
            _ => W::None,
        };
    }
    Some(arms)
}

struct Draw<'a> {
    cell: Cell,
    weights: Weights,
    out: &'a mut Vec<Shape>,
}

/// A line of thickness `t` centered `off` away from `center`, snapped to
/// whole pixels: `(start, end)`.
fn span(center: f32, off: f32, t: f32) -> (f32, f32) {
    let start = (center + off - t / 2.0).round();
    (start, start + t)
}

impl Draw<'_> {
    fn thickness(&self, w: W) -> f32 {
        match w {
            W::Heavy => self.weights.heavy,
            _ => self.weights.light,
        }
    }

    fn rect(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, alpha: f32) {
        if x1 > x0 && y1 > y0 {
            self.out.push(Shape::Rect {
                x0,
                y0,
                x1,
                y1,
                alpha,
            });
        }
    }

    /// The cell's x and y edges, snapped.
    fn edges(&self) -> (f32, f32, f32, f32) {
        let c = self.cell;
        (
            c.x.round(),
            (c.x + c.w).round(),
            c.y.round(),
            (c.y + c.h).round(),
        )
    }

    fn center(&self) -> (f32, f32) {
        let c = self.cell;
        (c.x + c.w / 2.0, c.y + c.h / 2.0)
    }

    /// Straight arms meeting at the center.
    fn lines(mut self, [left, right, up, down]: Arms) {
        let (x0, x1, y0, y1) = self.edges();
        let (cx, cy) = self.center();
        // Horizontal arms: along x, with up/down as the perpendicular arms.
        for (side, this, opposite, edge) in [(-1.0, left, right, x0), (1.0, right, left, x1)] {
            for (a, b, c0, c1) in self.arm(side, this, opposite, up, down, cx, cy, edge) {
                self.rect(a, c0, b, c1, 1.0);
            }
        }
        // Vertical arms: along y, with left/right as the perpendicular arms.
        for (side, this, opposite, edge) in [(-1.0, up, down, y0), (1.0, down, up, y1)] {
            for (a, b, c0, c1) in self.arm(side, this, opposite, left, right, cy, cx, edge) {
                self.rect(c0, a, c1, b, 1.0);
            }
        }
    }

    /// The rectangles of one arm, as `(main start, main end, cross start,
    /// cross end)`: "main" runs along the arm, from the center toward the
    /// cell edge on `side` (-1 or 1). `neg` and `pos` are the perpendicular
    /// arms on the negative and positive cross side.
    #[allow(clippy::too_many_arguments)]
    fn arm(
        &self,
        side: f32,
        this: W,
        opposite: W,
        neg: W,
        pos: W,
        main_center: f32,
        cross_center: f32,
        edge: f32,
    ) -> Vec<(f32, f32, f32, f32)> {
        let light = self.weights.light;
        let gap = light;
        // The inner end of the arm when it must cover a perpendicular line
        // (start, end) up to its far side.
        let cover = |(start, end): (f32, f32)| if side > 0.0 { start } else { end };
        let center = main_center.round();
        let perp_single = [neg, pos]
            .into_iter()
            .filter(|w| w.single())
            .map(|w| self.thickness(w))
            .fold(0.0, f32::max);
        let perp_double = neg == W::Double || pos == W::Double;
        let piece = |inner: f32, cross: (f32, f32)| {
            let (a, b) = if side > 0.0 {
                (inner, edge)
            } else {
                (edge, inner)
            };
            (a, b, cross.0, cross.1)
        };

        match this {
            W::None => Vec::new(),
            W::Light | W::Heavy => {
                let inner = if perp_double {
                    // Up to the far line of the double.
                    cover(span(main_center, -side * gap, light))
                } else if perp_single > 0.0 {
                    cover(span(main_center, 0.0, perp_single))
                } else {
                    center
                };
                vec![piece(inner, span(cross_center, 0.0, self.thickness(this)))]
            }
            W::Double => [(-gap, neg, pos), (gap, pos, neg)]
                .into_iter()
                .map(|(off, near, far)| {
                    let inner = if near == W::Double {
                        // Inner corner: meet the near double's inner line.
                        cover(span(main_center, side * gap, light))
                    } else if opposite != W::None {
                        center
                    } else if far == W::Double {
                        // Outer corner: meet the far double's outer line.
                        cover(span(main_center, -side * gap, light))
                    } else if near.single() || far.single() {
                        cover(span(main_center, 0.0, perp_single))
                    } else {
                        center
                    };
                    piece(inner, span(cross_center, off, light))
                })
                .collect(),
        }
    }

    /// ┄ ┅ ┆ ┇ ┈ ┉ ┊ ┋ ╌ ╍ ╎ ╏
    fn dashed(mut self, c: char) {
        let (count, heavy, horizontal) = match c {
            '┄' => (3, false, true),
            '┅' => (3, true, true),
            '┆' => (3, false, false),
            '┇' => (3, true, false),
            '┈' => (4, false, true),
            '┉' => (4, true, true),
            '┊' => (4, false, false),
            '┋' => (4, true, false),
            '╌' => (2, false, true),
            '╍' => (2, true, true),
            '╎' => (2, false, false),
            _ => (2, true, false),
        };
        let t = if heavy {
            self.weights.heavy
        } else {
            self.weights.light
        };
        let cell = self.cell;
        let (cx, cy) = self.center();
        let (start, len) = if horizontal {
            (cell.x, cell.w)
        } else {
            (cell.y, cell.h)
        };
        // Equal dashes with half a gap at each end, so they repeat evenly
        // across cells.
        let step = len / count as f32;
        let gap = (step * 0.35).max(1.0);
        for i in 0..count {
            let a = (start + step * i as f32 + gap / 2.0).round();
            let b = (start + step * (i + 1) as f32 - gap / 2.0).round();
            if horizontal {
                let (y0, y1) = span(cy, 0.0, t);
                self.rect(a, y0, b, y1, 1.0);
            } else {
                let (x0, x1) = span(cx, 0.0, t);
                self.rect(x0, a, x1, b, 1.0);
            }
        }
    }

    /// ╭ ╮ ╯ ╰: a quarter circle joining the middles of two edges, with
    /// straight parts so the lines leave the cell where straight lines do.
    fn arc(self, c: char) {
        let (x0, x1, y0, y1) = self.edges();
        let light = self.weights.light;
        // Centers of the snapped straight lines this joins.
        let (sx, _) = span(self.center().0, 0.0, light);
        let (sy, _) = span(self.center().1, 0.0, light);
        let (cx, cy) = (sx + light / 2.0, sy + light / 2.0);
        // Directions (from the center) of the two edges the arc reaches.
        let (h, v) = match c {
            '╭' => (1.0, 1.0),
            '╮' => (-1.0, 1.0),
            '╯' => (-1.0, -1.0),
            _ => (1.0, -1.0),
        };
        let r = (self.cell.w.min(self.cell.h) / 2.0).max(1.0);
        // Cubic approximation of a quarter circle.
        let k = 0.552_284_8 * r;
        let h_edge = if h > 0.0 { x1 } else { x0 };
        let v_edge = if v > 0.0 { y1 } else { y0 };
        let a = (cx + h * r, cy);
        let b = (cx, cy + v * r);
        let line = |p: (f32, f32)| [p, p, p];
        self.out.push(Shape::Stroke {
            start: (h_edge, cy),
            segments: vec![
                line(a),
                [(a.0 - h * k, a.1), (b.0, b.1 - v * k), b],
                line((cx, v_edge)),
            ],
            width: light,
        });
    }

    /// Powerline arrows, half circles, corner triangles and slashes. Filled
    /// shapes cover the whole cell height and touch the snapped cell edges,
    /// so they meet the colored backgrounds next to them without a seam.
    fn powerline(self, c: char) {
        let (x0, x1, y0, y1) = self.edges();
        let cy = (y0 + y1) / 2.0;
        let width = self.weights.light;
        let line = |p: (f32, f32)| [p, p, p];
        let polygon = |points: &[(f32, f32)]| Shape::Fill {
            start: points[0],
            segments: points[1..].iter().map(|&p| line(p)).collect(),
        };
        let polyline = |points: &[(f32, f32)]| Shape::Stroke {
            start: points[0],
            segments: points[1..].iter().map(|&p| line(p)).collect(),
            width,
        };
        // Half an ellipse filling the cell, bulging right or left: from the
        // top corner of the flat side, through the middle of the far edge,
        // to the bottom corner.
        let half_circle = |right: bool| {
            let (flat, far) = if right { (x0, x1) } else { (x1, x0) };
            let (rx, ry) = (far - flat, (y1 - y0) / 2.0);
            let k = 0.552_284_8;
            vec![
                [(flat + rx * k, y0), (far, cy - ry * k), (far, cy)],
                [(far, cy + ry * k), (flat + rx * k, y1), (flat, y1)],
            ]
        };
        let shape = match c {
            // Arrows: solid and thin, right then left.
            '\u{E0B0}' => polygon(&[(x0, y0), (x1, cy), (x0, y1)]),
            '\u{E0B1}' => polyline(&[(x0, y0), (x1, cy), (x0, y1)]),
            '\u{E0B2}' => polygon(&[(x1, y0), (x0, cy), (x1, y1)]),
            '\u{E0B3}' => polyline(&[(x1, y0), (x0, cy), (x1, y1)]),
            // Half circles: solid and thin, bulging right then left.
            '\u{E0B4}' => Shape::Fill {
                start: (x0, y0),
                segments: half_circle(true),
            },
            '\u{E0B5}' => Shape::Stroke {
                start: (x0, y0),
                segments: half_circle(true),
                width,
            },
            '\u{E0B6}' => Shape::Fill {
                start: (x1, y0),
                segments: half_circle(false),
            },
            '\u{E0B7}' => Shape::Stroke {
                start: (x1, y0),
                segments: half_circle(false),
                width,
            },
            // Corner triangles: lower left, lower right, upper left, upper right.
            '\u{E0B8}' => polygon(&[(x0, y0), (x1, y1), (x0, y1)]),
            '\u{E0BA}' => polygon(&[(x1, y0), (x1, y1), (x0, y1)]),
            '\u{E0BC}' => polygon(&[(x0, y0), (x1, y0), (x0, y1)]),
            '\u{E0BE}' => polygon(&[(x0, y0), (x1, y0), (x1, y1)]),
            // Separators: a backslash for E0B9 and E0BF, a slash for E0BB and E0BD.
            '\u{E0B9}' | '\u{E0BF}' => polyline(&[(x0, y0), (x1, y1)]),
            _ => polyline(&[(x0, y1), (x1, y0)]),
        };
        self.out.push(shape);
    }

    /// ╱ ╲ ╳
    fn diagonal(self, c: char) {
        let (x0, x1, y0, y1) = self.edges();
        let width = self.weights.light;
        let line = |from: (f32, f32), to: (f32, f32)| Shape::Stroke {
            start: from,
            segments: vec![[to, to, to]],
            width,
        };
        if c != '╲' {
            self.out.push(line((x0, y1), (x1, y0)));
        }
        if c != '╱' {
            self.out.push(line((x0, y0), (x1, y1)));
        }
    }

    /// ▀ ▁…▇ █ ▉…▏ ▐ ░ ▒ ▓ ▔ ▕ and the quadrants ▖…▟.
    fn block(mut self, c: char) {
        let cell = self.cell;
        // Fractions of the cell, snapped.
        let x = |f: f32| (cell.x + cell.w * f).round();
        let y = |f: f32| (cell.y + cell.h * f).round();
        let mut part = |fx0: f32, fy0: f32, fx1: f32, fy1: f32, alpha: f32| {
            self.rect(x(fx0), y(fy0), x(fx1), y(fy1), alpha)
        };
        let code = c as u32;
        match c {
            '▀' => part(0.0, 0.0, 1.0, 0.5, 1.0),
            // ▁ to █: lower 1/8 to 8/8.
            '\u{2581}'..='\u{2588}' => {
                let eighths = (code - 0x2580) as f32;
                part(0.0, 1.0 - eighths / 8.0, 1.0, 1.0, 1.0)
            }
            // ▉ to ▏: left 7/8 to 1/8.
            '\u{2589}'..='\u{258F}' => {
                let eighths = (0x2590 - code) as f32;
                part(0.0, 0.0, eighths / 8.0, 1.0, 1.0)
            }
            '▐' => part(0.5, 0.0, 1.0, 1.0, 1.0),
            '░' => part(0.0, 0.0, 1.0, 1.0, 0.25),
            '▒' => part(0.0, 0.0, 1.0, 1.0, 0.5),
            '▓' => part(0.0, 0.0, 1.0, 1.0, 0.75),
            '▔' => part(0.0, 0.0, 1.0, 0.125, 1.0),
            '▕' => part(0.875, 0.0, 1.0, 1.0, 1.0),
            _ => {
                // Quadrants: upper left, upper right, lower left, lower right.
                let [ul, ur, ll, lr] = match c {
                    '▖' => [false, false, true, false],
                    '▗' => [false, false, false, true],
                    '▘' => [true, false, false, false],
                    '▙' => [true, false, true, true],
                    '▚' => [true, false, false, true],
                    '▛' => [true, true, true, false],
                    '▜' => [true, true, false, true],
                    '▝' => [false, true, false, false],
                    '▞' => [false, true, true, false],
                    _ => [false, true, true, true], // ▟
                };
                for (on, fx, fy) in [
                    (ul, 0.0, 0.0),
                    (ur, 0.5, 0.0),
                    (ll, 0.0, 0.5),
                    (lr, 0.5, 0.5),
                ] {
                    if on {
                        part(fx, fy, fx + 0.5, fy + 0.5, 1.0);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CELL: Cell = Cell {
        x: 80.0,
        y: 40.0,
        w: 9.0,
        h: 20.0,
    };
    const WEIGHTS: Weights = Weights {
        light: 1.0,
        heavy: 2.0,
    };

    fn rects(c: char) -> Vec<(f32, f32, f32, f32)> {
        shapes(c, CELL, WEIGHTS)
            .unwrap()
            .into_iter()
            .map(|s| match s {
                Shape::Rect { x0, y0, x1, y1, .. } => (x0, y0, x1, y1),
                other => panic!("{c:?} drew {other:?}"),
            })
            .collect()
    }

    /// Whether the pixel at (x, y) (its top-left corner) is covered.
    fn covered(r: &[(f32, f32, f32, f32)], x: f32, y: f32) -> bool {
        r.iter()
            .any(|&(x0, y0, x1, y1)| x >= x0 && x < x1 && y >= y0 && y < y1)
    }

    #[test]
    fn every_character_in_the_ranges_is_drawn() {
        for c in '\u{2500}'..='\u{259F}' {
            assert!(supported(c));
            let s = shapes(c, CELL, WEIGHTS).unwrap_or_else(|| panic!("{c:?} not drawn"));
            assert!(!s.is_empty(), "{c:?} drew nothing");
        }
        for c in '\u{E0B0}'..='\u{E0BF}' {
            assert!(supported(c));
            assert_eq!(shapes(c, CELL, WEIGHTS).unwrap().len(), 1, "{c:?}");
        }
        assert!(!supported('A') && !supported('\u{25A0}') && !supported('\u{E0C0}'));
    }

    #[test]
    fn lines_reach_the_cell_edges() {
        // Horizontal lines span the snapped cell, so neighbors join.
        let h = rects('─');
        let left = h.iter().map(|r| r.0).fold(f32::MAX, f32::min);
        let right = h.iter().map(|r| r.2).fold(f32::MIN, f32::max);
        assert_eq!((left, right), (80.0, 89.0));
        // Vertical lines span the full line height, not the font's.
        let v = rects('│');
        let top = v.iter().map(|r| r.1).fold(f32::MAX, f32::min);
        let bottom = v.iter().map(|r| r.3).fold(f32::MIN, f32::max);
        assert_eq!((top, bottom), (40.0, 60.0));
    }

    #[test]
    fn lines_are_continuous_through_the_center() {
        for c in ['─', '┼', '┬', '━', '╋', '┿'] {
            let r = rects(c);
            let y = r.iter().find(|r| r.0 == 80.0).unwrap().1;
            for x in 80..89 {
                assert!(covered(&r, x as f32, y), "{c:?} has a hole at x={x}");
            }
        }
    }

    #[test]
    fn corners_are_closed() {
        // ┌: the horizontal and vertical lines share the corner pixel.
        let r = rects('┌');
        let (vx, _) = span(84.5, 0.0, 1.0);
        let (hy, _) = span(50.0, 0.0, 1.0);
        assert!(covered(&r, vx, hy));
        assert!(!covered(&r, vx - 1.0, hy), "┌ pokes out to the left");
        assert!(!covered(&r, vx, hy - 1.0), "┌ pokes out above");
    }

    #[test]
    fn double_corner_has_two_closed_corners() {
        let r = rects('╔');
        // Outer line pixels: x = 84.5 - 1, y = 50 - 1; inner: +1.
        let (outer_x, _) = span(84.5, -1.0, 1.0);
        let (inner_x, _) = span(84.5, 1.0, 1.0);
        let (outer_y, _) = span(50.0, -1.0, 1.0);
        let (inner_y, _) = span(50.0, 1.0, 1.0);
        assert!(covered(&r, outer_x, outer_y), "outer corner open");
        assert!(covered(&r, inner_x, inner_y), "inner corner open");
        // Nothing between the two lines, inside the corner.
        assert!(!covered(&r, inner_x - 1.0, inner_y + 2.0));
        assert!(!covered(&r, inner_x + 2.0, inner_y - 1.0));
        // Nothing outside the outer corner.
        assert!(!covered(&r, outer_x - 1.0, outer_y));
    }

    #[test]
    fn double_lines_stay_parallel_through_a_cross() {
        // ╬: the gap between the two horizontal lines is never filled.
        let r = rects('╬');
        let (gap_y, _) = span(50.0, 0.0, 1.0);
        for x in 80..89 {
            assert!(!covered(&r, x as f32, gap_y), "╬ gap filled at x={x}");
        }
    }

    #[test]
    fn blocks_fill_their_fraction() {
        assert_eq!(rects('█'), vec![(80.0, 40.0, 89.0, 60.0)]);
        assert_eq!(rects('▄'), vec![(80.0, 50.0, 89.0, 60.0)]);
        assert_eq!(rects('▌'), vec![(80.0, 40.0, 85.0, 60.0)]);
        assert_eq!(rects('▚').len(), 2);
        let Some(Shape::Rect { alpha, .. }) = shapes('▒', CELL, WEIGHTS).unwrap().pop() else {
            panic!()
        };
        assert_eq!(alpha, 0.5);
    }

    #[test]
    fn arcs_and_diagonals_are_strokes() {
        for c in ['╭', '╮', '╯', '╰', '╱', '╲'] {
            let s = shapes(c, CELL, WEIGHTS).unwrap();
            assert!(matches!(s.as_slice(), [Shape::Stroke { .. }]), "{c:?}");
        }
        // ╭ leaves through the right and bottom edges.
        let Shape::Stroke {
            start, segments, ..
        } = &shapes('╭', CELL, WEIGHTS).unwrap()[0]
        else {
            panic!()
        };
        assert_eq!(start.0, 89.0);
        assert_eq!(segments.last().unwrap()[2].1, 60.0);
    }

    #[test]
    fn powerline_arrows_fill_the_cell_height() {
        // The solid right arrow: the flat side spans the whole cell, the point
        // is at the middle of the right edge, so it meets the next cell's
        // background.
        let Shape::Fill { start, segments } = &shapes('\u{E0B0}', CELL, WEIGHTS).unwrap()[0] else {
            panic!()
        };
        assert_eq!(*start, (80.0, 40.0));
        assert_eq!(segments[0][2], (89.0, 50.0));
        assert_eq!(segments[1][2], (80.0, 60.0));
        // The left arrow mirrors it.
        let Shape::Fill { start, segments } = &shapes('\u{E0B2}', CELL, WEIGHTS).unwrap()[0] else {
            panic!()
        };
        assert_eq!(*start, (89.0, 40.0));
        assert_eq!(segments[0][2], (80.0, 50.0));
        // The thin arrows and half circles are outlines.
        for c in ['\u{E0B1}', '\u{E0B3}', '\u{E0B5}', '\u{E0B7}'] {
            assert!(matches!(
                shapes(c, CELL, WEIGHTS).unwrap()[0],
                Shape::Stroke { .. }
            ));
        }
    }

    #[test]
    fn powerline_half_circles_reach_the_far_edge() {
        // Bulging right: from the top-left corner through the middle of the
        // right edge to the bottom-left corner.
        let Shape::Fill { start, segments } = &shapes('\u{E0B4}', CELL, WEIGHTS).unwrap()[0] else {
            panic!()
        };
        assert_eq!(*start, (80.0, 40.0));
        assert_eq!(segments[0][2], (89.0, 50.0));
        assert_eq!(segments[1][2], (80.0, 60.0));
        // Bulging left.
        let Shape::Fill { start, segments } = &shapes('\u{E0B6}', CELL, WEIGHTS).unwrap()[0] else {
            panic!()
        };
        assert_eq!(*start, (89.0, 40.0));
        assert_eq!(segments[0][2], (80.0, 50.0));
    }
}
