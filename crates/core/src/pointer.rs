//! The pointer vocabulary.

/// Which pointer buttons are down.
///
/// Absolute state rather than a change, because that is what both ends carry: a
/// HID device reports each button as an element with a value of 0 or 1, and a
/// virtual pointing device's report carries the whole set as a bitmask. A
/// "button 2 went down" event would make some layer hold the set and be the only
/// thing that knew it, and a dropped event would leave a button stuck down.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Buttons(u32);

impl Buttons {
    pub const NONE: Self = Self(0);

    /// `button` is one-based, as HID's button page numbers them.
    pub const fn with(self, button: u8) -> Self {
        Self(self.0 | (1 << (button.saturating_sub(1) as u32)))
    }

    pub const fn without(self, button: u8) -> Self {
        Self(self.0 & !(1 << (button.saturating_sub(1) as u32)))
    }

    pub const fn set(self, button: u8, down: bool) -> Self {
        if down {
            self.with(button)
        } else {
            self.without(button)
        }
    }

    pub const fn holds(self, button: u8) -> bool {
        self.0 & (1 << (button.saturating_sub(1) as u32)) != 0
    }

    pub const fn any(self) -> bool {
        self.0 != 0
    }

    pub const fn bits(self) -> u32 {
        self.0
    }

    pub const fn from_bits(bits: u32) -> Self {
        Self(bits)
    }
}

/// One pointer report: how far it moved, how far it scrolled, and what is held.
///
/// The motion is relative and the buttons are absolute, which is the shape of
/// the reports on both sides of the relay. Splitting the axes into separate
/// events would be the more obvious vocabulary and the wrong one: the OS applies
/// its acceleration curve per report, so a diagonal delivered as one report along
/// each axis is accelerated as two short movements and travels less far than the
/// same motion the hardware described in one
/// (`docs/platform/macos/virtual-hid-device.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PointerReport {
    pub dx: i32,
    pub dy: i32,
    pub vertical_wheel: i32,
    pub horizontal_wheel: i32,
    pub buttons: Buttons,
}

impl PointerReport {
    pub const fn moved(dx: i32, dy: i32) -> Self {
        Self {
            dx,
            dy,
            vertical_wheel: 0,
            horizontal_wheel: 0,
            buttons: Buttons::NONE,
        }
    }

    /// Whether this report says anything at all.
    ///
    /// A report with no motion, no scroll and no buttons is not worth relaying —
    /// but "no buttons" is only silence next to a previous report that also had
    /// none, so the caller decides what to compare against.
    pub const fn is_still(&self) -> bool {
        self.dx == 0 && self.dy == 0 && self.vertical_wheel == 0 && self.horizontal_wheel == 0
    }
}

/// Which way the relayed wheel turns.
///
/// Here rather than in the host, because it is a conversion from what the hardware
/// said into what the machine is told, and there is one place for those (ADR-0003).
/// It also means the end-to-end suite can pin it.
///
/// How *far* the pointer travels is not here: that is a property of the device
/// macOS delivers through, and is set on the device itself
/// (`docs/platform/macos/pointer-acceleration.md`). Multiplying the reports instead
/// would multiply the single-unit reports a TrackPoint mostly makes, so slow
/// movement would go in steps.
/// What the output device's pointer should be set to, if anything.
///
/// Set outside the bounds the device keeps these two in and the value is either
/// refused or clamped somewhere out of sight, so they are clamped as they are taken
/// in — here, where a test can say what happens to a number nobody meant, rather
/// than beside the call that writes it.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Wanted {
    resolution: Option<f64>,
    acceleration: Option<f64>,
}

impl Wanted {
    /// The bounds macOS keeps these two properties in.
    const RESOLUTION: (f64, f64) = (10.0, 1995.0);
    const ACCELERATION: (f64, f64) = (0.0, 40.0);

    pub fn new(resolution: Option<f64>, acceleration: Option<f64>) -> Self {
        Self {
            resolution: resolution.map(|value| clamp(value, Self::RESOLUTION)),
            acceleration: acceleration.map(|value| clamp(value, Self::ACCELERATION)),
        }
    }

    pub fn resolution(&self) -> Option<f64> {
        self.resolution
    }

    pub fn acceleration(&self) -> Option<f64> {
        self.acceleration
    }

    pub fn is_empty(&self) -> bool {
        self.resolution.is_none() && self.acceleration.is_none()
    }
}

fn clamp(value: f64, (low, high): (f64, f64)) -> f64 {
    value.max(low).min(high)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Tuning {
    /// Whether to turn the vertical wheel over.
    ///
    /// macOS has one switch for every device, so a device relayed through favjit
    /// needs its own.
    pub invert_vertical_wheel: bool,

    /// The same for the horizontal one.
    ///
    /// A field of its own rather than sharing the vertical one's: a device can be
    /// upside down in one axis and not the other, and a single flag would make that
    /// unreachable.
    pub invert_horizontal_wheel: bool,
}

impl Tuning {
    /// Apply it to one report.
    pub fn apply(&self, report: PointerReport) -> PointerReport {
        PointerReport {
            dx: report.dx,
            dy: report.dy,
            vertical_wheel: if self.invert_vertical_wheel {
                -report.vertical_wheel
            } else {
                report.vertical_wheel
            },
            horizontal_wheel: if self.invert_horizontal_wheel {
                -report.horizontal_wheel
            } else {
                report.horizontal_wheel
            },
            buttons: report.buttons,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buttons_are_one_based() {
        let held = Buttons::NONE.with(1).with(3);
        assert!(held.holds(1));
        assert!(!held.holds(2));
        assert!(held.holds(3));
        assert_eq!(held.bits(), 0b101);
    }

    #[test]
    fn clearing_a_button_leaves_the_others() {
        let held = Buttons::NONE.with(1).with(2).without(1);
        assert!(!held.holds(1));
        assert!(held.holds(2));
        assert!(held.any());
    }

    #[test]
    fn a_value_outside_the_bounds_is_brought_into_them() {
        let low = Wanted::new(Some(1.0), Some(-1.0));
        assert_eq!(low.resolution(), Some(10.0));
        assert_eq!(low.acceleration(), Some(0.0));

        let high = Wanted::new(Some(3000.0), None);
        assert_eq!(high.resolution(), Some(1995.0));
        assert_eq!(high.acceleration(), None, "and nothing is invented");

        assert_eq!(Wanted::new(Some(50.0), None).resolution(), Some(50.0));
        assert!(Wanted::new(None, None).is_empty());
    }

    #[test]
    fn a_report_with_no_motion_is_still_even_while_a_button_is_held() {
        let report = PointerReport {
            buttons: Buttons::NONE.with(1),
            ..PointerReport::default()
        };
        assert!(report.is_still());
    }
}
