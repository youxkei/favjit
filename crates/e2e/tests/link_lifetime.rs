//! How long the link lasts, and what the run does when it stops being served.
//!
//! Two failures pull in opposite directions. A link that gave up on the first
//! connection it could not use would be a Windows keyboard that stops working
//! because something scanned the Mac. A link that never gave up would spin on a
//! socket that has stopped answering, holding the port with nothing being served.
//!
//! And when it does stop, the run ends. Nothing rebinds the socket inside a run and
//! the advertisement goes with it, so a converter that stayed up would look well
//! here while being unreachable from the other machine (ADR-0012) — ending is what
//! lets whatever supervises favjit open the socket again (ADR-0008).

use favjit_engine::link::FAILURES;
use favjit_engine::sink::{self, Ending, InputConfig, Request};
use favjit_engine::{DeviceId, Key, Layout};
use favjit_hid::report::{modifier_bit, Report};
use favjit_hid::{page, usage};
use favjit_host::OutputReport;
use favjit_host_sim::{identity, SimHost, SimLink};

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

const PAIRED: u8 = 0xaa;
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

fn converting() -> Request {
    Request::Injecting { listen: true }
}

/// A whole run, and how it ended, with that script on the other end of its link.
fn served(script: impl FnOnce(&mut SimLink)) -> (SimHost, Ending) {
    let mut link = SimLink::new(paired_list());
    script(&mut link);
    let mut mac = SimHost::new().with_link(link);
    let (ending, _) = sink::run(
        &converting(),
        Layout::dudrack(),
        None,
        InputConfig::default(),
        &mut mac,
        None,
    );
    (mac, ending)
}

/// One keystroke over the link, for a script that wants the link to have worked.
fn types(link: &mut SimLink) {
    link.connect(identity(PAIRED))
        .attach_external(REMOTE, 1, 2)
        .press(REMOTE, Key::K)
        .release(REMOTE, Key::K)
        .hang_up();
}

#[test]
fn a_link_that_is_serving_does_not_end_the_run() {
    // The ordinary case, and the one the others are worth measuring against: a run
    // ends because it was asked to, not because it has a link.
    let (mac, ending) = served(types);

    assert_eq!(ending, Ending::Converted);
    assert!(!keyboard_reports(&mac).is_empty());
}

#[test]
fn a_connection_that_cannot_be_used_does_not_end_the_link() {
    // What a port scan produces, and what a machine on the same desk being switched
    // off produces. The source that connects afterwards has to get in.
    let (mac, ending) = served(|link| {
        link.rejected_times(FAILURES - 1);
        types(link);
    });

    assert_eq!(ending, Ending::Converted);
    assert!(
        !keyboard_reports(&mac).is_empty(),
        "the source after them was served"
    );
}

#[test]
fn a_connection_that_worked_sets_the_count_back() {
    // Otherwise a run left going for a week would end on its own, having collected
    // enough unusable connections between working ones to reach the bound.
    let (mac, ending) = served(|link| {
        link.rejected_times(FAILURES - 1);
        types(link);
        link.rejected_times(FAILURES - 1);
    });

    assert_eq!(ending, Ending::Converted);
    assert!(!keyboard_reports(&mac).is_empty());
}

#[test]
fn enough_of_them_in_a_row_and_the_link_gives_up() {
    // A socket that fails every call would otherwise be a loop turning at full
    // speed for the rest of the run, with nothing being served at the end of it.
    let (mac, ending) = served(|link| {
        link.rejected_times(FAILURES);
        types(link);
    });

    assert_eq!(ending, Ending::LinkGone);
    assert_eq!(
        keyboard_reports(&mac),
        Vec::new(),
        "it gave up before the source that came after them"
    );
}

#[test]
fn a_listener_that_is_gone_ends_the_run() {
    // The socket itself, rather than a connection on it: there is nothing left to
    // accept on, and no run rebinds one.
    let (_, ending) = served(|link| {
        link.listener_gone();
    });

    assert_eq!(ending, Ending::LinkGone);
}

#[test]
fn what_the_link_delivered_before_it_went_is_converted_first() {
    // The keystrokes happened. A run that ended the moment its link did would drop
    // the last of them — and the release of a held key is the one that matters,
    // since a shift left down is down in every application (ADR-0002).
    let (mac, ending) = served(|link| {
        link.connect(identity(PAIRED))
            .attach_external(REMOTE, 1, 2)
            .press(REMOTE, Key::LeftShift)
            .hang_up();
        link.listener_gone();
    });

    assert_eq!(ending, Ending::LinkGone);
    assert_eq!(
        keyboard_reports(&mac),
        vec![
            after(&[(Key::LeftShift, true)]),
            after(&[(Key::LeftShift, true), (Key::LeftShift, false)]),
        ]
    );
}

#[test]
fn a_run_that_never_listened_is_not_ended_by_a_link() {
    // There is no loop turning alongside this one, so there is nothing to come back
    // and nothing to report — a dry run must not end as though its link had gone.
    let mut mac = SimHost::new();
    mac.attach_built_in(DeviceId(1)).tap(DeviceId(1), Key::K);

    let (ending, _) = sink::run(
        &Request::Injecting { listen: false },
        Layout::dudrack(),
        None,
        InputConfig::default(),
        &mut mac,
        None,
    );

    assert_eq!(ending, Ending::Converted);
}
