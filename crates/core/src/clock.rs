//! Time, as the host reports it.

use core::time::Duration;

/// A point on the host's monotonic clock.
///
/// `core` never reads a clock — there is no `now()` at the host surface.
/// Every timestamp arrives on a [`crate::HostEvent`] instead, because anything
/// `core` reads that is absent from the event stream makes a replay of a
/// recorded trace diverge (ADR-0009).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Instant(u64);

impl Instant {
    pub const ZERO: Self = Self(0);

    pub const fn from_nanos(nanos: u64) -> Self {
        Self(nanos)
    }

    pub const fn as_nanos(self) -> u64 {
        self.0
    }

    /// Saturates rather than wrapping: a clock that has run long enough to
    /// overflow should stop advancing deadlines, not move them backwards.
    pub fn saturating_add(self, duration: Duration) -> Self {
        let nanos = u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX);
        Self(self.0.saturating_add(nanos))
    }

    /// How long this is after `earlier`, and nothing when it is not after it.
    ///
    /// Saturating for the reason above and one more: two stamps that arrive out of
    /// order should read as no time passing rather than as an enormous interval,
    /// since what reads them is deciding whether something has been silent for too
    /// long.
    pub fn saturating_duration_since(self, earlier: Self) -> Duration {
        Duration::from_nanos(self.0.saturating_sub(earlier.0))
    }
}
