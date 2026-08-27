//! Both machines running, one after the other, with nothing between them but what
//! would cross a socket.
//!
//! `link_relay.rs` compares the two sides by handing the source's messages to the
//! sink's own stream, which is the right shape for asking whether conversion is the
//! same either way. `link_lifetime.rs` serves a link inside a run, with the other
//! end scripted by hand. Neither has a *source* on the other end — so the frames a
//! real forwarding run decided to send are read by nothing, and the device
//! renaming, the refusal of an unpaired machine and the release at the end of a
//! session are all checked against an idea of what a source would send.
//!
//! Checked in the bytes the sink actually wrote, decoded back with `favjit-hid`'s
//! inverse of the encoding — `Injected` is `engine`'s own record of what it
//! decided, not anything a device downstream of it ever sees, and which id the
//! sink renamed a source's device to never crosses the host boundary at all.

use favjit_engine::pairing::Identity;
use favjit_engine::sink::{self, InputConfig, Request};
use favjit_engine::{source, DeviceId, Key, Layout};
use favjit_hid::report::{modifier_bit, Report};
use favjit_hid::{page, usage};
use favjit_host::source::Driving;
use favjit_host::OutputReport;
use favjit_host_sim::{identity, SimHost, SimLink, SimSource};

/// The keyboard on the Windows side, as that machine numbers it.
const KEYBOARD: DeviceId = DeviceId(3);

/// The Windows machine's identity, as the Mac has pinned it.
const PAIRED: u8 = 7;

/// The sink the source run that produces what [`both_ends`] hands the
/// simulated link is pinned to — unrelated to [`PAIRED`], which is the
/// identity *that* link is scripted to see arrive.
const THIS_MACHINES_SINK: u8 = 21;

fn paired() -> Identity {
    identity(PAIRED)
}

/// The keyboard both halves of these tests attach, by the vendor and product a
/// rule names it with.
///
/// The same pair on both machines on purpose: the whole comparison is that a key
/// typed on one arrives converted the same way as one typed on the other, and a
/// rule that named only one of them would be comparing two layouts.
const KEYBOARD_IDENTITY: (u16, u16) = (1234, 5678);

fn converting() -> Request {
    Request::Injecting { listen: true }
}

fn forwarding() -> source::Request {
    source::Request::Relaying
}

