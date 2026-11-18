//! Shell integration marks (OSC 133): where prompts and command output
//! start, and whether commands succeeded.
//!
//! Shells send `OSC 133 ; A` when a prompt starts, `; B` when it ends, `; C`
//! when a command's output starts and `; D ; <exit code>` when it finishes.
//! Grid lines move as output scrolls, and nothing counts the lines that leave
//! a full scrollback, so a mark isn't kept as a line number. Instead,
//! [`PromptMarker`] adds an invisible zero-width character after the first
//! character printed once a prompt or an output starts. The parser stores it
//! with that cell, so the mark scrolls, reflows and is erased with its text.
//!
//! The marks are variation selectors 17 to 20 (U+E0100…E0103): zero width,
//! and only meaningful after CJK ideographs, where shells' prompts and
//! commands' first characters practically never are. The renderer, copy and
//! search ignore them ([`is_mark`], [`strip`]).

use alacritty_terminal::Term;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point};

/// A prompt starts here; the command run from the previous prompt had no
/// known result.
const PROMPT: char = '\u{E0100}';
/// A prompt starts here; the previous command succeeded.
const PROMPT_AFTER_SUCCESS: char = '\u{E0101}';
/// A prompt starts here; the previous command failed.
const PROMPT_AFTER_FAILURE: char = '\u{E0102}';
/// A command's output starts here.
const OUTPUT: char = '\u{E0103}';

/// Longest OSC body looked at; longer ones aren't ours.
const MAX_OSC: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark {
    /// A prompt starts, with the result of the command run from the
    /// previous prompt: `Some(true)` for success, `None` when unknown or
    /// when no command ran.
    Prompt { previous: Option<bool> },
    /// A command's output starts.
    Output,
}

impl Mark {
    fn to_char(self) -> char {
        match self {
            Mark::Prompt { previous: None } => PROMPT,
            Mark::Prompt {
                previous: Some(true),
            } => PROMPT_AFTER_SUCCESS,
            Mark::Prompt {
                previous: Some(false),
            } => PROMPT_AFTER_FAILURE,
            Mark::Output => OUTPUT,
        }
    }

    pub fn from_char(c: char) -> Option<Mark> {
        Some(match c {
            PROMPT => Mark::Prompt { previous: None },
            PROMPT_AFTER_SUCCESS => Mark::Prompt {
                previous: Some(true),
            },
            PROMPT_AFTER_FAILURE => Mark::Prompt {
                previous: Some(false),
            },
            OUTPUT => Mark::Output,
            _ => return None,
        })
    }
}

/// Whether `c` is one of the mark characters.
pub fn is_mark(c: char) -> bool {
    (PROMPT..=OUTPUT).contains(&c)
}

