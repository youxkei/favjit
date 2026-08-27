//! HID as far as favjit speaks it: what a page and a usage mean, and the reports a
//! device is told with.
//!
//! Here rather than beside the macOS calls that use it, because a host is only what
//! the end-to-end suite cannot drive — the OS calls, the sockets, the run loops
//! (ADR-0006). None of this is one of those. A usage number is a number and a report
//! is bytes, and both can be pressed against the layout by a test; what *reads and
//! writes* them is macOS's and lives there.
//!
//! Two of the four hosts have no use for any of it — Windows speaks scancodes to
//! `SendInput`, and the simulated machine speaks nothing at all. It is compiled all
//! the same: `core` naming a platform in a `cfg` is what ADR-0005 rules out, and a
//! table nobody calls costs a table.
//!
//! The numbers are `kHIDUsage_*` from the SDK's `IOHIDUsageTables.h`, except
//! Apple's own pages, which that header does not name. Those are read off
//! `pqrs/hid/usage_page.hpp` and `pqrs/hid/usage.hpp` in Karabiner's vendored
//! dependencies, which is also where the pairing of a control to its usage comes
//! from.

pub mod report;
mod tables;
pub mod usage;

/// The pages favjit reads keys, controls and pointers from.
pub mod page {
    pub const GENERIC_DESKTOP: u32 = 0x01;
    pub const KEYBOARD_OR_KEYPAD: u32 = 0x07;
    /// Where a pointer's buttons are, numbered from one.
    pub const BUTTON: u32 = 0x09;
    /// Where the volume, brightness and media controls are.
    pub const CONSUMER: u32 = 0x0C;
    /// Apple's own two, which the SDK's tables do not name.
    pub const APPLE_VENDOR_TOP_CASE: u32 = 0x00FF;
    pub const APPLE_VENDOR_KEYBOARD: u32 = 0xFF01;
}

/// `Keyboard` on the generic desktop page, which is what a keyboard says it is.
pub const KEYBOARD_COLLECTION: u32 = 0x06;

/// `Fn` on Apple's top-case page, and the one usage of that page the layout names.
pub const KEYBOARD_FN: u32 = 0x0003;
