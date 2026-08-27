//! The supervising process's judgement (ADR-0008).
//!
//! Its whole content is "a probe went in and no heartbeat came out, for longer than
//! this, so end it". That sentence is what must not be wrong, on either machine, so
//! it is here where the end-to-end suite drives it rather than once per platform
//! where nothing reaches it. What is left to a machine is the calls.
//!
//! **Nothing here reads a clock.** Every moment arrives on a [`Beat`], the way a
//! role's arrives on a [`crate::HostEvent`] — so how long a silence has lasted is
//! arithmetic over what the machine reported, and a simulated machine can produce
//! any silence at all without waiting through it.
//!
//! Nothing here knows what favjit is. A probe is a call that either went or did not,
//! a heartbeat is a moment, and what the supervised process does between them is
//! deliberately outside this module: a supervisor that understood its child would be
//! a second place for the child's logic to be wrong.

use core::time::Duration;

use crate::Instant;

/// What the supervision was told to allow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bound {
    /// How long the supervised process may go without reporting.
    ///
    /// Loose enough that an ordinary stall does not read as a wedge, tight enough
    /// that a person gets the keyboard back before reaching for the power button.
    /// Which numbers those are is the binary's, since they are a judgement about a
    /// person rather than about a program.
    pub silence: Duration,
    /// How often to ask.
    ///
    /// Separate from the silence because they answer different questions: how long to
    /// tolerate, and how soon to find out. A probe rate as slow as the bound would
    /// leave a wedge undetected for twice it.
    pub probe_every: Duration,
    /// How long a process gets to stop on its own before it is stopped.
    ///
    /// Here rather than in a host because the order it belongs to is here: a machine
    /// that held it would be deciding how long to leave a keyboard unusable, which is
    /// the one thing this program exists to bound.
    pub grace: Duration,
}

/// What the machine reported while the watchdog was waiting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BeatKind {
    /// The supervised loop came back round.
    Heartbeat,
    /// Nothing arrived before the wait was up.
    ///
    /// Reported rather than left as an absence, because the moment it came back with
    /// nothing is the moment the silence is measured against.
    Silence,
}

/// One thing the machine reported, and when.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Beat {
    pub at: Instant,
    pub kind: BeatKind,
}

/// How a supervised process ended on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exit {
    /// It chose its own status.
    Code(i32),
    /// Something else ended it, so it named no status of its own.
    ///
    /// Told apart from a code because they mean different things to whatever started
    /// the supervisor, and deciding which is not this module's (ADR-0006).
    Signalled,
}

/// The calls a watchdog makes on the machine it supervises.
///
/// Its own trait rather than [`crate::Host`]: a watchdog is the process nothing
/// supervises, it has no keyboards to read and no events to hand over, and a trait
/// covering both would give it three operations it must never call. Every operation is
/// one call into the platform and the answer it gave, and the order over them is
/// [`run`]'s (ADR-0006).
pub trait WatchdogHost {
    /// Start the process to supervise, and say when it started.
    ///
    /// `None` when it could not be started, which ends the supervision before anything
    /// is waited for: there is nothing to give a keyboard back from.
    ///
    /// The moment is the origin every later one is measured against, and it comes from
    /// here because `core` has no clock to ask.
    fn start(&mut self) -> Option<Instant>;

    /// Whether the process has ended on its own, and how.
    ///
    /// Its own operation rather than something the wait reports, because when it is
    /// asked is a decision: a process that has ended is not a wedge, and a supervisor
    /// that only asked after a silence would report the wrong one of the two.
    fn ended(&mut self) -> Option<Exit>;

    /// Wait up to `patience` for a heartbeat, and say what came back and when.
    ///
    /// The wait ADR-0006 allows a host to hold, for the reason a role's is: bounding
    /// it needs a clock. A machine that answers without letting the moment advance is
    /// one this loop cannot time out, which is why the answer carries the time.
    fn wait_for_a_heartbeat(&mut self, patience: Duration) -> Beat;

    /// Put a probe in, saying whether it went.
    ///
    /// Reported rather than acted on: a probe that could not be sent is a broken link
    /// and not a slow child, and treating it as a silence would say the wrong thing
    /// about a process that is working.
    fn probe(&mut self) -> bool;

