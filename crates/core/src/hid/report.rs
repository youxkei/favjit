//! The reports a HID keyboard device is told with, and the state they describe.
//!
//! A HID report is *state*, not an event: every one says what is down. So this
//! holds the state between keystrokes and answers with the reports that carry it
//! forward, where a report built from scratch per keystroke would say every other
//! key had just been released.
//!
//! Here rather than in the host that writes them, because writing them is the only
//! part of this a test cannot drive: the bytes, the slots and which report a page
//! goes in are all answerable in the suite, and a host is what is left over
//! (ADR-0006).
//!
//! Nothing here decides what the OS should be told. A set of modifier keys arrives
//! whole and is written as it stands; a repeat arrives as the key let go and
//! pressed again. Those are the sink's, and the suite's.

use super::{page, usage};
use crate::{Key, ModifierKeys, PointerReport};

/// The HID modifier byte's bits, in the order the descriptor declares them.
const LEFT_CONTROL: u8 = 1 << 0;
const LEFT_SHIFT: u8 = 1 << 1;
const LEFT_OPTION: u8 = 1 << 2;
const LEFT_COMMAND: u8 = 1 << 3;
const RIGHT_CONTROL: u8 = 1 << 4;
const RIGHT_SHIFT: u8 = 1 << 5;
const RIGHT_OPTION: u8 = 1 << 6;
const RIGHT_COMMAND: u8 = 1 << 7;

/// How many reports one key event can need.
///
/// Two, and only for a control: the modifier bits are declared in the keyboard's
/// report and the control's usage in its page's, so a control taken with a
/// modifier is the one event that cannot be said in one report.
const MOST_REPORTS: usize = 2;

/// The 32 usage slots every one of the device's reports carries.
///
/// The usages are **16-bit**, not the byte-wide ones an ordinary HID keyboard
/// report carries. The width is what lets a report reach usages above `0xff`, and
/// a report written to the byte-wide layout would be misread.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Keys([u16; 32]);

impl Keys {
    /// Add a usage, if there is room and it is not already there.
    ///
    /// Silently full is the right failure: the slots outnumber the fingers by four
    /// to one, so a full report means something is already wrong, and dropping the
    /// press is better than dropping an older key that is still physically down.
    fn press(&mut self, usage: u16) {
        if self.holds(usage) {
            return;
        }
        if let Some(slot) = self.0.iter_mut().find(|slot| **slot == 0) {
            *slot = usage;
        }
    }

    fn release(&mut self, usage: u16) {
        for slot in self.0.iter_mut() {
            if *slot == usage {
                *slot = 0;
            }
        }
    }

    pub fn holds(&self, usage: u16) -> bool {
        self.0.contains(&usage)
    }

    /// Only the tests ask, for the same reason [`Report::holds`] is theirs alone:
    /// nothing here reads the state back to decide anything.
    #[cfg(test)]
    fn any(&self) -> bool {
        self.0.iter().any(|usage| *usage != 0)
    }

    fn write(&self, out: &mut Vec<u8>) {
        for usage in self.0 {
            out.extend_from_slice(&usage.to_ne_bytes());
        }
    }
}

/// The report ids the output device's descriptor declares, one per page it carries
/// (`docs/platform/macos/virtual-hid-device.md`).
const KEYBOARD_REPORT_ID: u8 = 1;
const CONSUMER_REPORT_ID: u8 = 2;
const APPLE_VENDOR_TOP_CASE_REPORT_ID: u8 = 3;
const APPLE_VENDOR_KEYBOARD_REPORT_ID: u8 = 4;
const GENERIC_DESKTOP_REPORT_ID: u8 = 7;

/// The 67 bytes of a keyboard report, as the state they describe.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Report {
    pub modifiers: u8,
    pub keys: Keys,
}

impl Report {
    fn press(&mut self, usage: u16) {
        self.keys.press(usage);
    }

    fn release(&mut self, usage: u16) {
        self.keys.release(usage);
    }

    /// Only the tests ask, and that is the shape of this: nothing here reads the
    /// state back to decide anything, because every event carries the whole of what
    /// the OS is to be told.
    #[cfg(test)]
    pub fn holds(&self, usage: u16) -> bool {
        self.keys.holds(usage)
    }

    /// The bytes on the wire: report id, the modifier byte, one reserved byte, and
    /// the 32 usages.
    pub fn bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(67);
        out.push(KEYBOARD_REPORT_ID);
        out.push(self.modifiers);
        out.push(0);
        self.keys.write(&mut out);
        out
    }
}

