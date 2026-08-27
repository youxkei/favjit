//! Arithmetic over the instants a host reports.
//!
//! An extension trait rather than methods on the type, because the type is the
//! boundary's and the boundary holds no code (ADR-0005). A host reports a number
//! of nanoseconds; comparing two of them, or moving one on by a duration, is a
//! step in a run and belongs where the suite drives it.

use core::time::Duration;

use favjit_host::Instant;

pub(crate) trait Clock {
    fn from_nanos(nanos: u64) -> Self;

    fn as_nanos(&self) -> u64;

    /// Saturates rather than wrapping: a clock that has run long enough to
    /// overflow should stop advancing deadlines, not move them backwards.
    fn saturating_add(self, duration: Duration) -> Self;

    /// How long this is after `earlier`, and nothing when it is not after it.
    ///
    /// Saturating for the reason above and one more: two stamps that arrive out of
    /// order should read as no time passing rather than as an enormous interval,
    /// since what reads them is deciding whether something has been silent for too
    /// long.
    fn saturating_duration_since(self, earlier: Self) -> Duration;
}

impl Clock for Instant {
    fn from_nanos(nanos: u64) -> Self {
        Self { nanos }
    }

    fn as_nanos(&self) -> u64 {
        self.nanos
    }

    fn saturating_add(self, duration: Duration) -> Self {
        let nanos = u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX);
        Self {
            nanos: self.nanos.saturating_add(nanos),
        }
    }

    fn saturating_duration_since(self, earlier: Self) -> Duration {
        Duration::from_nanos(self.nanos.saturating_sub(earlier.nanos))
    }
}
