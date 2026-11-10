//! Draws kitty graphics placements at their marker cells (see
//! `lohtty_term::kitty` for how markers anchor images to the grid).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{Bounds, ContentMask, Pixels, RenderImage, Window, point, px, size};
use lohtty_term::kitty::{Graphics, Image, parse_marker};

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
}

impl ImageLayer {
    pub fn new(graphics: Graphics) -> Self {
        Self {
            graphics,
            cache: Rc::default(),
        }
    }

    /// Lays out the placements whose markers are visible.
    pub fn layout(
        &self,
        markers: Vec<Marker>,
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

        // Release textures that weren't used this frame.
        let mut released = Vec::new();
        cache.entries.retain(|_, (texture, used)| {
            let keep = *used == frame_no;
            if !keep {
                released.push(texture.clone());
            }
            keep
        });
        ImageFrame { draws, released }
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
