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
        ShellSpec::new("cmd.exe", vec!["/c".into(), "echo lohtty-ok".into()])
    } else {
        ShellSpec::new(
            "/bin/sh",
            vec!["-c".into(), "echo lohtty-ok; sleep 1".into()],
        )
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

/// An app that begins a synchronized update (mode 2026) and never ends it
/// must not freeze the screen: the update is flushed after its timeout.
#[test]
fn unterminated_synchronized_update_is_flushed() {
    let shell = if cfg!(windows) {
        ShellSpec::new(
            "powershell.exe",
            vec![
                "-NoProfile".into(),
                "-Command".into(),
                "[Console]::Write([char]27 + '[?2026hsync-flushed'); Start-Sleep 5".into(),
            ],
        )
    } else {
        ShellSpec::new(
            "/bin/sh",
            vec![
                "-c".into(),
                "printf '\\033[?2026hsync-flushed'; sleep 5".into(),
            ],
        )
    };
    let size = TermSize {
        cols: 80,
        rows: 24,
        cell_width: 8,
        cell_height: 16,
    };
    let (session, _events) = Session::spawn(&shell, size, 100, Box::new(PassThrough)).unwrap();

    // Well before the program exits (5 s), the text must be visible.
    let deadline = Instant::now() + Duration::from_secs(4);
    loop {
        let text = screen_text(&session);
        if text.contains("sync-flushed") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "synchronized update was never flushed:\n{text}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn resize_updates_grid_dimensions() {
    let shell = if cfg!(windows) {
        ShellSpec::new("cmd.exe", vec![])
    } else {
        ShellSpec::new("/bin/sh", vec![])
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
