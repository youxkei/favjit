//! What the source does about the link it depends on.
//!
//! The machine input comes from has the same rule as the one it goes to: a failure
//! must degrade to "favjit stopped working", never to "the keyboard stopped
//! working" (ADR-0008). On this side that means the keys are only taken while
//! there is somewhere to send them.

use favjit_core::link::Message;
use favjit_core::source::{self, Request, Suppressing};
use favjit_core::{DeviceId, DeviceInfo, Key};
use favjit_host_sim::SimSource;

const LOCAL: DeviceId = DeviceId(1);

/// A whole run that relays and suppresses, which is the only mode in which the
/// link is what these are about.
fn relaying(host: &mut SimSource) {
    source::run(&Request::Relaying, host);
}

#[test]
fn nothing_is_suppressed_until_there_is_a_link() {
    // Taking the keyboard before there is anywhere to send it is taking the
    // keyboard away: the person is left typing into nothing on the machine in
    // front of them.
    let mut host = SimSource::new();
    host.sink_missing(2);
    host.attach(DeviceInfo::external(LOCAL, 1, 2));
    host.tap(LOCAL, Key::K);

    relaying(&mut host);

    assert_eq!(host.suppressed_before_connecting(), 0);
}

#[test]
fn a_sink_that_is_not_there_yet_is_waited_for() {
    // The other machine may be asleep, rebooting or not on the network yet, and
    // none of those is a reason for this one to stop relaying for good.
    let mut host = SimSource::new();
    host.sink_missing(3);
    host.attach(DeviceInfo::external(LOCAL, 1, 2));
    host.tap(LOCAL, Key::K);

    relaying(&mut host);

    assert_eq!(host.connects(), 4);
    assert_eq!(host.sent().len(), 3);
}

#[test]
fn the_keys_are_taken_while_there_is_a_link_and_given_back_at_the_end() {
    // Taken once, when the link came up, and let go when there is nothing left to
    // relay: a run that ended still holding them would leave the machine it was
    // running on unusable until somebody noticed.
    let mut host = SimSource::new();
    host.attach(DeviceInfo::external(LOCAL, 1, 2));
    host.tap(LOCAL, Key::K);

    relaying(&mut host);

    assert_eq!(host.suppressions(), 1);
    assert!(!host.keyboards_taken());
    assert_eq!(host.sent().len(), 3);
}

#[test]
fn the_keys_are_given_back_when_the_link_drops() {
    // The link going away is exactly when the person needs their own keyboard: it
    // is how they get to the machine to find out what happened.
    //
    // Read as a sequence, because the state at the end says nothing at all: a run
    // releases everything as it stops, so one that held the keyboards for its whole
    // life looks afterwards exactly like one that gave them back the moment the link
    // went. What the rule actually is, is that the keyboards are never taken twice
    // without being given back in between — every way round this loop passes through
    // "there is no link", and that is where they come back (ADR-0008).
    let mut host = SimSource::new();
    host.attach(DeviceInfo::external(LOCAL, 1, 2));
    host.link_gone();
    host.tap(LOCAL, Key::K);

    relaying(&mut host);

    assert!(
        host.refusals().contains(&Suppressing::Everything),
        "nothing was ever taken, so this says nothing: {:?}",
        host.refusals()
    );
    assert!(
        !host
            .refusals()
            .windows(2)
            .any(|pair| pair == [Suppressing::Everything, Suppressing::Everything]),
        "the keyboards were taken again without being given back in between: {:?}",
        host.refusals()
    );
}

#[test]
fn input_from_before_the_link_is_not_relayed_afterwards() {
    // A keystroke that happened while there was nowhere to send it belongs to the
    // machine it was typed on. Sending it once the link comes up would type it
    // twice, seconds late.
    let mut host = SimSource::new();
    host.sink_missing(1);
    host.attach(DeviceInfo::external(LOCAL, 1, 2));
    host.tap(LOCAL, Key::K);

    relaying(&mut host);

    let sent: Vec<Message> = host.sent().iter().map(|s| s.message).collect();
    assert_eq!(
        sent,
        vec![
            Message::DeviceAttached(DeviceInfo::external(LOCAL, 1, 2)),
            Message::KeyDown {
                device: LOCAL,
                key: Key::K
            },
            Message::KeyUp {
                device: LOCAL,
                key: Key::K
            },
        ]
    );
}
