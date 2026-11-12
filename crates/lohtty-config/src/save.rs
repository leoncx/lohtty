//! Writing settings back to `config.toml` without losing the user's edits:
//! the file is updated in place, so comments, key order and settings this
//! module doesn't manage (`font.fallbacks`, window size...) are preserved.

use std::path::{Path, PathBuf};

use toml_edit::{Array, DocumentMut, Item, Table, Value, value};

use crate::{Config, ConfigError};

impl Config {
    /// Saves the settings managed by the settings dialog to the default path.
    pub fn save(&self) -> Result<PathBuf, ConfigError> {
        let path = Self::default_path().ok_or_else(|| ConfigError::Io {
            path: PathBuf::from("config.toml"),
            source: std::io::Error::other("no config directory on this platform"),
        })?;
        self.save_to(&path)?;
        Ok(path)
    }

    /// Updates `path` (creating it and its directory if needed).
    pub fn save_to(&self, path: &Path) -> Result<(), ConfigError> {
        let io = |source| ConfigError::Io {
            path: path.to_owned(),
            source,
        };
        let existing = match std::fs::read_to_string(path) {
            Ok(src) => src,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(io(e)),
        };
        let updated = self.apply_to(&existing).map_err(|e| ConfigError::Io {
            path: path.to_owned(),
            source: std::io::Error::new(std::io::ErrorKind::InvalidData, e),
        })?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(io)?;
        }
        std::fs::write(path, updated).map_err(io)
    }

    /// Returns `src` (a TOML document) with this config's managed settings applied.
    pub fn apply_to(&self, src: &str) -> Result<String, toml_edit::TomlError> {
        let mut doc: DocumentMut = src.parse()?;

        set(doc.as_table_mut(), "theme", self.theme.as_str().into());
        set(
            doc.as_table_mut(),
            "scrollback",
            (self.scrollback as i64).into(),
        );

        let shell = table(&mut doc, "shell");
        match &self.shell.program {
            Some(program) => set(shell, "program", program.as_str().into()),
            None => {
                shell.remove("program");
            }
        }
        let mut args = Array::new();
        for arg in &self.shell.args {
            args.push(arg.as_str());
        }
        set(shell, "args", args.into());

        let font = table(&mut doc, "font");
        set(font, "family", self.font.family.as_str().into());
        set(font, "size", round2(self.font.size).into());
        set(font, "ligatures", self.font.ligatures.into());

        let window = table(&mut doc, "window");
        set(window, "opacity", round2(self.window.opacity).into());
        set(window, "blur", self.window.blur.into());
        set(window, "padding", round2(self.window.padding).into());

        let usage = table(&mut doc, "usage");
        set(usage, "claude", self.usage.claude.into());
        set(usage, "codex", self.usage.codex.into());

        Ok(doc.to_string())
    }
}

/// Sets `table[key]`, keeping the existing value's surrounding comments and
/// spacing (e.g. a trailing `# comment`).
fn set(table: &mut Table, key: &str, mut new: Value) {
    if let Some(old) = table.get(key).and_then(Item::as_value) {
        *new.decor_mut() = old.decor().clone();
    }
    table[key] = value(new);
}

/// The table `key`, created if missing (or replacing a non-table value).
fn table<'a>(doc: &'a mut DocumentMut, key: &str) -> &'a mut Table {
    if !doc.contains_table(key) {
        doc[key] = Item::Table(Table::new());
    }
    doc[key].as_table_mut().expect("just ensured it is a table")
}

/// Avoids writing float noise such as `0.8500000238418579`.
fn round2(x: f32) -> f64 {
    (x as f64 * 100.0).round() / 100.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_a_fresh_file_that_parses_back() {
        let mut cfg = Config {
            theme: "tokyo-night-moon".into(),
            ..Default::default()
        };
        cfg.window.opacity = 0.85;
        cfg.shell.program = Some("pwsh.exe".into());
        cfg.shell.args = vec!["-NoLogo".into()];
        cfg.usage.codex = false;
        let out = cfg.apply_to("").unwrap();
        let back = Config::parse(&out, "x".into()).unwrap();
        assert_eq!(back, cfg);
        assert!(out.contains("opacity = 0.85"), "{out}");
    }

    #[test]
    fn preserves_comments_and_unmanaged_keys() {
        let src = "# my terminal\ntheme = \"tokyo-night\" # favourite\n\n[font]\n# icons\nfallbacks = [\"Symbols Nerd Font Mono\"]\nsize = 12.0\n\n[window]\nwidth = 1400.0\n";
        let mut cfg = Config::parse(src, "x".into()).unwrap();
        cfg.font.size = 16.0;
        let out = cfg.apply_to(src).unwrap();

        assert!(out.contains("# my terminal"), "{out}");
        assert!(out.contains("# favourite"), "{out}");
        assert!(out.contains("# icons"), "{out}");
        let back = Config::parse(&out, "x".into()).unwrap();
        assert_eq!(back.font.size, 16.0);
        assert_eq!(
            back.font.fallbacks.as_deref(),
            Some(&["Symbols Nerd Font Mono".to_string()][..])
        );
        assert_eq!(back.window.width, 1400.0);
    }

    #[test]
    fn default_shell_removes_the_program_key() {
        let src = "[shell]\nprogram = \"bash\"\n";
        let cfg = Config::default();
        let out = cfg.apply_to(src).unwrap();
        assert!(!out.contains("program"), "{out}");
    }

    #[test]
    fn save_to_creates_the_directory() {
        let dir = std::env::temp_dir().join(format!("lohtty-save-test-{}", std::process::id()));
        let path = dir.join("nested").join("config.toml");
        Config::default().save_to(&path).unwrap();
        let back = Config::parse(&std::fs::read_to_string(&path).unwrap(), path.clone()).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(back, Config::default());
    }
}
