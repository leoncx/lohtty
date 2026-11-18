use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};

use crate::{Config, config_dir};

/// Watches the config directory (config file and user themes), and the
/// folder of a config file named by `LOHTTY_CONFIG`, and calls
/// `on_change` when a `.toml` file is written, created or removed.
///
/// The directory is watched rather than the file because editors often save
/// by replacing the file. Dropping the watcher stops it.
pub struct ConfigWatcher {
    _watcher: RecommendedWatcher,
}

impl ConfigWatcher {
    pub fn spawn(on_change: impl Fn() + Send + 'static) -> notify::Result<Self> {
        let dir = config_dir()
            .ok_or_else(|| notify::Error::generic("no config directory on this platform"))?;
        // Create it so that a config file added later is still picked up.
        std::fs::create_dir_all(&dir).map_err(notify::Error::io)?;

        let mut watcher =
            notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
                let Ok(event) = res else { return };
                let relevant_kind = matches!(
                    event.kind,
                    EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
                );
                let toml = event
                    .paths
                    .iter()
                    .any(|p| p.extension().is_some_and(|e| e == "toml"));
                if relevant_kind && toml {
                    on_change();
                }
            })?;
        watcher.watch(&dir, RecursiveMode::Recursive)?;
        tracing::debug!(?dir, "watching config directory");
        if let Some(other) = Config::default_path()
            .as_deref()
            .and_then(std::path::Path::parent)
            .filter(|p| !p.starts_with(&dir) && p.is_dir())
        {
            watcher.watch(other, RecursiveMode::NonRecursive)?;
            tracing::debug!(dir = ?other, "watching the LOHTTY_CONFIG directory");
        }
        Ok(Self { _watcher: watcher })
    }
}
