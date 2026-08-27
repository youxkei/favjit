//! The POSIX half: two pipes, a shared region, and a child that inherits them.
//!
//! A pipe the parent already holds needs no name to bind and no permission model to
//! go with it, and it closes by itself when either end dies — which is the property
//! that matters, since noticing death is the whole job (ADR-0008).
//!
//! Calls and nothing else. Which order they go in, how long a silence may last and
//! what to do about one are `favjit_engine::watchdog`'s.

use core::time::Duration;
use std::ffi::c_void;
use std::os::fd::FromRawFd;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::process::{Child, Command};

use std::io::Write;
use std::os::unix::net::{UnixListener, UnixStream};

use favjit_engine::supervision::{
    BEAT_WITHIN, HEARTBEAT, PROBE, TRACE, TRACES_HANDED_OVER, TRACE_BYTES,
};
use favjit_engine::watchdog::{Beat, BeatKind, Exit, WatchdogHost};
use favjit_engine::Instant;
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

    /// Variadic, as the header has it, for the reason `fcntl` above is: declared
    /// with a fixed third argument this links and the mode never arrives, so the
    /// region is made with whatever was on the stack. A region whose mode has no
    /// owner-write bit is one the next supervisor's open is refused, which reaches a
    /// person as a run with no recording rather than as an error at the call.
    fn shm_open(name: *const i8, oflag: i32, ...) -> i32;
    /// Only a test takes a name back out: a supervisor that unlinked either region
    /// would be one whose successor finds nothing, which is the whole of what
    /// keeping a recording rests on.
    #[cfg(test)]
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

/// Where the run being supervised writes its recording.
///
/// Named, and the name outlives the process that opened it: a run that ends takes
/// its supervisor with it, so a region unlinked on the way in would be gone by the
/// time anybody could ask about the run that had just failed — which is every run
/// that fails without wedging (ADR-0009).
pub const LIVE: &str = "/favjit-trace-live";

