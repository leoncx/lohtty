//! The shell's working directory, so new panes can start where it is.
//!
//! Two sources:
//! - What the shell reports: OSC 7 (`ESC ] 7 ; file://host/path ST`, sent by
//!   many prompts and shell integrations) or ConEmu's OSC 9;9
//!   (`ESC ] 9 ; 9 ; "path" ST`, common with PowerShell on Windows). It keeps
//!   the path as the user typed it (symbolic links included), but may be
//!   missing or stale.
//! - The shell process's own directory (macOS and Linux), always current.
//!
//! [`pick`] prefers the reported path when it names the process's directory.

use std::path::{Path, PathBuf};

/// The path in an OSC 7 payload (`file://host/path`), if `host` is this
/// machine. Paths reported from another machine (through ssh) are ignored.
pub fn parse_osc7(payload: &str) -> Option<PathBuf> {
    let rest = payload.strip_prefix("file://")?;
    let (host, path) = rest.split_at(rest.find('/')?);
    if !is_local_host(host) {
        return None;
    }
    let path = percent_decode(path)?;
    // Windows: `/C:/Users/me` names `C:/Users/me`.
    let path = match path.as_bytes() {
        [b'/', drive, b':', ..] if drive.is_ascii_alphabetic() => path[1..].to_owned(),
        _ => path,
    };
    Some(PathBuf::from(path))
}

/// The path in an OSC 9;9 payload (after `9;`), quoted or not.
pub fn parse_osc9_9(payload: &str) -> Option<PathBuf> {
    let path = payload.trim();
    let path = path
        .strip_prefix('"')
        .and_then(|p| p.strip_suffix('"'))
        .unwrap_or(path);
    (!path.is_empty()).then(|| PathBuf::from(path))
}

/// The directory to start a new pane in: the reported one when it is the
/// process's directory (or the process's can't be read), else the process's.
pub fn pick(reported: Option<&Path>, process: Option<&Path>) -> Option<PathBuf> {
    let reported = reported.filter(|p| p.is_dir());
    match (reported, process) {
        (Some(r), Some(p)) if same_dir(r, p) => Some(r.to_owned()),
        (_, Some(p)) => Some(p.to_owned()),
        (r, None) => r.map(Path::to_owned),
    }
}

fn same_dir(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// The working directory of process `pid`.
#[cfg(target_os = "linux")]
pub fn process_cwd(pid: u32) -> Option<PathBuf> {
    std::fs::read_link(format!("/proc/{pid}/cwd")).ok()
}

/// The working directory of process `pid`.
#[cfg(target_os = "macos")]
pub fn process_cwd(pid: u32) -> Option<PathBuf> {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    let mut info: libc::proc_vnodepathinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_vnodepathinfo>() as libc::c_int;
    // SAFETY: `info` is a writable buffer of `size` bytes.
    let n = unsafe {
        libc::proc_pidinfo(
            pid as libc::c_int,
            libc::PROC_PIDVNODEPATHINFO,
            0,
            (&mut info as *mut libc::proc_vnodepathinfo).cast(),
            size,
        )
    };
    if n != size {
        return None;
    }
    // `vip_path` is a NUL-terminated `[c_char; MAXPATHLEN]`, split in rows.
    let raw = &info.pvi_cdir.vip_path;
    // SAFETY: the rows are contiguous `c_char`s, as many as the struct holds.
    let bytes: &[u8] =
        unsafe { std::slice::from_raw_parts(raw.as_ptr().cast(), std::mem::size_of_val(raw)) };
    let end = bytes.iter().position(|&b| b == 0)?;
    (end > 0).then(|| PathBuf::from(OsStr::from_bytes(&bytes[..end])))
}

/// The working directory of process `pid`: not read on this platform.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn process_cwd(_pid: u32) -> Option<PathBuf> {
    None
}

/// Whether an OSC 7 host names this machine: empty, `localhost`, or this
/// machine's name (compared without the domain, ignoring case).
fn is_local_host(host: &str) -> bool {
    let short = |h: &str| h.split('.').next().unwrap_or("").to_ascii_lowercase();
    host.is_empty()
        || host.eq_ignore_ascii_case("localhost")
        || hostname().is_some_and(|me| short(&me) == short(host))
}

#[cfg(unix)]
fn hostname() -> Option<String> {
    let mut buf = [0u8; 256];
    // SAFETY: `buf` is writable for its whole length.
    if unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) } != 0 {
        return None;
    }
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    Some(String::from_utf8_lossy(&buf[..end]).into_owned())
}

#[cfg(not(unix))]
fn hostname() -> Option<String> {
    std::env::var("COMPUTERNAME").ok()
}

/// Decodes `%XX` escapes; `None` if one is malformed or the result isn't UTF-8.
fn percent_decode(s: &str) -> Option<String> {
    let mut out = Vec::with_capacity(s.len());
    let mut bytes = s.bytes();
    while let Some(b) = bytes.next() {
        if b == b'%' {
            let hex = [bytes.next()?, bytes.next()?];
            out.push(u8::from_str_radix(std::str::from_utf8(&hex).ok()?, 16).ok()?);
        } else {
            out.push(b);
        }
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn osc7_paths() {
        assert_eq!(
            parse_osc7("file:///Users/me/My%20Project"),
            Some(PathBuf::from("/Users/me/My Project"))
        );
        assert_eq!(
            parse_osc7("file://localhost/tmp"),
            Some(PathBuf::from("/tmp"))
        );
        let me = hostname().unwrap();
        assert_eq!(
            parse_osc7(&format!("file://{}/tmp", me.to_uppercase())),
            Some(PathBuf::from("/tmp"))
        );
        assert_eq!(
            parse_osc7("file:///C:/Users/me"),
            Some(PathBuf::from("C:/Users/me"))
        );
        assert_eq!(parse_osc7("file://elsewhere.example/tmp"), None);
        assert_eq!(parse_osc7("file:///bad%2"), None);
        assert_eq!(parse_osc7("/tmp"), None);
    }

    #[test]
    fn osc9_9_paths() {
        assert_eq!(
            parse_osc9_9("\"C:\\Users\\me\""),
            Some(PathBuf::from("C:\\Users\\me"))
        );
        assert_eq!(parse_osc9_9("C:\\a"), Some(PathBuf::from("C:\\a")));
        assert_eq!(parse_osc9_9(""), None);
    }

    #[test]
    fn pick_prefers_the_reported_path_naming_the_same_directory() {
        let dir = std::env::temp_dir();
        let other = std::env::current_dir().unwrap();
        // The same directory, through a different spelling.
        let dotted = dir.join(".");
        assert_eq!(pick(Some(&dotted), Some(&dir)), Some(dotted.clone()));
        // A stale report loses to the process.
        assert_eq!(pick(Some(&other), Some(&dir)), Some(dir.clone()));
        // Without the process's, a reported directory that exists is used.
        assert_eq!(pick(Some(&dir), None), Some(dir.clone()));
        assert_eq!(pick(Some(Path::new("/no/such/dir")), None), None);
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn reads_own_working_directory() {
        let me = process_cwd(std::process::id()).unwrap();
        assert_eq!(
            me.canonicalize().unwrap(),
            std::env::current_dir().unwrap().canonicalize().unwrap()
        );
    }
}
