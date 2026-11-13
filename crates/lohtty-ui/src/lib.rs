//! gpui views: Workspace, TabBar, PaneGroup, TerminalView, SearchBar.

mod colors;
mod fonts;
mod notifier;
mod paint_now;
mod pane_tree;
mod perf;
mod search_bar;
mod settings;
mod terminal_view;
mod text_field;
mod tooltip;
mod usage_meter;
mod workspace;

use gpui::{App, Decorations, Pixels, Point, Window, WindowBackgroundAppearance, point, px};
use lohtty_config::{Config, WindowConfig, themes_dir};
use lohtty_theme::{Theme, ThemeError};

pub use terminal_view::{Copy, Paste, TerminalView};
pub use workspace::Workspace;

/// Whether lohtty asks to draw its own title bar (tabs in the window's top
/// bar) instead of the platform's: on Windows and Linux, next to caption
/// buttons it draws; on macOS, right of the traffic lights. On Linux the
/// compositor may still impose its own: see [`custom_titlebar`].
pub const CUSTOM_TITLEBAR: bool = true;

/// Whether this window has lohtty's title bar. Always on Windows and macOS;
/// on Linux when the compositor accepted client-side decorations (GNOME
/// always does; others may draw their own title bar instead).
pub fn custom_titlebar(window: &Window) -> bool {
    cfg!(any(windows, target_os = "macos"))
        || matches!(window.window_decorations(), Decorations::Client { .. })
}

/// Whether lohtty draws the minimize, maximize and close buttons: with its
/// own title bar, except on macOS (traffic lights).
pub fn caption_buttons(window: &Window) -> bool {
    !cfg!(target_os = "macos") && custom_titlebar(window)
}

/// Where the macOS traffic lights sit, from the window's top-left corner:
/// vertically centered on the 34 px tab bar.
pub const TRAFFIC_LIGHT_POSITION: Point<Pixels> = point(px(12.0), px(10.0));

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
        // Linux: the client-side decorations' shadow must be see-through
        // (the window's contents are painted opaque anyway).
        (false, _) if cfg!(target_os = "linux") => WindowBackgroundAppearance::Transparent,
        (false, _) => WindowBackgroundAppearance::Opaque,
        (true, false) => WindowBackgroundAppearance::Transparent,
        (true, true) => WindowBackgroundAppearance::Blurred,
    }
}
