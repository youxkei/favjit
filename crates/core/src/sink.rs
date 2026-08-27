//! The sink role: one run, one loop, one pipeline (ADR-0006).
//!
//! Everything a run of the converter does is here — bringing the machine up in the
//! order ADR-0008 requires, then converting what arrives from this machine's own
//! keyboards and from the other machine's, through the one pipeline both pass
//! through. What the platform provides is behind [`SinkHost`], one call at a time.

use core::time::Duration;

use crate::link::LinkHost;
use crate::pairing::{self, Identity, IdentityStore};
use crate::pointer::Tuning;
use crate::trace::{Checkpoint, HeldRecord, Record, Trace};
use crate::{
    Buttons, DeviceId, DeviceInfo, Ended, EventKind, Host, HostEvent, Injected, Instant, Key,
    Layout, ModifierKeys, Modifiers, Outcome, PointerReport,
};

/// What this role needs of a machine to bring it up and read its keyboards.
///
/// The sink's own rather than every role's, because the source reads its keyboards
/// on other terms — what it asks for is the *ability* to refuse them, later and per
/// event — and it ends for reasons of its own. What both roles share is [`Host`].
///
/// Split out from [`SinkHost`] so that [`watch`] can be given this and nothing
/// more: a run that exists to find out where a key reports must not be able to
/// produce a keystroke, and a boundary with no `inject` on it is that said in the
/// types rather than in a comment.
pub trait SinkInputHost: Host {
    /// Whether converting is switched on.
    ///
    /// Asked before anything else, because a machine switched off should be asked
    /// for nothing at all.
    fn switched_on(&mut self) -> bool;

    /// Wait for it to be switched on again, and say when it has been.
    ///
    /// A wait rather than an answer this end polls for, because there is nothing
    /// else for the process to do while converting is off — and everything that
    /// could be done instead would be done holding keyboards it should not have.
    fn wait_until_on(&mut self);

    /// Whether this process may read the keyboards at all.
    ///
    /// Asked before anything is brought up, because a machine that cannot read
    /// input has nothing to convert.
    fn may_read_input(&mut self) -> bool;

    /// Start reading this machine's own keyboards, exclusively or not.
    ///
    /// Nothing arrives from [`Host::next_event`] until this has been asked for,
    /// which is what lets the order be decided in `core`: taking a keyboard before
    /// there is anywhere to send its keystrokes is the outcome ADR-0008 rules out.
    fn take_input(&mut self, suppress: bool) -> bool;

    /// Why nothing more will arrive.
    ///
    /// Asked once, after the stream has ended, rather than reported through
    /// [`Host::next_event`]: what a run does about it is the same either way — it
    /// ends — and what differs is only what it says afterwards. A host that decided
    /// that would be deciding what the exit code means (ADR-0006).
    ///
    /// The condition is the waiter's to notice, because a keyboard nobody is typing
    /// on produces no event to notice it on.
    fn ended(&mut self) -> Ended;
}

/// What a run that converts needs of a machine, beyond reading its keyboards.
///
/// Declared here rather than in a host so that the dependency runs one way: `core`
/// says what it needs and each platform implements it, which is what lets the suite
/// hand this role a simulated machine (ADR-0005, ADR-0006).
///
/// A supertrait of [`SinkInputHost`] rather than a second object, because the two
/// halves share state on a real machine: the segment from an event arriving to its
/// keystroke being written is measured across them, and two objects could not see
/// both ends of it. [`IdentityStore`] comes with it for the same reason the run
/// establishes the identity itself: the file and the socket are one machine's.
pub trait SinkHost: SinkInputHost + IdentityStore {
    /// Ask to be woken at `at`, or with `None` cancel whatever was asked for.
    ///
    /// Here rather than beside reading the keyboards, because the only rule that
    /// wants it is the one repeating a held key: a run that converts nothing has
    /// nothing to be woken for.
    ///
    /// One outstanding wake-up, replaced rather than queued. A set of timers would
    /// need identities to cancel one of them, and that one rule has exactly one
    /// deadline outstanding at a time, so identities would be machinery for
    /// nothing.
    ///
    /// The wake-up arrives as [`EventKind::Timer`] on the ordinary stream, so
    /// `core` still reads time only off events and a recorded trace still replays
    /// (ADR-0010).
    ///
    /// Must not block, for the same reason as the rest of this surface.
    fn set_timer(&mut self, at: Option<Instant>);

    /// Bring up the device converted keystrokes go out through.
    fn open_output(&mut self) -> bool;

    /// Set the properties that belong to that device.
    fn tune_output(&mut self);

    /// Open the socket the other machine connects to, presenting this identity
    /// (ADR-0017).
    ///
    /// Handed the identity rather than finding one: which key this machine presents
    /// is what ADR-0004 rests on, and a host that looked it up itself would be
    /// deciding whether a file that is not an identity may be replaced.
    ///
    /// What comes back is the link's side of this machine, for [`crate::link::serve`]
    /// to be run against. Bound and not yet served, because those are two things
    /// that can fail separately and the second one decides the first one's fate.
    fn bind_link(&mut self, identity: &Identity) -> Option<Box<dyn LinkHost + Send>>;

