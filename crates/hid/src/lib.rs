//! What a page and a usage mean, and the reports a HID device is told with —
//! the vocabulary the two machines' `engine`s already have to agree on with
//! each other, not with a host (ADR-0005).
//!
//! A platform host has no use for any of it: a page and a usage stay numbers
//! IOKit or raw input handed over, and naming them is the entry point's to do,
//! reached from `engine`. What lives here is reachable from `engine` and from
//! `host-sim` alike, because standing in for the machine on the other end of a
//! keyboard means running this table for real, not a second copy of it
//! (ADR-0005, ADR-0007).
//!
//! The numbers are `kHIDUsage_*` from the SDK's `IOHIDUsageTables.h`, except
//! Apple's own pages, which that header does not name. Those are read off
//! `pqrs/hid/usage_page.hpp` and `pqrs/hid/usage.hpp` in Karabiner's vendored
//! dependencies, which is also where the pairing of a control to its usage comes
//! from.

mod key;
mod pointer;
pub mod rawmouse;
pub mod report;
pub mod scancode;
pub mod usage;

pub use key::Key;
pub use pointer::{Buttons, PointerReport, Wanted};

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
