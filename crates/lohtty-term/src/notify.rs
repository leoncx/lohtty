//! Desktop notifications requested by programs through escape sequences.
//! Coding agents (Claude Code, Codex, ...) and other tools use them to say
//! they finished or need input:
//!
//! - OSC 9 (iTerm2): `ESC ] 9 ; body ST`. Bodies that start with a number
//!   and `;` are ConEmu sub-commands (`9;4;…` is progress), not
//!   notifications, and are skipped.
//! - OSC 777 (rxvt, Ghostty): `ESC ] 777 ; notify ; title ; body ST`.
//! - OSC 99 (kitty): `ESC ] 99 ; key=value:… ; payload ST`, where `p` says
//!   whether the payload is the `title` (default) or the `body`, `e=1`
//!   marks it base64, and `d=0` means more parts follow under the same `i`.
//!
//! The same scanner also picks up what the status protocol needs
//! ([`crate::status`]): OSC 7501 reports and queries, the start of a shell
//! prompt (OSC 133 A) and full resets (RIS, `ESC c`).
//!
//! [`OscScanner`] only watches the output: the sequences still reach the
//! parser, which ignores them.

use std::collections::HashMap;

/// A notification to show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notification {
    pub title: Option<String>,
    pub body: String,
}

/// Something the scanner found in the output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OscEvent {
    Notification(Notification),
    /// An OSC 7501 report or query.
    Status(crate::status::Message),
    /// OSC 9;4 progress.
    Progress(crate::status::Progress),
    /// A shell prompt starts (OSC 133 A).
    PromptStart,
    /// A full reset (RIS).
    Reset,
}

/// Longest OSC kept; longer ones are skipped.
const MAX_OSC: usize = 8 * 1024;
/// Longest title or body shown.
const MAX_TEXT: usize = 256;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
enum State {
    #[default]
    Normal,
    Esc,
    Osc,
    OscEsc,
}

/// Finds notification sequences in a stream of PTY output, across reads.
#[derive(Default)]
pub struct OscScanner {
    state: State,
    osc: Vec<u8>,
    too_long: bool,
    /// Kitty notifications still receiving parts (`d=0`), by id.
    kitty: HashMap<String, (String, String)>,
}

impl OscScanner {
    /// Scans `input`, appending what it completes to `found`.
    pub fn scan(&mut self, input: &[u8], found: &mut Vec<OscEvent>) {
        for &b in input {
            match self.state {
                State::Normal => {
                    if b == 0x1b {
                        self.state = State::Esc;
                    }
                }
                State::Esc => {
                    if b == b'c' {
                        found.push(OscEvent::Reset);
                    }
                    self.state = if b == b']' {
                        self.osc.clear();
                        self.too_long = false;
                        State::Osc
                    } else if b == 0x1b {
                        State::Esc
                    } else {
                        State::Normal
                    };
                }
                State::Osc => match b {
                    0x07 => self.finish(found),
                    0x1b => self.state = State::OscEsc,
                    // CAN and SUB abort a sequence.
                    0x18 | 0x1a => self.state = State::Normal,
                    _ => self.push(b),
                },
                State::OscEsc => {
                    if b == b'\\' {
                        self.finish(found);
                    } else {
                        // Not a terminator: the OSC is abandoned and the ESC
                        // starts whatever follows.
                        self.state = State::Esc;
                        self.scan(&[b], found);
                    }
                }
            }
        }
    }

    fn push(&mut self, b: u8) {
        if self.osc.len() < MAX_OSC {
            self.osc.push(b);
        } else {
            self.too_long = true;
        }
    }