/// `text` without mark characters (for copied or searched text).
pub fn strip(text: &str) -> String {
    text.chars().filter(|&c| !is_mark(c)).collect()
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Scan {
    Ground,
    Esc,
    /// ESC followed by intermediate bytes (`ESC ( B`...).
    EscIntermediate,
    Csi,
    Osc,
    OscEsc,
    /// DCS, APC, PM or SOS: skipped up to ST.
    Str,
    StrEsc,
}

/// Inserts mark characters into the PTY output (see the module docs).
pub struct PromptMarker {
    scan: Scan,
    osc: Vec<u8>,
    /// The mark to add after the next printed character.
    pending: Option<Mark>,
    /// Continuation bytes left in the UTF-8 character being copied.
    utf8_left: u8,
    /// The last command's result (`OSC 133 ; D`), for the next prompt.
    last_result: Option<bool>,
}

impl Default for PromptMarker {
    fn default() -> Self {
        Self::new()
    }
}

const ESC: u8 = 0x1b;

impl PromptMarker {
    pub fn new() -> Self {
        Self {
            scan: Scan::Ground,
            osc: Vec::new(),
            pending: None,
            utf8_left: 0,
            last_result: None,
        }
    }

    /// Copies `input` to `out`, adding mark characters.
    pub fn mark(&mut self, input: &[u8], out: &mut Vec<u8>) {
        let mut i = 0;
        while i < input.len() {
            // Nothing to mark: copy up to the next ESC in one go.
            if self.scan == Scan::Ground && self.pending.is_none() && self.utf8_left == 0 {
                let n = input[i..]
                    .iter()
                    .position(|&b| b == ESC)
                    .unwrap_or(input.len() - i);
                out.extend_from_slice(&input[i..i + n]);
                i += n;
                if i == input.len() {
                    break;
                }
            }
            let b = input[i];
            i += 1;
            out.push(b);
            self.step(b, out);
        }
    }

    fn step(&mut self, b: u8, out: &mut Vec<u8>) {
        match self.scan {
            Scan::Ground => {
                if b == ESC {
                    self.utf8_left = 0;
                    self.scan = Scan::Esc;
                } else if b < 0x20 || b == 0x7f {
                    // Controls print nothing.
                } else {
                    if self.utf8_left > 0 && (0x80..0xc0).contains(&b) {
                        self.utf8_left -= 1;
                    } else {
                        self.utf8_left = match b {
                            0xc0..=0xdf => 1,
                            0xe0..=0xef => 2,
                            0xf0..=0xf7 => 3,
                            _ => 0,
                        };
                    }
                    if self.utf8_left == 0
                        && let Some(mark) = self.pending.take()
                    {
                        let mut buf = [0; 4];
                        out.extend_from_slice(mark.to_char().encode_utf8(&mut buf).as_bytes());
                    }
                }
            }
            Scan::Esc => {
                self.scan = match b {
                    b'[' => Scan::Csi,
                    b']' => {
                        self.osc.clear();
                        Scan::Osc
                    }
                    b'P' | b'_' | b'^' | b'X' => Scan::Str,
                    0x20..=0x2f => Scan::EscIntermediate,
                    ESC => Scan::Esc,
                    _ => Scan::Ground,
                }
            }
            Scan::EscIntermediate => {
                if !(0x20..=0x2f).contains(&b) {
                    self.scan = if b == ESC { Scan::Esc } else { Scan::Ground };
                }
            }
            Scan::Csi => match b {
                0x40..=0x7e => self.scan = Scan::Ground,
                ESC => self.scan = Scan::Esc,
                _ => {}
            },
            Scan::Osc => match b {
                0x07 => {
                    self.finish_osc();
                    self.scan = Scan::Ground;
                }
                ESC => self.scan = Scan::OscEsc,
                _ => {
                    if self.osc.len() <= MAX_OSC {
                        self.osc.push(b);
                    }
                }
            },
            Scan::OscEsc => {
                if b == b'\\' {
                    self.finish_osc();
                    self.scan = Scan::Ground;
                } else {
                    // An ESC ends the OSC; this byte follows that ESC.
                    self.scan = Scan::Esc;
                    self.step(b, out);
                }
            }
            Scan::Str => {
                if b == ESC {
                    self.scan = Scan::StrEsc;
                }
            }
            Scan::StrEsc => {
                self.scan = if b == b'\\' { Scan::Ground } else { Scan::Str };
            }
        }
    }

    fn finish_osc(&mut self) {
        if self.osc.len() > MAX_OSC {
            return;
        }
        let Some(body) = self.osc.strip_prefix(b"133;") else {
            return;
        };
        let mut fields = body.split(|&c| c == b';');
        match fields.next() {
            Some(b"A") => {
                self.pending = Some(Mark::Prompt {
                    previous: self.last_result.take(),
                });
            }
            Some(b"C") => self.pending = Some(Mark::Output),
            Some(b"D") => {
                self.last_result = fields
                    .next()
                    .and_then(|code| std::str::from_utf8(code).ok())
                    .and_then(|code| code.parse::<i64>().ok())
                    .map(|code| code == 0);
                // A command that printed nothing has no output mark.
                if self.pending == Some(Mark::Output) {
                    self.pending = None;
                }
            }
            _ => {}
        }
    }
}

/// A mark found in the grid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MarkAt {
    /// The marked cell, in grid coordinates (negative lines are scrollback).
    pub point: Point,
    pub mark: Mark,
}

