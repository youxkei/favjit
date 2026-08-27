//! What a run brings up before it takes a keyboard, and in what order.
//!
//! The order is not cosmetic. Suppression must never outlive the ability to process
//! input (ADR-0008), so every assertion here about what was *not* taken is that
//! rule: a keyboard held with nowhere to send its keystrokes is not "favjit stopped
//! converting" but "the keyboard stopped working".

use favjit_engine::sink::{self, Ending, InputConfig, Request};
use favjit_engine::{pointer, DeviceId, DeviceMatch, Key, Layout};
use favjit_host::EventKind;
use favjit_host_sim::{Did, SimHost, SimPointer, OUTPUT_VENDOR};

fn converting() -> Request {
    Request::Injecting { listen: false }
}

/// The same, accepting the other machine's input as well.
///
/// A variant of the injecting one rather than a request of its own, because
/// listening is what a delivering run does as well: there is no shape for accepting
/// input with nowhere to send it.
fn listening() -> Request {
    Request::Injecting { listen: true }
}

fn run(mac: &mut SimHost, request: &Request) -> Ending {
    sink::run(
        request,
        Layout::dudrack(),
        None,
        InputConfig::default(),
        mac,
        None,
    )
    .0
}

#[test]
fn a_run_that_is_switched_off_waits_and_takes_nothing() {
    // The whole of what "off" means: nothing is captured and nothing is seized, so
    // the keyboards are the machine's own. The run waits rather than exiting at
    // once, because whatever supervises it would restart a process that exited —
    // and it ends when the switch comes back rather than carrying on, so the
    // keyboards are taken by a run that starts from nothing.
    let mut mac = SimHost::new().with_converting_off();
    mac.attach_built_in(DeviceId(1)).tap(DeviceId(1), Key::K);

    assert_eq!(run(&mut mac, &converting()), Ending::SwitchedOff);
    assert_eq!(mac.did(), vec![Did::AskedIfSwitchedOn]);
    assert_eq!(mac.took_input(), None);
}

#[test]
fn the_switch_is_asked_about_before_anything_else() {
    // Before the permission and before the output, because a machine that is
    // switched off should be asked for nothing at all.
    let mut mac = SimHost::new();

    run(&mut mac, &converting());

    assert_eq!(mac.did().first(), Some(&Did::AskedIfSwitchedOn));
}

#[test]
fn nothing_is_taken_when_the_output_will_not_come_up() {
    // The output comes up first so that this failure happens before anything is
    // taken: the keyboards stay the machine's own, and what stopped working is
    // favjit.
    let mut mac = SimHost::new().with_no_output();
    mac.attach_built_in(DeviceId(1)).tap(DeviceId(1), Key::K);

    assert_eq!(run(&mut mac, &converting()), Ending::NoOutput);
    assert_eq!(
        mac.did(),
        vec![
            Did::AskedIfSwitchedOn,
            Did::AskedPermission,
            Did::OpenedOutput,
            // What the machine reported turned into what a person has to do
            // about it, which `warnings.rs` reads the wording of.
            Did::Warned,
        ]
    );
    assert_eq!(mac.took_input(), None, "no keyboard was taken");
}

#[test]
fn a_run_that_may_not_read_input_takes_nothing_and_opens_nothing() {
    // Refused before the output, because a converter that cannot read the keyboards
    // has nothing to convert — and bringing up a virtual keyboard first would leave
    // a device behind for a run that never happened.
    let mut mac = SimHost::new().with_no_permission();

    assert_eq!(run(&mut mac, &converting()), Ending::NoPermission);
    assert_eq!(
        mac.did(),
        vec![
            Did::AskedIfSwitchedOn,
            Did::AskedPermission,
            Did::RequestedPermission
        ]
    );
    assert_eq!(mac.took_input(), None);
}

