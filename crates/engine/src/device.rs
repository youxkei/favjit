//! What a keyboard is once a run has read what its machine said about it, the
//! scopes layout rules are written against, and matching one against a rule that
//! names one.
//!
//! [`crate::DeviceId`] is [`favjit_host`]'s — what a keyboard is called is the
//! machine's, and opaque. Everything else here is read off the properties or the
//! path the machine reported, because each of those readings is a table and a
//! table beside the API that produced its input is a decision no test can drive
//! (ADR-0005, ADR-0006).

use favjit_host::DeviceId;

/// One keyboard, as a run knows it.
///
/// `vendor_id` / `product_id` are optional because a keyboard that reports
/// neither still types: it just cannot be singled out by a rule that names one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceInfo {
    pub id: DeviceId,
    pub is_built_in: bool,
    pub vendor_id: Option<u16>,
    pub product_id: Option<u16>,
}

/// The USB HID usage page and usage a keyboard declares as its primary one.
///
/// The specification's own numbers, fixed by USB HID and not by this project's
/// layout — unlike a page and usage naming one particular key, which is a table
/// this project keeps and grows, these two never change and name a device
/// *class*.
const GENERIC_DESKTOP: i64 = 0x01;
const KEYBOARD_COLLECTION: i64 = 0x06;

/// The keyboard IOKit described with these properties, or nothing where they
/// describe something else.
///
/// **`is_built_in` is read from two signals, either of which is enough**,
/// because the whole layout hangs on this answer and each one alone is a single
/// point of failure. `Transport == "FIFO"` is the SPI bus the internal keyboard
/// sits on, and it is the only signal that works at all for a device with no
/// vendor or product id. The product string is the second: an internal keyboard
/// names itself `Apple Internal …`. Karabiner carries the same pair. See
/// `docs/platform/macos/hid-device-enumeration.md`.
///
/// `None` for a device that is not a keyboard. Read here rather than filtered by
/// the machine, for the reason every other table is: a page a run declines to
/// take is where a key reporting somewhere unexpected would be found, and only
/// this side can be driven to say so.
pub(crate) fn from_hid_properties(
    id: DeviceId,
    primary_usage_page: Option<i64>,
    primary_usage: Option<i64>,
    transport: Option<&str>,
    product: Option<&str>,
    vendor_id: Option<i64>,
    product_id: Option<i64>,
) -> Option<DeviceInfo> {
    if primary_usage_page != Some(GENERIC_DESKTOP) || primary_usage != Some(KEYBOARD_COLLECTION) {
        return None;
    }
    Some(DeviceInfo {
        id,
        is_built_in: transport == Some("FIFO")
            || product.is_some_and(|name| name.starts_with("Apple Internal ")),
        vendor_id: vendor_id.and_then(|n| u16::try_from(n).ok()),
        product_id: product_id.and_then(|n| u16::try_from(n).ok()),
    })
}

/// The keyboard raw input named by this interface path.
///
/// **Never built in**, whatever the path says. `is_built_in` is the sink's word
/// for the Mac's own keyboard, and it selects the layers Dudrack puts on that
/// keyboard; a laptop keyboard on the Windows machine claiming it would take
/// those layers on the wrong hardware. A keyboard forwarded from there is an
/// external one as far as the machine being typed into is concerned, which is
/// what it is.
pub(crate) fn from_a_path(id: DeviceId, path: &str) -> DeviceInfo {
    let (vendor_id, product_id) = match identity(path) {
        Some((vendor, product)) => (Some(vendor), Some(product)),
        None => (None, None),
    };
    DeviceInfo {
        id,
        is_built_in: false,
        vendor_id,
        product_id,
    }
}

/// The vendor and product ids in a device interface path.
///
/// The path is of the shape `\\?\HID#VID_17EF&PID_60E1&...#...#{guid}`. Both or
/// neither: a [`DeviceMatch`] needs the pair, so half an identity would be a
/// device that looks identified and matches nothing.
///
/// `None` for the keyboards that are not on a USB bus at all — a laptop's own
/// keyboard is behind `ACPI#PNP0303`, which names a class of device and not a
/// product. That is the case [`DeviceInfo`]'s optional ids exist for.
///
/// `pub(crate)`: [`crate::source::describe_devices`] is what a listing reads
/// this through, so that it and every other caller singling out a device by
/// its path agree — a second reader beside this one would send somebody
/// looking for a keyboard that is not there.
pub(crate) fn identity(path: &str) -> Option<(u16, u16)> {
    let upper = path.to_ascii_uppercase();
    let vendor = field(&upper, "VID_")?;
    let product = field(&upper, "PID_")?;
    Some((vendor, product))
}

