//! What the TrackPoint keyboard's pointer produces, end to end.
//!
//! A seize is per device and that keyboard is one device, so suppressing its keys
//! takes its pointer with it and the pointer has to come back out through favjit
//! (`docs/platform/macos/input-suppression.md`). These pin what the sink does with
//! a pointer report, including the two places it meets the keyboard state.
//!
//! Checked in the bytes the sink actually wrote, decoded back with
//! `favjit-hid`'s inverse of the encoding — `Injected` is `engine`'s own record
//! of what it decided, not anything the device downstream of it ever sees.

use core::time::Duration;

use favjit_engine::sink::{self, InputConfig, Request, Settings};
use favjit_engine::{pointer::Tuning, Buttons, DeviceId, Key, Layout, PointerReport};
use favjit_hid::report::{modifier_bit, pointing_from_bytes, Report as KeyboardReport};
use favjit_hid::{page, usage};
use favjit_host::{Instant, OutputReport};
use favjit_host_sim::SimHost;

/// Lenovo TrackPoint Keyboard II: the keyboard whose pointer this is about.
const TRACKPOINT: DeviceId = DeviceId(2);
/// The MacBook's own keyboard, which is where the space bar is a tap-hold — the
/// external ones keep their space, so the interaction between a pointer button
/// and a lazy hold can only be reached across two keyboards, and that is the
/// arrangement it happens in.
const BUILT_IN: DeviceId = DeviceId(1);
/// A keyboard no rule names, to pin that a pointer needs none.
const UNKNOWN: DeviceId = DeviceId(9);

/// A whole run of the program, on a simulated Mac.
///
/// Through the program rather than the converter's loop, because that is what a
/// person starts: a suite that called the loop directly would pass while the run
/// that reaches it was broken.
fn converting() -> Request {
    Request::Injecting { listen: false }
}

/// What a run wrote, on whichever page each report landed — a keyboard report
/// and a pointer report both cross this boundary, and the pipeline is free to
/// interleave them in whatever order the events that caused them arrived in.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Written {
    Keyboard(KeyboardReport),
    Pointer(PointerReport),
}

/// Every report a run wrote, decoded onto whichever variant its own page names.
fn decode(reports: &[(Instant, OutputReport, Vec<u8>)]) -> Vec<Written> {
    reports
        .iter()
        .map(|(_, report, bytes)| match *report {
            OutputReport::Keyboard => {
                Written::Keyboard(KeyboardReport::from_bytes(bytes).expect("a keyboard report"))
            }
            OutputReport::Pointing => {
                Written::Pointer(pointing_from_bytes(bytes).expect("a pointing report"))
            }
            // `OutputReport` is `#[non_exhaustive]`, and nothing here ever
            // asks for a control.
            other => panic!("not a keyboard or pointing report: {other:?}"),
        })
        .collect()
}

fn run(script: impl FnOnce(&mut SimHost)) -> Vec<Written> {
    let mut host = SimHost::new();
    host.attach_built_in(BUILT_IN);
    host.attach_external(TRACKPOINT, 6127, 24801);
    script(&mut host);
    sink::run(
        &converting(),
        Layout::dudrack(),
        None,
        InputConfig::default(),
        &mut host,
        None,
    );
    decode(&host.reports_while_reading())
}

/// The report after this sequence of presses (`true`) and releases (`false`).
fn after(steps: &[(Key, bool)]) -> KeyboardReport {
    let mut report = KeyboardReport::default();
    for &(key, down) in steps {
        match modifier_bit(key) {
            Some(bit) => match down {
                true => report.modifiers |= bit,
                false => report.modifiers &= !bit,
            },
            None => {
                let (on_page, code) = usage::of(key).expect("a key with a usage");
                assert_eq!(on_page, page::KEYBOARD_OR_KEYPAD, "not a plain key");
                match down {
                    true => report.press(code),
                    false => report.release(code),
                }
            }
        }
    }
    report
}

#[test]
fn a_pointer_report_is_relayed_as_it_stands() {
    let report = PointerReport::moved(7, -3);
    assert_eq!(
        run(|host| {
            host.pointer(TRACKPOINT, report);
        }),
        vec![Written::Pointer(report)]
    );
}

#[test]
fn the_axes_stay_in_one_report() {
    // Splitting a diagonal into one report per axis would be accelerated as two
    // short movements and travel less far than the same motion of the thumb, so
    // what arrives as one report has to leave as one.
    let reports = run(|host| {
        host.pointer(TRACKPOINT, PointerReport::moved(4, 4));
    });
    assert_eq!(reports.len(), 1);
}

