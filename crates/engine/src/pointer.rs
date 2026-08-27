//! Tuning the output pointer, and the pointer vocabulary it is applied to.
//!
//! The vocabulary itself — [`favjit_hid::Buttons`], [`favjit_hid::PointerReport`] —
//! is `favjit-hid`'s: both machines' `engine`s and `host-sim` have to agree on it
//! (ADR-0005). What is here is this machine's own: how far the device is told to
//! go for a given motion, and which way its wheels turn.

pub use favjit_hid::{Buttons, PointerReport};
pub use favjit_host::PointerHost;

/// The properties macOS keeps a pointing device's feel in.
///
/// Names and not numbers, because that is what the platform's own reader takes —
/// and here rather than beside it for the reason every other table is here: which
/// property a factor lives in decides whether a write takes effect at all, and a
/// name beside the API that writes it is a decision only a machine could show
/// (ADR-0006).
pub const RESOLUTION: &str = "HIDPointerResolution";
pub const ACCELERATION_TYPE: &str = "HIDPointerAccelerationType";
pub const ACCELERATION: &str = "HIDPointerAcceleration";
pub const MOUSE_ACCELERATION: &str = "HIDMouseAcceleration";
/// The identity properties a pointing device carries, which is how favjit's own
/// is picked out of the machine's list.
pub const VENDOR: &str = "VendorID";
pub const PRODUCT_NAME: &str = "Product";

/// What one pointing device was set to, and what it holds now.
///
/// Returned rather than said, because the two callers say it differently: a run
/// puts it in its log and the diagnostic mode puts it on stdout, and which is
/// which is not this sequence's business (ADR-0006).
#[derive(Debug, Clone, PartialEq)]
pub struct Tuned {
    /// Where in the machine's own list it was.
    pub at: usize,
    pub name: Option<String>,
    /// Which property its factor lives in.
    pub key: &'static str,
    /// The resolution before and after.
    pub resolution: (Option<f64>, Option<f64>),
    /// The factor before and after.
    pub acceleration: (Option<f64>, Option<f64>),
    /// Whether every write was accepted.
    pub accepted: bool,
}

/// Set the feel of the pointer converted input comes out through (ADR-0011).
///
/// Only the device this run opened, found by the vendor id it enumerates with: a
/// run that wrote these to every pointing device the machine has would be
/// changing the person's own mouse.
///
/// Nothing at all where the machine was told no numbers: writing a device's
/// existing values back would be a change to a device favjit was not asked to
/// touch.
pub fn tune(host: &mut dyn PointerHost) -> Vec<Tuned> {
    let (dpi, factor) = host.wanted_pointer_feel();
    if dpi.is_none() && factor.is_none() {
        return Vec::new();
    }
    let mine = host.output_vendor();
    // The full view first and the simpler one only if it will not open: the
    // simpler client cannot write these properties, so settling for it where
    // the other would have opened is a run that reports having tuned a device
    // it did not (ADR-0011).
    let Some(mut view) = host
        .open_event_system()
        .or_else(|| host.open_simple_event_system())
    else {
        return Vec::new();
    };
    let mut tuned = Vec::new();
    // The place in the machine's own list, which means nothing outside it and is
    // what a report names a device by where it has no product name.
    for (at, pointer) in view.look().iter_mut().enumerate() {
        if pointer.integer(VENDOR) != Some(mine) {
            continue;
        }
        // A device with no resolution has nothing here to set, which is how a
        // keyboard in the same list is told from a pointer — the property being
        // there is the same question as whether there is anything to tune.
        let Some(was_dpi) = pointer.fixed(RESOLUTION) else {
            continue;
        };
        let key = acceleration_property(pointer.as_mut());
        let was_factor = pointer.fixed(key);

        let mut accepted = true;
        if let Some(dpi) = dpi {
            accepted &= pointer.set_fixed(RESOLUTION, dpi);
        }
        // Always written, with whatever the device already had when none was
        // asked for: a resolution written on its own does not take effect until
        // an acceleration is written after it.
        if let Some(factor) = factor.or(was_factor) {
            accepted &= pointer.set_fixed(key, factor);
        }

        // Read back rather than reported as asked for: a write that was accepted
        // and ignored is the failure this exists to make visible.
        tuned.push(Tuned {
            at,
            name: pointer.text(PRODUCT_NAME),
            key,
            resolution: (Some(was_dpi), pointer.fixed(RESOLUTION)),
            acceleration: (was_factor, pointer.fixed(key)),
            accepted,
        });
    }
    tuned
}

/// Which property this device keeps its acceleration factor in.
///
/// Asked of the device, because it differs: a mouse and a pointing stick keep
/// their factor under different names, and writing the wrong one is accepted and
/// ignored. Only the two names favjit can read back — anything else falls to the
/// mouse default rather than being trusted, since a name that cannot be verified
/// would fail silently.
fn acceleration_property(pointer: &mut dyn favjit_host::Pointing) -> &'static str {
    if let Some(named) = pointer.text(ACCELERATION_TYPE) {
        if named == ACCELERATION {
            return ACCELERATION;
        }
        if named == MOUSE_ACCELERATION {
            return MOUSE_ACCELERATION;
        }
    }
    match pointer.fixed(ACCELERATION).is_some() {
        true => ACCELERATION,
        false => MOUSE_ACCELERATION,
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
    pub(crate) fn apply(&self, report: PointerReport) -> PointerReport {
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
    fn a_wheel_turned_over_reads_the_other_way() {
        // Both axes on one report, because a device can be upside down in one and
        // not the other and a single flag for both would make that unreachable.
        let tuned = Tuning {
            invert_vertical_wheel: true,
            invert_horizontal_wheel: false,
        };
        let report = PointerReport {
            vertical_wheel: 3,
            horizontal_wheel: -2,
            ..PointerReport::default()
        };

        let out = tuned.apply(report);
        assert_eq!(out.vertical_wheel, -3);
        assert_eq!(out.horizontal_wheel, -2, "the axis left alone is untouched");
    }
}