#[test]
fn a_machine_that_refuses_its_keyboards_ends_the_run_there() {
    // The output is already up by then, and it is let go with the process: nothing
    // is left holding a device for a run that converts nothing.
    let mut mac = SimHost::new().with_no_input();
    mac.attach_built_in(DeviceId(1)).tap(DeviceId(1), Key::K);

    assert_eq!(run(&mut mac, &converting()), Ending::NoInput);
    assert_eq!(
        mac.did(),
        vec![
            Did::AskedIfSwitchedOn,
            Did::AskedPermission,
            Did::OpenedOutput,
            Did::TunedOutput,
            Did::LookedForDevices,
        ]
    );
}

#[test]
fn the_output_is_up_and_tuned_before_a_keyboard_is_taken() {
    // The tuning belongs to the output device, so there is no service to carry it
    // until that device exists — and it is set on every run rather than once at
    // install, because it outlasts the process that set it.
    let mut mac = SimHost::new();

    run(&mut mac, &converting());

    assert_eq!(
        mac.did(),
        vec![
            Did::AskedIfSwitchedOn,
            Did::AskedPermission,
            Did::OpenedOutput,
            Did::TunedOutput,
            Did::LookedForDevices,
            Did::AskedForHeldDevice,
        ]
    );
}

#[test]
fn the_link_is_opened_after_the_output() {
    // A link that let input in from the other machine before there was anywhere to
    // send it would convert keystrokes into nothing.
    let mut mac = SimHost::new();

    run(&mut mac, &listening());

    assert_eq!(
        mac.did(),
        vec![
            Did::AskedIfSwitchedOn,
            Did::AskedPermission,
            Did::OpenedOutput,
            Did::TunedOutput,
            Did::BoundLink,
            Did::StartedTheLink,
            Did::LookedForDevices,
            Did::AskedForHeldDevice,
        ]
    );
}

#[test]
fn a_socket_that_cannot_be_opened_leaves_nothing_to_turn() {
    // Nothing is started, because there is nothing for it to serve: a loop turning
    // over an unbound link would be a link that reports every connection as failed
    // for as long as the run lasts.
    let mut mac = SimHost::new().with_no_link_socket();

    let ending = run(&mut mac, &listening());

    assert!(!mac.did().contains(&Did::StartedTheLink));
    assert_eq!(
        ending,
        Ending::Converted,
        "the keyboards in front of the person do not depend on the link"
    );
    assert_eq!(mac.took_input(), Some(true));
}

#[test]
fn a_link_nothing_can_turn_is_a_run_that_still_converts() {
    // The socket goes with the loop: it was bound and handed over, and a machine
    // that cannot turn the loop drops what it was handed rather than leaving a
    // socket that accepts a source and never answers it.
    let mut mac = SimHost::new().with_nothing_to_run_alongside();

    let ending = run(&mut mac, &listening());

    assert_eq!(
        mac.advertisements(),
        0,
        "nothing turned, so nothing said so"
    );
    assert_eq!(ending, Ending::Converted);
    assert_eq!(mac.took_input(), Some(true));
}

#[test]
fn the_link_says_this_machine_is_here_before_it_waits_for_anything() {
    // Nothing can connect to a machine it cannot find, so the advertisement is not
    // something to get round to after the first connection.
    let mut mac = SimHost::new();

    run(&mut mac, &listening());

    assert_eq!(mac.advertisements(), 1);
}

