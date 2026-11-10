//! Private modes lohtty handles itself, outside alacritty's parser.
//!
//! Win32 input mode (`CSI ? 9001 h`): Windows' ConPTY asks for it at
//! startup. While it's on, ConPTY also accepts key events encoded as
//! `CSI Vk ; Sc ; Uc ; Kd ; Cs ; Rc _`, which carry what plain VT input
//! can't, such as Shift with Enter, to console programs.

/// The win32-input-mode number.
const WIN32_INPUT_MODE: &str = "9001";
/// Longest CSI parameter string kept.
const MAX_PARAMS: usize = 64;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
enum State {
    #[default]
    Normal,
    Esc,
    Csi,
}

/// Watches PTY output for win32-input-mode being set or reset, across reads.
#[derive(Default)]
pub struct ModeWatcher {
    state: State,
    params: Vec<u8>,
}

impl ModeWatcher {
    /// Scans `input`; returns the mode's last new state in it, if it changed.
    pub fn scan(&mut self, input: &[u8]) -> Option<bool> {
        let mut change = None;
        for &b in input {
            match self.state {
                State::Normal => {
                    if b == 0x1b {
                        self.state = State::Esc;
                    }
                }
                State::Esc => {
                    self.state = match b {
                        b'[' => {
                            self.params.clear();
                            State::Csi
                        }
                        0x1b => State::Esc,
                        _ => State::Normal,
                    };
                }
                State::Csi => match b {
                    // Parameters and intermediates.
                    0x20..=0x3f => {
                        if self.params.len() < MAX_PARAMS {
                            self.params.push(b);
                        }
                    }
                    // Final byte.
                    0x40..=0x7e => {
                        if (b == b'h' || b == b'l')
                            && let Some(list) = self.params.strip_prefix(b"?")
                            && list
                                .split(|&c| c == b';')
                                .any(|p| p == WIN32_INPUT_MODE.as_bytes())
                        {
                            change = Some(b == b'h');
                        }
                        self.state = State::Normal;
                    }
                    0x1b => self.state = State::Esc,
                    _ => self.state = State::Normal,
                },
            }
        }
        change
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sees_set_and_reset_across_reads() {
        let mut w = ModeWatcher::default();
        // ConPTY's startup output.
        assert_eq!(w.scan(b"\x1b[6n\x1b[c\x1b[?1004h\x1b[?9001h"), Some(true));
        assert_eq!(w.scan(b"text \x1b[31m red"), None);
        assert_eq!(w.scan(b"\x1b[?90"), None);
        assert_eq!(w.scan(b"01l"), Some(false));
        // Within a list of modes.
        assert_eq!(w.scan(b"\x1b[?1;9001;25h"), Some(true));
        // Not a private mode, or another one.
        assert_eq!(w.scan(b"\x1b[9001h\x1b[?90011l"), None);
    }
}
