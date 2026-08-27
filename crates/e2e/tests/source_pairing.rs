//! The forwarding machine's half of pairing: a code entered here, a sink pinned.
//!
//! The mirror of `pairing.rs`. The sink shows the code and waits; this end is given
//! the code and connects, so the order is reversed — the offer goes out before an
//! answer comes back, and this machine's key goes out before the sink's arrives.
//!
//! What is pinned is one key and not a list (ADR-0004): a sink decides which sources
//! may type on it and can have several, and a source has exactly one machine it is
//! willing to hand its keyboard to.

use favjit_engine::pairing::{self, Identity, Paired};
use favjit_engine::source;
use favjit_host_sim::{SimSource, SimSourcePairing};

/// The code the person read off the Mac's screen.
const CODE: pairing::Code = *b"481502";

fn sink_key() -> Vec<u8> {
    vec![0x5a; pairing::KEY]
}

/// This machine's identity, which the run is given rather than asking its machine
/// for: what pairing does with it is send the public half.
fn mine() -> Identity {
    Identity::new(vec![0xcc; pairing::KEY], vec![0xcd; pairing::KEY]).expect("two halves")
}

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
/// wrote it would pass on a file the other machine could not read.
fn names(text: &str, key: &[u8]) -> bool {
    text.lines().any(|line| line.trim() == hex(key))
}

#[test]
fn the_right_code_pins_the_sink() {
    // After this, the relaying run has somewhere to send input and a key to open the
    // session with.
    let mut windows = SimSourcePairing::new().with_a_sink_that_knows(CODE, &sink_key());

    assert_eq!(
        pairing::pair_with(CODE, &mine(), &mut windows),
        Paired::Pinned(hex(&sink_key()))
    );
    assert!(names(
        &windows.pinned_text().unwrap_or_default(),
        &sink_key()
    ));
}

#[test]
fn a_code_the_sink_does_not_agree_with_pins_nothing() {
    // The same refusal as the other end's, arrived at the same way: what will not
    // open under this code is not written down.
    let mut windows = SimSourcePairing::new().with_a_sink_that_knows(*b"000000", &sink_key());

    assert_eq!(
        pairing::pair_with(CODE, &mine(), &mut windows),
        Paired::WrongCode
    );
    assert_eq!(windows.pinned_text(), None);
}

#[test]
fn a_sink_that_cannot_be_reached_pins_nothing() {
    // The ordinary state of a machine whose Mac is not showing a code: nothing to
    // connect to, so nothing is exchanged and nothing is written down.
    let mut windows = SimSourcePairing::new();

    assert_eq!(
        pairing::pair_with(CODE, &mine(), &mut windows),
        Paired::NoSink
    );
    assert_eq!(windows.pinned_text(), None);
}

#[test]
fn the_offer_goes_out_before_an_answer_is_waited_for() {
    // This end speaks first, which is what makes it the end that connects: waiting
    // for an answer to a question not yet asked is a wait nothing ends.
    let mut windows = SimSourcePairing::new().with_a_sink_that_knows(CODE, &sink_key());

    pairing::pair_with(CODE, &mine(), &mut windows);

    assert!(
        windows.offered_before_waiting(),
        "waited for an answer before sending the offer"
    );
}

#[test]
fn nothing_is_pinned_before_the_sinks_key_has_arrived() {
    // Pinning is what the exchange is for: a key written down before the sink had
    // sent one is a key the code did not vouch for, and what opened it is the secret
    // the code agreed.
    let mut windows = SimSourcePairing::new().with_a_sink_that_knows(CODE, &sink_key());

    pairing::pair_with(CODE, &mine(), &mut windows);

    assert!(
        windows.took_the_key_before_pinning(),
        "pinned a key before the sink had sent one"
    );
}

#[test]
fn a_sink_that_stops_answering_part_way_pins_nothing() {
    // A connection that goes during the exchange is not a wrong code, and the two
    // are different things to be told: one is worth trying again with the same
    // code, the other needs a new one.
    let mut windows = SimSourcePairing::new()
        .with_a_sink_that_knows(CODE, &sink_key())
        .that_stops_after_the_answer();

    assert_eq!(
        pairing::pair_with(CODE, &mine(), &mut windows),
        Paired::Interrupted
    );
    assert_eq!(windows.pinned_text(), None);
}

#[test]
fn the_sink_is_given_this_machines_key() {
    // Both directions: the sink cannot accept input from a machine whose key it has
    // not pinned, so pairing has to carry this one over as well.
    let mine = mine();
    let mut windows = SimSourcePairing::new().with_a_sink_that_knows(CODE, &sink_key());

    pairing::pair_with(CODE, &mine, &mut windows);

    assert_eq!(windows.gave_the_sink(), Some(mine.public().to_vec()));
}

#[test]
fn a_later_run_reads_the_file_as_the_sink_whose_digits_were_shown() {
    // The other half of what `names` asserts. That one reads the file without
    // `engine` and so says the format is the one the other machine can read; this
    // one puts the file through the reader a relaying run is given, because a
    // pairing that wrote a file its own next run cannot read is a machine that
    // pairs and then has nowhere to send input.
    let mut windows = SimSourcePairing::new().with_a_sink_that_knows(CODE, &sink_key());

    let shown = match pairing::pair_with(CODE, &mine(), &mut windows) {
        Paired::Pinned(digits) => digits,
        other => panic!("the code was the sink's, so this pairs: {other:?}"),
    };

    let text = windows
        .pinned_text()
        .expect("a pairing that pinned wrote a file");

    // Put through the reader a relaying run is given rather than `pairing`'s
    // directly, because that reader — and the digits it turns the file into —
    // are what a later run's `--identity` actually shows.
    let mut relaying = SimSource::new();
    relaying.with_pinned_sink_text(&text);
    let identified = source::identify(&mut relaying).expect("this machine can always make one");

    // The digits a run prints for the machine it sends input to are the ones
    // pairing put on the screen: comparing the two is how a person tells that this
    // machine is pinned to the Mac they paired with, so a second spelling of them
    // would make that comparison say nothing.
    assert_eq!(identified.sink, Some(shown));
}
