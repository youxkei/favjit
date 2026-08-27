//! The reports a HID keyboard device is told with, and the state they describe.
//!
//! A HID report is *state*, not an event: every one says what is down. So this
//! holds the state a device's report carries and answers with the bytes that
//! carry it forward — the same state and the same bytes on whichever machine
//! assembles them, since both machines' `engine`s have to agree on this shape
//! and not only on the numbers inside it (ADR-0005).
//!
//! Nothing here decides what the OS should be told: which usages ride together
//! and when a report is rebuilt from scratch are `engine`'s, over the pressing
//! and releasing this module answers for.

use crate::{Key, PointerReport};

/// The HID modifier byte's bits, in the order the descriptor declares them.
pub const LEFT_CONTROL: u8 = 1 << 0;
pub const LEFT_SHIFT: u8 = 1 << 1;
pub const LEFT_OPTION: u8 = 1 << 2;
pub const LEFT_COMMAND: u8 = 1 << 3;
pub const RIGHT_CONTROL: u8 = 1 << 4;
pub const RIGHT_SHIFT: u8 = 1 << 5;
pub const RIGHT_OPTION: u8 = 1 << 6;
pub const RIGHT_COMMAND: u8 = 1 << 7;

/// The bit `key` contributes to the modifier byte, if it is one of the eight the
/// descriptor declares there.
///
/// Caps Lock is a key and not a modifier bit in HID: the descriptor has no bit
/// for it, and what a keyboard sends is the key, with the OS holding the lock —
/// so it is not `None` because this table forgot it, but because a report has no
/// bit to give it.
pub const fn modifier_bit(key: Key) -> Option<u8> {
    Some(match key {
        Key::LeftControl => LEFT_CONTROL,
        Key::LeftShift => LEFT_SHIFT,
        Key::LeftOption => LEFT_OPTION,
        Key::LeftCommand => LEFT_COMMAND,
        Key::RightControl => RIGHT_CONTROL,
        Key::RightShift => RIGHT_SHIFT,
        Key::RightOption => RIGHT_OPTION,
        Key::RightCommand => RIGHT_COMMAND,
        _ => return None,
    })
}

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
    pub fn press(&mut self, usage: u16) {
        if self.holds(usage) {
            return;
        }
        if let Some(slot) = self.0.iter_mut().find(|slot| **slot == 0) {
            *slot = usage;
        }
    }

    pub fn release(&mut self, usage: u16) {
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
    /// nothing here reads the state back to decide anything. `pub` rather than
    /// `#[cfg(test)]`, because the tests that ask are `engine`'s, in a crate of
    /// their own, where a `cfg(test)` here would not be active for them.
    pub fn any(&self) -> bool {
        self.0.iter().any(|usage| *usage != 0)
    }

    /// How many slots are occupied. Theirs alone, for the same reason.
    pub fn count(&self) -> usize {
        self.0.iter().filter(|usage| **usage != 0).count()
    }

    /// Every usage this report is holding, for whoever has to say which key that
    /// is rather than only that one is down — a live report of what favjit would
    /// send, in particular, which names a key or it is not reporting anything a
    /// person watching could use.
    pub fn usages(&self) -> impl Iterator<Item = u16> + '_ {
        self.0.iter().copied().filter(|&usage| usage != 0)
    }

    fn write(&self, out: &mut Vec<u8>) {
        for usage in self.0 {
            out.extend_from_slice(&usage.to_ne_bytes());
        }
    }

    fn read(bytes: &[u8]) -> Self {
        let mut slots = [0u16; 32];
        for (slot, pair) in slots.iter_mut().zip(bytes.chunks_exact(2)) {
            *slot = u16::from_ne_bytes([pair[0], pair[1]]);
        }
        Self(slots)
    }
}

/// The report ids the output device's descriptor declares, one per page it carries
/// (`docs/platform/macos/virtual-hid-device.md`).
pub const KEYBOARD_REPORT_ID: u8 = 1;
pub const CONSUMER_REPORT_ID: u8 = 2;
pub const APPLE_VENDOR_TOP_CASE_REPORT_ID: u8 = 3;
pub const APPLE_VENDOR_KEYBOARD_REPORT_ID: u8 = 4;
pub const GENERIC_DESKTOP_REPORT_ID: u8 = 7;

/// The 67 bytes of a keyboard report, as the state they describe.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Report {
    pub modifiers: u8,
    pub keys: Keys,
}

impl Report {
    pub fn press(&mut self, usage: u16) {
        self.keys.press(usage);
    }

    pub fn release(&mut self, usage: u16) {
        self.keys.release(usage);
    }

    /// Only the tests ask, and that is the shape of this: nothing here reads the
    /// state back to decide anything, because every event carries the whole of what
    /// the OS is to be told. `pub` unconditionally, for the reason [`Keys::any`]
    /// is: the tests that ask are `engine`'s own, compiled without this crate's
    /// `cfg(test)` in force.
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

    /// The state [`Report::bytes`] wrote, for reading back what crossed the host
    /// boundary in [`crate::report`]'s own vocabulary rather than the platform's
    /// (ADR-0005): `host-sim` stands in for a device that only ever receives
    /// these bytes, so this is the one place its tests may decide what a report
    /// meant.
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != 67 || bytes[0] != KEYBOARD_REPORT_ID {
            return None;
        }
        Some(Self {
            modifiers: bytes[1],
            keys: Keys::read(&bytes[3..]),
        })
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
    pub const ALL: [Self; 4] = [
        Self::Consumer,
        Self::AppleVendorTopCase,
        Self::AppleVendorKeyboard,
        Self::GenericDesktop,
    ];

