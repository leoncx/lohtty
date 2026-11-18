use alacritty_terminal::event::VoidListener;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::term::{Config, Term};
use alacritty_terminal::vte::ansi::Processor;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;

use super::*;

/// Runs `input` through a fresh filter, optionally one byte per call.
fn run(graphics: &Graphics, input: &[u8], bytewise: bool) -> (Vec<u8>, Vec<u8>) {
    let mut filter = graphics.filter();
    let (mut out, mut reply) = (Vec::new(), Vec::new());
    if bytewise {
        for b in input {
            filter.filter(std::slice::from_ref(b), &mut out, &mut reply);
        }
    } else {
        filter.filter(input, &mut out, &mut reply);
    }
    (out, reply)
}

fn apc(control: &str, payload: &[u8]) -> Vec<u8> {
    let mut v = format!("\x1b_G{control};").into_bytes();
    v.extend_from_slice(STANDARD.encode(payload).as_bytes());
    v.extend_from_slice(b"\x1b\\");
    v
}

fn rgba(w: u32, h: u32) -> Vec<u8> {
    (0..w * h).flat_map(|i| [i as u8, 2, 3, 255]).collect()
}

#[test]
fn other_output_passes_through_even_when_split() {
    let g = Graphics::new();
    let input = b"a\x1b[31mb\x1b_Xother apc\x1b\\c\x1b";
    for bytewise in [false, true] {
        let (out, reply) = run(&g, input, bytewise);
        // The trailing lone ESC is held until more output arrives.
        assert_eq!(out, &input[..input.len() - 1]);
        assert!(reply.is_empty());
    }
}

#[test]
fn query_replies_and_stores_nothing() {
    let g = Graphics::new();
    let mut input = b"before".to_vec();
    input.extend(apc("a=q,i=31,s=1,v=1,f=24,t=d", &[1, 2, 3]));
    input.extend(b"after");
    for bytewise in [false, true] {
        let (out, reply) = run(&g, &input, bytewise);
        assert_eq!(out, b"beforeafter");
        assert_eq!(reply, b"\x1b_Gi=31;OK\x1b\\");
    }
    assert!(g.state.lock().unwrap().images.is_empty());
}

#[test]
fn bad_data_reports_an_error() {
    let g = Graphics::new();
    let (_, reply) = run(&g, &apc("a=q,i=7,s=2,v=2,f=32", &[0; 4]), false);
    assert!(reply.starts_with(b"\x1b_Gi=7;ENODATA:"));
    let (_, reply) = run(&g, &apc("a=p,i=99", b""), false);
    assert!(reply.starts_with(b"\x1b_Gi=99;ENOENT:"));
}

#[test]
fn replies_follow_ids_and_quiet() {
    let g = Graphics::new();
    // No id: no reply.
    let (_, reply) = run(&g, &apc("a=t,s=1,v=1", &rgba(1, 1)), false);
    assert!(reply.is_empty());
    // Quiet OK.
    let (_, reply) = run(&g, &apc("a=t,i=1,q=1,s=1,v=1", &rgba(1, 1)), false);
    assert!(reply.is_empty());
    // Image number: the terminal picks the id and reports both.
    let (_, reply) = run(&g, &apc("a=t,I=5,s=1,v=1", &rgba(1, 1)), false);
    let reply = String::from_utf8(reply).unwrap();
    assert!(reply.starts_with("\x1b_Gi=") && reply.ends_with(",I=5;OK\x1b\\"));
}

#[test]
fn chunked_transmission_is_reassembled() {
    let g = Graphics::new();
    g.set_cell_size(1, 1);
    let data = STANDARD.encode(rgba(3, 2));
    let (a, b) = data.split_at(8); // multiple of 4, as clients chunk
    let mut input = format!("\x1b_Ga=T,i=3,s=3,v=2,m=1;{a}\x1b\\").into_bytes();
    input.extend(format!("\x1b_Gm=0;{b}\x1b\\").into_bytes());
    for bytewise in [false, true] {
        let (out, reply) = run(&g, &input, bytewise);
        assert_eq!(reply, b"\x1b_Gi=3;OK\x1b\\");
        let image = &g.state.lock().unwrap().images[&3];
        assert_eq!((image.width, image.height), (3, 2));
        assert_eq!(&image.rgba[..], &rgba(3, 2)[..]);
        assert!(String::from_utf8(out).unwrap().contains(MARKER_SCHEME));
    }
}

