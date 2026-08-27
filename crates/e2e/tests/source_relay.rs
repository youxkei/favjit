//! What the source relays, and what it leaves on the machine it was typed on.
//!
//! The source converts nothing (ADR-0003), but relaying is not the same as
//! repeating everything the platform says: Windows describes a held key as a
//! stream of presses where a keyboard describes it as one, and it says what a
//! mouse button just did rather than what is held. What crosses the link is the
//! input, once — and this is where the difference is written down.
//!
//! It is here rather than in `host-windows` on purpose. The rules below are about
//! what the sink is told, not about what Windows said, so the host has nothing to
//! decide and this suite can check all of it.

use favjit_engine::link::Attached;
use favjit_engine::link::Message;
use favjit_engine::source::{self, Request};
use favjit_engine::{Buttons, DeviceId, Key, PointerReport};
use favjit_host::source::Driving;
use favjit_host::EventKind;
use favjit_host_sim::{identity, SimSource};

/// The sink this machine is pinned to.
const SINK: u8 = 2;

/// A whole run that relays and suppresses, which is what the flags below are
/// about: everything here is a question about what crosses the link.
fn relaying(host: &mut SimSource) {
    source::run(&Request::Relaying, false, host, None);
}

/// A machine that has been asked to send its keyboard over, before anything else
/// happens on it.
///
/// A run comes up with the keyboard on the machine it is running on and sends nothing
/// until it is asked to (ADR-0013), so every question here about what crosses the link
/// starts with the ask. Which way it was asked for is `switching.rs`'s subject, not this
/// one's.
fn sending() -> SimSource {
    let mut host = SimSource::default();
    host.asked_for(Driving::TheSink);
    host
}

const KEYBOARD: DeviceId = DeviceId(1);
const OTHER: DeviceId = DeviceId(2);
const MOUSE: DeviceId = DeviceId(3);

/// The keyboards these scripts attach, by the vendor and product a rule names
/// them with.
const TRACKPOINT: (u16, u16) = (0x17ef, 0x60e1);
const MOUSE_KEYBOARD: (u16, u16) = (0x046d, 0xc52b);

/// What the sink is told when the run announces one of them.
///
/// Written out here rather than taken from the machine: what the machine
/// presents is an interface path, and reading a vendor and a product out of one
/// is the run's own doing — an expectation built by the same reader would pass
/// on a path no rule could match (ADR-0006).
fn keyboard() -> Attached {
    Attached {
        device: KEYBOARD,
        is_built_in: false,
        vendor_id: Some(TRACKPOINT.0),
        product_id: Some(TRACKPOINT.1),
    }
}

fn other() -> Attached {
    Attached {
        device: OTHER,
        is_built_in: false,
        vendor_id: Some(MOUSE_KEYBOARD.0),
        product_id: Some(MOUSE_KEYBOARD.1),
    }
}

/// Attach the keyboard [`keyboard`] is the announcement of.
fn attach_keyboard(host: &mut SimSource) {
    host.attach_external(KEYBOARD, TRACKPOINT.0, TRACKPOINT.1);
}

/// Attach the keyboard [`other`] is the announcement of.
fn attach_other(host: &mut SimSource) {
    host.attach_external(OTHER, MOUSE_KEYBOARD.0, MOUSE_KEYBOARD.1);
}

/// Everything that reached the link, without the timestamps.
fn sent(host: &SimSource) -> Vec<Message> {
    host.sent().iter().map(|sent| sent.message).collect()
}

fn down(device: DeviceId, key: Key) -> Message {
    Message::KeyDown { device, key }
}

fn up(device: DeviceId, key: Key) -> Message {
    Message::KeyUp { device, key }
}

#[test]
fn a_key_held_down_is_relayed_as_one_press() {
    // Windows delivers a held key as a stream of presses. Only the first is the
    // key going down: the rest are its auto-repeat, and macOS produces the
    // repeats for whatever it is holding (`docs/platform/macos/let-the-machine-produce-the-key-repeats.md`), so relaying them would put
    // two repeat sources on one key.
    let mut host = sending();
    host.pinned_to(identity(SINK));
    attach_keyboard(&mut host);
    host.press(KEYBOARD, Key::J);
    host.press(KEYBOARD, Key::J);
    host.press(KEYBOARD, Key::J);
    host.release(KEYBOARD, Key::J);

    relaying(&mut host);

    assert_eq!(
        sent(&host),
        vec![
            Message::DeviceAttached(keyboard()),
            down(KEYBOARD, Key::J),
            up(KEYBOARD, Key::J),
        ]
    );
}

