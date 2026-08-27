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
//! a process is asked to stop before it is stopped are `favjit_engine::watchdog`'s.

use core::mem::size_of;
use core::time::Duration;
use std::ffi::c_void;
use std::os::windows::io::{AsRawHandle, FromRawHandle};
use std::os::windows::process::CommandExt;
use std::process::{Child, Command};

use favjit_engine::supervision::{HEARTBEAT, PROBE, TRACE, TRACE_BYTES};
use favjit_engine::watchdog::{Beat, BeatKind, Exit, WatchdogHost};
use favjit_engine::Instant;
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

/// `CREATE_NO_WINDOW`, which is what stops a console being made for the child.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// `JOBOBJECT_BASIC_LIMIT_INFORMATION`, in the header's order.
///
/// Only `limit_flags` is set. The rest is limits this asks for none of, and each
/// one is named rather than left as padding because a structure the system reads
/// has to say where every byte goes.
#[repr(C)]
#[derive(Default)]
struct JobBasicLimits {
    per_process_user_time: i64,
    per_job_user_time: i64,
    limit_flags: u32,
    minimum_working_set: usize,
    maximum_working_set: usize,
    active_process_limit: u32,
    affinity: usize,
    priority_class: u32,
    scheduling_class: u32,
}

/// `IO_COUNTERS`, which the extended limits carry and nothing here reads.
#[repr(C)]
#[derive(Default)]
struct IoCounters {
    reads: u64,
    writes: u64,
    others: u64,
    read_bytes: u64,
    write_bytes: u64,
    other_bytes: u64,
}

/// `JOBOBJECT_EXTENDED_LIMIT_INFORMATION`, which is the shape
/// `JobObjectExtendedLimitInformation` is set with.
#[repr(C)]
#[derive(Default)]
struct JobExtendedLimits {
    basic: JobBasicLimits,
    io: IoCounters,
    process_memory_limit: usize,
    job_memory_limit: usize,
    peak_process_memory: usize,
    peak_job_memory: usize,
}

/// `JobObjectExtendedLimitInformation`, the class of information being set.
const JOB_EXTENDED_LIMIT_INFORMATION: u32 = 9;

/// `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`.
///
/// **The whole reason a job is used at all.** A handle closes when the process
/// holding it goes, however it goes — including a `taskkill` on this supervisor —
/// and this bit is what makes the system then end everything in the job. Without
/// it, ending the supervisor leaves the process it was supervising running with
/// this machine's input still refused and nothing left to end it, which is the
/// outcome ADR-0008 exists to rule out.
const JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: u32 = 0x0000_2000;

/// `PAGE_READWRITE`, and `INVALID_HANDLE_VALUE` as the file to map.
///
/// **Not file-backed.** A mapping over a real file is the obvious way to make a
/// trace survive anything, and it would make a permanent on-disk keylog the
/// normal state of the machine — which is the same reason the other platform's
/// region is not (ADR-0009).
const PAGE_READWRITE: u32 = 0x04;
const INVALID_HANDLE_VALUE: Handle = usize::MAX as Handle;

/// `FILE_MAP_ALL_ACCESS`, which is what a region written from both ends needs.
const FILE_MAP_ALL_ACCESS: u32 = 0x000F_001F;

/// The shared region the run writes its trace into, and this process copies out
/// of.
///
/// **Made and copied, never read**, for the reason the other platform's is: the
/// component that ends a wedged converter should not also contain a parser for
/// the converter's internals, and a trace it cannot interpret is one it cannot
/// leak by accident either.
///
/// Anonymous and inheritable rather than named: a name is something else could
/// open, and a trace holds keystrokes. The child is given the handle's value in
/// an environment variable, which is the same way it is given the pipes and rests
/// on the same finding — a handle marked inheritable arrives in the child with the
/// value it had here
/// ([docs/platform/windows/inherited-pipes.md](../../../docs/platform/windows/inherited-pipes.md)).
struct Region {
    at: *mut u8,
    mapping: Handle,
}

