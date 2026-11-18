//! Draws kitty graphics placements at their marker cells (see
//! `lohtty_term::kitty` for how markers anchor images to the grid), and
//! virtual placements in their Unicode placeholder cells.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use gpui::{Bounds, ContentMask, Pixels, RenderImage, Window, point, px, size};
use lohtty_term::alacritty_terminal::term::cell::Cell;
use lohtty_term::alacritty_terminal::vte::ansi::Color;
use lohtty_term::kitty::placeholder::{self, PlaceholderCell};
use lohtty_term::kitty::{Graphics, Image, Placement, parse_marker};

pub(super) use lohtty_term::kitty::placeholder::PLACEHOLDER;

use super::CellMetrics;

/// A marker seen while walking the grid: (placement, image row) at a cell.
pub(super) struct Marker {
    placement: u64,
    image_row: u32,
    row: usize,
    col: usize,
}

/// Returns the marker carried by a cell's hyperlink, if any.
pub(super) fn marker(uri: &str, row: usize, col: usize) -> Option<Marker> {
    let (placement, image_row) = parse_marker(uri)?;
    Some(Marker {
        placement,
        image_row,
        row,
        col,
    })
}

/// A Unicode placeholder cell on screen, decoded.
#[derive(Clone, Copy)]
pub(super) struct PlaceholderAt {
    pub row: usize,
    pub col: usize,
    pub cell: PlaceholderCell,
}

/// Decodes a placeholder cell at (row, col); `left` is the placeholder just
/// left of it, for inherited values.
pub(super) fn placeholder(
    cell: &Cell,
    row: usize,
    col: usize,
    left: Option<PlaceholderCell>,
) -> PlaceholderAt {
    // Without shell integration marks, which aren't placeholder diacritics.
    let all = cell.zerowidth().unwrap_or(&[]);
    let kept: Vec<char>;
    let marks = if all.iter().any(|&c| lohtty_term::prompt::is_mark(c)) {
        kept = all
            .iter()
            .copied()
            .filter(|&c| !lohtty_term::prompt::is_mark(c))
            .collect();
        &kept[..]
    } else {
        all
    };
    let underline = cell.underline_color().map_or(0, color_id);
    PlaceholderAt {
        row,
        col,
        cell: placeholder::decode(color_id(cell.fg), underline, marks, left),
    }
}

/// The id a placeholder color encodes: the 24-bit value, or the 256-color
/// index (the first 16 may come as named colors).
fn color_id(color: Color) -> u32 {
    match color {
        Color::Spec(rgb) => (rgb.r as u32) << 16 | (rgb.g as u32) << 8 | rgb.b as u32,
        Color::Indexed(i) => i as u32,
        Color::Named(named) => {
            let i = named as u32;
            if i < 16 { i } else { 0 }
        }
    }
}

/// Images converted for gpui, kept while they stay on screen.
#[derive(Default)]
pub(super) struct ImageCache {
    /// Textures by image serial, with the frame that last used them.
    entries: HashMap<u64, (Arc<RenderImage>, u64)>,
    frame: u64,
}

/// The graphics store of a session, plus its texture cache.
#[derive(Clone)]
pub(super) struct ImageLayer {
    pub graphics: Graphics,
    pub cache: Rc<RefCell<ImageCache>>,
    /// When the repaint already scheduled for an animation is due.
    pub animation_wake: Rc<std::cell::Cell<Option<Instant>>>,
}

/// One image to draw this frame.
pub(super) struct ImageDraw {
    image: Arc<RenderImage>,
    bounds: Bounds<Pixels>,
    clip: Bounds<Pixels>,
    pub below_text: bool,
}

/// What the frame paints: images, and textures to release.
#[derive(Default)]
pub(super) struct ImageFrame {
    pub draws: Vec<ImageDraw>,
    released: Vec<Arc<RenderImage>>,
    /// When an animation drawn in this frame shows its next frame.
    pub next_animation_frame: Option<Instant>,
}

