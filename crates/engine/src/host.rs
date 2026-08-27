//! An input event, resolved to what it means, and what a role's loop decides to
//! tell the OS in response.
//!
//! [`favjit_host::HostEvent`] and [`favjit_host::EventKind`] are what a platform
//! reports — a page and a usage, a scancode, a plaintext frame not yet decoded.
//! [`HostEvent`] and [`EventKind`] here are what that resolves to: naming either
//! raw signal as a [`Key`] is a table, and a table is `engine`'s (ADR-0006), so
//! this is the shape a role's loop dispatches on once that table has run, the
//! same shape [`crate::link::Message`] carries — a keystroke read here and one
//! relayed from the other machine are the same fact by the time either is
//! converted.

use favjit_host::{DeviceId, Instant};

use crate::device::DeviceInfo;

use crate::{Key, ModifierKeys, PointerReport};

/// A resolved event, and when it happened.
///
/// The timestamp rides on the event rather than being read from a clock when
/// wanted, which is what lets a rule distinguish a tap from a hold while a role's
/// loop stays a function of its event stream alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HostEvent {
    pub at: Instant,
    pub kind: EventKind,
}

impl HostEvent {
    pub(crate) const fn new(at: Instant, kind: EventKind) -> Self {
        Self { at, kind }
    }
}

/// What a resolved event says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum EventKind {
    DeviceAttached(DeviceInfo),
    DeviceDetached(DeviceId),
    KeyDown {
        device: DeviceId,
        key: Key,
    },
    KeyUp {
        device: DeviceId,
        key: Key,
    },
    Pointer {
        device: DeviceId,
        report: PointerReport,
    },
    /// The wake-up a role's loop asked for has come due.
    Timer,
    /// The supervising watchdog asking whether the loop is still turning
    /// (ADR-0008).
    Probe,
    /// Somebody has asked for the keyboard to be driving this machine or the other
    /// one, by some means other than the chord (ADR-0013).
    ///
    /// On this stream and not a call the loop makes, because the loop's one wait is
    /// the stream itself: an ask that had to be polled for would be seen on the next
    /// keystroke, and having no usable keyboard is when it is asked.
    Asked(favjit_host::source::Driving),
}

/// One thing a role's loop decides to tell the OS.
///
/// **`modifiers` is the whole set of modifier keys that must be down for this
/// event, and a host delivers exactly that set — no more and no less.** A host
/// with modifier state of its own to add back would be deciding what reaches
/// applications where nothing can drive it (ADR-0006): a rule's mandatory modifier
/// is consumed *here*, so the shift that selected `'` out of a layer is already
/// gone from the set, and putting it back delivers `"`.
///
/// For a modifier key the set includes that key, since the set is what its report
/// is made of; the key and the set are not two separate claims.
///
/// A `KeyUp` of an ordinary key carries the set recorded when the matching
/// `KeyDown` was injected, so a shifted character is released as shifted;
/// [`Injected::Modifiers`] follows it where what is still held differs. A `KeyUp`
/// of a modifier key carries what is left instead — the set recorded at press time
/// can name a key another finger has since let go of, and asserting it again is the
/// stuck modifier ADR-0002 puts on the sink.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Injected {
    KeyDown {
        key: Key,
        modifiers: ModifierKeys,
    },
    KeyUp {
        key: Key,
        modifiers: ModifierKeys,
    },
    /// The modifier keys down at the OS, changed with no key of its own.
    ///
    /// What lets go of a modifier a keystroke borrowed — the shift inside
    /// Dudrack's `:` — once the key that borrowed it is up, and what puts a
    /// consumed one back while the finger is still on it. Its own event rather
    /// than something a host works out after a release: which of those two it is
    /// depends on what the sink still holds, and that is the sink's to know.
    Modifiers(ModifierKeys),
    /// A pointer report to deliver as it stands.
    ///
    /// Carried through rather than accumulated, because the acceleration the OS
    /// applies is per report: coalescing two of them into one, or splitting one
    /// into two, changes how far the cursor travels for the same movement of the
    /// user's thumb.
    Pointer(PointerReport),
}
