//! The POSIX half: two pipes, a shared region, and a child that inherits them.
//!
//! A pipe the parent already holds needs no name to bind and no permission model to
//! go with it, and it closes by itself when either end dies — which is the property
//! that matters, since noticing death is the whole job (ADR-0008).
//!
//! Calls and nothing else. Which order they go in, how long a silence may last and
//! what to do about one are `favjit_core::watchdog`'s.

use core::time::Duration;
use std::ffi::c_void;
use std::os::fd::FromRawFd;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command};

use favjit_core::supervision::{HEARTBEAT, PROBE, TRACE, TRACE_BYTES};
use favjit_core::watchdog::{Beat, BeatKind, Exit, WatchdogHost};
use favjit_core::Instant;
use log::{debug, error, info, warn};

use crate::beats::{Arrival, Beats, Clock};

extern "C" {
    fn pipe(fds: *mut i32) -> i32;
    fn write(fd: i32, buf: *const c_void, count: usize) -> isize;
    fn close(fd: i32) -> i32;
    /// Variadic, as the header has it. Declared with a fixed third argument instead,
    /// this still compiles and still links — and on arm64 the argument goes in a
    /// register while `fcntl` reads it off the stack, so the flag it applies is
    /// whatever was there. A `F_SETFD` that quietly sets the opposite bit is how the
    /// child loses the descriptors it was handed.
    fn fcntl(fd: i32, cmd: i32, ...) -> i32;
    fn kill(pid: i32, sig: i32) -> i32;

    fn shm_open(name: *const i8, oflag: i32, mode: u32) -> i32;
    fn shm_unlink(name: *const i8) -> i32;
    fn ftruncate(fd: i32, length: i64) -> i32;
    fn mmap(
        addr: *mut c_void,
        len: usize,
        prot: i32,
        flags: i32,
        fd: i32,
        offset: i64,
    ) -> *mut c_void;
    fn getpid() -> i32;
}

const F_SETFD: i32 = 2;
const FD_CLOEXEC: i32 = 1;
const SIGKILL: i32 = 9;
const SIGTERM: i32 = 15;

const PROT_READ: i32 = 0x01;
const PROT_WRITE: i32 = 0x02;
const MAP_SHARED: i32 = 0x0001;
const O_RDWR: i32 = 0x0002;
const O_CREAT: i32 = 0x0200;
const O_EXCL: i32 = 0x0800;

/// What the last failing call set errno to, spelled out.
fn last_error() -> String {
    std::io::Error::last_os_error().to_string()
}

fn make_pipe() -> Option<(i32, i32)> {
    let mut fds = [0i32; 2];
    if unsafe { pipe(fds.as_mut_ptr()) } != 0 {
        return None;
    }
    Some((fds[0], fds[1]))
}

/// The shared memory a trace is written into, held by this process because it is the
/// one that survives the kill (ADR-0009).
///
/// **Made and copied, never read.** Nothing here knows what a record is: the component
/// that ends a wedged converter should not also contain a parser for the converter's
/// internals, and a trace it cannot interpret is one it cannot leak by accident
/// either.
struct Region {
    at: *mut u8,
    fd: i32,
}

impl Region {
    fn create() -> std::io::Result<Self> {
        // The pid makes the name unique only for the moment before it is unlinked;
        // macOS caps a shared memory name at 31 bytes, which leaves no room for
        // anything more descriptive.
        let name = format!("/favjit-wd-{}\0", unsafe { getpid() });
        let fd = unsafe { shm_open(name.as_ptr() as *const i8, O_RDWR | O_CREAT | O_EXCL, 0o600) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        // Unlinked while still open: the mapping survives and nothing else can open
        // it by name. A trace holds keystrokes, so the fewer ways in the better.
        unsafe { shm_unlink(name.as_ptr() as *const i8) };
        if unsafe { ftruncate(fd, TRACE_BYTES as i64) } < 0 {
            return Err(std::io::Error::last_os_error());
        }
        let at = unsafe {
            mmap(
                std::ptr::null_mut(),
                TRACE_BYTES,
                PROT_READ | PROT_WRITE,
                MAP_SHARED,
                fd,
                0,
            )
        };
        if at as usize == usize::MAX {
            return Err(std::io::Error::last_os_error());
        }
        Ok(Self {
            at: at as *mut u8,
            fd,
        })
    }

    /// A copy of the bytes, taken without looking at them.
    fn snapshot(&self) -> Vec<u8> {
        let mut out = vec![0u8; TRACE_BYTES];
        // Sound because this process holds the mapping for its whole life. The child
        // writing the same pages is the point of the region, so a copy can be
        // inconsistent at its edges — which is why whatever reads it later has to
        // tolerate a record half written.
        out.copy_from_slice(unsafe { std::slice::from_raw_parts(self.at, TRACE_BYTES) });
        out
    }
}

/// This machine, as the judgement's boundary.
pub struct Unix {
    child_args: Vec<String>,
    trace_out: Option<String>,
    clock: Option<Clock>,
    /// Our end of the probe pipe, once there is a child at the other end of it.
    probe_write: Option<i32>,
    beats: Option<Beats>,
    child: Option<Child>,
    region: Option<Region>,
}

impl Unix {
    pub fn new(child_args: Vec<String>, trace_out: Option<String>) -> Self {
        Self {
            child_args,
            trace_out,
            clock: None,
            probe_write: None,
            beats: None,
            child: None,
            region: None,
        }
    }