#[test]
fn a_release_with_no_press_behind_it_is_not_relayed() {
    // A key that was already held when the source started reading. The sink
    // never saw it go down, and a release it cannot match is one that would go
    // looking through its held keys for something that was never there.
    let mut host = sending();
    host.pinned_to(identity(SINK));
    attach_keyboard(&mut host);
    host.release(KEYBOARD, Key::J);
    host.tap(KEYBOARD, Key::K);

    relaying(&mut host);

    assert_eq!(
        sent(&host),
        vec![
            Message::DeviceAttached(keyboard()),
            down(KEYBOARD, Key::K),
            up(KEYBOARD, Key::K),
        ]
    );
}

#[test]
fn several_keys_are_held_at_once() {
    // Ordinary chording. One slot for what is held would make the second key
    // press look like the first one repeating.
    let mut host = sending();
    host.pinned_to(identity(SINK));
    attach_keyboard(&mut host);
    host.press(KEYBOARD, Key::LeftShift);
    host.press(KEYBOARD, Key::J);
    host.release(KEYBOARD, Key::J);
    host.release(KEYBOARD, Key::LeftShift);

    relaying(&mut host);

    assert_eq!(
        sent(&host),
        vec![
            Message::DeviceAttached(keyboard()),
            down(KEYBOARD, Key::LeftShift),
            down(KEYBOARD, Key::J),
            up(KEYBOARD, Key::J),
            up(KEYBOARD, Key::LeftShift),
        ]
    );
}

#[test]
fn each_keyboard_holds_its_own_keys() {
    // The same key on two keyboards is two keys. Held keys kept in one set would
    // make the second keyboard's press look like the first keyboard's repeat, and
    // then the first keyboard's release would let go of both.
    let mut host = sending();
    host.pinned_to(identity(SINK));
    attach_keyboard(&mut host);
    attach_other(&mut host);
    host.press(KEYBOARD, Key::J);
    host.press(OTHER, Key::J);
    host.release(KEYBOARD, Key::J);
    host.release(OTHER, Key::J);

    relaying(&mut host);

    assert_eq!(
        sent(&host),
        vec![
            Message::DeviceAttached(keyboard()),
            Message::DeviceAttached(other()),
            down(KEYBOARD, Key::J),
            down(OTHER, Key::J),
            up(KEYBOARD, Key::J),
            up(OTHER, Key::J),
        ]
    );
}

#[test]
fn a_keyboard_that_went_away_and_came_back_is_holding_nothing() {
    // The sink releases a departed device's keys itself (ADR-0002), so after the
    // detach nothing is held at either end. A source that still thought the key
    // was down would drop the next press of it, and that key would do nothing
    // until some other key had been pressed.
    let mut host = sending();
    host.pinned_to(identity(SINK));
    attach_keyboard(&mut host);
    host.press(KEYBOARD, Key::J);
    host.detach(KEYBOARD);
    attach_keyboard(&mut host);
    host.press(KEYBOARD, Key::J);
    host.release(KEYBOARD, Key::J);

    relaying(&mut host);

    assert_eq!(
        sent(&host),
        vec![
            Message::DeviceAttached(keyboard()),
            down(KEYBOARD, Key::J),
            Message::DeviceDetached(KEYBOARD),
            Message::DeviceAttached(keyboard()),
            down(KEYBOARD, Key::J),
            up(KEYBOARD, Key::J),
        ]
    );
}

#[test]
fn the_keyboards_are_announced_again_on_a_new_link() {
    // A session that ended took the sink's knowledge of these keyboards with it:
    // it releases what they were holding and forgets them. Without saying they
    // are here again, every key from them arrives at a sink that has no device to
    // read its rules against — which converts nothing, so the layout silently
    // stops applying to the machine input is coming from.
    let mut host = sending();
    host.pinned_to(identity(SINK));
    attach_keyboard(&mut host);
    host.press(KEYBOARD, Key::J);
    host.release(KEYBOARD, Key::J);
    host.link_gone();
    // The message this one produces is the one that finds out the link has gone.
    host.tap(KEYBOARD, Key::K);
    host.link_back();
    host.tap(KEYBOARD, Key::L);

    relaying(&mut host);

    assert_eq!(
        sent(&host),
        vec![
            Message::DeviceAttached(keyboard()),
            down(KEYBOARD, Key::J),
            up(KEYBOARD, Key::J),
            Message::DeviceAttached(keyboard()),
            down(KEYBOARD, Key::L),
            up(KEYBOARD, Key::L),
        ]
    );
}

