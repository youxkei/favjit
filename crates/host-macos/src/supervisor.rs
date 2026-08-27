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

/// The descriptor a watchdog named in this variable.
///
/// `-1` where it named none, which is what a descriptor nobody handed over is
/// called on this machine: every call below is answered by the machine the same
/// way for it as for a pipe that has gone, so a run with no watchdog needs no
/// case of its own here — and whether it has one is read off this by whoever
/// asks (ADR-0006).
pub fn named_fd(name: &str) -> i32 {
    a_descriptor(std::env::var(name))
}

/// The descriptor a variable holds, and `-1` where it holds anything else.
///
/// Apart from the read rather than spelled onto the end of it, because the read
/// is the one call: what it answered is a string whether or not the string is a
/// number (ADR-0006).
fn a_descriptor(value: Result<String, std::env::VarError>) -> i32 {
    value.ok().and_then(|fd| fd.parse().ok()).unwrap_or(-1)
}

/// Stop this descriptor from blocking whatever reads or writes it.
///
/// Asked for apart from the descriptor rather than folded into reading it,
/// because both are calls into the machine and a host makes one of those per
/// operation: which of them happens first is the order a run drives (ADR-0006).
pub fn do_not_block(fd: i32) -> i32 {
    unsafe { fcntl(fd, F_SETFL, O_NONBLOCK) };
    fd
}

/// Whichever end of the watchdog link this process was given.
/// The two ends in the open rather than behind constructors, because putting two
/// numbers in two fields is not something to make: a run nobody is supervising
/// is `-1` in each, and which descriptors these are is read by whoever names
/// them (ADR-0006).
#[derive(Debug, Clone, Copy)]
pub struct Supervisor {
    pub probe: i32,
    pub heartbeat: i32,
}

impl Supervisor {
    /// Whether either end is a descriptor somebody handed over.
    ///
    /// `-1` is what this machine calls one nobody did, and reading it that way
    /// is what makes a run with no watchdog need no case of its own in the calls
    /// below.
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