    /// Turn this loop alongside the one calling, saying whether it started.
    ///
    /// The platform's, because a thread is a platform's; what runs on it is
    /// `core`'s, which is what keeps the sequence the link follows in reach of the
    /// suite (ADR-0006). The work owns everything it touches, so nothing is shared
    /// with the loop that started it but the stream the events go into.
    ///
    /// A simulated machine may turn it to a standstill before returning: what a
    /// test can ask is what the two loops did, not which order two schedulers put
    /// them in.
    fn run_alongside(&mut self, work: Box<dyn FnOnce() + Send>) -> bool;

    /// Hand a key event to the OS.
    ///
    /// Must not block indefinitely. A single loop has no other thread to make
    /// progress on, so a stalled injection stalls the loop — and a stalled loop
    /// holding input suppression is what ADR-0008 exists to prevent.
    fn inject(&mut self, injected: Injected);
}

/// How fast a held key repeats.
///
/// Configuration handed in rather than read from the host: the values belong to
/// the machine, and reading them per event would be a read that never appears in
/// the event stream, which is what ADR-0010 keeps out. They sit beside the
/// layout, and a trace that wants to replay needs both recorded the same way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Repeat {
    /// How long a key is held before the first repeat.
    pub initial: Duration,
    /// The gap between repeats after that.
    pub interval: Duration,
}

/// The key currently repeating, and when it is next due.
///
/// The physical key, not the one it produced: the output is looked up again on
/// each repeat, so nothing here can disagree with what is actually down.
#[derive(Debug, Clone, Copy)]
struct Repeating {
    device: DeviceId,
    key: Key,
    due: Instant,
}

/// What became of a physical key that is currently down.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HeldState {
    /// Injected, and down at the OS. The modifier keys are the ones it went down
    /// with, so its release can mirror them.
    Down { key: Key, modifiers: ModifierKeys },
    /// Nothing was injected, and nothing will be.
    Swallowed,
    /// Holding the Henkan layer.
    Henkan,
    /// Still undecided between a modifier and a keystroke.
    Undecided {
        hold: Key,
        tap: Key,
        /// The last moment a release can still count as a tap.
        deadline: Instant,
        /// `Some` once `hold` has actually been injected, carrying the modifier
        /// keys it went down with. While this is `None` the hold is real for rule
        /// matching and invisible to the OS.
        sent: Option<ModifierKeys>,
        /// Whether no other key has gone down since.
        alone: bool,
    },
}

/// A physical key that is currently down.
#[derive(Debug, Clone, Copy)]
struct Held {
    device: DeviceId,
    key: Key,
    state: HeldState,
}

/// Everything a run needs beyond the layout and the host.
///
/// One struct rather than a parameter each, so that adding what the next kind of
/// input needs does not move every call site again.
#[derive(Debug, Clone, Copy, Default)]
pub struct Settings {
    /// Who produces the auto-repeat, and how fast. `None` leaves it to the OS
    /// (ADR-0013).
    pub repeat: Option<Repeat>,
    pub pointer: Tuning,
}

impl From<Option<Repeat>> for Settings {
    fn from(repeat: Option<Repeat>) -> Self {
        Self {
            repeat,
            ..Self::default()
        }
    }
}

/// The sink's whole state: attached keyboards, keys currently down, and the
/// layout. Nothing else — the layer and the modifier set are derived from the
/// held keys rather than tracked alongside them, so they cannot fall out of
/// step with what is physically down.
#[derive(Debug, Clone)]
pub struct Sink {
    layout: Layout,
    devices: Vec<DeviceInfo>,
    held: Vec<Held>,
    pointer: Tuning,
    repeat: Option<Repeat>,
    /// At most one, because that is what a keyboard does: the key pressed last
    /// takes the repeat and the one before it stops, so a set of them would
    /// describe a state no hardware produces.
    repeating: Option<Repeating>,
    /// The pointer buttons the OS was last told about.
    ///
    /// One set for all devices, because there is one cursor: two sets would have
    /// to be merged before anything could be sent, and the merge is this.
    pointer_buttons: Buttons,
}

impl Sink {
    pub fn new(layout: Layout, settings: Settings) -> Self {
        Self {
            layout,
            devices: Vec::new(),
            held: Vec::new(),
            pointer: settings.pointer,
            repeat: settings.repeat,
            repeating: None,
            pointer_buttons: Buttons::NONE,
        }
    }

    /// The sink's whole state, as the records a trace stores it in.
    ///
    /// Everything the next event could be answered differently because of, and
    /// nothing else: the layout and the repeat rates are configuration the replay
    /// is given, not state, and recording them would let a trace replay against a
    /// layout that was never in force.
    fn checkpoint_records(&self) -> Vec<Record> {
        let mut records = Vec::with_capacity(1 + self.devices.len() + self.held.len());
        records.push(Record::CheckpointBegin {
            pointer_buttons: self.pointer_buttons,
            repeating: self.repeating.map(|r| (r.device, r.key, r.due)),
        });
        for info in &self.devices {
            records.push(Record::CheckpointDevice(*info));
        }
        for held in &self.held {
            records.push(Record::CheckpointHeld {
                device: held.device,
                key: held.key,
                state: match held.state {
                    HeldState::Down { key, modifiers } => HeldRecord::Down { key, modifiers },
                    HeldState::Swallowed => HeldRecord::Swallowed,
                    HeldState::Henkan => HeldRecord::Henkan,
                    HeldState::Undecided {
                        hold,
                        tap,
                        deadline,
                        sent,
                        alone,
                    } => HeldRecord::Undecided {
                        hold,
                        tap,
                        deadline,
                        sent,
                        alone,
                    },
                },
            });
        }
        records
    }

