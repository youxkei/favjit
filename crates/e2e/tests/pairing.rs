//! Pairing: a six-digit code this machine shows, and the keys that cross under it.
//!
//! ADR-0004 puts the code on the machine being controlled and makes it single-use.
//! Both halves are `core`'s: the sink asks its machine for bytes, shows the code it
//! made of them, and serves exactly one attempt — so a wrong code is not something an
//! attacker can walk through, and that is what makes six digits enough.
//!
//! The exchange runs for real. The source in front of the machine is played with the
//! same functions its own run would call, so a wrong code fails to open because the
//! arithmetic says so. What the simulated machine stands in for is the machine: the
//! screen, the socket, the file, and the bytes (ADR-0007).

use favjit_core::pairing::{self, Identity, Paired};
use favjit_host_sim::SimPairing;

/// The key the source presents once the code has been answered.
fn source_key() -> Vec<u8> {
    vec![0x51; pairing::KEY]
}

/// This machine's identity, which the run is given rather than asking its machine
/// for: what pairing does with it is send the public half.
fn mine() -> Identity {
    Identity::new(vec![0xbb; pairing::KEY], vec![0xbc; pairing::KEY]).expect("two halves")
}

#[test]
fn the_right_code_pins_the_source() {
    // The whole of what pairing is for: after it, the converting run accepts input
    // from that machine and from no other.
    let mut mac = SimPairing::new().with_a_source_that_knows_the_code(&source_key());

    assert_eq!(
        pairing::pair(&mine(), &mut mac),
        Paired::Pinned(pairing::hex(&source_key()))
    );
    assert!(mac.authorized().holds(&source_key()));
}

#[test]
fn a_wrong_code_pins_nothing() {
    // ADR-0004's default reaching the pairing step: what cannot be opened is not
    // trusted, and nothing is written down.
    let mut mac = SimPairing::new().with_a_source_that_has_the_code_wrong(&source_key());

    assert_eq!(pairing::pair(&mine(), &mut mac), Paired::WrongCode);
    assert!(!mac.authorized().holds(&source_key()));
    assert_eq!(mac.authorized().len(), 0);
}

#[test]
fn the_code_is_spent_on_the_first_attempt_whether_it_worked_or_not() {
    // The reason six digits is enough. A run that served a second attempt against
    // the same code would let an attacker walk the space at whatever rate this
    // machine accepts connections.
    let mut mac = SimPairing::new()
        .with_a_source_that_has_the_code_wrong(&source_key())
        .and_then_a_source_that_knows_it(&source_key());

    assert_eq!(pairing::pair(&mine(), &mut mac), Paired::WrongCode);
    assert_eq!(mac.attempts(), 1, "the second source was never served");
    assert_eq!(mac.authorized().len(), 0);
}

#[test]
fn the_source_is_given_this_machines_key_as_well() {
    // Both directions, because the source has to pin this machine too: it is the
    // end that opens the session, and it cannot address a machine whose key it does
    // not hold.
    let mine = mine();
    let mut mac = SimPairing::new().with_a_source_that_knows_the_code(&source_key());

    pairing::pair(&mine, &mut mac);

    assert_eq!(mac.gave_the_source(), Some(mine.public().to_vec()));
}

#[test]
fn a_machine_that_cannot_make_a_code_shows_none_and_waits_for_nobody() {
    // Nothing to display is nothing to pair against, and the run says so rather
    // than opening a socket that would accept an attempt no code protects.
    let mut mac = SimPairing::new().with_no_code();

    assert_eq!(pairing::pair(&mine(), &mut mac), Paired::NoCode);
    assert_eq!(mac.shown(), None);
    assert_eq!(mac.attempts(), 0);
}

#[test]
fn the_code_is_shown_before_anything_is_waited_for() {
    // The order is the point: a code produced after a source has connected is a
    // code nobody could have entered.
    let mut mac = SimPairing::new().with_a_source_that_knows_the_code(&source_key());

    pairing::pair(&mine(), &mut mac);

    assert!(
        mac.showed_before_waiting(),
        "waited for a source before showing the code"
    );
}

#[test]
fn nothing_is_pinned_before_the_sources_key_has_arrived() {
    // Pinning is what the exchange is for, so it comes last: a key written down
    // before the source had sent one is a key the code did not vouch for, and what
    // opened it is the secret the code agreed.
    let mut mac = SimPairing::new().with_a_source_that_knows_the_code(&source_key());

    pairing::pair(&mine(), &mut mac);

    assert!(
        mac.took_the_key_before_authorizing(),
        "pinned a key before the source had sent one"
    );
}

#[test]
fn a_source_that_never_connects_pairs_nothing() {
    // The ordinary state of a machine whose other end has not been started: the
    // code was shown, nobody answered it, and nothing is written down.
    let mut mac = SimPairing::new();

    assert_eq!(pairing::pair(&mine(), &mut mac), Paired::NoSource);
    assert!(mac.shown().is_some());
    assert_eq!(mac.authorized().len(), 0);
}