#[test]
fn chunks_padded_one_by_one_are_decoded() {
    // chafa pads every chunk, so '=' appears in the middle of the data.
    let g = Graphics::new();
    g.set_cell_size(1, 1);
    let pixels = rgba(3, 2);
    let (a, b) = pixels.split_at(5); // 5 bytes: padded with '='
    let mut input = format!("\x1b_Ga=t,i=7,s=3,v=2,m=1;{}\x1b\\", STANDARD.encode(a)).into_bytes();
    input.extend(format!("\x1b_Gm=0;{}\x1b\\", STANDARD.encode(b)).into_bytes());
    assert!(STANDARD.encode(a).ends_with('='));
    let (_, reply) = run(&g, &input, false);
    assert_eq!(reply, b"\x1b_Gi=7;OK\x1b\\");
    assert_eq!(&g.state.lock().unwrap().images[&7].rgba[..], &pixels[..]);

    // Unused bits set in a piece's last character are ignored, as in kitty:
    // "AAE=" and "AAF=" both decode to [0, 1].
    let mut input = b"\x1b_Ga=t,i=8,f=24,s=1,v=1,m=1;AAF=\x1b\\".to_vec();
    input.extend(b"\x1b_Gm=0;AA==\x1b\\");
    let (_, reply) = run(&g, &input, false);
    assert_eq!(reply, b"\x1b_Gi=8;OK\x1b\\");
    assert_eq!(
        &g.state.lock().unwrap().images[&8].rgba[..],
        &[0, 1, 0, 255][..]
    );
}

#[test]
fn png_and_rgb_decode() {
    let g = Graphics::new();
    let mut png = Vec::new();
    image::RgbaImage::from_raw(2, 1, vec![255, 0, 0, 255, 0, 255, 0, 128])
        .unwrap()
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    let (_, reply) = run(&g, &apc("a=t,i=1,f=100", &png), false);
    assert_eq!(reply, b"\x1b_Gi=1;OK\x1b\\");
    let (_, reply) = run(&g, &apc("a=t,i=2,f=24,s=1,v=1", &[9, 8, 7]), false);
    assert_eq!(reply, b"\x1b_Gi=2;OK\x1b\\");
    let state = g.state.lock().unwrap();
    assert_eq!(
        &state.images[&1].rgba[..],
        &[255, 0, 0, 255, 0, 255, 0, 128]
    );
    assert_eq!(&state.images[&2].rgba[..], &[9, 8, 7, 255]);
}

#[test]
fn zlib_compressed_data_decodes() {
    use std::io::Write as _;
    let g = Graphics::new();
    let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    enc.write_all(&rgba(2, 2)).unwrap();
    let (_, reply) = run(
        &g,
        &apc("a=t,i=4,s=2,v=2,o=z", &enc.finish().unwrap()),
        false,
    );
    assert_eq!(reply, b"\x1b_Gi=4;OK\x1b\\");
}

#[test]
fn cell_span_follows_size_and_aspect() {
    let g = Graphics::new();
    g.set_cell_size(10, 20);
    let place = |control: &str| {
        run(&g, &apc(control, &rgba(25, 30)), false);
        let state = g.state.lock().unwrap();
        let newest = &state.placements[state.placements.keys().max().unwrap()];
        (newest.cols, newest.rows)
    };
    assert_eq!(place("a=T,i=1,s=25,v=30"), (3, 2));
    assert_eq!(place("a=T,i=2,s=25,v=30,c=5"), (5, 3)); // 50 px wide -> 60 px tall
    assert_eq!(place("a=T,i=3,s=25,v=30,c=4,r=1"), (4, 1));
}

#[test]
fn delete_by_id_frees_with_uppercase() {
    let g = Graphics::new();
    run(&g, &apc("a=T,i=1,s=1,v=1", &rgba(1, 1)), false);
    run(&g, &apc("a=T,i=2,s=1,v=1", &rgba(1, 1)), false);
    run(&g, b"\x1b_Ga=d,d=i,i=1\x1b\\", false);
    {
        let state = g.state.lock().unwrap();
        assert!(state.images.contains_key(&1));
        assert_eq!(state.placements.len(), 1);
    }
    run(&g, b"\x1b_Ga=d,d=A\x1b\\", false);
    let state = g.state.lock().unwrap();
    assert!(state.images.is_empty() && state.placements.is_empty());
}

