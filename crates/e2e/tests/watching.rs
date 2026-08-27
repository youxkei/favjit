//! A run that watches the keyboards and converts none of it.
//!
//! What it exists for is finding out where a key reports — the PC-JIS thumb keys
//! were found this way rather than guessed. That has to be possible without logging
//! what was typed, so the one thing this run must never do is produce a keystroke.

use favjit_core::sink::{self, Ending};
use favjit_core::{DeviceId, DeviceInfo, Key};
use favjit_host_sim::{Did, SimHost};

const BUILT_IN: DeviceId = DeviceId(1);

#[test]
fn watching_converts_nothing_however_much_is_typed() {
    // The point of the mode: a key that came out converted would be a keystroke
    // this run produced, and a run that produces keystrokes is not a way to look at
    // a keyboard safely.
    let mut mac = SimHost::new();
    mac.attach(DeviceInfo::built_in(BUILT_IN))
        .tap(BUILT_IN, Key::K)
        .tap(BUILT_IN, Key::S);

    assert_eq!(sink::watch(&mut mac), Ending::Converted);
    assert_eq!(mac.injected(), Vec::new());
}

#[test]
fn watching_takes_the_keyboards_without_taking_them_exclusively() {
    // Shared, because this run is for looking: seizing would take the keyboard away
    // from the person while they press the key they are trying to identify.
    let mut mac = SimHost::new();
    mac.attach(DeviceInfo::built_in(BUILT_IN));

    sink::watch(&mut mac);

    assert_eq!(
        mac.did(),
        vec![
            Did::AskedIfSwitchedOn,
            Did::AskedPermission,
            Did::TookInput { suppressing: false },
        ]
    );
}

#[test]
fn a_run_that_may_not_read_input_watches_nothing() {
    let mut mac = SimHost::new().with_no_permission();

    assert_eq!(sink::watch(&mut mac), Ending::NoPermission);
    assert_eq!(mac.took_input(), None);
}

#[test]
fn a_machine_with_converting_switched_off_watches_nothing() {
    // The switch is checked by this run as well as by the one that converts: a mode
    // that read the keyboards while converting was off would be reading them at the
    // one moment somebody has said not to.
    let mut mac = SimHost::new().with_converting_off();

    assert_eq!(sink::watch(&mut mac), Ending::SwitchedOff);
    assert_eq!(mac.took_input(), None);
}

#[test]
fn every_event_watched_is_answered_with_one_heartbeat() {
    // ADR-0008's supervisor watches this run too, and it judges by silence: a loop
    // that reads keyboards without reporting that it came round would be killed for
    // being quiet while it was working.
    let mut mac = SimHost::new();
    mac.attach(DeviceInfo::built_in(BUILT_IN))
        .tap(BUILT_IN, Key::K)
        .probe();

    sink::watch(&mut mac);

    // The attach, the press and release, and the probe.
    assert_eq!(mac.heartbeats().len(), 4);
}

#[test]
fn watching_says_nothing_about_the_run() {
    // Neither cost a relaying or injecting run is warned about applies here: the
    // keyboards are shared, so no keystroke arrives twice and none is held for a
    // watchdog to be missing from.
    let mut mac = SimHost::new().with_no_watchdog();
    mac.attach(DeviceInfo::built_in(BUILT_IN))
        .tap(BUILT_IN, Key::K);

    sink::watch(&mut mac);

    assert_eq!(mac.warnings(), Vec::<String>::new());
}
