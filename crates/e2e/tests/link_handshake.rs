//! The order the handshake happens in, which is the sink's and not a platform's.
//!
//! Every step is one call into whatever the platform provides, so the sequence over
//! them is the thing that can be wrong: an answer sent before the source's own
//! message was opened, a key believed before the session exists, a frame read from
//! a connection that never became one. Each of these is a whole run of the
//! converter, so the sequence is driven from where it is actually reached
//! (ADR-0006, ADR-0017).

use favjit_core::pairing::Authorized;
use favjit_core::sink::{self, Request};
use favjit_core::Layout;
use favjit_host_sim::{Call, SimHost, SimLink};

fn key(fill: u8) -> Vec<u8> {
    vec![fill; 32]
}

const PAIRED: u8 = 0xaa;

fn paired_list() -> String {
    Authorized::added("", &key(PAIRED))
}

fn converting() -> Request {
    Request::Injecting { listen: true }
}

/// A whole run, with this script on the other end of the machine's link.
fn served(script: impl FnOnce(&mut SimLink)) -> SimHost {
    let mut link = SimLink::new(paired_list());
    script(&mut link);
    let mut mac = SimHost::new().with_link(link);
    sink::run(&converting(), Layout::dudrack(), None, &mut mac, None);
    mac
}

#[test]
fn the_source_is_read_first_and_answered_after() {
    // The source speaks first in this pattern, so an end that answered before
    // opening what it was sent would be answering nothing.
    let mac = served(|link| {
        link.connect(key(PAIRED)).hang_up();
    });

    assert_eq!(
        mac.link_calls(),
        vec![
            Call::Accepted,
            Call::TookHandshake,
            Call::Answered,
            Call::SentAnswer,
            Call::Peer,
            Call::Authorized,
            Call::TookRecord,
        ]
    );
}

#[test]
fn a_first_message_that_never_arrives_ends_the_connection_there() {
    // Nothing further is asked of a connection that sent nothing: an end that went
    // on to answer would be writing to a peer that is not talking.
    let mac = served(|link| {
        link.connects_and_says_nothing();
    });

    assert_eq!(mac.link_calls(), vec![Call::Accepted, Call::TookHandshake]);
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
        vec![Call::Accepted, Call::TookHandshake, Call::Answered]
    );
    assert!(!mac.link_calls().contains(&Call::SentAnswer));
}

#[test]
fn an_answer_that_cannot_be_sent_is_not_a_session() {
    // The connection is gone by the time the answer is written, and a key taken
    // from a handshake nobody received would be a source this end thinks is there.
    let mac = served(|link| {
        link.connects_and_drops_before_the_answer();
    });

    assert!(!mac.link_calls().contains(&Call::Peer));
    assert!(!mac.link_calls().contains(&Call::Authorized));
}

#[test]
fn the_list_is_read_only_after_there_is_a_key_to_look_up() {
    // The refusal ADR-0004 asks for is of a source that has proved which key it
    // holds: a list consulted before the handshake would be a list consulted for
    // nobody.
    let mac = served(|link| {
        link.connect(key(PAIRED)).hang_up();
    });

    let steps = mac.link_calls();
    let peer = steps.iter().position(|step| *step == Call::Peer);
    let list = steps.iter().position(|step| *step == Call::Authorized);
    assert!(peer < list, "{steps:?}");
}

#[test]
fn a_record_that_will_not_open_ends_the_session() {
    // Both ways a record can fail are one answer to the end reading it, and the
    // messages around it come from a stream whose meaning is already in doubt.
    let mac = served(|link| {
        link.connect(key(PAIRED))
            .sends_a_record_that_will_not_open();
    });

    assert_eq!(mac.delivered(), Vec::new());
    assert_eq!(mac.link_closed().len(), 1, "{:?}", mac.link_closed());
    assert_eq!(mac.injected(), Vec::new());
}