#[test]
fn same_placement_id_replaces() {
    let g = Graphics::new();
    run(&g, &apc("a=T,i=1,p=7,s=1,v=1", &rgba(1, 1)), false);
    run(&g, b"\x1b_Ga=p,i=1,p=7\x1b\\", false);
    run(&g, b"\x1b_Ga=p,i=1,p=8\x1b\\", false);
    assert_eq!(g.state.lock().unwrap().placements.len(), 2);
}

#[test]
fn quota_evicts_oldest_images() {
    let g = Graphics::new();
    {
        let mut state = g.state.lock().unwrap();
        state.bytes = QUOTA_BYTES - 4;
        state.images.insert(
            9,
            Image {
                width: 1,
                height: 1,
                rgba: vec![0; 4].into(),
                serial: 0,
            },
        );
        state.order.push_back(9);
    }
    run(&g, &apc("a=t,i=1,s=2,v=1", &rgba(2, 1)), false);
    let state = g.state.lock().unwrap();
    assert!(!state.images.contains_key(&9) && state.images.contains_key(&1));
}

/// Markers land in the grid one per image row, and the cursor ends after
/// the image (or stays with C=1).
#[test]
fn markers_anchor_rows_in_the_grid() {
    let g = Graphics::new();
    g.set_cell_size(10, 10);
    let size = crate::TermSize {
        cols: 20,
        rows: 6,
        cell_width: 10,
        cell_height: 10,
    };
    let mut term = Term::new(Config::default(), &size, VoidListener);
    let mut parser: Processor = Processor::new();

    let mut input = b"ab\r\n  ".to_vec();
    input.extend(apc("a=T,i=1,s=30,v=25", &rgba(30, 25))); // 3 x 3 cells
    input.extend(b"X");
    let (out, _) = run(&g, &input, false);
    parser.advance(&mut term, &out);

    let marker = |term: &Term<VoidListener>, line: i32, col: usize| {
        term.grid()[Point::new(Line(line), Column(col))]
            .hyperlink()
            .and_then(|h| parse_marker(h.uri()))
    };
    let serial = marker(&term, 1, 2).expect("first row marker").0;
    assert_eq!(marker(&term, 1, 2), Some((serial, 0)));
    assert_eq!(marker(&term, 2, 2), Some((serial, 1)));
    assert_eq!(marker(&term, 3, 2), Some((serial, 2)));
    assert_eq!(marker(&term, 1, 3), None);
    // Cursor after the image on its last row, so X lands at column 5.
    assert_eq!(term.grid()[Point::new(Line(3), Column(5))].c, 'X');
    assert_eq!(term.grid()[Point::new(Line(0), Column(0))].c, 'a');

    // C=1: markers written, cursor back where it was.
    let (out, _) = run(&g, b"\x1b_Ga=p,i=1,C=1\x1b\\Y", false);
    parser.advance(&mut term, &out);
    assert_eq!(term.grid()[Point::new(Line(3), Column(6))].c, 'Y');
    // Text written over a marker cell takes that row's marker away.
    assert_eq!(marker(&term, 3, 6), None);
    assert!(marker(&term, 4, 6).is_some() && marker(&term, 5, 6).is_some());
    assert_eq!(term.grid().screen_lines(), 6);
}

#[test]
fn file_and_temp_file_transmission() {
    let g = Graphics::new();
    let dir = std::env::temp_dir();
    let kept = dir.join("lohtty-kitty-test-file.rgba");
    let temp = dir.join("tty-graphics-protocol-lohtty-test.rgba");
    std::fs::write(&kept, rgba(1, 1)).unwrap();
    std::fs::write(&temp, rgba(1, 1)).unwrap();

    let (_, reply) = run(
        &g,
        &apc("a=t,i=1,t=f,s=1,v=1", kept.to_str().unwrap().as_bytes()),
        false,
    );
    assert_eq!(reply, b"\x1b_Gi=1;OK\x1b\\");
    assert!(kept.exists(), "a plain file is left alone");

    let (_, reply) = run(
        &g,
        &apc("a=t,i=2,t=t,s=1,v=1", temp.to_str().unwrap().as_bytes()),
        false,
    );
    assert_eq!(reply, b"\x1b_Gi=2;OK\x1b\\");
    assert!(!temp.exists(), "a temporary file is deleted after reading");

    let missing = dir.join("lohtty-kitty-missing.rgba");
    let (_, reply) = run(
        &g,
        &apc("a=t,i=3,t=f,s=1,v=1", missing.to_str().unwrap().as_bytes()),
        false,
    );
    assert!(reply.starts_with(b"\x1b_Gi=3;EBADF:"));
    let _ = std::fs::remove_file(kept);

    // Devices, pipes and network paths are refused without being opened.
    for path in [
        r"\\.\pipe\lohtty-test",
        r"\\server\share\x.png",
        "//server/x.png",
    ] {
        let (_, reply) = run(&g, &apc("a=t,i=4,t=f,f=100", path.as_bytes()), false);
        assert!(reply.starts_with(b"\x1b_Gi=4;EBADF:"), "{path}");
    }
    let (_, reply) = run(
        &g,
        &apc("a=t,i=5,t=f,f=100", dir.to_str().unwrap().as_bytes()),
        false,
    );
    assert!(reply.starts_with(b"\x1b_Gi=5;EBADF:not a regular file"));
}

