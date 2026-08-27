//! What the source does about the link it depends on.
//!
//! The machine input comes from has the same rule as the one it goes to: a failure
//! must degrade to "favjit stopped working", never to "the keyboard stopped
//! working" (ADR-0008). On this side that means the keys are only taken while
//! there is somewhere to send them.

use favjit_engine::link::{Attached, Message};
use favjit_engine::source::{self, Request, Suppressing};
use favjit_engine::{DeviceId, Key};
use favjit_host::source::Driving;
use favjit_host_sim::{identity, SimSource, SourceCall};

const LOCAL: DeviceId = DeviceId(1);

/// The sink this machine is pinned to.
const SINK: u8 = 2;

/// A whole run that relays and suppresses, which is the only mode in which the
/// link is what these are about. Pinned to [`SINK`], which every host here is
/// pinned to as well: the handshake has to open for the link to be what a test
/// scripted it to be.
fn relaying(host: &mut SimSource) {
    source::run(&Request::Relaying, false, host, None);
}

/// A machine that has been asked to send its keyboard over, before anything else.
///
/// A run comes up with the keyboard on the machine it is running on and looks for the
/// Mac when it is asked to send it (ADR-0013), so a question about the link starts here.
fn sending() -> SimSource {
    let mut host = SimSource::default();
    host.asked_for(Driving::TheSink);
    host
}

#[test]
fn nothing_is_suppressed_until_there_is_a_link() {
    // Taking the keyboard before there is anywhere to send it is taking the
    // keyboard away: the person is left typing into nothing on the machine in
    // front of them.
    let mut host = sending();
    host.pinned_to(identity(SINK));
    host.sink_missing(2);
    host.attach_external(LOCAL, 1, 2);
    host.tap(LOCAL, Key::K);

    relaying(&mut host);

    assert_eq!(host.suppressed_before_connecting(), 0);
}

