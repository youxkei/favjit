//! One recording, holding both machines' records (ADR-0009).
//!
//! **The forwarding machine's records cross the link.** It keeps them while there
//! is none — which is exactly when the interesting ones are made, since a link
//! that dropped is what a person is reading about — and sends everything it holds
//! once a session is up. So the converting machine's recording is the one that
//! holds both, and reading it needs one file and no second machine.
//!
//! What that buys is the question neither machine can answer alone: a link that
//! keeps dropping either had a write fail on the sending end or was let go by the
//! receiving one, and only a reading with both in it says which.

use favjit_engine::sink::{self, InputConfig, Request as SinkRequest};
use favjit_engine::trace::{Record, Side, Trace};
use favjit_engine::{source, DeviceId, Key, Layout};
use favjit_host::source::Driving;
use favjit_host_sim::{identity, SimHost, SimLink, SimSource};

/// The keyboard on the Windows side, as that machine numbers it.
const KEYBOARD: DeviceId = DeviceId(3);

/// The Windows machine's identity, as the Mac has pinned it.
const PAIRED: u8 = 7;

/// The sink the source run is pinned to.
const THIS_MACHINES_SINK: u8 = 21;

/// The vendor and product a rule names that keyboard with.
const KEYBOARD_IDENTITY: (u16, u16) = (0x17ef, 0x60e1);

/// What Windows says about a write that ran out of time, and one the other end
/// reset.
const TIMED_OUT: i32 = 10060;
const RESET_BY_PEER: i32 = 10054;

fn forwarding() -> source::Request {
    source::Request::Relaying
}

/// Serving the link, because that is what carries the other machine's records.
fn converting() -> SinkRequest {
    SinkRequest::Injecting { listen: true }
}

