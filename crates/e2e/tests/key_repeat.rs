//! What a held key does over time.
//!
//! These runs are given rates to repeat at, so what they pin is the shape favjit
//! produces when it is the one producing them: the queue carries one HID value per
//! press and nothing while a key is held, so there is nothing to pass on and the
//! shape is favjit's behaviour rather than a platform note. On macOS the repeats
//! come from the OS instead (ADR-0013), and a run there asks for none.
//!
//! The clock is the script's. Nothing here waits.

use core::time::Duration;

use favjit_core::sink::{self, Repeat, Request};
use favjit_core::{DeviceId, DeviceInfo, Injected, Key, Layout, ModifierKeys};
use favjit_host_sim::SimHost;

const BUILT_IN: DeviceId = DeviceId(1);

/// Round numbers rather than the machine's own 250ms/33.3ms, so the arithmetic in
/// each test is readable and a change to the defaults cannot quietly rewrite what
/// these tests mean.
const INITIAL: Duration = Duration::from_millis(250);
const INTERVAL: Duration = Duration::from_millis(100);

/// A whole run of the program, on a simulated Mac.
///
/// Through the program rather than the converter's loop, because that is what a
/// person starts: a suite that called the loop directly would pass while the run
/// that reaches it was broken.
fn run(script: impl FnOnce(&mut SimHost)) -> Vec<Injected> {
    let mut mac = SimHost::new();
    mac.attach(DeviceInfo::built_in(BUILT_IN));
    script(&mut mac);
    sink::run(
        &Request::Injecting { listen: false },
        Layout::dudrack(),
        Some(Repeat {
            initial: INITIAL,
            interval: INTERVAL,
        }),
        &mut mac,
        None,
    );
    mac.injected()
}

fn down(key: Key) -> Injected {
    Injected::KeyDown {
        key,
        modifiers: ModifierKeys::NONE,
    }
}

fn up(key: Key) -> Injected {
    Injected::KeyUp {
        key,
        modifiers: ModifierKeys::NONE,
    }
}

/// One repeat: the key let go and pressed again.
///
/// Both halves, because a key the OS already believes is down and is told about
/// again has been told nothing — the release is what makes the second press a
/// press (`docs/platform/macos/key-repeat.md`).
fn again(key: Key) -> [Injected; 2] {
    [up(key), down(key)]
}

/// A key pressed, repeated `repeats` times, and let go.
fn held(key: Key, repeats: usize) -> Vec<Injected> {
    let mut out = vec![down(key)];
    for _ in 0..repeats {
        out.extend(again(key));
    }
    out.push(up(key));
    out
}

/// Physical `s` on the built-in keyboard, which Dudrack types as `o`.
const S_TYPES: Key = Key::O;
/// Physical `d`, which types `e`.
const D_TYPES: Key = Key::E;

#[test]
fn a_tap_is_sent_once() {
    let injected = run(|host| {
        host.press(BUILT_IN, Key::S)
            .advance(Duration::from_millis(100))
            .release(BUILT_IN, Key::S);
    });

    assert_eq!(injected, held(S_TYPES, 0));
}

#[test]
fn a_key_held_past_the_initial_delay_is_sent_again() {
    let injected = run(|host| {
        host.press(BUILT_IN, Key::S)
            .advance(Duration::from_millis(260))
            .release(BUILT_IN, Key::S);
    });

    assert_eq!(injected, held(S_TYPES, 1));
}

#[test]
fn a_repeat_is_the_key_let_go_and_pressed_again() {
    // Spelled out once, because it is what makes a repeat visible at all: an
    // OS told twice that a key is down has been told it once.
    let injected = run(|host| {
        host.press(BUILT_IN, Key::S)
            .advance(Duration::from_millis(260))
            .release(BUILT_IN, Key::S);
    });

    assert_eq!(
        injected,
        vec![down(S_TYPES), up(S_TYPES), down(S_TYPES), up(S_TYPES)]
    );
}

