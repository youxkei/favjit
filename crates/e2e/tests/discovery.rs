//! How the forwarding machine finds the machine it relays to.
//!
//! The sink binds whatever port it is given and says where it is, so finding it is
//! not a name lookup: what the source needs back is a port, and a machine that
//! answered without one is not somewhere input can be sent. Which answers are the
//! sink, and how many are read before giving up, are decisions — so they are
//! `favjit_discovery`'s and driven here, and the socket is the host's (ADR-0006).

use favjit_discovery::Found;
use favjit_engine::link::{PAIRING, SERVICE};
use favjit_engine::source::{self, Request};
use favjit_host::source::Driving;
use favjit_host_sim::{identity, SimDiscovery, SimSource, SourceCall};

use std::time::Duration;

/// The instance name a sink registers under, which is a person's machine name.
const INSTANCE: &str = "the mac";

/// The sink this machine is pinned to.
const SINK: u8 = 2;

fn advertised(port: u16, address: Option<[u8; 4]>) -> SimDiscovery {
    let mut network = SimDiscovery::new();
    network.advertises(SERVICE, INSTANCE, port, address);
    network
}

/// A whole forwarding run on a machine that can see this network.
///
/// Through `source::run` and not through the lookup, because the lookup is only
/// reached by a run that is relaying: called directly it would pass while nothing
/// wired it up.
fn run(network: &mut SimDiscovery) -> SimSource {
    let mut windows = SimSource::new();
    // Asked for the keyboard first, because that is what makes a run look for the Mac at
    // all: one comes up with the keyboard on the machine it is running on (ADR-0013).
    windows.asked_for(Driving::TheSink);
    windows.pinned_to(identity(SINK));
    windows.run_for(Duration::from_secs(2));
    windows.on_a_network(network.clone());
    source::run(&Request::Relaying, false, &mut windows, None);
    *network = windows.network().clone();
    windows
}

fn look(network: &mut SimDiscovery) -> Option<Found> {
    run(network).found()
}

#[test]
fn the_answer_that_carries_a_port_is_the_one_that_is_used() {
    // The whole reason a name is not enough: only the service record says which
    // port the sink took.
    let mut network = advertised(51763, None);

    assert_eq!(
        look(&mut network),
        Some(Found {
            host: String::from("the-mac.local"),
            port: 51763,
            address: None,
        })
    );
}

#[test]
fn the_question_goes_out_before_any_answer_is_read() {
    // A run that read the network without asking would find whatever happened to
    // be passing, and a sink that had already announced itself minutes ago is not
    // something the next answer carries.
    let mut network = advertised(9000, None);
    look(&mut network);

    assert_eq!(network.asked().len(), 1, "asked once");
    assert!(
        network.asked()[0].windows(7).any(|w| w == b"_favjit"),
        "and asked for favjit's own service"
    );
}