    fn finish(&mut self, found: &mut Vec<OscEvent>) {
        self.state = State::Normal;
        if self.too_long {
            return;
        }
        let osc = String::from_utf8_lossy(&self.osc).into_owned();
        let (code, rest) = osc.split_once(';').unwrap_or((&osc, ""));
        let event = match code {
            "9" if rest == "4" || rest.starts_with("4;") => {
                crate::status::parse_progress(rest).map(OscEvent::Progress)
            }
            "9" => osc9(rest).map(OscEvent::Notification),
            "777" => osc777(rest).map(OscEvent::Notification),
            "99" => self.osc99(rest).map(OscEvent::Notification),
            "7501" => crate::status::parse(rest).map(OscEvent::Status),
            "133" if rest == "A" || rest.starts_with("A;") => Some(OscEvent::PromptStart),
            _ => None,
        };
        found.extend(event);
    }

    fn osc99(&mut self, rest: &str) -> Option<Notification> {
        let (meta, payload) = rest.split_once(';').unwrap_or((rest, ""));
        let mut id = String::new();
        let mut done = true;
        let mut is_body = false;
        let mut base64 = false;
        for pair in meta.split(':') {
            match pair.split_once('=') {
                Some(("i", v)) => id = v.to_owned(),
                Some(("d", v)) => done = v != "0",
                Some(("p", v)) => {
                    // Other payload kinds (buttons, icons, queries) aren't shown.
                    match v {
                        "title" => is_body = false,
                        "body" => is_body = true,
                        _ => return None,
                    }
                }
                Some(("e", v)) => base64 = v == "1",
                _ => {}
            }
        }
        let text = if base64 {
            String::from_utf8_lossy(&decode_base64(payload)?).into_owned()
        } else {
            payload.to_owned()
        };
        let entry = self.kitty.entry(id.clone()).or_default();
        if is_body {
            entry.1.push_str(&text);
        } else {
            entry.0.push_str(&text);
        }
        if !done {
            // Bound what an unfinished notification can hold.
            if entry.0.len() + entry.1.len() > MAX_OSC {
                self.kitty.remove(&id);
            }
            return None;
        }
        let (title, body) = self.kitty.remove(&id)?;
        // kitty shows the title alone when there's no body.
        let (title, body) = if body.is_empty() {
            (None, title)
        } else {
            (Some(title).filter(|t| !t.is_empty()), body)
        };
        notification(title, &body)
    }
}

fn osc9(rest: &str) -> Option<Notification> {
    // ConEmu sub-commands: a number, then `;` or nothing.
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    if digits > 0 && matches!(rest.as_bytes().get(digits), None | Some(b';')) {
        return None;
    }
    notification(None, rest)
}

fn osc777(rest: &str) -> Option<Notification> {
    let mut parts = rest.splitn(3, ';');
    if parts.next()? != "notify" {
        return None;
    }
    let title = parts.next().unwrap_or("");
    let body = parts.next().unwrap_or("");
    if body.is_empty() {
        notification(None, title)
    } else {
        notification(Some(title.to_owned()).filter(|t| !t.is_empty()), body)
    }
}

/// A notification with cleaned-up text, if there's anything to show.
fn notification(title: Option<String>, body: &str) -> Option<Notification> {
    let body = clean(body);
    let title = title.map(|t| clean(&t)).filter(|t| !t.is_empty());
    if body.is_empty() && title.is_none() {
        return None;
    }
    Some(Notification { title, body })
}

/// Control characters become spaces; the text is trimmed and shortened.
fn clean(text: &str) -> String {
    let text: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let text = text.trim();
    match text.char_indices().nth(MAX_TEXT) {
        Some((cut, _)) => format!("{}…", &text[..cut]),
        None => text.to_owned(),
    }
}