/// The marks of a cell, if any.
pub fn cell_marks(zerowidth: Option<&[char]>) -> impl Iterator<Item = Mark> + '_ {
    zerowidth
        .unwrap_or(&[])
        .iter()
        .filter_map(|&c| Mark::from_char(c))
}

/// The marks on lines `first..=last`, top to bottom.
pub fn marks_in<T>(term: &Term<T>, first: Line, last: Line) -> Vec<MarkAt> {
    let grid = term.grid();
    let first = first.max(Line(-(grid.history_size() as i32)));
    let last = last.min(Line(grid.screen_lines() as i32 - 1));
    let mut found = Vec::new();
    let mut line = first;
    while line <= last {
        for col in 0..grid.columns() {
            let point = Point::new(line, Column(col));
            found.extend(cell_marks(grid[point].zerowidth()).map(|mark| MarkAt { point, mark }));
        }
        line += 1;
    }
    found
}

/// Every mark in the scrollback and on the screen, top to bottom.
pub fn all_marks<T>(term: &Term<T>) -> Vec<MarkAt> {
    let grid = term.grid();
    marks_in(
        term,
        Line(-(grid.history_size() as i32)),
        Line(grid.screen_lines() as i32 - 1),
    )
}

/// The result of the command run from the prompt at `prompt`: told by the
/// next prompt below it. `None` while there is no next prompt, or when the
/// shell didn't say.
pub fn result_of<T>(term: &Term<T>, prompt: Point) -> Option<bool> {
    let grid = term.grid();
    let last = Line(grid.screen_lines() as i32 - 1);
    let mut line = prompt.line;
    while line <= last {
        let start = if line == prompt.line {
            prompt.column.0 + 1
        } else {
            0
        };
        for col in start..grid.columns() {
            let cell = &grid[Point::new(line, Column(col))];
            for mark in cell_marks(cell.zerowidth()) {
                if let Mark::Prompt { previous } = mark {
                    return previous;
                }
            }
        }
        line += 1;
    }
    None
}

/// Where the last command's output is: from its output mark up to the next
/// prompt mark (excluded), or up to the end of the screen while it runs.
pub fn last_output(marks: &[MarkAt]) -> Option<(Point, Option<Point>)> {
    let index = marks.iter().rposition(|m| m.mark == Mark::Output)?;
    let end = marks[index + 1..]
        .iter()
        .find(|m| matches!(m.mark, Mark::Prompt { .. }))
        .map(|m| m.point);
    Some((marks[index].point, end))
}

#[cfg(test)]
mod tests {
    use alacritty_terminal::event::VoidListener;
    use alacritty_terminal::term::Config;
    use alacritty_terminal::vte::ansi::Processor;

    use super::*;

    fn mark_all(input: &[u8], bytewise: bool) -> Vec<u8> {
        let mut marker = PromptMarker::new();
        let mut out = Vec::new();
        if bytewise {
            for b in input {
                marker.mark(std::slice::from_ref(b), &mut out);
            }
        } else {
            marker.mark(input, &mut out);
        }
        out
    }

    fn with(text: &str) -> String {
        text.replace("{P}", "\u{E0100}")
            .replace("{P+}", "\u{E0101}")
            .replace("{P-}", "\u{E0102}")
            .replace("{O}", "\u{E0103}")
    }

    #[test]
    fn marks_follow_the_first_printed_character() {
        let input = "\x1b]133;A\x07\x1b[32m> \x1b]133;B\x1b\\ls\r\n\x1b]133;C\x07\r\nfile\r\n\x1b]133;D;0\x07\x1b]133;A\x07$ ";
        let expected = with(
            "\x1b]133;A\x07\x1b[32m>{P} \x1b]133;B\x1b\\ls\r\n\x1b]133;C\x07\r\nf{O}ile\r\n\x1b]133;D;0\x07\x1b]133;A\x07${P+} ",
        );
        for bytewise in [false, true] {
            assert_eq!(
                String::from_utf8(mark_all(input.as_bytes(), bytewise)).unwrap(),
                expected
            );
        }
    }