#[test]
fn every_discovery_operation_is_ordered_by_the_source_run() {
    let mut network = advertised(9000, Some([192, 168, 1, 9]));

    let windows = run(&mut network);

    assert_eq!(
        windows.source_calls(),
        [
            SourceCall::AskedIfItWasAskedToStop,
            SourceCall::CheckedForASinkToLookFor,
            SourceCall::TriedFixedAddress,
            SourceCall::BoundDiscovery,
            SourceCall::SetDiscoveryTtl,
            SourceCall::StartedDiscoveryClock,
            SourceCall::SentDiscoveryQuestion,
            SourceCall::ReadDiscoveryClock,
            SourceCall::SetDiscoveryTimeout,
            SourceCall::ReceivedDiscoveryAnswer,
            SourceCall::ResolvedDiscoveryAnswer,
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
fn a_failed_discovery_operation_stops_before_the_next_one() {
    for (failed, expected) in [
        (
            SourceCall::BoundDiscovery,
            &[
                SourceCall::AskedIfItWasAskedToStop,
                SourceCall::CheckedForASinkToLookFor,
                SourceCall::TriedFixedAddress,
                SourceCall::BoundDiscovery,
            ][..],
        ),
        (
            SourceCall::SetDiscoveryTtl,
            &[
                SourceCall::AskedIfItWasAskedToStop,
                SourceCall::CheckedForASinkToLookFor,
                SourceCall::TriedFixedAddress,
                SourceCall::BoundDiscovery,
                SourceCall::SetDiscoveryTtl,
            ][..],
        ),
        (
            SourceCall::SentDiscoveryQuestion,
            &[
                SourceCall::AskedIfItWasAskedToStop,
                SourceCall::CheckedForASinkToLookFor,
                SourceCall::TriedFixedAddress,
                SourceCall::BoundDiscovery,
                SourceCall::SetDiscoveryTtl,
                SourceCall::StartedDiscoveryClock,
                SourceCall::SentDiscoveryQuestion,
            ][..],
        ),
        (
            SourceCall::SetDiscoveryTimeout,
            &[
                SourceCall::AskedIfItWasAskedToStop,
                SourceCall::CheckedForASinkToLookFor,
                SourceCall::TriedFixedAddress,
                SourceCall::BoundDiscovery,
                SourceCall::SetDiscoveryTtl,
                SourceCall::StartedDiscoveryClock,
                SourceCall::SentDiscoveryQuestion,
                SourceCall::ReadDiscoveryClock,
                SourceCall::SetDiscoveryTimeout,
            ][..],
        ),
        (
            SourceCall::ReceivedDiscoveryAnswer,
            &[
                SourceCall::AskedIfItWasAskedToStop,
                SourceCall::CheckedForASinkToLookFor,
                SourceCall::TriedFixedAddress,
                SourceCall::BoundDiscovery,
                SourceCall::SetDiscoveryTtl,
                SourceCall::StartedDiscoveryClock,
                SourceCall::SentDiscoveryQuestion,
                SourceCall::ReadDiscoveryClock,
                SourceCall::SetDiscoveryTimeout,
                SourceCall::ReceivedDiscoveryAnswer,
            ][..],
        ),
        (
            SourceCall::ResolvedDiscoveryAnswer,
            &[
                SourceCall::AskedIfItWasAskedToStop,
                SourceCall::CheckedForASinkToLookFor,
                SourceCall::TriedFixedAddress,
                SourceCall::BoundDiscovery,
                SourceCall::SetDiscoveryTtl,
                SourceCall::StartedDiscoveryClock,
                SourceCall::SentDiscoveryQuestion,
                SourceCall::ReadDiscoveryClock,
                SourceCall::SetDiscoveryTimeout,
                SourceCall::ReceivedDiscoveryAnswer,
                SourceCall::ResolvedDiscoveryAnswer,
            ][..],
        ),
    ] {
        let mut network = advertised(9000, Some([192, 168, 1, 9]));
        let mut windows = SimSource::new();
        windows.asked_for(Driving::TheSink);
        windows.pinned_to(identity(SINK));
        windows.run_for(Duration::from_secs(2));
        windows.on_a_network(network.clone());
        windows.whose_link_fails_at(failed);

        source::run(&Request::Relaying, false, &mut windows, None);

        assert_eq!(&windows.source_calls()[..expected.len()], expected);
        assert_eq!(
            windows.source_calls().get(expected.len()),
            Some(&SourceCall::AskedIfItWasAskedToStop)
        );
        network = windows.network().clone();
        let question_reached_the_network = expected.contains(&SourceCall::SentDiscoveryQuestion)
            && failed != SourceCall::SentDiscoveryQuestion;
        assert_eq!(
            network.asked().len(),
            usize::from(question_reached_the_network)
        );
    }
}

#[test]
fn an_address_in_the_answer_is_taken_rather_than_looked_up_again() {
    // A responder that sent the address along has saved a lookup, and the lookup it
    // saved is one that can fail on its own.
    let mut network = advertised(9000, Some([192, 168, 1, 9]));

    assert_eq!(
        look(&mut network).and_then(|found| found.address),
        Some([192, 168, 1, 9])
    );
}

#[test]
fn somebody_elses_service_is_read_past_rather_than_stopped_at() {
    // A desk on an office network hears printers and AirPlay receivers first.
    // Giving up on the first answer would be giving up on the sink, and connecting
    // to one would be typing into a printer.
    let mut network = SimDiscovery::new();
    network.advertises_service("_ipp._tcp.local", "a printer", 631, None);
    network.advertises(SERVICE, INSTANCE, 4242, None);

    assert_eq!(look(&mut network).map(|found| found.port), Some(4242));
}

#[test]
fn a_machine_that_is_only_showing_a_pairing_code_is_not_something_to_relay_to() {
    // The two names are what keep these apart (ADR-0012). The pairing port is
    // waiting for an offer, so a relaying run that took it would speak a handshake
    // at a listener that cannot answer one — and the same Mac offers both at once
    // while a code is up.
    let mut network = SimDiscovery::new();
    network.advertises(PAIRING, INSTANCE, 51763, None);

    assert_eq!(look(&mut network), None);
}

#[test]
fn an_irrelevant_answer_is_read_past_until_the_lookup_times_out() {
    let mut network = SimDiscovery::new();
    network.advertises(PAIRING, INSTANCE, 51763, None);

    let windows = run(&mut network);

    assert_eq!(
        windows.source_calls(),
        [
            SourceCall::AskedIfItWasAskedToStop,
            SourceCall::CheckedForASinkToLookFor,
            SourceCall::TriedFixedAddress,
            SourceCall::BoundDiscovery,
            SourceCall::SetDiscoveryTtl,
            SourceCall::StartedDiscoveryClock,
            SourceCall::SentDiscoveryQuestion,
            SourceCall::ReadDiscoveryClock,
            SourceCall::SetDiscoveryTimeout,
            SourceCall::ReceivedDiscoveryAnswer,
            SourceCall::ReadDiscoveryClock,
            SourceCall::SetDiscoveryTimeout,
            SourceCall::ReceivedDiscoveryAnswer,
            // The round after the lookup timed out, where the bound the run was
            // given has now passed: asked first, and nothing about a sink is
            // asked at all once it answers (ADR-0006).
            SourceCall::AskedIfItWasAskedToStop,
            SourceCall::AskedIfItWasAskedToStop,
        ]
    );
}

#[test]
fn an_answer_with_no_port_in_it_is_not_the_sink() {
    // A responder that named the instance and nothing else. Read as an answer it
    // would be a machine to connect to on a port nobody said.
    let mut network = SimDiscovery::new();
    network.advertises_without_a_port(SERVICE, INSTANCE);

    assert_eq!(look(&mut network), None);
}

#[test]
fn a_name_that_points_at_itself_is_refused_rather_than_followed() {
    // Two bytes from the network are all it takes to write, and a reader that
    // followed it would never come back — which is a source that never relays
    // again, on a machine whose keyboards it is holding.
    let mut network = SimDiscovery::new();
    network.answers_with_a_name_that_points_at_itself();

    assert_eq!(look(&mut network), None);
}

#[test]
fn a_truncated_answer_is_refused_at_every_length() {
    // What a datagram cut short leaves behind. Every prefix, so nothing is answered
    // off a record that was only half read: the port is two bytes, and half of it
    // is a number.
    let whole = advertised(1234, Some([127, 0, 0, 1]));
    let message = whole.advertisement().to_vec();

    for length in 0..message.len() {
        let mut network = SimDiscovery::new();
        network.answers_with(message[..length].to_vec());
        assert_eq!(
            look(&mut network),
            None,
            "{length} bytes of an answer is not an answer"
        );
    }
}

#[test]
fn a_network_that_answers_nothing_is_not_a_failure() {
    // The ordinary state of a machine that has not been switched on yet, and the
    // one the source waits through rather than stopping for.
    let mut network = SimDiscovery::new();

    assert_eq!(look(&mut network), None);
    assert_eq!(network.asked().len(), 1, "it still asked");
}
