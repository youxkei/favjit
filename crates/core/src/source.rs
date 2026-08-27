//! The source role: everything the Windows machine's input has to do (ADR-0002).
//!
//! It converts nothing. There is one conversion pipeline and it is the sink's
//! (ADR-0003), so this end observes and relays — which is also what keeps the two
//! machines from disagreeing about what a keystroke meant.
//!
//! What it does decide is when to take the keyboards. Only while there is
//! somewhere to send them: a source suppressing input it cannot relay is "the
//! keyboard stopped working" on the machine the person is typing on, which is the
//! one outcome ADR-0008 rules out.

use crate::link::Message;
use crate::HostEvent;

/// The boundary a source runs against.
///
/// Its own trait rather than [`crate::Host`]: a source injects nothing and a sink
/// sends nothing, and one trait covering both would give each an operation it must
/// never call. Every operation here is a call into the platform and the answer it
/// gave; what to do about the answer is [`run`]'s.
pub trait SourceHost {
    /// Find the other machine and open a session with it.
    ///
    /// Blocks while it looks, the way the sink's accept blocks: a caller that had
    /// to poll would need a clock, and `core` has none.
    fn connect(&mut self) -> Connected;

    /// Take the keyboards, or give them back.
    ///
    /// Separate from connecting because when it happens is a decision: the keys
    /// are only taken while there is a link to relay them over.
    fn suppress(&mut self, taking: bool);

    /// Block until the next thing happens outside, or return `None` once nothing
    /// more will.
    fn next_event(&mut self) -> Option<HostEvent>;

    /// Hand a message to the peer.
    ///
    /// `false` once the link has gone. Reported rather than acted on, because what
    /// to do about it is a decision — and this one is the difference between
    /// stopping and reading keyboards nobody will hear.
    ///
    /// Must not block indefinitely, for the reason the sink's injection must not:
    /// a single loop has no other thread to make progress on, and a source wedged
    /// on a socket is a keyboard that has stopped (ADR-0008).
    fn send(&mut self, message: Message) -> bool;

    /// Tell the supervisor the loop came back round (ADR-0008).
    ///
    /// Per machine: the process holding the Windows keyboards has its own
    /// watchdog, and the sink's heartbeat says nothing about this one.
    fn heartbeat(&mut self);
}

/// What trying to reach the other machine produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Connected {
    /// There is a session; input can be relayed.
    Ready,
    /// The other machine is not there — asleep, rebooting, or not on this network.
    ///
    /// A kind of its own rather than an error, because it is the ordinary state of
    /// a machine that has not been switched on yet.
    NotFound,
    /// Nothing more will work: there is no identity, or no keyboards to take.
    Done,
}

/// The source role, start to finish.
///
/// One loop over one event stream, the shape ADR-0006 asks of every role — with
/// the link either up or being waited for, and the keyboards taken only in the
/// first of those.
pub fn run(host: &mut dyn SourceHost) {
    loop {
        // Not suppressing while there is no link, so the machine in front of the
        // person keeps working: this is the whole of what makes a source safe to
        // leave running when the other machine is off.
        host.suppress(false);
        match host.connect() {
            Connected::Ready => {}
            // Round again. `connect` is what waits, so this is not a spin — and
            // the other machine coming back is the ordinary case, not an error.
            Connected::NotFound => continue,
            Connected::Done => return,
        }
        host.suppress(true);

        // Which of the two ways out of the loop below happened, because they are
        // not the same thing: a link that dropped is one to wait for again, and a
        // stream that ended is this process finishing.
        let mut relaying = true;
        while let Some(event) = host.next_event() {
            if let Some(message) = Message::of(event.kind) {
                if !host.send(message) {
                    // Back to waiting rather than stopping: the other machine
                    // rebooting is a link that comes back, and giving the keys up
                    // in the meantime is what the person needs.
                    relaying = false;
                    break;
                }
            }
            // After handling, not before: a heartbeat sent on the way in would
            // vouch for a loop that is about to wedge inside `send`.
            host.heartbeat();
        }

        if relaying {
            host.suppress(false);
            return;
        }
    }
}
