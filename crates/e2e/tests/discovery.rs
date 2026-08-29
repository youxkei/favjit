//! How the forwarding machine finds the machine it relays to.
//!
//! The sink binds whatever port it is given and says where it is, so finding it is
//! not a name lookup: what the source needs back is a port, and a machine that
//! answered without one is not somewhere input can be sent. Which answers are the
//! sink, and how many are read before giving up, are decisions — so they are
//! `core`'s and driven here, and the socket is the host's (ADR-0006).

use favjit_core::discovery::Found;
use favjit_core::source::{self, Request};
use favjit_host_sim::{SimDiscovery, SimSource};

/// The instance name a sink registers under, which is a person's machine name.
const INSTANCE: &str = "the mac";

fn advertised(port: u16, address: Option<[u8; 4]>) -> SimDiscovery {
    let mut network = SimDiscovery::new();
    network.advertises(INSTANCE, port, address);
    network
}

/// A whole forwarding run on a machine that can see this network.
///
/// Through `source::run` and not through the lookup, because the lookup is only
/// reached by a run that is relaying: called directly it would pass while nothing
/// wired it up.
fn look(network: &mut SimDiscovery) -> Option<Found> {
    let mut windows = SimSource::new();
    windows.on_a_network(network.clone());
    source::run(&Request::Relaying, &mut windows);
    *network = windows.network().clone();
    windows.found().cloned()
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
    network.advertises(INSTANCE, 4242, None);

    assert_eq!(look(&mut network).map(|found| found.port), Some(4242));
}

#[test]
fn a_machine_that_is_only_showing_a_pairing_code_is_not_something_to_relay_to() {
    // The two names are what keep these apart (ADR-0017). The pairing port is
    // waiting for an offer, so a relaying run that took it would speak a handshake
    // at a listener that cannot answer one — and the same Mac offers both at once
    // while a code is up.
    let mut network = SimDiscovery::new();
    network.advertises_pairing(INSTANCE, 51763, None);

    assert_eq!(look(&mut network), None);
}

#[test]
fn an_answer_with_no_port_in_it_is_not_the_sink() {
    // A responder that named the instance and nothing else. Read as an answer it
    // would be a machine to connect to on a port nobody said.
    let mut network = SimDiscovery::new();
    network.advertises_without_a_port(INSTANCE);

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