/// A terminal fed the way the session's reader does: filter, then parse up
/// to each checkpoint and show the filter the screen there.
struct Screen20x8 {
    graphics: Graphics,
    filter: KittyFilter,
    term: Term<VoidListener>,
    parser: Processor,
}

impl Screen20x8 {
    fn new() -> Self {
        let graphics = Graphics::new();
        graphics.set_cell_size(10, 10);
        let size = crate::TermSize {
            cols: 20,
            rows: 8,
            cell_width: 10,
            cell_height: 10,
        };
        Self {
            filter: graphics.filter(),
            graphics,
            term: Term::new(Config::default(), &size, VoidListener),
            parser: Processor::new(),
        }
    }

    fn feed(&mut self, input: &[u8]) {
        let (mut out, mut reply) = (Vec::new(), Vec::new());
        self.filter.filter(input, &mut out, &mut reply);
        let mut start = 0;
        for at in self.filter.take_checkpoints() {
            self.parser.advance(&mut self.term, &out[start..at]);
            self.filter.at_checkpoint(&self.term);
            start = at;
        }
        self.parser.advance(&mut self.term, &out[start..]);
    }

    /// Image ids of the remaining placements, sorted.
    fn placed(&self) -> Vec<u32> {
        let state = self.graphics.state.lock().unwrap();
        let mut ids: Vec<u32> = state.placements.values().map(|p| p.image_id).collect();
        ids.sort();
        ids
    }

    fn stored(&self, id: u32) -> bool {
        self.graphics.state.lock().unwrap().images.contains_key(&id)
    }
}

/// Image 1 at rows 1-2, columns 0-2; image 2 at rows 1-2, columns 5-7
/// (3 x 2 cells each, C=1 so the cursor stays put between them).
fn two_images() -> Screen20x8 {
    let mut s = Screen20x8::new();
    let mut input = b"\r\n".to_vec();
    input.extend(apc("a=T,i=1,s=30,v=20,C=1", &rgba(30, 20)));
    input.extend(b"\x1b[5C");
    input.extend(apc("a=T,i=2,s=30,v=20,C=1,z=3", &rgba(30, 20)));
    s.feed(&input);
    assert_eq!(s.placed(), vec![1, 2]);
    s
}

#[test]
fn delete_at_a_cell_removes_only_the_image_there() {
    let mut s = two_images();
    // Cell (column 7, row 3) counted from 1: image 2's middle column, row 2.
    s.feed(b"\x1b_Ga=d,d=p,x=7,y=3\x1b\\");
    assert_eq!(s.placed(), vec![1]);
    assert!(s.stored(2), "lower-case keeps the image data");
    // A cell outside every image deletes nothing.
    s.feed(b"\x1b_Ga=d,d=p,x=4,y=2\x1b\\");
    assert_eq!(s.placed(), vec![1]);
    // Upper-case frees the image too.
    s.feed(b"\x1b_Ga=d,d=P,x=1,y=2\x1b\\");
    assert!(s.placed().is_empty());
    assert!(!s.stored(1));
}

#[test]
fn delete_at_a_cell_can_require_a_z_index() {
    let mut s = two_images();
    s.feed(b"\x1b_Ga=d,d=q,x=6,y=2,z=0\x1b\\");
    assert_eq!(s.placed(), vec![1, 2], "image 2 has z=3");
    s.feed(b"\x1b_Ga=d,d=q,x=6,y=2,z=3\x1b\\");
    assert_eq!(s.placed(), vec![1]);
}

