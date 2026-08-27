//! Who gets to send input, and what happens to what they send.
//!
//! ADR-0004 puts the decision on the sink and makes refusal the default. Each of
//! these is a whole run of the converter with the other machine on the other end of
//! its link, so the sequence — ask who is calling, look them up, only then read
//! anything — is checked where it actually happens, and without a platform.

use favjit_core::pairing::Authorized;
use favjit_core::sink::{self, Request};
use favjit_core::{link, DeviceId, DeviceInfo, EventKind, Key, Layout};
use favjit_host_sim::{SimHost, SimLink};

fn key(fill: u8) -> Vec<u8> {
    vec![fill; 32]
}

/// A source that has been paired, and one that has not.
const PAIRED: u8 = 0xaa;
const STRANGER: u8 = 0xbb;

/// What the source calls its keyboard. What this machine calls it is
/// [`link::from_source`] of that, since both machines number from one.
const REMOTE: DeviceId = DeviceId(7);

fn here(device: DeviceId) -> DeviceId {
    link::from_source(device)
}

fn paired_list() -> String {
    Authorized::added("", &key(PAIRED))
}

/// A run that converts and listens, which is the only run a link belongs to.
fn converting() -> Request {
    Request::Injecting { listen: true }
}

/// A whole run, with this authorised list and this script on the other end.
fn served(authorized: String, script: impl FnOnce(&mut SimLink)) -> SimHost {
    let mut link = SimLink::new(authorized);
    script(&mut link);
    let mut mac = SimHost::new().with_link(link);
    sink::run(&converting(), Layout::dudrack(), None, &mut mac, None);
    mac
}

#[test]
fn input_from_a_paired_source_arrives_as_events() {
    let mac = served(paired_list(), |link| {
        link.connect(key(PAIRED))
            .attach(DeviceInfo::external(REMOTE, 1, 2))
            .press(REMOTE, Key::K)
            .release(REMOTE, Key::K)
            .hang_up();
    });

    assert_eq!(
        mac.delivered(),
        vec![
            EventKind::DeviceAttached(DeviceInfo::external(here(REMOTE), 1, 2)),
            EventKind::KeyDown {
                device: here(REMOTE),
                key: Key::K
            },
            EventKind::KeyUp {
                device: here(REMOTE),
                key: Key::K
            },
            // The session ended, so the keyboard is gone with it — which is what
            // releases anything it was holding (see `link_ending.rs`).
            EventKind::DeviceDetached(here(REMOTE)),
        ]
    );
    assert!(
        !mac.injected().is_empty(),
        "and the converter had it in the same stream as its own keyboards"
    );
}

#[test]
fn a_source_nobody_paired_gets_nothing_through() {
    // Refused before a frame is read, not after: input that reached the converter
    // and was then discarded would already have been converted.
    let mac = served(paired_list(), |link| {
        link.connect(key(STRANGER))
            .press(REMOTE, Key::K)
            .release(REMOTE, Key::K)
            .hang_up();
    });

    assert!(mac.delivered().is_empty());
    assert_eq!(mac.refused(), vec![key(STRANGER)]);
    assert_eq!(mac.frames_read(), 0);
    assert_eq!(mac.injected(), Vec::new());
}

#[test]
fn an_empty_list_refuses_everyone() {
    // The state a machine starts in, and the one a lost list leaves it in.
    let mac = served(String::new(), |link| {
        link.connect(key(PAIRED)).press(REMOTE, Key::K).hang_up();
    });

    assert!(mac.delivered().is_empty());
    assert_eq!(mac.refused(), vec![key(PAIRED)]);
}

#[test]
fn pairing_takes_effect_on_the_next_connection() {
    // The list is asked for per connection, so authorising a source does not need
    // the converter restarted — and neither does removing one.
    let mac = served(String::new(), |link| {
        link.connect(key(PAIRED)).press(REMOTE, Key::K).hang_up();
        link.pair(key(PAIRED));
        link.connect(key(PAIRED))
            .attach(DeviceInfo::external(REMOTE, 1, 2))
            .press(REMOTE, Key::K)
            .hang_up();
    });

    assert_eq!(mac.refused(), vec![key(PAIRED)]);
    assert_eq!(
        mac.delivered(),
        vec![
            EventKind::DeviceAttached(DeviceInfo::external(here(REMOTE), 1, 2)),
            EventKind::KeyDown {
                device: here(REMOTE),
                key: Key::K
            },
            EventKind::DeviceDetached(here(REMOTE)),
        ]
    );
}

#[test]
fn a_frame_that_cannot_be_read_ends_the_session() {
    // A peer talking a language this end does not is a peer to hang up on: acting
    // on the messages around one that could not be read is acting on a stream
    // whose meaning is already in doubt.
    let mac = served(paired_list(), |link| {
        link.connect(key(PAIRED))
            .press(REMOTE, Key::K)
            .nonsense()
            .press(REMOTE, Key::J)
            .hang_up();
    });

    assert_eq!(
        mac.delivered(),
        vec![
            EventKind::KeyDown {
                device: here(REMOTE),
                key: Key::K
            },
            EventKind::DeviceDetached(here(REMOTE)),
        ]
    );
    assert_eq!(mac.link_closed().len(), 1);
}

#[test]
fn one_source_at_a_time_and_the_next_one_is_served() {
    // A link that stopped after the first source would be a keyboard that works
    // until the Windows machine reboots.
    let mac = served(paired_list(), |link| {
        link.connect(key(PAIRED)).press(REMOTE, Key::K).hang_up();
        link.connect(key(PAIRED)).press(REMOTE, Key::J).hang_up();
    });

    assert_eq!(
        mac.delivered(),
        vec![
            EventKind::KeyDown {
                device: here(REMOTE),
                key: Key::K
            },
            EventKind::DeviceDetached(here(REMOTE)),
            EventKind::KeyDown {
                device: here(REMOTE),
                key: Key::J
            },
            EventKind::DeviceDetached(here(REMOTE)),
        ]
    );
}

#[test]
fn a_connection_that_never_became_a_session_does_not_stop_the_link() {
    // A handshake fails for a stranger, a wrong pattern or a truncated read, and
    // none of those is a reason to stop taking connections — a link that gave up
    // would be a keyboard that stops working because somebody port-scanned the
    // machine.
    let mac = served(paired_list(), |link| {
        link.rejected();
        link.connect(key(PAIRED)).press(REMOTE, Key::K).hang_up();
    });

    assert_eq!(
        mac.delivered(),
        vec![
            EventKind::KeyDown {
                device: here(REMOTE),
                key: Key::K
            },
            EventKind::DeviceDetached(here(REMOTE)),
        ]
    );
}

#[test]
fn input_stops_when_the_converter_has_gone() {
    // The events go to the loop that converts them, and nothing is on the other end
    // of that any more. Reading more from the source would be collecting keystrokes
    // to drop, and the source would have no way to know.
    let mac = served(paired_list(), |link| {
        link.converter_stopped();
        link.connect(key(PAIRED))
            .press(REMOTE, Key::K)
            .press(REMOTE, Key::J)
            .hang_up();
    });

    assert_eq!(mac.frames_read(), 1);
}
