//! Shared memory transmission (`t=s`): the payload names a shared memory
//! object holding the data. On Unix it's a POSIX object (`shm_open`), removed
//! with `shm_unlink` once read, as the protocol requires. On Windows it's a
//! named file mapping, which disappears when the client closes its handle.

use super::MAX_COMMAND_BYTES;

/// The bytes `offset..offset + size` of the named object (`size` 0: up to
/// its end).
pub(super) fn read(name: &str, offset: u64, size: u64) -> Result<Vec<u8>, String> {
    if name.is_empty() || name.len() > 250 || name.contains('\0') {
        return Err("EINVAL:bad shared memory name".into());
    }
    sys::read(name, offset, size)
}

/// The part of an object of `total` bytes to copy: (start, length).
fn range(total: u64, offset: u64, size: u64) -> Result<(usize, usize), String> {
    if offset > total {
        return Err("EINVAL:offset beyond the shared memory".into());
    }
    let available = total - offset;
    let len = if size == 0 {
        available
    } else {
        size.min(available)
    };
    if len > MAX_COMMAND_BYTES as u64 {
        return Err("EFBIG:shared memory too large".into());
    }
    Ok((offset as usize, len as usize))
}

#[cfg(unix)]
mod sys {
    use std::ffi::CString;
    use std::io;

    pub(super) fn read(name: &str, offset: u64, size: u64) -> Result<Vec<u8>, String> {
        // One name, not a path: "/name" or "name".
        let bare = name.strip_prefix('/').unwrap_or(name);
        if bare.is_empty() || bare.contains('/') {
            return Err("EINVAL:bad shared memory name".into());
        }
        let cname =
            CString::new(format!("/{bare}")).map_err(|_| "EINVAL:bad shared memory name")?;
        // SAFETY: `cname` is a valid C string.
        let fd = unsafe { libc::shm_open(cname.as_ptr(), libc::O_RDONLY, 0) };
        if fd < 0 {
            return Err(format!("EBADF:{}", io::Error::last_os_error()));
        }
        let result = copy(fd, offset, size);
        // SAFETY: `fd` was opened above; the name is valid. The object is
        // removed once read, as the protocol requires.
        unsafe {
            libc::close(fd);
            libc::shm_unlink(cname.as_ptr());
        }
        result
    }

    fn copy(fd: libc::c_int, offset: u64, size: u64) -> Result<Vec<u8>, String> {
        // SAFETY: `stat` is plain data, filled by `fstat` on a valid fd.
        let mut stat: libc::stat = unsafe { std::mem::zeroed() };
        if unsafe { libc::fstat(fd, &mut stat) } != 0 {
            return Err(format!("EBADF:{}", io::Error::last_os_error()));
        }
        let total = stat.st_size.max(0) as u64;
        let (start, len) = super::range(total, offset, size)?;
        if len == 0 {
            return Ok(Vec::new());
        }
        // Mapped rather than read: macOS can't `read` shared memory.
        let mapped = (start + len) as libc::size_t;
        // SAFETY: a read-only shared mapping of `mapped` bytes, which the
        // object holds (checked against its size above).
        let addr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                mapped,
                libc::PROT_READ,
                libc::MAP_SHARED,
                fd,
                0,
            )
        };
        if addr == libc::MAP_FAILED {
            return Err(format!("EBADF:{}", io::Error::last_os_error()));
        }
        // SAFETY: `start + len <= mapped` bytes are mapped at `addr`.
        let data =
            unsafe { std::slice::from_raw_parts((addr as *const u8).add(start), len) }.to_vec();
        // SAFETY: unmaps the mapping made above.
        unsafe { libc::munmap(addr, mapped) };
        Ok(data)
    }
}

#[cfg(windows)]
mod sys {
    use std::ffi::c_void;
    use std::io;

    const FILE_MAP_READ: u32 = 0x0004;

    #[repr(C)]
    pub(super) struct MemoryBasicInformation {
        base_address: *mut c_void,
        allocation_base: *mut c_void,
        allocation_protect: u32,
        partition_id: u16,
        region_size: usize,
        state: u32,
        protect: u32,
        kind: u32,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn OpenFileMappingW(access: u32, inherit: i32, name: *const u16) -> isize;
        fn MapViewOfFile(
            mapping: isize,
            access: u32,
            high: u32,
            low: u32,
            len: usize,
        ) -> *mut c_void;
        fn UnmapViewOfFile(address: *const c_void) -> i32;
        fn CloseHandle(handle: isize) -> i32;
        fn VirtualQuery(
            address: *const c_void,
            info: *mut MemoryBasicInformation,
            len: usize,
        ) -> usize;
    }

