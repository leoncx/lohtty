//! Terminal session: PTY, VT state (alacritty_terminal), events, input encoding.
//! Intentionally free of gpui so it can be unit tested.

mod filter;
pub mod input;
#[cfg(feature = "kitty-graphics")]
pub mod kitty;
pub mod mouse;
pub mod search;
mod session;
mod shell;

pub use alacritty_terminal;
pub use filter::{OutputFilter, PassThrough, Screen};
pub use session::{Session, SessionEvent, TermSize};
pub use shell::{ShellSpec, resolve_shell};
