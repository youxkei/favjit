//! What the supervising process decides (ADR-0008).
//!
//! `watchdog_liveness.rs` pins the other half of this — that each role answers every
//! probe it is given — from inside the roles. These pin the supervisor's own
//! judgement: when it asks, how long it waits, and what it does about a silence.
//!
//! It is here rather than read off the implementation because the judgement is the
//! part that must not be wrong, and it is now one copy for both machines: a bound
//! that was too tight would kill a converter mid-keystroke, and one that never fired
//! would leave a keyboard that does nothing — the outcome ADR-0008 exists to
//! prevent.
//!
//! No real time passes here. The simulated machine owns the clock, so a two-second
//! silence is produced by a wait that returns immediately (ADR-0007).

use core::time::Duration;

use favjit_core::watchdog::{run, Bound, Exit, Supervised};
use favjit_host_sim::{SimWatchdog, Told};

const SILENCE: Duration = Duration::from_secs(2);
const PROBE_EVERY: Duration = Duration::from_millis(250);
const GRACE: Duration = Duration::from_millis(200);

/// Two seconds of silence, probed four times a second, and a fifth of a second to
/// stop in — the numbers a person is judged by rather than a program: loose enough
/// that a burst of keys is not a wedge, tight enough that the keyboard comes back
/// before the power button does.
fn bound() -> Bound {
    Bound {
        silence: SILENCE,
        probe_every: PROBE_EVERY,
        grace: GRACE,
    }
}

#[test]
fn a_process_that_answers_every_probe_is_left_alone() {
    // The ordinary run. It ends because it was given a bound of its own, which is
    // how favjit gets measured, and the supervisor passes that through.
    let mut machine = SimWatchdog::new().that_exits_after(5, Exit::Code(0));

    assert_eq!(
        run(&bound(), &mut machine),
        Supervised::Ended(Exit::Code(0))
    );
    assert_eq!(machine.killed(), None);
}

#[test]
fn a_process_that_stops_answering_is_ended() {
    // The failure this exists for: alive, still holding the keyboards, no longer
    // able to give them back itself.
    let mut machine = SimWatchdog::new().that_never_answers();

    assert_eq!(run(&bound(), &mut machine), Supervised::Killed);
    assert!(machine.killed().is_some());
}

#[test]
fn it_is_ended_when_the_bound_has_passed_and_not_before() {
    // The bound is what separates a stall from a wedge. Ending early is a converter
    // killed mid-keystroke; ending late is a keyboard that does nothing for longer
    // than a person will wait.
    //
    // The decision falls at the bound and the ending a grace period later, because
    // the process is asked to stop first — so this is the whole of how long a keyboard
    // can be unusable before it comes back.
    let mut machine = SimWatchdog::new().that_never_answers();
    let started = machine.started();

    run(&bound(), &mut machine);

    assert_eq!(
        machine.killed(),
        Some(started.saturating_add(SILENCE).saturating_add(GRACE))
    );
}

#[test]
fn a_process_that_answers_late_but_within_the_bound_is_left_alone() {
    // Answering slower than the probe rate is not a wedge. A supervisor that
    // required an answer per probe would kill a converter that is merely busy.
    let mut machine = SimWatchdog::new()
        .that_answers(4)
        .that_exits_after(4, Exit::Code(0));

    assert_eq!(
        run(&bound(), &mut machine),
        Supervised::Ended(Exit::Code(0))
    );
    assert_eq!(machine.killed(), None);
}

#[test]
fn the_first_probe_goes_out_before_anything_is_waited_for() {
    // So that a process wedged from the moment it started is found out within one
    // bound rather than one bound and an interval.
    let mut machine = SimWatchdog::new().that_never_answers();
    let started = machine.started();

    run(&bound(), &mut machine);

    assert_eq!(machine.probes().first(), Some(&started));
}

#[test]
fn probes_go_out_at_the_rate_they_were_asked_for() {
    // Not once per turn round the loop. A heartbeat cuts the wait short, so a probe
    // per turn would take its rate from the round trip instead — thousands a second,
    // each one waking the loop that is trying to convert keystrokes.
    let mut machine = SimWatchdog::new()
        .that_answers(3)
        .that_exits_after(3, Exit::Code(0));
    let started = machine.started();

    run(&bound(), &mut machine);

    let expected: Vec<_> = (0..3)
        .map(|n| started.saturating_add(PROBE_EVERY * n))
        .collect();
    assert_eq!(machine.probes(), expected);
}

