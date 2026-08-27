//! What the Mac's own keyboards produce, end to end.
//!
//! Press a key on a keyboard, assert what reaches applications. Every case is a
//! whole run of the converter against a simulated machine (ADR-0007), so nothing
//! here reaches into the pipeline to ask it a question directly.
//!
//! Three keyboards are attached throughout, because most of what is asserted is
//! that the same physical key means different things depending on which one it
//! came from.

use core::time::Duration;

use favjit_engine::sink::{self, InputConfig, Request};
use favjit_engine::{DeviceId, Instant, Key, Layout};
use favjit_hid::report::{
    modifier_bit, ControlPage, ControlReport, Report as KeyboardReport, Sent,
};
use favjit_hid::{page, usage};
use favjit_host::{EventKind, OutputReport};
use favjit_host_sim::SimHost;

/// The MacBook's own keyboard. Physically US, typed in Dudrack.
const BUILT_IN: DeviceId = DeviceId(1);
/// Lenovo TrackPoint Keyboard II: JIS QWERTY, also typed in Dudrack.
const TRACKPOINT: DeviceId = DeviceId(2);
/// Any other external JIS keyboard, typed by its labels.
const RAW_JIS: DeviceId = DeviceId(3);

/// The modifier a rule adds to its output, as the key the sink names for it.
///
/// The left-hand key of each pair: a rule asks for a modifier and not for a side,
/// so the sink picks one, and these are what it picks. A rule that wants the other
/// hand emits that key instead.
const SHIFT: u8 = bit(Key::LeftShift);
const CONTROL: u8 = bit(Key::LeftControl);
const OPTION: u8 = bit(Key::LeftOption);
const COMMAND: u8 = bit(Key::LeftCommand);

/// The bit a modifier key contributes to a report's modifier byte.
///
/// Off [`modifier_bit`] rather than written down: it is the table a report is
/// read back against here, and a second set of bit positions beside it would
/// agree with the device by coincidence. Zero for a key that is no modifier,
/// which no caller below asks for.
const fn bit(key: Key) -> u8 {
    match modifier_bit(key) {
        Some(bit) => bit,
        None => 0,
    }
}

/// The two external keyboards, by the vendor and product a rule names each
/// with. The built-in one has neither, the way the Mac's own reports.
const TRACKPOINT_IDENTITY: (u16, u16) = (6127, 24801);
const RAW_JIS_IDENTITY: (u16, u16) = (0x1234, 0x5678);

/// Put all three keyboards on the machine.
fn attach_keyboards(host: &mut SimHost) {
    host.attach_built_in(BUILT_IN);
    host.attach_external(TRACKPOINT, TRACKPOINT_IDENTITY.0, TRACKPOINT_IDENTITY.1);
    host.attach_external(RAW_JIS, RAW_JIS_IDENTITY.0, RAW_JIS_IDENTITY.1);
}

/// A whole run, with every keyboard report it wrote decoded back into the
/// state it described — not the `Injected` values that produced them, which
/// is `engine`'s own record of what it decided rather than anything the
/// device downstream of it ever sees.
fn run(script: impl FnOnce(&mut SimHost)) -> Vec<KeyboardReport> {
    let mut host = SimHost::new();
    attach_keyboards(&mut host);
    script(&mut host);
    // No repeat: what a held key converts to is one question, and how often it
    // is sent again is another, pinned in `key_repeat.rs`.
    sink::run(
        &Request::Injecting { listen: false },
        Layout::dudrack(),
        None,
        InputConfig::default(),
        &mut host,
        None,
    );
    host.reports_while_reading()
        .iter()
        .map(|(_, report, bytes)| {
            assert_eq!(*report, OutputReport::Keyboard, "not a control");
            KeyboardReport::from_bytes(bytes).expect("the bytes of a keyboard report")
        })
        .collect()
}

/// The same run, decoded on whichever of the device's reports each one
/// actually carries — a control included, unlike [`run`].
fn device_reports(script: impl FnOnce(&mut SimHost)) -> Vec<Sent> {
    let mut host = SimHost::new();
    attach_keyboards(&mut host);
    script(&mut host);
    sink::run(
        &Request::Injecting { listen: false },
        Layout::dudrack(),
        None,
        InputConfig::default(),
        &mut host,
        None,
    );
    host.reports_while_reading()
        .iter()
        .map(|(_, report, bytes)| match *report {
            OutputReport::Keyboard => {
                Sent::Keyboard(KeyboardReport::from_bytes(bytes).expect("a keyboard report"))
            }
            OutputReport::Consumer => Sent::Control(
                ControlReport::from_bytes(ControlPage::Consumer, bytes).expect("a control report"),
            ),
            OutputReport::AppleVendorTopCase => Sent::Control(
                ControlReport::from_bytes(ControlPage::AppleVendorTopCase, bytes)
                    .expect("a control report"),
            ),
            OutputReport::AppleVendorKeyboard => Sent::Control(
                ControlReport::from_bytes(ControlPage::AppleVendorKeyboard, bytes)
                    .expect("a control report"),
            ),
            OutputReport::GenericDesktop => Sent::Control(
                ControlReport::from_bytes(ControlPage::GenericDesktop, bytes)
                    .expect("a control report"),
            ),
            // `OutputReport` is `#[non_exhaustive]`, and nothing here ever
            // asks for a pointing report.
            other => panic!("not a keyboard or control report: {other:?}"),
        })
        .collect()
}

/// The modifier byte of the last keyboard report a script produced.
fn last_modifiers(script: impl FnOnce(&mut SimHost)) -> u8 {
    device_reports(script)
        .into_iter()
        .rev()
        .find_map(|sent| match sent {
            Sent::Keyboard(report) => Some(report.modifiers),
            Sent::Control(_) => None,
        })
        .expect("at least one keyboard report")
}

