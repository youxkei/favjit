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

/// The file descriptors a watchdog hands down, named in the environment because
/// a child cannot be told which numbers to expect any other way.
pub const PROBE_FD_VAR: &str = "FAVJIT_PROBE_FD";
pub const HEARTBEAT_FD_VAR: &str = "FAVJIT_HEARTBEAT_FD";

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

/// Whichever end of the watchdog link this process was given.
#[derive(Debug, Clone, Copy, Default)]
pub struct Supervisor {
    probe: Option<i32>,
    heartbeat: Option<i32>,
}

impl Supervisor {
    /// Read the descriptors out of the environment, if a watchdog started us.
    ///
    /// Absence is not an error: running favjit by hand is how it gets measured,
    /// and refusing to start without a supervisor would make that impossible.
    /// What it costs is the protection, which is why the binary says so.
    pub fn from_env() -> Self {
        let fd = |name: &str| {
            std::env::var(name)
                .ok()
                .and_then(|v| v.parse::<i32>().ok())
                .inspect(|&fd| {
                    // Non-blocking in both directions, so neither a silent
                    // watchdog nor a full pipe can hold up the loop.
                    unsafe { fcntl(fd, F_SETFL, O_NONBLOCK) };
                })
        };
        Self {
            probe: fd(PROBE_FD_VAR),
            heartbeat: fd(HEARTBEAT_FD_VAR),
        }
    }

    pub fn is_supervised(&self) -> bool {
        self.probe.is_some() || self.heartbeat.is_some()
    }

    /// The probe descriptor, for a reader to wait on.
    pub fn probe_fd(&self) -> Option<i32> {
        self.probe
    }

    /// Drain any probes the watchdog has sent, and say how many arrived.
    pub fn take_probes(&self) -> usize {
        let Some(fd) = self.probe else { return 0 };
        let mut buf = [0u8; 64];
        let read = unsafe { read(fd, buf.as_mut_ptr() as *mut c_void, buf.len()) };
        if read > 0 {
            read as usize
        } else {
            0
        }
    }

    /// Report that the loop came back round, and say whether it got through.
    ///
    /// The answer matters: a heartbeat that fails silently looks exactly like a
    /// healthy loop from in here and exactly like a wedged one from the
    /// watchdog, so the process holding the keyboard would be killed for a
    /// broken pipe rather than for a fault of its own.
    pub fn beat(&self) -> Result<(), std::io::Error> {
        let Some(fd) = self.heartbeat else {
            return Ok(());
        };
        let byte = [b'.'];
        // Carried on rather than blocked on: a full pipe means the watchdog is
        // not reading, and it will time out and kill us, which is the outcome it
        // exists for.
        if unsafe { write(fd, byte.as_ptr() as *const c_void, 1) } < 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }
}
