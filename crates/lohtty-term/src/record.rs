//! Recording a pane to an asciicast file (asciinema's format, v3 or v2).
//!
//! A [`Recorder`] takes the PTY output as it arrives, resizes, prompt marks
//! (as markers, so players can jump command by command), optionally the
//! keyboard input, and the shell's exit code. Events are timestamped where
//! they happen and written by a thread of their own, so a slow disk never
//! holds up the terminal.
//!
//! A recording usually starts in the middle of a session, so its first
//! event redraws the screen as it is ([`screen_snapshot`]); players would
//! otherwise start from a blank screen.

use std::fmt::Write as _;
use std::fs::File;
use std::io::{BufWriter, Write as _};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use alacritty_terminal::Term;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::vte::ansi::{Color, NamedColor};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// asciicast v2: absolute times, read by older players.
    V2,
    /// asciicast v3 (asciinema 3): intervals between events.
    V3,
}

/// The terminal's colors, stored in the header so players show the
/// recording with them (`#rrggbb`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CastTheme {
    pub fg: String,
    pub bg: String,
    pub palette: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct Options {
    pub format: Format,
    /// Record what is typed too (`i` events): off unless asked, as it would
    /// include passwords.
    pub input: bool,
    /// Longest pause kept when playing back, in seconds.
    pub idle_time_limit: Option<f64>,
    pub title: Option<String>,
    /// The shell, stored as `SHELL` in the header's `env`.
    pub shell: Option<String>,
    pub theme: Option<CastTheme>,
}

enum Message {
    Event(Instant, char, String),
    /// The shell exited with this code (v3 `x` event); the file then closes.
    Exit(Instant, i32),
}

/// Writes an asciicast file (see the module docs).
pub struct Recorder {
    path: PathBuf,
    input: bool,
    tx: Option<mpsc::Sender<Message>>,
    writer: Option<JoinHandle<()>>,
    /// The end of a UTF-8 character split across two reads.
    partial: Vec<u8>,
}

impl Recorder {
    /// Creates `path` (never overwriting a file), writes the header and the
    /// screen as it is (`initial`), and starts the writer.
    pub fn start(
        path: &Path,
        options: &Options,
        cols: u16,
        rows: u16,
        initial: &str,
    ) -> std::io::Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let file = File::options().write(true).create_new(true).open(path)?;
        let mut out = BufWriter::new(file);
        writeln!(out, "{}", header(options, cols, rows))?;
        let start = Instant::now();
        let mut clock = Clock::new(options.format, start);
        if !initial.is_empty() {
            writeln!(out, "{}", event(clock.next(start), 'o', initial))?;
        }
        out.flush()?;

        let (tx, rx) = mpsc::channel::<Message>();
        let writer = std::thread::Builder::new()
            .name("cast-writer".into())
            .spawn(move || {
                let write = |out: &mut BufWriter<File>, line: String| {
                    if let Err(e) = writeln!(out, "{line}") {
                        tracing::warn!("recording: write failed: {e}");
                    }
                };
                while let Ok(first) = rx.recv() {
                    // Write what is queued, then flush once.
                    for message in std::iter::once(first).chain(rx.try_iter()) {
                        match message {
                            Message::Event(at, code, data) => {
                                write(&mut out, event(clock.next(at), code, &data));
                            }
                            Message::Exit(at, code) => {
                                if clock.format == Format::V3 {
                                    write(&mut out, event(clock.next(at), 'x', &code.to_string()));
                                }
                            }
                        }
                    }
                    let _ = out.flush();
                }
                let _ = out.flush();
            })?;
        Ok(Self {
            path: path.to_owned(),
            input: options.input,
            tx: Some(tx),
            writer: Some(writer),
            partial: Vec::new(),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn send(&self, message: Message) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(message);
        }
    }

    /// PTY output, as read.
    pub fn output(&mut self, bytes: &[u8]) {
        let text = utf8_chunk(&mut self.partial, bytes);
        if !text.is_empty() {
            self.send(Message::Event(Instant::now(), 'o', text));
        }
    }

    /// Bytes sent to the shell (kept only if the options asked for input).
    pub fn input(&self, bytes: &[u8]) {
        if self.input {
            let text = String::from_utf8_lossy(bytes).into_owned();
            self.send(Message::Event(Instant::now(), 'i', text));
        }
    }

    pub fn resize(&self, cols: u16, rows: u16) {
        self.send(Message::Event(
            Instant::now(),
            'r',
            format!("{cols}x{rows}"),
        ));
    }

    /// A navigation point (a prompt), with an optional label.
    pub fn marker(&self, label: &str) {
        self.send(Message::Event(Instant::now(), 'm', label.to_owned()));
    }