#[test]
fn a_probe_that_cannot_be_sent_is_named_rather_than_counted_as_silence() {
    // A broken link and a wedged process look the same from the outside, and they
    // are not the same thing. The process is still ended — a supervisor that cannot
    // ask has no grounds to keep the keyboards away — but what happened is said.
    let mut machine = SimWatchdog::new().where_probes_cannot_be_sent();

    assert_eq!(run(&bound(), &mut machine), Supervised::Killed);
    assert!(
        machine.warnings().iter().any(|said| said.contains("probe")),
        "nothing said the probe could not be sent: {:?}",
        machine.warnings()
    );
}

#[test]
fn a_process_that_cannot_be_started_ends_the_supervision() {
    // Nothing has the keyboards, so there is nothing to give back and nothing to
    // wait for.
    let mut machine = SimWatchdog::new().that_cannot_start();

    assert_eq!(run(&bound(), &mut machine), Supervised::NotStarted);
    assert_eq!(machine.probes(), Vec::new());
    assert!(!machine.kept_the_trace());
}

#[test]
fn the_trace_is_kept_after_the_process_has_been_ended() {
    // The order is the whole point of keeping it here (ADR-0009): the record of why
    // the kill was needed lives in memory the kill destroys, so it is taken once the
    // process is gone.
    let mut machine = SimWatchdog::new().that_never_answers();

    run(&bound(), &mut machine);

    assert!(machine.kept_the_trace());
    assert!(
        machine.killed_before_keeping(),
        "the trace was kept before the process was ended"
    );
}

#[test]
fn nothing_is_kept_when_the_process_ended_on_its_own() {
    // There was no kill, so nothing was about to destroy the region — and a run that
    // finished is not one anybody needs the record of.
    let mut machine = SimWatchdog::new().that_exits_after(2, Exit::Code(0));

    run(&bound(), &mut machine);

    assert!(!machine.kept_the_trace());
}

#[test]
fn the_exit_code_of_a_process_that_ended_on_its_own_is_passed_through() {
    // Whatever started the supervisor acts on it, so a failing run has to stay a
    // failing run through one more process.
    let mut machine = SimWatchdog::new().that_exits_after(1, Exit::Code(3));

    assert_eq!(
        run(&bound(), &mut machine),
        Supervised::Ended(Exit::Code(3))
    );
}

#[test]
fn a_process_that_something_else_ended_is_told_apart_from_one_that_chose_to() {
    // It named no status of its own, and it did not finish what it was doing. What
    // that means to whatever started the supervisor is the binary's; that the two are
    // different is here.
    let mut machine = SimWatchdog::new().that_exits_after(1, Exit::Signalled);

    assert_eq!(
        run(&bound(), &mut machine),
        Supervised::Ended(Exit::Signalled)
    );
    assert_eq!(machine.killed(), None);
}

#[test]
fn the_process_is_asked_to_stop_before_it_is_stopped() {
    // A process given the chance puts the keyboards back itself, which leaves nothing
    // for the platform to have to clean up. The pause between is what makes the asking
    // worth anything, and the trace is taken last because the ending is what would
    // destroy it.
    let mut machine = SimWatchdog::new().that_never_answers();

    run(&bound(), &mut machine);

    assert_eq!(
        machine.told(),
        [
            Told::AskedItToStop,
            Told::Paused(GRACE),
            Told::EndedIt,
            Told::KeptTheTrace,
        ]
    );
}

#[test]
fn nothing_is_waited_out_where_there_was_nothing_to_ask_with() {
    // A platform with no signal to send. Waiting out a grace period nobody was granted
    // is a keyboard left unusable for no reason, so the asking's answer is what decides
    // whether the pause happens at all.
    let mut machine = SimWatchdog::new()
        .that_never_answers()
        .where_asking_does_not_work();

    run(&bound(), &mut machine);

    assert_eq!(
        machine.told(),
        [Told::AskedItToStop, Told::EndedIt, Told::KeptTheTrace]
    );
}
