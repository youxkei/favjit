//! The Win32 half: two anonymous pipes the child inherits, and a child to end.
//!
//! The same shape as the POSIX half and for the same reason (ADR-0008): a pipe the
//! parent already holds needs no name to bind and no permission model to go with it,
//! and only the child can reach it. A named pipe or a loopback socket would be
//! reachable by every other process this person runs.
//!
//! Two things here were established on hardware rather than assumed, and are recorded
//! under [docs/platform/windows/inherited-pipes.md](../../../docs/platform/windows/inherited-pipes.md):
//! that `std::process::Command` passes on a handle marked inheritable, and that
//! `PIPE_NOWAIT` is accepted on an anonymous pipe.
//!
//! Calls and nothing else. When a probe is due, how long a silence may last, and that
//! a process is asked to stop before it is stopped are `favjit_core::watchdog`'s.

use core::mem::size_of;
use core::time::Duration;
use std::ffi::c_void;
use std::os::windows::io::FromRawHandle;
use std::process::{Child, Command};

use favjit_core::supervision::{HEARTBEAT, PROBE};
use favjit_core::watchdog::{Beat, BeatKind, Exit, WatchdogHost};
use favjit_core::Instant;
use log::{debug, error, info, warn};

use crate::beats::{Arrival, Beats, Clock};

/// Every opaque Win32 handle. One type for all of them because none is dereferenced
/// here: they are numbers this code passes back to the API it got them from.
type Handle = *mut c_void;

/// `SECURITY_ATTRIBUTES`, in the header's order.
///
/// Only `bInheritHandle` is set. A null descriptor is the default one for this
/// process's token, which is what confines the pipe to this user.
#[repr(C)]
struct SecurityAttributes {
    length: u32,
    descriptor: *mut c_void,
    inherit: i32,
}

/// The bit `SetHandleInformation` clears to keep a handle out of a child.
const HANDLE_FLAG_INHERIT: u32 = 0x0000_0001;

extern "system" {
    fn CreatePipe(
        read: *mut Handle,
        write: *mut Handle,
        attributes: *const SecurityAttributes,
        size: u32,
    ) -> i32;
    fn SetHandleInformation(object: Handle, mask: u32, flags: u32) -> i32;
    fn WriteFile(
        file: Handle,
        buffer: *const c_void,
        to_write: u32,
        written: *mut u32,
        overlapped: *mut c_void,
    ) -> i32;
    fn CloseHandle(object: Handle) -> i32;
}

/// What the last failing call set, spelled out.
fn last_error() -> String {
    std::io::Error::last_os_error().to_string()
}

/// A pipe whose two ends can both cross into a child, as `(read, write)`.
///
/// Both inheritable, because which end the child needs differs per pipe and the one
/// this process keeps is taken back out of the child below. A pipe with neither end
/// inheritable is one the child cannot reach at all.
fn make_pipe() -> Option<(Handle, Handle)> {
    let mut read: Handle = core::ptr::null_mut();
    let mut write: Handle = core::ptr::null_mut();
    let attributes = SecurityAttributes {
        length: size_of::<SecurityAttributes>() as u32,
        descriptor: core::ptr::null_mut(),
        inherit: 1,
    };
    // A zero size asks for the system's default buffer, which is far more than a
    // stream of single bytes needs.
    match unsafe { CreatePipe(&mut read, &mut write, &attributes, 0) } {
        0 => None,
        _ => Some((read, write)),
    }
}

/// Keep this end out of the child.
///
/// The point is not tidiness: a write end the child inherited would hold the pipe
/// open after the child had gone, so the read never ends and a dead process looks
/// like a quiet one.
fn keep(handle: Handle) {
    if unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0) } == 0 {
        warn!(
            "a handle could not be kept out of the child: {}",
            last_error()
        );
    }
}

/// This machine, as the judgement's boundary.
pub struct Windows {
    child_args: Vec<String>,
    /// Named for what the flag asks for. There is no trace on this platform, so what
    /// it holds is the path nothing is written to.
    trace_out: Option<String>,
    clock: Option<Clock>,
    /// Our end of the probe pipe, once there is a child at the other end of it.
    probe_write: Option<Handle>,
    beats: Option<Beats>,
    child: Option<Child>,
}

impl Windows {
    pub fn new(child_args: Vec<String>, trace_out: Option<String>) -> Self {
        Self {
            child_args,
            trace_out,
            clock: None,
            probe_write: None,
            beats: None,
            child: None,
        }
    }

    fn now(&self) -> Instant {
        self.clock.as_ref().map_or(Instant::ZERO, Clock::now)
    }
}