    /// Ends the recording, with the shell's exit code if it exited, and
    /// waits until the file is written.
    pub fn finish(mut self, exit_code: Option<i32>) -> PathBuf {
        if let Some(code) = exit_code {
            self.send(Message::Exit(Instant::now(), code));
        }
        self.close();
        self.path.clone()
    }

    fn close(&mut self) {
        self.tx = None;
        if let Some(writer) = self.writer.take() {
            let _ = writer.join();
        }
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        self.close();
    }
}

/// Event times in the file's format, rounded to milliseconds from the
/// start, so rounding never accumulates (an interval is the difference of
/// two rounded absolute times).
struct Clock {
    format: Format,
    start: Instant,
    last_ms: u64,
}

impl Clock {
    fn new(format: Format, start: Instant) -> Self {
        Self {
            format,
            start,
            last_ms: 0,
        }
    }

    fn next(&mut self, at: Instant) -> String {
        let elapsed = at.saturating_duration_since(self.start);
        // Events can arrive slightly out of order from different threads.
        let ms = ((elapsed.as_micros() + 500) / 1000).max(u128::from(self.last_ms)) as u64;
        let shown = match self.format {
            Format::V3 => ms - self.last_ms,
            Format::V2 => ms,
        };
        self.last_ms = ms;
        format!("{}.{:03}", shown / 1000, shown % 1000)
    }
}

fn header(options: &Options, cols: u16, rows: u16) -> String {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let mut h = String::from("{");
    match options.format {
        Format::V3 => {
            let _ = write!(
                h,
                "\"version\":3,\"term\":{{\"cols\":{cols},\"rows\":{rows},\"type\":\"xterm-256color\""
            );
            if let Some(theme) = &options.theme {
                let _ = write!(
                    h,
                    ",\"theme\":{{\"fg\":{},\"bg\":{},\"palette\":{}}}",
                    json_string(&theme.fg),
                    json_string(&theme.bg),
                    json_string(&theme.palette.join(":"))
                );
            }
            h.push('}');
        }
        Format::V2 => {
            let _ = write!(h, "\"version\":2,\"width\":{cols},\"height\":{rows}");
            if let Some(theme) = &options.theme {
                let _ = write!(
                    h,
                    ",\"theme\":{{\"fg\":{},\"bg\":{},\"palette\":{}}}",
                    json_string(&theme.fg),
                    json_string(&theme.bg),
                    json_string(&theme.palette.join(":"))
                );
            }
        }
    }
    let _ = write!(h, ",\"timestamp\":{timestamp}");
    if let Some(limit) = options.idle_time_limit {
        let _ = write!(h, ",\"idle_time_limit\":{limit}");
    }
    if let Some(title) = &options.title {
        let _ = write!(h, ",\"title\":{}", json_string(title));
    }
    let _ = write!(h, ",\"env\":{{\"TERM\":\"xterm-256color\"");
    if let Some(shell) = &options.shell {
        let _ = write!(h, ",\"SHELL\":{}", json_string(shell));
    }
    h.push_str("}}");
    h
}

fn event(time: String, code: char, data: &str) -> String {
    format!("[{time}, \"{code}\", {}]", json_string(data))
}

/// `text` as a JSON string literal.
pub(crate) fn json_string(text: &str) -> String {
    let mut s = String::with_capacity(text.len() + 2);
    s.push('"');
    for c in text.chars() {
        match c {
            '"' => s.push_str("\\\""),
            '\\' => s.push_str("\\\\"),
            '\n' => s.push_str("\\n"),
            '\r' => s.push_str("\\r"),
            '\t' => s.push_str("\\t"),
            c if (c as u32) < 0x20 || c == '\u{7f}' => {
                let _ = write!(s, "\\u{:04x}", c as u32);
            }
            c => s.push(c),
        }
    }
    s.push('"');
    s
}

/// Decodes `bytes` after the `partial` end of a previous chunk, keeping an
/// incomplete character at the end for the next call. Invalid bytes become
/// U+FFFD.
fn utf8_chunk(partial: &mut Vec<u8>, bytes: &[u8]) -> String {
    let mut data = std::mem::take(partial);
    data.extend_from_slice(bytes);
    let mut text = String::with_capacity(data.len());
    let mut rest = &data[..];
    loop {
        match std::str::from_utf8(rest) {
            Ok(valid) => {
                text.push_str(valid);
                break;
            }
            Err(e) => {
                let (valid, after) = rest.split_at(e.valid_up_to());
                // `valid` was just checked.
                text.push_str(std::str::from_utf8(valid).unwrap_or_default());
                match e.error_len() {
                    Some(bad) => {
                        text.push('\u{FFFD}');
                        rest = &after[bad..];
                    }
                    None => {
                        partial.extend_from_slice(after);
                        break;
                    }
                }
            }
        }
    }
    text
}

