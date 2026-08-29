//! Both machines running, one after the other, with nothing between them but what
//! would cross a socket.
//!
//! `link_relay.rs` compares the two sides by handing the source's messages to the
//! sink's own stream, which is the right shape for asking whether conversion is the
//! same either way. `link_lifetime.rs` serves a link inside a run, with the other
//! end scripted by hand. Neither has a *source* on the other end — so the frames a
//! real forwarding run decided to send are read by nothing, and the device
//! renaming, the refusal of an unpaired machine and the release at the end of a
//! session are all checked against an idea of what a source would send.

use favjit_core::link::from_source;
use favjit_core::pairing::hex;
use favjit_core::sink::{self, Request};
use favjit_core::{
    source, DeviceId, DeviceInfo, EventKind, Injected, Key, Layout, ModifierKeys as M,
};
use favjit_host_sim::{SimHost, SimLink, SimSource};

/// The keyboard on the Windows side, as that machine numbers it.
const KEYBOARD: DeviceId = DeviceId(3);

/// The Windows machine's identity, as the Mac has pinned it.
const PAIRED: [u8; 32] = [7; 32];

fn keyboard() -> DeviceInfo {
    DeviceInfo::external(KEYBOARD, 1234, 5678)
}

fn converting() -> Request {
    Request::Injecting { listen: true }
}

fn forwarding() -> source::Request {
    source::Request::Relaying
}

/// Run a whole forwarding run on the Windows machine, then a whole converting run
/// on the Mac with what it sent on the other end of the Mac's link.
///
/// Two runs and nothing else: the link is served inside the Mac's, which is where
/// it is served on a real machine, so the source's frames reach the converter the
/// way they would rather than by being scripted into its stream.
fn both_ends(peer: [u8; 32], script: impl Fn(&mut SimSource)) -> (Vec<EventKind>, Vec<Injected>) {
    let mut windows = SimSource::new();
    script(&mut windows);
    source::run(&forwarding(), &mut windows);

    let mut link = SimLink::new(format!("{}\n", hex(&PAIRED)));
    link.connect(peer.to_vec());
    link.relay(windows.sent());

    let mut mac = SimHost::new().with_link(link);
    sink::run(&converting(), Layout::dudrack(), None, &mut mac, None);
    (mac.delivered(), mac.injected())
}

/// The same script on a keyboard plugged into the Mac, for comparing against.
fn typed_here(script: impl Fn(&mut SimHost)) -> Vec<Injected> {
    let mut mac = SimHost::new();
    mac.attach(keyboard());
    script(&mut mac);
    sink::run(&converting(), Layout::dudrack(), None, &mut mac, None);
    mac.injected()
}

#[test]
fn a_keystroke_typed_on_windows_comes_out_as_one_typed_here() {
    // The whole topology through every piece of it at once, compared against the
    // one thing it has to equal (ADR-0003). Compared rather than named, because
    // what a key converts to is the layout's to say and this is about the path:
    // naming it here would make a rule change fail in two places.
    let (_, over_the_link) = both_ends(PAIRED, |windows| {
        windows.attach(keyboard());
        windows.tap(KEYBOARD, Key::International3);
    });
    let here = typed_here(|mac| {
        mac.tap(KEYBOARD, Key::International3);
    });

    assert_eq!(over_the_link, here);
    assert!(!here.is_empty(), "the script should convert to something");
}

#[test]
fn the_windows_keyboard_is_renamed_before_the_rules_see_it() {
    // Both machines number their devices from one, so a Windows keyboard called 3
    // would be converted as whatever the Mac calls 3. The link moves what arrives
    // into a range of its own, and this is the only test where the number the
    // rules read is the one a real source handed over.
    let (delivered, _) = both_ends(PAIRED, |windows| {
        windows.attach(keyboard());
        windows.tap(KEYBOARD, Key::K);
    });

    let devices: Vec<DeviceId> = delivered
        .iter()
        .filter_map(|kind| match kind {
            EventKind::DeviceAttached(info) => Some(info.id),
            EventKind::KeyDown { device, .. } | EventKind::KeyUp { device, .. } => Some(*device),
            _ => None,
        })
        .collect();

    assert!(!devices.is_empty());
    for device in devices {
        assert_eq!(device, from_source(KEYBOARD));
        assert_ne!(device, KEYBOARD, "the number the source used is not reused");
    }
}

#[test]
fn a_source_the_mac_has_not_paired_gets_nothing_through() {
    // Refused by the handshake not completing, before a frame is read — so the
    // keystrokes it relayed reach no rule and nothing is injected (ADR-0004).
    let (delivered, injected) = both_ends([9; 32], |windows| {
        windows.attach(keyboard());
        windows.tap(KEYBOARD, Key::K);
    });

    assert_eq!(delivered, Vec::new(), "nothing reached the converter");
    assert_eq!(injected, Vec::new(), "and nothing reached applications");
}

#[test]
fn a_key_still_held_when_the_session_ends_is_released_on_the_mac() {
    // The failure ADR-0002 puts on the sink, driven from the machine that caused
    // it: a dropped network sends no key-up, so the session ending has to say the
    // source's keyboards are gone and the sink has to let go of what it told the OS
    // was down. Left held, this is a modifier stuck in every application.
    let (_, injected) = both_ends(PAIRED, |windows| {
        windows.attach(keyboard());
        windows.press(KEYBOARD, Key::International3);
    });

    let held = match injected.as_slice() {
        [Injected::KeyDown { key, .. }, Injected::KeyUp { key: up, .. }] if key == up => *key,
        other => panic!("expected one key pressed and released, got {other:?}"),
    };
    assert_eq!(
        injected.last(),
        Some(&Injected::KeyUp {
            key: held,
            modifiers: M::NONE
        }),
        "the last thing applications see is the key being let go"
    );
}
