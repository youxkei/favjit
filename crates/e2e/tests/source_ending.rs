//! How a forwarding run ends, and what it says about it.
//!
//! Every one of these ends the run rather than carrying on differently, because
//! what suppression rests on is the run ending: input refused by a process that
//! has stopped relaying is the outcome ADR-0008 rules out. Which of them happened
//! is what whatever started favjit acts on, so it is reported rather than only
//! logged — and a host that decided what it meant would be deciding the exit code
//! (ADR-0006).

use favjit_engine::source::{run, Ending, Request};
use favjit_engine::{DeviceId, Key};
use favjit_host::source::Driving;
use favjit_host_sim::{identity, IdentityCall, SimSource};

const KEYBOARD: DeviceId = DeviceId(1);

/// The sink [`typed_on`] pins this machine to.
const SINK: u8 = 2;

/// A run that relays and suppresses, which is the mode every ending below is
/// reached from except the one that names the dry run.
fn relaying() -> Request {
    Request::Relaying
}

/// Relay against the sink [`typed_on`] pinned this machine to.
fn relayed(host: &mut SimSource) -> Ending {
    run(&relaying(), false, host, None).0
}

fn typed_on() -> SimSource {
    let mut host = SimSource::new();
    // Sent over first, because a run comes up with the keyboard on the machine it is
    // running on (ADR-0013) and what these are about is how a relaying one ends.
    host.asked_for(Driving::TheSink);
    host.pinned_to(identity(SINK));
    host.attach_external(KEYBOARD, 0x17ef, 0x60e1);
    host.tap(KEYBOARD, Key::J);
    host
}

#[test]
fn a_run_that_relayed_until_it_was_asked_to_stop_says_so() {
    let mut host = typed_on();

    assert_eq!(relayed(&mut host), Ending::Relayed);
}

#[test]
fn a_dry_run_ends_the_same_way() {
    // Nothing was relayed and nothing is wrong: the run did what it was asked and
    // stopped, which is the same ending as a relaying one.
    let mut host = typed_on();

    assert_eq!(
        run(&Request::DryRun, false, &mut host, None).0,
        Ending::Relayed
    );
}

#[test]
fn keyboards_that_cannot_be_read_end_the_run_before_it_connects() {
    // Asked first, because a machine whose keyboards this process cannot read has
    // nothing to forward — and opening a socket to say so would be a link to a
    // machine that will be sent nothing.
    let mut host = typed_on();
    host.cannot_read_input();

    assert_eq!(relayed(&mut host), Ending::NoInput);
    assert_eq!(host.connects(), 0);
    assert_eq!(host.suppressions(), 0);
}

#[test]
fn a_dry_run_that_cannot_read_the_keyboards_ends_there_too() {
    // The mode that reads and sends nothing still has nothing to do without the
    // keyboards, and it says the same thing about it: a dry run that reported
    // success having read nothing would be the answer a person checks their setup
    // with, saying the setup is fine.
    let mut host = typed_on();
    host.cannot_read_input();

    assert_eq!(
        run(&Request::DryRun, false, &mut host, None).0,
        Ending::NoInput
    );
    assert_eq!(host.connects(), 0, "and a dry run reaches no link either");
}

#[test]
fn reading_that_stops_while_the_run_is_going_is_not_the_same_as_being_asked_to() {
    // A capture that died leaves a stream that has ended, which is exactly what
    // the bound the run was given leaves. Told apart only by asking, and worth
    // telling apart: one is a run that finished and one is a run that was cut off.
    let mut host = typed_on();
    host.stopped_reading();

    assert_eq!(relayed(&mut host), Ending::InputGone);
}

#[test]
fn a_run_with_nowhere_to_relay_to_says_that_rather_than_waiting_for_ever() {
    // `Connected::Done` is the host saying no attempt will ever work. Waiting for
    // that to change is waiting for a person, and the run has nothing to hold in
    // the meantime.
    let mut host = typed_on();
    host.no_sink();

    assert_eq!(relayed(&mut host), Ending::NoLink);
    assert!(
        !host.keyboards_taken(),
        "and the keyboards are not held while it says so"
    );
}

#[test]
fn a_run_with_no_pinned_sink_says_so_before_looking() {
    // Asked for once, up front: it is not a question a machine answering mDNS
    // could change the answer to, so there is nothing to wait for and no reason
    // to open a socket first.
    let mut host = SimSource::new();
    host.asked_for(Driving::TheSink);
    host.attach_external(KEYBOARD, 0x17ef, 0x60e1);
    host.tap(KEYBOARD, Key::J);

    assert_eq!(
        relayed(&mut host),
        Ending::NoLink,
        "no sink pinned is the same ending as no sink to relay to"
    );
    assert_eq!(host.connects(), 0, "mDNS is never asked");
}

#[test]
fn a_run_whose_identity_cannot_be_made_says_so_before_looking() {
    // The same ending as no sink pinned, reached from the other fact a relaying
    // run asks for up front: a machine with nothing to present has nobody it
    // could open a session with either.
    let mut host = typed_on();
    host.whose_identity_fails_at(IdentityCall::Wrote);

    assert_eq!(relayed(&mut host), Ending::NoLink);
    assert_eq!(host.connects(), 0, "mDNS is never asked");
}

#[test]
fn the_run_works_out_why_it_stopped_from_facts_the_machine_answers_one_at_a_time() {
    // No host operation says why a run ended: each of these is one fact about
    // the machine, and which of them explains a stream with nothing left on it
    // is decided here — where the exit code whatever started favjit acts on is
    // read (ADR-0006).
    //
    // Nowhere to relay to comes first, because it is true whatever the bound
    // says; a bound that has passed is a run that finished; and a stream that
    // ran out with neither is reading having stopped.
    let mut nowhere = typed_on();
    nowhere.no_sink();
    assert_eq!(relayed(&mut nowhere), Ending::NoLink);

    let mut finished = typed_on();
    assert_eq!(relayed(&mut finished), Ending::Relayed);

    let mut cut_off = typed_on();
    cut_off.stopped_reading();
    assert_eq!(relayed(&mut cut_off), Ending::InputGone);
}
