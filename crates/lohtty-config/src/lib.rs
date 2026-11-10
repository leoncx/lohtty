//! User configuration (`config.toml`) and live reload.

mod save;
mod watch;

use std::path::PathBuf;

pub use watch::ConfigWatcher;

use serde::Deserialize;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("failed to read {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("invalid config {path}: {source}")]
    Parse {
        path: PathBuf,
        source: toml::de::Error,
    },
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(default)]
pub struct Config {
    pub shell: ShellConfig,
    pub font: FontConfig,
    pub window: WindowConfig,
    pub theme: String,
    pub scrollback: usize,
    pub graphics: GraphicsConfig,
    pub notifications: NotificationsConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            shell: ShellConfig::default(),
            font: FontConfig::default(),
            window: WindowConfig::default(),
            theme: "tokyo-night".into(),
            scrollback: 10_000,
            graphics: GraphicsConfig::default(),
            notifications: NotificationsConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(default)]
pub struct NotificationsConfig {
    /// Desktop notifications when a program in an unfocused window asks for
    /// attention (bell, or OSC 9 / 777 / 99). The tab mark and the taskbar
    /// flash don't depend on it.
    pub desktop: bool,
}

impl Default for NotificationsConfig {
    fn default() -> Self {
        Self { desktop: true }
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(default)]
pub struct GraphicsConfig {
    /// Kitty graphics protocol (images), in builds that include it. Applies
    /// to tabs and panes opened afterwards.
    pub kitty: bool,
}

impl Default for GraphicsConfig {
    fn default() -> Self {
        Self { kitty: true }
    }
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct ShellConfig {
    /// `None` means the platform default (PowerShell on Windows, `$SHELL` elsewhere).
    pub program: Option<String>,
    pub args: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(default)]
pub struct FontConfig {
    pub family: String,
    pub size: f32,
    pub ligatures: bool,
    /// Fonts tried, in order, for characters the main font lacks (e.g. Nerd
    /// Font icons). `None` picks installed fonts from [`DEFAULT_FALLBACKS`];
    /// the system fallback (emoji, CJK...) always applies afterwards.
    pub fallbacks: Option<Vec<String>>,
}

/// Fallback candidates used when `font.fallbacks` isn't set: common fonts
/// carrying Nerd Font icons (used by prompts such as oh-my-posh or starship).
pub const DEFAULT_FALLBACKS: &[&str] = &[
    "Symbols Nerd Font Mono",
    "Symbols Nerd Font",
    "CaskaydiaCove Nerd Font Mono",
    "CaskaydiaMono Nerd Font Mono",
    "FiraCode Nerd Font Mono",
    "JetBrainsMono Nerd Font Mono",
    "MesloLGS Nerd Font Mono",
    "Hack Nerd Font Mono",
];

/// The generic family name: the default on Linux, where it stands for the
/// first installed of [`MONOSPACE_CANDIDATES`].
pub const MONOSPACE: &str = "monospace";

/// Fonts tried, in order, for the generic [`MONOSPACE`] family: the usual
/// monospace fonts of Linux distributions first.
pub const MONOSPACE_CANDIDATES: &[&str] = &[
    "DejaVu Sans Mono",
    "Noto Sans Mono",
    "Liberation Mono",
    "Ubuntu Mono",
    "Cascadia Mono",
    "Source Code Pro",
    "Hack",
    "Menlo",
    "Consolas",
];

impl Default for FontConfig {
    fn default() -> Self {
        Self {
            // Cascadia Code ships with Windows Terminal and has ligatures;
            // Menlo ships with macOS.
            family: if cfg!(windows) {
                "Cascadia Code"
            } else if cfg!(target_os = "macos") {
                "Menlo"
            } else {
                MONOSPACE
            }
            .into(),
            size: 14.0,
            ligatures: true,
            fallbacks: None,
        }
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(default)]
pub struct WindowConfig {
    /// 0.0 (fully transparent) to 1.0 (opaque).
    pub opacity: f32,
    pub blur: bool,
    pub padding: f32,
    pub width: f32,
    pub height: f32,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            opacity: 1.0,
            blur: false,
            padding: 8.0,
            width: 1000.0,
            height: 650.0,
        }
    }
}

/// `%APPDATA%\lohtty` on Windows, `$XDG_CONFIG_HOME/lohtty` elsewhere.
pub fn config_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("lohtty"))
}

/// User themes: `<config dir>/themes/<name>.toml`.
pub fn themes_dir() -> Option<PathBuf> {
    config_dir().map(|d| d.join("themes"))
}

impl Config {
    /// `<config dir>/config.toml`.
    pub fn default_path() -> Option<PathBuf> {
        config_dir().map(|d| d.join("config.toml"))
    }

    pub fn parse(src: &str, path: PathBuf) -> Result<Self, ConfigError> {
        toml::from_str(src).map_err(|source| ConfigError::Parse { path, source })
    }

    /// Loads the config from the default path. A missing file yields the defaults.
    pub fn load() -> Result<Self, ConfigError> {
        let Some(path) = Self::default_path() else {
            return Ok(Self::default());
        };
        match std::fs::read_to_string(&path) {
            Ok(src) => Self::parse(&src, path),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                tracing::debug!(?path, "no config file, using defaults");
                Ok(Self::default())
            }
            Err(source) => Err(ConfigError::Io { path, source }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_file_gives_defaults() {
        assert_eq!(Config::parse("", "x".into()).unwrap(), Config::default());
    }

    #[test]
    fn font_fallbacks_default_to_auto() {
        assert_eq!(Config::default().font.fallbacks, None);
        let cfg = Config::parse(
            "[font]\nfallbacks = [\"Symbols Nerd Font Mono\"]\n",
            "x".into(),
        )
        .unwrap();
        assert_eq!(
            cfg.font.fallbacks.as_deref(),
            Some(&["Symbols Nerd Font Mono".to_string()][..])
        );
    }

    #[test]
    fn partial_override() {
        let cfg =
            Config::parse("theme = \"other\"\n[window]\nopacity = 0.8\n", "x".into()).unwrap();
        assert_eq!(cfg.theme, "other");
        assert_eq!(cfg.window.opacity, 0.8);
        assert_eq!(cfg.window.padding, WindowConfig::default().padding);
    }
}