#[test]
fn a_keyboard_the_configuration_names_is_never_touched() {
    // What the flag exists for is favjit's own output device (`docs/platform/macos/output-through-a-virtual-hid-device.md`): a run
    // that delivers seizes what it captures, so reading that one back would take
    // its own virtual keyboard away from itself and every converted keystroke
    // would come home instead of reaching an application. Which is why "left
    // alone" has to mean untouched rather than unconverted — a keyboard taken
    // exclusively and then ignored is a keyboard that has stopped working.
    const IGNORED: DeviceId = DeviceId(2);
    let mut mac = SimHost::new();
    mac.attach_built_in(DeviceId(1))
        .attach_external(IGNORED, 1452, 591)
        .tap(IGNORED, Key::K)
        .tap(DeviceId(1), Key::K);

    sink::run(
        &converting(),
        Layout::dudrack(),
        None,
        InputConfig {
            ignore: vec![DeviceMatch::new(1452, 591)],
            skip_built_in: false,
        },
        &mut mac,
        None,
    );

    let asked = mac.did();
    assert!(
        !asked.contains(&Did::TookDevice {
            device: IGNORED,
            exclusive: true
        }) && !asked.contains(&Did::ReadDevice { device: IGNORED }),
        "the named keyboard was touched: {asked:?}"
    );
    assert!(
        asked.contains(&Did::ReadDevice {
            device: DeviceId(1)
        }),
        "and the others still are: {asked:?}"
    );
    assert_eq!(
        mac.reports().len(),
        2,
        "one key typed, from the keyboard that was not named — the other's went \
         nowhere"
    );
}

#[test]
fn only_the_pointer_the_output_goes_through_is_tuned_and_the_factor_goes_last() {
    // A run that wrote these to every pointing device the machine has would be
    // changing the person's own mouse. And a resolution written on its own does
    // not take effect until an acceleration is written after it, so the order is
    // the whole of whether the first write does anything (ADR-0011).
    let mut mac = SimHost::new()
        .told_to_tune(Some(80.0), Some(0.8))
        .with_pointers(vec![
            SimPointer::new(0x1234).with_fixed(pointer::RESOLUTION, 400.0),
            SimPointer::new(OUTPUT_VENDOR).with_fixed(pointer::RESOLUTION, 400.0),
        ]);
    mac.attach_built_in(DeviceId(1)).tap(DeviceId(1), Key::K);

    run(&mut mac, &converting());

    assert_eq!(mac.pointers()[0].written(), &[], "somebody else's mouse");
    assert_eq!(
        mac.pointers()[1].written(),
        &[
            (String::from(pointer::RESOLUTION), 80.0),
            (String::from(pointer::MOUSE_ACCELERATION), 0.8),
        ],
        "favjit's own, resolution first"
    );
}

#[test]
fn a_pointer_that_says_which_property_holds_its_factor_is_written_there() {
    // A mouse and a pointing stick keep their factor under different names, and
    // writing the wrong one is accepted and ignored — so a run that guessed would
    // report a number it had set and changed nothing.
    let mut mac = SimHost::new()
        .told_to_tune(None, Some(0.5))
        .with_pointers(vec![SimPointer::new(OUTPUT_VENDOR)
            .with_fixed(pointer::RESOLUTION, 400.0)
            .with_text(pointer::ACCELERATION_TYPE, pointer::ACCELERATION)]);
    mac.attach_built_in(DeviceId(1)).tap(DeviceId(1), Key::K);

    run(&mut mac, &converting());

    assert_eq!(
        mac.pointers()[0].written(),
        &[(String::from(pointer::ACCELERATION), 0.5)],
        "no resolution was asked for, and the factor went where the device said"
    );
}

#[test]
fn a_device_with_no_resolution_is_left_alone() {
    // Which of the machine's services is a pointing device at all: a keyboard is
    // in the same list, and the property being there is the same question as
    // whether there is anything to tune.
    let mut mac = SimHost::new()
        .told_to_tune(Some(80.0), None)
        .with_pointers(vec![SimPointer::new(OUTPUT_VENDOR)]);
    mac.attach_built_in(DeviceId(1)).tap(DeviceId(1), Key::K);

    run(&mut mac, &converting());

    assert_eq!(mac.pointers()[0].written(), &[]);
}

#[test]
fn a_run_told_no_numbers_writes_nothing() {
    // Writing a device's existing values back would be a change to a device
    // favjit was not asked to touch, and these outlive the process that made it.
    let mut mac = SimHost::new().with_pointers(vec![
        SimPointer::new(OUTPUT_VENDOR).with_fixed(pointer::RESOLUTION, 400.0)
    ]);
    mac.attach_built_in(DeviceId(1)).tap(DeviceId(1), Key::K);

    run(&mut mac, &converting());

    assert_eq!(mac.pointers()[0].written(), &[]);
}

