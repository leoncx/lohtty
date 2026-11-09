//! Resolves terminal colors (named / indexed / truecolor) against the theme.

use alacritty_terminal::term::color::Colors;
use alacritty_terminal::vte::ansi::{Color, NamedColor, Rgb as TermRgb};
use gpui::Rgba;
use lohtty_theme::{Rgb, Theme};

pub fn to_rgba(c: Rgb) -> Rgba {
    gpui::rgb(c.to_hex())
}

fn from_term(c: TermRgb) -> Rgb {
    Rgb {
        r: c.r,
        g: c.g,
        b: c.b,
    }
}

/// Linear blend from `a` (t = 0) to `b` (t = 1).
pub fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Rgb {
        r: m(a.r, b.r),
        g: m(a.g, b.g),
        b: m(a.b, b.b),
    }
}

/// Mixes `fg` toward `bg` (used for dim text).
pub fn dim(fg: Rgb, bg: Rgb) -> Rgb {
    let mix = |a: u8, b: u8| ((a as u16 * 2 + b as u16) / 3) as u8;
    Rgb {
        r: mix(fg.r, bg.r),
        g: mix(fg.g, bg.g),
        b: mix(fg.b, bg.b),
    }
}

/// The xterm 256-color palette entry `index` (16..=255); 0..=15 come from the theme.
fn xterm_256(index: u8, theme: &Theme) -> Rgb {
    match index {
        0..=15 => theme.ansi[index as usize],
        16..=231 => {
            const STEPS: [u8; 6] = [0, 95, 135, 175, 215, 255];
            let i = index - 16;
            Rgb {
                r: STEPS[(i / 36) as usize],
                g: STEPS[((i / 6) % 6) as usize],
                b: STEPS[(i % 6) as usize],
            }
        }
        232..=255 => {
            let v = 8 + 10 * (index - 232);
            Rgb { r: v, g: v, b: v }
        }
    }
}

/// Resolves a palette slot (0-255, or a `NamedColor` discriminant) to a color,
/// honoring runtime overrides set by applications (OSC 4/10/11/12).
pub fn palette(index: usize, overrides: &Colors, theme: &Theme) -> Rgb {
    if let Some(c) = overrides[index] {
        return from_term(c);
    }
    const FOREGROUND: usize = NamedColor::Foreground as usize;
    const BACKGROUND: usize = NamedColor::Background as usize;
    const CURSOR: usize = NamedColor::Cursor as usize;
    const DIM_BLACK: usize = NamedColor::DimBlack as usize;
    const DIM_WHITE: usize = NamedColor::DimWhite as usize;
    const BRIGHT_FG: usize = NamedColor::BrightForeground as usize;
    const DIM_FG: usize = NamedColor::DimForeground as usize;

    match index {
        0..=255 => xterm_256(index as u8, theme),
        FOREGROUND | BRIGHT_FG => theme.foreground,
        BACKGROUND => theme.background,
        CURSOR => theme.cursor,
        DIM_BLACK..=DIM_WHITE => dim(theme.ansi[index - DIM_BLACK], theme.background),
        DIM_FG => dim(theme.foreground, theme.background),
        _ => theme.foreground,
    }
}

pub fn resolve(color: Color, overrides: &Colors, theme: &Theme) -> Rgb {
    match color {
        Color::Spec(c) => from_term(c),
        Color::Indexed(i) => palette(i as usize, overrides, theme),
        Color::Named(n) => palette(n as usize, overrides, theme),
    }
}

/// Converts a theme color into alacritty's color type (for OSC color replies).
pub fn to_term(c: Rgb) -> TermRgb {
    TermRgb {
        r: c.r,
        g: c.g,
        b: c.b,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cube_and_grayscale() {
        let t = Theme::default();
        assert_eq!(xterm_256(16, &t), Rgb::from_hex(0x000000));
        assert_eq!(xterm_256(231, &t), Rgb::from_hex(0xffffff));
        assert_eq!(xterm_256(196, &t), Rgb::from_hex(0xff0000));
        assert_eq!(xterm_256(232, &t), Rgb::from_hex(0x080808));
    }

    #[test]
    fn named_uses_theme() {
        let t = Theme::default();
        let c = Colors::default();
        assert_eq!(
            resolve(Color::Named(NamedColor::Background), &c, &t),
            t.background
        );
        assert_eq!(resolve(Color::Named(NamedColor::Red), &c, &t), t.ansi[1]);
    }
}