/// Which of the output device's other reports a control goes out on.
///
/// One report per page, because that is how the device declares them: the same
/// usage number is a different control on each page, so a single report with the
/// page written into it would arrive as whichever control that number means on the
/// page the report was declared for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlPage {
    Consumer,
    AppleVendorTopCase,
    AppleVendorKeyboard,
    GenericDesktop,
}

impl ControlPage {
    /// Every one the device declares.
    const ALL: [Self; 4] = [
        Self::Consumer,
        Self::AppleVendorTopCase,
        Self::AppleVendorKeyboard,
        Self::GenericDesktop,
    ];

    /// Which report carries this HID page, for a page the device has one for.
    pub fn of(page: u32) -> Option<Self> {
        // Compared rather than matched, because a `match` arm on one of these
        // constants binds the name instead of testing it.
        if page == self::page::CONSUMER {
            return Some(Self::Consumer);
        }
        if page == self::page::APPLE_VENDOR_TOP_CASE {
            return Some(Self::AppleVendorTopCase);
        }
        if page == self::page::APPLE_VENDOR_KEYBOARD {
            return Some(Self::AppleVendorKeyboard);
        }
        if page == self::page::GENERIC_DESKTOP {
            return Some(Self::GenericDesktop);
        }
        None
    }

    fn id(self) -> u8 {
        match self {
            Self::Consumer => CONSUMER_REPORT_ID,
            Self::AppleVendorTopCase => APPLE_VENDOR_TOP_CASE_REPORT_ID,
            Self::AppleVendorKeyboard => APPLE_VENDOR_KEYBOARD_REPORT_ID,
            Self::GenericDesktop => GENERIC_DESKTOP_REPORT_ID,
        }
    }
}

/// The 65 bytes of a control report: a report id and the 32 usages, with no
/// modifier byte — the modifiers of a control keystroke ride on the keyboard
/// report, which is where the descriptor declares those eight bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ControlReport {
    pub page: ControlPage,
    keys: Keys,
}

impl ControlReport {
    pub fn bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(65);
        out.push(self.page.id());
        self.keys.write(&mut out);
        out
    }
}

/// One report to write, on whichever of the device's reports carries it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sent {
    Keyboard(Report),
    Control(ControlReport),
}

/// What the device has been told is down on each of its control reports.
///
/// State per report for the reason the keyboard keeps it: a HID report says what
/// is down, so one built from scratch per control would say every other control
/// had just been released — and brightness held while the volume is turned down is
/// two of them at once.
#[derive(Debug, Clone, Copy, Default)]
struct Controls {
    consumer: Keys,
    top_case: Keys,
    apple_keyboard: Keys,
    generic_desktop: Keys,
}

impl Controls {
    fn slots(&mut self, page: ControlPage) -> &mut Keys {
        match page {
            ControlPage::Consumer => &mut self.consumer,
            ControlPage::AppleVendorTopCase => &mut self.top_case,
            ControlPage::AppleVendorKeyboard => &mut self.apple_keyboard,
            ControlPage::GenericDesktop => &mut self.generic_desktop,
        }
    }

    fn control(&mut self, page: ControlPage, usage: u16, down: bool) -> ControlReport {
        let keys = self.slots(page);
        if down {
            keys.press(usage);
        } else {
            keys.release(usage);
        }
        ControlReport { page, keys: *keys }
    }
}

/// What the device has been told, and what to tell it next.
///
/// The key slots and nothing else: no modifier state of its own, because every
/// event carries the whole set that must be down and one kept here would be
/// something that can disagree with what the sink believes
/// ([`Injected`](crate::Injected)).
#[derive(Debug, Clone, Copy, Default)]
pub struct Keyboard {
    report: Report,
    controls: Controls,
}

impl Keyboard {
    /// The report that says these modifier keys are down and nothing has changed
    /// besides.
    pub fn modifiers(&mut self, keys: ModifierKeys) -> Report {
        self.report.modifiers = bits(keys);
        self.report
    }

