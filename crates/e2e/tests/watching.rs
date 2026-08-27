//! A run that watches the keyboards and converts none of it.
//!
//! What it exists for is finding out where a key reports — the PC-JIS thumb keys
//! were found this way rather than guessed. That has to be possible without logging
//! what was typed, so the one thing this run must never do is produce a keystroke.

use favjit_engine::sink::{self, Ending};
use favjit_engine::{DeviceId, Key};
use favjit_host::EventKind;
use favjit_host_sim::{Did, SimHost};

const BUILT_IN: DeviceId = DeviceId(1);

/// A keyboard-page usage no table here names.
///
/// `F13`, which external keyboards do have and this layout has no name for. A
/// number rather than a `Key`, because the whole point is that no `Key` names
/// it: one that did would be resolved and never reported.
const UNNAMED: u32 = 0x68;

#[test]
fn watching_reads_however_much_is_typed_without_a_way_to_inject_it() {
    // `sink::watch` takes a `&mut dyn SinkInputHost`, not the `SinkHost` that
    // carries `send_report` — so unlike `asking_for_nothing.rs`'s runtime check,
    // there is no call this run could make that would produce a keystroke: it
    // fails to compile before it fails a test.
    let mut mac = SimHost::new();
    mac.attach_built_in(BUILT_IN)
        .tap(BUILT_IN, Key::K)
        .tap(BUILT_IN, Key::S);

    assert_eq!(sink::watch(&mut mac), Ending::Converted);
}

#[test]
fn watching_takes_the_keyboards_without_taking_them_exclusively() {
    // Shared, because this run is for looking: seizing would take the keyboard away
    // from the person while they press the key they are trying to identify.
    let mut mac = SimHost::new();
    mac.attach_built_in(BUILT_IN);

    sink::watch(&mut mac);

    assert_eq!(
        mac.did(),
        vec![
            Did::AskedIfSwitchedOn,
            Did::AskedPermission,
            Did::LookedForDevices,
            Did::TookDevice {
                device: BUILT_IN,
                exclusive: false,
            },
            Did::ReadDevice { device: BUILT_IN },
            Did::AskedForHeldDevice,
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
    mac.attach_built_in(BUILT_IN).tap(BUILT_IN, Key::K).probe();

    sink::watch(&mut mac);

    // The attach, the press and release, and the probe.
    assert_eq!(mac.heartbeats().len(), 4);
}

#[test]
fn a_usage_nothing_names_is_reported_rather_than_dropped() {
    // The whole of what this run is for. A usage no table names is the one thing
    // this mode has to say out loud: dropped quietly, the key a person is trying
    // to identify looks identical to one this machine never reported at all, and
    // `favjit --usages` has nothing to print.
    //
    // Scripted as the raw element rather than as a `Key`, because a `Key` is
    // exactly what does not exist for it yet — which is also why the run has to
    // read every element a device has rather than the ones a table already
    // covers (`favjit_host::sink::SinkInputHost::read_device`).
    let mut mac = SimHost::new();
    mac.attach_built_in(BUILT_IN).script(EventKind::HidValue {
        device: BUILT_IN,
        page: favjit_hid::page::KEYBOARD_OR_KEYPAD,
        usage: UNNAMED,
        stamp: 1,
        value: 1,
    });

    assert_eq!(sink::watch(&mut mac), Ending::Converted);
    let said = mac.warnings().join("\n");
    assert!(
        said.contains(&format!("{UNNAMED:#x}")),
        "the page and usage are what a person adds to the table, so the line has \
         to carry them; said: {said}"
    );
}

#[test]
fn a_machine_that_cannot_read_its_keyboards_ends_the_watching_there() {
    // This mode exists to answer "where does that key report", and a machine that
    // cannot read its keyboards cannot answer. Waiting instead would look exactly
    // like a key that reports nothing, which is the answer a person would then act
    // on.
    let mut mac = SimHost::new().with_no_input();
    mac.attach_built_in(BUILT_IN).tap(BUILT_IN, Key::K);

    assert_eq!(sink::watch(&mut mac), Ending::NoInput);
}

#[test]
fn an_element_value_that_is_neither_up_nor_down_is_no_keystroke() {
    // A key reports 0 or 1 and nothing else; an element that carries a range is a
    // dial or an axis, and reading one as a press would report a key nobody
    // touched.
    let mut mac = SimHost::new();
    mac.attach_built_in(BUILT_IN).script(EventKind::HidValue {
        device: BUILT_IN,
        page: favjit_hid::page::KEYBOARD_OR_KEYPAD,
        usage: UNNAMED,
        stamp: 1,
        value: 7,
    });

    assert_eq!(sink::watch(&mut mac), Ending::Converted);
    assert_eq!(
        mac.warnings(),
        Vec::<String>::new(),
        "and nothing to report about it either: what this mode reports is where a \
         key is, and that was no key"
    );
}

#[test]
fn a_queue_that_runs_dry_with_nothing_in_flight_is_no_report() {
    // Every device's queue runs dry between bursts, pointer or not. Reading that
    // as the end of a pointer report would send one the device never made — with
    // whatever the last one held, which is how a cursor drifts on its own.
    let mut mac = SimHost::new();
    mac.attach_built_in(BUILT_IN)
        .script(EventKind::HidValuesDone { device: BUILT_IN });

    assert_eq!(sink::watch(&mut mac), Ending::Converted);
    assert_eq!(mac.warnings(), Vec::<String>::new());
}

#[test]
fn a_device_named_by_a_path_is_nothing_this_machine_could_have_reported() {
    // The boundary's vocabulary is shared by both machines (ADR-0006), so the
    // kind a Windows host reports is a kind this one's stream *can* carry. A
    // sink's own host speaks HID, and a device from the other machine arrives
    // over the link already read — so a path here is a device to say nothing
    // about rather than one to take.
    let mut mac = SimHost::new();
    mac.attach_built_in(BUILT_IN)
        .script(EventKind::PathDeviceFound {
            device: DeviceId(9),
            path: String::from(r"\\?\HID#VID_17EF&PID_60E1#8&1e0b8ad9"),
        });

    assert_eq!(sink::watch(&mut mac), Ending::Converted);
    assert!(
        !mac.did().contains(&Did::ReadDevice {
            device: DeviceId(9)
        }),
        "{:?}",
        mac.did()
    );
    assert_eq!(mac.warnings(), Vec::<String>::new());
}

#[test]
fn a_hooked_key_is_nothing_this_machine_could_have_reported() {
    // The boundary's vocabulary is shared by both machines (ADR-0006), so the kind
    // a Windows host reports is a kind this one's stream *can* carry — and a run
    // here has no table to read it with. Dropped rather than guessed at: a make
    // code read as a usage is a key nobody pressed.
    let mut mac = SimHost::new();
    mac.attach_built_in(BUILT_IN).script(EventKind::HookedKey {
        device: BUILT_IN,
        make_code: 0x1f,
        vkey: 0x41,
        flags: 0,
    });

    assert_eq!(sink::watch(&mut mac), Ending::Converted);
    assert_eq!(mac.warnings(), Vec::<String>::new());
}

#[test]
fn watching_says_nothing_about_the_run() {
    // Neither cost a relaying or injecting run is warned about applies here: the
    // keyboards are shared, so no keystroke arrives twice and none is held for a
    // watchdog to be missing from.
    let mut mac = SimHost::new().with_no_watchdog();
    mac.attach_built_in(BUILT_IN).tap(BUILT_IN, Key::K);

    sink::watch(&mut mac);

    assert_eq!(mac.warnings(), Vec::<String>::new());
}
