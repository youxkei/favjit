//! The order the handshake happens in, which is the sink's and not a platform's.
//!
//! Every step is one call into whatever the platform provides, so the sequence over
//! them is the thing that can be wrong: an answer sent before the source's own
//! message was opened, a frame read from a connection that never became one. Each
//! of these is a whole run of the converter, so the sequence is driven from where
//! it is actually reached (ADR-0006, ADR-0012).

use favjit_engine::sink::{self, InputConfig, Request};
use favjit_engine::Layout;
use favjit_host_sim::{identity, Call, SimHost, SimLink};

const PAIRED: u8 = 0xaa;

/// The file `favjit --pair` leaves behind, holding that one key.
///
/// Written here rather than built with the simulator's own writer: a script that
/// started from the same function the simulator writes that file with would be
/// checking that function against itself.
fn paired_list() -> String {
    format!("{}\n", hex(identity(PAIRED).public()))
}

fn hex(bytes: &[u8]) -> String {
    use core::fmt::Write;
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

fn converting() -> Request {
    Request::Injecting { listen: true }
}

/// A whole run, with this script on the other end of the machine's link.
fn served(script: impl FnOnce(&mut SimLink)) -> SimHost {
    let mut link = SimLink::new(paired_list());
    script(&mut link);
    let mut mac = SimHost::new().with_link(link);
    sink::run(
        &converting(),
        Layout::dudrack(),
        None,
        InputConfig::default(),
        &mut mac,
        None,
    );
    mac
}

#[test]
fn advertisement_reads_the_listener_port_before_registering_it() {
    let mac = served(|_| {});

    assert_eq!(
        &mac.link_calls()[..2],
        [Call::ReadListenerPort, Call::Advertised]
    );
}

#[test]
fn a_port_that_cannot_be_read_skips_registration_and_still_serves() {
    let mac = served(|link| {
        link.connect(identity(PAIRED))
            .hang_up()
            .fails_at(Call::ReadListenerPort);
    });

    assert_eq!(mac.link_calls()[0], Call::ReadListenerPort);
    assert!(!mac.link_calls().contains(&Call::Advertised));
    assert!(mac.link_calls().contains(&Call::Accepted));
}

#[test]
fn failed_registration_still_serves_the_listener() {
    let mac = served(|link| {
        link.connect(identity(PAIRED))
            .hang_up()
            .fails_at(Call::Advertised);
    });

    assert_eq!(
        &mac.link_calls()[..3],
        [Call::ReadListenerPort, Call::Advertised, Call::Accepted]
    );
}

#[test]
fn the_source_is_read_first_and_answered_after() {
    // The source speaks first in this pattern, so an end that answered before
    // opening what it was sent would be answering nothing.
    let mac = served(|link| {
        link.connect(identity(PAIRED)).hang_up();
    });

    assert_eq!(
        mac.link_calls(),
        vec![
            Call::ReadListenerPort,
            Call::Advertised,
            Call::Accepted,
            Call::SetReadTimeout,
            Call::SetNoDelay,
            Call::TookHandshake,
            Call::SentAnswer,
            Call::FlushedAnswer,
            Call::Authorized,
            Call::TookRecord,
        ]
    );
}

#[test]
fn a_failed_connection_configuration_stops_before_the_handshake() {
    for failed in [Call::SetReadTimeout, Call::SetNoDelay] {
        let mac = served(|link| {
            link.connect(identity(PAIRED)).fails_at(failed);
        });

        assert_eq!(mac.link_calls().last(), Some(&failed));
        assert!(!mac.link_calls().contains(&Call::TookHandshake));
    }
}

#[test]
fn a_first_message_that_never_arrives_ends_the_connection_there() {
    // Nothing further is asked of a connection that sent nothing: an end that went
    // on to answer would be writing to a peer that is not talking.
    let mac = served(|link| {
        link.connects_and_says_nothing();
    });

    assert_eq!(
        mac.link_calls(),
        vec![
            Call::ReadListenerPort,
            Call::Advertised,
            Call::Accepted,
            Call::SetReadTimeout,
            Call::SetNoDelay,
            Call::TookHandshake,
        ]
    );
    assert_eq!(mac.link_closed().len(), 1, "{:?}", mac.link_closed());
}

#[test]
fn a_first_message_that_will_not_open_is_never_answered() {
    // A message this end cannot open is a machine that pinned another key, or
    // nothing that speaks this protocol at all. Answering it would tell either one
    // what this end's ephemeral key is.
    let mac = served(|link| {
        link.connects_with_nonsense();
    });

    assert_eq!(
        mac.link_calls(),
        vec![
            Call::ReadListenerPort,
            Call::Advertised,
            Call::Accepted,
            Call::SetReadTimeout,
            Call::SetNoDelay,
            Call::TookHandshake,
        ]
    );
    assert!(!mac.link_calls().contains(&Call::SentAnswer));
}

#[test]
fn an_answer_that_cannot_be_sent_is_not_a_session() {
    // The connection is gone by the time the answer is written, and a key taken
    // from a handshake nobody received would be a source this end thinks is there.
    let mac = served(|link| {
        link.connects_and_drops_before_the_answer(identity(PAIRED));
    });

    assert!(!mac.link_calls().contains(&Call::Authorized));
}

#[test]
fn an_answer_that_cannot_be_flushed_stops_before_authorization() {
    let mac = served(|link| {
        link.connect(identity(PAIRED)).answer_cannot_be_flushed();
    });

    assert_eq!(
        mac.link_calls(),
        vec![
            Call::ReadListenerPort,
            Call::Advertised,
            Call::Accepted,
            Call::SetReadTimeout,
            Call::SetNoDelay,
            Call::TookHandshake,
            Call::SentAnswer,
            Call::FlushedAnswer,
        ]
    );
}

#[test]
fn a_record_that_will_not_open_ends_the_session() {
    // Both ways a record can fail are one answer to the end reading it, and the
    // messages around it come from a stream whose meaning is already in doubt.
    let mac = served(|link| {
        link.connect(identity(PAIRED))
            .sends_a_record_that_will_not_open();
    });

    // The refusal and nothing else: the frame inside that record never reached
    // the converter, and what did is the record naming which failure it was
    // (ADR-0009).
    assert_eq!(
        mac.delivered(),
        vec![favjit_host::EventKind::LinkEnded {
            why: favjit_engine::link::Refused::RecordWillNotOpen as u32
        }]
    );
    assert_eq!(mac.link_closed().len(), 1, "{:?}", mac.link_closed());
    assert_eq!(mac.reports(), &[]);
}