#[test]
fn delete_by_column_and_row() {
    let mut s = two_images();
    s.feed(b"\x1b_Ga=d,d=x,x=2\x1b\\");
    assert_eq!(s.placed(), vec![2]);
    let mut s = two_images();
    s.feed(b"\x1b_Ga=d,d=y,y=1\x1b\\");
    assert_eq!(s.placed(), vec![1, 2], "row 1 is above the images");
    s.feed(b"\x1b_Ga=d,d=y,y=3\x1b\\");
    assert!(s.placed().is_empty());
}

#[test]
fn delete_at_the_cursor_sees_output_before_it_in_the_same_read() {
    let mut s = two_images();
    // One read: move the cursor onto image 2 (row 3, column 6), delete there.
    s.feed(b"\x1b[3;6H\x1b_Ga=d,d=c\x1b\\");
    assert_eq!(s.placed(), vec![1]);
    // And onto image 1 (row 3, column 3), the cursor moved by text in the
    // same read. (Text over column 1 would replace that row's marker, which
    // hides the image row.)
    s.feed(b"\x1b[3;2Ha\x1b_Ga=d,d=c\x1b\\");
    assert!(s.placed().is_empty());
}

#[test]
fn delete_by_id_range() {
    let mut s = two_images();
    s.feed(b"\x1b_Ga=d,d=r,x=2,y=9\x1b\\");
    assert_eq!(s.placed(), vec![1]);
    s.feed(b"\x1b_Ga=d,d=R,x=1,y=1\x1b\\");
    assert!(s.placed().is_empty() && !s.stored(1));
}

#[test]
fn virtual_placements_write_no_markers() {
    let g = Graphics::new();
    g.set_cell_size(10, 10);
    let (out, reply) = run(
        &g,
        &apc("a=T,i=4,U=1,c=6,r=3,s=30,v=20", &rgba(30, 20)),
        false,
    );
    assert_eq!(reply, b"\x1b_Gi=4;OK\x1b\\");
    assert!(out.is_empty(), "no markers and no cursor movement");
    let (p, image) = g.virtual_placement(4, 0).expect("virtual placement");
    assert_eq!((p.cols, p.rows), (6, 3));
    assert_eq!((image.width, image.height), (30, 20));
    // Without c and r the size follows the image, as for normal placements.
    run(&g, &apc("a=p,i=4,p=9,U=1", &[]), false);
    let (p, _) = g.virtual_placement(4, 9).unwrap();
    assert_eq!((p.cols, p.rows), (3, 2));
    // A cell without a placement id gets the lowest-numbered one.
    assert_eq!(g.virtual_placement(4, 0).unwrap().0.cols, 6);
    assert!(g.virtual_placement(5, 0).is_none());
}

#[test]
fn deletes_remove_virtual_placements() {
    let g = Graphics::new();
    g.set_cell_size(10, 10);
    run(
        &g,
        &apc("a=T,i=4,U=1,c=6,r=3,s=30,v=20", &rgba(30, 20)),
        false,
    );
    run(&g, &apc("a=p,i=4,p=2,U=1,c=2,r=1", &[]), false);
    run(&g, b"\x1b_Ga=d,d=i,i=4,p=2\x1b\\", false);
    assert!(g.virtual_placement(4, 2).is_none());
    assert!(g.virtual_placement(4, 0).is_some());
    // Positional deletes leave them: they have no position.
    run(&g, b"\x1b_Ga=d,d=p,x=1,y=1\x1b\\", false);
    assert!(g.virtual_placement(4, 0).is_some());
    // Upper-case by id frees the image once no placement is left.
    run(&g, b"\x1b_Ga=d,d=I,i=4\x1b\\", false);
    assert!(g.virtual_placement(4, 0).is_none());
    assert!(!g.state.lock().unwrap().images.contains_key(&4));
}

/// The frames of image `id`, as RGBA bytes, with their gaps.
fn frames(g: &Graphics, id: u32) -> Vec<(Vec<u8>, u32)> {
    let state = g.state.lock().unwrap();
    state.animations[&id]
        .frames
        .iter()
        .map(|f| (f.rgba.to_vec(), f.gap))
        .collect()
}

/// `n` pixels of an RGBA color given as `0xRRGGBBAA`.
fn px(color: u32, n: usize) -> Vec<u8> {
    color.to_be_bytes().repeat(n)
}