#[test]
fn a_sink_that_is_not_there_yet_is_waited_for() {
    // The other machine may be asleep, rebooting or not on the network yet, and
    // none of those is a reason for this one to stop relaying for good.
    let mut host = sending();
    host.pinned_to(identity(SINK));
    host.sink_missing(3);
    host.attach_external(LOCAL, 1, 2);
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
    let mut host = sending();
    host.pinned_to(identity(SINK));
    host.attach_external(LOCAL, 1, 2);
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
    let mut host = sending();
    host.pinned_to(identity(SINK));
    host.attach_external(LOCAL, 1, 2);
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
fn a_link_that_dropped_is_said_once_for_each_time_it_goes() {
    // The keyboard coming back to this machine is the whole of what the person sees, and
    // it is what a chord that went unheard looks like too. Nothing else in the run says
    // a link went: the looking that follows says something only when nothing answers,
    // and a link that comes straight back says nothing at all.
    //
    // Matched on a word rather than the sentence, and counted rather than merely found:
    // it is said per drop, so a run told once about two of them is one whose log cannot
    // be read as a sequence.
    let mut host = sending();
    host.pinned_to(identity(SINK));
    host.attach_external(LOCAL, 1, 2);
    host.link_gone();
    host.tap(LOCAL, Key::K);
    host.link_back();
    host.link_gone();
    host.tap(LOCAL, Key::L);
    host.link_back();
    host.tap(LOCAL, Key::M);

    relaying(&mut host);

    assert_eq!(
        host.warnings()
            .iter()
            .filter(|said| said.contains("the link to the other machine has gone"))
            .count(),
        2,
        "what the run said: {:?}",
        host.warnings()
    );
}

#[test]
fn a_link_that_dropped_is_looked_for_again_with_no_pause_in_front_of_it() {
    // The pause between attempts is for a machine that is not answering. A link that
    // was carrying keystrokes a moment ago is not that machine, and the person is
    // sitting at a keyboard that has just stopped reaching the screen they were typing
    // at — so the wait belongs after the attempt that fails, not in front of the first
    // one.
    let mut host = sending();
    host.pinned_to(identity(SINK));
    host.attach_external(LOCAL, 1, 2);
    host.link_gone();
    // The message this one produces is the one that finds out the link has gone.
    host.tap(LOCAL, Key::K);
    host.link_back();
    host.tap(LOCAL, Key::L);

    relaying(&mut host);

    let crossed: Vec<Message> = host.sent().iter().map(|sent| sent.message).collect();
    assert!(
        crossed.contains(&Message::KeyDown {
            device: LOCAL,
            key: Key::L
        }),
        "the link never came back: {crossed:?}"
    );
    assert_eq!(
        host.pauses(),
        0,
        "the keyboard sat out a wait meant for a machine that was not answering"
    );
}

#[test]
fn a_sink_that_answers_and_then_will_not_talk_is_looked_for_again() {
    // Between mDNS saying a machine is there and a session being open there are
    // four more things that can go: the connection, the handshake's first
    // message, its flush, and the answer to it. None of them is a machine to give up on — the
    // Mac may be up but not yet serving, or paired with somebody else — so each is
    // the same round again, with the keyboard left where it is until a session is
    // actually open.
    for attempt in 0..4 {
        let mut host = sending();
        host.pinned_to(identity(SINK));
        match attempt {
            0 => host.socket_refused(1),
            1 => host.handshake_dropped(1),
            2 => host.handshake_flush_dropped(1),
            _ => host.answer_dropped(1),
        };
        host.attach_external(LOCAL, 1, 2);
        host.tap(LOCAL, Key::K);

        relaying(&mut host);

        assert_eq!(
            host.suppressed_before_connecting(),
            0,
            "the keyboard was taken with no session to send it to (attempt {attempt})"
        );
        assert_eq!(
            host.sent().len(),
            3,
            "and the second attempt still relays what was typed after it (attempt \
             {attempt})"
        );
    }
}

#[test]
fn every_connection_operation_is_ordered_by_the_source_run() {
    let mut host = sending();
    host.pinned_to(identity(SINK));
    host.attach_external(LOCAL, 1, 2);
    host.tap(LOCAL, Key::K);

    relaying(&mut host);

    assert_eq!(
        host.source_calls(),
        [
            SourceCall::AskedIfItWasAskedToStop,
            SourceCall::CheckedForASinkToLookFor,
            SourceCall::TriedFixedAddress,
            SourceCall::Connected,
            SourceCall::SetNoDelay,
            SourceCall::SetReadTimeout,
            SourceCall::SetWriteTimeout,
            SourceCall::SentFirstMessage,
            SourceCall::FlushedFirstMessage,
            SourceCall::TookAnswer,
            // Last, once the stream has ended: why it did is worked out from
            // this fact rather than reported by the machine, so it is one more
            // operation the run orders (ADR-0006).
            SourceCall::AskedIfItWasAskedToStop,
        ]
    );
}

#[test]
fn a_failed_connection_operation_stops_before_the_next_one() {
    for (failed, connection_calls) in [
        (SourceCall::Connected, &[SourceCall::Connected][..]),
        (
            SourceCall::SetNoDelay,
            &[SourceCall::Connected, SourceCall::SetNoDelay][..],
        ),
        (
            SourceCall::SetReadTimeout,
            &[
                SourceCall::Connected,
                SourceCall::SetNoDelay,
                SourceCall::SetReadTimeout,
            ],
        ),
        (
            SourceCall::SetWriteTimeout,
            &[
                SourceCall::Connected,
                SourceCall::SetNoDelay,
                SourceCall::SetReadTimeout,
                SourceCall::SetWriteTimeout,
            ],
        ),
    ] {
        let mut host = sending();
        host.pinned_to(identity(SINK));
        host.whose_link_fails_at(failed);

        relaying(&mut host);

        let calls = host.source_calls();
        let prefix = [
            SourceCall::AskedIfItWasAskedToStop,
            SourceCall::CheckedForASinkToLookFor,
            SourceCall::TriedFixedAddress,
        ];
        assert_eq!(&calls[..prefix.len()], prefix);
        assert_eq!(
            &calls[prefix.len()..prefix.len() + connection_calls.len()],
            connection_calls
        );
        assert_eq!(
            calls.get(prefix.len() + connection_calls.len()),
            Some(&SourceCall::AskedIfItWasAskedToStop)
        );
    }
}

#[test]
fn input_from_before_the_link_is_not_relayed_afterwards() {
    // A keystroke that happened while there was nowhere to send it belongs to the
    // machine it was typed on. Sending it once the link comes up would type it
    // twice, seconds late.
    let mut host = sending();
    host.pinned_to(identity(SINK));
    host.sink_missing(1);
    host.attach_external(LOCAL, 1, 2);
    host.tap(LOCAL, Key::K);

    relaying(&mut host);

    let sent: Vec<Message> = host.sent().iter().map(|s| s.message).collect();
    assert_eq!(
        sent,
        vec![
            Message::DeviceAttached(Attached {
                device: LOCAL,
                is_built_in: false,
                vendor_id: Some(1),
                product_id: Some(2),
            }),
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