    /// Pick up where a checkpoint left off.
    fn from_checkpoint(layout: Layout, settings: Settings, checkpoint: &Checkpoint) -> Self {
        Self {
            layout,
            devices: checkpoint.devices.clone(),
            held: checkpoint
                .held
                .iter()
                .map(|(device, key, state)| Held {
                    device: *device,
                    key: *key,
                    state: match *state {
                        HeldRecord::Down { key, modifiers } => HeldState::Down { key, modifiers },
                        HeldRecord::Swallowed => HeldState::Swallowed,
                        HeldRecord::Henkan => HeldState::Henkan,
                        HeldRecord::Undecided {
                            hold,
                            tap,
                            deadline,
                            sent,
                            alone,
                        } => HeldState::Undecided {
                            hold,
                            tap,
                            deadline,
                            sent,
                            alone,
                        },
                    },
                })
                .collect(),
            pointer: settings.pointer,
            repeat: settings.repeat,
            repeating: checkpoint.repeating.map(|(device, key, due)| Repeating {
                device,
                key,
                due,
            }),
            pointer_buttons: checkpoint.pointer_buttons,
        }
    }

    /// The modifier keys the OS has actually been told about.
    ///
    /// The converted output of the keys that are down, which is what makes a
    /// remapped Caps Lock act as control — and the keys, not the modifiers, since
    /// this is the set an event is delivered with and a key is what a report names.
    /// A modifier that a rule *adds* to its own output — the shift inside Dudrack's
    /// `:` — is not here: it belongs to that one keystroke, not to the state.
    fn keys_in_effect(&self) -> ModifierKeys {
        self.held
            .iter()
            .fold(ModifierKeys::NONE, |acc, held| match held.state {
                HeldState::Down { key, .. } => acc.with(key),
                HeldState::Undecided {
                    hold,
                    sent: Some(_),
                    ..
                } => acc.with(hold),
                _ => acc,
            })
    }

    /// The modifiers rules are matched against.
    ///
    /// What is in effect plus the holds that have not been sent yet, as modifiers
    /// rather than keys: a rule asks for shift and not for a side, so matching on
    /// keys would make every symbol rule name both of them.
    ///
    /// Holding the space bar has to shift the next key even though nothing has
    /// reached the OS — deciding what that key converts to is exactly what the hold
    /// is for.
    fn modifiers_for_rules(&self) -> Modifiers {
        self.held
            .iter()
            .fold(self.keys_in_effect().kinds(), |acc, held| {
                match held.state {
                    HeldState::Undecided {
                        hold, sent: None, ..
                    } => union(acc, hold),
                    _ => acc,
                }
            })
    }

    fn henkan_held(&self) -> bool {
        self.held
            .iter()
            .any(|held| matches!(held.state, HeldState::Henkan))
    }

    fn held_index(&self, device: DeviceId, key: Key) -> Option<usize> {
        self.held
            .iter()
            .position(|h| h.device == device && h.key == key)
    }

    pub fn handle(&mut self, event: HostEvent, host: &mut dyn SinkHost) {
        match event.kind {
            EventKind::DeviceAttached(info) => {
                self.devices.retain(|d| d.id != info.id);
                self.devices.push(info);
            }
            EventKind::DeviceDetached(id) => {
                self.release_all(id, host);
                self.devices.retain(|d| d.id != id);
            }
            EventKind::KeyDown { device, key } => self.key_down(device, key, event.at, host),
            EventKind::KeyUp { device, key } => self.key_up(device, key, event.at, host),
            // The device is carried by the event and not consulted here: no rule
            // scopes the pointer, and the cursor is one whichever thumb stick
            // moved it. It stays on the event because a trace that could not say
            // which device a report came from could not be read against the
            // hardware (ADR-0009).
            EventKind::Pointer { report, .. } => self.pointer(report, host),
            EventKind::Timer => self.repeat_due(event.at, host),
            // Nothing to do with a probe but arrive at it. Getting here is the
            // answer the supervisor is waiting for, and it is delivered by the
            // heartbeat the loop sends after every event.
            EventKind::Probe => {}
        }
    }

    /// Hand the repeat to this key, after the initial delay.
    fn arm_repeat(&mut self, device: DeviceId, key: Key, at: Instant, host: &mut dyn SinkHost) {
        let Some(repeat) = self.repeat else { return };
        let due = at.saturating_add(repeat.initial);
        self.repeating = Some(Repeating { device, key, due });
        host.set_timer(Some(due));
    }

    /// Stop whatever was repeating, and take the wake-up back.
    ///
    /// Cancelling rather than letting the wake-up arrive and find nothing: a
    /// timer left running would wake the loop for as long as the process lives,
    /// and on the macOS side that is a run loop turning around an empty queue.
    fn disarm_repeat(&mut self, host: &mut dyn SinkHost) {
        if self.repeating.take().is_some() {
            host.set_timer(None);
        }
    }

