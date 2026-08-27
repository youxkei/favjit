//! Pairing with both machines in it, each driven by its own sequence.
//!
//! The two suites beside this one drive one end each, against a machine scripted with
//! what the other end would do. What they cannot show is that the two ends agree:
//! each is checked against an idea of its peer rather than against the peer's own
//! code. So the runs here are both of them — the sink's `pair` and the source's
//! `pair_with` — with the digits one showed carried to the other, and the assertions
//! are on the two of them together.
//!
//! The exchange runs for real at both ends, which is what makes this the strongest of
//! the three: the peer each machine is scripted with is played by the same functions
//! the peer's own run calls, so a construction the two ends disagreed about — the
//! string the exchange binds to, the nonce a direction seals under — would show up
//! here as a key that will not open (ADR-0012).
//!
//! The two runs go one after the other rather than at once, because that is what a
//! suite without threads can do (ADR-0007) — and what is being checked is what each
//! end ended up with, which does not depend on how the two were interleaved.

use favjit_engine::pairing::{self, Code, Identity, Paired};
use favjit_host_sim::{SimPairing, SimSourcePairing, SourcePairingCall};

/// The two machines' identities, as each run is given its own.
fn the_macs() -> Identity {
    Identity::new(vec![0xbb; pairing::KEY], vec![0xbc; pairing::KEY]).expect("two halves")
}

fn the_windows_machines() -> Identity {
    Identity::new(vec![0x51; pairing::KEY], vec![0x52; pairing::KEY]).expect("two halves")
}

/// Digits that are not the ones the Mac showed.
///
/// Built from the code rather than written down, so that a change to how a code is
/// produced cannot turn this into the right one by accident.
fn not(code: Code) -> Code {
    let mut wrong = code;
    wrong[0] = match code[0] {
        b'0' => b'1',
        _ => b'0',
    };
    wrong
}