impl ImageLayer {
    pub fn new(graphics: Graphics) -> Self {
        Self {
            graphics,
            cache: Rc::default(),
            animation_wake: Rc::default(),
        }
    }

    /// Lays out the placements whose markers are visible.
    pub fn layout(
        &self,
        markers: Vec<Marker>,
        placeholders: Vec<PlaceholderAt>,
        bounds: Bounds<Pixels>,
        m: CellMetrics,
    ) -> ImageFrame {
        // Image cells are measured in the size reported to the client.
        self.graphics.set_cell_size(
            f32::from(m.cell_width).round() as u32,
            f32::from(m.line_height).round() as u32,
        );
        let mut cache = self.cache.borrow_mut();
        cache.frame += 1;
        let frame_no = cache.frame;

        // Visible rows of each placement: (first marker, last visible row).
        let mut visible: HashMap<u64, (Marker, usize)> = HashMap::new();
        for mk in markers {
            match visible.get_mut(&mk.placement) {
                Some((first, last)) => {
                    *last = (*last).max(mk.row);
                    if mk.row < first.row {
                        *first = mk;
                    }
                }
                None => {
                    let row = mk.row;
                    visible.insert(mk.placement, (mk, row));
                }
            }
        }

        let mut draws = Vec::new();
        for (placement, (first, last_row)) in visible {
            let Some((p, image)) = self.graphics.lookup(placement) else {
                continue;
            };
            if p.src_w == 0 || p.src_h == 0 {
                continue;
            }
            let Some(texture) = cache.texture(&image, frame_no) else {
                continue;
            };
            let cell = |col: f32, row: f32| {
                point(
                    bounds.origin.x + m.cell_width * col,
                    bounds.origin.y + m.line_height * row,
                )
            };
            let top = first.row as f32 - first.image_row as f32;
            let origin =
                cell(first.col as f32, top) + point(px(p.offset_x as f32), px(p.offset_y as f32));
            let dest = if p.fit_cells {
                size(
                    m.cell_width * p.cols as f32 - px(p.offset_x as f32),
                    m.line_height * p.rows as f32 - px(p.offset_y as f32),
                )
            } else {
                size(px(p.src_w as f32), px(p.src_h as f32))
            };
            let dest = Bounds::new(origin, dest);
            // The whole image is drawn, scaled and shifted so the source
            // rectangle lands on `dest`, and clipped to it. Sampling then
            // continues across the rectangle's edges, so an image cut into
            // tiles (yazi places one per cell) shows no seams.
            let scale_x = f32::from(dest.size.width) / p.src_w as f32;
            let scale_y = f32::from(dest.size.height) / p.src_h as f32;
            let whole = Bounds::new(
                point(
                    dest.origin.x - px(p.src_x as f32 * scale_x),
                    dest.origin.y - px(p.src_y as f32 * scale_y),
                ),
                size(
                    px(image.width as f32 * scale_x),
                    px(image.height as f32 * scale_y),
                ),
            );
            // Only the rows whose markers are on screen are shown.
            let clip_top = cell(0.0, first.row as f32).y;
            let clip_bottom = cell(0.0, last_row as f32 + 1.0).y;
            let clip = Bounds::from_corners(
                point(dest.origin.x, clip_top.max(dest.origin.y)),
                point(
                    dest.origin.x + dest.size.width,
                    clip_bottom.min(dest.origin.y + dest.size.height),
                ),
            )
            .intersect(&bounds);
            draws.push(ImageDraw {
                image: texture,
                bounds: whole,
                clip,
                below_text: p.z < 0,
            });
        }

        // Placeholder cells: runs of consecutive cells of one placement on a
        // row are drawn together.
        let mut runs: Vec<(PlaceholderAt, usize)> = Vec::new();
        for at in placeholders {
            match runs.last_mut() {
                Some((first, len))
                    if first.row == at.row
                        && first.col + *len == at.col
                        && first.cell.image_id == at.cell.image_id
                        && first.cell.placement_id == at.cell.placement_id
                        && first.cell.row == at.cell.row
                        && first.cell.col + *len as u32 == at.cell.col =>
                {
                    *len += 1
                }
                _ => runs.push((at, 1)),
            }
        }
        let mut virtuals: HashMap<(u32, u32), Option<(Placement, Image)>> = HashMap::new();
        for (first, len) in runs {
            let key = (first.cell.image_id, first.cell.placement_id);
            let found = virtuals
                .entry(key)
                .or_insert_with(|| self.graphics.virtual_placement(key.0, key.1));
            let Some((p, image)) = found else {
                continue;
            };
            if p.src_w == 0 || p.src_h == 0 {
                continue;
            }
            let Some(texture) = cache.texture(image, frame_no) else {
                continue;
            };
            // The placement's grid of cells, placed so that the run's first
            // cell is the image cell it names.
            let grid_origin = point(
                bounds.origin.x + m.cell_width * (first.col as f32 - first.cell.col as f32),
                bounds.origin.y + m.line_height * (first.row as f32 - first.cell.row as f32),
            );
            let grid = size(m.cell_width * p.cols as f32, m.line_height * p.rows as f32);
            // The source fits the grid, keeping its aspect ratio, centered.
            let scale = (f32::from(grid.width) / p.src_w as f32)
                .min(f32::from(grid.height) / p.src_h as f32);
            let shown = size(px(p.src_w as f32 * scale), px(p.src_h as f32 * scale));
            let dest_origin = point(
                grid_origin.x + (grid.width - shown.width) / 2.,
                grid_origin.y + (grid.height - shown.height) / 2.,
            );
            let whole = Bounds::new(
                point(
                    dest_origin.x - px(p.src_x as f32 * scale),
                    dest_origin.y - px(p.src_y as f32 * scale),
                ),
                size(
                    px(image.width as f32 * scale),
                    px(image.height as f32 * scale),
                ),
            );
            let run = Bounds::new(
                point(
                    bounds.origin.x + m.cell_width * first.col as f32,
                    bounds.origin.y + m.line_height * first.row as f32,
                ),
                size(m.cell_width * len as f32, m.line_height),
            );
            let clip = run
                .intersect(&Bounds::new(dest_origin, shown))
                .intersect(&bounds);
            draws.push(ImageDraw {
                image: texture,
                bounds: whole,
                clip,
                below_text: p.z < 0,
            });
        }

        // Release textures that weren't used this frame.
        let mut released = Vec::new();
        cache.entries.retain(|_, (texture, used)| {
            let keep = *used == frame_no;
            if !keep {
                released.push(texture.clone());
            }
            keep
        });
        ImageFrame {
            draws,
            released,
            next_animation_frame: self.graphics.take_next_frame(),
        }
    }
}

