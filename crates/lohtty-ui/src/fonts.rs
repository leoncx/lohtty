//! Font names from the config, resolved against the installed fonts.

use std::collections::HashSet;
use std::sync::OnceLock;

use gpui::{FontFallbacks, SharedString, Window};
use lohtty_config::{DEFAULT_FALLBACKS, FontConfig, MONOSPACE, MONOSPACE_CANDIDATES};

/// The installed font family names, read once: fonts installed while lohtty
/// runs are picked up on the next start.
fn installed(window: &Window) -> &'static HashSet<String> {
    static INSTALLED: OnceLock<HashSet<String>> = OnceLock::new();
    INSTALLED.get_or_init(|| window.text_system().all_font_names().into_iter().collect())
}

/// The family to ask gpui for. gpui doesn't resolve the generic
/// [`MONOSPACE`] name on Linux (it falls back to a proportional font), so it
/// becomes the first installed of [`MONOSPACE_CANDIDATES`].
pub fn family(name: &str, window: &Window) -> SharedString {
    resolve_family(name, installed(window)).into()
}

fn resolve_family(name: &str, installed: &HashSet<String>) -> String {
    if !name.eq_ignore_ascii_case(MONOSPACE) || installed.contains(name) {
        return name.to_owned();
    }
    MONOSPACE_CANDIDATES
        .iter()
        .find(|c| installed.contains(**c))
        .map_or_else(|| name.to_owned(), |c| (*c).to_owned())
}

/// The configured fallback list, or the installed subset of the defaults.
pub fn fallbacks(font: &FontConfig, window: &Window) -> Option<FontFallbacks> {
    static INSTALLED_DEFAULTS: OnceLock<Vec<String>> = OnceLock::new();
    let names = match &font.fallbacks {
        Some(list) => list.clone(),
        None => INSTALLED_DEFAULTS
            .get_or_init(|| {
                let installed = installed(window);
                let found: Vec<String> = DEFAULT_FALLBACKS
                    .iter()
                    .filter(|name| installed.contains(**name))
                    .map(|name| name.to_string())
                    .collect();
                tracing::info!(?found, "default font fallbacks");
                found
            })
            .clone(),
    };
    (!names.is_empty()).then(|| FontFallbacks::from_fonts(names))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(names: &[&str]) -> HashSet<String> {
        names.iter().map(|n| n.to_string()).collect()
    }

    #[test]
    fn named_family_is_kept() {
        let installed = set(&["DejaVu Sans Mono"]);
        assert_eq!(resolve_family("Fira Code", &installed), "Fira Code");
    }

    #[test]
    fn monospace_becomes_first_installed_candidate() {
        let installed = set(&["Liberation Mono", "Noto Sans Mono"]);
        assert_eq!(resolve_family("monospace", &installed), "Noto Sans Mono");
        assert_eq!(resolve_family("Monospace", &installed), "Noto Sans Mono");
    }

    #[test]
    fn monospace_kept_when_nothing_matches() {
        assert_eq!(resolve_family("monospace", &set(&[])), "monospace");
        // A font literally named "monospace" wins.
        assert_eq!(
            resolve_family("monospace", &set(&["monospace", "Hack"])),
            "monospace"
        );
    }
}
