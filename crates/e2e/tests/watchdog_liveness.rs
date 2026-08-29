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

use core::time::Duration;

use favjit_core::sink::{self, Request};
use favjit_core::watchdog::{self, Bound, Exit, Supervised};
use favjit_core::{source, DeviceId, DeviceInfo, EventKind, Injected, Key, Layout, ModifierKeys};
use favjit_host_sim::{SimHost, SimSource, SimWatchdog};

/// The Mac's own keyboard, and the Windows machine's. The same number on purpose:
/// each machine numbers its devices from one, and nothing here puts the two in one
/// run.
const BUILT_IN: DeviceId = DeviceId(1);
const KEYBOARD: DeviceId = DeviceId(1);

fn host() -> SimHost {
    let mut host = SimHost::new();
    host.attach(DeviceInfo::built_in(BUILT_IN));
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
        mac,
        None,
    );
}

fn source() -> SimSource {
    let mut host = SimSource::new();
    host.attach(DeviceInfo::external(KEYBOARD, 0x17ef, 0x60e1));
    host
}

/// A whole run of the forwarding machine, relaying and suppressing — the mode in
/// which a watchdog would have something to protect.
fn forward(windows: &mut SimSource) {
    source::run(&source::Request::Relaying, windows);
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
    assert_eq!(host.injected(), Vec::<Injected>::new());
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
    // Right command holds the Henkan layer, where 'q' is '1'.
    assert_eq!(
        host.injected(),
        vec![
            Injected::KeyDown {
                key: Key::Digit1,
                modifiers: ModifierKeys::NONE
            },
            Injected::KeyUp {
                key: Key::Digit1,
                modifiers: ModifierKeys::NONE
            },
        ]
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

    assert_eq!(host.heartbeats().len(), 5, "the attach and four key events");
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
        5,
        "the attach, two probes, and the press and release"
    );
    assert_eq!(host.sent().len(), 3, "the attach, the press, the release");
}

#[test]
fn a_heartbeat_follows_the_work_rather_than_preceding_it() {
    // A heartbeat sent on the way into an event would vouch for a loop about to
    // wedge inside it. Read off the timestamps: each heartbeat carries the time
    // of the event it answers, so the counts have to line up event for event.
    let mut host = host();
    host.script(EventKind::KeyDown {
        device: BUILT_IN,
        key: Key::S,
    });
    run(&mut host);

    let beats = host.heartbeats().to_vec();
    assert_eq!(beats.len(), 2);
    // The injection for the key press is recorded before the heartbeat that
    // answers it, both stamped with the same event's time.
    let records = host.records();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].at, beats[1]);
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

    let mut watchdog = SimWatchdog::supervising(mac.heartbeats(), Exit::Code(0));
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
