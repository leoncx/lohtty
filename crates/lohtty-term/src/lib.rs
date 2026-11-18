//! Terminal session: PTY, VT state (alacritty_terminal), events, input encoding.
//! Intentionally free of gpui so it can be unit tested.

mod cwd;
mod filter;
mod foreground;
pub mod input;
#[cfg(feature = "kitty-graphics")]
pub mod kitty;
pub mod modes;
pub mod mouse;
pub mod notify;
pub mod search;
mod session;
mod shell;
pub mod status;

pub use alacritty_terminal;
pub use cwd::display_dir;
pub use filter::{OutputFilter, PassThrough, Screen};
pub use session::{Session, SessionEvent, ShellProbe, TermSize};
pub use shell::{ShellSpec, resolve_shell};
