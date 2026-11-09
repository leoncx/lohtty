//! Mouse events → xterm mouse reports (modes 1000/1002/1003, encodings 1005/1006).

use alacritty_terminal::term::TermMode;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Middle,
    Right,
    WheelUp,
    WheelDown,
}

impl MouseButton {
    fn is_wheel(self) -> bool {
        matches!(self, Self::WheelUp | Self::WheelDown)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseAction {
    Press,
    Release,
    Motion,
}

/// A mouse event at a grid cell (0-based).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MouseReport {
    /// `None` for motion with no button held.
    pub button: Option<MouseButton>,
    pub action: MouseAction,
    pub col: usize,
    pub row: usize,
    pub shift: bool,
    pub alt: bool,
    pub ctrl: bool,
}

/// True if the application wants mouse events (and the user isn't holding
/// Shift, which by convention forces local selection).
pub fn wants_mouse(mode: TermMode, shift: bool) -> bool {
    mode.intersects(TermMode::MOUSE_MODE) && !shift
}

/// Encodes a mouse event for the PTY, or `None` if the current mode doesn't
/// report it (or the position can't be encoded).
pub fn encode_mouse(r: &MouseReport, mode: TermMode) -> Option<Vec<u8>> {
    if !mode.intersects(TermMode::MOUSE_MODE) {
        return None;
    }
    if r.action == MouseAction::Motion {
        let allowed = if r.button.is_some() {
            mode.intersects(TermMode::MOUSE_DRAG | TermMode::MOUSE_MOTION)
        } else {
            mode.contains(TermMode::MOUSE_MOTION)
        };
        if !allowed {
            return None;
        }
    }
    if r.action == MouseAction::Release && r.button.is_some_and(MouseButton::is_wheel) {
        return None;
    }

    let button_code: u8 = match r.button {
        Some(MouseButton::Left) => 0,
        Some(MouseButton::Middle) => 1,
        Some(MouseButton::Right) => 2,
        Some(MouseButton::WheelUp) => 64,
        Some(MouseButton::WheelDown) => 65,
        None => 3,
    };
    let modifiers = 4 * r.shift as u8 + 8 * r.alt as u8 + 16 * r.ctrl as u8;
    let motion = if r.action == MouseAction::Motion {
        32
    } else {
        0
    };

    if mode.contains(TermMode::SGR_MOUSE) {
        let code = button_code + modifiers + motion;
        let suffix = if r.action == MouseAction::Release {
            'm'
        } else {
            'M'
        };
        return Some(format!("\x1b[<{code};{};{}{suffix}", r.col + 1, r.row + 1).into_bytes());
    }

    // Legacy encoding: releases don't say which button was released.
    let button_code = if r.action == MouseAction::Release {
        3
    } else {
        button_code
    };
    let code = 32 + button_code + modifiers + motion;
    let utf8 = mode.contains(TermMode::UTF8_MOUSE);
    let mut out = vec![0x1b, b'[', b'M', code];
    encode_coord(r.col, utf8, &mut out)?;
    encode_coord(r.row, utf8, &mut out)?;
    Some(out)
}

fn encode_coord(pos: usize, utf8: bool, out: &mut Vec<u8>) -> Option<()> {
    let value = pos + 1 + 32;
    if utf8 {
        // Mode 1005: values above 127 are sent as UTF-8 (up to 2047).
        let c = char::from_u32(u32::try_from(value).ok()?).filter(|_| value <= 2047)?;
        let mut buf = [0; 4];
        out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
    } else {
        out.push(u8::try_from(value).ok()?);
    }
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(button: Option<MouseButton>, action: MouseAction) -> MouseReport {
        MouseReport {
            button,
            action,
            col: 4,
            row: 2,
            shift: false,
            alt: false,
            ctrl: false,
        }
    }

    const CLICK: TermMode = TermMode::MOUSE_REPORT_CLICK;

    #[test]
    fn nothing_without_mouse_mode() {
        let r = report(Some(MouseButton::Left), MouseAction::Press);
        assert_eq!(encode_mouse(&r, TermMode::empty()), None);
    }

    #[test]
    fn sgr_press_and_release() {
        let mode = CLICK | TermMode::SGR_MOUSE;
        let press = report(Some(MouseButton::Left), MouseAction::Press);
        assert_eq!(encode_mouse(&press, mode).unwrap(), b"\x1b[<0;5;3M");
        let release = report(Some(MouseButton::Right), MouseAction::Release);
        assert_eq!(encode_mouse(&release, mode).unwrap(), b"\x1b[<2;5;3m");
    }

    #[test]
    fn sgr_modifiers_and_wheel() {
        let mode = CLICK | TermMode::SGR_MOUSE;
        let r = MouseReport {
            ctrl: true,
            ..report(Some(MouseButton::WheelUp), MouseAction::Press)
        };
        assert_eq!(encode_mouse(&r, mode).unwrap(), b"\x1b[<80;5;3M");
        let up = report(Some(MouseButton::WheelUp), MouseAction::Release);
        assert_eq!(encode_mouse(&up, mode), None);
    }

    #[test]
    fn legacy_encoding() {
        let press = report(Some(MouseButton::Left), MouseAction::Press);
        assert_eq!(encode_mouse(&press, CLICK).unwrap(), b"\x1b[M %#");
        let release = report(Some(MouseButton::Left), MouseAction::Release);
        assert_eq!(encode_mouse(&release, CLICK).unwrap(), b"\x1b[M#%#");
    }

    #[test]
    fn legacy_out_of_range_is_dropped_unless_utf8() {
        let far = MouseReport {
            col: 300,
            ..report(Some(MouseButton::Left), MouseAction::Press)
        };
        assert_eq!(encode_mouse(&far, CLICK), None);
        let utf8 = encode_mouse(&far, CLICK | TermMode::UTF8_MOUSE).unwrap();
        assert_eq!(&utf8[..4], b"\x1b[M ");
        assert_eq!(
            std::str::from_utf8(&utf8[4..6]).unwrap(),
            char::from_u32(333).unwrap().to_string()
        );
    }

    #[test]
    fn motion_depends_on_mode() {
        let drag = report(Some(MouseButton::Left), MouseAction::Motion);
        let hover = report(None, MouseAction::Motion);
        let sgr = TermMode::SGR_MOUSE;
        assert_eq!(encode_mouse(&drag, CLICK | sgr), None);
        assert_eq!(
            encode_mouse(&drag, TermMode::MOUSE_DRAG | sgr).unwrap(),
            b"\x1b[<32;5;3M"
        );
        assert_eq!(encode_mouse(&hover, TermMode::MOUSE_DRAG | sgr), None);
        assert_eq!(
            encode_mouse(&hover, TermMode::MOUSE_MOTION | sgr).unwrap(),
            b"\x1b[<35;5;3M"
        );
    }

    #[test]
    fn shift_forces_local_selection() {
        assert!(wants_mouse(CLICK, false));
        assert!(!wants_mouse(CLICK, true));
        assert!(!wants_mouse(TermMode::empty(), false));
    }
}
