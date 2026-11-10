//! Control data of a graphics command: `key=value` pairs separated by commas.

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Command {
    /// `a`: t (transmit), T (transmit and display), p (display), d (delete),
    /// q (query).
    pub action: u8,
    /// `f`: 24 (RGB), 32 (RGBA) or 100 (PNG).
    pub format: u32,
    /// `t`: d (direct), f (file), t (temporary file), s (shared memory).
    pub medium: u8,
    /// `o`: z for zlib-compressed data.
    pub compression: u8,
    /// `m=1`: more chunks follow.
    pub more: bool,
    /// `q`: 1 hides OK replies, 2 hides errors too.
    pub quiet: u32,
    /// `i`: image id chosen by the client.
    pub id: u32,
    /// `I`: image number; the terminal picks the id.
    pub number: u32,
    /// `p`: placement id.
    pub placement: u32,
    /// `s`, `v`: pixel size of raw RGB/RGBA data.
    pub width: u32,
    pub height: u32,
    /// `S`, `O`: size and offset to read from a file.
    pub size: u64,
    pub offset: u64,
    /// `x`, `y`, `w`, `h`: source rectangle.
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
    /// `X`, `Y`: pixel offset inside the first cell.
    pub offset_x: u32,
    pub offset_y: u32,
    /// `c`, `r`: cells to cover.
    pub cols: u32,
    pub rows: u32,
    /// `C=1`: don't move the cursor.
    pub keep_cursor: bool,
    /// `U=1`: virtual placement for Unicode placeholders.
    pub virtual_placement: bool,
    /// `z`: stacking order; negative is below text.
    pub z: i32,
    /// `d`: what to delete.
    pub delete: u8,
}

impl Default for Command {
    fn default() -> Self {
        Self {
            action: b't',
            format: 32,
            medium: b'd',
            compression: 0,
            more: false,
            quiet: 0,
            id: 0,
            number: 0,
            placement: 0,
            width: 0,
            height: 0,
            size: 0,
            offset: 0,
            x: 0,
            y: 0,
            w: 0,
            h: 0,
            offset_x: 0,
            offset_y: 0,
            cols: 0,
            rows: 0,
            keep_cursor: false,
            virtual_placement: false,
            z: 0,
            delete: b'a',
        }
    }
}

impl Command {
    /// Unknown keys and malformed values are ignored.
    pub fn parse(control: &[u8]) -> Self {
        let mut cmd = Self::default();
        for pair in control.split(|&c| c == b',') {
            let [key, b'=', value @ ..] = pair else {
                continue;
            };
            let text = std::str::from_utf8(value).unwrap_or("");
            let num = || text.parse::<u32>().unwrap_or(0);
            let byte = value.first().copied().unwrap_or(0);
            match key {
                b'a' => cmd.action = byte,
                b'f' => cmd.format = num(),
                b't' => cmd.medium = byte,
                b'o' => cmd.compression = byte,
                b'm' => cmd.more = num() == 1,
                b'q' => cmd.quiet = num(),
                b'i' => cmd.id = num(),
                b'I' => cmd.number = num(),
                b'p' => cmd.placement = num(),
                b's' => cmd.width = num(),
                b'v' => cmd.height = num(),
                b'S' => cmd.size = text.parse().unwrap_or(0),
                b'O' => cmd.offset = text.parse().unwrap_or(0),
                b'x' => cmd.x = num(),
                b'y' => cmd.y = num(),
                b'w' => cmd.w = num(),
                b'h' => cmd.h = num(),
                b'X' => cmd.offset_x = num(),
                b'Y' => cmd.offset_y = num(),
                b'c' => cmd.cols = num(),
                b'r' => cmd.rows = num(),
                b'C' => cmd.keep_cursor = num() == 1,
                b'U' => cmd.virtual_placement = num() == 1,
                b'z' => cmd.z = text.parse().unwrap_or(0),
                b'd' => cmd.delete = byte,
                _ => {}
            }
        }
        cmd
    }
}
