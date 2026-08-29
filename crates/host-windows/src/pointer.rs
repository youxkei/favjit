//! One `RAWMOUSE` → one [`PointerReport`].
//!
//! Two things have to be remembered between reports, and they are the reason
//! this is a type rather than a function. Windows reports a button as the
//! transition that happened to it, where both ends of the relay carry the whole
//! set that is held (see [`favjit_core::Buttons`]), so the set has to be kept
//! here. And a wheel is reported in one-hundred-and-twentieths of a click, where
//! the wheel that comes out the other end turns in clicks, so what is left over
//! has to be kept until the rest of the click arrives.

use favjit_core::{Buttons, PointerReport};

/// What one turn of the wheel is worth in the units Windows reports it in.
///
/// `WHEEL_DELTA`, the number every wheel message is a multiple — or a fraction —
/// of.
const WHEEL_DELTA: i32 = 120;

/// The bits in `RAWMOUSE.usFlags`.
const MOUSE_MOVE_ABSOLUTE: u16 = 0x0001;

/// The bits in `RAWMOUSE.usButtonFlags`.
const LEFT_DOWN: u16 = 0x0001;
const LEFT_UP: u16 = 0x0002;
const RIGHT_DOWN: u16 = 0x0004;
const RIGHT_UP: u16 = 0x0008;
const MIDDLE_DOWN: u16 = 0x0010;
const MIDDLE_UP: u16 = 0x0020;
const FOURTH_DOWN: u16 = 0x0040;
const FOURTH_UP: u16 = 0x0080;
const FIFTH_DOWN: u16 = 0x0100;
const FIFTH_UP: u16 = 0x0200;
const WHEEL: u16 = 0x0400;
const HORIZONTAL_WHEEL: u16 = 0x0800;

/// One `RAWMOUSE`, as the fields this relay reads.
///
/// A shape of its own rather than the structure Windows fills in, so that
/// everything below can be driven from a test: the structure has a union in it
/// and a layout to get right, and neither belongs in the part that decides what a
/// report means.
#[derive(Debug, Clone, Copy, Default)]
pub struct Raw {
    pub flags: u16,
    pub button_flags: u16,
    /// `usButtonData`, which carries the wheel's turn when a wheel bit is set.
    /// Unsigned as Windows declares it; the wheel's direction is in it as two's
    /// complement.
    pub button_data: u16,
    pub dx: i32,
    pub dy: i32,
}

impl Raw {
    /// Whether this report describes where the pointer *is* rather than how far
    /// it moved.
    ///
    /// Tablets, touch digitisers and a mouse arriving over remote desktop report
    /// this way. Nothing here can turn it into a movement: the relay carries a
    /// delta and the absolute position is in whatever coordinate space this
    /// machine's desktop happens to be, which is not the space the other
    /// machine's cursor lives in.
    pub fn is_absolute(&self) -> bool {
        self.flags & MOUSE_MOVE_ABSOLUTE != 0
    }
}

/// One device's pointer, between reports.
#[derive(Debug, Clone, Copy, Default)]
pub struct Pointing {
    buttons: Buttons,
    /// The part of a click that has arrived and not yet been sent on, per axis.
    ///
    /// A high-resolution wheel reports a fraction of `WHEEL_DELTA` per notch, so
    /// dividing each report on its own would round every one of them to nothing
    /// and the wheel would not turn at all.
    wheel: i32,
    horizontal_wheel: i32,
}

impl Pointing {
    /// What this report says, in the vocabulary both ends of the link carry.
    ///
    /// Every report, including one that says nothing new: whether a report is
    /// worth relaying is `core`'s, where the suite can check it, and this is
    /// only the translation.
    pub fn report(&mut self, raw: &Raw) -> PointerReport {
        for (down, up, button) in [
            (LEFT_DOWN, LEFT_UP, 1),
            (RIGHT_DOWN, RIGHT_UP, 2),
            (MIDDLE_DOWN, MIDDLE_UP, 3),
            (FOURTH_DOWN, FOURTH_UP, 4),
            (FIFTH_DOWN, FIFTH_UP, 5),
        ] {
            // Down before up, so a click that went down and came up inside one
            // report ends held-down-then-released rather than the other way
            // round: the release is the transition the other end must not miss.
            if raw.button_flags & down != 0 {
                self.buttons = self.buttons.with(button);
            }
            if raw.button_flags & up != 0 {
                self.buttons = self.buttons.without(button);
            }
        }

        // Signed, though Windows declares the field unsigned: a wheel turned
        // towards the user reports the two's complement of the turn.
        let turn = i32::from(raw.button_data as i16);
        let vertical_wheel = if raw.button_flags & WHEEL != 0 {
            self.wheel += turn;
            take_clicks(&mut self.wheel)
        } else {
            0
        };
        let horizontal_wheel = if raw.button_flags & HORIZONTAL_WHEEL != 0 {
            self.horizontal_wheel += turn;
            take_clicks(&mut self.horizontal_wheel)
        } else {
            0
        };

        let (dx, dy) = if raw.is_absolute() {
            (0, 0)
        } else {
            (raw.dx, raw.dy)
        };
        PointerReport {
            dx,
            dy,
            vertical_wheel,
            horizontal_wheel,
            buttons: self.buttons,
        }
    }
}

