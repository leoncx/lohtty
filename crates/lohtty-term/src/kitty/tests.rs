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
