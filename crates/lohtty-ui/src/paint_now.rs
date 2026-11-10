//! Asks the platform to repaint now instead of on the next vsync tick.
//!
//! On Windows, gpui draws only when a WM_PAINT arrives, and its vsync thread
//! sends one right after each desktop composition (`DwmFlush`). Output that
//! arrives between two compositions therefore waits for the next one, gets
//! drawn, and is shown one composition later. Invalidating the window right
//! away lets gpui draw and present as soon as the UI thread is free, so the
//! frame catches the next composition, which saves about one refresh interval.
//! Used for isolated output such as a keystroke's echo.

use gpui::Window;

#[derive(Default, Clone, Copy)]
pub struct PaintNow {
    #[cfg(windows)]
    hwnd: Option<isize>,
}

impl PaintNow {
    pub fn for_window(window: &Window) -> Self {
        #[cfg(windows)]
        {
            use raw_window_handle::{HasWindowHandle, RawWindowHandle};
            let hwnd = match HasWindowHandle::window_handle(window).map(|h| h.as_raw()) {
                Ok(RawWindowHandle::Win32(h)) => Some(h.hwnd.get()),
                _ => None,
            };
            Self { hwnd }
        }
        #[cfg(not(windows))]
        {
            let _ = window;
            Self {}
        }
    }

    /// Call after `cx.notify()`: the dirty window then draws on this WM_PAINT.
    pub fn request(&self) {
        #[cfg(windows)]
        if let Some(hwnd) = self.hwnd {
            #[link(name = "user32")]
            unsafe extern "system" {
                fn InvalidateRect(hwnd: isize, rect: *const core::ffi::c_void, erase: i32) -> i32;
            }
            // SAFETY: the handle belongs to a window of this process; an invalid
            // or destroyed one only makes the call fail.
            unsafe {
                InvalidateRect(hwnd, core::ptr::null(), 0);
            }
        }
    }
}
