//! Every process favjit runs, in one test: the source, the sink, and a watchdog over
//! each of them.
//!
//! `link_both_ends.rs` puts a whole source run and a whole sink run together and asks
//! what came out. `watchdog_liveness.rs` asks each role whether it answers its probes,
//! and `watchdog.rs` asks the judgement what it does about a silence. None of them
//! puts all four together, which leaves one thing unchecked: whether the heartbeats a
//! real relaying run makes, while it is holding keyboards and forwarding over a link,
//! are the heartbeats its own watchdog is satisfied by — and the same for the
//! converting run at the other end.
//!
//! Four whole runs, in turn, on the test's own thread (ADR-0007). Each machine's
//! watchdog is scripted from the heartbeats that machine's run actually made, so
//! nothing here is an idea of what a run would do.

use core::time::Duration;

use favjit_core::pairing::hex;
use favjit_core::sink;
use favjit_core::watchdog::{self, Bound, Exit, Supervised};
use favjit_core::{source, DeviceId, DeviceInfo, Injected, Key, Layout};
use favjit_host_sim::{SimHost, SimLink, SimSource, SimWatchdog};

/// The keyboard on the Windows side, as that machine numbers it.
const KEYBOARD: DeviceId = DeviceId(3);

/// The Windows machine's identity, as the Mac has pinned it.
const PAIRED: [u8; 32] = [7; 32];

/// What each machine's watchdog was told to allow — the same numbers on both, since
/// the bound is about a person's patience and there is one person.
fn bound() -> Bound {
    Bound {
        silence: Duration::from_secs(2),
        probe_every: Duration::from_millis(250),
        grace: Duration::from_millis(200),
    }
}

fn keyboard() -> DeviceInfo {
    DeviceInfo::external(KEYBOARD, 1234, 5678)
}

/// A whole supervision of a run that made these heartbeats and then finished.
fn supervise(beats: &[favjit_core::Instant]) -> (Supervised, SimWatchdog) {
    let mut watchdog = SimWatchdog::supervising(beats, Exit::Code(0));
    let ending = watchdog::run(&bound(), &mut watchdog);
    (ending, watchdog)
}

#[test]
fn all_four_run_and_neither_watchdog_takes_a_keyboard_away() {
    // The whole topology, supervised at both ends. What it is for is the one failure
    // no smaller test can produce: a run that is doing its job and a watchdog that
    // ends it anyway, which is a keyboard taken away from somebody who was typing
    // (ADR-0008).

    // The Windows machine: reading its keyboards, refusing them, relaying — with its
    // own watchdog's probes arriving on the same stream as the keystrokes, which is
    // where they arrive on a real machine.
    let mut windows = SimSource::new();
    windows.attach(keyboard());
    windows.probe();
    windows.tap(KEYBOARD, Key::International3);
    windows.probe();
    source::run(&source::Request::Relaying, &mut windows);

    // Its watchdog, over exactly the heartbeats that run made.
    let (forwarding, windows_watchdog) = supervise(windows.heartbeats());

    // The Mac: what the source sent, arriving as the frames of its link, converted and
    // injected — with its own watchdog's probes on its stream too.
    let mut link = SimLink::new(format!("{}\n", hex(&PAIRED)));
    link.connect(PAIRED.to_vec());
    link.relay(windows.sent());

    let mut mac = SimHost::new().with_link(link);
    mac.probe();
    sink::run(
        &sink::Request::Injecting { listen: true },
        Layout::dudrack(),
        None,
        &mut mac,
        None,
    );

    let (converting, mac_watchdog) = supervise(mac.heartbeats());

    // The keystroke made it all the way through.
    assert_ne!(
        mac.injected(),
        Vec::<Injected>::new(),
        "nothing reached applications, so there was nothing for the watchdogs to protect"
    );

    // And neither machine had its keyboards taken away.
    assert_eq!(forwarding, Supervised::Ended(Exit::Code(0)));
    assert_eq!(converting, Supervised::Ended(Exit::Code(0)));
    assert_eq!(
        windows_watchdog.killed(),
        None,
        "the forwarding run was ended by its own watchdog"
    );
    assert_eq!(
        mac_watchdog.killed(),
        None,
        "the converting run was ended by its own watchdog"
    );
}

#[test]
fn each_machines_watchdog_answers_for_that_machine_alone() {
    // A watchdog is per machine (ADR-0008). One that read the other machine's
    // heartbeats would report a wedged converter as healthy for as long as the
    // forwarding side kept typing, which is the failure this arrangement exists to
    // make impossible.
    //
    // Driven by giving the Mac's watchdog nothing of its own: the forwarding run's
    // heartbeats are no answer to a probe it never received.
    let mut windows = SimSource::new();
    windows.attach(keyboard());
    windows.tap(KEYBOARD, Key::International3);
    source::run(&source::Request::Relaying, &mut windows);

    let (forwarding, _) = supervise(windows.heartbeats());
    assert_eq!(forwarding, Supervised::Ended(Exit::Code(0)));

    // The Mac's watchdog, over a converting run that never answered.
    let mut mac_watchdog = SimWatchdog::new().that_never_answers();
    assert_eq!(
        watchdog::run(&bound(), &mut mac_watchdog),
        Supervised::Killed,
        "a converting run that stopped answering was left holding the Mac's keyboards"
    );
}