    /// The reports that carry one key event, or the key that has no usage.
    pub fn key(&mut self, key: Key, modifiers: ModifierKeys, down: bool) -> Result<Vec<Sent>, Key> {
        let Some((page, usage)) = usage::of(key) else {
            return Err(key);
        };
        let mut reports = Vec::with_capacity(MOST_REPORTS);

        // A control goes out on its own page's report, and the modifier bits of the
        // keystroke still ride on the keyboard's: there is nowhere else for them,
        // the descriptor declaring those eight bits in the keyboard report alone.
        if page != self::page::KEYBOARD_OR_KEYPAD {
            let Some(control_page) = ControlPage::of(page) else {
                return Err(key);
            };
            let modifiers = bits(modifiers);
            if modifiers != self.report.modifiers {
                self.report.modifiers = modifiers;
                reports.push(Sent::Keyboard(self.report));
            }
            reports.push(Sent::Control(self.controls.control(
                control_page,
                usage,
                down,
            )));
            return Ok(reports);
        }

        // A modifier key takes no key slot: the descriptor declares those eight
        // usages as the modifier byte, and the set this event carries is that byte
        // whole — including this key, since a modifier key and the set it belongs
        // to are one claim and not two.
        if modifier_bit(key).is_some() {
            self.report.modifiers = bits(modifiers);
            reports.push(Sent::Keyboard(self.report));
            return Ok(reports);
        }

        if down {
            self.report.modifiers = bits(modifiers);
            self.report.press(usage);
            reports.push(Sent::Keyboard(self.report));
        } else {
            // One report, and the modifiers of it are the ones the event carries:
            // what to do about a modifier the keystroke had borrowed is the sink's,
            // and it says so with an event of its own rather than leaving this to
            // work out what is still held.
            self.report.modifiers = bits(modifiers);
            self.report.release(usage);
            reports.push(Sent::Keyboard(self.report));
        }
        Ok(reports)
    }

    /// The reports that say nothing is down, one for every report the device has.
    ///
    /// All of them, and not the ones something went out on: which those were is a
    /// thing to remember and this remembers nothing it is not obliged to. An empty
    /// report said empty again is one write on a path that runs once, where a
    /// control left down repeats — brightness ramps for as long as the key is held.
    pub fn release_all(&mut self) -> Vec<Sent> {
        *self = Self::default();
        let mut reports = vec![Sent::Keyboard(self.report)];
        reports.extend(ControlPage::ALL.map(|page| {
            Sent::Control(ControlReport {
                page,
                keys: Keys::default(),
            })
        }));
        reports
    }
}

/// The 8 bytes of a pointing report: the buttons, then the two deltas and the two
/// wheels.
///
/// The deltas saturate rather than wrapping, because a byte is the whole of the
/// field: a movement of 200 points arriving as -56 would send the cursor the other
/// way.
pub fn pointing(report: PointerReport) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(8);
    bytes.extend_from_slice(&report.buttons.bits().to_ne_bytes());
    bytes.push(saturate(report.dx) as u8);
    bytes.push(saturate(report.dy) as u8);
    bytes.push(saturate(report.vertical_wheel) as u8);
    bytes.push(saturate(report.horizontal_wheel) as u8);
    bytes
}

fn saturate(value: i32) -> i8 {
    value.clamp(i8::MIN as i32, i8::MAX as i32) as i8
}

/// The bit a modifier key contributes, if it is one.
fn modifier_bit(key: Key) -> Option<u8> {
    Some(match key {
        Key::LeftControl => LEFT_CONTROL,
        Key::LeftShift => LEFT_SHIFT,
        Key::LeftOption => LEFT_OPTION,
        Key::LeftCommand => LEFT_COMMAND,
        Key::RightControl => RIGHT_CONTROL,
        Key::RightShift => RIGHT_SHIFT,
        Key::RightOption => RIGHT_OPTION,
        Key::RightCommand => RIGHT_COMMAND,
        // Caps Lock is a key and not a modifier bit in HID: the descriptor has no
        // bit for it, and what a keyboard sends is the key, with the OS holding the
        // lock. So it goes into a key slot like any letter.
        _ => return None,
    })
}

/// The modifier byte for a set of modifier keys.
///
/// Key by key off [`modifier_bit`], and not by taking the set's own bit pattern:
/// the two numberings are not the same thing, and one that happened to line up
/// today would be an agreement nothing states.
///
/// Caps Lock is left out because this byte has no bit for it — a set naming it is
/// a set with a key this report cannot mention, and the layout only ever emits it
/// as a key of its own, which takes a slot.
fn bits(keys: ModifierKeys) -> u8 {
    keys.keys()
        .filter_map(modifier_bit)
        .fold(0, |out, bit| out | bit)
}

#[cfg(test)]
mod tests {
    //! Every test here is one of the two things this module holds: a **table** —
    //! what the platform calls a key, a page, a bit — or an **encoding**: the bytes
    //! of a report, which report a usage goes in, that a report says what is down.
    //! None of them says what the OS *should* be told; that is the sink's, and the
    //! suite's.

    use super::*;

    const SEMICOLON: u16 = 0x33;

    fn keyboard() -> Keyboard {
        Keyboard::default()
    }