#[test]
fn buttons_and_wheel_are_relayed() {
    // No horizontal wheel: this keyboard's own stick has never been observed to
    // report one (`docs/platform/macos/input-suppression.md`), so a nonzero
    // value here would be a report this hardware cannot make.
    let report = PointerReport {
        dx: 0,
        dy: 0,
        vertical_wheel: -2,
        horizontal_wheel: 0,
        buttons: Buttons::NONE.with(1).with(3),
    };
    assert_eq!(
        run(|host| {
            host.pointer(TRACKPOINT, report);
        }),
        vec![Written::Pointer(report)]
    );
}

#[test]
fn a_button_still_down_when_the_run_ends_is_let_go_of() {
    // A button the OS is left holding is a drag that never finishes, and the
    // pointing report is the only thing that can say otherwise: the keyboard's
    // own release says nothing about a button, and the device outlives this
    // process.
    let mut host = SimHost::new();
    host.attach_external(TRACKPOINT, 6127, 24801);
    host.pointer(
        TRACKPOINT,
        PointerReport {
            buttons: Buttons::NONE.with(1),
            ..PointerReport::default()
        },
    );
    sink::run(
        &converting(),
        Layout::dudrack(),
        None,
        InputConfig::default(),
        &mut host,
        None,
    );

    let converting = host.reports_while_reading().len();
    assert_eq!(
        decode(&host.reports()[converting..]),
        [Written::Pointer(PointerReport::default())],
        "the button is let go of and nothing is moved doing it"
    );
}

#[test]
fn the_stamp_changing_is_a_report_boundary_as_well_as_the_queue_running_dry() {
    // A thumb kept moving keeps the queue full, so the values of one report can be
    // followed straight by the next one's with nothing in between to say the first
    // has ended. What says it is the stamp: every value of one report carries the
    // same one, so a run that waited for the queue to run dry would fold two
    // reports into one and move the cursor half as far.
    //
    // Scripted as the elements themselves, because a scripted `pointer` ends with
    // the queue running dry — which is the boundary this is *not* about.
    let stamped = |host: &mut SimHost, stamp: u64, dx: i64, dy: i64| {
        for (usage, value) in [(usage::POINTER_X, dx), (usage::POINTER_Y, dy)] {
            host.script(favjit_host::EventKind::HidValue {
                device: TRACKPOINT,
                page: page::GENERIC_DESKTOP,
                usage,
                stamp,
                value,
            });
        }
    };

    assert_eq!(
        run(|host| {
            stamped(host, 1, 3, 4);
            stamped(host, 2, -1, -2);
            host.probe();
        }),
        vec![Written::Pointer(PointerReport::moved(3, 4))],
        "the first is closed by the second's arrival, and the second is still open \
         when the run ends"
    );
}

#[test]
fn a_pointer_from_a_device_no_rule_knows_is_still_relayed() {
    // The host may already be suppressing that device, so dropping the report
    // would leave the cursor dead — the same reasoning that passes an unknown
    // keyboard's keys through unconverted.
    //
    // Attached like any other, because that is what makes it *read*: a device
    // nothing opened is one the machine sends nothing from, so what this is about
    // is a keyboard favjit took whose vendor and product no rule names.
    let report = PointerReport::moved(1, 1);
    assert_eq!(
        run(|host| {
            host.attach_external(UNKNOWN, 9, 9);
            host.pointer(UNKNOWN, report);
        }),
        vec![Written::Pointer(report)]
    );
}

#[test]
fn moving_the_pointer_does_not_spoil_a_tap() {
    // The built-in space bar is shift while held with something else and a space
    // when tapped alone. Moving the thumb stick is not typing, so a tap that
    // happens to include some motion is still a tap.
    let reports = run(|host| {
        host.press(BUILT_IN, Key::Spacebar)
            .advance(Duration::from_millis(50));
        host.pointer(TRACKPOINT, PointerReport::moved(3, 0));
        host.advance(Duration::from_millis(50))
            .release(BUILT_IN, Key::Spacebar);
    });

    assert_eq!(
        reports,
        vec![
            Written::Pointer(PointerReport::moved(3, 0)),
            Written::Keyboard(after(&[(Key::Spacebar, true)])),
            Written::Keyboard(KeyboardReport::default()),
        ]
    );
}

#[test]
fn a_pointer_button_makes_a_lazy_hold_real() {
    // A shift-click has to be a shift-click: the hold is invisible to the OS
    // until something needs it, and a button being pressed needs it exactly as a
    // key would. It also settles that the space was not tapped alone, so
    // releasing it types nothing.
    let pressed = PointerReport {
        buttons: Buttons::NONE.with(1),
        ..PointerReport::default()
    };
    let reports = run(|host| {
        host.press(BUILT_IN, Key::Spacebar)
            .advance(Duration::from_millis(50));
        host.pointer(TRACKPOINT, pressed);
        host.advance(Duration::from_millis(50))
            .release(BUILT_IN, Key::Spacebar);
    });

    assert_eq!(
        reports,
        vec![
            Written::Keyboard(after(&[(Key::LeftShift, true)])),
            Written::Pointer(pressed),
            Written::Keyboard(KeyboardReport::default()),
        ]
    );
}

