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

pub use favjit_host::watchdog::{Beat, BeatKind, Exit, WatchdogHost};

use crate::clock::Clock;

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

        // Before the wait and not after it, so a run that is answering normally
        // still hands the recording over promptly: the wait is bounded by the next
        // probe, and asking after it would put a whole probe interval between the
        // question and the answer.
        if host.asked_for_the_trace() {
            host.hand_the_trace_over();
        }

        // Bounded by the next probe rather than by the silence, so the rate is the one
        // that was asked for however long the process stays quiet.
        let beat = host.wait_for_a_heartbeat(next_probe.saturating_duration_since(at));
        at = beat.at;
        if beat.kind == BeatKind::Beat {
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
