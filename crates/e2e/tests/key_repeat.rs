//! What a held key does over time.
//!
//! These runs are given rates to repeat at, so what they pin is the shape favjit
//! produces when it is the one producing them: the queue carries one HID value per
//! press and nothing while a key is held, so there is nothing to pass on and the
//! shape is favjit's behaviour rather than a platform note. On macOS the repeats
//! come from the OS instead (`docs/platform/macos/let-the-machine-produce-the-key-repeats.md`), and a run there asks for none.
//!
//! The clock is the script's. Nothing here waits.
//!
//! Asserted on the bytes [`SinkHost::send_report`] actually received, decoded
//! back with `favjit-hid`'s own inverse of the encoding it wrote them with —
//! not on `Injected`, which is `engine`'s own record of what it decided rather
//! than anything a device downstream of it ever sees.
//!
//! [`SinkHost::send_report`]: favjit_host::sink::SinkHost::send_report

use core::time::Duration;

use favjit_engine::sink::{self, InputConfig, Repeat, Request};
use favjit_engine::{DeviceId, Key, Layout};
use favjit_hid::page;
use favjit_hid::report::{modifier_bit, Report};
use favjit_hid::usage;
use favjit_host::OutputReport;
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
///
/// Every report the run wrote, decoded back into the state it described —
/// `SimHost` itself keeps only the bytes, the same as a real device would.
fn run(script: impl FnOnce(&mut SimHost)) -> Vec<Report> {
    let mut mac = SimHost::new();
    mac.attach_built_in(BUILT_IN);
    script(&mut mac);
    sink::run(
        &Request::Injecting { listen: false },
        Layout::dudrack(),
        Some(Repeat {
            initial: INITIAL,
            interval: INTERVAL,
        }),
        InputConfig::default(),
        &mut mac,
        None,
    );
    mac.reports_while_reading()
        .iter()
        .map(|(_, report, bytes)| {
            assert_eq!(
                *report,
                OutputReport::Keyboard,
                "every key in this file types, and never a control"
            );
            Report::from_bytes(bytes).expect("the bytes of a keyboard report")
        })
        .collect()
}

/// One key event, the level a script states it and the level a report
/// describes it are read back at: which key, what else the OS was told is
/// down alongside it, and whether this is the press or the release.
#[derive(Debug, Clone, Copy)]
struct Stroke {
    key: Key,
    modifiers: u8,
    down: bool,
}

fn down(key: Key) -> Stroke {
    Stroke {
        key,
        modifiers: 0,
        down: true,
    }
}

fn up(key: Key) -> Stroke {
    Stroke {
        key,
        modifiers: 0,
        down: false,
    }
}

/// One repeat: the key let go and pressed again.
///
/// Both halves, because a key the OS already believes is down and is told about
/// again has been told nothing — the release is what makes the second press a
/// press (`docs/platform/macos/key-repeat.md`).
fn again(key: Key) -> [Stroke; 2] {
    [up(key), down(key)]
}

/// A key pressed, repeated `repeats` times, and let go.
fn held(key: Key, repeats: usize) -> Vec<Stroke> {
    let mut out = vec![down(key)];
    for _ in 0..repeats {
        out.extend(again(key));
    }
    out.push(up(key));
    out
}

/// The reports a real keyboard's report would carry for this sequence of
/// strokes, one per stroke and in order — the same state
/// [`favjit_hid::report::Report::press`]/`release` and the modifier byte
/// describe on the wire, carried forward exactly as one report does: nothing
/// here is rebuilt from scratch between strokes.
fn expected(strokes: &[Stroke]) -> Vec<Report> {
    let mut report = Report::default();
    strokes
        .iter()
        .map(|stroke| {
            report.modifiers = stroke.modifiers;
            match modifier_bit(stroke.key) {
                // A modifier key's own report names no usage: the byte just set is
                // the whole of what it says.
                Some(_) => {}
                None => {
                    let (on_page, code) = usage::of(stroke.key).expect("a key with a usage");
                    assert_eq!(on_page, page::KEYBOARD_OR_KEYPAD, "not a plain key");
                    match stroke.down {
                        true => report.press(code),
                        false => report.release(code),
                    }
                }
            }
            report
        })
        .collect()
}

/// Physical `s` on the built-in keyboard, which Dudrack types as `o`.
const S_TYPES: Key = Key::O;
/// Physical `d`, which types `e`.
const D_TYPES: Key = Key::E;

#[test]
fn a_tap_is_sent_once() {
    let reports = run(|host| {
        host.press(BUILT_IN, Key::S)
            .advance(Duration::from_millis(100))
            .release(BUILT_IN, Key::S);
    });

    assert_eq!(reports, expected(&held(S_TYPES, 0)));
}

#[test]
fn a_key_held_past_the_initial_delay_is_sent_again() {
    let reports = run(|host| {
        host.press(BUILT_IN, Key::S)
            .advance(Duration::from_millis(260))
            .release(BUILT_IN, Key::S);
    });

    assert_eq!(reports, expected(&held(S_TYPES, 1)));
}

#[test]
fn a_repeat_is_the_key_let_go_and_pressed_again() {
    // Spelled out once, because it is what makes a repeat visible at all: an
    // OS told twice that a key is down has been told it once.
    let reports = run(|host| {
        host.press(BUILT_IN, Key::S)
            .advance(Duration::from_millis(260))
            .release(BUILT_IN, Key::S);
    });

    assert_eq!(
        reports,
        expected(&[down(S_TYPES), up(S_TYPES), down(S_TYPES), up(S_TYPES)])
    );
}