    #[test]
    fn failures_and_unknown_results() {
        let input = "\x1b]133;D;127\x07\x1b]133;A\x07a\x1b]133;D\x07\x1b]133;A\x07b\x1b]133;A\x07c";
        assert_eq!(
            String::from_utf8(mark_all(input.as_bytes(), false)).unwrap(),
            with(
                "\x1b]133;D;127\x07\x1b]133;A\x07a{P-}\x1b]133;D\x07\x1b]133;A\x07b{P}\x1b]133;A\x07c{P}"
            )
        );
    }

    #[test]
    fn multibyte_characters_and_escape_sequences_are_skipped_whole() {
        // ESC ( B designates a charset: its B isn't printed. A command with
        // no output gets no output mark.
        let input =
            "\x1b]133;A\x07\x1b(B\x1b]0;title\x07\x1b_Gx\x1b\\❯ \x1b]133;C\x07\x1b]133;D;0\x07";
        assert_eq!(
            String::from_utf8(mark_all(input.as_bytes(), true)).unwrap(),
            with(
                "\x1b]133;A\x07\x1b(B\x1b]0;title\x07\x1b_Gx\x1b\\❯{P} \x1b]133;C\x07\x1b]133;D;0\x07"
            )
        );
    }

    #[test]
    fn other_output_is_unchanged() {
        let input = "plain \x1b[1mbold\x1b[0m é \x1b]8;;http://x\x1b\\link\x1b]8;;\x1b\\";
        assert_eq!(mark_all(input.as_bytes(), false), input.as_bytes());
        assert_eq!(strip(&with("a{P}b{O}")), "ab");
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

    fn term(input: &str) -> Term<VoidListener> {
        let mut term = Term::new(
            Config {
                scrolling_history: 100,
                ..Config::default()
            },
            &Size,
            VoidListener,
        );
        let mut out = Vec::new();
        PromptMarker::new().mark(input.as_bytes(), &mut out);
        Processor::<alacritty_terminal::vte::ansi::StdSyncHandler>::new().advance(&mut term, &out);
        term
    }

    const SESSION: &str = "\x1b]133;A\x07$ \x1b]133;B\x07ls\r\n\x1b]133;C\x07a\r\nb\r\n\x1b]133;D;1\x07\x1b]133;A\x07$ \x1b]133;B\x07true\r\n\x1b]133;C\x07\x1b]133;D;0\x07\x1b]133;A\x07$ ";

    #[test]
    fn marks_are_found_in_the_scrollback() {
        // Five lines on 3 rows: the first prompt and its output's first line
        // scrolled up.
        let t = term(SESSION);
        let marks = all_marks(&t);
        let at = |line, col| Point::new(Line(line), Column(col));
        assert_eq!(
            marks,
            vec![
                MarkAt {
                    point: at(-2, 0),
                    mark: Mark::Prompt { previous: None }
                },
                MarkAt {
                    point: at(-1, 0),
                    mark: Mark::Output
                },
                MarkAt {
                    point: at(1, 0),
                    mark: Mark::Prompt {
                        previous: Some(false)
                    }
                },
                MarkAt {
                    point: at(2, 0),
                    mark: Mark::Prompt {
                        previous: Some(true)
                    }
                },
            ]
        );
        assert_eq!(result_of(&t, at(-2, 0)), Some(false));
        assert_eq!(result_of(&t, at(1, 0)), Some(true));
        assert_eq!(result_of(&t, at(2, 0)), None);
        assert_eq!(last_output(&marks), Some((at(-1, 0), Some(at(1, 0)))));
        // The text keeps no mark.
        let text = t.bounds_to_string(at(-1, 0), at(0, 9));
        assert_eq!(strip(&text).trim_end(), "a\nb");
    }
}
