//! What the TrackPoint keyboard's pointer produces, end to end.
//!
//! A seize is per device and that keyboard is one device, so suppressing its keys
//! takes its pointer with it and the pointer has to come back out through favjit
//! (`docs/platform/macos/input-suppression.md`). These pin what the sink does with
//! a pointer report, including the two places it meets the keyboard state.

use core::time::Duration;

use favjit_core::sink::{self, Request, Settings};
use favjit_core::{
    pointer::Tuning, Buttons, DeviceId, DeviceInfo, EventKind, Injected, Key, Layout,
    ModifierKeys as M, PointerReport,
};
use favjit_host_sim::SimHost;

/// Lenovo TrackPoint Keyboard II: the keyboard whose pointer this is about.
const TRACKPOINT: DeviceId = DeviceId(2);
/// The MacBook's own keyboard, which is where the space bar is a tap-hold — the
/// external ones keep their space, so the interaction between a pointer button
/// and a lazy hold can only be reached across two keyboards, and that is the
/// arrangement it happens in.
const BUILT_IN: DeviceId = DeviceId(1);
/// A device the host never announced, to pin that a pointer needs no rules.
const UNKNOWN: DeviceId = DeviceId(9);

/// A whole run of the program, on a simulated Mac.
///
/// Through the program rather than the converter's loop, because that is what a
/// person starts: a suite that called the loop directly would pass while the run
/// that reaches it was broken.
fn converting() -> Request {
    Request::Injecting { listen: false }
}

fn run(script: impl FnOnce(&mut SimHost)) -> Vec<Injected> {
    let mut host = SimHost::new();
    host.attach(DeviceInfo::built_in(BUILT_IN));
    host.attach(DeviceInfo::external(TRACKPOINT, 6127, 24801));
    script(&mut host);
    sink::run(&converting(), Layout::dudrack(), None, &mut host, None);
    host.injected()
}

#[test]
fn a_pointer_report_is_relayed_as_it_stands() {
    let report = PointerReport::moved(7, -3);
    assert_eq!(
        run(|host| {
            host.pointer(TRACKPOINT, report);
        }),
        vec![Injected::Pointer(report)]
    );
}

#[test]
fn the_axes_stay_in_one_report() {
    // Splitting a diagonal into one report per axis would be accelerated as two
    // short movements and travel less far than the same motion of the thumb, so
    // what arrives as one report has to leave as one.
    let records = run(|host| {
        host.pointer(TRACKPOINT, PointerReport::moved(4, 4));
    });
    assert_eq!(records.len(), 1);
}

#[test]
fn buttons_and_wheel_are_relayed() {
    let report = PointerReport {
        dx: 0,
        dy: 0,
        vertical_wheel: -2,
        horizontal_wheel: 1,
        buttons: Buttons::NONE.with(1).with(3),
    };
    assert_eq!(
        run(|host| {
            host.pointer(TRACKPOINT, report);
        }),
        vec![Injected::Pointer(report)]
    );
}

#[test]
fn a_pointer_from_a_device_no_rule_knows_is_still_relayed() {
    // The host may already be suppressing that device, so dropping the report
    // would leave the cursor dead — the same reasoning that passes an unknown
    // keyboard's keys through unconverted.
    let report = PointerReport::moved(1, 1);
    assert_eq!(
        run(|host| {
            host.pointer(UNKNOWN, report);
        }),
        vec![Injected::Pointer(report)]
    );
}

#[test]
fn moving_the_pointer_does_not_spoil_a_tap() {
    // The built-in space bar is shift while held with something else and a space
    // when tapped alone. Moving the thumb stick is not typing, so a tap that
    // happens to include some motion is still a tap.
    let records = run(|host| {
        host.press(BUILT_IN, Key::Spacebar)
            .advance(Duration::from_millis(50));
        host.pointer(TRACKPOINT, PointerReport::moved(3, 0));
        host.advance(Duration::from_millis(50))
            .release(BUILT_IN, Key::Spacebar);
    });

    assert_eq!(
        records,
        vec![
            Injected::Pointer(PointerReport::moved(3, 0)),
            Injected::KeyDown {
                key: Key::Spacebar,
                modifiers: M::NONE
            },
            Injected::KeyUp {
                key: Key::Spacebar,
                modifiers: M::NONE
            },
        ]
    );
}

