//! What a watchdog and the run it supervises agree on (ADR-0008).
//!
//! Two programs rather than two machines, and the failure is the same shape as the
//! link's: a name spelled differently at one end is not an error but a probe nobody
//! reads and a heartbeat nobody sends, which reads as a wedge and ends a working
//! process. So it is stated here, once, for the reason [`crate::link`]'s constants
//! are ([ADR-0006](../../../docs/adr/0006-host-boundary.md)).
//!
//! Not behind the `watchdog` feature: both ends of the agreement need it, and only
//! one of them is a watchdog.

/// The environment a watchdog hands its child, and the child reads back.
///
/// A number in each, and what the number is belongs to the platform: a file
/// descriptor on Unix and a handle on Windows. Either way it is what a child needs in
/// order to reach a pipe its parent already holds, which is why one name covers both.
///
/// The environment and not an argument, because a child cannot be told which numbers
/// to expect any other way — and because favjit's own arguments are a person's to
/// write.
pub const PROBE: &str = "FAVJIT_PROBE_FD";
pub const HEARTBEAT: &str = "FAVJIT_HEARTBEAT_FD";

/// Where the trace is written, for the run to map (ADR-0009).
pub const TRACE: &str = "FAVJIT_TRACE_FD";

/// How large that region is.
///
/// Agreed rather than negotiated: the watchdog makes it and the run maps it, and a
/// run that mapped a different length would read records off the end of what was
/// made.
pub const TRACE_BYTES: usize = 1024 * 1024;