    pub(super) fn read(name: &str, offset: u64, size: u64) -> Result<Vec<u8>, String> {
        let wide: Vec<u16> = name.encode_utf16().chain([0]).collect();
        // SAFETY: `wide` is a NUL-terminated UTF-16 string.
        let mapping = unsafe { OpenFileMappingW(FILE_MAP_READ, 0, wide.as_ptr()) };
        if mapping == 0 {
            return Err(format!("EBADF:{}", io::Error::last_os_error()));
        }
        // SAFETY: maps the whole object read-only.
        let view = unsafe { MapViewOfFile(mapping, FILE_MAP_READ, 0, 0, 0) };
        let result = if view.is_null() {
            Err(format!("EBADF:{}", io::Error::last_os_error()))
        } else {
            // The view's size is the object's, rounded up to whole pages.
            let mut info: MemoryBasicInformation = unsafe { std::mem::zeroed() };
            // SAFETY: `info` is large enough for the answer.
            let got = unsafe {
                VirtualQuery(
                    view,
                    &mut info,
                    std::mem::size_of::<MemoryBasicInformation>(),
                )
            };
            let result = if got == 0 {
                Err(format!("EBADF:{}", io::Error::last_os_error()))
            } else {
                super::range(info.region_size as u64, offset, size).map(|(start, len)| {
                    // SAFETY: `start + len` is within the mapped region.
                    unsafe { std::slice::from_raw_parts((view as *const u8).add(start), len) }
                        .to_vec()
                })
            };
            // SAFETY: unmaps the view made above.
            unsafe { UnmapViewOfFile(view) };
            result
        };
        // SAFETY: closes the handle opened above.
        unsafe { CloseHandle(mapping) };
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_stay_inside_the_object() {
        assert_eq!(range(10, 0, 0), Ok((0, 10)));
        assert_eq!(range(10, 4, 3), Ok((4, 3)));
        assert_eq!(range(10, 4, 100), Ok((4, 6)));
        assert!(range(10, 11, 0).is_err());
    }

    #[test]
    fn bad_names_are_refused() {
        assert!(read("", 0, 0).is_err());
        assert!(read("a\0b", 0, 0).is_err());
        #[cfg(unix)]
        assert!(read("/a/b", 0, 0).is_err());
    }

    #[test]
    fn missing_objects_are_reported() {
        let err = read("lohtty-test-no-such-object", 0, 0).unwrap_err();
        assert!(err.starts_with("EBADF:"), "{err}");
    }

    const DATA: &[u8; 16] = b"0123456789abcdef";

    #[cfg(unix)]
    #[test]
    fn reads_a_posix_object_then_removes_it() {
        use std::ffi::CString;
        let name = format!("/lohtty-test-{}", std::process::id());
        let cname = CString::new(name.clone()).unwrap();
        // SAFETY: creates, sizes and fills a 16-byte object through a
        // writable mapping (macOS can't `write` shared memory).
        unsafe {
            let fd = libc::shm_open(cname.as_ptr(), libc::O_CREAT | libc::O_RDWR, 0o600);
            assert!(fd >= 0);
            assert_eq!(libc::ftruncate(fd, 16), 0);
            let addr = libc::mmap(
                std::ptr::null_mut(),
                16,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                fd,
                0,
            );
            assert_ne!(addr, libc::MAP_FAILED);
            std::ptr::copy_nonoverlapping(DATA.as_ptr(), addr as *mut u8, 16);
            libc::munmap(addr, 16);
            libc::close(fd);
        }
        // Named without the slash, as `kitty icat` does.
        assert_eq!(read(&name[1..], 4, 3).unwrap(), b"456");
        // SAFETY: `cname` is a valid C string.
        let reopened = unsafe { libc::shm_open(cname.as_ptr(), libc::O_RDONLY, 0) };
        assert!(reopened < 0, "the object is removed once read");
    }

    #[cfg(windows)]
    #[test]
    fn reads_a_named_file_mapping() {
        use std::ffi::c_void;
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn CreateFileMappingW(
                file: isize,
                attributes: *const c_void,
                protect: u32,
                high: u32,
                low: u32,
                name: *const u16,
            ) -> isize;
            fn MapViewOfFile(
                mapping: isize,
                access: u32,
                high: u32,
                low: u32,
                len: usize,
            ) -> *mut c_void;
            fn UnmapViewOfFile(address: *const c_void) -> i32;
            fn CloseHandle(handle: isize) -> i32;
        }
        const PAGE_READWRITE: u32 = 0x04;
        const FILE_MAP_WRITE: u32 = 0x02;
        let name = format!("lohtty-test-{}", std::process::id());
        let wide: Vec<u16> = name.encode_utf16().chain([0]).collect();
        // SAFETY: a 16-byte page-file-backed mapping, filled through a view;
        // the handle stays open while it's read, as a client's would.
        let mapping = unsafe {
            let mapping =
                CreateFileMappingW(-1, std::ptr::null(), PAGE_READWRITE, 0, 16, wide.as_ptr());
            assert_ne!(mapping, 0);
            let view = MapViewOfFile(mapping, FILE_MAP_WRITE, 0, 0, 16);
            assert!(!view.is_null());
            std::ptr::copy_nonoverlapping(DATA.as_ptr(), view as *mut u8, 16);
            UnmapViewOfFile(view);
            mapping
        };
        assert_eq!(read(&name, 4, 3).unwrap(), b"456");
        assert_eq!(&read(&name, 0, 16).unwrap()[..], &DATA[..]);
        // SAFETY: closes the handle created above.
        unsafe { CloseHandle(mapping) };
    }
}
