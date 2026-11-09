//! Color themes. UI-agnostic: colors are plain RGB, converted by `lohtty-ui`.

/// An sRGB color.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const fn from_hex(hex: u32) -> Self {
        Self {
            r: (hex >> 16) as u8,
            g: (hex >> 8) as u8,
            b: hex as u8,
        }
    }

    pub const fn to_hex(self) -> u32 {
        ((self.r as u32) << 16) | ((self.g as u32) << 8) | self.b as u32
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Theme {
    pub name: &'static str,
    pub foreground: Rgb,
    pub background: Rgb,
    pub cursor: Rgb,
    pub selection: Rgb,
    /// ANSI 0-15: black, red, green, yellow, blue, magenta, cyan, white, then bright variants.
    pub ansi: [Rgb; 16],
}

impl Default for Theme {
    fn default() -> Self {
        Self::tokyo_night()
    }
}

impl Theme {
    /// Tokyo Night (night variant).
    pub const fn tokyo_night() -> Self {
        const fn c(hex: u32) -> Rgb {
            Rgb::from_hex(hex)
        }
        Self {
            name: "tokyo-night",
            foreground: c(0xc0caf5),
            background: c(0x1a1b26),
            cursor: c(0xc0caf5),
            selection: c(0x283457),
            ansi: [
                c(0x15161e),
                c(0xf7768e),
                c(0x9ece6a),
                c(0xe0af68),
                c(0x7aa2f7),
                c(0xbb9af7),
                c(0x7dcfff),
                c(0xa9b1d6),
                c(0x414868),
                c(0xf7768e),
                c(0x9ece6a),
                c(0xe0af68),
                c(0x7aa2f7),
                c(0xbb9af7),
                c(0x7dcfff),
                c(0xc0caf5),
            ],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_roundtrip() {
        assert_eq!(Rgb::from_hex(0x1a1b26).to_hex(), 0x1a1b26);
    }

    #[test]
    fn default_is_tokyo_night() {
        assert_eq!(Theme::default().name, "tokyo-night");
    }
}
