//! What the MacBook's top row does once favjit holds the keyboard.
//!
//! That row sends `F1` to `F12` and nothing else, and the brightness and volume
//! icons printed on it are the OS's reading of its own keyboard
//! (`docs/platform/macos/hid-input-callbacks.md`). A key taken from that keyboard
//! and handed back through a virtual device has lost that reading, so the icons are
//! the layout's to reproduce — and these pin that it does, on that keyboard and not
//! on the others.

use favjit_core::sink::{self, Request};
use favjit_core::{DeviceId, DeviceInfo, Injected, Key, Layout, ModifierKeys as M};
use favjit_host_sim::SimHost;

/// The MacBook's own keyboard, whose row this is about.
const BUILT_IN: DeviceId = DeviceId(1);
/// An external keyboard, whose top row is printed `F1` and means it.
const EXTERNAL: DeviceId = DeviceId(2);

fn converting() -> Request {
    Request::Injecting { listen: false }
}

fn run(script: impl FnOnce(&mut SimHost)) -> Vec<Injected> {
    let mut host = SimHost::new();
    host.attach(DeviceInfo::built_in(BUILT_IN));
    host.attach(DeviceInfo::external(EXTERNAL, 6127, 24801));
    script(&mut host);
    sink::run(&converting(), Layout::dudrack(), None, &mut host, None);
    host.injected()
}

fn tapped(device: DeviceId, key: Key) -> Vec<Injected> {
    run(|host| {
        host.tap(device, key);
    })
}

fn down(key: Key) -> Injected {
    Injected::KeyDown {
        key,
        modifiers: M::NONE,
    }
}

fn up(key: Key) -> Injected {
    Injected::KeyUp {
        key,
        modifiers: M::NONE,
    }
}

#[test]
fn the_whole_row_is_what_its_icons_say() {
    // The pairing is Karabiner-Elements' own default for this row, which is what
    // the keyboard did before favjit took it.
    for (pressed, expected) in [
        (Key::F1, Key::BrightnessDown),
        (Key::F2, Key::BrightnessUp),
        (Key::F3, Key::MissionControl),
        (Key::F4, Key::Spotlight),
        (Key::F5, Key::Dictation),
        (Key::F6, Key::DoNotDisturb),
        (Key::F7, Key::Rewind),
        (Key::F8, Key::PlayPause),
        (Key::F9, Key::FastForward),
        (Key::F10, Key::Mute),
        (Key::F11, Key::VolumeDown),
        (Key::F12, Key::VolumeUp),
    ] {
        assert_eq!(
            tapped(BUILT_IN, pressed),
            vec![down(expected), up(expected)],
            "{pressed:?} on the built-in keyboard"
        );
    }
}

#[test]
fn the_release_is_converted_with_the_press() {
    // A control the OS is left holding ramps on its own — brightness runs to one
    // end of its range — so the half that matters is the one nothing else can
    // repair.
    let records = tapped(BUILT_IN, Key::F11);
    assert_eq!(records.last(), Some(&up(Key::VolumeDown)));
}

#[test]
fn an_external_keyboards_top_row_is_left_alone() {
    // Printed `F1` there, and a person pressing it means the function key. Nothing
    // about it is scoped to the Mac's own keyboard by accident: it is the only
    // keyboard whose row is printed with the icons.
    assert_eq!(
        tapped(EXTERNAL, Key::F1),
        vec![down(Key::F1), up(Key::F1)],
        "an external F1 passes straight through"
    );
}

#[test]
fn a_keyboard_that_sends_the_control_itself_is_passed_through() {
    // Some keyboards have volume keys of their own and send the control rather
    // than a function key. Nothing converts those, and nothing needs to.
    assert_eq!(
        tapped(EXTERNAL, Key::VolumeUp),
        vec![down(Key::VolumeUp), up(Key::VolumeUp)]
    );
}

#[test]
fn the_row_converts_in_every_layer() {
    // The layers are entered by holding a key on the home row, and the top row is
    // not part of any of them: brightness in the middle of a Henkan hold is still
    // brightness, not the layer's meaning for that position.
    let records = run(|host| {
        host.press(BUILT_IN, Key::RightCommand);
        host.tap(BUILT_IN, Key::F2);
        host.release(BUILT_IN, Key::RightCommand);
    });

    assert!(
        records.contains(&down(Key::BrightnessUp)),
        "the Henkan layer swallowed it: {records:?}"
    );
}
