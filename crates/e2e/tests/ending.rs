//! How a run ends, and what it says about it.
//!
//! Every one of these ends the run rather than changing what it converts, because
//! the seize is released with the process: a converter that stopped converting while
//! still holding the keyboards is the outcome ADR-0008 rules out. Which of them
//! happened is what whatever supervises favjit acts on, so it is reported rather
//! than only logged.

use favjit_core::sink::{self, Ending, Request};
use favjit_core::{DeviceId, DeviceInfo, Key, Layout};
use favjit_host_sim::SimHost;

const BUILT_IN: DeviceId = DeviceId(1);

fn converting() -> Request {
    Request::Injecting { listen: false }
}

/// Two keystrokes, so a machine can be scripted to end in the middle of them.
fn typing() -> SimHost {
    let mut mac = SimHost::new();
    mac.attach(DeviceInfo::built_in(BUILT_IN))
        .tap(BUILT_IN, Key::K)
        .tap(BUILT_IN, Key::S);
    mac
}

fn run(mac: &mut SimHost) -> Ending {
    sink::run(&converting(), Layout::dudrack(), None, mac, None)
}

#[test]
fn a_switch_thrown_while_converting_ends_the_run() {
    // Somebody choosing the menu item under a running converter. The keyboards come
    // back because the run finishes, which is the only way they do.
    let mut mac = typing().with_converting_off_after(2);

    assert_eq!(run(&mut mac), Ending::SwitchedOff);
}

#[test]
fn the_output_going_away_ends_the_run() {
    // A converted keystroke written into a closed socket reaches nothing, and
    // nothing about it looks wrong until somebody types — so the run ends instead of
    // converting into it.
    let mut mac = typing().with_output_lost_after(2);

    assert_eq!(run(&mut mac), Ending::OutputGone);
}

#[test]
fn a_stream_that_simply_runs_out_is_a_run_that_converted() {
    // The ordinary ending, and the one a bound or a signal produces: nothing is
    // wrong, so nothing is said beyond that it is over.
    let mut mac = typing();

    assert_eq!(run(&mut mac), Ending::Converted);
}
