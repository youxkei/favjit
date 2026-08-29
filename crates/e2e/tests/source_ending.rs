//! How a forwarding run ends, and what it says about it.
//!
//! Every one of these ends the run rather than carrying on differently, because
//! what suppression rests on is the run ending: input refused by a process that
//! has stopped relaying is the outcome ADR-0008 rules out. Which of them happened
//! is what whatever started favjit acts on, so it is reported rather than only
//! logged — and a host that decided what it meant would be deciding the exit code
//! (ADR-0006).

use favjit_core::source::{run, Ended, Ending, Request};
use favjit_core::{DeviceId, DeviceInfo, Key};
use favjit_host_sim::SimSource;

const KEYBOARD: DeviceId = DeviceId(1);

/// A run that relays and suppresses, which is the mode every ending below is
/// reached from except the one that names the dry run.
fn relaying() -> Request {
    Request::Relaying
}

fn typed_on() -> SimSource {
    let mut host = SimSource::new();
    host.attach(DeviceInfo::external(KEYBOARD, 0x17ef, 0x60e1));
    host.tap(KEYBOARD, Key::J);
    host
}

#[test]
fn a_run_that_relayed_until_it_was_asked_to_stop_says_so() {
    let mut host = typed_on();

    assert_eq!(run(&relaying(), &mut host), Ending::Relayed);
}

#[test]
fn a_dry_run_ends_the_same_way() {
    // Nothing was relayed and nothing is wrong: the run did what it was asked and
    // stopped, which is the same ending as a relaying one.
    let mut host = typed_on();

    assert_eq!(run(&Request::DryRun, &mut host), Ending::Relayed);
}

#[test]
fn keyboards_that_cannot_be_read_end_the_run_before_it_connects() {
    // Asked first, because a machine whose keyboards this process cannot read has
    // nothing to forward — and opening a socket to say so would be a link to a
    // machine that will be sent nothing.
    let mut host = typed_on();
    host.cannot_read_input();

    assert_eq!(run(&relaying(), &mut host), Ending::NoInput);
    assert_eq!(host.connects(), 0);
    assert_eq!(host.suppressions(), 0);
}

#[test]
fn reading_that_stops_while_the_run_is_going_is_not_the_same_as_being_asked_to() {
    // A capture that died leaves a stream that has ended, which is exactly what
    // the bound the run was given leaves. Told apart only by asking, and worth
    // telling apart: one is a run that finished and one is a run that was cut off.
    let mut host = typed_on();
    host.stopped_reading();

    assert_eq!(run(&relaying(), &mut host), Ending::InputGone);
}

#[test]
fn a_run_with_nowhere_to_relay_to_says_that_rather_than_waiting_for_ever() {
    // `Connected::Done` is the host saying no attempt will ever work — no identity,
    // no sink pinned. Waiting for that to change is waiting for a person, and the
    // run has nothing to hold in the meantime.
    let mut host = typed_on();
    host.no_sink();

    assert_eq!(run(&relaying(), &mut host), Ending::NoLink);
    assert!(
        !host.keyboards_taken(),
        "and the keyboards are not held while it says so"
    );
}

#[test]
fn the_reason_the_stream_ended_is_the_hosts_to_report_and_not_to_act_on() {
    // The host answers the question and nothing else: what each answer means for
    // the run is `core`'s, which is why the same `Ended` maps onto different
    // endings and the host never sees an `Ending`.
    for (reason, ending) in [
        (Ended::AsAsked, Ending::Relayed),
        (Ended::InputGone, Ending::InputGone),
    ] {
        let mut host = typed_on();
        host.ends_because(reason);

        assert_eq!(run(&relaying(), &mut host), ending);
    }
}