fn hex(bytes: &[u8]) -> String {
    use core::fmt::Write;
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// Whether the file the Mac wrote names that key.
///
/// Read here rather than through `engine`'s own reader: what the file says is
/// what this asserts about, and a check that parsed it with the same code that
/// wrote it would pass on a file no other machine could read.
fn names(text: &str, key: &[u8]) -> bool {
    text.lines().any(|line| line.trim() == hex(key))
}

#[test]
fn each_machine_ends_holding_the_other_s_key() {
    // The whole of what the exchange is for. Either half alone can look right while
    // holding a key its peer never presented, and this is the assertion that says
    // they are the same two keys.
    let mac_key = the_macs();
    let windows_key = the_windows_machines();
    let mut mac = SimPairing::new().with_a_source_that_knows_the_code(windows_key.public());
    let sink_ending = pairing::pair(&mac_key, &mut mac);

    // The digits the Mac showed rather than any written here: they are what the
    // person carries from one screen to the other, and a run that showed something
    // else would leave this end with nothing to open.
    let code = mac.shown().expect("the Mac showed a code");
    let mut windows = SimSourcePairing::new().with_a_sink_that_knows(code, mac_key.public());
    let source_ending = pairing::pair_with(code, &windows_key, &mut windows);

    assert_eq!(sink_ending, Paired::Pinned(hex(windows_key.public())));
    assert_eq!(
        source_ending,
        Paired::Pinned(hex(mac_key.public())),
        "the source pinned the key the Mac presented"
    );
    assert!(
        names(&mac.authorized_text(), windows_key.public()),
        "the Mac will accept input from the machine that paired"
    );
    assert!(
        names(&windows.pinned_text().unwrap_or_default(), mac_key.public()),
        "and that machine will send input to this Mac and to no other"
    );
}

#[test]
fn every_source_host_operation_is_ordered_by_the_pairing_run() {
    let mac_key = the_macs();
    let windows_key = the_windows_machines();
    let mut mac = SimPairing::new().with_a_source_that_knows_the_code(windows_key.public());
    pairing::pair(&mac_key, &mut mac);
    let code = mac.shown().expect("the Mac showed a code");
    let mut windows = SimSourcePairing::new().with_a_sink_that_knows(code, mac_key.public());

    pairing::pair_with(code, &windows_key, &mut windows);

    assert_eq!(
        windows.pairing_calls(),
        [
            SourcePairingCall::Connected,
            SourcePairingCall::SetReadTimeout,
            SourcePairingCall::SetWriteTimeout,
            SourcePairingCall::SentOffer,
            SourcePairingCall::FlushedOffer,
            SourcePairingCall::TookAnswer,
            SourcePairingCall::SentSealedKey,
            SourcePairingCall::FlushedSealedKey,
            SourcePairingCall::TookSealedKey,
            SourcePairingCall::MadeSinkDirectory,
            SourcePairingCall::PinnedSink,
        ]
    );
}

#[test]
fn a_failed_source_host_operation_stops_before_the_next_one() {
    let mac_key = the_macs();
    let windows_key = the_windows_machines();
    let mut mac = SimPairing::new().with_a_source_that_knows_the_code(windows_key.public());
    pairing::pair(&mac_key, &mut mac);
    let code = mac.shown().expect("the Mac showed a code");
    let sequence = [
        SourcePairingCall::Connected,
        SourcePairingCall::SetReadTimeout,
        SourcePairingCall::SetWriteTimeout,
        SourcePairingCall::SentOffer,
        SourcePairingCall::FlushedOffer,
        SourcePairingCall::TookAnswer,
        SourcePairingCall::SentSealedKey,
        SourcePairingCall::FlushedSealedKey,
        SourcePairingCall::TookSealedKey,
        SourcePairingCall::MadeSinkDirectory,
        SourcePairingCall::PinnedSink,
    ];

    for failed in sequence {
        let mut windows = SimSourcePairing::new()
            .with_a_sink_that_knows(code, mac_key.public())
            .whose_pairing_fails_at(failed);

        pairing::pair_with(code, &windows_key, &mut windows);

        assert_eq!(windows.pairing_calls().last(), Some(&failed));
        assert_eq!(windows.pinned_text(), None);
    }
}

#[test]
fn the_digits_the_mac_showed_are_the_ones_that_work() {
    // Six digits are only enough because the code is the code: a source given other
    // digits has to end with nothing, and the attempt it spent is gone.
    let mac_key = the_macs();
    let windows_key = the_windows_machines();
    let mut mac = SimPairing::new().with_a_source_that_has_the_code_wrong(windows_key.public());
    let sink_ending = pairing::pair(&mac_key, &mut mac);
    let code = mac.shown().expect("the Mac showed a code");

    let mut windows = SimSourcePairing::new().with_a_sink_that_knows(code, mac_key.public());
    let source_ending = pairing::pair_with(not(code), &windows_key, &mut windows);

    assert_eq!(sink_ending, Paired::WrongCode);
    assert_eq!(source_ending, Paired::WrongCode);
    assert!(
        mac.authorized_text().trim().is_empty(),
        "nothing is written down for a code that did not open"
    );
    assert_eq!(
        windows.pinned_text(),
        None,
        "and nothing is pinned at the source"
    );
}

#[test]
fn the_attempt_is_spent_on_the_machine_that_got_it_wrong() {
    // What makes the code single-use, seen from both ends: the Mac serves the one
    // attempt and stops, so the machine that then types the right digits has nothing
    // to reach — its answer has to be a fresh code rather than another guess.
    let windows_key = the_windows_machines();
    let mut mac = SimPairing::new()
        .with_a_source_that_has_the_code_wrong(windows_key.public())
        .and_then_a_source_that_knows_it(windows_key.public());

    assert_eq!(pairing::pair(&the_macs(), &mut mac), Paired::WrongCode);
    assert_eq!(mac.attempts(), 1, "the second source was never served");
    assert!(mac.authorized_text().trim().is_empty());
}

#[test]
fn a_sink_that_goes_quiet_part_way_through_pins_nothing_wherever_it_stops() {
    // The other end of `pairing.rs`'s own interruption: this end speaks first, so
    // the four peer exchanges are the offer out, the answer back, its key out and
    // the sink's key back. Any of them can be where the connection goes, and none
    // of them may leave a sink pinned — a machine sending input to one it never
    // finished agreeing with is sending it into a session that cannot open.
    let mac_key = the_macs();
    let windows_key = the_windows_machines();
    let mut mac = SimPairing::new().with_a_source_that_knows_the_code(windows_key.public());
    pairing::pair(&mac_key, &mut mac);
    let code = mac.shown().expect("the Mac showed a code");

    for stops_after in 0..4 {
        let mut windows = SimSourcePairing::new()
            .with_a_sink_that_knows(code, mac_key.public())
            .that_goes_quiet_after(stops_after);

        assert_eq!(
            pairing::pair_with(code, &windows_key, &mut windows),
            Paired::Interrupted,
            "a sink that carried {stops_after} of the exchange's calls"
        );
        assert_eq!(windows.pinned_text(), None, "and nothing is pinned");
    }
}

#[test]
fn a_machine_that_cannot_produce_its_half_of_the_exchange_pairs_nothing() {
    // The digits came from the other machine, and this end still needs bytes of
    // its own: what it offers is a keypair made for the one exchange. A machine
    // whose randomness will not answer has nothing to offer, and pinning the sink
    // anyway would be pinning it under a secret nobody agreed.
    let mac_key = the_macs();
    let windows_key = the_windows_machines();
    let mut mac = SimPairing::new().with_a_source_that_knows_the_code(windows_key.public());
    pairing::pair(&mac_key, &mut mac);
    let code = mac.shown().expect("the Mac showed a code");

    let mut windows = SimSourcePairing::new()
        .with_a_sink_that_knows(code, mac_key.public())
        .with_no_entropy();

    assert_eq!(
        pairing::pair_with(code, &windows_key, &mut windows),
        Paired::Interrupted
    );
    assert_eq!(windows.pinned_text(), None);
}

#[test]
fn a_sink_whose_answer_cannot_be_read_pins_nothing() {
    // The other end of `pairing.rs`'s unreadable offer. Bytes of the right length
    // that this end cannot finish the exchange with: told apart from a wrong code,
    // which is the answer that says the digits were the problem.
    let mac_key = the_macs();
    let windows_key = the_windows_machines();
    let mut mac = SimPairing::new().with_a_source_that_knows_the_code(windows_key.public());
    pairing::pair(&mac_key, &mut mac);
    let code = mac.shown().expect("the Mac showed a code");

    let mut windows = SimSourcePairing::new()
        .with_a_sink_that_knows(code, mac_key.public())
        .with_a_sink_that_answers_with_nonsense();

    assert_eq!(
        pairing::pair_with(code, &windows_key, &mut windows),
        Paired::Interrupted
    );
    assert_eq!(windows.pinned_text(), None);
}

#[test]
fn a_source_that_cannot_write_the_sink_down_says_so() {
    // The exchange worked and the key cannot be kept. Saying `Pinned` would leave
    // a person believing this machine will send input to that Mac, where the next
    // run has nowhere to send it and no reason to give.
    let mac_key = the_macs();
    let windows_key = the_windows_machines();
    let mut mac = SimPairing::new().with_a_source_that_knows_the_code(windows_key.public());
    pairing::pair(&mac_key, &mut mac);
    let code = mac.shown().expect("the Mac showed a code");

    let mut windows = SimSourcePairing::new()
        .with_a_sink_that_knows(code, mac_key.public())
        .that_cannot_keep_the_sink();

    assert!(
        matches!(
            pairing::pair_with(code, &windows_key, &mut windows),
            Paired::CannotKeep(_)
        ),
        "a sink that cannot be written down is not a pairing"
    );
    assert_eq!(windows.pinned_text(), None);
}

#[test]
fn neither_end_writes_anything_down_before_the_other_s_key_has_arrived() {
    // The order both halves rest on. A machine that wrote the key down first would
    // have authorised whoever connected, which is ADR-0004's default inverted — and
    // one that waited before showing its code would be waiting for digits nobody can
    // read yet.
    let mac_key = the_macs();
    let windows_key = the_windows_machines();
    let mut mac = SimPairing::new().with_a_source_that_knows_the_code(windows_key.public());
    pairing::pair(&mac_key, &mut mac);
    let code = mac.shown().expect("the Mac showed a code");

    let mut windows = SimSourcePairing::new().with_a_sink_that_knows(code, mac_key.public());
    pairing::pair_with(code, &windows_key, &mut windows);

    assert!(mac.showed_before_waiting());
    assert!(mac.took_the_key_before_authorizing());
    assert!(windows.offered_before_waiting());
    assert!(windows.took_the_key_before_pinning());
}