impl Region {
    fn create() -> std::io::Result<Self> {
        let attributes = SecurityAttributes {
            length: size_of::<SecurityAttributes>() as u32,
            descriptor: std::ptr::null_mut(),
            inherit: 1,
        };
        // The length in two halves because the call takes it that way, and the
        // high half is zero for every size this ever asks for.
        let mapping = unsafe {
            CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                &attributes,
                PAGE_READWRITE,
                0,
                TRACE_BYTES as u32,
                std::ptr::null(),
            )
        };
        if mapping.is_null() {
            return Err(std::io::Error::last_os_error());
        }
        let at = unsafe { MapViewOfFile(mapping, FILE_MAP_ALL_ACCESS, 0, 0, TRACE_BYTES) };
        if at.is_null() {
            unsafe { CloseHandle(mapping) };
            return Err(std::io::Error::last_os_error());
        }
        Ok(Self {
            at: at as *mut u8,
            mapping,
        })
    }

    /// The handle's value, for the child to be told.
    fn handle(&self) -> usize {
        self.mapping as usize
    }

    /// A copy of the bytes, taken without looking at them.
    fn snapshot(&self) -> Vec<u8> {
        let mut out = vec![0u8; TRACE_BYTES];
        // Sound because this process holds the mapping for its whole life. The
        // child writing the same pages is the point of the region, so a copy can
        // be inconsistent at its edges — which is why whatever reads it later has
        // to tolerate a record half written.
        out.copy_from_slice(unsafe { std::slice::from_raw_parts(self.at, TRACE_BYTES) });
        out
    }
}

extern "system" {
    fn CreatePipe(
        read: *mut Handle,
        write: *mut Handle,
        attributes: *const SecurityAttributes,
        size: u32,
    ) -> i32;
    fn SetHandleInformation(object: Handle, mask: u32, flags: u32) -> i32;
    fn CreateJobObjectW(attributes: *const SecurityAttributes, name: *const u16) -> Handle;
    fn SetInformationJobObject(
        job: Handle,
        class: u32,
        information: *const c_void,
        length: u32,
    ) -> i32;
    fn AssignProcessToJobObject(job: Handle, process: Handle) -> i32;
    fn CreateFileMappingW(
        file: Handle,
        attributes: *const SecurityAttributes,
        protect: u32,
        maximum_high: u32,
        maximum_low: u32,
        name: *const u16,
    ) -> Handle;
    fn MapViewOfFile(
        mapping: Handle,
        access: u32,
        offset_high: u32,
        offset_low: u32,
        to_map: usize,
    ) -> *mut c_void;
    fn WriteFile(
        file: Handle,
        buffer: *const c_void,
        to_write: u32,
        written: *mut u32,
        overlapped: *mut c_void,
    ) -> i32;
    fn CloseHandle(object: Handle) -> i32;
}

/// A job the processes put in it are ended with, when the last handle to it closes.
///
/// Made before the child and held for this process's whole life, so that the handle
/// closing *is* this process ending — which is what covers the one case killing the
/// child from here cannot: this supervisor being killed itself.
///
/// `None` when the job could not be made or limited. Carried on with rather than
/// stopped for: a supervisor that refused to start is a keyboard nobody can use at
/// all, where one without a job is the arrangement that was there before.
fn make_job() -> Option<Handle> {
    let job = unsafe { CreateJobObjectW(core::ptr::null(), core::ptr::null()) };
    if job.is_null() {
        warn!("no job object: {}", last_error());
        return None;
    }
    let limits = JobExtendedLimits {
        basic: JobBasicLimits {
            limit_flags: JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            ..JobBasicLimits::default()
        },
        ..JobExtendedLimits::default()
    };
    let set = unsafe {
        SetInformationJobObject(
            job,
            JOB_EXTENDED_LIMIT_INFORMATION,
            (&limits as *const JobExtendedLimits).cast(),
            size_of::<JobExtendedLimits>() as u32,
        )
    };
    if set == 0 {
        warn!(
            "the job object would not take its one limit: {}",
            last_error()
        );
        unsafe { CloseHandle(job) };
        return None;
    }
    Some(job)
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
    /// Where a copy of the region is written, if the flag asked for one.
    trace_out: Option<String>,
    /// The region the child writes its trace into, held for this process's whole
    /// life: the occasions a trace is most wanted are the ones the child cannot
    /// hand anything over on, so the memory belongs to the process that survives
    /// them (ADR-0009).
    region: Option<Region>,
    /// Where the supervised process's own output goes, if anywhere.
    ///
    /// The same file this program logs to. A process started by a logon task has no
    /// console behind its stderr, so without this favjit's log — the whole of what says
    /// why a run did what it did — is discarded by the system.
    log: Option<String>,
    clock: Option<Clock>,
    /// Our end of the probe pipe, once there is a child at the other end of it.
    probe_write: Option<Handle>,
    beats: Option<Beats>,
    child: Option<Child>,
    /// The job the child is put in, held so that this process ending closes it.
    ///
    /// Never closed here on purpose: closing it is what ends the child, and the one
    /// moment that has to work is the one where no code of ours runs at all.
    job: Option<Handle>,
}

impl Windows {
    pub fn new(child_args: Vec<String>, trace_out: Option<String>, log: Option<String>) -> Self {
        Self {
            child_args,
            trace_out,
            region: None,
            log,
            clock: None,
            probe_write: None,
            beats: None,
            child: None,
            job: None,
        }
    }

