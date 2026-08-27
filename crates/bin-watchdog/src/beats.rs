//! The heartbeat channel and the clock, which are the same on every platform.
//!
//! **The heartbeats are read on a thread of their own**, so that the bounded wait
//! ADR-0006 asks of a host is `recv_timeout` and not a per-platform poll. Two
//! platforms would otherwise need two ways to wait on a pipe with a deadline, and
//! that is the kind of thing this program should not contain two of.
//!
//! A thread inside the supervisor is not the arrangement ADR-0008 rejects. What it
//! rejects is supervising a loop from inside the process that holds it, which shares
//! the fate of whatever wedged. This thread's failure stops the heartbeats, and a
//! stopped heartbeat ends the child — which errs towards giving the keyboard back.

use core::time::Duration;
use std::io::Read;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};

use favjit_core::Instant;

/// What a wait came back with.
pub enum Arrival {
    Heartbeat,
    Silence,
}

/// Where the heartbeats arrive.
pub struct Beats {
    arrivals: Receiver<()>,
    /// Whether the reading thread has ended, which is what the far end closing looks
    /// like from here.
    closed: bool,
}

impl Beats {
    /// Read the pipe on a thread, forwarding one arrival per read.
    ///
    /// The bytes are not looked at. What a heartbeat says is that the loop came back
    /// round, and a supervisor that parsed them would be a supervisor with an
    /// opinion about its child's internals.
    pub fn read(mut pipe: std::fs::File) -> Self {
        let (arrivals, receiver) = mpsc::channel();
        std::thread::Builder::new()
            .name(String::from("favjit-heartbeats"))
            .spawn(move || {
                let mut buffer = [0u8; 256];
                loop {
                    match pipe.read(&mut buffer) {
                        // Nothing more is coming: the child has closed its end, or
                        // it is gone.
                        Ok(0) => return,
                        Ok(_) => {
                            if arrivals.send(()).is_err() {
                                return;
                            }
                        }
                        // A read that failed for any other reason is one this thread
                        // cannot recover from either, and its ending is what the
                        // judgement reads as silence.
                        Err(_) => return,
                    }
                }
            })
            // A thread that will not start is a supervisor that can never see a
            // heartbeat, which the judgement turns into ending the child. That is
            // the safe direction, and it is loud.
            .ok();
        Self {
            arrivals: receiver,
            closed: false,
        }
    }

    /// Wait up to `patience` for a heartbeat.
    pub fn next(&mut self, patience: Duration) -> Arrival {
        if self.closed {
            // A gone channel answers immediately, and the wait still has to take the
            // time it was given: without this the judgement would spin through the
            // rest of its bound rather than wait it out.
            std::thread::sleep(patience);
            return Arrival::Silence;
        }
        match self.arrivals.recv_timeout(patience) {
            Ok(()) => Arrival::Heartbeat,
            Err(RecvTimeoutError::Timeout) => Arrival::Silence,
            Err(RecvTimeoutError::Disconnected) => {
                self.closed = true;
                std::thread::sleep(patience);
                Arrival::Silence
            }
        }
    }
}

/// The clock every moment this program reports is on.
///
/// One base, taken when the child starts, so that every `Instant` the judgement
/// compares is measured from the same origin. `core` reads no clock of its own, which
/// is what makes the judgement drivable from a test (ADR-0006).
pub struct Clock {
    base: std::time::Instant,
}

impl Clock {
    pub fn start() -> Self {
        Self {
            base: std::time::Instant::now(),
        }
    }

    pub fn now(&self) -> Instant {
        Instant::from_nanos(u64::try_from(self.base.elapsed().as_nanos()).unwrap_or(u64::MAX))
    }
}