#[test]
fn a_pointer_report_answers_the_watchdog_like_any_other_event() {
    // The heartbeat goes out after every event handled, and a pointer report is
    // one. A relay that moved the cursor without reporting the loop had turned
    // would look like a wedge to the supervisor while it was working.
    let mut host = SimHost::new();
    host.attach_external(TRACKPOINT, 6127, 24801);
    host.pointer(TRACKPOINT, PointerReport::moved(2, 2));
    sink::run(
        &converting(),
        Layout::dudrack(),
        None,
        InputConfig::default(),
        &mut host,
        None,
    );

    // One for the attach, one for the report.
    assert_eq!(host.heartbeats().len(), 2);
}

#[test]
fn letting_a_button_go_is_relayed_even_though_nothing_moved() {
    // The release carries no motion and no buttons, so a filter that only asked
    // whether anything moved would drop it and leave the button held down
    // forever.
    let pressed = PointerReport {
        buttons: Buttons::NONE.with(1),
        ..PointerReport::default()
    };
    assert_eq!(
        run(|host| {
            host.pointer(TRACKPOINT, pressed);
            host.pointer(TRACKPOINT, PointerReport::default());
        }),
        vec![
            Written::Pointer(pressed),
            Written::Pointer(PointerReport::default()),
        ]
    );
}

#[test]
fn a_still_report_with_no_buttons_is_not_worth_sending() {
    // A device can report a value that resolves to no motion at all. Relaying it
    // would be a report the hardware did not make, and the OS accelerates per
    // report.
    assert_eq!(
        run(|host| {
            host.pointer(TRACKPOINT, PointerReport::default());
        }),
        vec![]
    );
}

/// The same, with the pointer tuned.
fn run_tuned(pointer: Tuning, script: impl FnOnce(&mut SimHost)) -> Vec<Written> {
    let mut host = SimHost::new();
    host.attach_built_in(BUILT_IN);
    host.attach_external(TRACKPOINT, 6127, 24801);
    script(&mut host);
    sink::run(
        &converting(),
        Layout::dudrack(),
        Settings {
            pointer,
            ..Settings::default()
        },
        InputConfig::default(),
        &mut host,
        None,
    );
    decode(&host.reports_while_reading())
}

#[test]
fn movement_reaches_the_machine_exactly_as_the_hardware_reported_it() {
    // How far the cursor travels is the output device's own resolution, set on the
    // device (`docs/platform/macos/pointer-acceleration.md`). Multiplying here as
    // well would be a second, coarser speed control fighting the first.
    //
    // No horizontal wheel: this keyboard's own stick has never been observed to
    // report one (`docs/platform/macos/input-suppression.md`) — `link_relay.rs`'s
    // `invert_horizontal_wheel_flips_the_sign_of_a_relayed_wheel` covers that axis
    // through a device that actually can.
    let report = PointerReport {
        dx: 3,
        dy: -7,
        vertical_wheel: 1,
        horizontal_wheel: 0,
        buttons: Buttons::NONE.with(2),
    };
    assert_eq!(
        run_tuned(
            Tuning {
                invert_vertical_wheel: true,
                invert_horizontal_wheel: true,
            },
            |host| {
                host.pointer(TRACKPOINT, report);
            }
        ),
        vec![Written::Pointer(PointerReport {
            vertical_wheel: -1,
            ..report
        })]
    );
}

#[test]
fn the_vertical_wheel_can_be_turned_over_on_its_own() {
    // Which way a wheel scrolls is a per-device preference on every other
    // platform, and macOS has one switch for all of them — so the device favjit
    // relays needs its own, and it is the vertical one that is upside down.
    let tuned = Tuning {
        invert_vertical_wheel: true,
        ..Tuning::default()
    };
    assert_eq!(
        run_tuned(tuned, |host| {
            host.pointer(
                TRACKPOINT,
                PointerReport {
                    vertical_wheel: 3,
                    ..PointerReport::default()
                },
            );
        }),
        vec![Written::Pointer(PointerReport {
            vertical_wheel: -3,
            ..PointerReport::default()
        })]
    );
}

#[test]
fn tuning_does_not_invent_movement_out_of_stillness() {
    // Turning a wheel that has not moved over is still a wheel that has not moved,
    // and a report with nothing in it is not worth sending: the drop is decided on
    // what will actually go out.
    let tuned = Tuning {
        invert_vertical_wheel: true,
        invert_horizontal_wheel: true,
    };
    assert_eq!(
        run_tuned(tuned, |host| {
            host.pointer(TRACKPOINT, PointerReport::default());
        }),
        vec![]
    );
}