    fn now(&self) -> Instant {
        self.clock
            .as_ref()
            .map_or_else(Instant::default, Clock::now)
    }
}

impl WatchdogHost for Windows {
    /// The pipes and the child, which is one operation from outside.
    ///
    /// Setup rather than a sequence of decisions: nothing in here looks at a result
    /// and chooses what to do next, so there is nothing for the suite to drive
    /// (ADR-0006). What a failure means — that there is nothing to supervise — is
    /// `engine`'s, and it is what `None` says.
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
        // Before the child, because the child is what writes into it: a run given
        // no region records nothing, which is the one thing worse than a trace
        // with a gap in it, and this process is the one that is still here
        // afterwards (ADR-0009).
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
            // Without a console of its own, for the reason this program has none: the
            // process supervised here is a console program, and a console program with
            // no console to inherit is given a fresh one — which with Windows Terminal
            // as the default is a terminal window on the desktop.
            .creation_flags(CREATE_NO_WINDOW)
            .env(PROBE, (probe_read as usize).to_string())
            .env(HEARTBEAT, (beat_write as usize).to_string());
        // Only where there is one, so a child given the name is one that can map
        // it: an empty value would be a handle of zero, which is a mapping the
        // run would take for real and write nowhere.
        if let Some(region) = self.region.as_ref() {
            command.env(TRACE, region.handle().to_string());
        }

        // Into the same file this program logs to, so that one file says what the
        // supervisor saw and what the supervised process was doing at the time. Two files
        // would have to be read side by side to answer the only question ever asked of
        // them, which is why the keys stopped arriving.
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

        let child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                error!("could not start {}: {error}", self.child_args[0]);
                return None;
            }
        };
        // Put in the job before anything else is done with it, so the window in which
        // it could outlive this process is as short as spawning it.
        self.job = make_job();
        if let Some(job) = self.job {
            if unsafe { AssignProcessToJobObject(job, child.as_raw_handle()) } == 0 {
                warn!(
                    "the supervised process is not in the job, so ending this one would \
                     leave it running: {}",
                    last_error()
                );
            }
        }

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
            // Windows has no signal for a status that cannot be read to name: what
            // ended the child other than itself is unknown here, not this call's to
            // decide.
            Ok(Some(status)) => Some(status.code().map_or(Exit::Unknown, Exit::Code)),
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
    /// `engine`'s decision to make on this answer, not this machine's. Nothing is lost
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

    /// Nobody asks this machine for its recording, because this machine is not
    /// where one is read.
    ///
    /// The records the run here makes cross the link and land in the converting
    /// machine's recording, including the ones made while there was no link to
    /// send them over (ADR-0009). So there is nothing here a reader would want
    /// that is not already over there, and a second place to ask would be a second
    /// half-answer to the question a reading exists for.
    ///
    /// The region above is still this process's to hold: it is what the run keeps
    /// its records in until a session can carry them.
    fn asked_for_the_trace(&mut self) -> bool {
        false
    }

    /// Nothing, for the reason above.
    fn hand_the_trace_over(&mut self) {}

    /// Say a trace was kept, and write it out only where asked to.
    ///
    /// A trace holds whatever was typed in the window it covers, passwords included —
    /// that is inherent, since reading a run needs the actual keys. So nothing is
    /// written without the flag, and what it contains is said here rather than left
    /// to be discovered.
    ///
    /// What this machine records is the forwarding run: which events it resolved,
    /// and what each record it sent answered with. Read beside the converting
    /// machine's own, the two line up on the numbers the records crossed under
    /// (ADR-0009).
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

    /// The sizes the SDK's headers produce on the 64-bit build.
    ///
    /// Checked because the length is handed to `SetInformationJobObject` alongside the
    /// structure, and it is the length that says which class of information this is: a
    /// wrong one is refused, and what a refused limit leaves is a job that ends nothing
    /// — a supervised process outliving its supervisor, silently.
    #[test]
    #[cfg(target_pointer_width = "64")]
    fn the_job_limits_are_the_size_the_headers_say() {
        assert_eq!(size_of::<JobBasicLimits>(), 64);
        assert_eq!(size_of::<IoCounters>(), 48);
        assert_eq!(size_of::<JobExtendedLimits>(), 144);
    }

    #[test]
    fn a_job_is_made_and_takes_its_one_limit() {
        // What the child being ended with this process rests on, minus the child: a job
        // that could not be made or could not be limited is reported and carried on
        // from, so the failure would otherwise be invisible until a supervisor was
        // killed and its child was found still holding the keyboards.
        let job = make_job().expect("a job");
        assert!(!job.is_null());
        unsafe { CloseHandle(job) };
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