/// Standard base64, padding optional; `None` if invalid.
pub(crate) fn decode_base64(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in text.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => break,
            _ => return None,
        };
        acc = acc << 6 | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan(input: &[u8]) -> Vec<Notification> {
        events(input)
            .into_iter()
            .filter_map(|e| match e {
                OscEvent::Notification(n) => Some(n),
                _ => None,
            })
            .collect()
    }

    fn events(input: &[u8]) -> Vec<OscEvent> {
        let mut found = Vec::new();
        OscScanner::default().scan(input, &mut found);
        found
    }

    #[test]
    fn status_prompt_and_reset() {
        use crate::status::Message;
        assert_eq!(
            events(b"\x1b]7501;?\x1b\\"),
            vec![OscEvent::Status(Message::Query)]
        );
        assert!(matches!(
            events(b"\x1b]7501;state=working\x07")[..],
            [OscEvent::Status(Message::Set { .. })]
        ));
        assert_eq!(
            events(b"\x1b]133;A\x07\x1b]133;B\x07"),
            vec![OscEvent::PromptStart]
        );
        assert_eq!(events(b"x\x1bcy"), vec![OscEvent::Reset]);
    }

    fn note(title: Option<&str>, body: &str) -> Notification {
        Notification {
            title: title.map(str::to_owned),
            body: body.to_owned(),
        }
    }

    #[test]
    fn osc9_with_bel_or_st() {
        assert_eq!(
            scan(b"x\x1b]9;Task done\x07y"),
            vec![note(None, "Task done")]
        );
        assert_eq!(scan(b"\x1b]9;Waiting\x1b\\"), vec![note(None, "Waiting")]);
    }

    #[test]
    fn conemu_subcommands_are_not_notifications() {
        assert!(scan(b"\x1b]9;4;1;50\x07").is_empty(), "progress");
        assert!(scan(b"\x1b]9;9;C:\\\x07").is_empty(), "working directory");
        assert!(scan(b"\x1b]9;4\x07").is_empty());
        // A number that's part of a sentence is a message.
        assert_eq!(
            scan(b"\x1b]9;3 tests failed\x07"),
            vec![note(None, "3 tests failed")]
        );
    }

    #[test]
    fn osc777_notify() {
        assert_eq!(
            scan(b"\x1b]777;notify;Claude Code;Needs your permission\x07"),
            vec![note(Some("Claude Code"), "Needs your permission")]
        );
        // The body may contain semicolons.
        assert_eq!(
            scan(b"\x1b]777;notify;T;a;b\x1b\\"),
            vec![note(Some("T"), "a;b")]
        );
        assert!(scan(b"\x1b]777;other;x\x07").is_empty());
    }

    #[test]
    fn osc99_simple_and_in_parts() {
        assert_eq!(scan(b"\x1b]99;;Hello\x1b\\"), vec![note(None, "Hello")]);
        let parts = b"\x1b]99;i=1:d=0;Build\x1b\\\x1b]99;i=1:p=body;finished in 3s\x1b\\";
        assert_eq!(scan(parts), vec![note(Some("Build"), "finished in 3s")]);
        // Base64 payloads ("SGk=" is "Hi").
        assert_eq!(scan(b"\x1b]99;e=1;SGk=\x1b\\"), vec![note(None, "Hi")]);
        // Other payload kinds are ignored.
        assert!(scan(b"\x1b]99;p=icon;abc\x1b\\").is_empty());
    }

    #[test]
    fn split_reads_and_other_sequences() {
        let mut scanner = OscScanner::default();
        let mut found = Vec::new();
        for chunk in [
            &b"\x1b[31mred\x1b]0;title\x07\x1b]9;Do"[..],
            b"ne\x1b",
            b"\\",
        ] {
            scanner.scan(chunk, &mut found);
        }
        assert_eq!(found, vec![OscEvent::Notification(note(None, "Done"))]);
    }

    #[test]
    fn text_is_cleaned_and_bounded() {
        assert_eq!(scan(b"\x1b]9;  a\tb  \x07"), vec![note(None, "a b")]);
        let long = format!("\x1b]9;{}\x07", "x".repeat(1000));
        assert_eq!(scan(long.as_bytes())[0].body.chars().count(), MAX_TEXT + 1);
        let huge = format!("\x1b]9;{}\x07", "x".repeat(MAX_OSC + 10));
        assert!(scan(huge.as_bytes()).is_empty());
        assert!(scan(b"\x1b]9;   \x07").is_empty());
    }
}
