//! Keeping the other machine's keyboards apart from this machine's, and from
//! each other.
//!
//! Device ids are each machine's own numbering, and the rules read them: the
//! MacBook's built-in keyboard takes Dudrack's layers while an external one takes
//! the raw-JIS remaps (ADR-0003's one pipeline still asks *which* keyboard). Two
//! machines numbering from one would put a Windows keyboard on the built-in rules.
//!
//! None of this is asked about directly: the id a device ends up with is
//! `engine`'s own bookkeeping, never handed back across the host boundary. What
//! crosses it is bytes, so what these check is the difference a mix-up would
//! actually make to them — a key resolved under the wrong device's rules, or
//! released when it should have stayed down.

use favjit_engine::sink::InputConfig;
use favjit_engine::sink::Request;
use favjit_engine::{sink, DeviceId, Key, Layout};
use favjit_hid::report::Report;
use favjit_hid::{page, usage};
use favjit_host::OutputReport;
use favjit_host_sim::{identity, SimHost, SimLink};

/// A run that converts, since what these are about is which keyboard the rules see
/// rather than how the machine was brought up.
fn converting() -> Request {
    Request::Injecting { listen: true }
}

const PAIRED: u8 = 0xaa;

/// The id the Mac gives its own keyboard, and the one a source may also use.
const ONE: DeviceId = DeviceId(1);

/// The file `favjit --pair` leaves behind, holding that one key.
///
/// Written here rather than built with the simulator's own writer: a script that
/// started from the same function the simulator writes that file with would be
/// checking that function against itself.
fn paired_list() -> String {
    format!("{}\n", hex(identity(PAIRED).public()))
}

fn hex(bytes: &[u8]) -> String {
    use core::fmt::Write;
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// A whole run of the converter, with that script arriving over its link.
fn over_the_link(script: impl FnOnce(&mut SimLink)) -> SimHost {
    let mut link = SimLink::new(paired_list());
    link.connect(identity(PAIRED));
    script(&mut link);

    let mut mac = SimHost::new().with_link(link);
    // The Mac's own keyboard is here too, with the id a source might reuse: the
    // question is whether the sink can tell them apart at all.
    mac.attach_built_in(ONE);
    sink::run(
        &converting(),
        Layout::dudrack(),
        None,
        InputConfig::default(),
        &mut mac,
        None,
    );
    mac
}

/// Every report the run wrote, decoded back into the keyboard state it
/// described — nothing here types on a control's page.
fn keyboard_reports(mac: &SimHost) -> Vec<Report> {
    mac.reports()
        .iter()
        .map(|(_, report, bytes)| {
            assert_eq!(*report, OutputReport::Keyboard, "not a control");
            Report::from_bytes(bytes).expect("the bytes of a keyboard report")
        })
        .collect()
}

/// The report a keyboard holding only these keys would carry, from empty.
fn holding(keys: &[Key]) -> Report {
    after(&keys.iter().map(|&key| (key, true)).collect::<Vec<_>>())
}

/// The report after this sequence of presses (`true`) and releases (`false`),
/// carried forward exactly as one report does: a later step never starts over
/// from empty, so which slot a key lands in depends on what is already down
/// when it does.
fn after(steps: &[(Key, bool)]) -> Report {
    let mut report = Report::default();
    for &(key, down) in steps {
        let (on_page, code) = usage::of(key).expect("a key with a usage");
        assert_eq!(on_page, page::KEYBOARD_OR_KEYPAD, "not a plain key");
        match down {
            true => report.press(code),
            false => report.release(code),
        }
    }
    report
}

#[test]
fn a_remote_keyboard_numbered_one_is_not_the_built_in_one() {
    // The built-in space bar is a tap-hold in Dudrack and an external one is not.
    // A remote keyboard taken for the built-in one would turn its space into
    // shift, which a byte-for-byte look at the report would catch: shift held
    // alone writes a report of its own before the space ever does.
    let mac = over_the_link(|link| {
        link.attach_external(ONE, 1234, 5678)
            .press(ONE, Key::Spacebar)
            .release(ONE, Key::Spacebar)
            .hang_up();
    });

    assert_eq!(
        keyboard_reports(&mac),
        vec![holding(&[Key::Spacebar]), Report::default()],
        "a plain space down and up, not a shift the built-in's own tap-hold would add"
    );
}

#[test]
fn a_key_released_over_the_link_does_not_stay_down() {
    // The sink's held-key bookkeeping is keyed on the device id it resolved,
    // once per device: a press that arrived under one id and a release under
    // another would leave the report holding a key nobody is touching any
    // more, through however many cycles follow.
    let mac = over_the_link(|link| {
        link.attach_external(ONE, 1234, 5678)
            .press(ONE, Key::J)
            .release(ONE, Key::J)
            .press(ONE, Key::J)
            .release(ONE, Key::J)
            .hang_up();
    });

    assert_eq!(
        keyboard_reports(&mac),
        vec![
            holding(&[Key::J]),
            Report::default(),
            holding(&[Key::J]),
            Report::default(),
        ]
    );
}

#[test]
fn two_source_devices_are_not_one_device_that_answers_to_both_ids() {
    // If the sink read the two as one device, the first one's detach would
    // release everything held under that shared bookkeeping — including the
    // second keyboard's key, which nothing has told this machine has gone.
    let mac = over_the_link(|link| {
        link.attach_external(DeviceId(1), 1, 2)
            .attach_external(DeviceId(2), 3, 4)
            .press(DeviceId(1), Key::J)
            .press(DeviceId(2), Key::K)
            .detach(DeviceId(1))
            .hang_up();
    });

    assert_eq!(
        keyboard_reports(&mac),
        vec![
            after(&[(Key::J, true)]),
            after(&[(Key::J, true), (Key::K, true)]),
            // The first keyboard's `j` let go because it left; the second
            // keyboard's `k` is still down, because nothing said it left too.
            after(&[(Key::J, true), (Key::K, true), (Key::J, false)]),
            // The session ending is what finally says the second keyboard has
            // gone too, releasing `k` along with it.
            after(&[
                (Key::J, true),
                (Key::K, true),
                (Key::J, false),
                (Key::K, false)
            ]),
        ]
    );
}