    fn repeat_due(&mut self, at: Instant, host: &mut dyn SinkHost) {
        let (Some(repeat), Some(mut repeating)) = (self.repeat, self.repeating) else {
            return;
        };
        // The key can be gone: a release and this wake-up can both be waiting by
        // the time either is read, and the release does not get to assume it was
        // handled first.
        let Some(index) = self.held_index(repeating.device, repeating.key) else {
            self.disarm_repeat(host);
            return;
        };
        // Only a key that reached the OS repeats. A hold still undecided must not
        // be resolved by this: nothing was tapped, and the tap window is what
        // decides that, not the repeat clock.
        if let HeldState::Down { key, modifiers } = self.held[index].state {
            // Let go and pressed again, and not a second press: a key the OS
            // already believes is down, pressed again, has told it nothing — one
            // held that way produces exactly one character
            // (`docs/platform/macos/key-repeat.md`).
            //
            // Under the same modifiers both times, and not through the release
            // path: a repeat is the same keystroke again, so the set it went down
            // with must not be settled up in the middle of it.
            host.inject(Injected::KeyUp { key, modifiers });
            host.inject(Injected::KeyDown { key, modifiers });
        }
        repeating.due = at.saturating_add(repeat.interval);
        self.repeating = Some(repeating);
        host.set_timer(Some(repeating.due));
    }

    /// Relay a pointer report, and let a button meet the keyboard state.
    fn pointer(&mut self, report: PointerReport, host: &mut dyn SinkHost) {
        // Tuned first, so that everything below decides on what will actually be
        // sent: a report that is still once scaled is one the hardware may have
        // described as motion, and relaying it would be an acceleration curve
        // applied to nothing.
        let report = self.pointer.apply(report);
        let buttons_changed = report.buttons != self.pointer_buttons;

        // A button going down needs the outstanding lazy holds to be real first,
        // so that holding space and clicking is a shift-click rather than a plain
        // one. Motion is left alone deliberately: moving a thumb stick is not
        // typing, and a space tapped with a nudge of the cursor in the middle is
        // still a tap.
        if buttons_changed && report.buttons.any() {
            self.send_lazy_holds(host);
        }

        // Nothing moved and no button changed hands. Relaying it would be a
        // report the hardware never made, and the OS applies its acceleration
        // curve per report, so an extra one is not free.
        if report.is_still() && !buttons_changed {
            return;
        }

        self.pointer_buttons = report.buttons;
        host.inject(Injected::Pointer(report));
    }

    fn key_down(&mut self, device: DeviceId, key: Key, at: Instant, host: &mut dyn SinkHost) {
        // Auto-repeat: repeat what the first press resolved to instead of
        // resolving again. Re-resolving would let a layer taken up mid-repeat
        // change the key under the user's finger, and leave the eventual key-up
        // releasing something other than what went down.
        if let Some(index) = self.held_index(device, key) {
            if let HeldState::Down { key, modifiers } = self.held[index].state {
                host.inject(Injected::KeyDown { key, modifiers });
            }
            return;
        }

        // Any other key going down settles that this one was not tapped alone.
        self.interrupt_taps();

        let outcome = match self.devices.iter().find(|d| d.id == device).copied() {
            Some(info) => self.layout.resolve(
                &info,
                key,
                at,
                self.modifiers_for_rules(),
                self.henkan_held(),
            ),
            // A keyboard the host never announced. Passing the key through
            // unconverted beats dropping it: the host may already be
            // suppressing the physical event, and a swallowed keystroke is the
            // failure ADR-0008 rules out.
            None => Outcome::Emit {
                key,
                consumed: Modifiers::NONE,
                added: Modifiers::NONE,
            },
        };

        // Held onto because the arms below rebind `key` to what the rule emits,
        // and the repeat is owned by the finger, not by the character.
        let physical = key;
        let state = match outcome {
            Outcome::Emit {
                key,
                consumed,
                added,
            } => {
                // A key that needs a modifier is what makes a lazy hold real,
                // so the hold has to reach the OS first. A modifier of our own
                // needs none, which is what keeps a tap from leaving a stray
                // shift behind when the user only chorded two modifiers.
                if key.modifier().is_none() {
                    self.send_lazy_holds(host);
                }
                // What is held, less what the rule consumed, plus what it added —
                // and the emitted key itself where that is a modifier, because a
                // modifier key *is* its report's set and a set that left it out
                // would be the key never reaching the OS at all.
                let modifiers = self
                    .keys_in_effect()
                    .without_kinds(consumed)
                    .with_kinds(added)
                    .with(key);
                host.inject(Injected::KeyDown { key, modifiers });
                // The repeat follows the key that produced a character, and only
                // that: a modifier taking it would turn holding shift into a
                // stream of shifts, and a swallowed key or an undecided hold has
                // nothing to stream. Neither of those stops the key already
                // repeating either — chording a modifier onto a repeating key is
                // ordinary typing.
                if key.modifier().is_none() {
                    self.arm_repeat(device, physical, at, host);
                }
                HeldState::Down { key, modifiers }
            }
            Outcome::Swallow => HeldState::Swallowed,
            Outcome::HoldHenkan => HeldState::Henkan,
            Outcome::TapHold {
                hold,
                tap,
                deadline,
            } => HeldState::Undecided {
                hold,
                tap,
                deadline,
                sent: None,
                alone: true,
            },
        };

        self.held.push(Held { device, key, state });
    }

