//! gpui views: Workspace, TabBar, PaneGroup, TerminalView, SearchBar.

mod colors;
mod paint_now;
mod pane_tree;
mod perf;
mod search_bar;
mod settings;
mod terminal_view;
mod text_field;
mod tooltip;
mod workspace;

use gpui::{App, WindowBackgroundAppearance};
use lohtty_config::{Config, WindowConfig, themes_dir};
use lohtty_theme::{Theme, ThemeError};

pub use terminal_view::{Copy, Paste, TerminalView};
pub use workspace::Workspace;

/// Whether lohtty draws its own title bar (tabs next to the caption buttons)
/// instead of the platform's. Windows only for now: macOS would need room for
/// the traffic lights, Linux depends on the compositor.
pub const CUSTOM_TITLEBAR: bool = cfg!(windows);

/// Registers key bindings. Call once at startup.
pub fn init(cx: &mut App) {
    cx.bind_keys(terminal_view::key_bindings());
    cx.bind_keys(workspace::key_bindings());
}

/// Loads the theme named in the config (user themes dir first, then bundled).
pub fn load_theme(config: &Config) -> Result<Theme, ThemeError> {
    Theme::load(&config.theme, themes_dir().as_deref())
}

/// The window background mode implied by the opacity and blur settings.
pub fn window_background(window: &WindowConfig) -> WindowBackgroundAppearance {
    match (window.opacity < 1.0, window.blur) {
        (false, _) => WindowBackgroundAppearance::Opaque,
        (true, false) => WindowBackgroundAppearance::Transparent,
        (true, true) => WindowBackgroundAppearance::Blurred,
    }
}
