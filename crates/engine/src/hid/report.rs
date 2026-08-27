//! What an `Injected` event does to the device's reports.
//!
//! The bytes, the slots and which report a page goes in are `favjit-hid`'s
//! (ADR-0005), re-exported below; what is here is the one decision that table
//! cannot make on its own — which usages ride together on one report, and when a
//! report is rebuilt from scratch rather than carried forward.

pub use favjit_hid::report::{
    modifier_bit, pointing, ControlPage, ControlReport, Keys, Report, Sent,
};

use crate::{hid::page, hid::usage, Injected, Key, ModifierKeys, PointerReport};

/// How many reports one key event can need.
///
/// Two, and only for a control: the modifier bits are declared in the keyboard's
/// report and the control's usage in its page's, so a control taken with a
/// modifier is the one event that cannot be said in one report.
const MOST_REPORTS: usize = 2;

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
/// something that can disagree with what the sink believes ([`Injected`]).
#[derive(Debug, Clone, Copy, Default)]
pub struct Keyboard {
    report: Report,
    controls: Controls,
}

impl Keyboard {
    /// The report that says these modifier keys are down and nothing has changed
    /// besides.
    fn modifiers(&mut self, keys: ModifierKeys) -> Report {
        self.report.modifiers = bits(keys);
        self.report
    }

    /// The reports that carry one key event, or the key that has no usage.
    fn key(&mut self, key: Key, modifiers: ModifierKeys, down: bool) -> Result<Vec<Sent>, Key> {
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
}

/// What one `Injected` event renders to.
///
/// A keyboard report goes through `device` and keeps state there; a pointing
/// report is one call's worth of bytes with nothing to keep, which is why it
/// carries the report itself rather than a further call to make.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rendered {
    Keyboard(Vec<Sent>),
    Pointing(PointerReport),
}

/// Turn one `Injected` event into what a device is actually told, against the
/// state `device` is keeping.
///
/// **The one door into a [`Keyboard`]**, which is why its own methods are not
/// `pub`: a caller reaching them directly would be a second place deciding how
/// an `Injected` maps to a report, and a report's own encoding is a step
/// ADR-0006 keeps out of a host. `pub(crate)` rather than `pub`, because the
/// call belongs to [`crate::sink::Sink`] alone — a host handed this and a
/// `Keyboard` of its own could render an `Injected` `Sink` never produced,
/// which is a second door in.
pub(crate) fn render(device: &mut Keyboard, injected: Injected) -> Result<Rendered, Key> {
    Ok(match injected {
        Injected::Pointer(report) => Rendered::Pointing(report),
        Injected::Modifiers(keys) => {
            Rendered::Keyboard(vec![Sent::Keyboard(device.modifiers(keys))])
        }
        Injected::KeyDown { key, modifiers } => {
            Rendered::Keyboard(device.key(key, modifiers, true)?)
        }
        Injected::KeyUp { key, modifiers } => {
            Rendered::Keyboard(device.key(key, modifiers, false)?)
        }
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
    //! Every test here is one of the two things this module holds: which usages
    //! ride together on one report, and when a report is rebuilt from scratch
    //! rather than carried forward. None of them says what the OS *should* be
    //! told; that is the sink's, and the suite's. Whether the bytes and the slots
    //! themselves are right is `favjit-hid`'s own tests to answer.

    use favjit_hid::report::{
        CONSUMER_REPORT_ID, KEYBOARD_REPORT_ID, LEFT_COMMAND, LEFT_SHIFT, RIGHT_SHIFT,
    };

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
        assert_eq!(report.keys.count(), 1);
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