    fn key_up(&mut self, device: DeviceId, key: Key, at: Instant, host: &mut dyn SinkHost) {
        let Some(index) = self.held_index(device, key) else {
            // A release with no press behind it — the key was already down when
            // the sink started, or its press was seen by someone else. Release
            // it unconverted rather than swallowing it; a spurious key-up is
            // harmless, a missing one strands a modifier inside applications.
            let modifiers = self.keys_in_effect();
            host.inject(Injected::KeyUp { key, modifiers });
            return;
        };

        match self.held.remove(index).state {
            HeldState::Down { key, modifiers } => self.release_at_the_os(key, modifiers, host),
            HeldState::Swallowed | HeldState::Henkan => {}
            HeldState::Undecided {
                tap,
                deadline,
                sent: None,
                alone: true,
                ..
            } if at <= deadline => {
                // Held alone and let go in time: a tap. Read the modifiers
                // after the entry is gone, so the hold that just ended is not
                // counted.
                let modifiers = self.keys_in_effect();
                host.inject(Injected::KeyDown {
                    key: tap,
                    modifiers,
                });
                self.release_at_the_os(tap, modifiers, host);
            }
            HeldState::Undecided {
                hold,
                sent: Some(modifiers),
                ..
            } => self.release_at_the_os(hold, modifiers, host),
            // Held past the tap window, or interrupted and never needed. The
            // hold was never sent, so there is nothing to release and no
            // keystroke to type.
            HeldState::Undecided { .. } => {}
        }
        self.disarm_if_released(host);
    }

    /// Let one key go at the OS, and settle the modifiers behind it.
    ///
    /// Two events where a keyboard has one, and in this order. The key goes up
    /// under the set it went down with, so a shifted character is released as
    /// shifted — one event doing both leaves applications to decide whether the
    /// key-up happened before or after the flags changed, and a release arriving
    /// unshifted is the failure that shows up as a stuck or wrong character. Then
    /// what is actually still held, which is what lets go of a modifier the
    /// keystroke had only borrowed and puts back one its rule had consumed.
    ///
    /// A modifier key is released under what is left rather than under what it went
    /// down with: the recorded set can name a key another finger has since let go
    /// of, and re-asserting it would be a modifier stuck down in every application.
    fn release_at_the_os(
        &mut self,
        key: Key,
        went_down_with: ModifierKeys,
        host: &mut dyn SinkHost,
    ) {
        let left = self.keys_in_effect();
        let modifiers = match key.modifier().is_some() {
            true => left,
            false => went_down_with,
        };
        host.inject(Injected::KeyUp { key, modifiers });
        if left != modifiers {
            host.inject(Injected::Modifiers(left));
        }
    }

    /// Drop the repeat once the key carrying it is no longer down.
    ///
    /// Asked of the held list rather than compared against the key just
    /// released, so that a keyboard being torn out answers it too — and so a
    /// release of some *other* key leaves the repeat running, which is what a
    /// keyboard does.
    fn disarm_if_released(&mut self, host: &mut dyn SinkHost) {
        if let Some(repeating) = self.repeating {
            if self.held_index(repeating.device, repeating.key).is_none() {
                self.disarm_repeat(host);
            }
        }
    }

    /// Give every undecided hold to the OS, in the order the keys went down.
    fn send_lazy_holds(&mut self, host: &mut dyn SinkHost) {
        for index in 0..self.held.len() {
            if let HeldState::Undecided {
                hold, sent: None, ..
            } = self.held[index].state
            {
                // The hold itself as well as what is held alongside it: the set is
                // the whole of what its report says, so a hold left out of its own
                // event would reach the OS as nothing at all.
                let modifiers = self.keys_in_effect().with(hold);
                host.inject(Injected::KeyDown {
                    key: hold,
                    modifiers,
                });
                if let HeldState::Undecided { sent, alone, .. } = &mut self.held[index].state {
                    *sent = Some(modifiers);
                    *alone = false;
                }
            }
        }
    }

    fn interrupt_taps(&mut self) {
        for held in &mut self.held {
            if let HeldState::Undecided { alone, .. } = &mut held.state {
                *alone = false;
            }
        }
    }

    /// Let go of everything a keyboard was holding.
    ///
    /// In reverse order of press, so a modifier is released after the keys it
    /// was modifying. The other way round hands applications a key-up whose
    /// modifier has already gone, which is how a shifted character ends up
    /// arriving unshifted.
    fn release_all(&mut self, device: DeviceId, host: &mut dyn SinkHost) {
        let mut index = self.held.len();
        while index > 0 {
            index -= 1;
            if self.held[index].device != device {
                continue;
            }
            match self.held.remove(index).state {
                HeldState::Down { key, modifiers } => self.release_at_the_os(key, modifiers, host),
                HeldState::Undecided {
                    hold,
                    sent: Some(modifiers),
                    ..
                } => self.release_at_the_os(hold, modifiers, host),
                // An undecided hold torn down with the keyboard is not a tap.
                // The user unplugged something; they did not type a character.
                _ => {}
            }
        }
        self.disarm_if_released(host);
    }
}

fn union(modifiers: Modifiers, key: Key) -> Modifiers {
    match key.modifier() {
        Some(modifier) => modifiers.union(modifier),
        None => modifiers,
    }
}