fn pixels(colors: &[u32]) -> Vec<u8> {
    colors.iter().flat_map(|c| c.to_be_bytes()).collect()
}

const RED: u32 = 0xff0000ff;
const GREEN: u32 = 0x00ff00ff;
const BLUE: u32 = 0x0000ffff;

#[test]
fn frames_are_composed_on_a_base_frame_or_a_color() {
    let g = Graphics::new();
    run(&g, &apc("a=t,i=1,s=2,v=1", &px(RED, 2)), false);
    // On a copy of frame 1, a green pixel at x=1: the reply names frame 2.
    let (_, reply) = run(&g, &apc("a=f,i=1,c=1,x=1,s=1,v=1", &px(GREEN, 1)), false);
    assert_eq!(reply, b"\x1b_Gi=1,r=2;OK\x1b\\");
    // On a blue canvas, with its own gap.
    let control = format!("a=f,i=1,Y={BLUE},s=1,v=1,z=120");
    let (_, reply) = run(&g, &apc(&control, &px(GREEN, 1)), false);
    assert_eq!(reply, b"\x1b_Gi=1,r=3;OK\x1b\\");
    // Editing frame 2 (r=2) in place, made gapless (negative gap).
    let (_, reply) = run(&g, &apc("a=f,i=1,r=2,s=1,v=1,z=-1", &px(BLUE, 1)), false);
    assert_eq!(reply, b"\x1b_Gi=1,r=2;OK\x1b\\");
    assert_eq!(
        frames(&g, 1),
        vec![
            (px(RED, 2), 0),
            (pixels(&[BLUE, GREEN]), 0),
            (pixels(&[GREEN, BLUE]), 120),
        ]
    );
    // The image still shows its root frame.
    assert_eq!(
        &g.state.lock().unwrap().images[&1].rgba[..],
        &px(RED, 2)[..]
    );
}

#[test]
fn new_frames_get_the_default_gap() {
    let g = Graphics::new();
    run(&g, &apc("a=t,i=1,s=1,v=1", &px(RED, 1)), false);
    run(&g, &apc("a=f,i=1,s=1,v=1", &px(GREEN, 1)), false);
    assert_eq!(frames(&g, 1)[1].1, 40);
}

#[test]
fn frames_blend_unless_replacing() {
    let g = Graphics::new();
    run(&g, &apc("a=t,i=1,s=1,v=1", &px(RED, 1)), false);
    let clear = 0x00ff0000; // transparent green
    run(&g, &apc("a=f,i=1,c=1,s=1,v=1", &px(clear, 1)), false);
    run(&g, &apc("a=f,i=1,c=1,X=1,s=1,v=1", &px(clear, 1)), false);
    let f = frames(&g, 1);
    assert_eq!(f[1].0, px(RED, 1));
    assert_eq!(f[2].0, px(clear, 1));
}

#[test]
fn frame_errors_reply() {
    let g = Graphics::new();
    let (_, reply) = run(&g, &apc("a=f,i=5,s=1,v=1", &px(RED, 1)), false);
    assert!(reply.starts_with(b"\x1b_Gi=5;ENOENT:"));
    run(&g, &apc("a=t,i=5,s=1,v=1", &px(RED, 1)), false);
    // Larger than the image.
    let (_, reply) = run(&g, &apc("a=f,i=5,s=2,v=1", &px(RED, 2)), false);
    assert!(reply.starts_with(b"\x1b_Gi=5;EINVAL:"), "{reply:?}");
    // No such base frame.
    let (_, reply) = run(&g, &apc("a=f,i=5,c=9,s=1,v=1", &px(RED, 1)), false);
    assert!(reply.starts_with(b"\x1b_Gi=5;EINVAL:"), "{reply:?}");
    assert_eq!(frames(&g, 5).len(), 1);
}

#[test]
fn chunked_frames_and_image_numbers() {
    let g = Graphics::new();
    let (_, reply) = run(&g, &apc("a=t,I=3,s=2,v=1", &px(RED, 2)), false);
    let id = u32::MAX;
    assert_eq!(reply, format!("\x1b_Gi={id},I=3;OK\x1b\\").into_bytes());
    let data = STANDARD.encode(px(GREEN, 2));
    let (a, b) = data.split_at(4);
    let mut input = format!("\x1b_Ga=f,I=3,s=2,v=1,m=1;{a}\x1b\\").into_bytes();
    input.extend(format!("\x1b_Gm=0;{b}\x1b\\").into_bytes());
    let (_, reply) = run(&g, &input, true);
    assert_eq!(reply, format!("\x1b_Gi={id},I=3,r=2;OK\x1b\\").into_bytes());
    assert_eq!(frames(&g, id)[1].0, px(GREEN, 2));
}