#[test]
fn nothing_held_on_the_old_link_is_held_on_the_new_one() {
    // A key held while the link dropped is one the sink has already released, and
    // the finger is still on it: Windows goes on delivering its auto-repeat. The
    // next of those is what the new link should carry as the key going down —
    // otherwise the key does nothing until it is released and pressed again.
    let mut host = sending();
    host.pinned_to(identity(SINK));
    attach_keyboard(&mut host);
    host.press(KEYBOARD, Key::J);
    host.link_gone();
    host.press(KEYBOARD, Key::K);
    host.link_back();
    host.press(KEYBOARD, Key::J);
    host.release(KEYBOARD, Key::J);

    relaying(&mut host);

    assert_eq!(
        sent(&host),
        vec![
            Message::DeviceAttached(keyboard()),
            down(KEYBOARD, Key::J),
            Message::DeviceAttached(keyboard()),
            down(KEYBOARD, Key::J),
            up(KEYBOARD, Key::J),
        ]
    );
}

#[test]
fn nothing_is_said_over_a_new_link_until_there_is_something_to_relay() {
    // The announcement rides in front of the first message the new link carries,
    // so an event that turns out not to be worth relaying carries nothing — here
    // the release of a key that was held when the link dropped, which the sink has
    // already let go of. A device announced off the back of a dropped event would
    // be a message the link carried for nothing.
    let mut host = sending();
    host.pinned_to(identity(SINK));
    attach_keyboard(&mut host);
    host.press(KEYBOARD, Key::J);
    host.link_gone();
    // The message this one produces is the one that finds out the link has gone.
    host.press(KEYBOARD, Key::K);
    host.link_back();
    host.release(KEYBOARD, Key::J);

    relaying(&mut host);

    assert_eq!(
        sent(&host),
        vec![Message::DeviceAttached(keyboard()), down(KEYBOARD, Key::J)]
    );
}

#[test]
fn one_keyboard_going_away_leaves_another_holding_what_it_held() {
    // A detach is per device, and so is what is held. Forgetting more than the
    // one that went would make the other keyboard's next press look like a fresh
    // one, and its eventual release would let go of nothing.
    let mut host = sending();
    host.pinned_to(identity(SINK));
    attach_keyboard(&mut host);
    attach_other(&mut host);
    host.press(KEYBOARD, Key::J);
    host.press(OTHER, Key::J);
    host.detach(KEYBOARD);
    // Still held on the other keyboard, so this is its auto-repeat.
    host.press(OTHER, Key::J);
    host.release(OTHER, Key::J);

    relaying(&mut host);

    assert_eq!(
        sent(&host),
        vec![
            Message::DeviceAttached(keyboard()),
            Message::DeviceAttached(other()),
            down(KEYBOARD, Key::J),
            down(OTHER, Key::J),
            Message::DeviceDetached(KEYBOARD),
            up(OTHER, Key::J),
        ]
    );
}

#[test]
fn a_pointer_report_that_says_nothing_is_not_relayed() {
    // A mouse that has not moved, is not scrolling and is holding nothing. Every
    // message given to the link is a message the link carries, and a keyboard's
    // worth of latency is what a mouse sitting still would spend it on.
    let mut host = sending();
    host.pinned_to(identity(SINK));
    host.pointer(MOUSE, PointerReport::default());
    host.pointer(MOUSE, PointerReport::moved(1, -1));
    host.pointer(MOUSE, PointerReport::default());

    relaying(&mut host);

    assert_eq!(
        sent(&host),
        vec![Message::Pointer {
            device: MOUSE,
            report: PointerReport::moved(1, -1)
        }]
    );
}

#[test]
fn a_button_changing_with_nothing_else_in_the_report_is_relayed() {
    // A click is a press and a release with no movement between them. Either one
    // dropped for saying nothing would leave the button held on the other
    // machine, which is the pointer's version of a stuck modifier — and the
    // release is the half that matters.
    let held = PointerReport {
        buttons: Buttons::NONE.with(1),
        ..PointerReport::default()
    };
    let mut host = sending();
    host.pinned_to(identity(SINK));
    host.pointer(MOUSE, held);
    host.pointer(MOUSE, held);
    host.pointer(MOUSE, PointerReport::default());

    relaying(&mut host);

    assert_eq!(
        sent(&host),
        vec![
            Message::Pointer {
                device: MOUSE,
                report: held
            },
            Message::Pointer {
                device: MOUSE,
                report: PointerReport::default()
            },
        ],
        "the press and the release, and not the report that repeated the press"
    );
}

#[test]
fn a_device_the_sink_was_never_told_about_is_not_reported_as_gone() {
    // A mouse is not in the sink's device list: its reports carry no key for a
    // rule to be about, so nothing announces it. Saying it has gone would be an
    // event about a device the sink has no record of — and it is what releases a
    // device's held keys there, which is not a thing to ask for twice.
    let mut host = sending();
    host.pinned_to(identity(SINK));
    host.pointer(MOUSE, PointerReport::moved(2, 2));
    host.detach(MOUSE);

    relaying(&mut host);

    assert_eq!(
        sent(&host),
        vec![Message::Pointer {
            device: MOUSE,
            report: PointerReport::moved(2, 2)
        }]
    );
}

