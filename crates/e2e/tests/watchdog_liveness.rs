//! What the supervising watchdog is promised (ADR-0008).
//!
//! The watchdog's whole judgement is "a probe went in and no heartbeat came
//! out". These pin the two halves of that from the loop's side: `watchdog.rs` drives
//! the judgement against a scripted machine, and here it is the run that is real.
//!
//! Both roles are here. A watchdog is per machine, so what the source promises is a
//! separate promise from what the sink promises — and the source's is the easier one
//! to break, because it is the role that decides some of its events are not worth
//! relaying.
//!
//! The last two tests are both halves at once, which is the only place the promise is
//! checked against itself rather than each side against an idea of the other.
//!
//! What a run typed is read off [`SinkHost::send_report`]'s own bytes, decoded
//! with `favjit-hid`'s inverse of the encoding — not off `engine`'s internal
//! record of what it decided to inject, which is not anything a downstream
//! device ever sees.
//!
//! [`SinkHost::send_report`]: favjit_host::sink::SinkHost::send_report

use core::time::Duration;

use favjit_engine::sink::{self, InputConfig, Request};
use favjit_engine::watchdog::{self, Bound, Exit, Supervised};
use favjit_engine::{source, DeviceId, Key, Layout};
use favjit_hid::report::Report;
use favjit_hid::{page, usage};
use favjit_host::source::Driving;
use favjit_host::OutputReport;
use favjit_host_sim::{identity, SimHost, SimSource, SimWatchdog};

/// The Mac's own keyboard, and the Windows machine's. The same number on purpose:
/// each machine numbers its devices from one, and nothing here puts the two in one
/// run.
const BUILT_IN: DeviceId = DeviceId(1);
const KEYBOARD: DeviceId = DeviceId(1);

/// The sink the Windows machine is pinned to.
const SINK: u8 = 2;

fn host() -> SimHost {
    let mut host = SimHost::new();
    host.attach_built_in(BUILT_IN);
    host
}

/// A whole run of the program, on a simulated Mac.
///
/// Through the program rather than the converter's loop, because that is what a
/// person starts: a suite that called the loop directly would pass while the run
/// that reaches it was broken.
fn run(mac: &mut SimHost) {
    sink::run(
        &Request::Injecting { listen: false },
        Layout::dudrack(),
        None,
        InputConfig::default(),
        mac,
        None,
    );
}

/// Every report the run wrote, decoded back into the keyboard state it
/// described — nothing here types on a control's page.
fn keyboard_reports(host: &SimHost) -> Vec<Report> {
    host.reports_while_reading()
        .iter()
        .map(|(_, report, bytes)| {
            assert_eq!(*report, OutputReport::Keyboard, "not a control");
            Report::from_bytes(bytes).expect("the bytes of a keyboard report")
        })
        .collect()
}

/// The report a keyboard with nothing else down would carry for `key` alone.
fn holding(key: Key) -> Report {
    let (on_page, code) = usage::of(key).expect("a key with a usage");
    assert_eq!(on_page, page::KEYBOARD_OR_KEYPAD, "not a plain key");
    let mut report = Report::default();
    report.press(code);
    report
}

/// The beats a run makes on its way to a link, beside the one each event gets.
///
/// Two: one before the connection and one before the handshake, which are the waits on
/// that path a run cannot take in pieces — so what it owes its supervisor there is a
/// beat on each side of them. Counted rather than left out of the sums below, because a
/// third blocking call added there with no beat in front of it is exactly the failure
/// this file exists to catch: a run ended while it was working.
const BEATS_PER_LINK: usize = 2;

/// The forwarding machine, asked to send its keyboard over before anything else.
///
/// A run comes up with the keyboard on the machine it is running on (ADR-0013), and what
/// a watchdog has something to protect is the state after the ask: that is where the keys
/// are refused.
fn source() -> SimSource {
    let mut host = SimSource::new();
    host.asked_for(Driving::TheSink);
    host.pinned_to(identity(SINK));
    host.attach_external(KEYBOARD, 0x17ef, 0x60e1);
    host
}

/// A whole run of the forwarding machine, relaying and suppressing — the mode in
/// which a watchdog would have something to protect.
fn forward(windows: &mut SimSource) {
    source::run(&source::Request::Relaying, false, windows, None);
}

#[test]
fn every_event_is_answered_with_one_heartbeat() {
    let mut host = host();
    host.tap(BUILT_IN, Key::S);
    host.tap(BUILT_IN, Key::D);
    run(&mut host);

    // One device attach, then two presses and two releases.
    assert_eq!(host.heartbeats().len(), 5);
}

