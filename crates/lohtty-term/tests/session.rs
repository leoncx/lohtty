//! Spawns a real shell through the platform PTY (ConPTY on Windows) and checks
//! that its output lands in the grid.

use std::time::{Duration, Instant};

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point};
use lohtty_term::{PassThrough, Session, ShellSpec, TermSize};

fn screen_text(session: &Session) -> String {
    let term = session.term().lock();
    let grid = term.grid();
    let mut text = String::new();
    for line in 0..grid.screen_lines() as i32 {
        for col in 0..grid.columns() {
            text.push(grid[Point::new(Line(line), Column(col))].c);
        }
        text.push('\n');
    }
    text
}

#[test]
fn shell_output_reaches_the_grid() {
    let shell = if cfg!(windows) {
        ShellSpec {
            program: "cmd.exe".into(),
            args: vec!["/c".into(), "echo lohtty-ok".into()],
        }
    } else {
        ShellSpec {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), "echo lohtty-ok; sleep 1".into()],
        }
    };
    let size = TermSize {
        cols: 80,
        rows: 24,
        cell_width: 8,
        cell_height: 16,
    };
    let (session, _events) = Session::spawn(&shell, size, 100, Box::new(PassThrough)).unwrap();

    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let text = screen_text(&session);
        if text.contains("lohtty-ok") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "output never reached the grid:\n{text}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn resize_updates_grid_dimensions() {
    let shell = if cfg!(windows) {
        ShellSpec {
            program: "cmd.exe".into(),
            args: vec![],
        }
    } else {
        ShellSpec {
            program: "/bin/sh".into(),
            args: vec![],
        }
    };
    let size = TermSize {
        cols: 80,
        rows: 24,
        cell_width: 8,
        cell_height: 16,
    };
    let (session, _events) = Session::spawn(&shell, size, 100, Box::new(PassThrough)).unwrap();
    session.resize(TermSize {
        cols: 120,
        rows: 40,
        ..size
    });
    let term = session.term().lock();
    assert_eq!(term.columns(), 120);
    assert_eq!(term.screen_lines(), 40);
}