    /// Ask the process to stop, saying whether there was a way to ask.
    ///
    /// `false` on a machine with nothing to ask with, which is not a failure: it is
    /// what makes the pause below pointless there, and skipping it is [`run`]'s
    /// decision rather than the machine's.
    fn ask_it_to_stop(&mut self) -> bool;

    /// Wait, once, for as long as this.
    ///
    /// The other wait ADR-0006 allows, and the same reason: `core` has no clock. What
    /// it is for is the moment between asking and insisting.
    fn pause(&mut self, how_long: Duration);

    /// End the process, so the keyboards come back.
    fn end_it(&mut self);

    /// Keep whatever the run recorded (ADR-0009).
    ///
    /// Called after the process is gone, because the ending is what would otherwise
    /// destroy the record of why it was needed. What a machine does with it — write it
    /// where it was told to, or say it is only in memory — is the machine's, since the
    /// place to write is that machine's configuration and not a decision about the
    /// process. A machine that keeps no trace says so.
    fn keep_the_trace(&mut self);

    /// Say something about the supervision itself.
    ///
    /// The one thing this process says while it is working, so it is a call rather
    /// than a logger: the suite has no other way to see that a broken probe link was
    /// noticed rather than counted as silence.
    fn warn(&mut self, message: core::fmt::Arguments);
}

/// How the supervision ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Supervised {
    /// The process could not be started.
    NotStarted,
    /// It ended on its own.
    ///
    /// Not a failure: a bounded run is how favjit gets measured, and a process that
    /// has stopped is not one holding a keyboard.
    Ended(Exit),
    /// It went quiet for longer than the bound, and was ended.
    Killed,
}

/// Supervise one process, start to finish.
///
/// Probe on the clock rather than once per turn round the loop: a heartbeat cuts the
/// wait short, so a probe per turn would set the rate from the round trip instead of
/// from the bound — thousands a second, each one waking the loop that is trying to
/// convert keystrokes.
///
/// The first probe is due immediately, so a process that never answers at all is found
/// out within one bound rather than one bound and an interval.
pub fn run(bound: &Bound, host: &mut dyn WatchdogHost) -> Supervised {
    let Some(started) = host.start() else {
        return Supervised::NotStarted;
    };
    let mut at = started;
    let mut last_beat = started;
    let mut next_probe = started;

    loop {
        // At the top of every turn, which is both before the first wait and after each
        // one: the wait is where a process most often ends, and one that has ended is
        // not a wedge.
        if let Some(exit) = host.ended() {
            return Supervised::Ended(exit);
        }

        if at >= next_probe {
            if !host.probe() {
                host.warn(format_args!(
                    "a probe could not be sent: the link to the supervised process is broken, so \
                     its silence says nothing about whether it is working"
                ));
            }
            next_probe = at.saturating_add(bound.probe_every);
        }

        // Bounded by the next probe rather than by the silence, so the rate is the one
        // that was asked for however long the process stays quiet.
        let beat = host.wait_for_a_heartbeat(next_probe.saturating_duration_since(at));
        at = beat.at;
        if beat.kind == BeatKind::Heartbeat {
            last_beat = at;
        }

        let silent = at.saturating_duration_since(last_beat);
        if silent >= bound.silence {
            host.warn(format_args!(
                "no heartbeat for {silent:?}: ending the run so the keyboard comes back"
            ));
            end(bound, host);
            return Supervised::Killed;
        }
    }
}

/// Ask, then insist.
///
/// A process given the chance to release the keyboards itself leaves nothing for the
/// platform to have to clean up, and insisting a moment later covers the case where it
/// cannot. The pause is skipped where there was nothing to ask with, because waiting
/// out a grace period nobody was granted is a keyboard left unusable for no reason.
fn end(bound: &Bound, host: &mut dyn WatchdogHost) {
    if host.ask_it_to_stop() {
        host.pause(bound.grace);
    }
    host.end_it();
    // After the ending and not before: what a trace is worth keeping for is the wedge,
    // and the ending is what destroys it (ADR-0009).
    host.keep_the_trace();
}
