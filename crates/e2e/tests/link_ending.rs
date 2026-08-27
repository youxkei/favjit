//! What happens to keys that were down when the link went away.
//!
//! The sink is what the OS believes. A source that vanishes mid-keystroke must not
//! leave a modifier held in every application, which is the failure ADR-0002 puts
//! on the sink — and a network that drops gives no detach, so nothing but the end
//! of the session says the keys are gone.
//!
//! Checked in the reports the run actually wrote: which device the sink resolved
//! a key to is `engine`'s own bookkeeping and never crosses back out, but whether
//! a held key came back up does, in the bytes.

use favjit_engine::link::Attached;
use favjit_engine::sink::{InputConfig, Request};
use favjit_engine::{sink, DeviceId, Key, Layout};
use favjit_hid::report::{modifier_bit, Report};
use favjit_hid::{page, usage};
use favjit_host::OutputReport;
use favjit_host_sim::{identity, SimHost, SimLink};

/// A run that converts, since what these are about is what the sink does with what
/// the link delivered rather than how the machine was brought up.
fn converting() -> Request {
    Request::Injecting { listen: true }
}

const PAIRED: u8 = 0xaa;

/// What the source calls the keyboard. What this machine calls it in return is
/// never asked here: both machines number their own devices from one, and
/// resolving that pair onto one id is `engine`'s bookkeeping, invisible on
/// either side of the host boundary it lives behind.
const REMOTE: DeviceId = DeviceId(7);

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

/// A whole run of the converter, with the other machine on the other end of its
/// link.
fn served(script: impl FnOnce(&mut SimLink)) -> SimHost {
    let mut link = SimLink::new(paired_list());
    script(&mut link);
    let mut mac = SimHost::new().with_link(link);
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

/// The report after this sequence of presses (`true`) and releases (`false`).
fn after(steps: &[(Key, bool)]) -> Report {
    let mut report = Report::default();
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
fn a_shift_held_when_the_link_drops_comes_back_up() {
    // The whole point: what the OS was told is down has to be told it is up, or
    // every window is shift-clicked from then on. Nothing but the session
    // ending says so — a network that drops gives no detach of its own.
    let mac = served(|link| {
        link.connect(identity(PAIRED))
            .attach(Attached {
                device: REMOTE,
                is_built_in: false,
                vendor_id: Some(1),
                product_id: Some(2),
            })
            .press(REMOTE, Key::LeftShift)
            .hang_up();
    });

    assert_eq!(
        keyboard_reports(&mac),
        vec![
            after(&[(Key::LeftShift, true)]),
            after(&[(Key::LeftShift, true), (Key::LeftShift, false)]),
        ]
    );
}

#[test]
fn a_device_that_only_ever_sent_keys_is_still_released() {
    // A source may connect with keys already down, so the first thing the sink
    // hears about a device can be a key event rather than an announcement.
    // Releasing only what was announced would leave this one held.
    let mac = served(|link| {
        link.connect(identity(PAIRED))
            .press(REMOTE, Key::LeftShift)
            .hang_up();
    });

    assert_eq!(
        keyboard_reports(&mac),
        vec![
            after(&[(Key::LeftShift, true)]),
            after(&[(Key::LeftShift, true), (Key::LeftShift, false)]),
        ]
    );
}

#[test]
fn a_detach_the_source_sent_is_not_repeated_at_the_end() {
    // A source that unplugged a keyboard says so, and the sink has already
    // released it there. Saying it again at the end of the session would be a
    // second release for a report that already shows nothing held — harmless to
    // the OS, but not a thing the hardware ever asked for.
    let mac = served(|link| {
        link.connect(identity(PAIRED))
            .attach(Attached {
                device: REMOTE,
                is_built_in: false,
                vendor_id: Some(1),
                product_id: Some(2),
            })
            .press(REMOTE, Key::LeftShift)
            .detach(REMOTE)
            .hang_up();
    });

    assert_eq!(
        keyboard_reports(&mac),
        vec![
            after(&[(Key::LeftShift, true)]),
            after(&[(Key::LeftShift, true), (Key::LeftShift, false)]),
        ],
        "exactly the explicit detach's own release, nothing further at the end"
    );
}

#[test]
fn the_end_of_a_session_is_said_once_for_each_one_that_ends() {
    // The releases are what the OS is told, and nothing tells the person. A source
    // that stopped forwarding has its own line for the same moment, and which end
    // let go first is a question neither log answers alone — so this end says its
    // half whether or not it had a reason of its own to give.
    //
    // Counted rather than merely found: two sessions ending is two endings, and a
    // line said once per run would be a log that cannot be read as a sequence.
    let mac = served(|link| {
        link.connect(identity(PAIRED))
            .press(REMOTE, Key::LeftShift)
            .hang_up();
        link.connect(identity(PAIRED))
            .press(DeviceId(9), Key::A)
            .hang_up();
    });

    assert_eq!(
        mac.link_closed()
            .iter()
            .filter(|said| said.contains("session is over"))
            .count(),
        2,
        "what the link said: {:?}",
        mac.link_closed()
    );
}

#[test]
fn the_next_session_starts_with_nothing_held() {
    // Each connection is its own set of hands: a device released at the end of
    // one must not disturb the next one's own key, which arrives on a device
    // number the first session never mentioned.
    let mac = served(|link| {
        link.connect(identity(PAIRED))
            .attach(Attached {
                device: REMOTE,
                is_built_in: false,
                vendor_id: Some(1),
                product_id: Some(2),
            })
            .press(REMOTE, Key::LeftShift)
            .hang_up();
        link.connect(identity(PAIRED))
            .press(DeviceId(9), Key::A)
            .hang_up();
    });

    assert_eq!(
        keyboard_reports(&mac),
        vec![
            after(&[(Key::LeftShift, true)]),
            after(&[(Key::LeftShift, true), (Key::LeftShift, false)]),
            after(&[(Key::A, true)]),
            after(&[(Key::A, true), (Key::A, false)]),
        ]
    );
}
