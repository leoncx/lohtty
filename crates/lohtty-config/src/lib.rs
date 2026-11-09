//! User configuration (`config.toml`) and live reload.

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
}

impl Default for Config {
    fn default() -> Self {
        Self {
            shell: ShellConfig::default(),
            font: FontConfig::default(),
            window: WindowConfig::default(),
            theme: "tokyo-night".into(),
            scrollback: 10_000,
        }
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
}

impl Default for FontConfig {
    fn default() -> Self {
        Self {
            family: if cfg!(windows) {
                "Cascadia Mono"
            } else {
                "monospace"
            }
            .into(),
            size: 14.0,
            ligatures: true,
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
    fn partial_override() {
        let cfg =
            Config::parse("theme = \"other\"\n[window]\nopacity = 0.8\n", "x".into()).unwrap();
        assert_eq!(cfg.theme, "other");
        assert_eq!(cfg.window.opacity, 0.8);
        assert_eq!(cfg.window.padding, WindowConfig::default().padding);
    }
}
