//! Whether the shell itself is in the foreground, i.e. waiting at its prompt
//! rather than running a program.

/// Whether the shell (process `pid`, the session leader on the PTY whose
/// master is `fd`) owns the terminal: the foreground process group is its own.
/// `None` when the terminal can't be asked.
#[cfg(unix)]
pub fn shell_in_foreground(pid: u32, fd: std::os::fd::RawFd) -> Option<bool> {
    // SAFETY: `tcgetpgrp` only reads; a closed fd makes it fail.
    let group = unsafe { libc::tcgetpgrp(fd) };
    (group > 0).then_some(group as u32 == pid)
}

/// Whether the shell (process `pid`) runs no program. A console has no
/// foreground process group, so this checks that the shell has no child
/// process. `None` when processes can't be listed.
#[cfg(windows)]
pub fn shell_in_foreground(pid: u32) -> Option<bool> {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };

    // SAFETY: the snapshot handle is checked and closed; `entry` is a
    // zeroed `PROCESSENTRY32W` with `dwSize` set, as the API requires.
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return None;
        }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut child = false;
        let mut more = Process32FirstW(snapshot, &mut entry) != 0;
        while more {
            if entry.th32ParentProcessID == pid && entry.th32ProcessID != pid {
                child = true;
                break;
            }
            more = Process32NextW(snapshot, &mut entry) != 0;
        }
        CloseHandle(snapshot);
        Some(!child)
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn a_process_with_a_child_is_not_in_the_foreground() {
        let me = std::process::id();
        assert_eq!(shell_in_foreground(me), Some(true));
        let mut child = std::process::Command::new("cmd")
            .args(["/c", "ping -n 3 127.0.0.1 >nul"])
            .spawn()
            .unwrap();
        assert_eq!(shell_in_foreground(me), Some(false));
        child.kill().ok();
        child.wait().unwrap();
        assert_eq!(shell_in_foreground(me), Some(true));
    }
}
