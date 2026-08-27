//! What the MacBook's top row does once favjit holds the keyboard.
//!
//! That row sends `F1` to `F12` and nothing else, and the brightness and volume
//! icons printed on it are the OS's reading of its own keyboard
//! (`docs/platform/macos/hid-input-callbacks.md`). A key taken from that keyboard
//! and handed back through a virtual device has lost that reading, so the icons are
//! the layout's to reproduce — and these pin that it does, on that keyboard and not
//! on the others.
//!
//! This row is the only thing that reaches the output device's *other* reports,
//! one per page (`ControlPage`), and which report a control goes out on is the
//! whole of whether it arrives: the same usage number is a different control on
//! each page. So these read the bytes back on the page the run wrote them to —
//! `ControlReport::from_bytes` refuses bytes whose report id is not that page's,
//! which is the check that a decided-and-then-misaddressed control cannot pass.

use favjit_engine::sink::{self, InputConfig, Request};
use favjit_engine::{DeviceId, Key, Layout};
use favjit_hid::report::{ControlPage, ControlReport, Keys, Report, Sent};
use favjit_hid::{page, usage};
use favjit_host::OutputReport;
use favjit_host_sim::SimHost;

/// The MacBook's own keyboard, whose row this is about.
const BUILT_IN: DeviceId = DeviceId(1);
/// An external keyboard, whose top row is printed `F1` and means it.
const EXTERNAL: DeviceId = DeviceId(2);

fn converting() -> Request {
    Request::Injecting { listen: false }
}

/// Every report a run wrote, decoded on the page its own report id names.
fn run(script: impl FnOnce(&mut SimHost)) -> Vec<Sent> {
    let mut host = SimHost::new();
    host.attach_built_in(BUILT_IN);
    host.attach_external(EXTERNAL, 6127, 24801);
    script(&mut host);
    sink::run(
        &converting(),
        Layout::dudrack(),
        None,
        InputConfig::default(),
        &mut host,
        None,
    );
    host.reports()
        .iter()
        .map(|(_, report, bytes)| decode(*report, bytes))
        .collect()
}

/// One report, read back through the inverse of the encoding that wrote it.
///
/// The page comes from which report the host was asked to write rather than
/// from the bytes: two pages can share a report id only by coincidence, so a
/// decode that guessed would agree with a control sent on the wrong page.
fn decode(report: OutputReport, bytes: &[u8]) -> Sent {
    let control = |page| {
        Sent::Control(
            ControlReport::from_bytes(page, bytes).expect("the bytes of a control report"),
        )
    };
    match report {
        OutputReport::Keyboard => {
            Sent::Keyboard(Report::from_bytes(bytes).expect("the bytes of a keyboard report"))
        }
        OutputReport::Consumer => control(ControlPage::Consumer),
        OutputReport::AppleVendorTopCase => control(ControlPage::AppleVendorTopCase),
        OutputReport::AppleVendorKeyboard => control(ControlPage::AppleVendorKeyboard),
        OutputReport::GenericDesktop => control(ControlPage::GenericDesktop),
        // `OutputReport` is `#[non_exhaustive]`, and nothing here moves a pointer.
        other => panic!("not a keyboard or control report: {other:?}"),
    }
}

fn tapped(device: DeviceId, key: Key) -> Vec<Sent> {
    run(|host| {
        host.tap(device, key);
    })
}

/// The report a device holding just this key carries, on whichever of the
/// output device's reports this key's own page is declared on.
fn holding(key: Key) -> Sent {
    let (on_page, code) = usage::of(key).expect("a key with a usage");
    match ControlPage::of(on_page) {
        Some(on_page) => {
            let mut keys = Keys::default();
            keys.press(code);
            Sent::Control(ControlReport {
                page: on_page,
                keys,
            })
        }
        None => {
            assert_eq!(on_page, page::KEYBOARD_OR_KEYPAD, "not a plain key");
            let mut report = Report::default();
            report.press(code);
            Sent::Keyboard(report)
        }
    }
}

/// The report that follows it once the key is up, holding nothing.
///
/// On the same page as the press, and that is the point: a release written to
/// the keyboard report would leave the control held down on its own page, where
/// the OS goes on ramping brightness with nothing to stop it.
fn released(key: Key) -> Sent {
    let (on_page, _) = usage::of(key).expect("a key with a usage");
    match ControlPage::of(on_page) {
        Some(page) => Sent::Control(ControlReport {
            page,
            keys: Keys::default(),
        }),
        None => Sent::Keyboard(Report::default()),
    }
}

