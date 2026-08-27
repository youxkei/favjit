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
//! here as a key that will not open (ADR-0017).
//!
//! The two runs go one after the other rather than at once, because that is what a
//! suite without threads can do (ADR-0007) — and what is being checked is what each
//! end ended up with, which does not depend on how the two were interleaved.

use favjit_core::pairing::{self, Code, Identity, Paired};
use favjit_host_sim::{SimPairing, SimSourcePairing};

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

    assert_eq!(
        sink_ending,
        Paired::Pinned(pairing::hex(windows_key.public()))
    );
    assert_eq!(
        source_ending,
        Paired::Pinned(pairing::hex(mac_key.public())),
        "the source pinned the key the Mac presented"
    );
    assert!(
        mac.authorized().holds(windows_key.public()),
        "the Mac will accept input from the machine that paired"
    );
    assert_eq!(
        windows.pinned().as_deref(),
        Some(mac_key.public()),
        "and that machine will send input to this Mac and to no other"
    );
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
        mac.authorized().is_empty(),
        "nothing is written down for a code that did not open"
    );
    assert_eq!(
        windows.pinned(),
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
    assert!(mac.authorized().is_empty());
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