/// What this run of the sink was asked to be.
///
/// The sink's own rather than one shape for both roles: what a machine that
/// converts is asked for is not what a machine that forwards is, and one struct
/// covering both would carry a field the other must ignore.
///
/// **Three flags would be eight runs and two of them are not modes.** Taking the
/// keyboards while delivering nowhere swallows every keystroke on this machine,
/// which is the one outcome ADR-0008 rules out — and it would be *asked for* rather
/// than failed into, so nothing downstream would notice. Accepting the other
/// machine's input while delivering nowhere converts it into nothing. Neither is a
/// run worth warning about, because neither is a run: what suppressing and
/// listening are properties *of* is delivering, so they are fields of the variant
/// that delivers and the two cannot be asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request {
    /// Convert for real and deliver nothing.
    ///
    /// What makes it safe to be what a bare command does: nothing is taken from this
    /// machine, and nothing reaches applications.
    DryRun,
    /// Deliver the converted keystrokes to the OS.
    ///
    /// Always exclusively, which is why there is no flag for it: the physical
    /// keystroke is delivered by the OS as well, so a run that injected without
    /// taking the keyboards would type every key twice — once unconverted from the
    /// keyboard and once converted from here.
    Injecting {
        /// Accept input from the other machine as well as this one's keyboards.
        ///
        /// A choice of its own and not half of a mode: converting only the Mac's own
        /// keyboards is a whole thing to want, and so is taking the other machine's
        /// input too.
        listen: bool,
    },
}

/// Say what this run costs, if it costs anything.
///
/// One cost, and it is not the request's doing: what [`Request`] can express is
/// safe by construction, so what is left to warn about is the machine. A run that
/// delivers holds the keyboards through a wedge, and whether anything is watching
/// to end it is something only the machine knows (ADR-0008).
fn warn_about(request: &Request, host: &mut dyn SinkHost) {
    if matches!(request, Request::Injecting { .. }) && !host.is_supervised() {
        host.warn(format_args!(
            "no watchdog: a wedge here keeps the keyboards and nothing will notice; run under \
             favjit-watchdog"
        ));
    }
}

/// How a run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ending {
    /// It converted until there was nothing more, or until it was asked to stop.
    Converted,
    /// Converting was switched off.
    SwitchedOff,
    /// The device converted keystrokes go out through went away.
    OutputGone,
    /// The link stopped being served, so nothing can reach this machine from the
    /// other one any more.
    ///
    /// The run ends rather than carrying on with the keyboards in front of the
    /// person: nothing rebinds the socket, and the advertisement goes with it, so
    /// from the other machine a converter that stayed up would be indistinguishable
    /// from one that is not running (ADR-0017). Ending is what lets whatever
    /// supervises favjit start it again, which is how the socket comes back.
    LinkGone,
    /// This process may not read the keyboards, so there is nothing to convert.
    NoPermission,
    /// There is nowhere to send converted keystrokes.
    NoOutput,
    /// The keyboards could not be taken.
    NoInput,
}

/// The two answers every run needs before it opens anything, or the ending that
/// says it will not.
///
/// A run switched off ends when it is switched on again rather than going on to
/// convert, so that whatever restarts it takes the keyboards from nothing: carrying
/// on here would resume a machine whose state was assembled before the switch.
fn switched_off(host: &mut dyn SinkInputHost) -> Option<Ending> {
    if !host.switched_on() {
        host.wait_until_on();
        return Some(Ending::SwitchedOff);
    }
    if !host.may_read_input() {
        return Some(Ending::NoPermission);
    }
    None
}

/// Watch what the keyboards do and convert none of it.
///
/// Takes them shared, because this run is for looking at a keyboard: seizing would
/// take it away from the person while they press the key they are identifying. It
/// injects nothing at all, which is why it needs no output and asks for none — that
/// is what makes it a safe way to find out where a key reports (ADR-0006).
pub fn watch(host: &mut dyn SinkInputHost) -> Ending {
    if let Some(off) = switched_off(host) {
        return off;
    }
    if !host.take_input(false) {
        return Ending::NoInput;
    }
    while let Some(_event) = host.next_event() {
        host.heartbeat();
    }
    ending(host.ended())
}