fn hex(bytes: &[u8]) -> String {
    use core::fmt::Write;
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// Every report a run wrote, decoded back into the keyboard state it
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

/// Run a whole forwarding run on the Windows machine, then a whole converting run
/// on the Mac with what it sent on the other end of the Mac's link.
///
/// Two runs and nothing else: the link is served inside the Mac's, which is where
/// it is served on a real machine, so the source's frames reach the converter the
/// way they would rather than by being scripted into its stream.
fn both_ends(peer: Identity, script: impl Fn(&mut SimSource)) -> SimHost {
    let mut windows = SimSource::new();
    // Sent over first, because a run comes up with the keyboard on the machine it is
    // running on and nothing crosses until it is asked to (ADR-0013).
    windows.asked_for(Driving::TheSink);
    windows.pinned_to(identity(THIS_MACHINES_SINK));
    script(&mut windows);
    source::run(&forwarding(), false, &mut windows, None);

    let mut link = SimLink::new(format!("{}\n", hex(paired().public())));
    link.connect(peer);
    link.relay(&windows.sent());

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

/// The same script on a keyboard plugged into the Mac, for comparing against.
fn typed_here(script: impl Fn(&mut SimHost)) -> Vec<Report> {
    let mut mac = SimHost::new();
    mac.attach_external(KEYBOARD, KEYBOARD_IDENTITY.0, KEYBOARD_IDENTITY.1);
    script(&mut mac);
    sink::run(
        &converting(),
        Layout::dudrack(),
        None,
        InputConfig::default(),
        &mut mac,
        None,
    );
    keyboard_reports(&mac)
}

#[test]
fn a_keystroke_typed_on_windows_comes_out_as_one_typed_here() {
    // The whole topology through every piece of it at once, compared against the
    // one thing it has to equal (ADR-0003). Compared rather than named, because
    // what a key converts to is the layout's to say and this is about the path:
    // naming it here would make a rule change fail in two places.
    let over_the_link = both_ends(paired(), |windows| {
        windows.attach_external(KEYBOARD, KEYBOARD_IDENTITY.0, KEYBOARD_IDENTITY.1);
        windows.tap(KEYBOARD, Key::International3);
    });
    let here = typed_here(|mac| {
        mac.tap(KEYBOARD, Key::International3);
    });

    assert_eq!(keyboard_reports(&over_the_link), here);
    assert!(!here.is_empty(), "the script should convert to something");
}

#[test]
fn the_windows_keyboard_is_renamed_before_the_rules_see_it() {
    // Both machines number their devices from one, so a Windows keyboard called 3
    // would be converted as whatever the Mac calls 3 if the number were reused
    // verbatim. Given a Mac keyboard 3 of its own here — built-in, where the
    // space bar is a tap-hold in Dudrack — a renamed id keeps the source's plain
    // external space bar plain; a reused one would turn it into shift.
    let mut windows = SimSource::new();
    // Sent over first, because a run comes up with the keyboard on the machine it is
    // running on and nothing crosses until it is asked to (ADR-0013).
    windows.asked_for(Driving::TheSink);
    windows.pinned_to(identity(THIS_MACHINES_SINK));
    windows.attach_external(KEYBOARD, KEYBOARD_IDENTITY.0, KEYBOARD_IDENTITY.1);
    windows.press(KEYBOARD, Key::Spacebar);
    windows.release(KEYBOARD, Key::Spacebar);
    source::run(&forwarding(), false, &mut windows, None);

    let mut link = SimLink::new(format!("{}\n", hex(paired().public())));
    link.connect(paired());
    link.relay(&windows.sent());

    let mut mac = SimHost::new().with_link(link);
    mac.attach_built_in(KEYBOARD);
    sink::run(
        &converting(),
        Layout::dudrack(),
        None,
        InputConfig::default(),
        &mut mac,
        None,
    );

    assert_eq!(
        keyboard_reports(&mac),
        vec![after(&[(Key::Spacebar, true)]), Report::default()],
        "a plain space down and up, not a shift the built-in's own tap-hold would add"
    );
}

#[test]
fn a_source_the_mac_has_not_paired_gets_nothing_through() {
    // Refused by the handshake not completing, before a frame is read — so the
    // keystrokes it relayed reach no rule and nothing is injected (ADR-0004).
    let mac = both_ends(identity(9), |windows| {
        windows.attach_external(KEYBOARD, KEYBOARD_IDENTITY.0, KEYBOARD_IDENTITY.1);
        windows.tap(KEYBOARD, Key::K);
    });

    // The refusal and nothing else: no keystroke reached the converter, and what
    // did is the record that says why (ADR-0009).
    assert_eq!(
        mac.delivered(),
        vec![favjit_host::EventKind::LinkEnded {
            why: favjit_engine::link::Refused::NotPaired as u32
        }],
        "nothing but the refusal reached the converter"
    );
    assert_eq!(
        keyboard_reports(&mac),
        Vec::new(),
        "and nothing reached applications"
    );
}

#[test]
fn a_key_still_held_when_the_session_ends_is_released_on_the_mac() {
    // The failure ADR-0002 puts on the sink, driven from the machine that caused
    // it: a dropped network sends no key-up, so the session ending has to say the
    // source's keyboards are gone and the sink has to let go of what it told the OS
    // was down. Left held, this is a modifier stuck in every application.
    let mac = both_ends(paired(), |windows| {
        windows.attach_external(KEYBOARD, KEYBOARD_IDENTITY.0, KEYBOARD_IDENTITY.1);
        windows.press(KEYBOARD, Key::International3);
    });

    // Read off whatever the layout actually converted `International3` to,
    // rather than named here: what a key converts to is the layout's own rule to
    // state, and this is only about whether the session's end let go of it.
    let reports = keyboard_reports(&mac);
    assert_eq!(reports.len(), 2, "one press, one release: {reports:?}");
    assert_ne!(reports[0], Report::default(), "the key reached the OS");
    assert_eq!(
        reports[1],
        Report::default(),
        "the last thing applications see is the key being let go"
    );
}
