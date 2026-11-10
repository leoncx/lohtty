//! Draws kitty graphics placements at their marker cells (see
//! `lohtty_term::kitty` for how markers anchor images to the grid).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{Bounds, ContentMask, Pixels, RenderImage, Window, point, px, size};
use lohtty_term::kitty::{Graphics, Image, Placement, parse_marker};

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

/// (stored image, source rectangle): one converted texture each.
type Key = (u64, [u32; 4]);

/// Images converted for gpui, kept while they stay on screen.
#[derive(Default)]
pub(super) struct ImageCache {
    entries: HashMap<Key, (Arc<RenderImage>, u64)>,
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
            let Some(texture) = cache.texture(&image, &p, frame_no) else {
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
            // Only the rows whose markers are on screen are shown.
            let clip_top = cell(0.0, first.row as f32).y;
            let clip_bottom = cell(0.0, last_row as f32 + 1.0).y;
            let clip = Bounds::from_corners(
                point(dest.origin.x, clip_top),
                point(dest.origin.x + dest.size.width, clip_bottom),
            )
            .intersect(&bounds);
            draws.push(ImageDraw {
                image: texture,
                bounds: dest,
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
    /// The gpui texture for a placement's source rectangle, converted once.
    fn texture(&mut self, image: &Image, p: &Placement, frame: u64) -> Option<Arc<RenderImage>> {
        let key = (image.serial, [p.src_x, p.src_y, p.src_w, p.src_h]);
        if let Some((texture, used)) = self.entries.get_mut(&key) {
            *used = frame;
            return Some(texture.clone());
        }
        if p.src_w == 0 || p.src_h == 0 {
            return None;
        }
        // Crop to the source rectangle and convert RGBA to gpui's BGRA.
        let stride = image.width as usize * 4;
        let mut bgra = Vec::with_capacity(p.src_w as usize * p.src_h as usize * 4);
        for y in p.src_y..p.src_y + p.src_h {
            let start = y as usize * stride + p.src_x as usize * 4;
            let row = image.rgba.get(start..start + p.src_w as usize * 4)?;
            for px in row.as_chunks::<4>().0 {
                bgra.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
            }
        }
        let buffer = image::RgbaImage::from_raw(p.src_w, p.src_h, bgra)?;
        let texture = Arc::new(RenderImage::new([image::Frame::new(buffer)]));
        self.entries.insert(key, (texture.clone(), frame));
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
