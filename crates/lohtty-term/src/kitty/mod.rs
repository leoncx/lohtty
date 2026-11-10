//! The kitty graphics protocol (`ESC _ G <control> ; <payload> ESC \`).
//!
//! alacritty's parser drops APC sequences and has no notion of image cells,
//! so [`KittyFilter`] takes graphics commands out of the PTY stream before
//! the parser sees them. Images are kept in [`Graphics`], shared with the UI.
//!
//! **Anchoring.** When an image is displayed, the filter injects one marker
//! cell per image row into the stream it hands to the parser: a blank cell
//! carrying an OSC 8 hyperlink `lohtty-kitty:<placement>:<row>`, written at
//! the image's left column. The parser puts the markers wherever its cursor
//! really is, so they scroll, get cleared and leave scrollback together with
//! the text, and writing them moves the cursor past the image as the protocol
//! requires. The renderer draws a placement wherever its markers are visible.
//! The cost: the cell under each row's first column is overwritten with a
//! blank, which is usually empty anyway (programs print images on fresh lines).

mod command;
mod decode;

use std::collections::{HashMap, VecDeque};
use std::io::Write as _;
use std::sync::{Arc, Mutex};

use crate::filter::{OutputFilter, Screen};
use command::Command;

/// URI scheme of the marker hyperlinks (see the module docs).
pub const MARKER_SCHEME: &str = "lohtty-kitty:";

/// Total decoded image data kept, like kitty's default quota. The oldest
/// images are dropped first.
const QUOTA_BYTES: usize = 320 * 1024 * 1024;
/// Largest single command (control data plus base64 payload) accepted.
const MAX_COMMAND_BYTES: usize = 400 * 1024 * 1024;
/// Largest decoded image, in pixels (10000 × 10000).
const MAX_PIXELS: u64 = 100_000_000;

/// A decoded image, RGBA 8 bits per channel.
#[derive(Clone)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Arc<[u8]>,
    /// Unique per stored image, so caches can tell a re-sent image apart.
    pub serial: u64,
}

/// Where and how an image is shown. The position comes from the markers.
#[derive(Clone, Debug, PartialEq)]
pub struct Placement {
    pub image_id: u32,
    /// Cells covered.
    pub cols: u32,
    pub rows: u32,
    /// Source rectangle in image pixels.
    pub src_x: u32,
    pub src_y: u32,
    pub src_w: u32,
    pub src_h: u32,
    /// Offset from the top-left cell, in pixels.
    pub offset_x: u32,
    pub offset_y: u32,
    /// Stretch the source to exactly `cols` × `rows` cells (set when the
    /// client gave `c` or `r`); otherwise draw it at its pixel size.
    pub fit_cells: bool,
    /// Drawn below text when negative.
    pub z: i32,
}

#[derive(Default)]
struct State {
    images: HashMap<u32, Image>,
    /// Insertion order, for eviction.
    order: VecDeque<u32>,
    bytes: usize,
    /// Image number (`I`) → the id it was given.
    numbers: HashMap<u32, u32>,
    placements: HashMap<u64, Placement>,
    /// (image id, placement id) → placement serial, for replacement.
    keyed: HashMap<(u32, u32), u64>,
    next_serial: u64,
    next_image_id: u32,
    /// Cell size in pixels, as reported to the client.
    cell: (u32, u32),
}

/// Image store shared by the PTY reader (through [`KittyFilter`]) and the UI.
#[derive(Clone)]
pub struct Graphics {
    state: Arc<Mutex<State>>,
}

impl Default for Graphics {
    fn default() -> Self {
        Self::new()
    }
}

impl Graphics {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(State {
                cell: (8, 16),
                // Ids handed out for `I` or id-less images count down from
                // the top, away from the small ids clients pick themselves.
                next_image_id: u32::MAX,
                ..State::default()
            })),
        }
    }

    /// The filter to install on the session.
    pub fn filter(&self) -> KittyFilter {
        KittyFilter {
            graphics: self.clone(),
            scan: Scan::Normal,
            apc: Vec::new(),
            pending: None,
            deferred: VecDeque::new(),
        }
    }

    /// Cell size in pixels, used to turn image sizes into rows and columns.
    pub fn set_cell_size(&self, width: u32, height: u32) {
        self.state.lock().unwrap().cell = (width.max(1), height.max(1));
    }

    /// The placement a marker refers to, with its image.
    pub fn lookup(&self, placement: u64) -> Option<(Placement, Image)> {
        let state = self.state.lock().unwrap();
        let p = state.placements.get(&placement)?;
        let image = state.images.get(&p.image_id)?;
        Some((p.clone(), image.clone()))
    }

    pub fn is_empty(&self) -> bool {
        self.state.lock().unwrap().placements.is_empty()
    }
}

