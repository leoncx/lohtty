//! Keyboard and paste → xterm byte sequences.

use alacritty_terminal::term::TermMode;

/// A key press, independent of the UI toolkit.
#[derive(Debug, Clone, Copy, Default)]
pub struct KeyInput<'a> {
    /// Key name, gpui-style: "a", "enter", "left", "f5", ...
    pub key: &'a str,
    /// The text the key would produce with the current layout, if any.
    pub key_char: Option<&'a str>,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
}

impl KeyInput<'_> {
    /// xterm modifier parameter: 1 + shift + 2*alt + 4*ctrl.
    fn modifier_param(&self) -> u8 {
        1 + self.shift as u8 + 2 * self.alt as u8 + 4 * self.ctrl as u8
    }

    fn has_mods(&self) -> bool {
        self.ctrl || self.alt || self.shift
    }
}

/// True when the key just produces text. Such keys are left to the platform's
/// text input path (WM_CHAR / IME), which also handles dead keys, AltGr and
/// injected characters.
pub fn is_text_key(k: &KeyInput) -> bool {
    let produces_text = k.key_char.is_some_and(|t| !t.is_empty());
    let altgr = k.ctrl
        && k.alt
        && k.key_char
            .is_some_and(|t| !t.is_empty() && !t.eq_ignore_ascii_case(k.key));
    let plain = !k.ctrl && !k.alt && produces_text;
    (plain || altgr) && named_key(k, TermMode::empty()).is_none()
}

/// Encodes a key press for the PTY. Returns `None` if the key produces nothing.
pub fn encode_key(k: &KeyInput, mode: TermMode) -> Option<Vec<u8>> {
    if let Some(seq) = named_key(k, mode) {
        return Some(seq);
    }

    // AltGr arrives as Ctrl+Alt on Windows: if it produced a different
    // character (e.g. AZERTY AltGr+0 = '@'), send that character.
    if k.ctrl
        && k.alt
        && let Some(text) = k.key_char
        && !text.is_empty()
        && !text.eq_ignore_ascii_case(k.key)
    {
        return Some(text.as_bytes().to_vec());
    }

    if k.ctrl
        && let Some(byte) = ctrl_byte(k.key)
    {
        return Some(alt_prefixed(k.alt, &[byte]));
    }

    let text = k.key_char.filter(|t| !t.is_empty())?;
    Some(alt_prefixed(k.alt, text.as_bytes()))
}

fn alt_prefixed(alt: bool, bytes: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(bytes.len() + 1);
    if alt {
        v.push(0x1b);
    }
    v.extend_from_slice(bytes);
    v
}

fn ctrl_byte(key: &str) -> Option<u8> {
    let mut chars = key.chars();
    let c = chars.next()?;
    if chars.next().is_some() {
        return if key == "space" { Some(0) } else { None };
    }
    Some(match c.to_ascii_lowercase() {
        c @ 'a'..='z' => c as u8 - b'a' + 1,
        '@' | '2' => 0x00,
        '[' | '3' => 0x1b,
        '\\' | '4' => 0x1c,
        ']' | '5' => 0x1d,
        '^' | '6' => 0x1e,
        '_' | '-' | '7' | '/' => 0x1f,
        '8' | '?' => 0x7f,
        _ => return None,
    })
}

fn named_key(k: &KeyInput, mode: TermMode) -> Option<Vec<u8>> {
    let m = k.modifier_param();
    let app_cursor = mode.contains(TermMode::APP_CURSOR);

    // CSI 1;m X  (or SS3 X / CSI X without modifiers)
    let cursor = |x: char| -> Vec<u8> {
        if k.has_mods() {
            format!("\x1b[1;{m}{x}").into_bytes()
        } else if app_cursor {
            format!("\x1bO{x}").into_bytes()
        } else {
            format!("\x1b[{x}").into_bytes()
        }
    };
    // CSI n ~  /  CSI n;m ~
    let tilde = |n: u8| -> Vec<u8> {
        if k.has_mods() {
            format!("\x1b[{n};{m}~").into_bytes()
        } else {
            format!("\x1b[{n}~").into_bytes()
        }
    };
    // F1-F4: SS3 X  /  CSI 1;m X
    let ss3 = |x: char| -> Vec<u8> {
        if k.has_mods() {
            format!("\x1b[1;{m}{x}").into_bytes()
        } else {
            format!("\x1bO{x}").into_bytes()
        }
    };

    Some(match k.key {
        "enter" => alt_prefixed(k.alt, b"\r"),
        "tab" if k.shift => b"\x1b[Z".to_vec(),
        "tab" => alt_prefixed(k.alt, b"\t"),
        "escape" => alt_prefixed(k.alt, b"\x1b"),
        "backspace" if k.ctrl => alt_prefixed(k.alt, b"\x08"),
        "backspace" => alt_prefixed(k.alt, b"\x7f"),
        "up" => cursor('A'),
        "down" => cursor('B'),
        "right" => cursor('C'),
        "left" => cursor('D'),
        "home" => cursor('H'),
        "end" => cursor('F'),
        "insert" => tilde(2),
        "delete" => tilde(3),
        "pageup" => tilde(5),
        "pagedown" => tilde(6),
        "f1" => ss3('P'),
        "f2" => ss3('Q'),
        "f3" => ss3('R'),
        "f4" => ss3('S'),
        "f5" => tilde(15),
        "f6" => tilde(17),
        "f7" => tilde(18),
        "f8" => tilde(19),
        "f9" => tilde(20),
        "f10" => tilde(21),
        "f11" => tilde(23),
        "f12" => tilde(24),
        _ => return None,
    })
}