/// The four hex digits after `name` in an upper-cased path.
///
/// Only where the four digits are all there: a path that carries `VID_`
/// followed by something else is one this cannot read, and reading three digits
/// plus whatever follows would invent an identity.
fn field(upper: &str, name: &str) -> Option<u16> {
    let at = upper.find(name)? + name.len();
    let digits = upper.get(at..at + 4)?;
    u16::from_str_radix(digits, 16).ok()
}

/// A keyboard with no vendor or product id, the way the Mac's own reports.
///
/// `cfg(test)`, and not a counterpart to `favjit_host_sim`'s pair: what attaches a
/// keyboard is a host, so nothing outside a unit test in this crate has one of
/// these to build. A run reads what the host reported.
#[cfg(test)]
pub(crate) const fn built_in(id: DeviceId) -> DeviceInfo {
    DeviceInfo {
        id,
        is_built_in: true,
        vendor_id: None,
        product_id: None,
    }
}

/// A keyboard named by USB identity.
#[cfg(test)]
pub(crate) const fn external(id: DeviceId, vendor_id: u16, product_id: u16) -> DeviceInfo {
    DeviceInfo {
        id,
        is_built_in: false,
        vendor_id: Some(vendor_id),
        product_id: Some(product_id),
    }
}

/// One external keyboard, named by USB identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceMatch {
    pub vendor_id: u16,
    pub product_id: u16,
}

impl DeviceMatch {
    pub const fn new(vendor_id: u16, product_id: u16) -> Self {
        Self {
            vendor_id,
            product_id,
        }
    }

    pub(crate) fn matches(&self, info: &DeviceInfo) -> bool {
        info.vendor_id == Some(self.vendor_id) && info.product_id == Some(self.product_id)
    }
}

/// Which keyboards a rule applies to.
///
/// The split is by how a keyboard is *typed*, not by how it is attached. An
/// external keyboard the user types in Dudrack belongs with the built-in one,
/// which is why there is no plain "external" scope: `RawJis` is every external
/// keyboard the configuration has not claimed for Dudrack.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// Every keyboard.
    Any,
    /// The Mac's own keyboard.
    BuiltIn,
    /// Everything typed in Dudrack: the built-in keyboard plus the external
    /// keyboards named in the configuration.
    Dudrack,
    /// The Dudrack-typed external keyboards alone — for the JIS keys the
    /// built-in keyboard does not have.
    DudrackExternal,
    /// External keyboards that keep their JIS labels.
    RawJis,
    /// Whatever arrives over the link, and nothing attached here.
    ///
    /// Its own scope rather than a case of `RawJis`, because a keyboard at the other
    /// machine and a keyboard under the person's other hand are different keyboards
    /// however alike they are printed: one is where the person is typing *from* and the
    /// other is beside where they are typing *into*. What identifies it is the number
    /// every forwarded event carries ([`crate::link::is_from_source`]) rather than a
    /// vendor and product, which is what makes it nameable at all — a hook on the
    /// Windows side says which key and not which keyboard.
    Forwarded,
}

#[cfg(test)]
mod tests {
    use super::*;

    const TRACKPOINT: &str = r"\\?\HID#VID_17EF&PID_60E1&MI_01&Col01#8&1e0b8ad9&0&0000#{884b96c3-56ef-11d1-bc8c-00a0c91405dd}";

    /// A keyboard's own properties, with the two that name a device class
    /// already set to what a keyboard declares.
    fn keyboard(
        transport: Option<&str>,
        product: Option<&str>,
        vendor_id: Option<i64>,
        product_id: Option<i64>,
    ) -> Option<DeviceInfo> {
        from_hid_properties(
            DeviceId(1),
            Some(GENERIC_DESKTOP),
            Some(KEYBOARD_COLLECTION),
            transport,
            product,
            vendor_id,
            product_id,
        )
    }

