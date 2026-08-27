//! The shared memory a trace is written into (ADR-0009).
//!
//! **Created by the watchdog and handed down**, not allocated by favjit: the
//! occasions that most need a trace are the ones favjit cannot produce one on —
//! a wedge cannot be asked for its memory, and the kill that gives the keyboard
//! back runs no code at all. So the memory belongs to the process that survives
//! that, and favjit writes into a region somebody else is holding.
//!
//! Passed as an inherited descriptor, for the same reasons as the watchdog's
//! pipes: no name, no permission model, and it closes when both ends are gone.
//! The shared memory object is unlinked as soon as it is mapped, so it has no name
//! anything else could open.
//!
//! **Not file-backed.** Mapping it to a file is the obvious way to make a trace
//! survive anything, and it would make a permanent on-disk keylog the normal state
//! of the machine.

use std::ffi::{c_int, c_void, CStr};

/// The descriptor the watchdog passes down.
pub const TRACE_FD_VAR: &str = "FAVJIT_TRACE_FD";

/// How big a region the watchdog makes.
///
/// A megabyte is thirty-two thousand records: minutes of typing, or seconds of
/// pointer movement, which is the asymmetry the checkpoint budget in `core`
/// exists for. Fixed rather than configurable because the only reason to change
/// it is to hold more keystrokes, and that decision belongs with the one about
/// keeping them at all.
pub const TRACE_BYTES: usize = 1024 * 1024;

const PROT_READ: c_int = 0x01;
const PROT_WRITE: c_int = 0x02;
const MAP_SHARED: c_int = 0x0001;
const MAP_FAILED: *mut c_void = usize::MAX as *mut c_void;
const O_RDWR: c_int = 0x0002;
const O_CREAT: c_int = 0x0200;
const O_EXCL: c_int = 0x0800;

extern "C" {
    fn shm_open(name: *const i8, oflag: c_int, mode: u32) -> c_int;
    fn shm_unlink(name: *const i8) -> c_int;
    fn ftruncate(fd: c_int, length: i64) -> c_int;
    fn mmap(
        addr: *mut c_void,
        len: usize,
        prot: c_int,
        flags: c_int,
        fd: c_int,
        offset: i64,
    ) -> *mut c_void;
    fn munmap(addr: *mut c_void, len: usize) -> c_int;
    fn close(fd: c_int) -> c_int;
    fn getpid() -> i32;
}

/// A mapped region, and the descriptor it came from.
pub struct Region {
    at: *mut u8,
    len: usize,
    /// Kept open so it can be handed to a child, and closed with the mapping.
    fd: c_int,
}

// The pointer is a mapping this owns; nothing else has it while this does.
unsafe impl Send for Region {}

impl Region {
    /// Make one, and unlink its name immediately.
    pub fn create(len: usize) -> std::io::Result<Self> {
        // The pid in the name only has to make it unique for the moment between
        // creating and unlinking; macOS caps a shared memory name at 31 bytes, so
        // there is no room for anything more descriptive anyway.
        let name = format!("/favjit-{}\0", unsafe { getpid() });
        let as_c = CStr::from_bytes_with_nul(name.as_bytes())
            .map_err(|_| std::io::Error::other("the region's name is not a C string"))?;

        let fd = unsafe { shm_open(as_c.as_ptr(), O_RDWR | O_CREAT | O_EXCL, 0o600) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        // Unlinked while it is still open: the mapping survives, and no other
        // process can open it by name — a trace holds keystrokes, so the fewer
        // ways in the better.
        unsafe { shm_unlink(as_c.as_ptr()) };

        if unsafe { ftruncate(fd, len as i64) } < 0 {
            let error = std::io::Error::last_os_error();
            unsafe { close(fd) };
            return Err(error);
        }
        Self::map(fd, len)
    }

    /// Map a region somebody else made.
    pub fn from_fd(fd: c_int, len: usize) -> std::io::Result<Self> {
        Self::map(fd, len)
    }

    fn map(fd: c_int, len: usize) -> std::io::Result<Self> {
        let at = unsafe {
            mmap(
                std::ptr::null_mut(),
                len,
                PROT_READ | PROT_WRITE,
                MAP_SHARED,
                fd,
                0,
            )
        };
        if at == MAP_FAILED {
            let error = std::io::Error::last_os_error();
            unsafe { close(fd) };
            return Err(error);
        }
        Ok(Self {
            at: at as *mut u8,
            len,
            fd,
        })
    }

    /// The descriptor, for handing to a child.
    pub fn fd(&self) -> c_int {
        self.fd
    }

    /// The memory, for `core` to write a trace into.
    pub fn bytes(&mut self) -> &mut [u8] {
        // Sound because this owns the mapping and hands out one borrow at a time.
        // The other process writing the same pages is the point of the region, and
        // it is why a reader takes a copy rather than reading in place.
        unsafe { std::slice::from_raw_parts_mut(self.at, self.len) }
    }

    /// A copy of what the region holds.
    ///
    /// Copied rather than borrowed, because the process writing it may still be
    /// running: a reader walking the ring while it moves would see a record half
    /// written. A copy can be inconsistent at its edges too, which is why the
    /// reader in `core` treats an unreadable record as one to skip.
    pub fn snapshot(&self) -> Vec<u8> {
        let mut out = vec![0u8; self.len];
        // Sound for the same reason as above, and read-only here.
        out.copy_from_slice(unsafe { std::slice::from_raw_parts(self.at, self.len) });
        out
    }
}

impl Drop for Region {
    fn drop(&mut self) {
        unsafe {
            munmap(self.at as *mut c_void, self.len);
            close(self.fd);
        }
    }
}

/// The region a parent passed down, if there is one.
pub fn inherited() -> Option<Region> {
    let fd: c_int = std::env::var(TRACE_FD_VAR).ok()?.parse().ok()?;
    match Region::from_fd(fd, TRACE_BYTES) {
        Ok(region) => Some(region),
        Err(error) => {
            log::warn!("cannot map the trace region on fd {fd}: {error}; running without a trace");
            None
        }
    }
}