    fn now(&self) -> Instant {
        self.clock.as_ref().map_or(Instant::ZERO, Clock::now)
    }
}

impl WatchdogHost for Unix {
    /// The pipes, the region and the child, which is one operation from outside.
    ///
    /// Everything in here is setup rather than a sequence of decisions: nothing looks
    /// at a result and chooses what to do next, so there is nothing for the suite to
    /// drive (ADR-0006). What a failure means — that there is nothing to supervise —
    /// is `core`'s, and it is what `None` says.
    fn start(&mut self) -> Option<Instant> {
        let (Some((probe_read, probe_write)), Some((beat_read, beat_write))) =
            (make_pipe(), make_pipe())
        else {
            error!("could not make the pipes: {}", last_error());
            return None;
        };

        // Our own ends stay out of the child, so the pipes report the child's death
        // rather than staying open on descriptors it inherited.
        unsafe {
            fcntl(beat_read, F_SETFD, FD_CLOEXEC);
            fcntl(probe_write, F_SETFD, FD_CLOEXEC);
        }

        // The trace's memory is this process's, because the occasions that most need
        // one are the ones the child cannot answer for: a wedge cannot be asked for
        // its memory, and the kill below runs no code in it at all. Made here, it is
        // still here afterwards (ADR-0009).
        self.region = match Region::create() {
            Ok(region) => Some(region),
            Err(error) => {
                warn!("no trace this run: cannot make the shared region ({error})");
                None
            }
        };

        let mut command = Command::new(&self.child_args[0]);
        command
            .args(&self.child_args[1..])
            .env(PROBE, probe_read.to_string())
            .env(HEARTBEAT, beat_write.to_string());
        let trace_fd = self.region.as_ref().map(|region| region.fd);
        if let Some(fd) = trace_fd {
            command.env(TRACE, fd.to_string());
        }
        // The child's ends are the only ones it should hold, and Rust closes nothing
        // it did not open, so the inherited descriptors are cleared of CLOEXEC here
        // rather than left to chance.
        unsafe {
            command.pre_exec(move || {
                fcntl(probe_read, F_SETFD, 0);
                fcntl(beat_write, F_SETFD, 0);
                // The region's descriptor is cleared explicitly for the same reason
                // as the pipes: whether `shm_open` left CLOEXEC set is not something
                // to depend on, and a child that lost it records into nothing.
                if let Some(fd) = trace_fd {
                    fcntl(fd, F_SETFD, 0);
                }
                Ok(())
            });
        }

        let child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                error!("could not start {}: {error}", self.child_args[0]);
                return None;
            }
        };
        unsafe {
            close(probe_read);
            close(beat_write);
        }

        let pid = child.id();
        info!("supervising pid {pid}");
        debug!("pipes: probe -> {probe_read}, beats {beat_write} -> {beat_read}");
        self.child = Some(child);
        self.probe_write = Some(probe_write);
        // Wrapped in a `File` so the reading thread is ordinary std: the descriptor
        // is this process's own and closing it is what ends that thread.
        self.beats = Some(Beats::read(unsafe {
            std::fs::File::from_raw_fd(beat_read)
        }));

        let clock = Clock::start();
        let started = clock.now();
        self.clock = Some(clock);
        Some(started)
    }

    fn ended(&mut self) -> Option<Exit> {
        let child = self.child.as_mut()?;
        match child.try_wait() {
            // A child that a signal ended names no status of its own. What that means
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
        let Some(fd) = self.probe_write else {
            return false;
        };
        let probe = *b"?";
        let sent = unsafe { write(fd, probe.as_ptr() as *const c_void, 1) };
        sent > 0
    }

    /// `SIGTERM`, which a process can act on.
    ///
    /// There is always a way to ask here, so this never answers `false`.
    fn ask_it_to_stop(&mut self) -> bool {
        let Some(child) = self.child.as_mut() else {
            return false;
        };
        unsafe { kill(child.id() as i32, SIGTERM) };
        true
    }

    fn pause(&mut self, how_long: Duration) {
        std::thread::sleep(how_long);
    }

    /// `SIGKILL`, which it cannot.
    fn end_it(&mut self) {
        let Some(child) = self.child.as_mut() else {
            return;
        };
        unsafe { kill(child.id() as i32, SIGKILL) };
        // Reaped, so the process is gone rather than a zombie by the time this
        // returns: what happens next is the trace being taken, and it is taken because
        // the process is not writing to the region any more.
        let _ = child.wait();
    }

    /// Say a trace was kept, and write it out only where asked to.
    ///
    /// A trace holds whatever was typed in the window it covers, passwords included —
    /// that is inherent, since replaying a conversion bug needs the actual keys. So
    /// nothing is written without the flag, and what it contains is said here rather
    /// than left to be discovered.
    fn keep_the_trace(&mut self) {
        let Some(region) = self.region.as_ref() else {
            return;
        };
        let bytes = region.snapshot();
        let Some(path) = self.trace_out.as_deref() else {
            info!(
                "a {} KiB trace of the run was kept in memory. It is gone when this process \
                 exits; pass --trace-out PATH to write it out. It contains the keystrokes of the \
                 window it covers",
                bytes.len() / 1024
            );
            return;
        };
        match std::fs::write(path, &bytes) {
            Ok(()) => warn!(
                "wrote the trace to {path}. It contains the keystrokes of the window it covers — \
                 everything typed on the captured keyboards, passwords included"
            ),
            Err(error) => error!("could not write the trace to {path}: {error}"),
        }
    }

    fn warn(&mut self, message: core::fmt::Arguments) {
        warn!("{message}");
    }
}