    #[test]
    fn a_device_that_is_not_a_keyboard_is_not_one_this_run_reads() {
        // Every HID device the machine finds is reported, mice included, so the
        // answer to which of them types is here. A mouse taken as a keyboard
        // would be seized, and the pointer would stop working.
        assert_eq!(
            from_hid_properties(
                DeviceId(1),
                Some(GENERIC_DESKTOP),
                Some(0x02),
                Some("USB"),
                None,
                None,
                None
            ),
            None,
            "a mouse"
        );
        assert_eq!(
            from_hid_properties(DeviceId(1), None, None, None, None, None, None),
            None,
            "a device that declares neither"
        );
    }

    #[test]
    fn either_signal_alone_says_the_keyboard_is_the_macs_own() {
        // Two signals because the whole layout hangs on this answer: the bus is
        // the only one that works for a device with no vendor or product, and
        // the product string is the only one that survives a machine whose
        // internal keyboard is not on that bus.
        assert!(
            keyboard(Some("FIFO"), None, None, None)
                .expect("a keyboard")
                .is_built_in
        );
        assert!(
            keyboard(
                Some("USB"),
                Some("Apple Internal Keyboard / Trackpad"),
                None,
                None
            )
            .expect("a keyboard")
            .is_built_in
        );
        assert!(
            !keyboard(
                Some("USB"),
                Some("Lenovo TrackPoint Keyboard II"),
                None,
                None
            )
            .expect("a keyboard")
            .is_built_in
        );
    }

    #[test]
    fn a_vendor_id_too_large_to_be_one_is_no_identity_rather_than_a_wrapped_one() {
        // IOKit holds these as numbers with room for more than a USB id has: a
        // value past that truncated would be a rule matching a keyboard that is
        // not the one it names.
        let info = keyboard(Some("USB"), None, Some(0x1_0000), Some(0x60E1)).expect("a keyboard");
        assert_eq!(info.vendor_id, None);
        assert_eq!(info.product_id, Some(0x60E1));
    }

    #[test]
    fn a_usb_keyboard_is_named_by_its_vendor_and_product_in_its_path() {
        // The pair the sink's configuration matches on: this is how a keyboard
        // forwarded from Windows gets the Dudrack rules rather than the raw-JIS
        // ones.
        let info = from_a_path(DeviceId(3), TRACKPOINT);
        assert_eq!(info.vendor_id, Some(0x17EF));
        assert_eq!(info.product_id, Some(0x60E1));
    }

    #[test]
    fn lower_case_hex_in_a_path_is_the_same_identity() {
        // Windows has written both, and a match that only read one case would
        // leave the same keyboard identified on one machine and anonymous on
        // another.
        let lower = r"\\?\hid#vid_17ef&pid_60e1#8&1e0b8ad9&0&0000#{884b96c3}";
        assert_eq!(
            from_a_path(DeviceId(3), lower),
            from_a_path(DeviceId(3), TRACKPOINT)
        );
    }

    #[test]
    fn a_keyboard_that_is_not_on_a_usb_bus_has_no_identity_rather_than_a_made_up_one() {
        // A laptop's own keyboard, behind `ACPI#PNP0303`. Announced anyway — it
        // still converts, it just cannot be singled out by a rule.
        let acpi = r"\\?\ACPI#PNP0303#4&1cf8b0e6&0#{884b96c3-56ef-11d1-bc8c-00a0c91405dd}";
        let info = from_a_path(DeviceId(1), acpi);
        assert_eq!(info.vendor_id, None);
        assert_eq!(info.product_id, None);
    }

    #[test]
    fn half_an_identity_in_a_path_is_no_identity() {
        // A path carrying a vendor and no product, or four characters that are
        // not hex. Either would otherwise become a pair with a zero in it, which
        // is a real vendor id on some devices.
        for path in [
            r"\\?\HID#VID_17EF#8&1e0b8ad9",
            r"\\?\HID#VID_ZZZZ&PID_60E1#8&1e",
            r"\\?\HID#VID_17E",
        ] {
            assert_eq!(from_a_path(DeviceId(1), path).vendor_id, None, "{path}");
        }
    }

    #[test]
    fn nothing_named_by_a_path_claims_to_be_the_macs_own_keyboard() {
        // The flag chooses which layers the sink applies, so a Windows keyboard
        // that set it would be converted as the machine's own built-in one.
        for path in [TRACKPOINT, r"\\?\ACPI#PNP0303#4&1cf8b0e6&0#{884b96c3}"] {
            assert!(!from_a_path(DeviceId(1), path).is_built_in);
        }
    }
}