/// Escape sequences that draw the screen as it is: text, colors and
/// attributes, then the cursor. Shell integration marks are left out.
pub fn screen_snapshot<T>(term: &Term<T>) -> String {
    let grid = term.grid();
    let mut out = String::from("\x1b[0m\x1b[H\x1b[2J");
    let mut current = String::from("0");
    for row in 0..grid.screen_lines() {
        let line = Line(row as i32);
        // The last non-blank cell, so trailing spaces aren't written.
        let last = (0..grid.columns()).rev().find(|&col| {
            let cell = &grid[Point::new(line, Column(col))];
            cell.c != ' ' || cell.bg != Color::Named(NamedColor::Background)
        });
        let Some(last) = last else {
            continue;
        };
        let _ = write!(out, "\x1b[{};1H", row + 1);
        for col in 0..=last {
            let cell = &grid[Point::new(line, Column(col))];
            if cell
                .flags
                .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
            {
                continue;
            }
            let style = sgr(cell.fg, cell.bg, cell.flags);
            if style != current {
                let _ = write!(out, "\x1b[{style}m");
                current = style;
            }
            out.push(cell.c);
            if let Some(zw) = cell.zerowidth() {
                out.extend(zw.iter().filter(|&&c| !crate::prompt::is_mark(c)));
            }
        }
    }
    let cursor = grid.cursor.point;
    let _ = write!(
        out,
        "\x1b[0m\x1b[{};{}H",
        cursor.line.0.max(0) + 1,
        cursor.column.0 + 1
    );
    out
}

/// SGR parameters for a cell's style, starting with a reset.
fn sgr(fg: Color, bg: Color, flags: Flags) -> String {
    let mut s = String::from("0");
    for (flag, code) in [
        (Flags::BOLD, "1"),
        (Flags::DIM, "2"),
        (Flags::ITALIC, "3"),
        (Flags::UNDERLINE, "4"),
        (Flags::INVERSE, "7"),
        (Flags::HIDDEN, "8"),
        (Flags::STRIKEOUT, "9"),
    ] {
        if flags.contains(flag) {
            s.push(';');
            s.push_str(code);
        }
    }
    color(&mut s, fg, 30, 90, 38);
    color(&mut s, bg, 40, 100, 48);
    s
}

fn color(s: &mut String, color: Color, base: u8, bright: u8, extended: u8) {
    match color {
        Color::Named(named) => {
            let i = named as usize;
            if i < 8 {
                let _ = write!(s, ";{}", base as usize + i);
            } else if i < 16 {
                let _ = write!(s, ";{}", bright as usize + i - 8);
            }
            // Default colors (and dim variants) need nothing after the reset.
        }
        Color::Indexed(i) => {
            let _ = write!(s, ";{extended};5;{i}");
        }
        Color::Spec(rgb) => {
            let _ = write!(s, ";{extended};2;{};{};{}", rgb.r, rgb.g, rgb.b);
        }
    }
}

/// The local date and time as `YYYYMMDD-HHMMSS`, for file names.
pub fn local_timestamp() -> String {
    let (y, mo, d, h, mi, s) = local_time();
    format!("{y:04}{mo:02}{d:02}-{h:02}{mi:02}{s:02}")
}

#[cfg(windows)]
fn local_time() -> (u16, u16, u16, u16, u16, u16) {
    use windows_sys::Win32::System::SystemInformation::GetLocalTime;
    // SAFETY: GetLocalTime fills the structure it is given.
    let t = unsafe {
        let mut t = std::mem::zeroed();
        GetLocalTime(&mut t);
        t
    };
    (t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond)
}

