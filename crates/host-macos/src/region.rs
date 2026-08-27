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

use std::ffi::{c_int, c_void};

use favjit_host::Trouble;

const PROT_READ: c_int = 0x01;
const PROT_WRITE: c_int = 0x02;
const MAP_SHARED: c_int = 0x0001;
const MAP_FAILED: *mut c_void = usize::MAX as *mut c_void;

extern "C" {
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
}

/// The pages, which go when this does.
///
/// Its own type rather than two fields of a `Region`, because unmapping is one
/// call and closing the descriptor is another: a `Drop` doing both would be a
/// host sequencing two calls into the machine, and the order between them is not
/// a thing to decide (ADR-0006) — it falls out of a mapping being dropped before
/// the descriptor holding it open.
struct Mapping {
    at: *mut u8,
    len: usize,
}

impl Drop for Mapping {
    fn drop(&mut self) {
        unsafe { munmap(self.at as *mut c_void, self.len) };
    }
}

/// The descriptor, which closes when this goes.
struct Descriptor(c_int);

impl Drop for Descriptor {
    fn drop(&mut self) {
        unsafe { close(self.0) };
    }
}

/// A mapped region, and the descriptor it came from.
///
/// Mapping one and nothing else. Making the shared memory object is the
/// supervisor's, and it is written where its `main` is: the memory has to belong
/// to the process that survives the kill (ADR-0009), so a second way of making
/// one here would be a second way of making one that nothing uses.
pub struct Region {
    /// Declared before the descriptor, which is the order they go in: the
    /// mapping is what the descriptor is holding open, and a descriptor closed
    /// first would leave pages mapped with nothing behind them.
    mapping: Mapping,
    /// Kept open so it can be handed to a child, and closed with the mapping.
    fd: Descriptor,
}

// The pointer is a mapping this owns; nothing else has it while this does.
unsafe impl Send for Region {}

impl Region {
    /// Map a region somebody else made.
    pub fn from_fd(fd: c_int, len: usize) -> std::io::Result<Self> {
        landed(
            unsafe {
                mmap(
                    std::ptr::null_mut(),
                    len,
                    PROT_READ | PROT_WRITE,
                    MAP_SHARED,
                    fd,
                    0,
                )
            },
            Descriptor(fd),
            len,
        )
    }

    /// The descriptor, for handing to a child.
    pub fn fd(&self) -> c_int {
        self.fd.0
    }

    /// The memory, for `engine` to write a trace into.
    pub fn bytes(&mut self) -> &mut [u8] {
        // Sound because this owns the mapping and hands out one borrow at a time.
        // The other process writing the same pages is the point of the region, and
        // it is why a reader takes a copy rather than reading in place.
        unsafe { std::slice::from_raw_parts_mut(self.mapping.at, self.mapping.len) }
    }

    /// A copy of what the region holds.
    ///
    /// Copied rather than borrowed, because the process writing it may still be
    /// running: a reader walking the ring while it moves would see a record half
    /// written. A copy can be inconsistent at its edges too, which is why the
    /// reader in `engine` treats an unreadable record as one to skip.
    pub fn snapshot(&self) -> Vec<u8> {
        let mut out = vec![0u8; self.mapping.len];
        // Sound for the same reason as above, and read-only here.
        out.copy_from_slice(unsafe {
            std::slice::from_raw_parts(self.mapping.at, self.mapping.len)
        });
        out
    }
}

/// The region a mapping landed in, or what the machine said about not landing.
///
/// `MAP_FAILED` and not a sign: a mapping's answer is an address, and every
/// address including the one this compares against is a value the call could
/// otherwise have returned.
///
/// The descriptor is taken by value and goes with the answer: where the mapping
/// did not happen there is no `Region` to hold it, and a descriptor left open on
/// that path is one nothing closes.
fn landed(answered: *mut c_void, fd: Descriptor, len: usize) -> std::io::Result<Region> {
    match answered == MAP_FAILED {
        true => Err(std::io::Error::last_os_error()),
        false => Ok(Region {
            mapping: Mapping {
                at: answered as *mut u8,
                len,
            },
            fd,
        }),
    }
}

/// The descriptor a parent named in the environment, where it named one.
///
/// Nothing where the name is unset or holds something that is not a descriptor,
/// which is a run nobody asked for a trace from rather than a failure. What that
/// means for the run is decided where the region is asked for (ADR-0006).
pub fn passed_down(variable: &str) -> Option<c_int> {
    std::env::var(variable).ok().and_then(|fd| fd.parse().ok())
}

/// What the machine said about a descriptor that was handed down and would not
/// map.
pub fn no_region(fd: c_int, error: std::io::Error) -> Trouble {
    Trouble(format!("the region on fd {fd}: {error}"))
}