#[test]
fn a_button_held_when_the_link_dropped_is_released_over_the_new_one() {
    // The sink releases a departed device's *keys*; it has nothing to say about a
    // button, so one that was down is still down there. Coming back with no
    // record of it would make the report that lets go look like no change at all,
    // and the button would stay down for good.
    let held = PointerReport {
        buttons: Buttons::NONE.with(1),
        ..PointerReport::default()
    };
    let mut host = sending();
    host.pinned_to(identity(SINK));
    host.pointer(MOUSE, held);
    host.link_gone();
    // The message this one produces is the one that finds out the link has gone.
    host.press(KEYBOARD, Key::K);
    host.link_back();
    host.pointer(MOUSE, PointerReport::default());

    relaying(&mut host);

    assert_eq!(
        sent(&host),
        vec![
            Message::Pointer {
                device: MOUSE,
                report: held
            },
            Message::Pointer {
                device: MOUSE,
                report: PointerReport::default()
            },
        ]
    );
}

#[test]
fn each_pointer_is_compared_against_its_own_last_report() {
    // Two mice, one holding a button. A single record of what is held would make
    // the still mouse's empty report look like the other one letting go.
    let held = PointerReport {
        buttons: Buttons::NONE.with(1),
        ..PointerReport::default()
    };
    let mut host = sending();
    host.pinned_to(identity(SINK));
    host.pointer(MOUSE, held);
    host.pointer(OTHER, PointerReport::default());
    host.pointer(MOUSE, held);

    relaying(&mut host);

    assert_eq!(
        sent(&host),
        vec![Message::Pointer {
            device: MOUSE,
            report: held
        }]
    );
}

#[test]
fn a_report_that_is_not_a_key_reaches_the_other_machine_as_nothing() {
    // Two of the things arriving on this stream are not a key, and each carries
    // a make code that would otherwise be read as one. The third — the pause
    // key, which reports left control's make code behind an `E1` prefix — is a
    // `RAWKEYBOARD` spelling and cannot reach a hook, so what says it is not a
    // key is `favjit_hid::scancode`'s own test rather than a run.
    let mut host = sending();
    host.pinned_to(identity(SINK));
    attach_keyboard(&mut host);
    for kind in [
        // Windows saying it has no scan code for this.
        EventKind::HookedKey {
            device: KEYBOARD,
            make_code: 0x00,
            vkey: 0x41,
            flags: 0,
        },
        // The filler beside an extended key.
        EventKind::HookedKey {
            device: KEYBOARD,
            make_code: 0x2a,
            vkey: favjit_hid::scancode::VK_FILLER,
            flags: 0,
        },
    ] {
        host.script(kind);
    }

    relaying(&mut host);

    assert_eq!(
        sent(&host),
        vec![Message::DeviceAttached(keyboard())],
        "the keyboard's own announcement crossed and not one of these did"
    );
}

#[test]
fn right_shift_crosses_as_right_shift_and_not_as_a_position_with_no_name() {
    // Windows reports it with the extended bit set and right shift is not an
    // extended position: read as one it is `E0 0x36`, which nothing names, so
    // the key would do nothing at all on the Mac.
    let mut host = sending();
    host.pinned_to(identity(SINK));
    attach_keyboard(&mut host);
    host.script(EventKind::HookedKey {
        device: KEYBOARD,
        make_code: 0x36,
        vkey: favjit_hid::scancode::VK_RSHIFT,
        flags: favjit_hid::scancode::LLKHF_EXTENDED,
    });

    relaying(&mut host);

    assert_eq!(
        sent(&host),
        vec![
            Message::DeviceAttached(keyboard()),
            Message::KeyDown {
                device: KEYBOARD,
                key: Key::RightShift
            }
        ]
    );
}

#[test]
fn the_extended_bit_still_separates_the_two_keys_that_share_a_number() {
    // Cleared for right shift alone. Dropped for everything would put right
    // control on the left-hand key, and the two are not interchangeable — one is
    // under the little finger and the other is not.
    let mut host = sending();
    host.pinned_to(identity(SINK));
    attach_keyboard(&mut host);
    host.script(EventKind::HookedKey {
        device: KEYBOARD,
        make_code: 0x1d,
        vkey: 0x11,
        flags: favjit_hid::scancode::LLKHF_EXTENDED,
    });

    relaying(&mut host);

    assert_eq!(
        sent(&host),
        vec![
            Message::DeviceAttached(keyboard()),
            Message::KeyDown {
                device: KEYBOARD,
                key: Key::RightControl
            }
        ]
    );
}