    /// One key event, as the keyboard reports it produced.
    ///
    /// Panicking on a control is what keeps these about the keyboard: a key that
    /// started going out on another page would fail here rather than quietly pass
    /// an assertion about a report nobody sent.
    fn typed(
        keyboard: &mut Keyboard,
        key: Key,
        modifiers: ModifierKeys,
        down: bool,
    ) -> Vec<Report> {
        keyboard
            .key(key, modifiers, down)
            .expect("sendable")
            .into_iter()
            .map(|sent| match sent {
                Sent::Keyboard(report) => report,
                Sent::Control(report) => panic!("{key:?} went out as {report:?}"),
            })
            .collect()
    }

    /// The same for a control, which goes out on a page of its own.
    fn controlled(
        keyboard: &mut Keyboard,
        key: Key,
        modifiers: ModifierKeys,
        down: bool,
    ) -> Vec<Sent> {
        keyboard.key(key, modifiers, down).expect("sendable")
    }

    #[test]
    fn a_keyboard_report_is_sixty_seven_bytes_of_id_modifiers_and_usages() {
        let mut keyboard = keyboard();
        let reports = typed(
            &mut keyboard,
            Key::O,
            ModifierKeys::of(&[Key::RightShift, Key::LeftCommand]),
            true,
        );
        let bytes = reports[0].bytes();

        assert_eq!(bytes.len(), 67);
        assert_eq!(bytes[0], KEYBOARD_REPORT_ID);
        assert_eq!(bytes[1], RIGHT_SHIFT | LEFT_COMMAND);
        assert_eq!(bytes[2], 0, "the reserved byte");
        assert_eq!(&bytes[3..5], &[0x12, 0x00], "`o`, and little-endian");
    }

    #[test]
    fn a_control_report_is_sixty_five_bytes_behind_its_own_id() {
        // A consumer usage above `0xff`, which is where the width of a slot
        // matters: written a byte at a time this would be a different control.
        let mut keyboard = keyboard();
        let sent = controlled(&mut keyboard, Key::Dictation, ModifierKeys::NONE, true);

        let [Sent::Control(report)] = sent.as_slice() else {
            panic!("{sent:?}");
        };
        let bytes = report.bytes();
        assert_eq!(bytes.len(), 65);
        assert_eq!(bytes[0], CONSUMER_REPORT_ID);
        assert_eq!(&bytes[1..3], &[0xCF, 0x00]);
    }

    #[test]
    fn the_modifier_byte_is_the_set_and_the_set_is_all_of_it() {
        // Down, up, a modifier key of its own, and a change with no key at all:
        // every one writes the set it was handed and nothing besides.
        let mut keyboard = keyboard();
        let shift = ModifierKeys::of(&[Key::LeftShift]);

        assert_eq!(
            typed(&mut keyboard, Key::O, shift, true)[0].modifiers,
            LEFT_SHIFT
        );
        assert_eq!(
            typed(&mut keyboard, Key::O, shift, false)[0].modifiers,
            LEFT_SHIFT
        );
        assert_eq!(
            typed(&mut keyboard, Key::LeftShift, shift, true)[0].modifiers,
            LEFT_SHIFT
        );
        assert_eq!(keyboard.modifiers(ModifierKeys::NONE).modifiers, 0);
        assert_eq!(
            typed(&mut keyboard, Key::O, ModifierKeys::NONE, true)[0].modifiers,
            0
        );
    }

    #[test]
    fn a_modifier_key_takes_no_slot_and_caps_lock_takes_one() {
        let mut keyboard = keyboard();
        let modifier = typed(
            &mut keyboard,
            Key::LeftShift,
            ModifierKeys::of(&[Key::LeftShift]),
            true,
        );
        assert_eq!(modifier.len(), 1);
        assert!(!modifier[0].holds(0xE1));

        // Caps Lock is the exception, and the descriptor is why: it has no bit in
        // the modifier byte, so it goes into a slot like any letter.
        let lock = typed(&mut keyboard, Key::CapsLock, ModifierKeys::NONE, true);
        assert_eq!(lock[0].modifiers, 0);
        assert!(lock[0].holds(0x39));
    }

    #[test]
    fn a_report_says_what_is_down_so_one_key_leaving_leaves_the_others() {
        let mut keyboard = keyboard();
        typed(&mut keyboard, Key::O, ModifierKeys::NONE, true);
        typed(&mut keyboard, Key::A, ModifierKeys::NONE, true);
        let reports = typed(&mut keyboard, Key::O, ModifierKeys::NONE, false);

        assert!(!reports[0].holds(0x12));
        assert!(reports[0].holds(0x04), "`a` is still down");
    }