/// Where the run before it is kept.
///
/// Two regions rather than one written twice, because the recording has to be
/// somewhere the next run is not writing: a single region would hold the new run's
/// first second in place of the failed run's last minute.
pub const KEPT: &str = "/favjit-trace-kept";

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
    /// Open one by name, making it where it is not there yet.
    ///
    /// Root-only, and that is the whole of what guards it: a name that persists is
    /// a name another process can open, and mode is what says which. The supervisor
    /// already answers for the recording over a socket only root may connect to, so
    /// the reach is the one that was already granted rather than a new one.
    ///
    /// Sized only on the open that made it. `ftruncate` on one that is already
    /// there would be a second answer to how large it is, and the one that loses is
    /// the run mapping it.
    fn open(name: &str) -> std::io::Result<Self> {
        let name = format!("{name}\0");
        let mut made = true;
        let mut fd =
            unsafe { shm_open(name.as_ptr() as *const i8, O_RDWR | O_CREAT | O_EXCL, 0o600) };
        if fd < 0 {
            made = false;
            fd = unsafe { shm_open(name.as_ptr() as *const i8, O_RDWR, 0o600) };
        }
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        if made && unsafe { ftruncate(fd, TRACE_BYTES as i64) } < 0 {
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

    /// The pages themselves, to be written into.
    fn pages(&mut self) -> &mut [u8] {
        // Sound for the reason [`Region::snapshot`] is: this process holds the
        // mapping for its whole life.
        unsafe { std::slice::from_raw_parts_mut(self.at, TRACE_BYTES) }
    }

    /// Put these bytes in, as many as fit.
    fn take_in(&mut self, bytes: &[u8]) {
        let how_many = bytes.len().min(TRACE_BYTES);
        self.pages()[..how_many].copy_from_slice(&bytes[..how_many]);
    }

    /// Leave nothing a reader would take for a recording.
    ///
    /// The whole region and not the few bytes a reader goes by, because which bytes
    /// those are is the recording's own format — and a supervisor that knew the
    /// format would be a second place for it to be wrong, in the component that has
    /// to stay trivial (ADR-0009). Zeroing a megabyte is paid before the child is
    /// started, which is not the path a keyboard comes back on.
    fn forget(&mut self) {
        self.pages().fill(0);
    }
}

/// This machine, as the judgement's boundary.
pub struct Unix {
    child_args: Vec<String>,
    trace_out: Option<String>,
    /// Where the supervised process's own output goes, if anywhere.
    ///
    /// Nothing on this platform needs it — launchd keeps what a daemon writes — and it
    /// is here because the flag is one program's rather than one platform's.
    log: Option<String>,
    clock: Option<Clock>,
    /// Our end of the probe pipe, once there is a child at the other end of it.
    probe_write: Option<i32>,
    beats: Option<Beats>,
    child: Option<Child>,
    region: Option<Region>,
    /// The run before this one, moved aside when this one started.
    kept: Option<Region>,
    /// What the two regions above are named.
    ///
    /// Handed in for the reason the socket path is: a name that persists is one a
    /// test has to be able to point somewhere of its own, or the two tests that
    /// open a region are one test that depends on which ran first (ADR-0006).
    live_at: String,
    kept_at: String,
    /// Where somebody asks for the recording while the run is still going.
    ///
    /// A socket rather than a signal, because what the asking process wants back
    /// is the bytes: a signal can say "now" and nothing else, so the answer would
    /// have to be a file this process wrote — and this process is root, so a path
    /// it took from the asker would be a way to have root write anywhere
    /// (ADR-0009).
    asking: Option<UnixListener>,
    /// Whoever is waiting for it, between the question and the answer: the run
    /// decides what to do about the question, so accepting and answering are two
    /// operations (ADR-0006).
    asked: Option<UnixStream>,
    /// Where the listener above is bound.
    asked_at: String,
}

impl Unix {
    /// `asked_at` is where somebody asks for the recording.
    ///
    /// Handed in rather than read from the agreement itself, for the reason every
    /// other name this process needs is: what a run listens on is a fact about
    /// the run, and a test that cannot point it somewhere writable is a test that
    /// cannot drive the one operation here with two ends to it (ADR-0006).
    pub fn new(
        child_args: Vec<String>,
        trace_out: Option<String>,
        log: Option<String>,
        asked_at: String,
    ) -> Self {
        Self::keeping(child_args, trace_out, log, asked_at, LIVE, KEPT)
    }

    /// The same, with the recordings kept under names of the caller's choosing.
    pub fn keeping(
        child_args: Vec<String>,
        trace_out: Option<String>,
        log: Option<String>,
        asked_at: String,
        live_at: &str,
        kept_at: &str,
    ) -> Self {
        Self {
            asked_at,
            child_args,
            trace_out,
            log,
            live_at: live_at.to_owned(),
            kept_at: kept_at.to_owned(),
            clock: None,
            probe_write: None,
            beats: None,
            child: None,
            region: None,
            kept: None,
            asking: None,
            asked: None,
        }
    }

    fn now(&self) -> Instant {
        self.clock
            .as_ref()
            .map_or_else(Instant::default, Clock::now)
    }
}

impl WatchdogHost for Unix {
    /// The pipes, the region and the child, which is one operation from outside.
    ///
    /// Everything in here is setup rather than a sequence of decisions: nothing looks
    /// at a result and chooses what to do next, so there is nothing for the suite to
    /// drive (ADR-0006). What a failure means — that there is nothing to supervise —
    /// is `engine`'s, and it is what `None` says.
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
        self.region = match Region::open(&self.live_at) {
            Ok(region) => Some(region),
            Err(error) => {
                warn!("no trace this run: cannot make the shared region ({error})");
                None
            }
        };
        // The run before this one is moved aside here rather than handed over on the
        // way down, and that is what makes it survive at all: the ways a run ends
        // that are worth reading are the ways that also end the supervisor, and one
        // of them is a kill from outside, which runs no code here either. Keeping at
        // the next start needs nothing of the process that is gone (ADR-0009).
        self.kept = match Region::open(&self.kept_at) {
            Ok(kept) => Some(kept),
            Err(error) => {
                warn!("the run before this one cannot be kept ({error})");
                None
            }
        };
        if let (Some(live), Some(kept)) = (self.region.as_mut(), self.kept.as_mut()) {
            kept.take_in(&live.snapshot());
            // So that a run which records nothing reads as nothing. Left as it is,
            // the region would answer for this run with the one before it, and a
            // recording that names the wrong run is worse than an empty one.
            live.forget();
        }

        let mut command = Command::new(&self.child_args[0]);
        command
            .args(&self.child_args[1..])
            .env(PROBE, probe_read.to_string())
            .env(HEARTBEAT, beat_write.to_string());
        let trace_fd = self.region.as_ref().map(|region| region.fd);
        if let Some(fd) = trace_fd {
            command.env(TRACE, fd.to_string());
        }
        // Into the same file this program logs to, so that one file says what the
        // supervisor saw and what the supervised process was doing at the time.
        if let Some(path) = &self.log {
            match std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
            {
                Ok(file) => match file.try_clone() {
                    Ok(second) => {
                        command.stdout(file).stderr(second);
                    }
                    Err(error) => warn!("cannot hand {path} to the child twice: {error}"),
                },
                Err(error) => warn!("cannot write {path}: {error}"),
            }
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
            // is `engine`'s and the binary's, not this call's.
            Ok(Some(status)) => Some(status.code().map_or_else(
                || status.signal().map_or(Exit::Unknown, Exit::Signal),
                Exit::Code,
            )),
            // A status that cannot be read is not the same as a child that has ended.
            Ok(None) | Err(_) => None,
        }
    }

    fn wait_for_a_heartbeat(&mut self, patience: Duration) -> Beat {
        let kind = match self.beats.as_mut() {
            Some(beats) => match beats.next(patience) {
                Arrival::Heartbeat => BeatKind::Beat,
                Arrival::Silence => BeatKind::Silent,
            },
            None => BeatKind::Silent,
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
    /// Whether somebody has connected to ask for the recording.
    ///
    /// The socket is opened here rather than beside the pipes, because a run that
    /// nobody ever asks is the ordinary one: a listener made at startup would be a
    /// name in the filesystem for every run, and this one appears the first time
    /// the question is put — which is also the first moment there is anything worth
    /// answering with.
    ///
    /// Non-blocking, because this is asked from the loop that has a keyboard's
    /// return to see to (ADR-0008): a wait here for somebody who may never come
    /// would be the supervisor stopping for the one thing it must not stop for.
    fn asked_for_the_trace(&mut self) -> bool {
        if self.asking.is_none() {
            // Whatever a previous run left, since a socket file outlives the
            // process that bound it and binding onto one that is there fails.
            let _ = std::fs::remove_file(&self.asked_at);
            self.asking = match UnixListener::bind(&self.asked_at) {
                Ok(listener) => {
                    let _ = listener.set_nonblocking(true);
                    Some(listener)
                }
                Err(error) => {
                    warn!("nobody can ask for the trace: {} ({error})", self.asked_at);
                    return false;
                }
            };
        }
        let Some(listener) = self.asking.as_ref() else {
            return false;
        };
        match listener.accept() {
            Ok((stream, _)) => {
                // Blocking with a bound, and not the non-blocking the listener
                // has: a recording is far larger than a socket's buffer, so a
                // non-blocking write hands over the first few kilobytes of it and
                // reports the rest as would-block. The bound is what keeps a
                // reader that stops reading from stopping the supervisor, which is
                // the one thing it must not do (ADR-0008).
                let _ = stream.set_nonblocking(false);
                let _ = stream.set_write_timeout(Some(BEAT_WITHIN));
                self.asked = Some(stream);
                true
            }
            Err(_) => false,
        }
    }

    /// Write the recording to whoever asked, and go on supervising.
    ///
    /// The bytes and not a path: this process is root, and a path it took from the
    /// asker would be a way to have root write anywhere. What the asker does with
    /// them is its own business, at its own privilege (ADR-0009).
    fn hand_the_trace_over(&mut self) {
        let Some(mut asked) = self.asked.take() else {
            return;
        };
        let Some(region) = self.region.as_ref() else {
            // Nothing to hand over, and the connection closes saying so: a reader
            // left waiting on a socket that will never answer cannot tell that
            // apart from a supervisor that has wedged.
            return;
        };
        // This run's and then the run before it, one after the other, because the
        // agreement is a count and an order rather than a question the asker puts.
        // A supervisor with nothing kept writes the first alone, which is the answer
        // a reader gets from one that has only just come up.
        let mut bytes = Vec::with_capacity(TRACE_BYTES * TRACES_HANDED_OVER);
        for each in [Some(region), self.kept.as_ref()]
            .into_iter()
            .flatten()
            .take(TRACES_HANDED_OVER)
        {
            bytes.extend_from_slice(&each.snapshot());
        }
        match asked.write_all(&bytes) {
            Ok(()) => warn!(
                "handed the trace to whoever asked. It contains the keystrokes of the window it \
                 covers — everything typed on the captured keyboards, passwords included"
            ),
            Err(error) => warn!("could not hand the trace over: {error}"),
        }
    }

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    /// A socket path nothing else is using, under the directory a test may write.
    fn somewhere() -> String {
        format!(
            "{}/favjit-asked-{}.sock",
            std::env::temp_dir().to_string_lossy(),
            unique()
        )
    }

    /// A number no other test in this run is using.
    fn unique() -> u128 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("a clock past the epoch")
            .as_nanos()
    }

    /// A region name nothing else is using.
    ///
    /// Short, because macOS caps a shared memory name at 31 bytes and a name over
    /// it is an open that fails rather than a name that is truncated.
    fn a_name(what: &str) -> String {
        format!("/favjit-{what}-{}", unique() % 100_000_000)
    }

    /// Take the name out of the system, so a test leaves nothing another run of it
    /// would open instead of making its own.
    fn forget_the_name(name: &str) {
        let name = format!("{name}\0");
        unsafe { shm_unlink(name.as_ptr() as *const i8) };
    }

    #[test]
    fn a_recording_is_there_for_a_second_open_of_the_same_name() {
        // The property the whole of keeping rests on: a run that fails takes its
        // supervisor with it, so the recording has to still be there for the next
        // supervisor to move aside. A region unlinked while open maps fine for the
        // process that made it and is gone here.
        //
        // The descriptor is let go of first, because the supervisor that comes next
        // inherits none: a region reachable only through the one that made it is one
        // nothing after it can reach.
        let name = a_name("out");

        let mut first = Region::open(&name).expect("a region");
        first.take_in(&[0x42]);
        unsafe { close(first.fd) };

        let second = Region::open(&name).expect("the same region");
        assert_eq!(
            second.snapshot()[0],
            0x42,
            "the recording is not what the second open found"
        );

        forget_the_name(&name);
    }

    #[test]
    fn a_region_made_by_one_supervisor_is_the_size_the_next_one_maps() {
        // The size is set on the open that made it, so the second open has to find
        // it already that long — one that sized it again would be a second answer
        // to how large it is.
        let name = a_name("size");

        let first = Region::open(&name).expect("a region");
        assert_eq!(first.snapshot().len(), TRACE_BYTES);
        unsafe { close(first.fd) };

        let second = Region::open(&name).expect("the same region");
        assert_eq!(second.snapshot().len(), TRACE_BYTES);

        forget_the_name(&name);
    }

    #[test]
    fn starting_a_run_moves_the_run_before_it_aside_and_leaves_its_own_region_empty() {
        // What makes a failed run readable at all: nothing is asked of the process
        // that is gone, so this holds however it went — including a kill from
        // outside, which runs no code in it (ADR-0009).
        let live = a_name("live");
        let kept = a_name("kept");
        let child = vec!["/usr/bin/true".to_owned()];

        let mut before = Unix::keeping(child.clone(), None, None, somewhere(), &live, &kept);
        before.start().expect("a supervisor with a child");
        before
            .region
            .as_mut()
            .expect("the region it made")
            .take_in(&[0x42]);
        drop(before);

        let mut after = Unix::keeping(child, None, None, somewhere(), &live, &kept);
        after.start().expect("a second supervisor");

        assert_eq!(
            after.kept.as_ref().expect("the kept region").snapshot()[0],
            0x42,
            "the run before this one was not kept"
        );
        assert_eq!(
            after.region.as_ref().expect("the live region").snapshot()[0],
            0,
            "this run's region still holds the run before it"
        );

        forget_the_name(&live);
        forget_the_name(&kept);
    }

    /// Both ends of the one operation here that has two, driven over a real
    /// socket.
    ///
    /// The IO itself is what this exercises, which is the one kind of test that
    /// belongs beside a platform's calls (ADR-0006): what a supervisor does about
    /// the question is `favjit_engine::watchdog`'s and the suite drives that.
    #[test]
    fn a_recording_asked_for_over_the_socket_comes_back_over_it() {
        let at = somewhere();
        let live = a_name("askl");
        let kept = a_name("askk");
        let mut machine = Unix::keeping(Vec::new(), None, None, at.clone(), &live, &kept);
        let mut region = Region::open(&live).expect("a region");
        // Written where a reader would look for the first record, so what comes
        // back is checked to be the region and not an empty buffer of the right
        // length. The bytes and not a record: nothing here interprets one.
        region.take_in(&[0x42]);
        machine.region = Some(region);
        let mut before = Region::open(&kept).expect("a second region");
        before.take_in(&[0x43]);
        machine.kept = Some(before);

        // Nobody has asked yet, which is what binds the socket.
        assert!(!machine.asked_for_the_trace());

        let asking = std::thread::spawn({
            let at = at.clone();
            move || {
                let mut asked = UnixStream::connect(&at).expect("the supervisor is listening");
                let mut bytes = Vec::new();
                asked.read_to_end(&mut bytes).expect("the recording");
                bytes
            }
        });

        // Asked over and over, because a connection takes a moment to arrive and
        // the answer is what the run gives on the way round after it does.
        let mut asked = false;
        for _ in 0..1000 {
            if machine.asked_for_the_trace() {
                asked = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(asked, "the connection never arrived");
        machine.hand_the_trace_over();

        let bytes = asking.join().expect("the asking thread");
        assert_eq!(
            bytes.len(),
            TRACE_BYTES * TRACES_HANDED_OVER,
            "an answer carries this run's recording and the run before it"
        );
        assert_eq!(bytes[0], 0x42, "what came back is not the region");
        assert_eq!(
            bytes[TRACE_BYTES], 0x43,
            "the run before this one is not behind it"
        );

        let _ = std::fs::remove_file(&at);
        forget_the_name(&live);
        forget_the_name(&kept);
    }
}