    /// Which report carries this HID page, for a page the device has one for.
    pub fn of(page: u32) -> Option<Self> {
        // Compared rather than matched, because a `match` arm on one of these
        // constants binds the name instead of testing it.
        if page == crate::page::CONSUMER {
            return Some(Self::Consumer);
        }
        if page == crate::page::APPLE_VENDOR_TOP_CASE {
            return Some(Self::AppleVendorTopCase);
        }
        if page == crate::page::APPLE_VENDOR_KEYBOARD {
            return Some(Self::AppleVendorKeyboard);
        }
        if page == crate::page::GENERIC_DESKTOP {
            return Some(Self::GenericDesktop);
        }
        None
    }

    pub fn id(self) -> u8 {
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
    pub keys: Keys,
}

impl ControlReport {
    pub fn bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(65);
        out.push(self.page.id());
        self.keys.write(&mut out);
        out
    }

    /// The state [`ControlReport::bytes`] wrote, for the reason
    /// [`Report::from_bytes`] exists. `page` has to be given rather than read
    /// off the bytes: two pages can share a report id only by coincidence, and
    /// nothing in the bytes themselves says which one this report was declared
    /// on.
    pub fn from_bytes(page: ControlPage, bytes: &[u8]) -> Option<Self> {
        if bytes.len() != 65 || bytes[0] != page.id() {
            return None;
        }
        Some(Self {
            page,
            keys: Keys::read(&bytes[1..]),
        })
    }
}

/// One report to write, on whichever of the device's reports carries it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sent {
    Keyboard(Report),
    Control(ControlReport),
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

/// The report [`pointing`] wrote, for the reason [`Report::from_bytes`] exists.
///
/// The deltas and wheels come back exactly as [`saturate`] left them, not as
/// whatever `dx`/`dy` the caller of [`pointing`] originally gave it: the
/// boundary already lost anything past a byte, and a decode that widened them
/// back out would claim the report said more than the bytes actually carry.
pub fn pointing_from_bytes(bytes: &[u8]) -> Option<PointerReport> {
    let &[b0, b1, b2, b3, dx, dy, vertical_wheel, horizontal_wheel] = bytes else {
        return None;
    };
    Some(PointerReport {
        buttons: crate::Buttons::from_bits(u32::from_ne_bytes([b0, b1, b2, b3])),
        dx: i32::from(dx as i8),
        dy: i32::from(dy as i8),
        vertical_wheel: i32::from(vertical_wheel as i8),
        horizontal_wheel: i32::from(horizontal_wheel as i8),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_report_with_nothing_pressed_holds_nothing() {
        let report = Report::default();
        assert!(!report.holds(0x04));
    }

    #[test]
    fn pressing_the_same_usage_twice_uses_one_slot() {
        let mut report = Report::default();
        report.press(0x04);
        report.press(0x04);
        let mut expected = Report::default();
        expected.press(0x04);
        assert_eq!(report, expected);
    }

    #[test]
    fn releasing_clears_every_slot_that_matches() {
        let mut report = Report::default();
        report.press(0x04);
        report.release(0x04);
        assert!(!report.holds(0x04));
    }

    #[test]
    fn a_keyboard_report_survives_the_bytes_it_writes() {
        let mut report = Report {
            modifiers: LEFT_SHIFT | RIGHT_COMMAND,
            ..Report::default()
        };
        report.press(0x04);
        report.press(0x05);
        assert_eq!(Report::from_bytes(&report.bytes()), Some(report));
    }

    #[test]
    fn bytes_of_the_wrong_length_or_id_are_not_a_keyboard_report() {
        assert_eq!(Report::from_bytes(&[KEYBOARD_REPORT_ID; 66]), None);
        assert_eq!(Report::from_bytes(&[0u8; 67]), None);
    }

    #[test]
    fn a_control_report_survives_the_bytes_it_writes() {
        let mut report = ControlReport {
            page: ControlPage::Consumer,
            keys: Keys::default(),
        };
        report.keys.press(0xE9);
        assert_eq!(
            ControlReport::from_bytes(ControlPage::Consumer, &report.bytes()),
            Some(report)
        );
    }

    #[test]
    fn a_control_report_is_read_back_on_the_page_it_was_declared_for() {
        let mut report = ControlReport {
            page: ControlPage::Consumer,
            keys: Keys::default(),
        };
        report.keys.press(0xE9);
        assert_eq!(
            ControlReport::from_bytes(ControlPage::AppleVendorTopCase, &report.bytes()),
            None
        );
    }

    #[test]
    fn a_pointing_report_survives_the_bytes_it_writes() {
        let report = PointerReport {
            dx: -3,
            dy: 7,
            vertical_wheel: 1,
            horizontal_wheel: -1,
            buttons: crate::Buttons::NONE.with(1),
        };
        assert_eq!(pointing_from_bytes(&pointing(report)), Some(report));
    }

    #[test]
    fn a_pointing_delta_past_a_byte_comes_back_saturated_rather_than_widened() {
        let report = PointerReport::moved(200, 0);
        let decoded = pointing_from_bytes(&pointing(report)).expect("eight bytes");
        assert_eq!(decoded.dx, i32::from(i8::MAX));
    }
}
