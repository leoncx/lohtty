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
    if mode.contains(TermMode::DISAMBIGUATE_ESC_CODES)
        && let Some(seq) = kitty_key(k)
    {
        return Some(seq);
    }
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

/// The kitty keyboard protocol's "disambiguate escape codes" level (flag 1),
/// which programs such as Claude Code and Codex request: keys whose legacy
/// encoding is ambiguous are sent as `CSI code ; modifiers u`. That's Escape,
/// Enter, Tab and Backspace with modifiers (so Shift+Enter differs from
/// Enter), and Ctrl or Alt with a character key. Other keys keep their legacy
/// encodings, which already carry modifiers.
fn kitty_key(k: &KeyInput) -> Option<Vec<u8>> {
    let mods = k.modifier_param();
    let csi_u = |code: u32| {
        if mods == 1 {
            format!("\x1b[{code}u").into_bytes()
        } else {
            format!("\x1b[{code};{mods}u").into_bytes()
        }
    };
    match k.key {
        "escape" => Some(csi_u(27)),
        "enter" if k.has_mods() => Some(csi_u(13)),
        "tab" if k.has_mods() => Some(csi_u(9)),
        "backspace" if k.has_mods() => Some(csi_u(127)),
        _ if !(k.ctrl || k.alt) => None,
        // AltGr (Ctrl+Alt on Windows) typing a character stays text.
        _ if k.ctrl
            && k.alt
            && k.key_char
                .is_some_and(|t| !t.is_empty() && !t.eq_ignore_ascii_case(k.key)) =>
        {
            None
        }
        "space" => Some(csi_u(32)),
        key => {
            // The key's own (unshifted) character, lower case.
            let mut chars = key.chars();
            let c = chars.next()?;
            if chars.next().is_some() {
                return None;
            }
            let c = c.to_lowercase().next()?;
            Some(csi_u(c as u32))
        }
    }
}

/// Encodes a key for ConPTY's win32 input mode, when it carries something
/// plain VT input can't: Enter with Shift, Ctrl or Alt (so console programs
/// see Shift+Enter, as in Windows Terminal). A key-down and a key-up record,
/// `CSI Vk ; Sc ; Uc ; Kd ; Cs ; Rc _`. `None` for every other key.
pub fn encode_win32(k: &KeyInput) -> Option<Vec<u8>> {
    if k.key != "enter" || !k.has_mods() {
        return None;
    }
    const VK_RETURN: u32 = 0x0d;
    const SHIFT_PRESSED: u32 = 0x10;
    const LEFT_CTRL_PRESSED: u32 = 0x08;
    const LEFT_ALT_PRESSED: u32 = 0x02;
    let state = if k.shift { SHIFT_PRESSED } else { 0 }
        | if k.ctrl { LEFT_CTRL_PRESSED } else { 0 }
        | if k.alt { LEFT_ALT_PRESSED } else { 0 };
    // Ctrl+Enter types a line feed, as on a Windows console.
    let ch = if k.ctrl { 0x0a } else { 0x0d };
    let scan = scan_code(VK_RETURN);
    let record = |down: u8| format!("\x1b[{VK_RETURN};{scan};{ch};{down};{state};1_");
    Some(format!("{}{}", record(1), record(0)).into_bytes())
}

/// The scan code of a virtual key with the current keyboard layout.
fn scan_code(vk: u32) -> u32 {
    #[cfg(windows)]
    {
        #[link(name = "user32")]
        unsafe extern "system" {
            fn MapVirtualKeyW(code: u32, map_type: u32) -> u32;
        }
        const MAPVK_VK_TO_VSC: u32 = 0;
        // SAFETY: a pure lookup in the keyboard layout.
        let scan = unsafe { MapVirtualKeyW(vk, MAPVK_VK_TO_VSC) };
        if scan != 0 {
            return scan;
        }
    }
    // Enter's scan code on every PC keyboard.
    if vk == 0x0d { 0x1c } else { 0 }
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

    fn kitty(k: KeyInput) -> Vec<u8> {
        encode_key(&k, TermMode::DISAMBIGUATE_ESC_CODES).unwrap()
    }

    fn named(key: &str, shift: bool, ctrl: bool, alt: bool) -> KeyInput<'_> {
        KeyInput {
            key,
            key_char: None,
            shift,
            ctrl,
            alt,
        }
    }

    #[test]
    fn kitty_disambiguates_modified_keys() {
        // Shift+Enter differs from Enter; plain Enter stays a carriage return.
        assert_eq!(kitty(named("enter", true, false, false)), b"\x1b[13;2u");
        assert_eq!(kitty(named("enter", false, true, false)), b"\x1b[13;5u");
        assert_eq!(kitty(named("enter", false, false, false)), b"\r");
        assert_eq!(kitty(named("escape", false, false, false)), b"\x1b[27u");
        assert_eq!(kitty(named("tab", true, false, false)), b"\x1b[9;2u");
        assert_eq!(
            kitty(named("backspace", false, false, true)),
            b"\x1b[127;3u"
        );
        // Ctrl/Alt with a character: its unshifted code point.
        let mut ctrl_a = key("a");
        ctrl_a.ctrl = true;
        assert_eq!(kitty(ctrl_a), b"\x1b[97;5u");
        let mut ctrl_shift_a = key("a");
        ctrl_shift_a.ctrl = true;
        ctrl_shift_a.shift = true;
        assert_eq!(kitty(ctrl_shift_a), b"\x1b[97;6u");
        let mut alt_space = key("space");
        alt_space.alt = true;
        assert_eq!(kitty(alt_space), b"\x1b[32;3u");
        // Arrows keep their legacy form.
        assert_eq!(kitty(named("up", true, false, false)), b"\x1b[1;2A");
        // AltGr text stays text (AZERTY AltGr+0 = '@').
        let altgr = KeyInput {
            key: "0",
            key_char: Some("@"),
            ctrl: true,
            alt: true,
            shift: false,
        };
        assert_eq!(kitty(altgr), b"@");
    }

    #[test]
    fn win32_input_mode_carries_modified_enter() {
        let shift_enter = encode_win32(&named("enter", true, false, false)).unwrap();
        let text = String::from_utf8(shift_enter).unwrap();
        // Key down then key up, Vk 13, character 13, Shift pressed (0x10).
        let records: Vec<&str> = text.split_terminator('_').collect();
        assert_eq!(records.len(), 2);
        let fields = |r: &str| -> Vec<u32> {
            r.trim_start_matches("\x1b[")
                .split(';')
                .map(|f| f.parse().unwrap())
                .collect()
        };
        let (down, up) = (fields(records[0]), fields(records[1]));
        assert_eq!(
            (down[0], down[2], down[3], down[4], down[5]),
            (13, 13, 1, 0x10, 1)
        );
        assert_eq!((up[3], up[4]), (0, 0x10));
        assert!(down[1] != 0, "scan code");
        // Ctrl+Enter is a line feed with Ctrl pressed.
        let ctrl =
            String::from_utf8(encode_win32(&named("enter", false, true, false)).unwrap()).unwrap();
        assert_eq!(
            fields(ctrl.split_terminator('_').next().unwrap())[2..5],
            [10, 1, 0x08]
        );
        // Plain Enter and other keys keep the usual encoding.
        assert!(encode_win32(&named("enter", false, false, false)).is_none());
        assert!(encode_win32(&named("tab", true, false, false)).is_none());
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