#[cfg(unix)]
fn local_time() -> (u16, u16, u16, u16, u16, u16) {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs()) as libc::time_t;
    // SAFETY: localtime_r writes into the `tm` it is given.
    let tm = unsafe {
        let mut tm = std::mem::zeroed::<libc::tm>();
        libc::localtime_r(&now, &mut tm);
        tm
    };
    (
        (tm.tm_year + 1900) as u16,
        (tm.tm_mon + 1) as u16,
        tm.tm_mday as u16,
        tm.tm_hour as u16,
        tm.tm_min as u16,
        tm.tm_sec as u16,
    )
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use alacritty_terminal::event::VoidListener;
    use alacritty_terminal::term::Config;
    use alacritty_terminal::vte::ansi::{Processor, StdSyncHandler};

    use super::*;

    fn options(format: Format) -> Options {
        Options {
            format,
            input: false,
            idle_time_limit: Some(2.0),
            title: Some("t \"1\"".into()),
            shell: Some("pwsh".into()),
            theme: Some(CastTheme {
                fg: "#c0caf5".into(),
                bg: "#1a1b26".into(),
                palette: vec!["#000000".into(); 16],
            }),
        }
    }

    #[test]
    fn headers() {
        let v3 = header(&options(Format::V3), 80, 24);
        assert!(v3.starts_with(
            "{\"version\":3,\"term\":{\"cols\":80,\"rows\":24,\"type\":\"xterm-256color\",\"theme\":{\"fg\":\"#c0caf5\",\"bg\":\"#1a1b26\",\"palette\":\"#000000:"
        ));
        assert!(v3.contains(",\"idle_time_limit\":2,\"title\":\"t \\\"1\\\"\""));
        assert!(v3.ends_with(",\"env\":{\"TERM\":\"xterm-256color\",\"SHELL\":\"pwsh\"}}"));
        let v2 = header(&options(Format::V2), 80, 24);
        assert!(v2.starts_with("{\"version\":2,\"width\":80,\"height\":24,\"theme\":{"));
    }

    #[test]
    fn times_are_intervals_or_absolute_without_drift() {
        let start = Instant::now();
        let at = |us: u64| start + Duration::from_micros(us);
        let mut v3 = Clock::new(Format::V3, start);
        let mut v2 = Clock::new(Format::V2, start);
        // 0.4 ms steps: each rounds to 0 or 1 ms, but the sum stays exact.
        let times: Vec<u64> = (1..=10).map(|i| i * 400).collect();
        let intervals: Vec<String> = times.iter().map(|&t| v3.next(at(t))).collect();
        let total: f64 = intervals.iter().map(|s| s.parse::<f64>().unwrap()).sum();
        assert!((total - 0.004).abs() < 1e-9, "{intervals:?}");
        assert_eq!(v2.next(at(1_234_567)), "1.235");
        // Out of order: never negative, never backwards.
        assert_eq!(v2.next(at(1_000_000)), "1.235");
        assert_eq!(v3.next(at(0)), "0.000");
    }

    #[test]
    fn json_strings_escape_controls() {
        assert_eq!(
            json_string("a\"b\\c\n\r\t\x1b[0m\x07é"),
            "\"a\\\"b\\\\c\\n\\r\\t\\u001b[0m\\u0007é\""
        );
    }

    #[test]
    fn utf8_split_across_reads_is_kept_whole() {
        let mut partial = Vec::new();
        let bytes = "é❯".as_bytes();
        assert_eq!(utf8_chunk(&mut partial, &bytes[..1]), "");
        assert_eq!(utf8_chunk(&mut partial, &bytes[1..3]), "é");
        assert_eq!(utf8_chunk(&mut partial, &bytes[3..]), "❯");
        assert!(partial.is_empty());
        assert_eq!(utf8_chunk(&mut partial, b"a\xffb"), "a\u{FFFD}b");
    }

    struct Size;
    impl Dimensions for Size {
        fn total_lines(&self) -> usize {
            3
        }
        fn screen_lines(&self) -> usize {
            3
        }
        fn columns(&self) -> usize {
            10
        }
    }

    #[test]
    fn snapshot_redraws_text_colors_and_cursor() {
        let mut term = Term::new(Config::default(), &Size, VoidListener);
        Processor::<StdSyncHandler>::new().advance(
            &mut term,
            b"ab\x1b[1;31mc\x1b[0m\r\n\r\n  \x1b[38;2;1;2;3mx\x1b[0my",
        );
        assert_eq!(
            screen_snapshot(&term),
            "\x1b[0m\x1b[H\x1b[2J\x1b[1;1Hab\x1b[0;1;31mc\x1b[3;1H\x1b[0m  \x1b[0;38;2;1;2;3mx\x1b[0my\x1b[0m\x1b[3;5H"
        );
    }

    #[test]
    fn a_recording_file_has_header_snapshot_events_and_exit() {
        let dir = std::env::temp_dir().join(format!("lohtty-rec-{}", std::process::id()));
        let path = dir.join("a.cast");
        let _ = std::fs::remove_file(&path);
        let mut rec = Recorder::start(&path, &options(Format::V3), 10, 3, "screen").unwrap();
        rec.output("h\u{e9}".as_bytes());
        rec.resize(20, 5);
        rec.marker("");
        rec.input(b"secret");
        let saved = rec.finish(Some(1));
        let text = std::fs::read_to_string(&saved).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert!(lines[0].starts_with("{\"version\":3"));
        let events: Vec<String> = lines[1..]
            .iter()
            .map(|l| l.split_once(", ").unwrap().1.to_owned())
            .collect();
        assert_eq!(
            events,
            vec![
                "\"o\", \"screen\"]",
                "\"o\", \"hé\"]",
                "\"r\", \"20x5\"]",
                "\"m\", \"\"]",
                "\"x\", \"1\"]",
            ]
        );
        // Never overwrites.
        assert!(Recorder::start(&path, &options(Format::V3), 10, 3, "").is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn local_timestamp_shape() {
        let ts = local_timestamp();
        assert_eq!(ts.len(), 15);
        assert_eq!(&ts[8..9], "-");
        assert!(ts.starts_with("20"));
    }
}
