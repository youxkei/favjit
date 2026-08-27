//! What a watchdog and the run it supervises agree on (ADR-0008).
//!
//! Two programs rather than two machines, and the failure is the same shape as the
//! link's: a name spelled differently at one end is not an error but a probe nobody
//! reads and a heartbeat nobody sends, which reads as a wedge and ends a working
//! process. So it is stated here, once, for the reason [`crate::link`]'s constants
//! are ([ADR-0006](../../../docs/adr/0006-host-boundary.md)).
//!
//! Not behind the `watchdog` feature: both ends of the agreement need it, and only
//! one of them is a watchdog.

/// The environment a watchdog hands its child, and the child reads back.
///
/// A number in each, and what the number is belongs to the platform: a file
/// descriptor on Unix and a handle on Windows. Either way it is what a child needs in
/// order to reach a pipe its parent already holds, which is why one name covers both.
///
/// The environment and not an argument, because a child cannot be told which numbers
/// to expect any other way — and because favjit's own arguments are a person's to
/// write.
pub const PROBE: &str = "FAVJIT_PROBE_FD";
pub const HEARTBEAT: &str = "FAVJIT_HEARTBEAT_FD";

/// The longest a run may go without saying its loop came round.
///
/// **The run's promise and the supervisor's allowance are one number seen from two
/// sides**, so it is stated once here for the reason the names above are. A run whose
/// longest wait outlasts it is ended while it is working; a supervisor that allows less
/// than it ends every run that is looking for the other machine.
///
/// Which it did: a run under a supervisor that allowed two seconds was killed while
/// waiting the two seconds its own lookup was given, and the keyboard it had refused
/// nothing on came back to a machine with nothing forwarding. What that cost was not the
/// keyboard — a run looking for the Mac refuses nothing — but every attempt to reach it.
///
/// So every wait a run takes in one piece is at most this, and a wait longer than this is
/// taken in pieces with a beat between them.
///
/// Two seconds, which is how long the run's longest single wait wants to be rather than a
/// number picked for its own sake: what waits that long is the question it asks the
/// network for the other machine, and an answer arriving after the question stopped being
/// listened for costs a whole round of asking again. Set to one second it did: pressing
/// the chord switched a few seconds later, or not until the second question.
pub const BEAT_WITHIN: core::time::Duration = core::time::Duration::from_secs(2);

/// Where the trace is written, for the run to map (ADR-0009).
pub const TRACE: &str = "FAVJIT_TRACE_FD";

/// How large that region is.
///
/// Agreed rather than negotiated: the watchdog makes it and the run maps it, and a
/// run that mapped a different length would read records off the end of what was
/// made.
pub const TRACE_BYTES: usize = 1024 * 1024;

/// How many recordings an answer to [`TRACE_ASKED_FOR`] carries, and in what
/// order: the run going on now, then the run before it.
///
/// Both every time, rather than the asker naming which one it wants: the
/// supervisor would then have to read from whoever asked before it could answer,
/// and a read from something that may never write is the supervisor stopping for
/// the one thing it must not stop for (ADR-0008). An answer shorter than this is
/// a supervisor with only the current run to give, which is what one that has
/// just come up has.
pub const TRACES_HANDED_OVER: usize = 2;

/// Where a supervisor listens for somebody asking for the recording (ADR-0009).
///
/// Stated here for the reason the descriptor names above are: two processes have
/// to agree on it, and a second copy of the agreement is a program that asks
/// somewhere nobody is listening. Unlike those, this one is a name in the
/// filesystem rather than an inherited handle — the process that asks is not the
/// supervisor's child, so there is nothing for it to have inherited.
pub const TRACE_ASKED_FOR: &str = "/var/run/favjit-trace.sock";

/// Whether a run has already said that its beats are not arriving.
///
/// The policy rather than the flag: once and not per event, because the pipe
/// does not come back and a line per keystroke would be the loudest thing in
/// the log for the rest of the run. Kept by the loop that beats, so a run that
/// has said it once carries on doing its job in silence (ADR-0006: how often
/// it is said).
#[derive(Debug, Default)]
pub(crate) struct Beating {
    said: bool,
}

impl Beating {
    /// Say the loop came back round, and say so about a beat that did not
    /// arrive the first time it does not.
    pub(crate) fn beat<H: favjit_host::Host + ?Sized>(&mut self, host: &mut H) {
        // Asked first, because a machine has one answer for a beat nobody is
        // waiting for and for one that did not arrive: a run started by hand has
        // no pipe to write down, and reporting that as a heartbeat which failed
        // would say the watchdog is about to end a process no watchdog started.
        if !host.is_supervised() {
            return;
        }
        let Err(trouble) = host.heartbeat() else {
            return;
        };
        if self.said {
            return;
        }
        self.said = true;
        let said = trouble.0;
        host.warn(format_args!(
            "the heartbeat is not reaching the watchdog ({said}); it will end this process for a \
             broken link rather than for a fault"
        ));
    }
}
