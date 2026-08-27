//! The boundary between `core` and the machine it runs on (ADR-0006).

use crate::{DeviceId, DeviceInfo, Instant, Key, ModifierKeys, PointerReport};

/// Something that happened outside `core`, and when.
///
/// One stream, not one per concern: ADR-0006 makes each role a single loop, and
/// that is what keeps the end-to-end suite deterministic.
///
/// The timestamp rides on the event rather than being read from a clock when
/// wanted. That is what lets a rule distinguish a tap from a hold while `core`
/// stays a function of its event stream alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HostEvent {
    pub at: Instant,
    pub kind: EventKind,
}

impl HostEvent {
    pub const fn new(at: Instant, kind: EventKind) -> Self {
        Self { at, kind }
    }
}

/// What happened.
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
    /// One pointer report from a device that also has a keyboard.
    ///
    /// It arrives on this stream rather than on one of its own because a seize is
    /// per device: suppressing the TrackPoint keyboard's keys takes its pointer
    /// with it, so the pointer has to be relayed by whatever is holding the
    /// keyboard, and a second loop for it would be concurrency inside `core`
    /// (ADR-0006).
    Pointer {
        device: DeviceId,
        report: PointerReport,
    },
    /// The wake-up [`Host::set_timer`] asked for has come due.
    ///
    /// Carries no payload: the loop holds whatever it was waiting to do, and a
    /// timer that named its own purpose would let two of them disagree about
    /// which one is outstanding.
    Timer,
    /// The supervising watchdog asking whether the loop is still turning
    /// (ADR-0008).
    ///
    /// A kind of its own rather than a reserved key, so that nothing arriving on
    /// this path can become an injected keystroke however the tables are
    /// written. The loop needs to do nothing with it: reaching the next
    /// [`Host::heartbeat`] is the answer.
    Probe,
}

/// Why a machine has no more events to give.
///
/// Facts the platform observed, not what to do about them: every one of these ends
/// the run, and which it was decides only what is said about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Ended {
    /// The run was asked to stop, or the bound it was given has passed.
    AsAsked,
    /// Converting was switched off.
    SwitchedOff,
    /// The device converted keystrokes go out through has gone.
    OutputGone,
    /// The loop the run handed over to turn alongside its own has come back.
    ///
    /// Reported after everything that loop put into the stream, because those
    /// events happened: a run that dropped them on its way out would lose the last
    /// keystrokes the other machine sent.
    AlongsideStopped,
}

/// One thing `core` asks the host to tell the OS.
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

/// What every role needs of the machine it runs on, whatever it does with the
/// input.
///
/// The stream events arrive on, and the two ways a process reports outwards. Each
/// role's own boundary builds on this one ([`crate::sink::SinkInputHost`],
/// [`crate::source::SourceHost`]), and a run given one of those cannot reach past
/// it — which is how it is said, in the types, that [`crate::sink::watch`] produces
/// no keystroke.
///
/// Not `take_input`, which both roles have and which is not one operation: the sink
/// seizes the keyboards as it asks, and the source only becomes *able* to refuse
/// them, later and per event. Not `ended` either — the two roles end for different
/// reasons, and the enums naming them share one variant out of five.
///
/// Implemented once per platform, plus a simulated one for the end-to-end suite.
/// Every one of these is one call into the platform and the answer it gave. A host
/// that decided anything would be deciding it where nothing can drive it
/// (ADR-0006).
pub trait Host {
    /// Block until the next thing happens outside, or return `None` once
    /// nothing more will.
    fn next_event(&mut self) -> Option<HostEvent>;

    /// Whether something is watching this process (ADR-0008).
    ///
    /// Asked rather than assumed, because it decides what a run that holds the
    /// keyboards is: supervised, a wedge ends the process and gives them back;
    /// unsupervised, nothing does. What follows from the answer is `core`'s, which
    /// is why it is a question here and not a warning on the machine's side.
    fn is_supervised(&mut self) -> bool;

    /// Put a line where this machine's log goes.
    ///
    /// The wording is `core`'s because the thing being reported is `core`'s
    /// conclusion, and the writing is the machine's because `core` reaches nothing
    /// outside itself. A front-end that has to remember to say it is one that can
    /// forget, so nothing here is left to a caller of a role's loop.
    ///
    /// [`core::fmt::Arguments`] rather than a formatted string, so that a line
    /// nobody is listening for costs no allocation, and rather than a value per
    /// thing that can be said, because a machine has nothing to do with such a
    /// value but format it.
    ///
    /// Must not block, for the same reason as the rest of this surface.
    fn warn(&mut self, message: core::fmt::Arguments);

    /// Tell the supervisor the loop came back round (ADR-0008).
    ///
    /// Called by the role loop itself, once per event handled. That is the whole
    /// point: a heartbeat from a thread of its own would keep reporting health
    /// while the loop it is meant to be vouching for sits wedged. Paired with
    /// [`EventKind::Probe`] it also catches a loop that is turning but no longer
    /// receiving, since a probe that goes in and produces no heartbeat is
    /// indistinguishable from a stopped loop — which is the correct verdict.
    ///
    /// Must not block. A supervisor that has gone away must not be able to stop
    /// the process that is holding the keyboard.
    fn heartbeat(&mut self);
}
