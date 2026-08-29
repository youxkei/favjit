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

use favjit_core::supervision::{HEARTBEAT, PROBE};

use crate::ffi::{Handle, ReadFile, SetNamedPipeHandleState, WriteFile, PIPE_NOWAIT};

/// Whichever end of the watchdog link this process was given.
///
/// The handles are kept as numbers rather than as `Handle`, because the capture thread
/// needs a copy of this and a raw pointer is not `Send`. A handle is a number the API
/// handed over and not memory this code reads, so nothing is lost by saying so.
#[derive(Debug, Clone, Copy, Default)]
pub struct Supervisor {
    probe: Option<usize>,
    heartbeat: Option<usize>,
}

impl Supervisor {
    /// Read the handles out of the environment, if a watchdog started us.
    ///
    /// Absence is not an error: running favjit by hand is how it gets measured, and
    /// refusing to start without a supervisor would make that impossible. What it
    /// costs is the protection, which is why the run says so.
    pub fn from_env() -> Self {
        let handle = |name: &str| {
            std::env::var(name)
                .ok()
                .and_then(|value| value.parse::<usize>().ok())
                .filter(|handle| *handle != 0)
                .inspect(|handle| {
                    // Non-blocking in both directions, so neither a silent watchdog
                    // nor a full pipe can hold up the loop.
                    let mut mode = PIPE_NOWAIT;
                    unsafe {
                        SetNamedPipeHandleState(
                            *handle as Handle,
                            &mut mode,
                            core::ptr::null_mut(),
                            core::ptr::null_mut(),
                        )
                    };
                })
        };
        Self {
            probe: handle(PROBE),
            heartbeat: handle(HEARTBEAT),
        }
    }

    pub fn is_supervised(&self) -> bool {
        self.probe.is_some() || self.heartbeat.is_some()
    }

    /// Whether there is a probe pipe to look at, for a loop deciding whether to wake
    /// itself in order to look.
    pub fn watches_probes(&self) -> bool {
        self.probe.is_some()
    }

    /// Drain any probes the watchdog has sent, and say how many arrived.
    ///
    /// Nothing distinguishes "no probes" from "the read failed", and nothing needs
    /// to: both mean there is nothing to answer this time round, and a non-blocking
    /// pipe with nothing in it is reported as either by the platform.
    pub fn take_probes(&self) -> usize {
        let Some(handle) = self.probe else { return 0 };
        let mut buffer = [0u8; 64];
        let mut read = 0u32;
        let taken = unsafe {
            ReadFile(
                handle as Handle,
                buffer.as_mut_ptr().cast::<c_void>(),
                buffer.len() as u32,
                &mut read,
                core::ptr::null_mut(),
            )
        };
        match taken != 0 {
            true => read as usize,
            false => 0,
        }
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
        let Some(handle) = self.heartbeat else {
            return Ok(());
        };
        let byte = *b".";
        let mut written = 0u32;
        let sent = unsafe {
            WriteFile(
                handle as Handle,
                byte.as_ptr().cast::<c_void>(),
                1,
                &mut written,
                core::ptr::null_mut(),
            )
        };
        match sent != 0 && written == 1 {
            true => Ok(()),
            false => Err(std::io::Error::last_os_error()),
        }
    }
}
