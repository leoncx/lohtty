//! Getting the user's attention when lohtty isn't in front: flashing the
//! taskbar button and showing a desktop notification. Windows only for now;
//! elsewhere both do nothing.
//!
//! Notifications are notification-area balloons, which Windows 10 and 11
//! show as toasts. Unlike WinRT toasts, they need no registered app id, so
//! they also work for the portable build. A hidden message-only window
//! receives the balloon's events; it lives on the UI thread, whose message
//! loop (gpui's) dispatches them.

use gpui::Window;

/// Shows notifications and reports which one was clicked.
pub struct Notifier {
    #[cfg(windows)]
    inner: Option<windows::Tray>,
}

impl Notifier {
    /// A notifier, and the receiver of the tokens of clicked notifications.
    pub fn new() -> (Self, async_channel::Receiver<u64>) {
        let (tx, rx) = async_channel::unbounded();
        #[cfg(windows)]
        {
            let inner = windows::Tray::new(tx)
                .inspect_err(|e| tracing::warn!("desktop notifications disabled: {e}"))
                .ok();
            (Self { inner }, rx)
        }
        #[cfg(not(windows))]
        {
            drop(tx);
            (Self {}, rx)
        }
    }

    /// Shows a notification, replacing the previous one. Clicking it sends
    /// `token` to the receiver.
    pub fn notify(&self, title: &str, body: &str, token: u64) {
        #[cfg(windows)]
        if let Some(tray) = &self.inner {
            tray.show(title, body, token);
        }
        #[cfg(not(windows))]
        let _ = (title, body, token);
    }
}

/// What the taskbar button shows as progress.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskbarProgress {
    None,
    /// A green bar, 0–100.
    Normal(u8),
    /// A moving marquee: busy, amount unknown.
    Indeterminate,
    /// A yellow bar, 0–100: waiting.
    Paused(u8),
    /// A red bar.
    Error,
}

/// Shows progress on the window's taskbar button (Windows), as Windows
/// Terminal does. Repeated calls with the same value do nothing.
pub fn taskbar_progress(window: &Window, progress: TaskbarProgress) {
    #[cfg(windows)]
    windows::taskbar_progress(window, progress);
    #[cfg(not(windows))]
    let _ = (window, progress);
}

/// Flashes the window's taskbar button until the window is activated.
pub fn flash(window: &Window) {
    #[cfg(windows)]
    windows::flash(window);
    #[cfg(not(windows))]
    let _ = window;
}

#[cfg(windows)]
mod windows {
    use std::cell::{Cell, RefCell};
    use std::ffi::c_void;
    use std::ptr::null;

