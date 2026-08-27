//! How a run ends, and what it says about it.
//!
//! Every one of these ends the run rather than changing what it converts, because
//! the seize is released with the process: a converter that stopped converting while
//! still holding the keyboards is the outcome ADR-0008 rules out. Which of them
//! happened is what whatever supervises favjit acts on, so it is reported rather
//! than only logged.

use core::time::Duration;

use favjit_engine::sink::{self, Ending, InputConfig, Request};
use favjit_engine::{DeviceId, Key, Layout};
use favjit_hid::report::{modifier_bit, Report};
use favjit_host::OutputReport;
use favjit_host_sim::{Did, SimHost};

const BUILT_IN: DeviceId = DeviceId(1);

fn converting() -> Request {
    Request::Injecting { listen: false }
}

/// Two keystrokes, so a machine can be scripted to end in the middle of them.
fn typing() -> SimHost {
    let mut mac = SimHost::new();
    mac.attach_built_in(BUILT_IN)
        .tap(BUILT_IN, Key::K)
        .tap(BUILT_IN, Key::S);
    mac
}

fn run(mac: &mut SimHost) -> Ending {
    sink::run(
        &converting(),
        Layout::dudrack(),
        None,
        InputConfig::default(),
        mac,
        None,
    )
    .0
}

/// What the run wrote after its stream had run out, in order.
///
/// Read as the reports past the ones a script's own events produced, rather
/// than as a tail of a known length: how much a run has to let go of is a
/// property of what was still down when it stopped, and a fixed count would
/// pass whatever it found there.
fn on_the_way_out(mac: &SimHost) -> Vec<(OutputReport, Vec<u8>)> {
    let converting = mac.reports_while_reading().len();
    mac.reports()[converting..]
        .iter()
        .map(|(_, report, bytes)| (*report, bytes.clone()))
        .collect()
}

#[test]
fn a_key_still_down_when_the_run_ends_is_let_go_of() {
    // The device converted keystrokes go out through belongs to a driver and
    // outlives this process, so a key left down in the last report is that key
    // down for whatever runs next — and the keyboards go back at the same
    // moment, which is the person typing on the key that is stuck.
    let mut mac = SimHost::new();
    mac.attach_built_in(BUILT_IN).press(BUILT_IN, Key::K);

    assert_eq!(run(&mut mac), Ending::Converted);
    assert_eq!(
        on_the_way_out(&mac),
        vec![(OutputReport::Keyboard, Report::default().bytes())],
        "one report, and it holds nothing"
    );
}

#[test]
fn a_run_that_left_nothing_down_writes_nothing_on_the_way_out() {
    // Not a blanket reset of the device: it is shared, so a report saying that
    // nothing at all is held would speak for whatever else has it open too.
    let mut mac = typing();

    assert_eq!(run(&mut mac), Ending::Converted);
    assert_eq!(on_the_way_out(&mac), vec![]);
}

#[test]
fn a_modifier_is_let_go_of_after_the_key_it_was_modifying() {
    // The other way round hands applications a key-up whose modifier has
    // already gone, which is how a shifted character arrives lower case — so
    // one report per key, in reverse order of press, rather than a single
    // report holding nothing.
    let mut mac = SimHost::new();
    mac.attach_built_in(BUILT_IN)
        .press(BUILT_IN, Key::LeftShift)
        .press(BUILT_IN, Key::K);

    assert_eq!(run(&mut mac), Ending::Converted);

    let still_shifted = Report {
        modifiers: modifier_bit(Key::LeftShift).expect("a modifier"),
        ..Report::default()
    };
    assert_eq!(
        on_the_way_out(&mac),
        vec![
            (OutputReport::Keyboard, still_shifted.bytes()),
            (OutputReport::Keyboard, Report::default().bytes()),
        ],
        "the key first, with the modifier still in the byte, and the modifier after it"
    );
}

#[test]
fn a_switch_thrown_while_converting_ends_the_run() {
    // Somebody choosing the menu item under a running converter. The keyboards come
    // back because the run finishes, which is the only way they do.
    let mut mac = typing().with_converting_off_after(2);

    assert_eq!(run(&mut mac), Ending::SwitchedOff);
    assert!(mac.did().ends_with(&[
        Did::AskedForHeldDevice,
        Did::ReleasedDevice { device: BUILT_IN },
        Did::AskedForHeldDevice,
    ]));
}

#[test]
fn the_output_going_away_ends_the_run() {
    // A converted keystroke written into a closed socket reaches nothing, and
    // nothing about it looks wrong until somebody types — so the run ends instead of
    // converting into it.
    let mut mac = typing().with_output_lost_after(2);

    assert_eq!(run(&mut mac), Ending::OutputGone);
}

/// A machine whose keyboards do not go quiet after the run's reason to end has
/// arrived: something arrives every few milliseconds for as long as the script
/// runs, which is a person moving the mouse or holding a key down.
///
/// The gap is well under the bound a wait is given, so a run that only looks at
/// how it might have ended once a wait has come back empty never gets to look.
fn busy_after(mac: &mut SimHost) -> &mut SimHost {
    for _ in 0..40 {
        mac.advance(Duration::from_millis(10)).tap(BUILT_IN, Key::S);
    }
    mac
}

#[test]
fn the_output_going_away_under_a_busy_keyboard_ends_the_run_within_the_bound() {
    // Read off a run whose output connection the service had closed while pointer
    // reports were still arriving every ten milliseconds from the other machine:
    // every one of them was converted into the closed socket, and the run kept
    // the keyboards for as long as the stream stayed busy — which was until the
    // person gave up and stopped. The bound a wait is given is the bound on
    // noticing, and a stream that never lets a wait run out must not stretch it.
    let mut mac = SimHost::new();
    mac.attach_built_in(BUILT_IN).tap(BUILT_IN, Key::K);
    busy_after(&mut mac);
    let mut mac = mac.with_output_lost_after(3);

    assert_eq!(run(&mut mac), Ending::OutputGone);
    let converted_after_the_loss = mac.reports_while_reading().len() - 2;
    // Within a quarter of a second at ten milliseconds a tap, and never the
    // whole of what was scripted.
    assert!(
        converted_after_the_loss <= 2 * 25,
        "converted {converted_after_the_loss} reports into a connection that had gone"
    );
}

#[test]
fn a_switch_thrown_under_a_busy_keyboard_ends_the_run_within_the_bound() {
    // The same shape for the menu item: the control file changing produces no
    // event, so a run that reads it only when nothing is arriving stays on
    // while the person types — and what they type is being converted, which is
    // what they asked to stop.
    let mut mac = SimHost::new();
    mac.attach_built_in(BUILT_IN).tap(BUILT_IN, Key::K);
    busy_after(&mut mac);
    let mut mac = mac.with_converting_off_after(3);

    assert_eq!(run(&mut mac), Ending::SwitchedOff);
    let converted_after_the_switch = mac.reports_while_reading().len() - 2;
    assert!(
        converted_after_the_switch <= 2 * 25,
        "converted {converted_after_the_switch} reports after converting was switched off"
    );
}

#[test]
fn a_stream_that_simply_runs_out_is_a_run_that_converted() {
    // The ordinary ending, and the one a bound or a signal produces: nothing is
    // wrong, so nothing is said beyond that it is over.
    let mut mac = typing();

    assert_eq!(run(&mut mac), Ending::Converted);
}