fn hex(bytes: &[u8]) -> String {
    use core::fmt::Write;
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// A key typed on the Windows machine, once it has been asked for.
fn one_keystroke(windows: &mut SimSource) {
    windows.attach_external(KEYBOARD, KEYBOARD_IDENTITY.0, KEYBOARD_IDENTITY.1);
    windows.press(KEYBOARD, Key::S);
    windows.release(KEYBOARD, Key::S);
}

/// Run the forwarding machine, then hand what it sent to the converting one, and
/// give back the converting machine's recording — the only one a reader needs.
fn the_recording(script: impl Fn(&mut SimSource)) -> Vec<u8> {
    let mut on_windows = vec![0u8; 64 * 1024];
    let mut windows = SimSource::new();
    windows.asked_for(Driving::TheSink);
    windows.pinned_to(identity(THIS_MACHINES_SINK));
    script(&mut windows);
    source::run(&forwarding(), false, &mut windows, Some(&mut on_windows));

    let mut link = SimLink::new(format!("{}\n", hex(identity(PAIRED).public())));
    link.connect(identity(PAIRED));
    link.relay(&windows.sent());

    let mut on_the_mac = vec![0u8; 64 * 1024];
    let mut mac = SimHost::new().with_link(link);
    sink::run(
        &converting(),
        Layout::dudrack(),
        None,
        InputConfig::default(),
        &mut mac,
        Some(&mut on_the_mac),
    );
    on_the_mac
}

/// Every record in it, and which machine made each one.
fn read(bytes: &[u8]) -> Vec<(Side, Record)> {
    Trace::read(bytes).records().collect()
}

/// Which end let the link go, and why.
fn how_the_link_ended(records: &[(Side, Record)]) -> Vec<(Side, String)> {
    records
        .iter()
        .filter_map(|(side, record)| match record {
            Record::Sent { code, .. } if *code != 0 => {
                Some((*side, format!("the write failed, error {code}")))
            }
            Record::LinkEnded { why } => Some((
                *side,
                favjit_engine::link::Refused::from_number(*why).map_or_else(
                    || format!("unnamed ({why})"),
                    |reason| reason.said().to_owned(),
                ),
            )),
            _ => None,
        })
        .collect()
}

#[test]
fn the_converting_machines_recording_holds_the_forwarding_machines_records_too() {
    let bytes = the_recording(one_keystroke);
    let records = read(&bytes);

    // Both machines are in one file, each record named by the machine that made
    // it: that is what makes a reading of the link possible from one place.
    assert!(
        records
            .iter()
            .any(|(side, record)| *side == Side::Source && matches!(record, Record::Event(_))),
        "the forwarding machine's own events are missing"
    );
    assert!(
        records
            .iter()
            .any(|(side, record)| *side == Side::Sink && matches!(record, Record::Injected { .. })),
        "the converting machine's own injections are missing"
    );
}

#[test]
fn every_record_that_crossed_is_in_it_from_both_ends_under_the_same_number() {
    let bytes = the_recording(one_keystroke);
    let records = read(&bytes);

    let sent: Vec<u64> = records
        .iter()
        .filter_map(|(side, record)| match record {
            Record::Sent { at, .. } if *side == Side::Source => Some(*at),
            _ => None,
        })
        .collect();
    let received: Vec<u64> = records
        .iter()
        .filter_map(|(side, record)| match record {
            Record::Received { at } if *side == Side::Sink => Some(*at),
            _ => None,
        })
        .collect();

    assert!(!sent.is_empty(), "nothing was recorded as sent");
    assert!(!received.is_empty(), "nothing was recorded as arriving");
    // Every number the receiving end wrote is one the sending end wrote too: the
    // count is the session's own, so neither end can name a record the other did
    // not send.
    //
    // Not the same list, because the two ends record different things at
    // different numbers: a keystroke is a `Sent` on one side and an arrival on
    // the other, while a frame carrying the forwarding machine's own history
    // takes a number of its own and is not an arrival at all.
    for at in &received {
        assert!(sent.contains(at), "arrival {at} was never sent: {sent:?}");
    }
    // And they only go up, so the order the reading is in is the order they went.
    assert!(sent.windows(2).all(|pair| pair[0] < pair[1]), "{sent:?}");
}

#[test]
fn what_the_forwarding_machine_recorded_while_there_was_no_link_arrives_once_there_is_one() {
    // The case that makes this worth doing at all: the records that explain a
    // dropped link are made when there is no link to send them over, so they are
    // kept and sent on the session after — and a reading that lost them would
    // lose exactly the ones a person is looking for.
    let bytes = the_recording(|windows| {
        one_keystroke(windows);
        windows.answers_a_send_with(TIMED_OUT);
        windows.link_gone();
        windows.press(KEYBOARD, Key::A);
        windows.link_back();
        windows.press(KEYBOARD, Key::J);
    });

    let ended = how_the_link_ended(&read(&bytes));

    assert!(
        ended
            .iter()
            .any(|(side, said)| *side == Side::Source && said.contains(&TIMED_OUT.to_string())),
        "{ended:?}"
    );
}

#[test]
fn a_write_that_ran_out_of_time_and_one_the_other_end_reset_read_differently() {
    // The two failures a person has to tell apart, and the whole reason the
    // number crosses rather than a flag.
    let timed_out = the_recording(|windows| {
        one_keystroke(windows);
        windows.answers_a_send_with(TIMED_OUT);
        windows.link_gone();
        windows.press(KEYBOARD, Key::A);
        windows.link_back();
        windows.press(KEYBOARD, Key::J);
    });
    let reset = the_recording(|windows| {
        one_keystroke(windows);
        windows.answers_a_send_with(RESET_BY_PEER);
        windows.link_gone();
        windows.press(KEYBOARD, Key::A);
        windows.link_back();
        windows.press(KEYBOARD, Key::J);
    });

    let first = how_the_link_ended(&read(&timed_out));
    let second = how_the_link_ended(&read(&reset));

    assert!(
        first
            .iter()
            .any(|(_, said)| said.contains(&TIMED_OUT.to_string())),
        "{first:?}"
    );
    assert!(
        second
            .iter()
            .any(|(_, said)| said.contains(&RESET_BY_PEER.to_string())),
        "{second:?}"
    );
    // Neither reading carries the other's, so a person with one is not left
    // choosing between them.
    assert!(!first
        .iter()
        .any(|(_, said)| said.contains(&RESET_BY_PEER.to_string())));
    assert!(!second
        .iter()
        .any(|(_, said)| said.contains(&TIMED_OUT.to_string())));
}

#[test]
fn a_link_the_receiving_end_let_go_reads_as_that_ends_doing() {
    // The other half of the question: this end refused, and the forwarding
    // machine's half of the reading has no failed write on it.
    let mut windows = SimSource::new();
    windows.asked_for(Driving::TheSink);
    windows.pinned_to(identity(THIS_MACHINES_SINK));
    one_keystroke(&mut windows);
    let mut on_windows = vec![0u8; 64 * 1024];
    source::run(&forwarding(), false, &mut windows, Some(&mut on_windows));

    // Pinned to one identity and connected to by another, which is the whole of
    // what an unpaired source is.
    let mut link = SimLink::new(format!("{}\n", hex(identity(PAIRED).public())));
    link.connect(identity(PAIRED + 1));
    link.relay(&windows.sent());

    let mut on_the_mac = vec![0u8; 64 * 1024];
    let mut mac = SimHost::new().with_link(link);
    sink::run(
        &converting(),
        Layout::dudrack(),
        None,
        InputConfig::default(),
        &mut mac,
        Some(&mut on_the_mac),
    );

    let ended = how_the_link_ended(&read(&on_the_mac));

    assert!(
        ended
            .iter()
            .any(|(side, said)| *side == Side::Sink && said.contains("not paired")),
        "{ended:?}"
    );
    assert!(
        !ended.iter().any(|(side, _)| *side == Side::Source),
        "{ended:?}"
    );
}