#[test]
fn the_repeats_come_at_the_interval_after_the_first() {
    // Held for 550ms: the first repeat at 250, then 350, 450 and 550.
    let injected = run(|host| {
        host.press(BUILT_IN, Key::S)
            .advance(Duration::from_millis(550))
            .release(BUILT_IN, Key::S);
    });

    assert_eq!(injected, held(S_TYPES, 4));
}

#[test]
fn releasing_the_key_stops_the_repeat() {
    // The release is followed by a long silence and then another key, which is
    // what gives a repeat still running the chance to show up.
    let injected = run(|host| {
        host.press(BUILT_IN, Key::S)
            .advance(Duration::from_millis(300))
            .release(BUILT_IN, Key::S)
            .advance(Duration::from_secs(5))
            .press(BUILT_IN, Key::D)
            .advance(Duration::from_millis(100))
            .release(BUILT_IN, Key::D);
    });

    assert_eq!(
        injected,
        [held(S_TYPES, 1), held(D_TYPES, 0)].concat(),
        "and the second key is not repeating either"
    );
}

#[test]
fn the_key_pressed_last_takes_the_repeat() {
    // `s` is never released: rolling onto another key must move the repeat even
    // while the first finger is still down, or holding a chord would stream the
    // wrong character.
    let injected = run(|host| {
        host.press(BUILT_IN, Key::S)
            .advance(Duration::from_millis(300))
            .press(BUILT_IN, Key::D)
            .advance(Duration::from_millis(300))
            .release(BUILT_IN, Key::D);
    });

    assert_eq!(
        injected,
        [
            vec![down(S_TYPES)],
            again(S_TYPES).to_vec(),
            // `s` stays down at the OS; the repeat moves to `d` and nothing lifts
            // the finger still on it.
            vec![down(D_TYPES)],
            again(D_TYPES).to_vec(),
            vec![up(D_TYPES)],
        ]
        .concat()
    );
}

#[test]
fn a_modifier_neither_repeats_nor_stops_the_repeat() {
    // Shift held down while a key streams: the modifier itself must not repeat,
    // and it must not interrupt what is repeating either.
    let injected = run(|host| {
        host.press(BUILT_IN, Key::S)
            .advance(Duration::from_millis(300))
            .press(BUILT_IN, Key::LeftShift)
            .advance(Duration::from_millis(200))
            .release(BUILT_IN, Key::LeftShift);
    });

    assert_eq!(
        injected,
        [
            vec![down(S_TYPES)],
            again(S_TYPES).to_vec(),
            // Its own event names it: a modifier key's set is the whole of what
            // reaches the OS, so a shift left out of its own press would be a
            // shift nothing ever saw.
            vec![Injected::KeyDown {
                key: Key::LeftShift,
                modifiers: ModifierKeys::of(&[Key::LeftShift]),
            }],
            // The set the press went down with, repeated as it was: a repeat is
            // the same keystroke again, and resolving it afresh would need the
            // rule's consumed and added sets a second time.
            again(S_TYPES).to_vec(),
            again(S_TYPES).to_vec(),
            vec![up(Key::LeftShift)],
        ]
        .concat()
    );
}

#[test]
fn an_undecided_hold_does_not_repeat() {
    // The built-in space bar is shift when held. Held alone past the tap window
    // nothing reaches the OS at all, so there is nothing to repeat — and the
    // repeat clock must not be what settles the hold, because deciding that is
    // the tap window's job.
    let injected = run(|host| {
        host.press(BUILT_IN, Key::Spacebar)
            .advance(Duration::from_secs(2))
            .release(BUILT_IN, Key::Spacebar);
    });

    assert_eq!(injected, Vec::<Injected>::new());
}

#[test]
fn unplugging_the_keyboard_stops_the_repeat() {
    let injected = run(|host| {
        host.press(BUILT_IN, Key::S)
            .advance(Duration::from_millis(300))
            .detach(BUILT_IN)
            .advance(Duration::from_secs(5))
            .probe();
    });

    assert_eq!(
        injected,
        held(S_TYPES, 1),
        "the detach releases it, and nothing repeats after that"
    );
}