#[test]
fn a_device_that_is_not_a_keyboard_is_never_taken() {
    // The machine reports every HID device it finds, so which of them types is
    // the run's own answer. A mouse taken as a keyboard would be seized, and the
    // pointer would stop working with nothing said about it.
    let mut mac = SimHost::new();
    mac.script(EventKind::HidDeviceFound {
        // The generic desktop page, at the usage a mouse declares rather than a
        // keyboard's: the pair is what names a device class, and a run that
        // read only the page would take both.
        device: DeviceId(1),
        primary_usage_page: Some(0x01),
        primary_usage: Some(0x02),
        transport: Some(String::from("USB")),
        product: Some(String::from("a mouse")),
        vendor_id: Some(1),
        product_id: Some(2),
    });
    mac.attach_external(DeviceId(2), 1, 2);

    run(&mut mac, &converting());

    let asked = mac.did();
    assert!(
        !asked.contains(&Did::TookDevice {
            device: DeviceId(1),
            exclusive: true
        }),
        "the mouse was taken: {asked:?}"
    );
    assert!(
        asked.contains(&Did::TookDevice {
            device: DeviceId(2),
            exclusive: true
        }),
        "and the keyboard beside it was not: {asked:?}"
    );
}

#[test]
fn either_signal_alone_makes_a_keyboard_the_macs_own() {
    // The two the machine's own keyboard can be recognised by, each without the
    // other: the SPI bus is the only one that works for a device with no vendor
    // or product, and the product string is the only one that survives a machine
    // whose internal keyboard is not on that bus. `skip_built_in` is what reads
    // the answer, so a signal that stopped being one shows up as a keyboard
    // favjit takes when it was asked not to.
    for (transport, product) in [("FIFO", "a keyboard"), ("USB", "Apple Internal Keyboard")] {
        let mut mac = SimHost::new();
        mac.script(EventKind::HidDeviceFound {
            device: DeviceId(1),
            primary_usage_page: Some(0x01),
            primary_usage: Some(0x06),
            transport: Some(String::from(transport)),
            product: Some(String::from(product)),
            vendor_id: None,
            product_id: None,
        });

        sink::run(
            &converting(),
            Layout::dudrack(),
            None,
            InputConfig {
                ignore: Vec::new(),
                skip_built_in: true,
            },
            &mut mac,
            None,
        );

        assert!(
            !mac.did().contains(&Did::TookDevice {
                device: DeviceId(1),
                exclusive: true
            }),
            "transport {transport:?} product {product:?}: {:?}",
            mac.did()
        );
    }
}

#[test]
fn the_macs_own_keyboard_can_be_left_alone_by_itself() {
    // Its own flag because it has no vendor or product for `ignore` to name, and
    // what it is for is a person who wants favjit on an external keyboard while
    // the built-in one keeps typing what it is printed with.
    let mut mac = SimHost::new();
    mac.attach_built_in(DeviceId(1))
        .attach_external(DeviceId(2), 1, 2)
        .tap(DeviceId(1), Key::K)
        .tap(DeviceId(2), Key::K);

    sink::run(
        &converting(),
        Layout::dudrack(),
        None,
        InputConfig {
            ignore: Vec::new(),
            skip_built_in: true,
        },
        &mut mac,
        None,
    );

    let asked = mac.did();
    assert!(
        !asked.contains(&Did::ReadDevice {
            device: DeviceId(1)
        }) && asked.contains(&Did::ReadDevice {
            device: DeviceId(2)
        }),
        "{asked:?}"
    );
}