/// The whole clicks in what has accumulated, leaving the rest behind.
fn take_clicks(accumulated: &mut i32) -> i32 {
    let clicks = *accumulated / WHEEL_DELTA;
    *accumulated -= clicks * WHEEL_DELTA;
    clicks
}

#[cfg(test)]
mod tests {
    use super::*;

    fn moved(dx: i32, dy: i32) -> Raw {
        Raw {
            dx,
            dy,
            ..Raw::default()
        }
    }

    fn wheel(turn: i16) -> Raw {
        Raw {
            button_flags: WHEEL,
            button_data: turn as u16,
            ..Raw::default()
        }
    }

    #[test]
    fn a_movement_is_carried_as_it_was_reported() {
        // Not scaled and not split: how far the cursor travels is set on the
        // device the other machine delivers through, and the acceleration curve
        // there is applied per report.
        let mut pointing = Pointing::default();
        assert_eq!(pointing.report(&moved(-3, 7)), PointerReport::moved(-3, 7));
    }

    #[test]
    fn a_button_going_down_and_coming_up_are_both_carried() {
        let mut pointing = Pointing::default();
        let press = Raw {
            button_flags: LEFT_DOWN,
            ..Raw::default()
        };
        let release = Raw {
            button_flags: LEFT_UP,
            ..Raw::default()
        };

        assert_eq!(pointing.report(&press).buttons.bits(), 0b1);
        assert_eq!(pointing.report(&release).buttons.bits(), 0);
    }

    #[test]
    fn a_button_held_through_a_movement_stays_held() {
        // Windows reports the transition once, so a report that read the buttons
        // out of it alone would let go of the button on the next movement.
        let mut pointing = Pointing::default();
        pointing.report(&Raw {
            button_flags: RIGHT_DOWN,
            ..Raw::default()
        });

        assert!(pointing.report(&moved(1, 1)).buttons.holds(2));
    }

    #[test]
    fn a_click_inside_one_report_ends_released() {
        // Both transitions in one report. Read in the other order it would end
        // held down, and the button would stay down on the other machine until
        // the next click.
        let mut pointing = Pointing::default();
        let click = Raw {
            button_flags: LEFT_DOWN | LEFT_UP,
            ..Raw::default()
        };
        assert_eq!(pointing.report(&click).buttons.bits(), 0);
    }

    #[test]
    fn the_wheel_turns_in_clicks() {
        let mut pointing = Pointing::default();
        assert_eq!(pointing.report(&wheel(120)).vertical_wheel, 1);
        assert_eq!(pointing.report(&wheel(-240)).vertical_wheel, -2);
    }

    #[test]
    fn a_high_resolution_wheel_turns_once_the_notches_add_up() {
        // Each report is a fraction of a click. Rounding one on its own gives
        // zero every time, which is a wheel that does not turn.
        let mut pointing = Pointing::default();
        assert_eq!(pointing.report(&wheel(40)).vertical_wheel, 0);
        assert_eq!(pointing.report(&wheel(40)).vertical_wheel, 0);
        assert_eq!(pointing.report(&wheel(40)).vertical_wheel, 1);
    }

    #[test]
    fn the_two_wheels_keep_their_own_leftovers() {
        // One accumulator shared between the axes would let a nudge sideways
        // finish a click of the vertical wheel.
        let mut pointing = Pointing::default();
        pointing.report(&wheel(60));
        let sideways = Raw {
            button_flags: HORIZONTAL_WHEEL,
            button_data: 60u16,
            ..Raw::default()
        };
        assert_eq!(pointing.report(&sideways).horizontal_wheel, 0);
        assert_eq!(pointing.report(&wheel(60)).vertical_wheel, 1);
    }

    #[test]
    fn an_absolute_report_carries_its_buttons_and_not_its_position() {
        // The position is in this machine's desktop coordinates, which say
        // nothing about where the other machine's cursor is.
        let mut pointing = Pointing::default();
        let tablet = Raw {
            flags: MOUSE_MOVE_ABSOLUTE,
            button_flags: LEFT_DOWN,
            dx: 12000,
            dy: 8000,
            ..Raw::default()
        };

        let report = pointing.report(&tablet);
        assert_eq!((report.dx, report.dy), (0, 0));
        assert!(report.buttons.holds(1));
    }
}
