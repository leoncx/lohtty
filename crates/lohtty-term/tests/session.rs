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

/// Sizes changing in a burst (a window animating) reach the shell as the
/// first and the last only, so it redraws its prompt once per burst.
#[cfg(unix)]
#[test]
fn resize_burst_reaches_the_pty_settled() {
    let shell = ShellSpec::new(
        "/bin/sh",
        vec![
            "-c".into(),
            "trap 'stty size' WINCH; echo ready; while :; do sleep 0.05; done".into(),
        ],
    );
    let size = TermSize {
        cols: 80,
        rows: 24,
        cell_width: 8,
        cell_height: 16,
    };
    let (session, _events) = Session::spawn(&shell, size, 100, Box::new(PassThrough)).unwrap();
    let wait_for = |needle: &str| {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let text = screen_text(&session);
            if text.contains(needle) {
                return text;
            }
            assert!(Instant::now() < deadline, "never saw {needle:?}:\n{text}");
            std::thread::sleep(Duration::from_millis(50));
        }
    };
    wait_for("ready");

    for cols in 100..=110 {
        session.resize(TermSize {
            cols,
            rows: 30,
            ..size
        });
        std::thread::sleep(Duration::from_millis(10));
    }
    let text = wait_for("30 110");
    assert!(text.contains("30 100"), "first size missing:\n{text}");
    for cols in 101..110 {
        assert!(
            !text.contains(&format!("30 {cols}")),
            "30 {cols} reached the shell:\n{text}"
        );
    }
}

/// The shell starts in the requested directory, and the session follows its
/// `cd`s (read from the process, as this shell reports nothing).
#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn shell_starts_in_cwd_and_session_follows_it() {
    let start = std::env::temp_dir().canonicalize().unwrap();
    let mut shell = ShellSpec::new(
        "/bin/sh",
        vec!["-c".into(), "pwd -P; cd /usr && echo moved; sleep 5".into()],
    );
    shell.cwd = Some(start.clone());
    let size = TermSize {
        cols: 200,
        rows: 24,
        cell_width: 8,
        cell_height: 16,
    };
    let (session, _events) = Session::spawn(&shell, size, 100, Box::new(PassThrough)).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let text = loop {
        let text = screen_text(&session);
        if text.contains("moved") {
            break text;
        }
        assert!(Instant::now() < deadline, "never moved:\n{text}");
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(
        text.contains(start.to_str().unwrap()),
        "didn't start in {start:?}:\n{text}"
    );
    assert_eq!(
        session.cwd().unwrap().canonicalize().unwrap(),
        std::path::Path::new("/usr").canonicalize().unwrap()
    );
}
