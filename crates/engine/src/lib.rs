//! favjit's platform-independent half.
//!
//! Everything here is platform-independent: no threads, no clock, no network or
//! filesystem calls. What a machine does is reached only through the traits this
//! crate states — [`Host`] and, beside each role's loop, that role's own (ADR-0006).
//! That is what makes the
//! end-to-end suite deterministic and a recorded trace replayable.

mod clock;
pub mod control;
mod device;
pub mod discovery;

/// What a HID device is told and understood in. Not behind a feature: `engine` naming
/// a platform in a `cfg` is what ADR-0005 rules out, and this is a table.
///
/// `pub(crate)`, not `pub`: nothing here appears in an entry point's own signature —
/// a report's bytes are what crosses the host boundary, never the `Report` that
/// built them — so a caller reading a page, a usage or a report belongs on
/// `favjit-hid` directly, the crate this table actually lives in.
pub(crate) mod hid;

mod host;
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
///
/// `pub(crate)`: driving the construction is [`pairing`] and [`link`]'s own, not
/// an entry point's — a caller standing in for a machine's own source of
/// randomness reaches `favjit-noise` directly, the same crate this is a
/// passthrough of.
pub(crate) mod noise;

/// What a watchdog and the run it supervises agree on. Not behind a role feature or
/// the watchdog's: both ends of the agreement need it, and only one is a watchdog.
pub mod supervision;

/// What the device converted keystrokes go out through is asked for, and what
/// its answers mean. Behind the sink's feature because only a converting run
/// has an output at all.
/// The loop that reads this machine's own keyboards, which a host turns.
///
/// `pub` because a host is what starts the thread and hands the loop over, and
/// the two binaries are what put the two together (ADR-0006).
pub mod capture;

#[cfg(feature = "sink")]
mod output;

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

pub use device::{DeviceInfo, DeviceMatch, Scope};
pub use favjit_hid::{Buttons, Key, PointerReport};
pub use favjit_host::{DeviceId, Ended, Instant};
pub use host::{EventKind, HostEvent, Injected};
pub use layout::{Action, FromMods, Layer, Layout, Optional, Outcome, Rule};
pub use modifiers::{ModifierKeys, Modifiers};
