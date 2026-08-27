//! The shared memory a trace is written into (ADR-0009).
//!
//! **Made by the watchdog and handed down**, not by favjit: the occasions that
//! most need a trace are the ones favjit cannot produce one on — a wedge cannot
//! be asked for its memory, and the kill that gives the keyboards back runs no
//! code at all. So the memory belongs to the process that survives that, and
//! favjit writes into a region somebody else is holding.
//!
//! Passed as an inherited handle, for the same reasons as the watchdog's pipes:
//! no name, no permission model of its own, and it goes when both ends are gone.
//! A handle marked inheritable arrives here with the value it had in the parent
//! ([docs/platform/windows/inherited-pipes.md](../../../docs/platform/windows/inherited-pipes.md)),
//! so what this reads out of the environment is that number.
//!
//! **Not file-backed**, for the reason the other machine's is not: a mapping over
//! a real file is the obvious way to make a trace survive anything, and it would
//! make a permanent on-disk keylog the normal state of the machine.

use core::ffi::c_void;

use favjit_host::Trouble;

use crate::ffi::Handle;

/// `FILE_MAP_ALL_ACCESS`, which is what a region written from both ends needs.
const FILE_MAP_ALL_ACCESS: u32 = 0x000F_001F;

#[link(name = "kernel32")]
extern "system" {
    fn MapViewOfFile(
        mapping: Handle,
        access: u32,
        offset_high: u32,
        offset_low: u32,
        to_map: usize,
    ) -> *mut c_void;
    fn UnmapViewOfFile(at: *const c_void) -> i32;
    fn CloseHandle(object: Handle) -> i32;
}

/// The pages, which go when this does.
///
/// Its own type rather than two fields of a `Region`, because unmapping is one
/// call and closing the handle is another: a `Drop` doing both would be a host
/// sequencing two calls into the machine, and the order between them is not a
/// thing to decide (ADR-0006) — it falls out of a mapping being dropped before
/// the handle holding it open.
struct Mapping(*mut u8);

impl Drop for Mapping {
    fn drop(&mut self) {
        unsafe { UnmapViewOfFile(self.0 as *const c_void) };
    }
}

/// The handle, which closes when this goes.
struct Mapped(Handle);

impl Drop for Mapped {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

/// A mapped region, and the handle it came from.
pub struct Region {
    /// Declared before the handle, which is the order they go in: the mapping is
    /// what the handle is holding open, and a handle closed first would leave
    /// pages mapped with nothing behind them.
    at: Mapping,
    len: usize,
    /// Kept so the mapping and the handle go together, and closed with it.
    /// Never read, because closing it is the whole of what it is for.
    _mapping: Mapped,
}

// The pointer is a mapping this owns; nothing else has it while this does.
unsafe impl Send for Region {}

impl Region {
    /// Map a region somebody else made.
    ///
    /// Not `pub`: a handle is a pointer here, and a public function taking one
    /// would be one a caller could hand anything to. [`on_handle`] takes the
    /// number the environment holds instead.
    fn from_handle(mapping: Handle, len: usize) -> std::io::Result<Self> {
        landed(
            unsafe { MapViewOfFile(mapping, FILE_MAP_ALL_ACCESS, 0, 0, len) },
            Mapped(mapping),
            len,
        )
    }

    /// The memory, for `engine` to write a trace into.
    pub fn bytes(&mut self) -> &mut [u8] {
        // Sound because this owns the mapping and hands out one borrow at a time.
        // The other process reading the same pages is the point of the region, and
        // it is why a reader takes a copy rather than reading in place.
        unsafe { std::slice::from_raw_parts_mut(self.at.0, self.len) }
    }
}

/// The region a mapping landed in, or what the machine said about it landing
/// nowhere.
///
/// The handle is taken by value and goes with the answer: where the mapping did
/// not happen there is no `Region` to hold it, and a handle left open on that
/// path is one nothing closes.
fn landed(answered: *mut c_void, mapping: Mapped, len: usize) -> std::io::Result<Region> {
    match answered.is_null() {
        true => Err(std::io::Error::last_os_error()),
        false => Ok(Region {
            at: Mapping(answered as *mut u8),
            len,
            _mapping: mapping,
        }),
    }
}

/// Map the region a handle names.
pub fn on_handle(handle: usize, len: usize) -> std::io::Result<Region> {
    Region::from_handle(handle as Handle, len)
}

/// What the machine said about a handle that was handed down and would not map.
pub fn no_region(handle: usize, error: std::io::Error) -> Trouble {
    Trouble(format!("the region on handle {handle}: {error}"))
}

/// The handle a parent named in the environment, where it named one.
///
/// Nothing where the name is unset or holds something that is not a handle,
/// which is favjit run by hand rather than a failure: a run nobody is
/// supervising records nothing, and that is the ordinary way to try it
/// (ADR-0008).
pub fn passed_down(variable: &str) -> Option<usize> {
    std::env::var(variable)
        .ok()
        .and_then(|handle| handle.parse().ok())
}