impl ImageCache {
    /// The gpui texture of an image, converted once and shared by all its
    /// placements.
    fn texture(&mut self, image: &Image, frame: u64) -> Option<Arc<RenderImage>> {
        if let Some((texture, used)) = self.entries.get_mut(&image.serial) {
            *used = frame;
            return Some(texture.clone());
        }
        // Convert RGBA to gpui's BGRA.
        let bgra: Vec<u8> = image
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|px| [px[2], px[1], px[0], px[3]])
            .collect();
        let buffer = image::RgbaImage::from_raw(image.width, image.height, bgra)?;
        let texture = Arc::new(RenderImage::new([image::Frame::new(buffer)]));
        self.entries.insert(image.serial, (texture.clone(), frame));
        Some(texture)
    }
}

impl ImageFrame {
    pub fn paint(&self, below_text: bool, window: &mut Window) {
        for d in self.draws.iter().filter(|d| d.below_text == below_text) {
            window.with_content_mask(Some(ContentMask { bounds: d.clip }), |window| {
                let _ = window.paint_image(d.bounds, Default::default(), d.image.clone(), 0, false);
            });
        }
    }

    /// Frees the GPU copies of textures that left the screen.
    pub fn release(self, window: &mut Window) {
        for texture in self.released {
            let _ = window.drop_image(texture);
        }
    }
}
