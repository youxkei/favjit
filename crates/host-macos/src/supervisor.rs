//! The link to the supervising watchdog (ADR-0008).
//!
//! Two inherited pipes and nothing else. A socket would need a name to bind and
//! a permission model to go with it; shared memory would need a region the
//! watchdog can map and agree on the layout of. A pipe the parent already holds
//! needs neither, and it closes by itself when either end dies — which is the
//! property that matters, since the watchdog's whole job is to notice death.
//!
//! Both directions are non-blocking. A supervisor that has stopped reading must
//! not be able to stall the process that is holding the keyboard, which is the
//! same rule ADR-0006 puts on every outbound host call.

use std::ffi::c_void;

extern "C" {
    fn read(fd: i32, buf: *mut c_void, count: usize) -> isize;
    fn write(fd: i32, buf: *const c_void, count: usize) -> isize;
    /// Variadic, as the header has it. Declared with a fixed third argument
    /// instead, this still compiles and still links — and on arm64 the argument
    /// goes in a register while `fcntl` reads it off the stack, so the flag it
    /// applies is whatever was there rather than the one asked for.
    fn fcntl(fd: i32, cmd: i32, ...) -> i32;
}

const F_SETFL: i32 = 4;
const O_NONBLOCK: i32 = 0x0004;

/// One end of the link, ready to be used without blocking.
///
/// Non-blocking in both directions, so neither a silent watchdog nor a full pipe
/// can hold up the loop.
pub fn end(name: &str) -> i32 {
    do_not_block(named_fd(name))
}

/// The descriptor a watchdog named in this variable.
///
/// `-1` where it named none, which is what a descriptor nobody handed over is
/// called on this machine: every call below is answered by the machine the same
/// way for it as for a pipe that has gone, so a run with no watchdog needs no
/// case of its own here — and whether it has one is read off this by whoever
/// asks (ADR-0006).
fn named_fd(name: &str) -> i32 {
    std::env::var(name)
        .ok()
        .and_then(|fd| fd.parse().ok())
        .unwrap_or(-1)
}

/// Stop this descriptor from blocking whatever reads or writes it.
fn do_not_block(fd: i32) -> i32 {
    unsafe { fcntl(fd, F_SETFL, O_NONBLOCK) };
    fd
}

/// Whichever end of the watchdog link this process was given.
#[derive(Debug, Clone, Copy)]
pub struct Supervisor {
    probe: i32,
    heartbeat: i32,
}

impl Supervisor {
    /// The two ends, each read by a call of its own.
    ///
    /// Named by whoever calls [`end`] rather than a name stated here: three
    /// processes have to agree on them, and `engine::supervision` is where that
    /// agreement is stated once (ADR-0006).
    ///
    /// Absence is not an error: running favjit by hand is how it gets measured,
    /// and refusing to start without a supervisor would make that impossible.
    /// What it costs is the protection, which is why the binary says so.
    pub fn on(probe: i32, heartbeat: i32) -> Self {
        Self { probe, heartbeat }
    }

    /// Neither end, for a run nobody is supervising.
    pub fn none() -> Self {
        Self {
            probe: -1,
            heartbeat: -1,
        }
    }

    pub fn is_supervised(&self) -> bool {
        self.probe >= 0 || self.heartbeat >= 0
    }

    /// Drain any probes the watchdog has sent, and say how many arrived.
    pub fn take_probes(&self) -> usize {
        let mut buf = [0u8; 64];
        how_many(unsafe { read(self.probe, buf.as_mut_ptr() as *mut c_void, buf.len()) })
    }

    /// Report that the loop came back round, and say whether it got through.
    ///
    /// The answer matters: a heartbeat that fails silently looks exactly like a
    /// healthy loop from in here and exactly like a wedged one from the
    /// watchdog, so the process holding the keyboard would be killed for a
    /// broken pipe rather than for a fault of its own.
    ///
    /// Carried on rather than blocked on: a full pipe means the watchdog is not
    /// reading, and it will time out and kill us, which is the outcome it exists
    /// for.
    pub fn beat(&self) -> Result<(), std::io::Error> {
        let byte = [b'.'];
        went(unsafe { write(self.heartbeat, byte.as_ptr() as *const c_void, 1) })
    }
}

/// How many bytes a read took, and none where it answered with a failure.
///
/// A pipe with nothing in it is not a failure here: what is wanted is a count of
/// probes, and none arriving is a count of zero.
fn how_many(answered: isize) -> usize {
    match answered > 0 {
        true => answered as usize,
        false => 0,
    }
}

/// Whether a write went, as the machine's own count of bytes says.
fn went(answered: isize) -> Result<(), std::io::Error> {
    match answered < 0 {
        true => Err(std::io::Error::last_os_error()),
        false => Ok(()),
    }
}