/// One run of the sink, from what it was asked for to the keystrokes it converts.
///
/// Here rather than in a binary or a host because the order is the part that can be
/// wrong, and the order is what ADR-0008 is about: the output comes up before any
/// keyboard is taken, so a machine with nowhere to send keystrokes leaves the
/// keyboards alone. A binary parses the arguments and builds the host; everything
/// after that is this.
///
/// A dry run opens no output at all: with nothing suppressed, injecting delivers
/// every keystroke twice, and a run that exists to change nothing outside this
/// process should not leave a virtual keyboard behind either.
pub fn run(
    request: &Request,
    layout: Layout,
    settings: impl Into<Settings>,
    host: &mut dyn SinkHost,
    trace: Option<&mut [u8]>,
) -> Ending {
    if let Some(off) = switched_off(host) {
        return off;
    }

    // Before anything is opened or taken, because a cost named after the keyboards
    // are held is one the person can no longer decide about.
    warn_about(request, host);

    // The keyboards are taken exclusively only by a run that delivers somewhere, and
    // it is the shape of the request that makes that so rather than a check here:
    // `suppress` is a field of the variant that injects, so there is no request in
    // which it is set and nothing is being delivered (ADR-0008).
    let suppress = match request {
        Request::DryRun => false,
        Request::Injecting { listen } => {
            if !host.open_output() {
                return Ending::NoOutput;
            }
            // After the device is up, because until then there is no service to
            // carry the properties.
            host.tune_output();
            // After the output too: input let in from the other machine before there
            // is anywhere to send it would be converted into nothing, which is why
            // listening is a property of injecting rather than a flag beside it
            // (ADR-0017).
            if *listen {
                // Established here rather than left to the host, so that the
                // sequence deciding what this machine presents is the same one the
                // suite drives (ADR-0017). Only for a run that will listen: an
                // identity is a file written for a link, and a run that opens no
                // socket should leave none behind.
                match pairing::identity(&mut *host) {
                    Ok(identity) => {
                        // The socket goes with the loop that serves it: a link left
                        // bound with nothing turning would take the connection and
                        // never answer, which from the other machine is worse than
                        // a machine that is not there.
                        if let Some(mut link) = host.bind_link(&identity) {
                            host.run_alongside(Box::new(move || crate::link::serve(&mut *link)));
                        }
                    }
                    // Said and not returned, because the run carries on: the
                    // keyboards in front of the person do not depend on the link,
                    // and a link that silently never came up looks from the other
                    // machine exactly like one that is refusing.
                    Err(why) => host.warn(format_args!("no identity, so no link: {why}")),
                }
            }
            // Always, for a run that delivers: the physical keystroke arrives
            // alongside the converted one otherwise, and that is not a mode
            // ([`Request::Injecting`]).
            true
        }
    };

    if !host.take_input(suppress) {
        return Ending::NoInput;
    }

    let settings = settings.into();
    match trace {
        // Recorded when a supervisor provided the memory, and not at all otherwise:
        // a trace this process allocated for itself would be lost in exactly the
        // failures it exists for, and it would be a keylog nobody asked for
        // (ADR-0009).
        Some(bytes) => convert_traced(layout, settings, host, &mut Trace::new(bytes)),
        None => convert(layout, settings, host),
    }
    ending(host.ended())
}

/// What the end of the stream means for the run.
///
/// Every one of them ends it, so this decides only what is said afterwards — and
/// that has to be decided somewhere a test can read it, since the exit code is what
/// whatever supervises favjit acts on.
fn ending(ended: Ended) -> Ending {
    match ended {
        Ended::AsAsked => Ending::Converted,
        Ended::SwitchedOff => Ending::SwitchedOff,
        Ended::OutputGone => Ending::OutputGone,
        // The only loop this role hands over is the link's, so the machine saying
        // that loop came back is the link no longer being served.
        Ended::AlongsideStopped => Ending::LinkGone,
    }
}

/// Convert whatever arrives, until nothing more will.
///
/// One loop over one event stream, which is what makes a trace replayable and
/// the end-to-end suite deterministic (ADR-0006). A run reaches this through
/// [`run`]; a replay drives it directly, because a recording is not a machine to
/// bring up.
pub fn convert(layout: Layout, settings: impl Into<Settings>, host: &mut dyn SinkHost) {
    let mut sink = Sink::new(layout, settings.into());
    while let Some(event) = host.next_event() {
        sink.handle(event, host);
        // After handling, not before: a heartbeat sent on the way in would
        // vouch for a loop that is about to wedge inside `handle`, which is one
        // of the failures ADR-0008 exists to catch.
        host.heartbeat();
    }
}

/// The same, from the state a trace's checkpoint recorded.
///
/// What makes a bounded trace replayable: the records that survive eviction
/// describe changes to a state, and this is that state
/// (`docs/adr/0009-trace-and-replay.md`).
pub fn convert_from(
    layout: Layout,
    settings: impl Into<Settings>,
    checkpoint: &Checkpoint,
    host: &mut dyn SinkHost,
) {
    let mut sink = Sink::from_checkpoint(layout, settings.into(), checkpoint);
    while let Some(event) = host.next_event() {
        sink.handle(event, host);
        host.heartbeat();
    }
}

/// The same, recording everything into a trace as it goes.
///
/// The recording wraps the host rather than living inside the sink, so that what
/// is written is exactly what crossed the boundary: a sink that recorded its own
/// intentions could record one thing and do another, and the whole value of a
/// trace is that replaying it cannot disagree with the run.
fn convert_traced(
    layout: Layout,
    settings: impl Into<Settings>,
    host: &mut dyn SinkHost,
    trace: &mut Trace<'_>,
) {
    let mut sink = Sink::new(layout, settings.into());
    let mut recorder = Recording {
        host,
        trace,
        since_checkpoint: 0,
        checkpoint_at: None,
    };
    recorder.checkpoint(&sink, None);

    while let Some(event) = recorder.host.next_event() {
        // The checkpoint goes in *before* the event that prompted it, and the loop
        // writes the event itself for that reason. Recorded the other way round,
        // the event would sit in the segment being left behind: eviction would drop
        // it while keeping the checkpoint taken before it happened, and the replay
        // would be missing one event with nothing to say so.
        if recorder.checkpoint_is_due(event.at) {
            recorder.checkpoint(&sink, Some(event.at));
        }
        recorder.record(Record::Event(event));

        sink.handle(event, &mut recorder);
        recorder.heartbeat();
    }
}