/// Parses a marker hyperlink URI into (placement serial, image row).
pub fn parse_marker(uri: &str) -> Option<(u64, u32)> {
    let (serial, row) = uri.strip_prefix(MARKER_SCHEME)?.split_once(':')?;
    Some((serial.parse().ok()?, row.parse().ok()?))
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Scan {
    Normal,
    /// Saw ESC.
    Esc,
    /// Saw ESC _.
    EscUnderscore,
    /// Inside a graphics command.
    Apc,
    /// Saw ESC inside a graphics command (ST expected).
    ApcEsc,
    /// Inside a command that is too large; skipped up to its end.
    Discard,
    DiscardEsc,
}

/// A chunked transmission in progress (`m=1`).
struct Pending {
    cmd: Command,
    payload: Vec<u8>,
}

/// Takes kitty graphics commands out of the PTY output (see the module docs).
pub struct KittyFilter {
    graphics: Graphics,
    scan: Scan,
    apc: Vec<u8>,
    pending: Option<Pending>,
    /// Deletes by position, waiting for the screen at their place in the
    /// output: (offset in the last `out`, command).
    deferred: VecDeque<(usize, Command)>,
}

const ESC: u8 = 0x1b;

impl OutputFilter for KittyFilter {
    fn filter(&mut self, input: &[u8], out: &mut Vec<u8>, pty_reply: &mut Vec<u8>) {
        // Deletes not handed to the reader (it always takes them) are stale.
        self.deferred.clear();
        let mut i = 0;
        while i < input.len() {
            let b = input[i];
            match self.scan {
                Scan::Normal => {
                    // Copy up to the next ESC in one go.
                    match input[i..].iter().position(|&c| c == ESC) {
                        Some(n) => {
                            out.extend_from_slice(&input[i..i + n]);
                            self.scan = Scan::Esc;
                            i += n + 1;
                        }
                        None => {
                            out.extend_from_slice(&input[i..]);
                            i = input.len();
                        }
                    }
                    continue;
                }
                Scan::Esc => {
                    if b == b'_' {
                        self.scan = Scan::EscUnderscore;
                    } else {
                        out.push(ESC);
                        self.scan = Scan::Normal;
                        continue; // reprocess `b`
                    }
                }
                Scan::EscUnderscore => {
                    if b == b'G' {
                        self.scan = Scan::Apc;
                        self.apc.clear();
                    } else {
                        out.extend_from_slice(&[ESC, b'_']);
                        self.scan = Scan::Normal;
                        continue;
                    }
                }
                Scan::Apc => {
                    // Copy up to the next ESC in one go.
                    let n = input[i..]
                        .iter()
                        .position(|&c| c == ESC)
                        .unwrap_or(input.len() - i);
                    self.apc.extend_from_slice(&input[i..i + n]);
                    i += n;
                    if self.apc.len() > MAX_COMMAND_BYTES {
                        tracing::warn!("kitty graphics command too large, dropped");
                        self.apc = Vec::new();
                        self.pending = None;
                        self.scan = Scan::Discard;
                    } else if i < input.len() {
                        self.scan = Scan::ApcEsc;
                        i += 1;
                    }
                    continue;
                }
                Scan::ApcEsc => {
                    if b == b'\\' {
                        let apc = std::mem::take(&mut self.apc);
                        self.handle(&apc, out, pty_reply);
                        self.apc = apc;
                        self.apc.clear();
                        self.scan = Scan::Normal;
                    } else {
                        self.apc.push(ESC);
                        self.scan = Scan::Apc;
                        continue;
                    }
                }
                Scan::Discard => {
                    if b == ESC {
                        self.scan = Scan::DiscardEsc;
                    }
                }
                Scan::DiscardEsc => {
                    self.scan = if b == b'\\' {
                        Scan::Normal
                    } else {
                        Scan::Discard
                    };
                }
            }
            i += 1;
        }
    }

    fn take_checkpoints(&mut self) -> Vec<usize> {
        self.deferred.iter().map(|(at, _)| *at).collect()
    }

    fn at_checkpoint(&mut self, screen: &dyn Screen) {
        if let Some((_, cmd)) = self.deferred.pop_front() {
            self.delete_at(&cmd, screen);
        }
    }
}

impl KittyFilter {
    fn handle(&mut self, apc: &[u8], out: &mut Vec<u8>, reply: &mut Vec<u8>) {
        let (control, payload) = match apc.iter().position(|&c| c == b';') {
            Some(n) => (&apc[..n], &apc[n + 1..]),
            None => (apc, &[][..]),
        };
        let cmd = Command::parse(control);

        // Continuation of a chunked transmission: only `m` (and `q`) matter.
        if let Some(pending) = self.pending.as_mut() {
            pending.payload.extend_from_slice(payload);
            if cmd.more {
                return;
            }
            let Pending {
                cmd: first,
                payload,
            } = self.pending.take().unwrap();
            self.finish(first, &payload, out, reply);
            return;
        }
        if cmd.more {
            self.pending = Some(Pending {
                cmd,
                payload: payload.to_vec(),
            });
            return;
        }
        self.finish(cmd, payload, out, reply);
    }

    fn finish(&mut self, cmd: Command, payload: &[u8], out: &mut Vec<u8>, reply: &mut Vec<u8>) {
        let result = match cmd.action {
            b'q' => decode::decode(&cmd, payload).map(|_| None),
            b't' | b'T' => decode::decode(&cmd, payload).map(|image| {
                let id = self.store(&cmd, image);
                if cmd.action == b'T' {
                    self.place(&cmd, id, out);
                }
                Some(id)
            }),
            b'p' => self.resolve(&cmd).map(|id| {
                self.place(&cmd, id, out);
                Some(id)
            }),
            b'd' => {
                if matches!(
                    cmd.delete.to_ascii_lowercase(),
                    b'c' | b'p' | b'q' | b'x' | b'y'
                ) {
                    // Needs the screen as it is at this point of the output.
                    self.deferred.push_back((out.len(), cmd));
                } else {
                    self.delete(&cmd);
                }
                return;
            }
            other => Err(format!(
                "EINVAL:unsupported action {}",
                char::from(other).escape_default()
            )),
        };
        if let Err(e) = &result {
            tracing::debug!(error = %e, "kitty graphics command failed");
        }
        respond(&cmd, result, reply);
    }

    /// Stores a decoded image under the client's id, a fresh id for an image
    /// number, or a fresh id when neither is given.
    fn store(&mut self, cmd: &Command, image: decode::Decoded) -> u32 {
        let mut state = self.graphics.state.lock().unwrap();
        let id = if cmd.id != 0 {
            cmd.id
        } else {
            let id = state.next_image_id;
            state.next_image_id -= 1;
            if cmd.number != 0 {
                state.numbers.insert(cmd.number, id);
            }
            id
        };
        // Re-sending an id replaces the image and drops its placements.
        remove_image(&mut state, id);
        state.next_serial += 1;
        let size = image.rgba.len();
        let stored = Image {
            width: image.width,
            height: image.height,
            rgba: image.rgba.into(),
            serial: state.next_serial,
        };
        state.images.insert(id, stored);
        state.order.push_back(id);
        state.bytes += size;
        while state.bytes > QUOTA_BYTES {
            let Some(oldest) = state.order.front().copied() else {
                break;
            };
            if oldest == id {
                break;
            }
            remove_image(&mut state, oldest);
        }
        id
    }

    /// The image a command refers to (`i`, or `I` for the newest image with
    /// that number).
    fn resolve(&self, cmd: &Command) -> Result<u32, String> {
        let state = self.graphics.state.lock().unwrap();
        let id = if cmd.id != 0 {
            Some(cmd.id)
        } else {
            state.numbers.get(&cmd.number).copied()
        };
        id.filter(|id| state.images.contains_key(id))
            .ok_or_else(|| "ENOENT:image not found".to_owned())
    }

    /// Creates a placement and writes its row markers into `out`.
    fn place(&mut self, cmd: &Command, image_id: u32, out: &mut Vec<u8>) {
        if cmd.virtual_placement {
            // Unicode placeholders aren't supported; nothing is drawn.
            return;
        }
        let mut state = self.graphics.state.lock().unwrap();
        let Some(image) = state.images.get(&image_id) else {
            return;
        };
        let (iw, ih) = (image.width, image.height);
        let src_x = cmd.x.min(iw);
        let src_y = cmd.y.min(ih);
        let src_w = if cmd.w == 0 {
            iw - src_x
        } else {
            cmd.w.min(iw - src_x)
        };
        let src_h = if cmd.h == 0 {
            ih - src_y
        } else {
            cmd.h.min(ih - src_y)
        };
        let (cw, ch) = state.cell;
        let (offset_x, offset_y) = (cmd.offset_x.min(cw - 1), cmd.offset_y.min(ch - 1));
        // Display size in pixels: the source size unless cells are given, in
        // which case a single given dimension keeps the aspect ratio.
        let (cols, rows) = match (cmd.cols, cmd.rows) {
            (0, 0) => (
                (src_w + offset_x).div_ceil(cw),
                (src_h + offset_y).div_ceil(ch),
            ),
            (c, 0) => {
                let height = (c * cw) as u64 * src_h as u64 / src_w.max(1) as u64;
                (c, (height as u32).div_ceil(ch))
            }
            (0, r) => {
                let width = (r * ch) as u64 * src_w as u64 / src_h.max(1) as u64;
                ((width as u32).div_ceil(cw), r)
            }
            (c, r) => (c, r),
        };
        let (cols, rows) = (cols.clamp(1, 1000), rows.clamp(1, 1000));
        let placement = Placement {
            image_id,
            cols,
            rows,
            src_x,
            src_y,
            src_w,
            src_h,
            offset_x,
            offset_y,
            fit_cells: cmd.cols != 0 || cmd.rows != 0,
            z: cmd.z,
        };
        state.next_serial += 1;
        let serial = state.next_serial;
        if cmd.placement != 0
            && let Some(old) = state.keyed.insert((image_id, cmd.placement), serial)
        {
            state.placements.remove(&old);
        }
        state.placements.insert(serial, placement);
        drop(state);
        write_markers(out, serial, rows, cols, !cmd.keep_cursor);
    }

    /// Deletes by id, number, z-index, id range, or everything.
    fn delete(&mut self, cmd: &Command) {
        let state = self.graphics.state.lock().unwrap();
        let doomed: Vec<u64> = match cmd.delete.to_ascii_lowercase() {
            b'a' => state.placements.keys().copied().collect(),
            b'i' | b'n' => {
                let id = if cmd.delete.eq_ignore_ascii_case(&b'i') {
                    Some(cmd.id)
                } else {
                    state.numbers.get(&cmd.number).copied()
                };
                state
                    .placements
                    .iter()
                    .filter(|(serial, p)| {
                        Some(p.image_id) == id
                            && (cmd.placement == 0
                                || state.keyed.get(&(p.image_id, cmd.placement)) == Some(serial))
                    })
                    .map(|(s, _)| *s)
                    .collect()
            }
            b'z' => state
                .placements
                .iter()
                .filter(|(_, p)| p.z == cmd.z)
                .map(|(s, _)| *s)
                .collect(),
            // Images whose id is in x..=y.
            b'r' => state
                .placements
                .iter()
                .filter(|(_, p)| (cmd.x..=cmd.y).contains(&p.image_id))
                .map(|(s, _)| *s)
                .collect(),
            // By position: see `delete_at`. Others (animation frames) don't
            // apply.
            _ => Vec::new(),
        };
        drop(state);
        self.remove_placements(doomed, cmd.delete);
    }

    /// Deletes the placements on screen that cover a cell (`p`, `q` with a
    /// z-index), the cursor's cell (`c`), a column (`x`) or a row (`y`).
    /// Cells and rows are counted from 1, as in the protocol.
    fn delete_at(&mut self, cmd: &Command, screen: &dyn Screen) {
        // Each visible placement's column and rows, from its markers.
        let mut seen: HashMap<u64, (usize, Vec<usize>)> = HashMap::new();
        screen.for_each_link(&mut |uri, row, col| {
            if let Some((serial, _)) = parse_marker(uri) {
                seen.entry(serial).or_insert((col, Vec::new())).1.push(row);
            }
        });
        let (cursor_row, cursor_col) = screen.cursor();
        let (x, y) = (cmd.x as usize, cmd.y as usize);
        let state = self.graphics.state.lock().unwrap();
        let doomed: Vec<u64> = seen
            .into_iter()
            .filter(|(serial, (col, rows))| {
                let Some(p) = state.placements.get(serial) else {
                    return false;
                };
                let cols = *col..*col + p.cols.max(1) as usize;
                let covers =
                    |row: usize, column: usize| rows.contains(&row) && cols.contains(&column);
                match cmd.delete.to_ascii_lowercase() {
                    b'c' => covers(cursor_row, cursor_col),
                    b'p' => x > 0 && y > 0 && covers(y - 1, x - 1),
                    b'q' => x > 0 && y > 0 && covers(y - 1, x - 1) && p.z == cmd.z,
                    b'x' => x > 0 && cols.contains(&(x - 1)),
                    b'y' => y > 0 && rows.contains(&(y - 1)),
                    _ => false,
                }
            })
            .map(|(serial, _)| serial)
            .collect();
        drop(state);
        self.remove_placements(doomed, cmd.delete);
    }

    /// Removes placements; an upper-case `delete` also frees the images left
    /// without placements.
    fn remove_placements(&mut self, doomed: Vec<u64>, delete: u8) {
        let mut state = self.graphics.state.lock().unwrap();
        let free = delete.is_ascii_uppercase();
        let mut touched = Vec::new();
        for serial in doomed {
            if let Some(p) = state.placements.remove(&serial) {
                touched.push(p.image_id);
            }
        }
        let State {
            keyed, placements, ..
        } = &mut *state;
        keyed.retain(|_, serial| placements.contains_key(serial));
        if free {
            if delete == b'A' {
                // Everything is gone from the screen: free every image.
                touched = state.images.keys().copied().collect();
            }
            for id in touched {
                if !state.placements.values().any(|p| p.image_id == id) {
                    remove_image(&mut state, id);
                }
            }
        }
    }
}

fn remove_image(state: &mut State, id: u32) {
    if let Some(old) = state.images.remove(&id) {
        state.bytes -= old.rgba.len();
        state.order.retain(|&i| i != id);
        state.placements.retain(|_, p| p.image_id != id);
        state.keyed.retain(|(image, _), _| *image != id);
    }
}

/// One marker per image row (see the module docs). With `move_cursor`, the
/// cursor ends after the image on its last row, scrolling as needed;
/// otherwise it is restored and nothing scrolls.
fn write_markers(out: &mut Vec<u8>, serial: u64, rows: u32, cols: u32, move_cursor: bool) {
    if !move_cursor {
        out.extend_from_slice(b"\x1b7");
    }
    for row in 0..rows {
        let _ = write!(
            out,
            "\x1b]8;;{MARKER_SCHEME}{serial}:{row}\x1b\\ \x1b]8;;\x1b\\"
        );
        if row + 1 < rows {
            // Back to the image's left column, then down a row.
            out.extend_from_slice(if move_cursor {
                b"\x1b[D\n"
            } else {
                b"\x1b[D\x1b[B"
            });
        }
    }
    if move_cursor {
        if cols > 1 {
            let _ = write!(out, "\x1b[{}C", cols - 1);
        }
    } else {
        out.extend_from_slice(b"\x1b8");
    }
}

/// Replies like kitty: only when the client named the image (`i` or `I`),
/// and not when quieted (`q=1` hides OK, `q=2` hides everything).
fn respond(cmd: &Command, result: Result<Option<u32>, String>, reply: &mut Vec<u8>) {
    if cmd.id == 0 && cmd.number == 0 {
        return;
    }
    let message = match &result {
        Ok(_) if cmd.quiet >= 1 => return,
        Err(_) if cmd.quiet >= 2 => return,
        Ok(_) => "OK",
        Err(e) => e.as_str(),
    };
    let id = match result {
        Ok(Some(id)) => id,
        _ => cmd.id,
    };
    let _ = write!(reply, "\x1b_G");
    let mut sep = "";
    if id != 0 {
        let _ = write!(reply, "i={id}");
        sep = ",";
    }
    if cmd.number != 0 {
        let _ = write!(reply, "{sep}I={}", cmd.number);
        sep = ",";
    }
    if cmd.placement != 0 {
        let _ = write!(reply, "{sep}p={}", cmd.placement);
    }
    let _ = write!(reply, ";{message}\x1b\\");
}

#[cfg(test)]
mod tests;