#[test]
fn control_runs_the_animation_when_drawn() {
    let ms = std::time::Duration::from_millis;
    let g = Graphics::new();
    run(&g, &apc("a=T,i=1,s=1,v=1", &px(RED, 1)), false);
    run(&g, &apc("a=f,i=1,s=1,v=1,z=50", &px(GREEN, 1)), false);
    let placement = *g.state.lock().unwrap().placements.keys().next().unwrap();
    // Stopped: drawing shows the root frame and asks for no wake-up.
    let (_, image) = g.lookup(placement).unwrap();
    assert_eq!(&image.rgba[..], &px(RED, 1)[..]);
    assert!(g.take_next_frame().is_none());
    // Control commands get no reply when they succeed. The root frame has no
    // gap until the client gives it one.
    let (_, reply) = run(&g, &apc("a=a,i=1,r=1,z=30", b""), false);
    assert!(reply.is_empty());
    let (_, reply) = run(&g, &apc("a=a,i=1,s=3", b""), false);
    assert!(reply.is_empty());
    let start = g.state.lock().unwrap().animations[&1].shown_at;
    let shown = |at: u64| {
        let mut state = g.state.lock().unwrap();
        let image = shown_image(&mut state, 1, start + ms(at)).unwrap();
        (image.rgba.to_vec(), image.serial)
    };
    let (root, root_serial) = shown(10);
    assert_eq!(root, px(RED, 1));
    assert_eq!(g.take_next_frame(), Some(start + ms(30)));
    assert!(g.take_next_frame().is_none());
    let (second, second_serial) = shown(30);
    assert_eq!(second, px(GREEN, 1));
    assert_ne!(second_serial, root_serial);
    assert_eq!(g.take_next_frame(), Some(start + ms(80)));
    assert_eq!(shown(80).0, px(RED, 1));
    assert!(g.take_next_frame().is_some());
    // Choosing a frame, then stopping.
    run(&g, &apc("a=a,i=1,c=2,s=1", b""), false);
    assert_eq!(shown(10_000).0, px(GREEN, 1));
    assert!(g.take_next_frame().is_none());
    // An unknown image is an error.
    let (_, reply) = run(&g, &apc("a=a,i=9,s=3", b""), false);
    assert!(reply.starts_with(b"\x1b_Gi=9;ENOENT:"));
}

#[test]
fn frames_compose_onto_each_other() {
    let g = Graphics::new();
    run(&g, &apc("a=t,i=1,s=2,v=2", &px(RED, 4)), false);
    let control = format!("a=f,i=1,Y={BLUE},s=1,v=1,X=1");
    run(&g, &apc(&control, &px(BLUE, 1)), false);
    // A 1×1 rectangle of frame 1 at (0, 0) onto frame 2 at (1, 1).
    let (_, reply) = run(&g, &apc("a=c,i=1,r=1,c=2,w=1,h=1,x=1,y=1", b""), false);
    assert_eq!(reply, b"\x1b_Gi=1;OK\x1b\\");
    assert_eq!(frames(&g, 1)[1].0, pixels(&[BLUE, BLUE, BLUE, RED]));
    for (control, error) in [
        // Missing source or destination frame.
        ("a=c,i=1,r=3,c=2", "ENOENT"),
        ("a=c,i=1,r=1,c=3", "ENOENT"),
        // Destination or source out of bounds.
        ("a=c,i=1,r=1,c=2,w=2,h=1,x=1", "EINVAL"),
        ("a=c,i=1,r=1,c=2,w=1,h=1,X=2", "EINVAL"),
        // Overlapping rectangles in the same frame.
        ("a=c,i=1,r=2,c=2", "EINVAL"),
    ] {
        let (_, reply) = run(&g, &apc(control, b""), false);
        let prefix = format!("\x1b_Gi=1;{error}:");
        assert!(reply.starts_with(prefix.as_bytes()), "{control}: {reply:?}");
    }
    // The same frame, rectangles apart: the bottom-right pixel replaces the
    // top-left one.
    let (_, reply) = run(&g, &apc("a=c,i=1,r=2,c=2,w=1,h=1,X=1,Y=1,C=1", b""), false);
    assert_eq!(reply, b"\x1b_Gi=1;OK\x1b\\");
    assert_eq!(frames(&g, 1)[1].0, pixels(&[RED, BLUE, BLUE, RED]));
}