/// A host with a trace behind it.
struct Recording<'a, 'b, 'c> {
    host: &'a mut dyn SinkHost,
    trace: &'b mut Trace<'c>,
    /// Records written since the last checkpoint.
    since_checkpoint: usize,
    /// When that checkpoint was, by the clock the events carry.
    ///
    /// Kept here rather than read back out of the trace: a checkpoint record
    /// carries the state and not the time, and scanning the ring for the last one
    /// would be a walk of the whole buffer per event.
    checkpoint_at: Option<Instant>,
}

/// How many records may follow a checkpoint before the next one.
///
/// A count as well as a clock, because the two bound different things: a minute of
/// typing is a few hundred records and a minute of pointer movement is tens of
/// thousands, so a clock alone would make the window depend on what the user was
/// doing rather than on a budget.
const RECORDS_PER_SEGMENT: usize = 512;

/// And a clock, because a quiet run would otherwise checkpoint once and keep a
/// window that reached back to whenever the process started — which is history
/// nobody needs and space the recent past wants.
const SEGMENT_SECONDS: u64 = 60;

impl Recording<'_, '_, '_> {
    fn checkpoint(&mut self, sink: &Sink, at: Option<Instant>) {
        for record in sink.checkpoint_records() {
            self.trace.push(record);
        }
        self.since_checkpoint = 0;
        if at.is_some() {
            self.checkpoint_at = at;
        }
    }

    fn checkpoint_is_due(&self, at: Instant) -> bool {
        if self.since_checkpoint >= RECORDS_PER_SEGMENT {
            return true;
        }
        // A segment no bigger than a quarter of the buffer, so an eviction leaves
        // most of the window rather than nearly all of it.
        if self.since_checkpoint >= self.trace.capacity() / 4 {
            return true;
        }
        match self.checkpoint_at {
            Some(last) => {
                at.as_nanos().saturating_sub(last.as_nanos()) >= SEGMENT_SECONDS * 1_000_000_000
            }
            None => false,
        }
    }

    fn record(&mut self, record: Record) {
        self.trace.push(record);
        self.since_checkpoint += 1;
    }
}

impl SinkInputHost for Recording<'_, '_, '_> {
    /// Passed through, all of them: what this wrapper is for is the outbound calls
    /// a replay has to reproduce, and bringing the machine up happens once, before
    /// the loop that records anything.
    fn switched_on(&mut self) -> bool {
        self.host.switched_on()
    }

    fn wait_until_on(&mut self) {
        self.host.wait_until_on();
    }

    fn may_read_input(&mut self) -> bool {
        self.host.may_read_input()
    }

    fn take_input(&mut self, suppress: bool) -> bool {
        self.host.take_input(suppress)
    }

    fn ended(&mut self) -> Ended {
        self.host.ended()
    }
}

impl Host for Recording<'_, '_, '_> {
    /// Passed through without recording, because the loop above records the event
    /// itself — it has to, to get a checkpoint in ahead of one.
    fn next_event(&mut self) -> Option<HostEvent> {
        self.host.next_event()
    }

    fn is_supervised(&mut self) -> bool {
        self.host.is_supervised()
    }

    fn warn(&mut self, message: core::fmt::Arguments) {
        // Not recorded, for the same reason as the heartbeat: it says nothing about
        // what the loop did with an event, and a replay whose lines differ is still
        // the same run.
        self.host.warn(message);
    }

    fn heartbeat(&mut self) {
        // Not recorded: it is the one outbound call that says nothing about what
        // the loop did, only that it came back round, and a replay that produced
        // heartbeats in different places would still be the same run.
        self.host.heartbeat();
    }
}

/// Passed through and not recorded: a trace is a recording of the loop, and the
/// identity is settled before the first event arrives — a replay from it would be
/// establishing an identity for a link that is not being served.
impl IdentityStore for Recording<'_, '_, '_> {
    fn read(&mut self) -> Option<Vec<u8>> {
        self.host.read()
    }

    fn make(&mut self) -> Option<Identity> {
        self.host.make()
    }

    fn keep(&mut self, bytes: &[u8]) -> bool {
        self.host.keep(bytes)
    }
}

impl SinkHost for Recording<'_, '_, '_> {
    fn set_timer(&mut self, at: Option<Instant>) {
        self.host.set_timer(at);
        self.record(Record::SetTimer(at));
    }

    fn open_output(&mut self) -> bool {
        self.host.open_output()
    }

    fn tune_output(&mut self) {
        self.host.tune_output();
    }

    fn bind_link(&mut self, identity: &Identity) -> Option<Box<dyn LinkHost + Send>> {
        self.host.bind_link(identity)
    }

    fn run_alongside(&mut self, work: Box<dyn FnOnce() + Send>) -> bool {
        self.host.run_alongside(work)
    }

    fn inject(&mut self, injected: Injected) {
        self.host.inject(injected);
        // `ok` is always true because this surface cannot fail: `inject` returns
        // nothing, so a rejected keystroke is something only the host knows about
        // and nothing `core` can behave differently on. The field is here because
        // the day it can fail, a trace without it would replay differently from
        // the run it recorded.
        self.record(Record::Injected { injected, ok: true });
    }
}