    #[test]
    fn one_event_is_one_report_unless_a_control_needs_the_modifier_byte() {
        let mut keyboard = keyboard();
        assert_eq!(
            typed(&mut keyboard, Key::O, ModifierKeys::NONE, true).len(),
            1
        );
        assert_eq!(
            typed(&mut keyboard, Key::O, ModifierKeys::NONE, false).len(),
            1
        );
        assert_eq!(
            controlled(&mut keyboard, Key::VolumeUp, ModifierKeys::NONE, true).len(),
            1
        );
        // The bits are declared in the keyboard's report and the usage in the
        // consumer's, so this is the one event that cannot be said in one.
        assert_eq!(
            controlled(
                &mut keyboard,
                Key::VolumeDown,
                ModifierKeys::of(&[Key::LeftShift]),
                true
            )
            .len(),
            2
        );
    }

    #[test]
    fn each_page_has_its_own_report_and_they_do_not_share_slots() {
        // The same usage number is a different control on each page, so one page's
        // report must never carry another's usage.
        let mut keyboard = keyboard();
        controlled(&mut keyboard, Key::MissionControl, ModifierKeys::NONE, true);
        let sent = controlled(&mut keyboard, Key::BrightnessUp, ModifierKeys::NONE, true);

        let [Sent::Control(report)] = sent.as_slice() else {
            panic!("{sent:?}");
        };
        assert_eq!(report.page, ControlPage::Consumer);
        assert!(report.keys.holds(0x6F));
        assert_eq!(report.keys.0.iter().filter(|usage| **usage != 0).count(), 1);
    }

    #[test]
    fn release_all_empties_every_report_the_device_has() {
        let mut keyboard = keyboard();
        typed(&mut keyboard, Key::O, ModifierKeys::NONE, true);
        controlled(&mut keyboard, Key::BrightnessDown, ModifierKeys::NONE, true);

        let reports = keyboard.release_all();
        assert_eq!(reports.len(), 1 + ControlPage::ALL.len());
        for report in reports {
            match report {
                Sent::Keyboard(report) => assert_eq!(report, Report::default()),
                Sent::Control(report) => assert!(!report.keys.any()),
            }
        }
    }

    #[test]
    fn a_key_no_usage_names_is_reported_rather_than_sent() {
        let mut keyboard = keyboard();
        assert_eq!(
            keyboard.key(Key::Fn, ModifierKeys::NONE, true),
            Err(Key::Fn),
            "Fn is read from Apple's top-case page and never emitted"
        );
    }

    #[test]
    fn every_modifier_key_has_its_own_bit() {
        let keys = [
            Key::LeftControl,
            Key::LeftShift,
            Key::LeftOption,
            Key::LeftCommand,
            Key::RightControl,
            Key::RightShift,
            Key::RightOption,
            Key::RightCommand,
        ];
        let mut seen = 0u8;
        for key in keys {
            let bit = modifier_bit(key).expect("a modifier key has a bit");
            assert_eq!(seen & bit, 0, "{key:?} shares a bit with an earlier key");
            seen |= bit;
        }
        assert_eq!(seen, u8::MAX);
    }

    #[test]
    fn a_set_of_modifier_keys_says_what_the_byte_says() {
        assert_eq!(bits(ModifierKeys::NONE), 0);
        assert_eq!(
            bits(ModifierKeys::of(&[Key::LeftShift, Key::RightShift])),
            LEFT_SHIFT | RIGHT_SHIFT,
            "both sides of one modifier at once, which a set can say"
        );
        assert_eq!(
            bits(ModifierKeys::of(&[Key::CapsLock])),
            0,
            "and the one key this byte has no bit for"
        );
    }

    #[test]
    fn a_pointing_report_is_eight_bytes_and_a_delta_past_one_saturates() {
        let bytes = pointing(PointerReport {
            dx: 200,
            dy: -200,
            vertical_wheel: -3,
            horizontal_wheel: 0,
            buttons: crate::Buttons::NONE.with(1),
        });

        assert_eq!(bytes.len(), 8);
        assert_eq!(bytes[4], 127, "a byte is the whole of the field");
        assert_eq!(bytes[5] as i8, -128);
        assert_eq!(bytes[6] as i8, -3);
    }

    #[test]
    fn a_shifted_character_is_released_under_the_set_it_was_given() {
        let mut keyboard = keyboard();
        let held = ModifierKeys::of(&[Key::LeftShift]);
        typed(&mut keyboard, Key::Semicolon, held, true);
        let reports = typed(&mut keyboard, Key::Semicolon, held, false);

        assert_eq!(reports.len(), 1);
        assert!(!reports[0].holds(SEMICOLON));
        assert_eq!(reports[0].modifiers, LEFT_SHIFT);
    }
}