#[test]
fn the_whole_row_is_what_its_icons_say() {
    // The pairing is Karabiner-Elements' own default for this row, which is what
    // the keyboard did before favjit took it.
    //
    // Three of the device's four control reports are reached from this one table —
    // the consumer page, Apple's own keyboard page and the generic desktop — so a
    // control written to the wrong one of them fails here rather than on a machine.
    for (pressed, expected) in [
        (Key::F1, Key::BrightnessDown),
        (Key::F2, Key::BrightnessUp),
        (Key::F3, Key::MissionControl),
        (Key::F4, Key::Spotlight),
        (Key::F5, Key::Dictation),
        (Key::F6, Key::DoNotDisturb),
        (Key::F7, Key::Rewind),
        (Key::F8, Key::PlayPause),
        (Key::F9, Key::FastForward),
        (Key::F10, Key::Mute),
        (Key::F11, Key::VolumeDown),
        (Key::F12, Key::VolumeUp),
    ] {
        assert_eq!(
            tapped(BUILT_IN, pressed),
            vec![holding(expected), released(expected)],
            "{pressed:?} on the built-in keyboard"
        );
    }
}

#[test]
fn the_release_is_converted_with_the_press() {
    // A control the OS is left holding ramps on its own — brightness runs to one
    // end of its range — so the half that matters is the one nothing else can
    // repair.
    let reports = tapped(BUILT_IN, Key::F11);
    assert_eq!(reports.last(), Some(&released(Key::VolumeDown)));
}

#[test]
fn an_external_keyboards_top_row_is_left_alone() {
    // Printed `F1` there, and a person pressing it means the function key. Nothing
    // about it is scoped to the Mac's own keyboard by accident: it is the only
    // keyboard whose row is printed with the icons.
    assert_eq!(
        tapped(EXTERNAL, Key::F1),
        vec![holding(Key::F1), released(Key::F1)],
        "an external F1 passes straight through"
    );
}

#[test]
fn a_keyboard_that_sends_the_control_itself_is_passed_through() {
    // Some keyboards have volume keys of their own and send the control rather
    // than a function key. Nothing converts those, and nothing needs to.
    assert_eq!(
        tapped(EXTERNAL, Key::VolumeUp),
        vec![holding(Key::VolumeUp), released(Key::VolumeUp)]
    );
}

#[test]
fn a_control_goes_out_on_its_own_page_and_not_the_keyboards() {
    // What the four reports are for: a control's usage number means something
    // else on the keyboard page, so one written there arrives as whatever that
    // number is a key for. Asserted as the report the host was asked for rather
    // than only as the bytes, because that is the half the bytes cannot say.
    let reports = tapped(BUILT_IN, Key::F12);

    assert!(
        reports.iter().all(
            |sent| matches!(sent, Sent::Control(control) if control.page == ControlPage::Consumer)
        ),
        "the volume keys are on the consumer page: {reports:?}"
    );
}

#[test]
fn a_modifier_held_over_a_control_still_reaches_the_keyboard_report() {
    // Shift+volume is how macOS steps the volume finely, so the modifier has to
    // arrive with the control. It cannot ride the control's own report — the
    // descriptor declares those eight bits on the keyboard's report and nowhere
    // else — so the keyboard report goes out beside it, carrying the modifier and
    // no key.
    let shift = Report {
        modifiers: favjit_hid::report::modifier_bit(Key::LeftShift).expect("a modifier"),
        ..Report::default()
    };

    let reports = run(|host| {
        host.press(BUILT_IN, Key::LeftShift);
        host.tap(BUILT_IN, Key::F12);
        host.release(BUILT_IN, Key::LeftShift);
    });

    assert_eq!(
        reports,
        vec![
            Sent::Keyboard(shift),
            holding(Key::VolumeUp),
            released(Key::VolumeUp),
            Sent::Keyboard(Report::default()),
        ],
        "the modifier is said on the keyboard report, the control on its own"
    );
}

#[test]
fn the_row_converts_in_every_layer() {
    // The layers are entered by holding a key on the home row, and the top row is
    // not part of any of them: brightness in the middle of a Henkan hold is still
    // brightness, not the layer's meaning for that position.
    let reports = run(|host| {
        host.press(BUILT_IN, Key::RightCommand);
        host.tap(BUILT_IN, Key::F2);
        host.release(BUILT_IN, Key::RightCommand);
    });

    assert_eq!(
        reports,
        vec![holding(Key::BrightnessUp), released(Key::BrightnessUp)],
        "the Henkan layer swallowed it"
    );
}
