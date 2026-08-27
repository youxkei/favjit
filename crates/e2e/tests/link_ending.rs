//! What happens to keys that were down when the link went away.
//!
//! The sink is what the OS believes. A source that vanishes mid-keystroke must not
//! leave a modifier held in every application, which is the failure ADR-0002 puts
//! on the sink — and a network that drops gives no detach, so nothing but the end
//! of the session says the keys are gone.

use favjit_core::pairing::Authorized;
use favjit_core::sink::Request;
use favjit_core::{link, sink, DeviceId, DeviceInfo, EventKind, Injected, Key, Layout};
use favjit_host_sim::{SimHost, SimLink};

/// A run that converts, since what these are about is what the sink does with what
/// the link delivered rather than how the machine was brought up.
fn converting() -> Request {
    Request::Injecting { listen: true }
}

fn key(fill: u8) -> Vec<u8> {
    vec![fill; 32]
}

const PAIRED: u8 = 0xaa;

/// What the source calls the keyboard, and what this machine calls it: the two are
/// not the same, because both machines number their own devices from one.
const REMOTE: DeviceId = DeviceId(7);

fn here(device: DeviceId) -> DeviceId {
    link::from_source(device)
}

fn paired_list() -> String {
    Authorized::added("", &key(PAIRED))
}

/// A whole run of the converter, with the other machine on the other end of its
/// link.
///
/// The machine is asked afterwards both what its link put into the stream and what
/// the converter made of it: those two answers are the same run, and a suite that
/// carried the events across by hand would be the one thing on the real machine
/// that nothing checks.
fn served(script: impl FnOnce(&mut SimLink)) -> SimHost {
    let mut link = SimLink::new(paired_list());
    script(&mut link);
    let mut mac = SimHost::new().with_link(link);
    sink::run(&converting(), Layout::dudrack(), None, &mut mac, None);
    mac
}

#[test]
fn a_session_that_ends_says_the_peers_devices_are_gone() {
    // Delivered as a detach, because that is what the sink already answers by
    // releasing what the device held — a second way of saying it would be a second
    // path for a held key to survive.
    let mac = served(|link| {
        link.connect(key(PAIRED))
            .attach(DeviceInfo::external(REMOTE, 1, 2))
            .press(REMOTE, Key::LeftShift)
            .hang_up();
    });

    assert_eq!(
        mac.delivered(),
        vec![
            EventKind::DeviceAttached(DeviceInfo::external(here(REMOTE), 1, 2)),
            EventKind::KeyDown {
                device: here(REMOTE),
                key: Key::LeftShift
            },
            EventKind::DeviceDetached(here(REMOTE)),
        ]
    );
}

#[test]
fn a_device_that_only_ever_sent_keys_is_still_said_to_be_gone() {
    // A source may connect with keys already down, so the first thing the sink
    // hears about a device can be a key event. Releasing only what was announced
    // would leave that one held.
    let mac = served(|link| {
        link.connect(key(PAIRED))
            .press(REMOTE, Key::LeftShift)
            .hang_up();
    });

    assert_eq!(
        mac.delivered().last(),
        Some(&EventKind::DeviceDetached(here(REMOTE)))
    );
}

#[test]
fn the_shift_a_dropped_link_was_holding_comes_back_up() {
    // The whole point, through the converter: what the OS was told is down has to
    // be told it is up, or every window is shift-clicked from then on.
    let mac = served(|link| {
        link.connect(key(PAIRED))
            .attach(DeviceInfo::external(REMOTE, 1, 2))
            .press(REMOTE, Key::LeftShift)
            .hang_up();
    });

    assert_eq!(
        mac.injected(),
        vec![
            // Down naming itself, up naming what is left: the set is the whole of
            // what the OS is told, so an empty one on the way down would be a
            // shift that never arrived and a full one on the way up would be a
            // shift that never left.
            Injected::KeyDown {
                key: Key::LeftShift,
                modifiers: favjit_core::ModifierKeys::of(&[Key::LeftShift])
            },
            Injected::KeyUp {
                key: Key::LeftShift,
                modifiers: favjit_core::ModifierKeys::NONE
            },
        ]
    );
}

#[test]
fn a_detach_the_source_sent_is_not_repeated_at_the_end() {
    // A source that unplugged a keyboard says so, and the sink has already released
    // it. Saying it again would be an event the hardware never produced.
    let mac = served(|link| {
        link.connect(key(PAIRED))
            .attach(DeviceInfo::external(REMOTE, 1, 2))
            .press(REMOTE, Key::LeftShift)
            .detach(REMOTE)
            .hang_up();
    });

    let detaches = mac
        .delivered()
        .iter()
        .filter(|kind| matches!(kind, EventKind::DeviceDetached(_)))
        .count();
    assert_eq!(detaches, 1);
}

#[test]
fn the_next_session_starts_with_nothing_held() {
    // Each connection is its own set of hands: a device released at the end of one
    // must not be released again at the end of the next.
    let mac = served(|link| {
        link.connect(key(PAIRED))
            .attach(DeviceInfo::external(REMOTE, 1, 2))
            .hang_up();
        link.connect(key(PAIRED))
            .press(DeviceId(9), Key::A)
            .hang_up();
    });

    assert_eq!(
        mac.delivered(),
        vec![
            EventKind::DeviceAttached(DeviceInfo::external(here(REMOTE), 1, 2)),
            EventKind::DeviceDetached(here(REMOTE)),
            EventKind::KeyDown {
                device: here(DeviceId(9)),
                key: Key::A
            },
            EventKind::DeviceDetached(here(DeviceId(9))),
        ]
    );
}