    use gpui::Window;
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows_sys::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
    };
    use windows_sys::core::GUID;

    use super::TaskbarProgress;
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::Shell::{
        NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_LARGE_ICON, NIIF_USER, NIM_ADD, NIM_DELETE,
        NIM_MODIFY, NIM_SETVERSION, NIN_BALLOONHIDE, NIN_BALLOONTIMEOUT, NIN_BALLOONUSERCLICK,
        NOTIFYICON_VERSION_4, NOTIFYICONDATAW, Shell_NotifyIconW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, FLASHW_TIMERNOFG, FLASHW_TRAY, FLASHWINFO,
        FlashWindowEx, HICON, HWND_MESSAGE, IMAGE_ICON, LR_DEFAULTCOLOR, LR_SHARED, LoadImageW,
        RegisterClassW, WM_APP, WM_LBUTTONUP, WNDCLASSW,
    };

    /// Message the notification area sends about our icon.
    const CALLBACK: u32 = WM_APP + 1;
    const ICON_ID: u32 = 1;

    thread_local! {
        /// The click channel and the token of the notification showing, for
        /// the window procedure.
        static STATE: RefCell<Option<(async_channel::Sender<u64>, u64)>> = const { RefCell::new(None) };
        /// Whether our icon is in the notification area. The window procedure
        /// removes it when a balloon is clicked or goes away.
        static SHOWN: Cell<bool> = const { Cell::new(false) };
    }

    pub struct Tray {
        hwnd: HWND,
        icon: HICON,
        large_icon: HICON,
    }

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain([0]).collect()
    }

    /// Copies `text` into a fixed-size, NUL-terminated buffer, shortened.
    fn fill<const N: usize>(buf: &mut [u16; N], text: &str) {
        let units: Vec<u16> = text.encode_utf16().take(N - 1).collect();
        buf[..units.len()].copy_from_slice(&units);
        buf[units.len()] = 0;
    }

    /// The application icon (resource 1, embedded by the build script).
    fn app_icon(size: i32) -> HICON {
        // SAFETY: loads a shared icon resource of this module; a missing
        // resource yields null, which Windows accepts (no icon).
        unsafe {
            LoadImageW(
                GetModuleHandleW(null()),
                1 as _,
                IMAGE_ICON,
                size,
                size,
                LR_DEFAULTCOLOR | LR_SHARED,
            )
        }
    }

    impl Tray {
        pub fn new(clicks: async_channel::Sender<u64>) -> Result<Self, String> {
            let class = wide("lohtty-notifier");
            // SAFETY: registers a window class with a valid procedure and a
            // NUL-terminated name that outlives the call; registering it twice
            // fails harmlessly.
            unsafe {
                let wc = WNDCLASSW {
                    lpfnWndProc: Some(window_proc),
                    hInstance: GetModuleHandleW(null()),
                    lpszClassName: class.as_ptr(),
                    ..std::mem::zeroed()
                };
                RegisterClassW(&wc);
            }
            // SAFETY: a message-only window of the class above.
            let hwnd = unsafe {
                CreateWindowExW(
                    0,
                    class.as_ptr(),
                    null(),
                    0,
                    0,
                    0,
                    0,
                    0,
                    HWND_MESSAGE,
                    std::ptr::null_mut(),
                    GetModuleHandleW(null()),
                    null(),
                )
            };
            if hwnd.is_null() {
                return Err(std::io::Error::last_os_error().to_string());
            }
            STATE.with_borrow_mut(|s| *s = Some((clicks, 0)));
            Ok(Self {
                hwnd,
                icon: app_icon(16),
                large_icon: app_icon(48),
            })
        }

        fn data(&self) -> NOTIFYICONDATAW {
            NOTIFYICONDATAW {
                cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
                hWnd: self.hwnd,
                uID: ICON_ID,
                ..Default::default()
            }
        }

        pub fn show(&self, title: &str, body: &str, token: u64) {
            STATE.with_borrow_mut(|s| {
                if let Some((_, current)) = s {
                    *current = token;
                }
            });
            let mut data = self.data();
            data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_INFO;
            data.uCallbackMessage = CALLBACK;
            data.hIcon = self.icon;
            data.hBalloonIcon = self.large_icon;
            data.dwInfoFlags = NIIF_USER | NIIF_LARGE_ICON;
            fill(&mut data.szTip, "lohtty");
            fill(&mut data.szInfoTitle, title);
            // An empty body would remove the balloon instead of showing it.
            fill(&mut data.szInfo, if body.is_empty() { " " } else { body });
            // SAFETY: `data` is a fully initialized NOTIFYICONDATAW for our
            // window.
            unsafe {
                if SHOWN.get() {
                    Shell_NotifyIconW(NIM_MODIFY, &data);
                } else {
                    Shell_NotifyIconW(NIM_ADD, &data);
                    data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
                    Shell_NotifyIconW(NIM_SETVERSION, &data);
                    SHOWN.set(true);
                }
            }
        }
    }

    impl Drop for Tray {
        fn drop(&mut self) {
            remove_icon(self.hwnd);
            STATE.with_borrow_mut(|s| *s = None);
            // SAFETY: destroys the window created in `new`.
            unsafe { DestroyWindow(self.hwnd) };
        }
    }

    /// Takes our icon out of the notification area (no-op if absent).
    fn remove_icon(hwnd: HWND) {
        SHOWN.set(false);
        let data = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: hwnd,
            uID: ICON_ID,
            ..Default::default()
        };
        // SAFETY: identifies our icon; deleting a missing one just fails.
        unsafe { Shell_NotifyIconW(NIM_DELETE, &data) };
    }

    unsafe extern "system" fn window_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        if msg == CALLBACK {
            // With NOTIFYICON_VERSION_4 the event is in the low word.
            let event = (lparam as u32) & 0xffff;
            match event {
                NIN_BALLOONUSERCLICK | WM_LBUTTONUP => {
                    STATE.with_borrow(|s| {
                        if let Some((clicks, token)) = s {
                            let _ = clicks.try_send(*token);
                        }
                    });
                    remove_icon(hwnd);
                }
                NIN_BALLOONTIMEOUT | NIN_BALLOONHIDE => remove_icon(hwnd),
                _ => {}
            }
            return 0;
        }
        // SAFETY: default handling for every other message.
        unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
    }

    fn hwnd(window: &Window) -> Option<HWND> {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        match HasWindowHandle::window_handle(window).map(|h| h.as_raw()) {
            Ok(RawWindowHandle::Win32(handle)) => Some(handle.hwnd.get() as HWND),
            _ => None,
        }
    }

    /// The `ITaskbarList3` methods used, by vtable slot (after IUnknown's
    /// three, `ITaskbarList`'s five and `ITaskbarList2`'s one).
    #[repr(C)]
    struct TaskbarListVtbl {
        _iunknown: [usize; 3],
        hr_init: unsafe extern "system" fn(*mut c_void) -> i32,
        _taskbar_list: [usize; 4],
        _taskbar_list2: [usize; 1],
        set_progress_value: unsafe extern "system" fn(*mut c_void, HWND, u64, u64) -> i32,
        set_progress_state: unsafe extern "system" fn(*mut c_void, HWND, i32) -> i32,
    }

    thread_local! {
        /// The taskbar object (created on first use), and the last progress
        /// shown per window.
        static TASKBAR: Cell<*mut c_void> = const { Cell::new(std::ptr::null_mut()) };
        static SHOWN_PROGRESS: RefCell<Vec<(HWND, TaskbarProgress)>> = const { RefCell::new(Vec::new()) };
    }

    fn taskbar() -> Option<*mut c_void> {
        let existing = TASKBAR.get();
        if !existing.is_null() {
            return Some(existing);
        }
        const CLSID_TASKBAR_LIST: GUID = GUID::from_u128(0x56fdf344_fd6d_11d0_958a_006097c9a090);
        const IID_ITASKBAR_LIST3: GUID = GUID::from_u128(0xea1afb91_9e28_4b86_90e9_9e9f8a5eefaf);
        let mut object: *mut c_void = std::ptr::null_mut();
        // SAFETY: COM is (or gets) initialized on this UI thread; a failed
        // creation leaves `object` null.
        unsafe {
            CoInitializeEx(null(), COINIT_APARTMENTTHREADED as u32);
            let hr = CoCreateInstance(
                &CLSID_TASKBAR_LIST,
                std::ptr::null_mut(),
                CLSCTX_INPROC_SERVER,
                &IID_ITASKBAR_LIST3,
                &mut object,
            );
            if hr < 0 || object.is_null() {
                return None;
            }
            let vtbl = *(object as *const *const TaskbarListVtbl);
            ((*vtbl).hr_init)(object);
        }
        TASKBAR.set(object);
        Some(object)
    }

    pub fn taskbar_progress(window: &Window, progress: TaskbarProgress) {
        let Some(hwnd) = hwnd(window) else {
            return;
        };
        let unchanged = SHOWN_PROGRESS.with_borrow_mut(|shown| {
            match shown.iter_mut().find(|(h, _)| *h == hwnd) {
                Some((_, last)) if *last == progress => true,
                Some((_, last)) => {
                    *last = progress;
                    false
                }
                None => {
                    shown.push((hwnd, progress));
                    progress == TaskbarProgress::None
                }
            }
        });
        if unchanged {
            return;
        }
        let Some(object) = taskbar() else {
            return;
        };
        // TBPF_* flags.
        let (state, value) = match progress {
            TaskbarProgress::None => (0, None),
            TaskbarProgress::Indeterminate => (1, None),
            TaskbarProgress::Normal(v) => (2, Some(v)),
            TaskbarProgress::Error => (4, Some(100)),
            TaskbarProgress::Paused(v) => (8, Some(v)),
        };
        // SAFETY: `object` is a live ITaskbarList3 owned by this thread, and
        // `hwnd` this process's window.
        unsafe {
            let vtbl = *(object as *const *const TaskbarListVtbl);
            ((*vtbl).set_progress_state)(object, hwnd, state);
            if let Some(v) = value {
                ((*vtbl).set_progress_value)(object, hwnd, v.min(100) as u64, 100);
            }
        }
    }

    pub fn flash(window: &Window) {
        let Some(hwnd) = hwnd(window) else {
            return;
        };
        let info = FLASHWINFO {
            cbSize: std::mem::size_of::<FLASHWINFO>() as u32,
            hwnd,
            // Until the window comes to the foreground.
            dwFlags: FLASHW_TRAY | FLASHW_TIMERNOFG,
            uCount: 0,
            dwTimeout: 0,
        };
        // SAFETY: the handle belongs to this process's window.
        unsafe { FlashWindowEx(&info) };
    }
}
