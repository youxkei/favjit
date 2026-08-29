//! What Windows says a device is, as [`DeviceInfo`].
//!
//! The USB identity comes out of the device's interface path, which is the only
//! place Raw Input puts it for a keyboard: the structure `RIDI_DEVICEINFO` fills
//! in carries a vendor and a product for a device it calls a HID one, and a
//! keyboard gets the keyboard shape of that structure instead — type, subtype,
//! function key count, and nothing that identifies the hardware.

use favjit_core::{DeviceId, DeviceInfo};

/// The vendor and product ids in a device interface path.
///
/// The path is the string `RIDI_DEVICENAME` hands back, of the shape
/// `\\?\HID#VID_17EF&PID_60E1&...#...#{guid}`. Both or neither: a
/// [`favjit_core::DeviceMatch`] needs the pair, so half an identity would be a
/// device that looks identified and matches nothing.
///
/// `None` for the keyboards that are not on a USB bus at all — a laptop's own
/// keyboard is behind `ACPI#PNP0303`, which names a class of device and not a
/// product. That is the case [`DeviceInfo`]'s optional ids exist for.
pub fn identity(path: &str) -> Option<(u16, u16)> {
    let vendor = field(path, "VID_")?;
    let product = field(path, "PID_")?;
    Some((vendor, product))
}

/// The four hex digits after `name` in the path.
///
/// Matched case-insensitively, and only where the four digits are all there: a
/// path that carries `VID_` followed by something else is one this cannot read,
/// and reading three digits plus whatever follows would invent an identity.
fn field(path: &str, name: &str) -> Option<u16> {
    let upper = path.to_ascii_uppercase();
    let at = upper.find(name)? + name.len();
    let digits = upper.get(at..at + 4)?;
    u16::from_str_radix(digits, 16).ok()
}

/// What to announce for a device Windows calls `path`.
///
/// **Never built in**, whatever the path says. `is_built_in` is the sink's word
/// for the Mac's own keyboard, and it selects the layers Dudrack puts on that
/// keyboard; a laptop keyboard on the Windows machine claiming it would take
/// those layers on the wrong hardware. A keyboard forwarded from here is an
/// external one as far as the machine being typed into is concerned, which is
/// what it is.
pub fn info(id: DeviceId, path: &str) -> DeviceInfo {
    match identity(path) {
        Some((vendor, product)) => DeviceInfo::external(id, vendor, product),
        None => DeviceInfo {
            id,
            is_built_in: false,
            vendor_id: None,
            product_id: None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TRACKPOINT: &str = r"\\?\HID#VID_17EF&PID_60E1&MI_01&Col01#8&1e0b8ad9&0&0000#{884b96c3-56ef-11d1-bc8c-00a0c91405dd}";

    #[test]
    fn a_usb_keyboard_is_named_by_its_vendor_and_product() {
        // The pair the sink's configuration matches on: this is how a keyboard
        // forwarded from Windows gets the Dudrack rules rather than the raw-JIS
        // ones.
        assert_eq!(identity(TRACKPOINT), Some((0x17EF, 0x60E1)));
        let info = info(DeviceId(3), TRACKPOINT);
        assert_eq!(info.vendor_id, Some(0x17EF));
        assert_eq!(info.product_id, Some(0x60E1));
    }

    #[test]
    fn lower_case_hex_in_a_path_is_the_same_identity() {
        // Windows has written both, and a match that only read one case would
        // leave the same keyboard identified on one machine and anonymous on
        // another.
        let lower = r"\\?\hid#vid_17ef&pid_60e1#8&1e0b8ad9&0&0000#{884b96c3}";
        assert_eq!(identity(lower), identity(TRACKPOINT));
    }

    #[test]
    fn a_keyboard_that_is_not_on_a_usb_bus_has_no_identity_rather_than_a_made_up_one() {
        // A laptop's own keyboard. Announced anyway — it still converts, it just
        // cannot be singled out by a rule.
        let acpi = r"\\?\ACPI#PNP0303#4&1cf8b0e6&0#{884b96c3-56ef-11d1-bc8c-00a0c91405dd}";
        assert_eq!(identity(acpi), None);
        let info = info(DeviceId(1), acpi);
        assert_eq!(info.vendor_id, None);
        assert_eq!(info.product_id, None);
    }

    #[test]
    fn half_an_identity_is_no_identity() {
        // A path carrying a vendor and no product, or four characters that are
        // not hex. Either would otherwise become a pair with a zero in it, which
        // is a real vendor id on some devices.
        assert_eq!(identity(r"\\?\HID#VID_17EF#8&1e0b8ad9"), None);
        assert_eq!(identity(r"\\?\HID#VID_ZZZZ&PID_60E1#8&1e"), None);
        assert_eq!(identity(r"\\?\HID#VID_17E"), None);
    }

    #[test]
    fn nothing_forwarded_from_here_claims_to_be_the_macs_own_keyboard() {
        // The flag chooses which layers the sink applies, so a Windows keyboard
        // that set it would be converted as the machine's own built-in one.
        for path in [TRACKPOINT, r"\\?\ACPI#PNP0303#4&1cf8b0e6&0#{884b96c3}"] {
            assert!(!info(DeviceId(1), path).is_built_in);
        }
    }
}