impl WatchdogHost for Windows {
    /// The pipes and the child, which is one operation from outside.
    ///
    /// Setup rather than a sequence of decisions: nothing in here looks at a result
    /// and chooses what to do next, so there is nothing for the suite to drive
    /// (ADR-0006). What a failure means — that there is nothing to supervise — is
    /// `core`'s, and it is what `None` says.
    fn start(&mut self) -> Option<Instant> {
        let (Some((probe_read, probe_write)), Some((beat_read, beat_write))) =
            (make_pipe(), make_pipe())
        else {
            error!("could not make the pipes: {}", last_error());
            return None;
        };
        keep(probe_write);
        keep(beat_read);

        // The handle values as the child will see them: a handle is inherited with the
        // same value it has here, which is what makes telling the child a number
        // enough.
        let mut command = Command::new(&self.child_args[0]);
        command
            .args(&self.child_args[1..])
            .env(PROBE, (probe_read as usize).to_string())
            .env(HEARTBEAT, (beat_write as usize).to_string());

        let child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                error!("could not start {}: {error}", self.child_args[0]);
                return None;
            }
        };
        // The child holds its own copies now, and ours have to go: the beat pipe ends
        // when every write end is closed, and this process holding one would keep a
        // dead child looking quiet rather than gone.
        unsafe {
            CloseHandle(probe_read);
            CloseHandle(beat_write);
        }

        info!("supervising pid {}", child.id());
        debug!("pipes: probe -> {probe_read:?}, beats {beat_write:?} -> {beat_read:?}");
        self.child = Some(child);
        self.probe_write = Some(probe_write);
        // Wrapped in a `File` so the reading thread is ordinary std: the handle is this
        // process's own and closing it is what ends that thread.
        self.beats = Some(Beats::read(unsafe {
            std::fs::File::from_raw_handle(beat_read.cast())
        }));

        let clock = Clock::start();
        let started = clock.now();
        self.clock = Some(clock);
        Some(started)
    }

    fn ended(&mut self) -> Option<Exit> {
        let child = self.child.as_mut()?;
        match child.try_wait() {
            // A child something else ended names no status of its own. What that means
            // is `core`'s and the binary's, not this call's.
            Ok(Some(status)) => Some(status.code().map_or(Exit::Signalled, Exit::Code)),
            // A status that cannot be read is not the same as a child that has ended.
            Ok(None) | Err(_) => None,
        }
    }

    fn wait_for_a_heartbeat(&mut self, patience: Duration) -> Beat {
        let kind = match self.beats.as_mut() {
            Some(beats) => match beats.next(patience) {
                Arrival::Heartbeat => BeatKind::Heartbeat,
                Arrival::Silence => BeatKind::Silence,
            },
            None => BeatKind::Silence,
        };
        Beat {
            at: self.now(),
            kind,
        }
    }

    fn probe(&mut self) -> bool {
        let Some(handle) = self.probe_write else {
            return false;
        };
        let probe = *b"?";
        let mut written = 0u32;
        let sent = unsafe {
            WriteFile(
                handle,
                probe.as_ptr().cast(),
                1,
                &mut written,
                core::ptr::null_mut(),
            )
        };
        sent != 0 && written == 1
    }

    /// Nothing, because there is nothing to ask with.
    ///
    /// Windows has no signal a process can catch in order to put its own keyboards
    /// back, so the grace period the POSIX half spends is skipped here — which is
    /// `core`'s decision to make on this answer, not this machine's. Nothing is lost
    /// by it: what suppresses on this platform is a pair of hooks belonging to the
    /// process, and a process that is gone has no hook procedure left to call.
    fn ask_it_to_stop(&mut self) -> bool {
        false
    }

    fn pause(&mut self, how_long: Duration) {
        std::thread::sleep(how_long);
    }

    fn end_it(&mut self) {
        let Some(child) = self.child.as_mut() else {
            return;
        };
        if let Err(error) = child.kill() {
            error!("could not end the supervised process: {error}");
        }
        let _ = child.wait();
    }

    /// Nothing, because there is no trace on this platform.
    ///
    /// ADR-0009's region is the converting run's recording, and the run this supervises
    /// is the forwarding one: it converts nothing, so there is nothing to replay. Said
    /// rather than silent, because a person who passed `--trace-out` asked for a file
    /// and should not have to find out from its absence.
    fn keep_the_trace(&mut self) {
        if let Some(path) = self.trace_out.as_deref() {
            warn!(
                "nothing was written to {path}: the run this supervises records no trace. A trace \
                 is the converting machine's (ADR-0009), and this is the forwarding one"
            );
        }
    }

    fn warn(&mut self, message: core::fmt::Arguments) {
        warn!("{message}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The size the SDK's header produces on the 64-bit build.
    ///
    /// Checked because `CreatePipe` reads the structure this crate reserved: one that
    /// is too small is a read past the end of it, and one whose fields have moved asks
    /// for the opposite of what was meant — a pipe no child can inherit, whose symptom
    /// is a supervised process that never sees a probe.
    #[test]
    #[cfg(target_pointer_width = "64")]
    fn the_security_attributes_are_the_size_the_header_says() {
        assert_eq!(size_of::<SecurityAttributes>(), 24);
    }

    #[test]
    fn a_pipe_is_made_with_both_ends_usable() {
        // The whole of what `start` rests on, minus the child: without a pipe there is
        // no probe to send and no heartbeat to wait for, and the failure would look
        // exactly like a process that never answered.
        let (read, write) = make_pipe().expect("a pipe");
        assert!(!read.is_null());
        assert!(!write.is_null());

        let byte = *b"?";
        let mut written = 0u32;
        let sent = unsafe {
            WriteFile(
                write,
                byte.as_ptr().cast(),
                1,
                &mut written,
                core::ptr::null_mut(),
            )
        };
        assert_ne!(sent, 0, "writing to the pipe: {}", last_error());
        assert_eq!(written, 1);

        keep(read);
        unsafe {
            CloseHandle(read);
            CloseHandle(write);
        }
    }
}