#[test]
fn the_repeats_come_at_the_interval_after_the_first() {
    // Held for 550ms: the first repeat at 250, then 350, 450 and 550.
    let reports = run(|host| {
        host.press(BUILT_IN, Key::S)
            .advance(Duration::from_millis(550))
            .release(BUILT_IN, Key::S);
    });

    assert_eq!(reports, expected(&held(S_TYPES, 4)));
}

#[test]
fn releasing_the_key_stops_the_repeat() {
    // The release is followed by a long silence and then another key, which is
    // what gives a repeat still running the chance to show up.
    let reports = run(|host| {
        host.press(BUILT_IN, Key::S)
            .advance(Duration::from_millis(300))
            .release(BUILT_IN, Key::S)
            .advance(Duration::from_secs(5))
            .press(BUILT_IN, Key::D)
            .advance(Duration::from_millis(100))
            .release(BUILT_IN, Key::D);
    });

    assert_eq!(
        reports,
        expected(&[held(S_TYPES, 1), held(D_TYPES, 0)].concat()),
        "and the second key is not repeating either"
    );
}

#[test]
fn the_key_pressed_last_takes_the_repeat() {
    // `s` is never released: rolling onto another key must move the repeat even
    // while the first finger is still down, or holding a chord would stream the
    // wrong character.
    let reports = run(|host| {
        host.press(BUILT_IN, Key::S)
            .advance(Duration::from_millis(300))
            .press(BUILT_IN, Key::D)
            .advance(Duration::from_millis(300))
            .release(BUILT_IN, Key::D);
    });

    assert_eq!(
        reports,
        expected(
            &[
                vec![down(S_TYPES)],
                again(S_TYPES).to_vec(),
                // `s` stays down at the OS; the repeat moves to `d` and nothing lifts
                // the finger still on it.
                vec![down(D_TYPES)],
                again(D_TYPES).to_vec(),
                vec![up(D_TYPES)],
            ]
            .concat()
        )
    );
}

#[test]
fn a_modifier_neither_repeats_nor_stops_the_repeat() {
    // Shift held down while a key streams: the modifier itself must not repeat,
    // and it must not interrupt what is repeating either.
    let reports = run(|host| {
        host.press(BUILT_IN, Key::S)
            .advance(Duration::from_millis(300))
            .press(BUILT_IN, Key::LeftShift)
            .advance(Duration::from_millis(200))
            .release(BUILT_IN, Key::LeftShift);
    });

    assert_eq!(
        reports,
        expected(
            &[
                vec![down(S_TYPES)],
                again(S_TYPES).to_vec(),
                // Its own event names it: a modifier key's set is the whole of what
                // reaches the OS, so a shift left out of its own press would be a
                // shift nothing ever saw.
                vec![Stroke {
                    key: Key::LeftShift,
                    modifiers: modifier_bit(Key::LeftShift).expect("a modifier key"),
                    down: true,
                }],
                // The set the press went down with, repeated as it was: a repeat is
                // the same keystroke again, and resolving it afresh would need the
                // rule's consumed and added sets a second time.
                again(S_TYPES).to_vec(),
                again(S_TYPES).to_vec(),
                vec![up(Key::LeftShift)],
            ]
            .concat()
        )
    );
}

#[test]
fn an_undecided_hold_does_not_repeat() {
    // The built-in space bar is shift when held. Held alone past the tap window
    // nothing reaches the OS at all, so there is nothing to repeat — and the
    // repeat clock must not be what settles the hold, because deciding that is
    // the tap window's job.
    let reports = run(|host| {
        host.press(BUILT_IN, Key::Spacebar)
            .advance(Duration::from_secs(2))
            .release(BUILT_IN, Key::Spacebar);
    });

    assert_eq!(reports, Vec::<Report>::new());
}

#[test]
fn unplugging_the_keyboard_stops_the_repeat() {
    let reports = run(|host| {
        host.press(BUILT_IN, Key::S)
            .advance(Duration::from_millis(300))
            .detach(BUILT_IN)
            .advance(Duration::from_secs(5))
            .probe();
    });

    assert_eq!(
        reports,
        expected(&held(S_TYPES, 1)),
        "the detach releases it, and nothing repeats after that"
    );
}

#[test]
fn a_wake_up_that_arrives_after_the_key_has_gone_repeats_nothing() {
    // The wake-up and whatever took the key away can both be waiting by the time
    // either is read, and the one that gets there second does not get to assume
    // the other was handled first. A repeat that carried on from here would be a
    // key held down at the OS by a keyboard that is not plugged in — the stuck
    // key ADR-0002 puts on this side.
    //
    // The keyboard is detached at the moment the repeat is due rather than before
    // it, which is what puts both in flight at once.
    let reports = run(|host| {
        host.press(BUILT_IN, Key::S)
            .advance(INITIAL)
            .detach(BUILT_IN)
            .advance(INTERVAL * 4)
            .probe();
    });

    assert_eq!(
        reports,
        expected(&held(S_TYPES, 1)),
        "the repeat that was due lands, and nothing after it does"
    );
}