/// Prepares clipboard text for the PTY: newlines become carriage returns (as
/// if typed), and in bracketed-paste mode the text is wrapped in
/// `ESC[200~ … ESC[201~` with any embedded ESC removed, so pasted content can
/// never end the bracket early or inject escape sequences.
pub fn encode_paste(text: &str, mode: TermMode) -> Vec<u8> {
    let normalized = text.replace("\r\n", "\r").replace('\n', "\r");
    if mode.contains(TermMode::BRACKETED_PASTE) {
        let mut out = b"\x1b[200~".to_vec();
        out.extend(normalized.bytes().filter(|&b| b != 0x1b));
        out.extend_from_slice(b"\x1b[201~");
        out
    } else {
        normalized.into_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paste_normalizes_newlines() {
        assert_eq!(encode_paste("a\r\nb\nc", TermMode::empty()), b"a\rb\rc");
    }

    #[test]
    fn bracketed_paste_wraps_and_strips_escapes() {
        let out = encode_paste("ls\n\x1b[201~rm -rf /", TermMode::BRACKETED_PASTE);
        assert_eq!(out, b"\x1b[200~ls\r[201~rm -rf /\x1b[201~");
    }

    fn enc(k: KeyInput) -> Vec<u8> {
        encode_key(&k, TermMode::empty()).unwrap()
    }

    fn key(key: &str) -> KeyInput<'_> {
        KeyInput {
            key,
            key_char: Some(key),
            ..Default::default()
        }
    }

    #[test]
    fn plain_text() {
        assert_eq!(enc(key("a")), b"a");
        let k = KeyInput {
            key: "e",
            key_char: Some("é"),
            ..Default::default()
        };
        assert_eq!(enc(k), "é".as_bytes());
    }

    #[test]
    fn ctrl_letters() {
        assert_eq!(
            enc(KeyInput {
                ctrl: true,
                ..key("c")
            }),
            b"\x03"
        );
        assert_eq!(
            enc(KeyInput {
                ctrl: true,
                ..key("space")
            }),
            b"\x00"
        );
    }

    #[test]
    fn alt_prefixes_escape() {
        assert_eq!(
            enc(KeyInput {
                alt: true,
                ..key("x")
            }),
            b"\x1bx"
        );
    }

    #[test]
    fn altgr_produces_character() {
        let k = KeyInput {
            key: "0",
            key_char: Some("@"),
            ctrl: true,
            alt: true,
            shift: false,
        };
        assert_eq!(enc(k), b"@");
    }

    #[test]
    fn text_keys_are_left_to_text_input() {
        assert!(is_text_key(&key("a")));
        assert!(is_text_key(&KeyInput {
            key: "0",
            key_char: Some("@"),
            ctrl: true,
            alt: true,
            shift: false,
        }));
        assert!(!is_text_key(&KeyInput {
            ctrl: true,
            ..key("c")
        }));
        assert!(!is_text_key(&KeyInput {
            alt: true,
            ..key("x")
        }));
        assert!(!is_text_key(&KeyInput {
            key: "enter",
            key_char: Some("\r"),
            ..Default::default()
        }));
    }

    #[test]
    fn arrows_respect_app_cursor_and_modifiers() {
        let up = KeyInput {
            key: "up",
            ..Default::default()
        };
        assert_eq!(enc(up), b"\x1b[A");
        assert_eq!(encode_key(&up, TermMode::APP_CURSOR).unwrap(), b"\x1bOA");
        assert_eq!(enc(KeyInput { ctrl: true, ..up }), b"\x1b[1;5A");
    }

    #[test]
    fn special_keys() {
        let named = |key| KeyInput {
            key,
            ..Default::default()
        };
        assert_eq!(enc(named("enter")), b"\r");
        assert_eq!(enc(named("backspace")), b"\x7f");
        assert_eq!(enc(named("delete")), b"\x1b[3~");
        assert_eq!(enc(named("f1")), b"\x1bOP");
        assert_eq!(enc(named("f5")), b"\x1b[15~");
        assert_eq!(
            enc(KeyInput {
                shift: true,
                ..named("tab")
            }),
            b"\x1b[Z"
        );
    }
}
