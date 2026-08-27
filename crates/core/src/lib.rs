//! favjit's platform-independent half.
//!
//! Everything here is pure: no threads, no clock, no network. What a machine does
//! is reached only through the traits this crate states — [`Host`] and, beside each
//! role's loop, that role's own (ADR-0006). That is what makes the
//! end-to-end suite deterministic and a recorded trace replayable.

mod clock;
mod device;

/// What a HID device is told and understood in. Not behind a feature: `core` naming
/// a platform in a `cfg` is what ADR-0005 rules out, and this is a table.
pub mod hid;

/// Where the file that says favjit is off lives (ADR-0012). Three processes agree on
/// it, so it is written once.
pub mod control;
mod host;
mod key;
mod layout;
mod modifiers;
pub mod pointer;

/// What crosses the link. Not behind a role feature: both ends read the same
/// bytes, and a format compiled into only one of them is a format that can drift.
pub mod link;

/// Who each end will talk to. Not behind a role feature either: a source pins the
/// sink it will send to, and a sink pins the sources it will accept.
pub mod pairing;

/// The session those bytes travel in. Beside the format for the same reason: one
/// construction per platform fails as a record that will not open.
pub mod noise;

/// What a watchdog and the run it supervises agree on. Not behind a role feature or
/// the watchdog's: both ends of the agreement need it, and only one is a watchdog.
pub mod supervision;

#[cfg(feature = "sink")]
pub mod sink;
#[cfg(feature = "source")]
pub mod source;
pub mod trace;

/// What the supervising process decides (ADR-0008). Behind a feature of its own
/// rather than a role's: the watchdog supervises both roles and is neither, and what
/// it links has to stay as little as it can be.
#[cfg(feature = "watchdog")]
pub mod watchdog;

pub use clock::Instant;
pub use device::{DeviceId, DeviceInfo, DeviceMatch, Scope};
pub use host::{Ended, EventKind, Host, HostEvent, Injected};
pub use key::Key;
pub use layout::{Action, FromMods, Layer, Layout, Optional, Outcome, Rule};
pub use modifiers::{ModifierKeys, Modifiers};
pub use pointer::{Buttons, PointerReport};