#[test]
fn a_probe_produces_a_heartbeat_and_nothing_else() {
    let mut host = host();
    host.probe();
    run(&mut host);

    // Two events, two heartbeats: the attach and the probe.
    assert_eq!(host.heartbeats().len(), 2);
    // A probe must not be able to become a keystroke however the tables are
    // written, which is why it is a kind of its own rather than a reserved key.
    assert_eq!(host.reports(), &[]);
}

#[test]
fn a_probe_answers_while_a_key_is_held() {
    // The interesting case for a watchdog is a loop that is mid-chord rather
    // than idle: a probe arriving between a press and its release must still
    // come back, and must not disturb what the held key resolves to.
    let mut host = host();
    host.press(BUILT_IN, Key::RightCommand);
    host.probe();
    host.tap(BUILT_IN, Key::Q);
    host.probe();
    host.release(BUILT_IN, Key::RightCommand);
    run(&mut host);

    // The attach, two probes, and the press and release of each of the two keys.
    assert_eq!(host.heartbeats().len(), 7);
    // Right command holds the Henkan layer, where 'q' is '1' — a layer with
    // nothing of its own to report, so only the digit's press and release ever
    // reach a report.
    assert_eq!(
        keyboard_reports(&host),
        vec![holding(Key::Digit1), Report::default()]
    );
}

#[test]
fn the_source_answers_every_event_it_handles_and_not_every_message_it_sends() {
    // The two counts differ on purpose: a held key's auto-repeat is an event the
    // loop came back round on and a message it decided not to send. A heartbeat
    // hung off the sending instead would report a source reading a held key as one
    // that had stopped turning, and its watchdog would kill it mid-keystroke.
    let mut host = source();
    host.press(KEYBOARD, Key::J);
    host.press(KEYBOARD, Key::J);
    host.press(KEYBOARD, Key::J);
    host.release(KEYBOARD, Key::J);

    forward(&mut host);

    assert_eq!(
        host.heartbeats().len(),
        6 + BEATS_PER_LINK,
        "the ask, the attach, four key events, and the link"
    );
    assert_eq!(host.sent().len(), 3, "the attach, one press, one release");
}

#[test]
fn a_probe_to_the_source_is_answered_and_crosses_nothing() {
    // The probe is this machine's watchdog asking about this machine. Relaying it
    // would be telling the Mac about the state of the Windows side, and it must
    // not be able to become a keystroke there however the tables are written.
    let mut host = source();
    host.probe();
    host.tap(KEYBOARD, Key::J);
    host.probe();

    forward(&mut host);

    assert_eq!(
        host.heartbeats().len(),
        6 + BEATS_PER_LINK,
        "the ask, the attach, two probes, the press and release, and the link"
    );
    assert_eq!(host.sent().len(), 3, "the attach, the press, the release");
}

#[test]
fn a_heartbeat_follows_the_work_rather_than_preceding_it() {
    // A heartbeat sent on the way into an event would vouch for a loop about to
    // wedge inside it. Read off the timestamps: each heartbeat carries the time
    // of the event it answers, so the counts have to line up event for event.
    let mut host = host();
    host.press(BUILT_IN, Key::S);
    run(&mut host);

    let beats = host.heartbeats().to_vec();
    assert_eq!(beats.len(), 2);
    // The report the press wrote is stamped with the same time as the heartbeat
    // that answers it.
    let reports = host.reports_while_reading();
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].0, beats[1]);
}

#[test]
fn a_converting_run_satisfies_a_real_watchdog() {
    // Both halves of the promise, against each other. Everything above says the run
    // answers what it is asked; `watchdog.rs` says the judgement ends a process that
    // stops answering. Neither says the answers a real run makes are the answers this
    // judgement is satisfied by — which is what is left to get wrong, and this is
    // where it would show.
    let mut mac = host();
    mac.probe();
    mac.tap(BUILT_IN, Key::S);
    mac.probe();
    run(&mut mac);

    let mut watchdog = SimWatchdog::supervising(&mac.heartbeats(), Exit::Code(0));
    let bound = Bound {
        silence: Duration::from_secs(2),
        probe_every: Duration::from_millis(250),
        grace: Duration::from_millis(200),
    };

    assert_eq!(
        watchdog::run(&bound, &mut watchdog),
        Supervised::Ended(Exit::Code(0)),
        "the converting run's own heartbeats left its watchdog nothing to do"
    );
    assert_eq!(watchdog.killed(), None);
}
