//! Color themes, loaded from TOML. UI-agnostic: colors are plain RGB,
//! converted by `lohtty-ui`.
//!
//! A theme is looked up by name: `<user themes dir>/<name>.toml` first, then
//! the bundled themes (`tokyo-night`, `tokyo-night-storm`, `tokyo-night-moon`,
//! `tokyo-night-day`).

use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use serde::Deserialize;

pub const DEFAULT_THEME: &str = "tokyo-night";

const BUNDLED: &[(&str, &str)] = &[
    ("tokyo-night", include_str!("../themes/tokyo-night.toml")),
    (
        "tokyo-night-storm",
        include_str!("../themes/tokyo-night-storm.toml"),
    ),
    (
        "tokyo-night-moon",
        include_str!("../themes/tokyo-night-moon.toml"),
    ),
    (
        "tokyo-night-day",
        include_str!("../themes/tokyo-night-day.toml"),
    ),
];

/// An sRGB color, written `"#rrggbb"` in theme files.
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

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid color `{0}`, expected \"#rrggbb\"")]
pub struct ParseColorError(String);

impl FromStr for Rgb {
    type Err = ParseColorError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let hex = s.strip_prefix('#').unwrap_or(s);
        if hex.len() != 6 {
            return Err(ParseColorError(s.to_owned()));
        }
        u32::from_str_radix(hex, 16)
            .map(Rgb::from_hex)
            .map_err(|_| ParseColorError(s.to_owned()))
    }
}

impl fmt::Display for Rgb {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{:06x}", self.to_hex())
    }
}

impl<'de> Deserialize<'de> for Rgb {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Theme {
    pub name: String,
    pub foreground: Rgb,
    pub background: Rgb,
    pub cursor: Rgb,
    pub selection: Rgb,
    /// ANSI 0-15: black, red, green, yellow, blue, magenta, cyan, white, then bright variants.
    pub ansi: [Rgb; 16],
}

#[derive(Debug, thiserror::Error)]
pub enum ThemeError {
    #[error("theme `{0}` not found (bundled themes: {list})", list = bundled_names().collect::<Vec<_>>().join(", "))]
    NotFound(String),
    #[error("failed to read theme {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("invalid theme `{name}`: {source}")]
    Parse {
        name: String,
        source: toml::de::Error,
    },
}

impl Default for Theme {
    fn default() -> Self {
        Self::bundled(DEFAULT_THEME).expect("default theme is bundled and valid")
    }
}

/// Names of the themes shipped with lohtty.
pub fn bundled_names() -> impl Iterator<Item = &'static str> {
    BUNDLED.iter().map(|(name, _)| *name)
}

/// Every theme name that [`Theme::load`] accepts: bundled themes first, then
/// user themes (`<user_dir>/<name>.toml`) sorted by name, without duplicates.
pub fn available_names(user_dir: Option<&Path>) -> Vec<String> {
    let mut names: Vec<String> = bundled_names().map(str::to_owned).collect();
    let mut user: Vec<String> = user_dir
        .and_then(|dir| std::fs::read_dir(dir).ok())
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            (path.extension()? == "toml")
                .then(|| path.file_stem()?.to_str().map(str::to_owned))
                .flatten()
        })
        .filter(|name| !names.contains(name))
        .collect();
    user.sort();
    names.append(&mut user);
    names
}

impl Theme {
    pub fn parse(name: &str, src: &str) -> Result<Self, ThemeError> {
        toml::from_str(src).map_err(|source| ThemeError::Parse {
            name: name.to_owned(),
            source,
        })
    }

    /// A bundled theme by name.
    pub fn bundled(name: &str) -> Result<Self, ThemeError> {
        let (_, src) = BUNDLED
            .iter()
            .find(|(n, _)| *n == name)
            .ok_or_else(|| ThemeError::NotFound(name.to_owned()))?;
        Self::parse(name, src)
    }

    /// Loads `name` from `user_dir/<name>.toml` if present, else from the bundled themes.
    pub fn load(name: &str, user_dir: Option<&Path>) -> Result<Self, ThemeError> {
        if let Some(dir) = user_dir {
            let path = dir.join(format!("{name}.toml"));
            match std::fs::read_to_string(&path) {
                Ok(src) => return Self::parse(name, &src),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(source) => return Err(ThemeError::Io { path, source }),
            }
        }
        Self::bundled(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_roundtrip() {
        assert_eq!(Rgb::from_hex(0x1a1b26).to_hex(), 0x1a1b26);
        assert_eq!("#1a1b26".parse::<Rgb>().unwrap(), Rgb::from_hex(0x1a1b26));
        assert_eq!(Rgb::from_hex(0x1a1b26).to_string(), "#1a1b26");
        assert!("#12345".parse::<Rgb>().is_err());
        assert!("#zzzzzz".parse::<Rgb>().is_err());
    }

    #[test]
    fn all_bundled_themes_parse() {
        for name in bundled_names() {
            Theme::bundled(name).unwrap_or_else(|e| panic!("{name}: {e}"));
        }
    }

    #[test]
    fn default_is_tokyo_night() {
        let t = Theme::default();
        assert_eq!(t.name, "Tokyo Night");
        assert_eq!(t.background, Rgb::from_hex(0x1a1b26));
    }

    #[test]
    fn user_dir_overrides_bundled() {
        let dir = std::env::temp_dir().join(format!("lohtty-theme-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let custom = BUNDLED[0].1.replace("Tokyo Night\"", "Custom\"");
        std::fs::write(dir.join("tokyo-night.toml"), custom).unwrap();
        let t = Theme::load("tokyo-night", Some(&dir)).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(t.name, "Custom");
    }

    #[test]
    fn available_names_lists_bundled_then_user_themes() {
        let dir = std::env::temp_dir().join(format!("lohtty-theme-list-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("zebra.toml"), "").unwrap();
        std::fs::write(dir.join("alpha.toml"), "").unwrap();
        std::fs::write(dir.join("tokyo-night.toml"), "").unwrap(); // overrides a bundled one
        std::fs::write(dir.join("notes.txt"), "").unwrap();
        let names = available_names(Some(&dir));
        std::fs::remove_dir_all(&dir).unwrap();

        let bundled: Vec<String> = bundled_names().map(str::to_owned).collect();
        assert_eq!(&names[..bundled.len()], &bundled[..]);
        assert_eq!(&names[bundled.len()..], ["alpha", "zebra"]);
        assert_eq!(available_names(None), bundled);
    }

    #[test]
    fn unknown_theme_is_an_error() {
        assert!(matches!(
            Theme::load("nope", None),
            Err(ThemeError::NotFound(_))
        ));
    }
}