#[test]
fn a_pointer_button_makes_a_lazy_hold_real() {
    // A shift-click has to be a shift-click: the hold is invisible to the OS
    // until something needs it, and a button being pressed needs it exactly as a
    // key would. It also settles that the space was not tapped alone, so
    // releasing it types nothing.
    let records = run(|host| {
        host.press(BUILT_IN, Key::Spacebar)
            .advance(Duration::from_millis(50));
        host.pointer(
            TRACKPOINT,
            PointerReport {
                buttons: Buttons::NONE.with(1),
                ..PointerReport::default()
            },
        );
        host.advance(Duration::from_millis(50))
            .release(BUILT_IN, Key::Spacebar);
    });

    assert_eq!(
        records,
        vec![
            Injected::KeyDown {
                key: Key::LeftShift,
                modifiers: M::of(&[Key::LeftShift])
            },
            Injected::Pointer(PointerReport {
                buttons: Buttons::NONE.with(1),
                ..PointerReport::default()
            }),
            Injected::KeyUp {
                key: Key::LeftShift,
                modifiers: M::NONE
            },
        ]
    );
}

#[test]
fn a_pointer_report_answers_the_watchdog_like_any_other_event() {
    // The heartbeat goes out after every event handled, and a pointer report is
    // one. A relay that moved the cursor without reporting the loop had turned
    // would look like a wedge to the supervisor while it was working.
    let mut host = SimHost::new();
    host.attach(DeviceInfo::external(TRACKPOINT, 6127, 24801));
    host.pointer(TRACKPOINT, PointerReport::moved(2, 2));
    sink::run(&converting(), Layout::dudrack(), None, &mut host, None);

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
            Injected::Pointer(pressed),
            Injected::Pointer(PointerReport::default()),
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
            host.script(EventKind::Pointer {
                device: TRACKPOINT,
                report: PointerReport::default(),
            });
        }),
        vec![]
    );
}

/// The same, with the pointer tuned.
fn run_tuned(pointer: Tuning, script: impl FnOnce(&mut SimHost)) -> Vec<Injected> {
    let mut host = SimHost::new();
    host.attach(DeviceInfo::built_in(BUILT_IN));
    host.attach(DeviceInfo::external(TRACKPOINT, 6127, 24801));
    script(&mut host);
    sink::run(
        &converting(),
        Layout::dudrack(),
        Settings {
            pointer,
            ..Settings::default()
        },
        &mut host,
        None,
    );
    host.injected()
}

#[test]
fn movement_reaches_the_machine_exactly_as_the_hardware_reported_it() {
    // How far the cursor travels is the output device's own resolution, set on the
    // device (`docs/platform/macos/pointer-acceleration.md`). Multiplying here as
    // well would be a second, coarser speed control fighting the first.
    let report = PointerReport {
        dx: 3,
        dy: -7,
        vertical_wheel: 1,
        horizontal_wheel: 2,
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
        vec![Injected::Pointer(PointerReport {
            vertical_wheel: -1,
            horizontal_wheel: -2,
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
                    horizontal_wheel: 2,
                    ..PointerReport::default()
                },
            );
        }),
        vec![Injected::Pointer(PointerReport {
            vertical_wheel: -3,
            horizontal_wheel: 2,
            ..PointerReport::default()
        })]
    );
}

#[test]
fn both_wheel_axes_can_be_turned_over_together() {
    // Which is how the tool this replaces was set up on this machine: reverse on
    // both axes. They are separate fields because a device can be upside down in
    // one axis and not the other, and one flag for both would make that
    // unreachable.
    let tuned = Tuning {
        invert_vertical_wheel: true,
        invert_horizontal_wheel: true,
    };
    assert_eq!(
        run_tuned(tuned, |host| {
            host.pointer(
                TRACKPOINT,
                PointerReport {
                    vertical_wheel: 3,
                    horizontal_wheel: -2,
                    ..PointerReport::default()
                },
            );
        }),
        vec![Injected::Pointer(PointerReport {
            vertical_wheel: -3,
            horizontal_wheel: 2,
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
            host.script(EventKind::Pointer {
                device: TRACKPOINT,
                report: PointerReport::default(),
            });
        }),
        vec![]
    );
}
