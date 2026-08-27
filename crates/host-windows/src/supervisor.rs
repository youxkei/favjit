//! The link to the supervising watchdog, on this end (ADR-0008).
//!
//! Two inherited pipes and nothing else, for the reason the macOS end has two: a pipe
//! the parent already holds needs no name to bind and no permission model to go with
//! it, and it closes by itself when either end dies — which is the property that
//! matters, since the watchdog's whole job is to notice death.
//!
//! Both directions are non-blocking. A supervisor that has stopped reading must not
//! be able to stall the process that is holding the keyboards, which is the same rule
//! ADR-0006 puts on every outbound host call.

use core::ffi::c_void;

use crate::ffi::{Handle, ReadFile, SetNamedPipeHandleState, WriteFile, PIPE_NOWAIT};

/// One end of the link, ready to be used without blocking.
///
/// Non-blocking in both directions, so neither a silent watchdog nor a full pipe
/// can hold up the loop.
pub fn end(name: &str) -> usize {
    do_not_wait(named_handle(name))
}

/// The handle a watchdog named in this variable.
///
/// Zero where it named none, which is what Windows leaves where a handle was not
/// inherited: every call below is answered the same way for it as for a pipe that
/// has gone, so a run with no watchdog needs no case of its own here — and
/// whether it has one is read off this by whoever asks (ADR-0006).
fn named_handle(name: &str) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|handle| handle.parse::<usize>().ok())
        .unwrap_or(0)
}

/// Stop this pipe from blocking whatever reads or writes it.
fn do_not_wait(handle: usize) -> usize {
    let mut mode = PIPE_NOWAIT;
    unsafe {
        SetNamedPipeHandleState(
            handle as Handle,
            &mut mode,
            core::ptr::null_mut(),
            core::ptr::null_mut(),
        )
    };
    handle
}

/// Whichever end of the watchdog link this process was given.
///
/// The handles are kept as numbers rather than as `Handle`, because the capture thread
/// needs a copy of this and a raw pointer is not `Send`. A handle is a number the API
/// handed over and not memory this code reads, so nothing is lost by saying so.
#[derive(Debug, Clone, Copy)]
pub struct Supervisor {
    probe: usize,
    heartbeat: usize,
}

impl Supervisor {
    /// The two ends, each read by a call of its own.
    ///
    /// Named by whoever calls [`end`] rather than a name stated here: three
    /// processes have to agree on them, and `engine::supervision` is where that
    /// agreement is stated once (ADR-0006).
    ///
    /// Absence is not an error: running favjit by hand is how it gets measured, and
    /// refusing to start without a supervisor would make that impossible. What it
    /// costs is the protection, which is why the run says so.
    pub fn on(probe: usize, heartbeat: usize) -> Self {
        Self { probe, heartbeat }
    }

    /// Neither end, for a run nobody is supervising.
    pub fn none() -> Self {
        Self {
            probe: 0,
            heartbeat: 0,
        }
    }

    pub fn is_supervised(&self) -> bool {
        self.probe != 0 || self.heartbeat != 0
    }

    /// Whether there is a probe pipe to look at, for a loop deciding whether to wake
    /// itself in order to look.
    pub fn watches_probes(&self) -> bool {
        self.probe != 0
    }

    /// Drain any probes the watchdog has sent, and say how many arrived — none
    /// unless `now` says this is the moment to look.
    ///
    /// Nothing distinguishes "no probes" from "the read failed", and nothing needs
    /// to: both mean there is nothing to answer this time round, and a non-blocking
    /// pipe with nothing in it is reported as either by the platform. That is what
    /// lets the read be made either way and `now` decide which pipe it is made on:
    /// a read of no pipe at all fails, which is the same nothing — and the same
    /// nothing a run with no watchdog gets.
    pub fn take_probes(&self, now: bool) -> usize {
        let mut buffer = [0u8; 64];
        let mut read = 0u32;
        let taken = unsafe {
            ReadFile(
                the_pipe(self.probe, now),
                buffer.as_mut_ptr().cast::<c_void>(),
                buffer.len() as u32,
                &mut read,
                core::ptr::null_mut(),
            )
        };
        how_many(taken, read)
    }

    /// Report that the loop came back round, and say whether it got through.
    ///
    /// The answer matters: a heartbeat that fails silently looks exactly like a
    /// healthy loop from in here and exactly like a wedged one from the watchdog, so
    /// the process holding the keyboards would be ended for a broken pipe rather than
    /// for a fault of its own.
    ///
    /// A pipe with no room is that same failure: the watchdog is not reading, and it
    /// will time out and end this process, which is the outcome it exists for.
    pub fn beat(&self) -> Result<(), std::io::Error> {
        let byte = *b".";
        let mut written = 0u32;
        let sent = unsafe {
            WriteFile(
                self.heartbeat as Handle,
                byte.as_ptr().cast::<c_void>(),
                1,
                &mut written,
                core::ptr::null_mut(),
            )
        };
        went(sent, written)
    }
}

/// The pipe to read the probes off, and no pipe at all where this is not the
/// moment to look.
///
/// No pipe rather than no read: a read that had to be asked about first would be
/// a turning beside the call it makes (ADR-0006), and a read of no pipe fails,
/// which is the same nothing a pipe with nothing in it answers.
fn the_pipe(probe: usize, now: bool) -> Handle {
    match now {
        true => probe as Handle,
        false => core::ptr::null_mut(),
    }
}

/// How many bytes a read took, and none where it answered with a failure.
///
/// A pipe with nothing in it is not a failure here: what is wanted is a count of
/// probes, and none arriving is a count of zero.
fn how_many(taken: i32, read: u32) -> usize {
    match taken != 0 {
        true => read as usize,
        false => 0,
    }
}

/// Whether the whole of a write went, as the call's answer and its count say.
///
/// Both, because a write reported as having succeeded with nothing written is a
/// heartbeat that never left.
fn went(sent: i32, written: u32) -> Result<(), std::io::Error> {
    match sent != 0 && written == 1 {
        true => Ok(()),
        false => Err(std::io::Error::last_os_error()),
    }
}