#[test]
fn a_keyboard_this_process_cannot_have_is_named_and_the_rest_carry_on() {
    // Something else holds it, or this process is not privileged to take it. The
    // run has to carry on with the keyboards it did get: ending here would mean
    // one keyboard held by another program stops favjit converting on every
    // other, and the code is said because "nothing happens when I type" needs a
    // reason a person can act on.
    const REFUSED: DeviceId = DeviceId(2);
    let mut mac = SimHost::new().with_a_keyboard_it_cannot_take(REFUSED, -536_870_207);
    mac.attach_built_in(DeviceId(1))
        .attach_external(REFUSED, 1, 2)
        .tap(REFUSED, Key::K)
        .tap(DeviceId(1), Key::K);

    run(&mut mac, &converting());

    assert!(
        !mac.did().contains(&Did::ReadDevice { device: REFUSED }),
        "a keyboard that could not be taken must not be read"
    );
    assert_eq!(
        mac.warnings()
            .iter()
            .filter(|said| said.contains("0xe000"))
            .count(),
        1,
        "the code has to be said: {:?}",
        mac.warnings()
    );
    assert_eq!(
        mac.reports().len(),
        2,
        "and the keyboard it did get converts"
    );
}

#[test]
fn a_keyboard_that_goes_away_between_taking_and_reading_is_not_announced() {
    // The window between the two calls is real: a keyboard unplugged inside it is
    // taken and then cannot be read. Announcing it anyway would leave the run
    // holding rules for a device whose keys can never arrive.
    const GONE: DeviceId = DeviceId(2);
    let mut mac = SimHost::new().with_a_keyboard_it_cannot_read(GONE);
    mac.attach_built_in(DeviceId(1))
        .attach_external(GONE, 1, 2)
        .tap(DeviceId(1), Key::K);

    run(&mut mac, &converting());

    let asked = mac.did();
    assert!(
        asked.contains(&Did::ReadDevice { device: GONE }),
        "the run has to have tried: {asked:?}"
    );
    assert_eq!(mac.reports().len(), 2, "and the other keyboard still types");
}

#[test]
fn a_keyboard_taken_before_it_goes_away_is_still_released() {
    const GONE: DeviceId = DeviceId(1);
    let mut mac = SimHost::new().with_a_keyboard_it_cannot_read(GONE);
    mac.attach_built_in(GONE);

    run(&mut mac, &converting());

    assert!(mac.did().ends_with(&[
        Did::AskedForHeldDevice,
        Did::ReleasedDevice { device: GONE },
        Did::AskedForHeldDevice,
    ]));
}

#[test]
fn every_keyboard_is_released_one_at_a_time() {
    let mut mac = SimHost::new();
    mac.attach_built_in(DeviceId(1))
        .attach_external(DeviceId(2), 1, 2);

    run(&mut mac, &converting());

    assert!(mac.did().ends_with(&[
        Did::AskedForHeldDevice,
        Did::ReleasedDevice {
            device: DeviceId(2),
        },
        Did::AskedForHeldDevice,
        Did::ReleasedDevice {
            device: DeviceId(1),
        },
        Did::AskedForHeldDevice,
    ]));
}

#[test]
fn a_failed_release_stops_before_the_next_keyboard() {
    let mut mac = SimHost::new().with_a_keyboard_it_cannot_release(DeviceId(2), -1);
    mac.attach_built_in(DeviceId(1))
        .attach_external(DeviceId(2), 1, 2);

    run(&mut mac, &converting());

    assert!(mac.did().ends_with(&[
        Did::AskedForHeldDevice,
        Did::ReleasedDevice {
            device: DeviceId(2),
        },
    ]));
}

#[test]
fn a_dry_run_opens_no_output_and_no_link_and_takes_nothing_exclusively() {
    // What makes a dry run safe to start: it leaves neither a virtual keyboard nor
    // an open socket behind, and the keyboards stay the machine's own. There is
    // nothing to ask for beyond the mode, which is the point — suppressing and
    // listening are what a *delivering* run does, so a dry run has no way to be
    // asked for either of them.
    let mut mac = SimHost::new();

    run(&mut mac, &Request::DryRun);

    assert_eq!(
        mac.did(),
        vec![
            Did::AskedIfSwitchedOn,
            Did::AskedPermission,
            Did::LookedForDevices,
            Did::AskedForHeldDevice,
        ]
    );
}