/// The report `holds` alone, followed by `key`, would carry — built forward
/// from `base`, the same way one report is carried from the last rather than
/// rebuilt from scratch.
fn expected_after(base: KeyboardReport, key: Key, modifiers: u8) -> KeyboardReport {
    let mut report = base;
    report.modifiers = modifiers;
    // A modifier key's own report names no usage: the byte just set is the
    // whole of what it says.
    if modifier_bit(key).is_none() {
        let (on_page, code) = usage::of(key).expect("a key with a usage");
        assert_eq!(on_page, page::KEYBOARD_OR_KEYPAD, "not a plain key");
        report.press(code);
    }
    report
}

/// The report after this sequence of presses (`true`) and releases (`false`),
/// named by the key each output report would carry rather than by an input
/// key that got there some other way — a rule's own remap is what turns one
/// into the other, and this is asserting what came out the far side of it.
fn after(steps: &[(Key, bool)]) -> KeyboardReport {
    let mut report = KeyboardReport::default();
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

/// The report immediately after `key` goes down while `holds` are down, or
/// `None` if the pipeline swallows it — and the report `holds` alone settle
/// into, for building the report a `want` would have to match against.
///
/// Only the down side, and nothing past it: a tap also releases `key`, and
/// releasing `holds` afterward would add their own reports right where a
/// swallowed press would otherwise read as empty. What matters here is
/// whether *this* press produced a report, not what came after it.
///
/// The holds are physical keys, not modifier flags — on the built-in keyboard
/// `option` is reached by holding the physical left *command* key, and saying so
/// is the point. Their own report is read from a separate, shorter run rather
/// than assumed from `want`: the sink is deterministic, so the prefix is
/// identical either way, and this is what lets a mismatch be read off actual
/// bytes rather than a guess about them.
fn press(device: DeviceId, holds: &[Key], key: Key) -> (Option<KeyboardReport>, KeyboardReport) {
    let prefix = run(|host| {
        for &hold in holds {
            host.press(device, hold);
        }
    });
    let base = prefix.last().copied().unwrap_or_default();

    let full = run(|host| {
        for &hold in holds {
            host.press(device, hold);
        }
        host.press(device, key);
    });

    (full.get(prefix.len()).copied(), base)
}

/// Whether `key` while `holds` are down resolves to `want` (`None` for
/// swallowed) — for a test with a single case of its own and no need for
/// [`Report`]'s collector.
fn resolves_to(device: DeviceId, holds: &[Key], key: Key, want: Option<(Key, u8)>) -> bool {
    let (next, base) = press(device, holds, key);
    match want {
        None => next.is_none(),
        Some((out_key, out_mods)) => next == Some(expected_after(base, out_key, out_mods)),
    }
}

/// Collects every mismatch so one run reports the whole picture.
#[derive(Default)]
struct Report(Vec<String>);

impl Report {
    fn expect(&mut self, device: DeviceId, holds: &[Key], key: Key, want: Option<(Key, u8)>) {
        if !resolves_to(device, holds, key, want) {
            let (next, _) = press(device, holds, key);
            self.0.push(format!(
                "device {:?}, holding {:?}, pressing {:?}: expected {:?}, report was {:?}",
                device.0, holds, key, want, next
            ));
        }
    }

    fn expect_all(&mut self, device: DeviceId, holds: &[Key], cases: &[(Key, (Key, u8))]) {
        for &(key, want) in cases {
            self.expect(device, holds, key, Some(want));
        }
    }

    fn finish(self) {
        assert!(
            self.0.is_empty(),
            "{} mismatch(es):\n{}",
            self.0.len(),
            self.0.join("\n")
        );
    }
}

const fn plain(key: Key) -> (Key, u8) {
    (key, 0)
}

/// A modifier key's own output.
///
/// The byte names the key, and has to: a modifier key is delivered *as* that
/// byte, so one that left itself out would be a modifier the OS never saw go
/// down.
const fn itself(key: Key) -> (Key, u8) {
    (key, bit(key))
}

const fn shifted(key: Key) -> (Key, u8) {
    (key, SHIFT)
}

// ---------------------------------------------------------------------------
// Dudrack Neutral layer
// ---------------------------------------------------------------------------

#[rustfmt::skip]
const NEUTRAL: &[(Key, (Key, u8))] = &[
    (Key::Q,           shifted(Key::Semicolon)),  // ':'
    (Key::W,           plain(Key::Comma)),
    (Key::E,           plain(Key::Period)),
    (Key::R,           plain(Key::P)),
    (Key::T,           plain(Key::Y)),
    (Key::Y,           plain(Key::F)),
    (Key::U,           plain(Key::G)),
    (Key::I,           plain(Key::C)),
    (Key::O,           plain(Key::R)),
    (Key::P,           plain(Key::L)),
    (Key::OpenBracket, plain(Key::Slash)),
    (Key::A,           plain(Key::A)),
    (Key::S,           plain(Key::O)),
    (Key::D,           plain(Key::E)),
    (Key::F,           plain(Key::U)),
    (Key::G,           plain(Key::I)),
    (Key::H,           plain(Key::D)),
    (Key::J,           plain(Key::H)),
    (Key::K,           plain(Key::T)),
    (Key::L,           plain(Key::N)),
    (Key::Semicolon,   plain(Key::S)),
    (Key::Quote,       plain(Key::Hyphen)),
    (Key::Z,           plain(Key::Semicolon)),
    (Key::X,           plain(Key::Q)),
    (Key::C,           plain(Key::J)),
    (Key::V,           plain(Key::K)),
    (Key::B,           plain(Key::X)),
    (Key::N,           plain(Key::B)),
    (Key::M,           plain(Key::M)),
    (Key::Comma,       plain(Key::W)),
    (Key::Period,      plain(Key::V)),
    (Key::Slash,       plain(Key::Z)),
];

/// Shift is consumed by these, so the symbol replaces the shifted letter.
#[rustfmt::skip]
const NEUTRAL_SHIFTED: &[(Key, (Key, u8))] = &[
    (Key::Q,     shifted(Key::Digit8)),     // '*'
    (Key::Quote, plain(Key::EqualSign)),    // '='
    (Key::Z,     shifted(Key::EqualSign)),  // '+'
];

#[test]
fn built_in_types_dudrack_neutral() {
    let mut report = Report::default();
    report.expect_all(BUILT_IN, &[], NEUTRAL);
    report.expect_all(BUILT_IN, &[Key::LeftShift], NEUTRAL_SHIFTED);
    report.finish();
}

#[test]
fn shift_passes_through_the_neutral_layer() {
    let mut report = Report::default();
    // A letter with no shifted entry of its own keeps its shift.
    report.expect(BUILT_IN, &[Key::LeftShift], Key::S, Some((Key::O, SHIFT)));
    // And the side it was held on: the right shift stays the right one, since the
    // set names keys rather than the modifier they stand for.
    report.expect(
        BUILT_IN,
        &[Key::RightShift],
        Key::J,
        Some((Key::H, bit(Key::RightShift))),
    );
    report.finish();
}

// ---------------------------------------------------------------------------
// Dudrack Henkan layer
// ---------------------------------------------------------------------------

#[rustfmt::skip]
const HENKAN: &[(Key, (Key, u8))] = &[
    (Key::Q,         plain(Key::Digit1)),
    (Key::W,         plain(Key::Digit2)),
    (Key::E,         plain(Key::Digit3)),
    (Key::R,         plain(Key::Digit4)),
    (Key::T,         plain(Key::Digit5)),
    (Key::Y,         plain(Key::Digit6)),
    (Key::U,         plain(Key::Digit7)),
    (Key::I,         plain(Key::Digit8)),
    (Key::O,         plain(Key::Digit9)),
    (Key::P,         plain(Key::Digit0)),
    (Key::A,         plain(Key::Tab)),
    (Key::S,         plain(Key::Escape)),
    (Key::D,         plain(Key::ReturnOrEnter)),
    (Key::F,         plain(Key::DeleteOrBackspace)),
    (Key::G,         plain(Key::DeleteForward)),
    (Key::H,         shifted(Key::Digit2)),      // '@'
    (Key::J,         plain(Key::Backslash)),     // '\'
    (Key::K,         plain(Key::OpenBracket)),   // '['
    (Key::L,         plain(Key::CloseBracket)),  // ']'
    (Key::Semicolon, plain(Key::Backslash)),     // '\'
    (Key::Z,         plain(Key::LeftArrow)),
    (Key::X,         plain(Key::DownArrow)),
    (Key::C,         plain(Key::UpArrow)),
    (Key::V,         plain(Key::RightArrow)),
    (Key::B,         plain(Key::X)),
    (Key::N,         plain(Key::JapaneseKana)),
    (Key::M,         plain(Key::JapaneseEisuu)),
    (Key::Comma,     (Key::LeftArrow,  COMMAND)),
    (Key::Period,    (Key::RightArrow, COMMAND)),
    (Key::Slash,     shifted(Key::Digit6)),      // '^'
];

#[rustfmt::skip]
const HENKAN_SHIFTED: &[(Key, (Key, u8))] = &[
    (Key::W,         shifted(Key::Quote)),      // '"'
    (Key::Y,         shifted(Key::Digit7)),     // '&'
    (Key::U,         plain(Key::Quote)),        // '\''
    (Key::I,         shifted(Key::Digit9)),     // '('
    (Key::O,         shifted(Key::Digit0)),     // ')'
    (Key::P,         plain(Key::Digit0)),       // shift is consumed, not '0)' -> ')'
    (Key::H,         plain(Key::Grave)),        // '`'
    (Key::J,         shifted(Key::Hyphen)),     // '_'
    (Key::Semicolon, shifted(Key::Backslash)),  // '|'
    (Key::Slash,     shifted(Key::Grave)),      // '~'
];

#[test]
fn right_command_holds_the_henkan_layer_on_the_built_in_keyboard() {
    let mut report = Report::default();
    report.expect_all(BUILT_IN, &[Key::RightCommand], HENKAN);
    report.expect_all(
        BUILT_IN,
        &[Key::RightCommand, Key::LeftShift],
        HENKAN_SHIFTED,
    );
    report.finish();
}

#[test]
fn a_key_the_henkan_layer_leaves_alone_keeps_its_neutral_meaning() {
    let mut report = Report::default();
    // No Henkan entry for the bracket key, so Neutral's '/' stands.
    report.expect(
        BUILT_IN,
        &[Key::RightCommand],
        Key::OpenBracket,
        Some(plain(Key::Slash)),
    );
    report.expect(
        BUILT_IN,
        &[Key::RightCommand],
        Key::Quote,
        Some(plain(Key::Hyphen)),
    );
    report.finish();
}

#[test]
fn the_henkan_key_itself_types_nothing() {
    assert!(resolves_to(BUILT_IN, &[], Key::RightCommand, None));
    assert!(resolves_to(TRACKPOINT, &[], Key::JapanesePcXfer, None));
}

// ---------------------------------------------------------------------------
// Modifier remaps
// ---------------------------------------------------------------------------

#[test]
fn built_in_modifier_positions_are_swapped() {
    let mut report = Report::default();
    report.expect_all(
        BUILT_IN,
        &[],
        &[
            (Key::CapsLock, itself(Key::LeftControl)),
            (Key::Tab, itself(Key::LeftCommand)),
            (Key::LeftCommand, itself(Key::LeftOption)),
            (Key::LeftOption, itself(Key::LeftCommand)),
            (Key::Fn, itself(Key::RightCommand)),
        ],
    );
    report.finish();
}

#[test]
fn a_remapped_modifier_acts_as_what_it_became() {
    let mut report = Report::default();
    // Caps Lock is control, so it reaches the layout as control rather than as
    // the caps-lock flag.
    report.expect(BUILT_IN, &[Key::CapsLock], Key::S, Some((Key::O, CONTROL)));
    // The physical left command key is option.
    report.expect(
        BUILT_IN,
        &[Key::LeftCommand],
        Key::J,
        Some((Key::H, OPTION)),
    );
    // The physical left option key is command.
    report.expect(
        BUILT_IN,
        &[Key::LeftOption],
        Key::S,
        Some((Key::O, COMMAND)),
    );
    report.finish();
}

#[test]
fn fn_becomes_a_command_key_and_not_the_henkan_layer() {
    // Fn produces right command as output, and the Henkan rule matches the
    // physical right command key — so holding Fn must leave the Neutral layer
    // in place rather than opening Henkan (where 'q' would be '1').
    assert!(resolves_to(
        BUILT_IN,
        &[Key::Fn],
        Key::Q,
        // The command is the *right* one, which is the key Fn became and the key
        // actually down: what the OS is told is the keys and not the modifier.
        Some((Key::Semicolon, SHIFT | bit(Key::RightCommand))),
    ));
}

// ---------------------------------------------------------------------------
// The Dudrack-typed external keyboard
// ---------------------------------------------------------------------------

#[test]
fn the_trackpoint_keyboard_rides_the_dudrack_layers() {
    let mut report = Report::default();
    report.expect_all(TRACKPOINT, &[], NEUTRAL);
    report.expect_all(TRACKPOINT, &[Key::LeftShift], NEUTRAL_SHIFTED);
    report.expect_all(TRACKPOINT, &[Key::JapanesePcXfer], HENKAN);
    report.expect_all(
        TRACKPOINT,
        &[Key::JapanesePcXfer, Key::LeftShift],
        HENKAN_SHIFTED,
    );
    report.finish();
}

#[test]
fn xfer_and_nfer_together_consume_the_shift_in_the_report_too() {
    // The physical chord this is about: 変換 holds Henkan, 無変換 is a key that
    // *resolves* to shift rather than shift itself, and Henkan's 'u' with shift
    // held is the plain quote `'` — ansi::SINGLE_QUOTE, which consumes it. Every
    // test above holds `Key::LeftShift` directly; this holds the key a person's
    // other thumb is actually on, and it checks the report the device was
    // written, not only the `Injected` value the sink computed from it. The two
    // can disagree — a rule's consumed modifier reaching the OS as a bit anyway
    // is exactly that, and it is invisible to every assertion above.
    const QUOTE_USAGE: u16 = 0x34;
    let reports = device_reports(|host| {
        host.press(TRACKPOINT, Key::JapanesePcXfer)
            .press(TRACKPOINT, Key::JapanesePcNfer)
            .tap(TRACKPOINT, Key::U);
    });
    let holding_quote = reports.into_iter().find_map(|sent| match sent {
        Sent::Keyboard(report) if report.keys.holds(QUOTE_USAGE) => Some(report),
        _ => None,
    });
    let report = holding_quote.expect("a report holding Quote's usage");
    assert_eq!(
        report.modifiers, 0,
        "無変換's shift must be gone from the report that carries the key it \
         selected, not only from `Injected`: {report:?}"
    );
}

#[test]
fn both_shift_keys_held_together_are_both_in_the_report() {
    // Rolling both shift keys is ordinary typing, and letting go of one must not
    // read as "no shift is held any more" — the report is a set, and the set
    // still has the other key in it.
    let one = last_modifiers(|host| {
        host.press(BUILT_IN, Key::LeftShift);
    });
    let both = last_modifiers(|host| {
        host.press(BUILT_IN, Key::LeftShift)
            .press(BUILT_IN, Key::RightShift);
    });

    assert_ne!(one, 0, "the first shift is in the byte");
    assert_eq!(both & one, one, "and it stays there once the second joins");
    assert_ne!(
        both, one,
        "which is a different byte, not the same one twice"
    );
}

#[test]
fn letting_one_modifier_go_leaves_the_other_down() {
    // The set an event carries is the one recorded when its key went down, so a
    // release must not be read as "these are the only modifiers now" — shift
    // going up must not take command down with it.
    let shift = last_modifiers(|host| {
        host.press(BUILT_IN, Key::LeftShift);
    });
    let both = last_modifiers(|host| {
        host.press(BUILT_IN, Key::LeftShift)
            .press(BUILT_IN, Key::LeftCommand);
    });
    let after = last_modifiers(|host| {
        host.press(BUILT_IN, Key::LeftShift)
            .press(BUILT_IN, Key::LeftCommand)
            .release(BUILT_IN, Key::LeftShift);
    });

    assert_eq!(
        after,
        both & !shift,
        "shift's bit is gone and nothing else is"
    );
    assert_ne!(after, 0, "command is still down");
}

#[test]
fn the_trackpoint_keyboard_does_not_take_the_raw_jis_remaps() {
    let mut report = Report::default();
    // Its quote key is Dudrack's '-', not JIS's ':'.
    report.expect(TRACKPOINT, &[], Key::Quote, Some(plain(Key::Hyphen)));
    // Its 変換 key holds the layer; on a raw-JIS keyboard the same key is かな.
    report.expect(TRACKPOINT, &[], Key::JapanesePcXfer, None);
    report.expect(
        RAW_JIS,
        &[],
        Key::JapanesePcXfer,
        Some(plain(Key::JapaneseKana)),
    );
    report.finish();
}

#[test]
fn the_trackpoint_keyboards_jis_thumb_keys_carry_modifiers() {
    let mut report = Report::default();
    report.expect_all(
        TRACKPOINT,
        &[],
        &[
            (Key::CapsLock, itself(Key::LeftControl)),
            (Key::Tab, itself(Key::LeftCommand)),
            (Key::JapanesePcNfer, itself(Key::LeftShift)),
            (Key::JapanesePcKatakana, itself(Key::LeftCommand)),
        ],
    );
    // 右Alt is where a thumb lands by accident.
    report.expect(TRACKPOINT, &[], Key::RightOption, None);
    // 無変換 held is shift held.
    report.expect(
        TRACKPOINT,
        &[Key::JapanesePcNfer],
        Key::Q,
        Some(shifted(Key::Digit8)),
    );
    // The MacBook-physical swaps do not apply here.
    report.expect(
        TRACKPOINT,
        &[],
        Key::LeftCommand,
        Some(itself(Key::LeftCommand)),
    );
    report.expect(
        TRACKPOINT,
        &[],
        Key::LeftOption,
        Some(itself(Key::LeftOption)),
    );
    report.finish();
}

// ---------------------------------------------------------------------------
// Raw-JIS external keyboards
// ---------------------------------------------------------------------------

#[rustfmt::skip]
const JIS_LABELS: &[(Key, (Key, u8))] = &[
    (Key::EqualSign,      shifted(Key::Digit6)),      // '^'
    (Key::OpenBracket,    shifted(Key::Digit2)),      // '@'
    (Key::CloseBracket,   plain(Key::OpenBracket)),   // '['
    (Key::NonUsPound,     plain(Key::CloseBracket)),  // ']'
    (Key::Backslash,      plain(Key::CloseBracket)),  // ']' on the other wiring
    (Key::Quote,          shifted(Key::Semicolon)),   // ':'
    (Key::International1, plain(Key::Backslash)),     // '\'
    (Key::International3, plain(Key::Backslash)),     // '\'
];

#[rustfmt::skip]
const JIS_LABELS_SHIFTED: &[(Key, (Key, u8))] = &[
    (Key::Digit2,         shifted(Key::Quote)),        // '"'
    (Key::Digit6,         shifted(Key::Digit7)),       // '&'
    (Key::Digit7,         plain(Key::Quote)),          // '\''
    (Key::Digit8,         shifted(Key::Digit9)),       // '('
    (Key::Digit9,         shifted(Key::Digit0)),       // ')'
    (Key::Hyphen,         plain(Key::EqualSign)),      // '='
    (Key::EqualSign,      shifted(Key::Grave)),        // '~'
    (Key::OpenBracket,    plain(Key::Grave)),          // '`'
    (Key::CloseBracket,   shifted(Key::OpenBracket)),  // '{'
    (Key::NonUsPound,     shifted(Key::CloseBracket)), // '}'
    (Key::Backslash,      shifted(Key::CloseBracket)), // '}'
    (Key::Semicolon,      shifted(Key::EqualSign)),    // '+'
    (Key::Quote,          shifted(Key::Digit8)),       // '*'
    (Key::International1, shifted(Key::Hyphen)),       // '_'
    (Key::International3, shifted(Key::Backslash)),    // '|'
];

#[test]
fn a_raw_jis_keyboard_types_its_labels() {
    let mut report = Report::default();
    report.expect_all(RAW_JIS, &[], JIS_LABELS);
    report.expect_all(RAW_JIS, &[Key::LeftShift], JIS_LABELS_SHIFTED);
    report.finish();
}

#[test]
fn a_raw_jis_keyboard_keeps_qwerty() {
    let mut report = Report::default();
    // The Dvorak layers are scoped to the Dudrack keyboards, so letters here
    // are themselves.
    report.expect_all(
        RAW_JIS,
        &[],
        &[
            (Key::S, plain(Key::S)),
            (Key::J, plain(Key::J)),
            (Key::Q, plain(Key::Q)),
            (Key::Semicolon, plain(Key::Semicolon)),
        ],
    );
    report.finish();
}

#[test]
fn caps_lock_on_a_raw_jis_keyboard_stays_the_lock_it_is_printed_as() {
    // The remap to control is scoped to the Dudrack keyboards, so here the key is
    // itself — and being held changes nothing about what the keys beside it type.
    // A rule that had taken caps lock for a modifier of its own would turn a
    // person's lock into a layer, and the key that says `S` would stop saying it.
    let mut report = Report::default();
    report.expect(RAW_JIS, &[], Key::CapsLock, Some(itself(Key::CapsLock)));
    report.expect(RAW_JIS, &[Key::CapsLock], Key::S, Some(plain(Key::S)));
    report.finish();
}

#[test]
fn right_control_is_itself_on_the_macs_own_keyboards() {
    // The keyboard forwarded from Windows has right control remapped, because a PC
    // keyboard has one where a Mac has nothing much (`link_relay.rs`). A keyboard
    // plugged in here is a different keyboard — it is under the person's other hand
    // rather than at the other machine — and nothing about it needs moving.
    let mut report = Report::default();
    report.expect_all(
        RAW_JIS,
        &[],
        &[(Key::RightControl, itself(Key::RightControl))],
    );
    report.expect_all(
        TRACKPOINT,
        &[],
        &[(Key::RightControl, itself(Key::RightControl))],
    );
    report.finish();
}

#[test]
fn pc_jis_ime_keys_become_apple_jis_ones() {
    let mut report = Report::default();
    report.expect_all(
        RAW_JIS,
        &[],
        &[
            (Key::JapanesePcNfer, plain(Key::JapaneseEisuu)),
            (Key::JapanesePcXfer, plain(Key::JapaneseKana)),
            // Some of these keyboards deliver 変換 at the grave position.
            (Key::Grave, plain(Key::JapaneseKana)),
        ],
    );
    report.finish();
}

#[test]
fn the_jis_reading_survives_an_option_chord() {
    // Option is tolerated rather than consumed, so a window-manager chord sees
    // the JIS symbol rather than the US one at that position.
    let mut report = Report::default();
    report.expect(
        RAW_JIS,
        &[Key::LeftOption],
        Key::Quote,
        Some((Key::Semicolon, SHIFT | OPTION)),
    );
    report.expect(
        RAW_JIS,
        &[Key::LeftOption, Key::LeftShift],
        Key::Quote,
        Some((Key::Digit8, SHIFT | OPTION)),
    );
    report.finish();
}

// ---------------------------------------------------------------------------
// Cmd+H guards
// ---------------------------------------------------------------------------

#[test]
fn hide_and_hide_others_are_unreachable() {
    let mut report = Report::default();
    // On Dudrack the key that yields 'h' is physical 'j'. Command is reached by
    // holding the physical left option key.
    report.expect(BUILT_IN, &[Key::LeftOption], Key::J, None);
    report.expect(BUILT_IN, &[Key::LeftOption, Key::LeftCommand], Key::J, None);
    report.expect(TRACKPOINT, &[Key::Tab], Key::J, None);
    // On a raw-JIS keyboard it is physical 'h'.
    report.expect(RAW_JIS, &[Key::LeftCommand], Key::H, None);
    report.expect(RAW_JIS, &[Key::LeftCommand, Key::LeftOption], Key::H, None);
    report.finish();
}

#[test]
fn the_hide_guard_does_not_eat_the_backslash_on_the_henkan_layer() {
    let mut report = Report::default();
    // Under Henkan physical 'j' is '\', not 'h', so cmd+'\' must survive.
    report.expect(
        BUILT_IN,
        &[Key::LeftOption, Key::RightCommand],
        Key::J,
        Some((Key::Backslash, COMMAND)),
    );
    // And without command, physical 'j' is just Neutral 'h'.
    report.expect(
        BUILT_IN,
        &[Key::LeftCommand],
        Key::J,
        Some((Key::H, OPTION)),
    );
    report.finish();
}

#[test]
fn holding_both_command_keys_while_reaching_for_at_is_a_no_op() {
    let mut report = Report::default();
    // Left command leaks option through its remap, and Henkan's '@' is shift+2,
    // so the chord would surface as option+shift+2 — a window-manager binding.
    report.expect(
        BUILT_IN,
        &[Key::LeftCommand, Key::RightCommand],
        Key::H,
        None,
    );
    report.expect(
        BUILT_IN,
        &[Key::LeftCommand, Key::RightCommand, Key::LeftShift],
        Key::H,
        None,
    );
    // '@' without the contamination still types.
    report.expect(
        BUILT_IN,
        &[Key::RightCommand],
        Key::H,
        Some(shifted(Key::Digit2)),
    );
    // Outside Henkan, option+physical-'h' is Neutral 'd'.
    report.expect(
        BUILT_IN,
        &[Key::LeftCommand],
        Key::H,
        Some((Key::D, OPTION)),
    );
    // The guard is built-in only: the TrackPoint keyboard has no such misfire.
    report.expect(
        TRACKPOINT,
        &[Key::LeftOption, Key::JapanesePcXfer],
        Key::H,
        Some((Key::Digit2, SHIFT | OPTION)),
    );
    report.finish();
}

// ---------------------------------------------------------------------------
// Home / End
// ---------------------------------------------------------------------------

#[test]
fn home_and_end_are_line_navigation_everywhere() {
    let mut report = Report::default();
    for device in [BUILT_IN, TRACKPOINT, RAW_JIS] {
        report.expect(device, &[], Key::Home, Some((Key::LeftArrow, COMMAND)));
        report.expect(device, &[], Key::End, Some((Key::RightArrow, COMMAND)));
    }
    // Shift still selects.
    report.expect(
        RAW_JIS,
        &[Key::LeftShift],
        Key::Home,
        Some((Key::LeftArrow, COMMAND | SHIFT)),
    );
    report.finish();
}

// ---------------------------------------------------------------------------
// Keys the pipeline has nothing to say about
// ---------------------------------------------------------------------------

#[test]
fn an_unmatched_key_passes_through_with_its_modifiers() {
    let mut report = Report::default();
    // No Dudrack entry for the backslash position on the built-in keyboard, and
    // the raw-JIS table is out of scope there.
    report.expect(BUILT_IN, &[], Key::Backslash, Some(plain(Key::Backslash)));
    report.expect(BUILT_IN, &[], Key::Escape, Some(plain(Key::Escape)));
    // Physical 'h' on a raw-JIS keyboard, with no command to trigger the guard.
    report.expect(RAW_JIS, &[], Key::H, Some(plain(Key::H)));
    report.expect(RAW_JIS, &[Key::LeftOption], Key::X, Some((Key::X, OPTION)));
    report.finish();
}

#[test]
fn a_usage_nothing_names_reaches_no_application() {
    // A different thing from the key above, which has a name and no rule: this
    // one has no name at all. `F13` — external keyboards do have it and this
    // layout does not name it, so it arrives as an element and stops there.
    //
    // Nothing rather than something harmless: the usage numbers a table does not
    // cover are where a key reports somewhere unexpected, and a report invented
    // for one would type whatever that number happens to mean on the page it
    // went out on. `watching.rs` holds the other half — that it is reported to
    // the person rather than dropped in silence.
    const F13: u32 = 0x68;

    let reports = run(|host| {
        host.script(EventKind::HidValue {
            device: BUILT_IN,
            page: page::KEYBOARD_OR_KEYPAD,
            usage: F13,
            stamp: 1,
            value: 1,
        });
    });

    assert_eq!(reports, Vec::new());
}

// ---------------------------------------------------------------------------
// Down and up have to agree
// ---------------------------------------------------------------------------

#[test]
fn a_key_releases_what_it_pressed_even_if_the_layer_moved() {
    // Take up Henkan, press physical 'h' (= '@'), drop Henkan, then release.
    // The release has to be '@' — recomputing it would release Neutral 'd' and
    // strand shift+2 inside the application.
    let reports = run(|host| {
        host.press(BUILT_IN, Key::RightCommand);
        host.press(BUILT_IN, Key::H);
        host.release(BUILT_IN, Key::RightCommand);
        host.release(BUILT_IN, Key::H);
    });

    assert_eq!(
        reports,
        vec![
            after(&[(Key::LeftShift, true), (Key::Digit2, true)]),
            after(&[
                (Key::LeftShift, true),
                (Key::Digit2, true),
                (Key::Digit2, false),
            ]),
            // And the shift `@` borrowed is let go once its key is up. Nothing
            // holds it — the rule added it for that one keystroke — so a run that
            // stopped at the release would leave every window shift-clicked.
            KeyboardReport::default(),
        ]
    );
}

#[test]
fn auto_repeat_repeats_what_the_first_press_resolved_to() {
    let reports = run(|host| {
        host.press(BUILT_IN, Key::S);
        host.press(BUILT_IN, Key::S);
        host.press(BUILT_IN, Key::S);
        host.release(BUILT_IN, Key::S);
    });

    let held = after(&[(Key::O, true)]);
    assert_eq!(reports, vec![held, held, held, KeyboardReport::default()]);
}

#[test]
fn unplugging_a_keyboard_releases_what_it_was_holding() {
    // The matching key-up will never arrive, so the sink has to produce it —
    // otherwise control stays stuck down for every application.
    let reports = run(|host| {
        host.press(BUILT_IN, Key::CapsLock);
        host.detach(BUILT_IN);
    });

    assert_eq!(
        reports,
        vec![
            after(&[(Key::LeftControl, true)]),
            KeyboardReport::default()
        ]
    );
}

#[test]
fn unplugging_a_keyboard_drops_the_layer_it_was_holding() {
    // Henkan held on the TrackPoint keyboard must not survive its disconnection
    // and leave the built-in keyboard on the wrong layer.
    let reports = run(|host| {
        host.press(TRACKPOINT, Key::JapanesePcXfer);
        host.detach(TRACKPOINT);
        host.tap(BUILT_IN, Key::Q);
    });

    assert_eq!(
        reports,
        vec![
            after(&[(Key::LeftShift, true), (Key::Semicolon, true)]),
            after(&[
                (Key::LeftShift, true),
                (Key::Semicolon, true),
                (Key::Semicolon, false),
            ]),
            // The shift `:` borrowed, let go with it.
            KeyboardReport::default(),
        ]
    );
}

#[test]
fn a_release_with_no_press_behind_it_is_passed_on() {
    // A key already down when the sink starts: releasing it unconverted is the
    // safe direction, since swallowing it would leave it stuck.
    let reports = run(|host| {
        host.release(BUILT_IN, Key::LeftShift);
    });

    assert_eq!(reports, vec![KeyboardReport::default()]);
}

// ---------------------------------------------------------------------------
// The three keyboards, at once
// ---------------------------------------------------------------------------

#[test]
fn the_same_key_means_three_things_at_once() {
    // One pipeline, three keyboards, no shared state between them beyond the
    // layer — this is the whole point of ADR-0003.
    let reports = run(|host| {
        host.tap(BUILT_IN, Key::Quote);
        host.tap(TRACKPOINT, Key::Quote);
        host.tap(RAW_JIS, Key::Quote);
    });

    assert_eq!(
        reports,
        vec![
            // Dudrack: '-'
            after(&[(Key::Hyphen, true)]),
            KeyboardReport::default(),
            // Dudrack again, because this keyboard is typed in Dudrack too.
            after(&[(Key::Hyphen, true)]),
            KeyboardReport::default(),
            // JIS label: ':'
            after(&[(Key::LeftShift, true), (Key::Semicolon, true)]),
            after(&[
                (Key::LeftShift, true),
                (Key::Semicolon, true),
                (Key::Semicolon, false),
            ]),
            // The shift `:` borrowed, let go with it.
            KeyboardReport::default(),
        ]
    );
}

#[test]
fn a_modifier_on_one_keyboard_applies_to_a_key_on_another() {
    // Shift held on the external keyboard while a letter is typed on the
    // built-in one: macOS reports modifiers globally, and the pipeline follows.
    let reports = run(|host| {
        host.press(RAW_JIS, Key::LeftShift);
        host.tap(BUILT_IN, Key::Q);
        host.release(RAW_JIS, Key::LeftShift);
    });

    assert_eq!(
        reports,
        vec![
            after(&[(Key::LeftShift, true)]),
            // Neutral shift+'q' is '*', not shift+':'.
            after(&[(Key::LeftShift, true), (Key::Digit8, true)]),
            after(&[
                (Key::LeftShift, true),
                (Key::Digit8, true),
                (Key::Digit8, false),
            ]),
            KeyboardReport::default(),
        ]
    );
}

// ---------------------------------------------------------------------------
// Space and Shift on the built-in keyboard
// ---------------------------------------------------------------------------
//
// Hold the space bar and it is shift; tap it and it is a space. The shift is
// lazy: it reaches the OS only once a key actually needs it, so a tap never
// leaves a stray shift behind.

/// Long enough to be a hold, short enough to still be within the tap window.
const BRIEF: Duration = Duration::from_millis(50);
/// Past the tap window.
const AGES: Duration = Duration::from_secs(2);

#[test]
fn tapping_space_types_a_space() {
    let reports = run(|host| {
        host.hold(BUILT_IN, Key::Spacebar, BRIEF);
    });

    assert_eq!(
        reports,
        vec![after(&[(Key::Spacebar, true)]), KeyboardReport::default()]
    );
}

#[test]
fn holding_space_shifts_the_next_key() {
    let reports = run(|host| {
        host.press(BUILT_IN, Key::Spacebar);
        host.advance(BRIEF);
        host.tap(BUILT_IN, Key::Q);
        host.release(BUILT_IN, Key::Spacebar);
    });

    // Neutral shift+'q' is '*', and no space is typed.
    assert_eq!(
        reports,
        vec![
            after(&[(Key::LeftShift, true)]),
            after(&[(Key::LeftShift, true), (Key::Digit8, true)]),
            after(&[
                (Key::LeftShift, true),
                (Key::Digit8, true),
                (Key::Digit8, false),
            ]),
            KeyboardReport::default(),
        ]
    );
}

#[test]
fn the_shift_arrives_only_when_a_key_needs_it() {
    // Not when the space bar went down. The timestamps are the assertion: a
    // shift sent early would be stamped at 0 and would sit held over whatever
    // the user did in between.
    let mut host = SimHost::new();
    attach_keyboards(&mut host);
    host.press(BUILT_IN, Key::Spacebar);
    host.advance(BRIEF);
    host.tap(BUILT_IN, Key::Q);
    host.release(BUILT_IN, Key::Spacebar);
    sink::run(
        &Request::Injecting { listen: false },
        Layout::dudrack(),
        None,
        InputConfig::default(),
        &mut host,
        None,
    );

    let at = Instant {
        nanos: u64::try_from(BRIEF.as_nanos()).expect("a millisecond count fits in a u64"),
    };
    assert!(
        host.reports().iter().all(|(when, _, _)| *when == at),
        "{:?}",
        host.reports()
    );
}

#[test]
fn the_shift_is_sent_once_however_many_keys_follow() {
    let reports = run(|host| {
        host.press(BUILT_IN, Key::Spacebar);
        host.advance(BRIEF);
        host.tap(BUILT_IN, Key::Q);
        host.tap(BUILT_IN, Key::A);
        host.release(BUILT_IN, Key::Spacebar);
    });

    assert_eq!(
        reports,
        vec![
            after(&[(Key::LeftShift, true)]),
            after(&[(Key::LeftShift, true), (Key::Digit8, true)]),
            after(&[
                (Key::LeftShift, true),
                (Key::Digit8, true),
                (Key::Digit8, false),
            ]),
            after(&[
                (Key::LeftShift, true),
                (Key::Digit8, true),
                (Key::Digit8, false),
                (Key::A, true),
            ]),
            after(&[
                (Key::LeftShift, true),
                (Key::Digit8, true),
                (Key::Digit8, false),
                (Key::A, true),
                (Key::A, false),
            ]),
            KeyboardReport::default(),
        ]
    );
}

#[test]
fn holding_space_past_the_tap_window_types_nothing() {
    // Held, then abandoned. A stray space here would land in the middle of
    // whatever the user was doing.
    let reports = run(|host| {
        host.hold(BUILT_IN, Key::Spacebar, AGES);
    });

    assert_eq!(reports, Vec::new());
}

#[test]
fn a_modifier_pressed_alongside_space_costs_the_space_but_not_the_shift() {
    // A modifier needs no shift to be delivered, so the lazy shift stays unsent
    // — not as a key and not as a flag either, since nothing has been told
    // about it. The space bar was no longer alone, so it types nothing.
    let reports = run(|host| {
        host.press(BUILT_IN, Key::Spacebar);
        host.advance(BRIEF);
        host.tap(BUILT_IN, Key::LeftOption);
        host.release(BUILT_IN, Key::Spacebar);
    });

    assert_eq!(
        reports,
        vec![
            after(&[(Key::LeftCommand, true)]),
            KeyboardReport::default()
        ]
    );
}

#[test]
fn space_shifts_the_henkan_layer_too() {
    let reports = run(|host| {
        host.press(BUILT_IN, Key::RightCommand);
        host.press(BUILT_IN, Key::Spacebar);
        host.advance(BRIEF);
        host.tap(BUILT_IN, Key::W);
        host.release(BUILT_IN, Key::Spacebar);
        host.release(BUILT_IN, Key::RightCommand);
    });

    // Henkan shift+'w' is '"'.
    assert_eq!(
        reports,
        vec![
            after(&[(Key::LeftShift, true)]),
            after(&[(Key::LeftShift, true), (Key::Quote, true)]),
            after(&[
                (Key::LeftShift, true),
                (Key::Quote, true),
                (Key::Quote, false),
            ]),
            KeyboardReport::default(),
        ]
    );
}

#[test]
fn space_on_an_external_keyboard_is_only_ever_a_space() {
    // The swap is MacBook-physical; every other keyboard keeps its space bar.
    for device in [TRACKPOINT, RAW_JIS] {
        let reports = run(|host| {
            host.press(device, Key::Spacebar);
            host.advance(AGES);
            host.tap(device, Key::A);
            host.release(device, Key::Spacebar);
        });

        assert_eq!(
            reports,
            vec![
                after(&[(Key::Spacebar, true)]),
                after(&[(Key::Spacebar, true), (Key::A, true)]),
                after(&[(Key::Spacebar, true), (Key::A, true), (Key::A, false),]),
                KeyboardReport::default(),
            ],
            "device {:?}",
            device.0
        );
    }
}

#[test]
fn unplugging_the_keyboard_mid_hold_releases_the_shift_it_sent() {
    let reports = run(|host| {
        host.press(BUILT_IN, Key::Spacebar);
        host.advance(BRIEF);
        host.press(BUILT_IN, Key::Q);
        host.detach(BUILT_IN);
    });

    assert_eq!(
        reports,
        vec![
            after(&[(Key::LeftShift, true)]),
            after(&[(Key::LeftShift, true), (Key::Digit8, true)]),
            after(&[
                (Key::LeftShift, true),
                (Key::Digit8, true),
                (Key::Digit8, false),
            ]),
            KeyboardReport::default(),
        ]
    );
}

#[test]
fn unplugging_the_keyboard_mid_tap_types_no_space() {
    // The tap was never completed, so there is nothing to type — and nothing to
    // release either, because the lazy shift was never sent.
    let reports = run(|host| {
        host.press(BUILT_IN, Key::Spacebar);
        host.detach(BUILT_IN);
    });

    assert_eq!(reports, Vec::new());
}
