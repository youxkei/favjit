//! Pairing: a six-digit code this machine shows, and the keys that cross under it.
//!
//! ADR-0004 puts the code on the machine being controlled and makes it single-use.
//! Both halves are `engine`'s: the sink asks its machine for bytes, shows the code it
//! made of them, and serves exactly one attempt — so a wrong code is not something an
//! attacker can walk through, and that is what makes six digits enough.
//!
//! The exchange runs for real. The source in front of the machine is played with the
//! same functions its own run would call, so a wrong code fails to open because the
//! arithmetic says so. What the simulated machine stands in for is the machine: the
//! screen, the socket, the file, and the bytes (ADR-0007).

use favjit_engine::pairing::{self, Identity, Paired};
use favjit_host_sim::{PairingCall, SimPairing};

/// The key the source presents once the code has been answered.
fn source_key() -> Vec<u8> {
    vec![0x51; pairing::KEY]
}

/// This machine's identity, which the run is given rather than asking its machine
/// for: what pairing does with it is send the public half.
fn mine() -> Identity {
    Identity::new(vec![0xbb; pairing::KEY], vec![0xbc; pairing::KEY]).expect("two halves")
}

/// A key as the hex the pairing text carries it — the same digits `Paired::Pinned`
/// names, checked against here rather than assumed to look like anything.
fn hex(bytes: &[u8]) -> String {
    use core::fmt::Write;
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// Whether the file this machine wrote names that key.
///
/// Read here rather than through `engine`'s own reader: what the file says is
/// what this asserts about, and a check that parsed it with the same code that
/// wrote it would pass on a file no other machine could read.
fn names(text: &str, key: &[u8]) -> bool {
    text.lines().any(|line| line.trim() == hex(key))
}

#[test]
fn the_right_code_pins_the_source() {
    // The whole of what pairing is for: after it, the converting run accepts input
    // from that machine and from no other.
    let mut mac = SimPairing::new().with_a_source_that_knows_the_code(&source_key());

    assert_eq!(
        pairing::pair(&mine(), &mut mac),
        Paired::Pinned(hex(&source_key()))
    );
    assert!(names(&mac.authorized_text(), &source_key()));
}

#[test]
fn every_host_operation_is_ordered_by_the_pairing_run() {
    let mut mac = SimPairing::new().with_a_source_that_knows_the_code(&source_key());

    pairing::pair(&mine(), &mut mac);

    assert_eq!(
        mac.pairing_calls(),
        [
            PairingCall::BoundListener,
            PairingCall::ReadListenerPort,
            PairingCall::Advertised,
            PairingCall::ShowedCode,
            PairingCall::ShowedInstruction,
            PairingCall::SetBlocking,
            PairingCall::AcceptedSource,
            PairingCall::SetReadTimeout,
            PairingCall::SetWriteTimeout,
            PairingCall::TookOffer,
            PairingCall::SentAnswer,
            PairingCall::FlushedAnswer,
            PairingCall::TookSealedKey,
            PairingCall::SentSealedKey,
            PairingCall::FlushedSealedKey,
            PairingCall::ReadAuthorized,
            PairingCall::MadeAuthorizedDirectory,
            PairingCall::Authorized,
        ]
    );
}

#[test]
fn a_failed_host_operation_stops_before_the_next_one() {
    let stopping = [
        PairingCall::BoundListener,
        PairingCall::ReadListenerPort,
        PairingCall::SetBlocking,
        PairingCall::AcceptedSource,
        PairingCall::SetReadTimeout,
        PairingCall::SetWriteTimeout,
        PairingCall::SentAnswer,
        PairingCall::FlushedAnswer,
        PairingCall::TookSealedKey,
        PairingCall::SentSealedKey,
        PairingCall::FlushedSealedKey,
        PairingCall::MadeAuthorizedDirectory,
        PairingCall::Authorized,
    ];

    for failed in stopping {
        let mut mac = SimPairing::new()
            .with_a_source_that_knows_the_code(&source_key())
            .whose_pairing_fails_at(failed);

        pairing::pair(&mine(), &mut mac);

        if failed == PairingCall::ReadListenerPort {
            assert_eq!(
                mac.pairing_calls(),
                [PairingCall::BoundListener, PairingCall::ReadListenerPort]
            );
        } else {
            assert_eq!(mac.pairing_calls().last(), Some(&failed));
        }
        assert!(mac.authorized_text().trim().is_empty());
    }
}

#[test]
fn advertisement_failure_still_shows_the_code_and_serves_the_listener() {
    let mut mac = SimPairing::new()
        .with_a_source_that_knows_the_code(&source_key())
        .whose_pairing_fails_at(PairingCall::Advertised);

    assert_eq!(
        pairing::pair(&mine(), &mut mac),
        Paired::Pinned(hex(&source_key()))
    );
    assert!(mac.pairing_calls().contains(&PairingCall::AcceptedSource));
}

#[test]
fn the_two_lines_are_formatted_and_written_in_order_by_the_pairing_run() {
    let mut mac = SimPairing::new();

    pairing::pair(&mine(), &mut mac);

    let shown = mac.shown().expect("a code was shown");
    let digits = core::str::from_utf8(&shown).expect("the code is digits");
    assert_eq!(
        mac.shown_lines(),
        [
            format!("pairing code: {digits}"),
            "type it on the other machine within a minute: favjit --pair <those digits>"
                .to_string(),
        ]
    );
}

#[test]
fn a_wrong_code_pins_nothing() {
    // ADR-0004's default reaching the pairing step: what cannot be opened is not
    // trusted, and nothing is written down.
    let mut mac = SimPairing::new().with_a_source_that_has_the_code_wrong(&source_key());

    assert_eq!(pairing::pair(&mine(), &mut mac), Paired::WrongCode);
    assert!(!names(&mac.authorized_text(), &source_key()));
    assert!(mac.authorized_text().trim().is_empty());
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
    assert!(mac.authorized_text().trim().is_empty());
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
    assert!(mac.authorized_text().trim().is_empty());
}

#[test]
fn a_source_that_goes_quiet_part_way_through_pins_nothing_wherever_it_stops() {
    // A cable pulled, a machine shut down, a network that dropped: the exchange
    // has four peer exchanges and any of them can be the last. Every one of them has
    // to end as an interruption and write nothing down — told apart from a wrong
    // code, which is the answer that means *this* machine will not do, and from
    // "no source", which means nobody came.
    for stops_after in 0..4 {
        let mut mac = SimPairing::new()
            .with_a_source_that_knows_the_code(&source_key())
            .that_goes_quiet_after(stops_after);

        assert_eq!(
            pairing::pair(&mine(), &mut mac),
            Paired::Interrupted,
            "a source that carried {stops_after} of the exchange's calls"
        );
        assert!(
            mac.authorized_text().trim().is_empty(),
            "and nothing is written down"
        );
    }
}

#[test]
fn a_source_whose_offer_cannot_be_read_pins_nothing() {
    // Bytes of the right length that the exchange cannot read: a machine speaking
    // a different version of it, or something else on the port. It ends as an
    // interruption rather than a wrong code, because a wrong code is a thing the
    // person can fix by reading the screen again and this is not.
    let mut mac = SimPairing::new()
        .with_a_source_that_knows_the_code(&source_key())
        .with_a_source_that_offers_nonsense();

    assert_eq!(pairing::pair(&mine(), &mut mac), Paired::Interrupted);
    assert!(mac.authorized_text().trim().is_empty());
}

#[test]
fn a_machine_that_cannot_write_the_list_says_so_rather_than_claiming_a_pair() {
    // The exchange worked and the key cannot be kept — a read-only disk, or a
    // directory that is not there. Saying `Pinned` here would leave a person
    // believing the two machines are paired, and the next converting run would
    // refuse the source with nothing to explain it.
    let mut mac = SimPairing::new()
        .with_a_source_that_knows_the_code(&source_key())
        .that_cannot_keep_the_list();

    assert!(
        matches!(pairing::pair(&mine(), &mut mac), Paired::CannotKeep(_)),
        "a list that cannot be kept is not a pairing"
    );
}

#[test]
fn a_list_a_person_left_without_a_final_newline_does_not_join_two_keys() {
    // The file is a person's to edit, so it can end mid-line. A key appended
    // straight onto the end of another is neither of them, and the run that reads
    // the list back would refuse both machines.
    let existing = hex(&[0x11; pairing::KEY]);
    let mut mac = SimPairing::new()
        .with_a_source_that_knows_the_code(&source_key())
        .holding(&existing);

    assert_eq!(
        pairing::pair(&mine(), &mut mac),
        Paired::Pinned(hex(&source_key()))
    );
    assert_eq!(
        mac.authorized_text().lines().collect::<Vec<_>>(),
        vec![existing.as_str(), hex(&source_key()).as_str()],
        "one key per line, both of them whole"
    );
}
