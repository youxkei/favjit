//! What a forwarding run does before it relays anything, and what a run that was
//! not asked to relay does instead.
//!
//! Through `source::run` rather than the relay loop, because that is what a person
//! starts: the mode a run is in decides whether it opens a socket and whether it
//! takes the keyboards away, and a suite that called the loop directly would pass
//! while every one of those decisions was wrong.

use favjit_core::source::{run, Request};
use favjit_core::{DeviceId, DeviceInfo, Key};
use favjit_host_sim::SimSource;

const KEYBOARD: DeviceId = DeviceId(1);

fn keyboard() -> DeviceInfo {
    DeviceInfo::external(KEYBOARD, 0x17ef, 0x60e1)
}

/// A machine with a keyboard on it and something typed.
fn typed_on() -> SimSource {
    let mut host = SimSource::new();
    host.attach(keyboard());
    host.tap(KEYBOARD, Key::J);
    host
}

#[test]
fn a_run_that_is_not_relaying_reaches_for_nothing_outside_the_machine() {
    // The default, and the whole of what makes it safe to be the default: no
    // socket is opened, so a machine with nothing paired and no Mac switched on
    // still runs, and nothing this run reads leaves it.
    let mut host = typed_on();

    run(&Request::DryRun, &mut host);

    assert_eq!(host.connects(), 0, "nothing is connected to");
    assert_eq!(host.sent().len(), 0, "nothing is sent");
}

#[test]
fn a_run_that_is_not_relaying_never_takes_the_keyboards() {
    // Input refused with nowhere to send it is a keyboard that has stopped, and
    // there is no way to ask for it: refusing is what relaying does, so the mode
    // that sends nothing has nothing to refuse with.
    let mut host = typed_on();

    run(&Request::DryRun, &mut host);

    assert_eq!(host.suppressions(), 0);
    assert!(!host.keyboards_taken());
}

#[test]
fn a_run_that_is_not_relaying_still_reads_the_keyboards() {
    // What it is for: seeing which keys arrive and what they are named, on a
    // machine where nothing has been set up.
    let mut host = typed_on();

    run(&Request::DryRun, &mut host);

    assert_eq!(
        host.heartbeats().len(),
        3,
        "the attach and the press and release"
    );
}

#[test]
fn a_relaying_run_takes_the_keyboards_once_and_gives_them_back() {
    // Refusing is not a separate thing to ask for: a run that relays takes this
    // machine's input, because a keystroke that stayed here as well would land on
    // both screens.
    let mut host = typed_on();

    run(&Request::Relaying, &mut host);

    assert_eq!(host.suppressions(), 1);
    assert!(!host.keyboards_taken());
    assert_eq!(host.sent().len(), 3, "the attach, the press, the release");
}