#[test]
fn composing_onto_the_shown_frame_updates_the_image() {
    let g = Graphics::new();
    run(&g, &apc("a=t,i=1,s=1,v=1", &px(RED, 1)), false);
    run(&g, &apc("a=f,i=1,s=1,v=1", &px(GREEN, 1)), false);
    let before = g.state.lock().unwrap().images[&1].serial;
    run(&g, &apc("a=c,i=1,r=2,c=1,C=1", b""), false);
    let state = g.state.lock().unwrap();
    assert_eq!(&state.images[&1].rgba[..], &px(GREEN, 1)[..]);
    assert_ne!(state.images[&1].serial, before);
}

#[test]
fn deleting_frames() {
    let g = Graphics::new();
    run(&g, &apc("a=t,i=1,s=1,v=1", &px(RED, 1)), false);
    for color in [GREEN, BLUE] {
        run(&g, &apc("a=f,i=1,s=1,v=1", &px(color, 1)), false);
    }
    assert_eq!(g.state.lock().unwrap().bytes, 12);
    // Show frame 3, then delete frame 2: frame 3 stays shown.
    run(&g, &apc("a=a,i=1,c=3", b""), false);
    run(&g, &apc("a=d,d=f,i=1,r=2", b""), false);
    let colors: Vec<_> = frames(&g, 1).into_iter().map(|f| f.0).collect();
    assert_eq!(colors, vec![px(RED, 1), px(BLUE, 1)]);
    {
        let state = g.state.lock().unwrap();
        assert_eq!(state.animations[&1].current, 1);
        assert_eq!(&state.images[&1].rgba[..], &px(BLUE, 1)[..]);
        assert_eq!(state.bytes, 8);
    }
    // No `r`: the root frame goes, and the next one becomes the root.
    run(&g, &apc("a=d,d=f,i=1", b""), false);
    assert_eq!(frames(&g, 1).len(), 1);
    assert_eq!(
        &g.state.lock().unwrap().images[&1].rgba[..],
        &px(BLUE, 1)[..]
    );
    // The last frame stays with `f`, and goes with the image with `F`.
    run(&g, &apc("a=d,d=f,i=1", b""), false);
    assert!(g.state.lock().unwrap().images.contains_key(&1));
    run(&g, &apc("a=d,d=F,i=1", b""), false);
    let state = g.state.lock().unwrap();
    assert!(state.images.is_empty() && state.animations.is_empty());
    assert_eq!(state.bytes, 0);
}

#[test]
fn resending_an_image_drops_its_frames() {
    let g = Graphics::new();
    run(&g, &apc("a=t,i=1,s=1,v=1", &px(RED, 1)), false);
    run(&g, &apc("a=f,i=1,s=1,v=1", &px(GREEN, 1)), false);
    run(&g, &apc("a=t,i=1,s=1,v=1", &px(BLUE, 1)), false);
    let state = g.state.lock().unwrap();
    assert!(state.animations.is_empty());
    assert_eq!(state.bytes, 4);
}

#[test]
fn frames_count_toward_the_quota() {
    let g = Graphics::new();
    run(&g, &apc("a=t,i=1,s=1,v=1", &px(RED, 1)), false);
    run(&g, &apc("a=t,i=2,s=1,v=1", &px(RED, 1)), false);
    g.state.lock().unwrap().bytes = QUOTA_BYTES - 3;
    // A new frame of image 2 evicts image 1, the oldest.
    let (_, reply) = run(&g, &apc("a=f,i=2,s=1,v=1", &px(GREEN, 1)), false);
    assert_eq!(reply, b"\x1b_Gi=2,r=2;OK\x1b\\");
    assert!(!g.state.lock().unwrap().images.contains_key(&1));
    // Nothing else to evict: the frame is refused.
    g.state.lock().unwrap().bytes = QUOTA_BYTES - 2;
    let (_, reply) = run(&g, &apc("a=f,i=2,s=1,v=1", &px(GREEN, 1)), false);
    assert!(reply.starts_with(b"\x1b_Gi=2;ENOSPC:"), "{reply:?}");
    assert_eq!(frames(&g, 2).len(), 2);
}
