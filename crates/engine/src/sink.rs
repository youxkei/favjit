//! The sink role: one run, one loop, one pipeline (ADR-0006).
//!
//! Everything a run of the converter does is here — bringing the machine up in the
//! order ADR-0008 requires, then converting what arrives from this machine's own
//! keyboards and from the other machine's, through the one pipeline both pass
//! through. What the platform provides is behind [`SinkHost`], one call at a time.

use core::time::Duration;

use favjit_host::sink::{SinkHost, SinkInputHost};
use favjit_host::{Entropy, Host, IdentityStore, NoOutput, OutputReport};

use crate::clock::Clock;
use crate::hid::report::{self, Rendered};
use crate::pairing;
use crate::pointer::Tuning;
use crate::supervision::Beating;
use crate::trace::{Checkpoint, HeldRecord, Record, Trace};
use crate::{
    Buttons, DeviceId, DeviceInfo, DeviceMatch, Ended, EventKind, HostEvent, Injected, Instant,
    Key, Layout, ModifierKeys, Modifiers, Outcome, PointerReport,
};

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
    /// (`docs/platform/macos/let-the-machine-produce-the-key-repeats.md`).
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

/// The sink's whole state: attached keyboards, keys currently down, the
/// layout, and the device every converted key is rendered against. The layer
/// and the modifier set are derived from the held keys rather than tracked
/// alongside them, so they cannot fall out of step with what is physically
/// down.
struct Sink {
    /// The device every converted key is written to.
    ///
    /// Held here rather than reached through the host, so that a write is asked
    /// of a device that is there: what a run holds is the connection the output
    /// came up on, and there is no converting before it has.
    writing: Box<dyn favjit_host::sink::Injecting>,
    /// Nanoseconds per keystroke, in the two segments this end can see.
    ///
    /// Measured here and not by the host, because both readings are of this
    /// loop: when it began handling the event, and how long the one write took.
    /// A host that kept them would be keeping what a call answered (ADR-0006).
    latency: Latency,
    /// When the event being handled came out of the wait, which is what the
    /// first of those segments is measured from.
    handling: Instant,
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
    /// What every `Injected` this run has produced has actually rendered to.
    ///
    /// Kept here and not in the host: a report is built by pressing and
    /// releasing usages against whatever the last report said, so the host on
    /// the other side of [`SinkHost::inject`] would have to be handed this
    /// state to render at all — and a host trusted with it could render an
    /// `Injected` this run never produced, which is a converter with a second
    /// door in (ADR-0006).
    device: report::Keyboard,
    /// What [`Sink::handle`] injected while handling the event just given to it,
    /// and whether each one found a report to ride.
    ///
    /// Cleared at the top of [`Sink::handle`] rather than drained by it: a trace
    /// wants this, and the untraced loop does not, so the loop that does is the
    /// one that reads it — a callback threaded through fifteen private methods
    /// for the one caller that cares would be machinery for that caller alone.
    pending_injections: Vec<(Injected, bool, i32)>,
    /// Whether this run has already said that what it converts is reaching
    /// nothing, which is the policy and not the fact: the fact comes back from
    /// every write, and a line per keystroke would be the loudest thing in the
    /// log for the rest of the run.
    said_it_goes_nowhere: bool,
}

impl Sink {
    fn new(
        layout: Layout,
        settings: Settings,
        writing: Box<dyn favjit_host::sink::Injecting>,
    ) -> Self {
        Self {
            writing,
            latency: Latency::default(),
            handling: Instant::default(),
            layout,
            devices: Vec::new(),
            held: Vec::new(),
            pointer: settings.pointer,
            repeat: settings.repeat,
            repeating: None,
            pointer_buttons: Buttons::NONE,
            device: report::Keyboard::default(),
            pending_injections: Vec::new(),
            said_it_goes_nowhere: false,
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
    fn from_checkpoint(
        layout: Layout,
        settings: Settings,
        checkpoint: &Checkpoint,
        writing: Box<dyn favjit_host::sink::Injecting>,
    ) -> Self {
        Self {
            writing,
            latency: Latency::default(),
            handling: Instant::default(),
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
            // Not in `Checkpoint`: a resumed run is a fresh process, whose device
            // has never sent a report either, so this starts exactly as `new`'s
            // does rather than carrying report state a fresh device does not have.
            device: report::Keyboard::default(),
            pending_injections: Vec::new(),
            said_it_goes_nowhere: false,
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

    /// The one call every method below reaches [`SinkHost::send_report`] through,
    /// so `injected` is rendered against `self.device` exactly once: a second
    /// render of the same event elsewhere could only disagree with this one,
    /// since a report is built by pressing and releasing usages against
    /// whatever the last one said.
    fn inject(&mut self, injected: Injected, host: &mut dyn SinkHost) {
        let rendered = report::render(&mut self.device, injected);
        let ok = rendered.is_ok();
        let mut code = 0;
        if let Ok(rendered) = rendered {
            // Read on either side of the write, so the two segments are
            // separated: one number covering both would not say whether the
            // time went into the conversion or into the call, and those have
            // different remedies.
            let before = host.now();
            code = send(rendered, &mut *self.writing);
            let after = host.now();
            self.latency
                .pipeline
                .push(before.nanos.saturating_sub(self.handling.nanos));
            self.latency
                .post
                .push(after.nanos.saturating_sub(before.nanos));
            // Once, not per keystroke: nothing is going to open the device
            // part way through a run, so the rest of it would be this line
            // (ADR-0006: how often it is said). The number itself goes on the
            // trace rather than into the sentence — what a person needs here is
            // that nothing is arriving, and which error it was is read after the
            // run (ADR-0009).
            if code != 0 && !self.said_it_goes_nowhere {
                self.said_it_goes_nowhere = true;
                host.warn(format_args!(
                    "converted input has nowhere to go: nothing is reaching applications"
                ));
            }
        }
        self.pending_injections.push((injected, ok, code));
    }

    /// What this run injected while handling the event just given to [`handle`],
    /// and whether each one found a report to ride.
    pub(crate) fn pending_injections(&self) -> &[(Injected, bool, i32)] {
        &self.pending_injections
    }

    /// The wake-up this run is waiting on, if a held key is due to repeat.
    pub(crate) fn current_timer(&self) -> Option<Instant> {
        self.repeating.map(|r| r.due)
    }

    pub(crate) fn handle(&mut self, event: HostEvent, host: &mut dyn SinkHost) {
        self.pending_injections.clear();
        // Every event, a timer's wake-up included: one that did not would leave
        // the previous event's time standing, and a repeat injected under it
        // would read as however long the key had been held.
        self.handling = event.at;
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
            // Which machine the keyboard is driving is the source's question: this end
            // converts whatever arrives and has no say in what is sent (ADR-0002).
            EventKind::Asked(_) => {}
        }
    }

    /// Hand the repeat to this key, after the initial delay.
    fn arm_repeat(&mut self, device: DeviceId, key: Key, at: Instant) {
        let Some(repeat) = self.repeat else { return };
        let due = at.saturating_add(repeat.initial);
        self.repeating = Some(Repeating { device, key, due });
    }

    /// Stop whatever was repeating, and take the wake-up back.
    ///
    /// Nothing else to cancel: the wake-up a run waits for is read off
    /// [`Sink::current_timer`] fresh on every wait, so taking a repeat out of
    /// [`Sink::repeating`] here is a wake-up nobody is waiting for any more,
    /// without a second place that also has to be told.
    fn disarm_repeat(&mut self) {
        self.repeating = None;
    }

    fn repeat_due(&mut self, at: Instant, host: &mut dyn SinkHost) {
        let (Some(repeat), Some(mut repeating)) = (self.repeat, self.repeating) else {
            return;
        };
        // The key can be gone: a release and this wake-up can both be waiting by
        // the time either is read, and the release does not get to assume it was
        // handled first.
        let Some(index) = self.held_index(repeating.device, repeating.key) else {
            self.disarm_repeat();
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
            self.inject(Injected::KeyUp { key, modifiers }, host);
            self.inject(Injected::KeyDown { key, modifiers }, host);
        }
        repeating.due = at.saturating_add(repeat.interval);
        self.repeating = Some(repeating);
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
        self.inject(Injected::Pointer(report), host);
    }

    fn key_down(&mut self, device: DeviceId, key: Key, at: Instant, host: &mut dyn SinkHost) {
        // Auto-repeat: repeat what the first press resolved to instead of
        // resolving again. Re-resolving would let a layer taken up mid-repeat
        // change the key under the user's finger, and leave the eventual key-up
        // releasing something other than what went down.
        if let Some(index) = self.held_index(device, key) {
            if let HeldState::Down { key, modifiers } = self.held[index].state {
                self.inject(Injected::KeyDown { key, modifiers }, host);
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
                if crate::modifiers::modifier_of(key).is_none() {
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
                self.inject(Injected::KeyDown { key, modifiers }, host);
                // The repeat follows the key that produced a character, and only
                // that: a modifier taking it would turn holding shift into a
                // stream of shifts, and a swallowed key or an undecided hold has
                // nothing to stream. Neither of those stops the key already
                // repeating either — chording a modifier onto a repeating key is
                // ordinary typing.
                if crate::modifiers::modifier_of(key).is_none() {
                    self.arm_repeat(device, physical, at);
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
            self.inject(Injected::KeyUp { key, modifiers }, host);
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
                self.inject(
                    Injected::KeyDown {
                        key: tap,
                        modifiers,
                    },
                    host,
                );
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
        self.disarm_if_released();
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
        let modifiers = match crate::modifiers::modifier_of(key).is_some() {
            true => left,
            false => went_down_with,
        };
        self.inject(Injected::KeyUp { key, modifiers }, host);
        if left != modifiers {
            self.inject(Injected::Modifiers(left), host);
        }
    }

    /// Drop the repeat once the key carrying it is no longer down.
    ///
    /// Asked of the held list rather than compared against the key just
    /// released, so that a keyboard being torn out answers it too — and so a
    /// release of some *other* key leaves the repeat running, which is what a
    /// keyboard does.
    fn disarm_if_released(&mut self) {
        if let Some(repeating) = self.repeating {
            if self.held_index(repeating.device, repeating.key).is_none() {
                self.disarm_repeat();
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
                self.inject(
                    Injected::KeyDown {
                        key: hold,
                        modifiers,
                    },
                    host,
                );
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

    /// Let go of everything this run told the OS was down, on the way out.
    ///
    /// Rendered through the ordinary release rather than written as a report
    /// with nothing in it per page: one of those releases a key and the modifier
    /// it was taken with in the same report, and applications read that as the
    /// key having arrived unmodified — a shifted character coming out lower
    /// case. The order below is what avoids it.
    ///
    /// Only what this run said, and not a blanket reset of the device: the
    /// device outlives the process and belongs to a driver, so a report saying
    /// that nothing at all is held would also speak for whatever else has it
    /// open (`docs/platform/macos/output-through-a-virtual-hid-device.md`).
    pub(crate) fn release_everything(&mut self, host: &mut dyn SinkHost) {
        self.release_held(None, host);
        if self.pointer_buttons.any() {
            // Straight to the render rather than through [`Sink::pointer`]: a
            // report with no motion in it and no button left down is one that
            // call has nothing to relay, and the buttons are exactly what has
            // to be said.
            self.pointer_buttons = Buttons::NONE;
            self.inject(Injected::Pointer(PointerReport::default()), host);
        }
    }

    /// Let go of everything a keyboard was holding.
    fn release_all(&mut self, device: DeviceId, host: &mut dyn SinkHost) {
        self.release_held(Some(device), host);
    }

    /// Let go of what is held, by one keyboard or by every one of them.
    ///
    /// In reverse order of press, so a modifier is released after the keys it
    /// was modifying. The other way round hands applications a key-up whose
    /// modifier has already gone, which is how a shifted character ends up
    /// arriving unshifted.
    fn release_held(&mut self, device: Option<DeviceId>, host: &mut dyn SinkHost) {
        let mut index = self.held.len();
        while index > 0 {
            index -= 1;
            if device.is_some_and(|only| self.held[index].device != only) {
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
        self.disarm_if_released();
    }
}

/// Write every report a render produced, on whichever of the device's reports
/// each belongs to, and answer with what the machine said.
///
/// The first failure's number and not the last's: a render is several reports
/// and the one that first would not go is what explains the rest, since the
/// device is in the state that report was refused in.
fn send(rendered: Rendered, writing: &mut dyn favjit_host::sink::Injecting) -> i32 {
    match rendered {
        Rendered::Keyboard(sent) => {
            let mut trouble = 0;
            for one in sent {
                // Every one of them, whether or not the last went: a report
                // left unwritten because an earlier one failed would be a key
                // the OS still believes is down.
                let code = match one {
                    report::Sent::Keyboard(report) => {
                        writing.send_report(OutputReport::Keyboard, &report.bytes())
                    }
                    report::Sent::Control(control) => {
                        writing.send_report(control_report(control.page), &control.bytes())
                    }
                };
                if trouble == 0 {
                    trouble = code;
                }
            }
            trouble
        }
        Rendered::Pointing(report) => {
            writing.send_report(OutputReport::Pointing, &report::pointing(report))
        }
    }
}

/// Nanoseconds per keystroke, one list per segment of the path.
///
/// Every sample and not a summary, because the summary is arithmetic and where
/// it is read is not where it is taken: a run puts these on stdout as it ends
/// and a percentile computed per keystroke would be work in the interactive
/// path.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Latency {
    /// From the HID system stamping a value to the capture reaching it.
    pub arrival: Vec<u64>,
    /// From this loop taking the event out of the wait to the report being
    /// ready to write.
    pub pipeline: Vec<u64>,
    /// The write itself.
    pub post: Vec<u64>,
}

impl Latency {
    /// The two segments this loop measured, with the one the platform stamped.
    ///
    /// Put together as the run ends rather than as each sample is taken, because
    /// the two are read at different places: the arrival is what a wait carried
    /// and the other two are what the write took, and a loop that merged them
    /// per keystroke would be doing that work in the interactive path.
    fn with(self, arrival: Vec<u64>) -> Self {
        Self { arrival, ..self }
    }
}

/// Which of the output device's reports one control page goes out on.
///
/// Written out rather than derived from the page's report id: the ids and the
/// order the platform posts them in are not the same sequence, so a mapping
/// inferred from either number would pair a page with the wrong request
/// (`docs/platform/macos/virtual-hid-device.md`).
fn control_report(page: report::ControlPage) -> OutputReport {
    match page {
        report::ControlPage::Consumer => OutputReport::Consumer,
        report::ControlPage::AppleVendorTopCase => OutputReport::AppleVendorTopCase,
        report::ControlPage::AppleVendorKeyboard => OutputReport::AppleVendorKeyboard,
        report::ControlPage::GenericDesktop => OutputReport::GenericDesktop,
    }
}

fn union(modifiers: Modifiers, key: Key) -> Modifiers {
    match crate::modifiers::modifier_of(key) {
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
    /// from one that is not running (ADR-0012). Ending is what lets whatever
    /// supervises favjit start it again, which is how the socket comes back.
    LinkGone,
    /// This process may not read the keyboards, so there is nothing to convert.
    NoPermission,
    /// There is nowhere to send converted keystrokes.
    NoOutput,
    /// The keyboards could not be taken.
    NoInput,
}

/// How long this loop may go without checking back in on conditions that
/// produce no event of their own — the output going away, a stop being asked
/// for, converting being switched off.
///
/// A bound on the loop and not only on a wait: a wait is given it as its
/// deadline, and a loop that has not waited that long because something kept
/// arriving looks anyway once it has passed ([`LookedIn`]). Bounding the wait
/// alone leaves the looking to a stream going quiet, and a pointer being moved
/// or a key held down is a stream that does not — read off a run that kept the
/// keyboards for as long as the other machine's mouse was moving, converting
/// every report into a connection the service had already closed.
///
/// A constant `engine` states once rather than a host's to choose, since how
/// promptly those are noticed is the same question on every platform.
const RESPONSIVENESS: Duration = Duration::from_millis(250);

/// When this loop last looked at the facts that produce no event of their own.
///
/// Its own record rather than a count of events: how many arrive in a quarter
/// of a second is the keyboard's business, and the bound is on the clock.
#[derive(Debug, Default)]
struct LookedIn {
    at: Option<Instant>,
}

impl LookedIn {
    /// Whether [`RESPONSIVENESS`] has passed since the last look, taking this
    /// one as the new last look if so.
    ///
    /// The first call is due, so a run that comes up under a condition that
    /// ends it learns so on its first event rather than a quarter of a second
    /// in.
    fn due(&mut self, now: Instant) -> bool {
        let due = self
            .at
            .is_none_or(|at| now >= at.saturating_add(RESPONSIVENESS));
        if due {
            self.at = Some(now);
        }
        due
    }
}

/// Wait for the next raw signal, for at most [`RESPONSIVENESS`] or until `timer`,
/// whichever is sooner.
///
/// The one instant this comparison needs that `engine` does not otherwise hold —
/// what the clock reads right now — is asked for first and nowhere else, so the
/// comparison itself is arithmetic over instants already in hand by the time
/// [`Host::next_event`] is called (ADR-0006).
fn wait<H: Host + ?Sized>(
    host: &mut H,
    timer: Option<Instant>,
    facts: &mut Facts,
) -> Option<favjit_host::HostEvent> {
    let now = host.now();
    let responsive = now.saturating_add(RESPONSIVENESS);
    // The soonest of the two, and `RESPONSIVENESS` on its own where there is no
    // timer: a wait bounded only by a repeat that is not due is a run that looks
    // at none of the facts below until one is.
    let deadline = timer.map_or(responsive, |timer| timer.min(responsive));
    facts.took_in(host.next_event(deadline))
}

/// What this run has been told that no keystroke says.
///
/// Held by the run and not by a host, because each of them arrives on the one
/// stream and the run is the end every event reaches: a host that kept them
/// would be keeping what a call answered (ADR-0006). Kept once told rather than
/// asked for again, because none of them is ever undone.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Facts {
    /// Nothing more will arrive on this stream.
    input_gone: bool,
    /// The loop handed over to turn alongside this one has come back.
    alongside_stopped: bool,
    /// How long ago the platform stamped each value that arrived.
    arrival: Vec<u64>,
}

impl Facts {
    /// Take in what one event says, and answer with the ones that are input.
    ///
    /// The three it absorbs are not keystrokes, so a loop handed one would have
    /// to ask which kind it was before it could convert it — and a wait that
    /// carried one is a wait that carried no input, which is what `None` says.
    fn took_in(
        &mut self,
        arrived: Option<favjit_host::HostEvent>,
    ) -> Option<favjit_host::HostEvent> {
        let event = arrived?;
        match &event.kind {
            favjit_host::EventKind::InputGone => {
                self.input_gone = true;
                None
            }
            favjit_host::EventKind::AlongsideStopped => {
                self.alongside_stopped = true;
                None
            }
            favjit_host::EventKind::Delay(nanos) => {
                self.arrival.push(*nanos);
                None
            }
            _ => Some(event),
        }
    }
}

/// The two answers every run needs before it opens anything, or the ending that
/// says it will not.
///
/// A run switched off ends when it is switched on again rather than going on to
/// convert, so that whatever restarts it takes the keyboards from nothing: carrying
/// on here would resume a machine whose state was assembled before the switch.
fn switched_off(host: &mut dyn SinkInputHost) -> Option<Ending> {
    // Its own, and thrown away with this loop: nothing has been read yet, so
    // what a wait here carries is the wait and not a fact about the run.
    let mut waiting = Facts::default();
    while !host.switched_on() {
        if host.stop_requested() {
            break;
        }
        wait(host, None, &mut waiting);
    }
    if !host.switched_on() {
        return Some(Ending::SwitchedOff);
    }
    if !host.may_read_input() && !host.request_input_permission() {
        return Some(Ending::NoPermission);
    }
    None
}

/// Which keyboards this run leaves alone, and how closely it watches the rest.
///
/// A parameter of the run rather than the host's own to read, since which
/// keyboards convert is a rule and a rule is `engine`'s (ADR-0006): a host that
/// read a list from wherever it liked could disagree with what a test asserts
/// against.
#[derive(Debug, Clone, Default)]
pub struct InputConfig {
    /// Keyboards to leave entirely alone — not announced, not converted, not
    /// seized.
    pub ignore: Vec<DeviceMatch>,
    /// Leave the Mac's own keyboard entirely alone.
    ///
    /// Not expressible through `ignore`, which matches on a vendor and product id
    /// the internal keyboard does not have.
    pub skip_built_in: bool,
}

/// One device's pointer, between the element values that describe it.
///
/// A HID report's values arrive one at a time and share a stamp; the OS applies
/// its acceleration per report, so a diagonal relayed as one report per axis is
/// accelerated as two short movements and falls short of the same motion of the
/// thumb (`docs/platform/macos/virtual-hid-device.md`). What ends one report and
/// starts the next is either the stamp changing or the device's queue running dry.
#[derive(Debug, Default, Clone, Copy)]
struct Pointing {
    buttons: Buttons,
    pending: PointerReport,
    stamp: u64,
    open: bool,
}

impl Pointing {
    fn take(&mut self) -> Option<PointerReport> {
        if !self.open {
            return None;
        }
        let report = PointerReport {
            buttons: self.buttons,
            ..self.pending
        };
        self.pending = PointerReport::default();
        self.open = false;
        Some(report)
    }
}

/// Turns what a host reports into what a role's loop dispatches on.
///
/// The state a resolution needs and cannot be asked for again: which devices this
/// run has decided to read, and each one's pointer, assembled from element values
/// that arrive one at a time. Neither belongs to [`Sink`] — a checkpoint describes
/// converted state, not the hardware a fresh process has not opened yet.
struct Resolver {
    config: InputConfig,
    exclusive: bool,
    pointers: Vec<(DeviceId, Pointing)>,
}

impl Resolver {
    fn new(config: InputConfig, exclusive: bool) -> Self {
        Self {
            config,
            exclusive,
            pointers: Vec::new(),
        }
    }

    fn pointing(&mut self, device: DeviceId) -> &mut Pointing {
        if let Some(at) = self.pointers.iter().position(|(id, _)| *id == device) {
            return &mut self.pointers[at].1;
        }
        self.pointers.push((device, Pointing::default()));
        &mut self.pointers.last_mut().unwrap().1
    }

    /// A keyboard the host found. Decide whether to read it, and say what the
    /// role's loop should make of that.
    fn found(
        &mut self,
        info: DeviceInfo,
        host: &mut dyn SinkInputHost,
        reading: &mut dyn favjit_host::sink::Capturing,
    ) -> Option<EventKind> {
        if (self.config.skip_built_in && info.is_built_in)
            || self.config.ignore.iter().any(|m| m.matches(&info))
        {
            return None;
        }
        let code = reading.take_device(info.id, self.exclusive);
        if code != 0 {
            host.warn(format_args!(
                "cannot open vendor={:?} product={:?}: {code:#010x}",
                info.vendor_id, info.product_id
            ));
            return None;
        }
        if !reading.read_device(info.id) {
            return None;
        }
        Some(EventKind::DeviceAttached(info))
    }

    /// One HID element value. Assembles a pointer report across the values of one
    /// report, and names a keyboard usage — either is at most one resolved event,
    /// since a pointer value only ever closes the *previous* report.
    fn value(
        &mut self,
        device: DeviceId,
        page: u32,
        usage: u32,
        stamp: u64,
        value: i64,
        host: &mut dyn SinkInputHost,
    ) -> Option<EventKind> {
        if crate::hid::usage::pointer(page, usage) {
            let pointing = self.pointing(device);
            let closed = if pointing.open && stamp != pointing.stamp {
                pointing.take()
            } else {
                None
            };
            let pointing = self.pointing(device);
            pointing.stamp = stamp;
            pointing.open = true;
            if page == crate::hid::page::GENERIC_DESKTOP {
                match usage {
                    crate::hid::usage::POINTER_X => pointing.pending.dx = value as i32,
                    crate::hid::usage::POINTER_Y => pointing.pending.dy = value as i32,
                    crate::hid::usage::POINTER_WHEEL => {
                        pointing.pending.vertical_wheel = value as i32
                    }
                    _ => {}
                }
            } else {
                pointing.buttons = pointing.buttons.set(usage as u8, value != 0);
            }
            return closed.map(|report| EventKind::Pointer { device, report });
        }

        let down = match value {
            0 => false,
            1 => true,
            _ => return None,
        };
        match crate::hid::usage::named(page, usage) {
            Some(key) if down => Some(EventKind::KeyDown { device, key }),
            Some(key) => Some(EventKind::KeyUp { device, key }),
            None => {
                if down {
                    host.warn(format_args!(
                        "no key for page {page:#x} usage {usage:#x} on device {}",
                        device.0
                    ));
                }
                None
            }
        }
    }

    /// The device's queue ran dry: draining is as certain a report boundary as
    /// the stamp changing, and it is the only one the last report of a burst
    /// gets.
    fn drained(&mut self, device: DeviceId) -> Option<EventKind> {
        self.pointing(device)
            .take()
            .map(|report| EventKind::Pointer { device, report })
    }
}

/// Turn one raw signal into what it means, or nothing when it means nothing on
/// its own — a pointer value still being assembled, an unread frame that will
/// not decode, a usage no table names.
///
/// The one place a [`favjit_host::EventKind`] becomes an [`EventKind`], reached
/// from every entry point below so that input relayed from the other machine
/// resolves the same way as input read here (ADR-0006).
fn resolve(
    raw: favjit_host::HostEvent,
    resolver: &mut Resolver,
    host: &mut dyn SinkInputHost,
    reading: &mut dyn favjit_host::sink::Capturing,
) -> Option<HostEvent> {
    let kind = match raw.kind {
        favjit_host::EventKind::HidDeviceFound {
            device,
            primary_usage_page,
            primary_usage,
            transport,
            product,
            vendor_id,
            product_id,
        } => {
            // Nothing said about a device that is not a keyboard: the machine
            // reports every one it finds, so a mouse arriving here is the
            // ordinary case rather than something to warn about.
            let info = crate::device::from_hid_properties(
                device,
                primary_usage_page,
                primary_usage,
                transport.as_deref(),
                product.as_deref(),
                vendor_id,
                product_id,
            )?;
            resolver.found(info, host, reading)?
        }
        // A sink's own host speaks HID: a path is how the machine input is
        // forwarded from names a device, and it arrives over the link already
        // read rather than as this kind.
        favjit_host::EventKind::PathDeviceFound { .. } => return None,
        favjit_host::EventKind::DeviceLost(id) => {
            resolver.pointers.retain(|(known, _)| *known != id);
            EventKind::DeviceDetached(id)
        }
        favjit_host::EventKind::HidValue {
            device,
            page,
            usage,
            stamp,
            value,
        } => resolver.value(device, page, usage, stamp, value, host)?,
        favjit_host::EventKind::HidValuesDone { device } => resolver.drained(device)?,
        // Windows-only: never reported by a sink's own host, which speaks HID and
        // not scancodes.
        favjit_host::EventKind::HookedKey { .. } => return None,
        favjit_host::EventKind::Record { frame: bytes, .. } => {
            let frame: [u8; crate::link::FRAME] = bytes.try_into().ok()?;
            crate::link::from_the_peer(crate::link::Message::decode(&frame)?)?
        }
        favjit_host::EventKind::Probe => EventKind::Probe,
        // `favjit_host::EventKind` is `#[non_exhaustive]`: a kind this crate does
        // not yet name resolves to nothing rather than failing the whole stream.
        _ => return None,
    };
    Some(HostEvent::new(raw.at, kind))
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
    let Some(mut reading) = host.look_for_devices(Box::new(|turning| {
        crate::capture::convert(turning, CAPTURE_POLL)
    })) else {
        return Ending::NoInput;
    };
    // Nothing is written: this mode exists to say where a key reports, so a
    // report on its way out would be a keystroke a run that injects nothing
    // delivered.
    let mut resolver = Resolver::new(InputConfig::default(), false);
    let mut beating = Beating::default();
    let mut facts = Facts::default();
    let mut looked = LookedIn::default();
    let ended = loop {
        match wait(host, None, &mut facts) {
            Some(raw) => {
                resolve(raw, &mut resolver, host, reading.as_mut());
                beating.beat(host);
                if looked.due(host.now()) {
                    if let Some(ended) = ended_meanwhile(host, false, &facts) {
                        break ending(ended);
                    }
                }
            }
            None => {
                if let Some(ended) = ended_meanwhile(host, false, &facts) {
                    break ending(ended);
                }
            }
        }
    };
    release_devices(reading.as_mut());
    ended
}

/// Why the stream ended, asked one fact at a time: once a wait has come back
/// empty, and once [`RESPONSIVENESS`] has passed under events that kept it from
/// doing so.
///
/// `engine`'s to prioritise, over answers each fact-query gave on its own — no
/// host operation combines any two of them (ADR-0006). `output_opened` says
/// whether asking about the output means anything yet: a run that never opened
/// one has nothing there to have gone.
fn ended_meanwhile(
    host: &mut dyn SinkInputHost,
    output_opened: bool,
    facts: &Facts,
) -> Option<Ended> {
    if output_opened && !host.output_connected() {
        return Some(Ended::OutputGone);
    }
    if facts.alongside_stopped {
        return Some(Ended::AlongsideStopped);
    }
    if host.stop_requested() {
        return Some(Ended::AsAsked);
    }
    if !host.switched_on() {
        return Some(Ended::SwitchedOff);
    }
    None
}

/// Say what a person has to do about an output device that did not come up.
///
/// Here rather than beside the socket, because the two answers ask for different
/// things: one is a privilege or a missing package and the other is a driver that
/// never activated, and a run that said the same sentence for both would send
/// somebody looking in the wrong place (ADR-0006, `docs/platform/macos/output-through-a-virtual-hid-device.md`).
fn say_why_there_is_no_output(trouble: NoOutput, host: &mut dyn SinkHost) {
    match trouble {
        NoOutput::NoService => host.warn(format_args!(
            "cannot reach the virtual HID device service; it needs root, and the \
             Karabiner-DriverKit-VirtualHIDDevice package installed"
        )),
        NoOutput::NotReady {
            driver_activated,
            driver_connected,
            version_mismatched,
        } => host.warn(format_args!(
            "the virtual HID device service never reported a ready keyboard \
             (driver_activated={driver_activated} driver_connected={driver_connected} \
             version_mismatched={version_mismatched})"
        )),
        // `NoOutput` is `#[non_exhaustive]`: a reason this crate does not yet
        // name is still a device that did not come up, with nothing more
        // specific to say about it than that.
        _ => host.warn(format_args!(
            "the device converted input goes out through did not come up"
        )),
    }
}

/// Say what the output pointer was set to (ADR-0011).
///
/// The sequence is [`crate::pointer::tune`]'s, so a run and the diagnostic mode
/// that tunes without bringing one up cannot come to write these in a different
/// order. What is left here is the saying, which differs between the two.
fn tune_the_output(host: &mut dyn SinkHost) {
    for tuned in crate::pointer::tune(host) {
        host.warn(format_args!(
            "output pointer {:?}: resolution {:?} -> {:?}, acceleration {:?} -> {:?} (in {}){}",
            tuned.name.as_deref().unwrap_or("(unnamed)"),
            tuned.resolution.0,
            tuned.resolution.1,
            tuned.acceleration.0,
            tuned.acceleration.1,
            tuned.key,
            if tuned.accepted { "" } else { " (refused)" }
        ));
    }
}

/// Give back each keyboard held exclusively, stopping at the first failed close.
fn release_devices(reading: &mut dyn favjit_host::sink::Capturing) {
    while let Some(held) = reading.next_held_device() {
        let code = reading.give_it_back(held.at);
        // Let go of it only where the close went, and stop there either way: a
        // list that lost an entry the machine still holds is a keyboard nothing
        // will try for again, and a loop that carried on would ask for the same
        // one forever.
        reading.forget_what_was_given_back(held.at, code);
        if code != 0 {
            return;
        }
    }
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
    input: InputConfig,
    host: &mut dyn SinkHost,
    trace: Option<&mut [u8]>,
) -> (Ending, Latency) {
    let settings = settings.into();
    let ended = match trace {
        // Recorded when a supervisor provided the memory, and not at all otherwise:
        // a trace this process allocated for itself would be lost in exactly the
        // failures it exists for, and it would be a keylog nobody asked for
        // (ADR-0009).
        //
        // The recording wraps the host for the whole run and not only for the loop
        // that converts: what a trace is worth rests on it seeing everything that
        // crossed the boundary, and a run that opened the output and took the
        // keyboards around it would leave the recording unable to say why a run
        // that converted nothing did not.
        Some(bytes) => {
            let mut trace = Trace::new(bytes);
            let mut recorder = Recording {
                host,
                trace: &mut trace,
                since_checkpoint: 0,
                checkpoint_at: None,
            };
            match bring_up(request, &mut recorder) {
                Err(stopped) => (stopped, Latency::default()),
                Ok(brought) => {
                    let mut reading = brought.reading;
                    let (over, latency) = convert_traced(
                        layout,
                        settings,
                        input,
                        brought.suppress,
                        &mut recorder,
                        reading.as_mut(),
                        brought.writing,
                    );
                    release_devices(reading.as_mut());
                    (ending(over), latency)
                }
            }
        }
        None => match bring_up(request, host) {
            Err(stopped) => (stopped, Latency::default()),
            Ok(brought) => {
                let mut reading = brought.reading;
                let (over, latency) = convert(
                    layout,
                    settings,
                    input,
                    brought.suppress,
                    host,
                    reading.as_mut(),
                    brought.writing,
                );
                release_devices(reading.as_mut());
                (ending(over), latency)
            }
        },
    };
    ended
}

/// Bring the machine up in the order ADR-0008 requires, and say whether the
/// keyboards are to be held exclusively — or which ending stopped the run
/// before there was anything to convert.
///
/// Its own function rather than the top of [`run`], so that a run recording a
/// trace and a run recording none reach it the same way: the one difference
/// between them is which host they are given, and a bring-up written once is
/// one that cannot come to differ between them.
/// How often the capture started by [`SinkInputHost::look_for_devices`] checks
/// for a device request or a watchdog probe between one arriving and the next.
///
/// Short enough that neither waits behind the other for long, on a thread with
/// nothing else to interrupt it: the run loop it turns is its own, and nothing
/// but this cadence brings it back round to look.
const CAPTURE_POLL: Duration = Duration::from_millis(50);

/// How long to wait for the output device to become ready.
///
/// Long enough for a driver that has to activate, short enough that a machine
/// without the package installed says so rather than appearing to hang. There is
/// nothing else to wait for: the service reports readiness, and a wrong protocol
/// version is silently ignored, so a timeout is the only signal it will ever give
/// (`docs/platform/macos/virtual-hid-device.md`).
const OUTPUT_WAIT: Duration = Duration::from_secs(5);

/// How long one read spends waiting while the output device is still being
/// waited for.
///
/// The device reports readiness when it has it, so this only bounds how long
/// after that the run notices — short enough not to add to a start-up a person
/// is watching, long enough not to spin.
const OUTPUT_READY_POLL: Duration = Duration::from_millis(20);

/// How often the connection to the output device is kept alive, as the
/// service's own other clients do it.
///
/// Posting reports is no substitute: a session where nothing is typed would go
/// quiet and be torn down for it.
const OUTPUT_HEARTBEAT: Duration = Duration::from_secs(3);

/// How long one write to the output device may take.
///
/// A write that cannot complete this quickly is a write that would stall the
/// one loop, and a stalled loop holding a seize is what ADR-0008 exists to
/// prevent. Losing a report is the lesser failure and it is reported.
const OUTPUT_WRITE_TIMEOUT: Duration = Duration::from_millis(50);

/// Open the output device, ask it for the two devices a run sends through, and
/// wait for it to say both are up.
///
/// Driven a call at a time on the connection this holds, rather than waited for
/// inside the machine: which requests go out, what the answers mean, and how
/// long they are allowed are the run's, and a machine that decided any of them
/// would be deciding what a person is told about a driver that never activated
/// (ADR-0006, `docs/platform/macos/output-through-a-virtual-hid-device.md`).
///
/// The bound is arithmetic over instants the machine reports rather than a clock
/// of `engine`'s own (ADR-0010): each read is one bounded wait the host makes,
/// and what says the bound has passed is [`Host::now`] against the instant this
/// started from.
///
/// Read here and not on the run's own stream, because nothing on that stream is
/// about the output: an event delivered to it would be one the converting loop
/// has to read past, and every keyboard event scripted ahead of the first one
/// this waited for would be a keystroke a run that had not yet looked for its
/// devices threw away.
///
/// Handed over to the loop that serves it last, once it is up: the frames that
/// prove it are read here, and a loop turning behind this one would take them
/// out of the wait's own hands.
fn bring_up_the_output(
    host: &mut dyn SinkHost,
) -> Result<Box<dyn favjit_host::sink::Injecting>, NoOutput> {
    let mut reaching = host.reach_the_output()?;
    // Before its two ends are taken and not after: the writing end is what a
    // report goes out on, and one taken before the bound was set is one a
    // converting run can block on for as long as the service feels like.
    if !reaching.do_not_block_writes(OUTPUT_WRITE_TIMEOUT) {
        return Err(NoOutput::NoService);
    }
    let Some(favjit_host::sink::Opened {
        serving: mut output,
        writing,
    }) = reaching.both_ends()
    else {
        return Err(NoOutput::NoService);
    };
    if !output.set_read_timeout(OUTPUT_READY_POLL) {
        return Err(NoOutput::NoService);
    }
    // Before anything is waited for: readiness arrives as the answer to one of
    // these, and a wait entered before they went out is a wait for nothing
    // (`docs/platform/macos/output-through-a-virtual-hid-device.md`).
    if !crate::output::ask_for_the_devices(&mut *output) {
        return Err(NoOutput::NoService);
    }

    let started = host.now();
    let mut status = crate::output::Status::default();
    while !status.ready() {
        if crate::output::one_frame(&mut *output, &mut status).is_some() {
            return Err(status.why_not());
        }
        if host.now().saturating_duration_since(started) >= OUTPUT_WAIT {
            return Err(status.why_not());
        }
    }

    match host.run_output_alongside(Box::new(move || {
        crate::output::serve(&mut *output, OUTPUT_HEARTBEAT)
    })) {
        true => Ok(writing),
        // A device up with nobody answering for it is a connection the service
        // tears down as soon as it asks anything, so this is the output not
        // being there rather than a run carrying on without a loop.
        false => Err(NoOutput::NoService),
    }
}

/// What a bring-up left the run holding.
struct Brought {
    /// Whether the keyboards are held exclusively.
    suppress: bool,
    /// The loop reading them.
    reading: Box<dyn favjit_host::sink::Capturing>,
    /// Where the converted keystrokes go.
    writing: Box<dyn favjit_host::sink::Injecting>,
}

fn bring_up(request: &Request, host: &mut dyn SinkHost) -> Result<Brought, Ending> {
    if let Some(off) = switched_off(host) {
        return Err(off);
    }

    // Before anything is opened or taken, because a cost named after the keyboards
    // are held is one the person can no longer decide about.
    warn_about(request, host);

    // The keyboards are taken exclusively only by a run that delivers somewhere, and
    // it is the shape of the request that makes that so rather than a check here:
    // `suppress` is a field of the variant that injects, so there is no request in
    // which it is set and nothing is being delivered (ADR-0008).
    let (suppress, writing) = match request {
        // Nothing is opened, which is the whole of what this mode is: with
        // nothing suppressed a run that injected would deliver every keystroke
        // twice, and a virtual keyboard left behind is a change outside a
        // process that exists to make none.
        Request::DryRun => (false, host.instead_of_the_output()),
        Request::Injecting { listen } => {
            let writing = match bring_up_the_output(host) {
                Ok(writing) => writing,
                Err(trouble) => {
                    say_why_there_is_no_output(trouble, host);
                    return Err(Ending::NoOutput);
                }
            };
            // After the device is up, because until then there is no service to
            // carry the properties.
            tune_the_output(host);
            // After the output too: input let in from the other machine before there
            // is anywhere to send it would be converted into nothing, which is why
            // listening is a property of injecting rather than a flag beside it
            // (ADR-0012).
            if *listen {
                // Established here rather than left to the host, so that the
                // sequence deciding what this machine presents is the same one the
                // suite drives (ADR-0012). Only for a run that will listen: an
                // identity is a file written for a link, and a run that opens no
                // socket should leave none behind.
                match pairing::identity(host) {
                    Ok(identity) => {
                        // The socket goes with the loop that serves it: a link left
                        // bound with nothing turning would take the connection and
                        // never answer, which from the other machine is worse than
                        // a machine that is not there.
                        match host.bind_link() {
                            Some(mut link) => {
                                host.run_alongside(Box::new(move || {
                                    crate::link::serve(&identity, &mut *link)
                                }));
                            }
                            // Not passed over in silence, and not left to the
                            // machine that could not listen either: a run whose
                            // socket never bound looks from the other machine
                            // exactly like one that is refusing, and the only
                            // end that can say which is this one (ADR-0006).
                            None => host.warn(format_args!(
                                "nothing is listening for the other machine, so no link"
                            )),
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
            (true, writing)
        }
    };

    let Some(reading) = host.look_for_devices(Box::new(|turning| {
        crate::capture::convert(turning, CAPTURE_POLL)
    })) else {
        return Err(Ending::NoInput);
    };

    Ok(Brought {
        suppress,
        reading,
        writing,
    })
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
        // `Ended` is `#[non_exhaustive]`: a reason this crate does not yet name
        // is still a stream that ended, with nothing more specific to say about
        // it than that.
        _ => Ending::Converted,
    }
}

/// Convert whatever arrives, until nothing more will.
///
/// One loop over one event stream, which is what makes a trace replayable and
/// the end-to-end suite deterministic (ADR-0006). A run reaches this through
/// [`run`]; a replay drives it directly, because a recording is not a machine to
/// bring up.
fn convert(
    layout: Layout,
    settings: impl Into<Settings>,
    input: InputConfig,
    exclusive: bool,
    host: &mut dyn SinkHost,
    reading: &mut dyn favjit_host::sink::Capturing,
    writing: Box<dyn favjit_host::sink::Injecting>,
) -> (Ended, Latency) {
    let mut sink = Sink::new(layout, settings.into(), writing);
    let mut resolver = Resolver::new(input, exclusive);
    let mut beating = Beating::default();
    let mut facts = Facts::default();
    let mut looked = LookedIn::default();
    let ended = loop {
        let timer = sink.current_timer();
        match wait(host, timer, &mut facts) {
            Some(raw) => {
                let Some(event) = resolve(raw, &mut resolver, host, reading) else {
                    continue;
                };
                sink.handle(event, host);
                // After handling, not before: a heartbeat sent on the way in
                // would vouch for a loop that is about to wedge inside
                // `handle`, which is one of the failures ADR-0008 exists to
                // catch.
                beating.beat(host);
                // Also after handling: an event already taken off the stream is
                // converted rather than dropped, since a release dropped here
                // is a key the machine believes is still down.
                if looked.due(host.now()) {
                    if let Some(ended) = ended_meanwhile(host, exclusive, &facts) {
                        break ended;
                    }
                }
            }
            None => match due(host, timer) {
                Some(event) => {
                    sink.handle(event, host);
                    beating.beat(host);
                    if looked.due(host.now()) {
                        if let Some(ended) = ended_meanwhile(host, exclusive, &facts) {
                            break ended;
                        }
                    }
                }
                None => {
                    if let Some(ended) = ended_meanwhile(host, exclusive, &facts) {
                        break ended;
                    }
                }
            },
        }
    };
    // Here rather than after the keyboards go back: this is the last thing the
    // person is typing on being told anything, and a key given back while the
    // OS still believes a modifier is down is that modifier held over whatever
    // they type next.
    sink.release_everything(host);
    (ended, sink.latency.with(facts.arrival))
}

/// The repeat's wake-up, if this wait ended because `timer` was reached rather
/// than because nothing arrived by [`RESPONSIVENESS`].
///
/// Asked once a wait has come back empty rather than folded into the wait
/// itself, since which of the two explains an empty wait is `engine`'s to tell
/// apart, not a host operation's (ADR-0006).
fn due(host: &mut dyn Host, timer: Option<Instant>) -> Option<HostEvent> {
    let due = timer?;
    (host.now() >= due).then(|| HostEvent::new(due, EventKind::Timer))
}

/// The same, from the state a trace's checkpoint recorded.
///
/// What makes a bounded trace replayable: the records that survive eviction
/// describe changes to a state, and this is that state
/// (`docs/adr/0009-trace-and-replay.md`).
fn convert_from(
    layout: Layout,
    settings: impl Into<Settings>,
    checkpoint: &Checkpoint,
    events: &[HostEvent],
    host: &mut dyn SinkHost,
    writing: Box<dyn favjit_host::sink::Injecting>,
) {
    let mut sink = Sink::from_checkpoint(layout, settings.into(), checkpoint, writing);
    let mut beating = Beating::default();
    for &event in events {
        sink.handle(event, host);
        beating.beat(host);
    }
    // The reproduction ends the way the run did, and for the same reason: a
    // replay injects into the machine it is run on, so a key the trace left
    // down would be one the person is now holding.
    sink.release_everything(host);
}

/// Replay what a trace holds after one of its checkpoints (ADR-0009).
///
/// The one entry point a replay drives, mirroring what [`run`] is for a live
/// machine: a trace already carries `engine`'s own decoded stream, so all a
/// replay decides is *where it starts*.
///
/// `from` numbers the checkpoints the retained window holds, oldest first
/// ([`crate::trace::Reader::checkpoints`] says how many there are). A trace is
/// therefore not one case but one per checkpoint: the oldest reproduces as much
/// of the run as survived, and a later one reproduces the end of it from a state
/// closer to it — which is what makes a long window worth keeping and a
/// half-second reproduction of the same incident possible.
pub fn replay(
    layout: Layout,
    settings: impl Into<Settings>,
    trace: &crate::trace::Reader<'_>,
    from: usize,
    host: &mut dyn SinkHost,
    writing: Box<dyn favjit_host::sink::Injecting>,
) -> Replayed {
    let Some((checkpoint, events)) = trace.checkpoint(from) else {
        return Replayed::NoSuchCheckpoint;
    };
    convert_from(layout, settings, &checkpoint, &events, host, writing);
    Replayed::Events(events.len())
}

/// What a replay was given to work with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Replayed {
    /// The events after that checkpoint, every one of them handled.
    Events(usize),
    /// The trace holds no checkpoint with that number, so nothing was replayed
    /// and nothing was injected — what asking for a window that has since been
    /// evicted past looks like.
    NoSuchCheckpoint,
}

/// The same, recording everything into a trace as it goes.
///
/// The recording wraps the host rather than living inside the sink, so that what
/// is written is exactly what crossed the boundary: a sink that recorded its own
/// intentions could record one thing and do another, and the whole value of a
/// trace is that replaying it cannot disagree with the run.
fn convert_traced(
    layout: Layout,
    settings: Settings,
    input: InputConfig,
    exclusive: bool,
    recorder: &mut Recording<'_, '_, '_>,
    reading: &mut dyn favjit_host::sink::Capturing,
    writing: Box<dyn favjit_host::sink::Injecting>,
) -> (Ended, Latency) {
    let mut sink = Sink::new(layout, settings, writing);
    let mut resolver = Resolver::new(input, exclusive);
    let mut beating = Beating::default();
    let mut facts = Facts::default();
    let mut looked = LookedIn::default();
    recorder.checkpoint(&sink, None);

    let ended = loop {
        let timer = sink.current_timer();
        // Through the recorder and not through the host inside it, every call:
        // what a trace is worth rests on the recording seeing everything that
        // crossed the boundary, and a loop that reached past it for the reads
        // would leave the recording able to say only half of what happened —
        // silently, and only for the calls nothing had thought to route.
        match wait(&mut *recorder, timer, &mut facts) {
            Some(raw) => {
                // Before it is resolved, because the number belongs to the record
                // that arrived and resolving turns that into a keystroke, which
                // carries no such thing: what the two machines' recordings are
                // lined up on is this (ADR-0009).
                match &raw.kind {
                    favjit_host::EventKind::Record { at, .. } => {
                        recorder.record(Record::Received { at: *at })
                    }
                    // The one record that says this end let go, which is half of
                    // what a reading of the link answers.
                    favjit_host::EventKind::LinkEnded { why } => {
                        recorder.record(Record::LinkEnded { why: *why })
                    }
                    // The other machine's, put in as it wrote them.
                    favjit_host::EventKind::Recorded(bytes) => {
                        recorder.record_from_the_source(bytes)
                    }
                    // What the loop serving the output device put on its
                    // connection, and the way it came back — a frame this run
                    // does not act on, so it is recorded here and nowhere else.
                    // A frame number this crate has no name for is left out
                    // rather than recorded as a number: what a reading is for is
                    // telling the frames apart.
                    favjit_host::EventKind::OutputFrame { frame, code } => {
                        if let Some(frame) = crate::trace::Served::from_number(*frame) {
                            recorder.record(Record::Served { frame, code: *code })
                        }
                    }
                    favjit_host::EventKind::OutputEnded { why } => {
                        recorder.record(Record::OutputEnded { why: *why })
                    }
                    _ => {}
                }
                let Some(event) = resolve(raw, &mut resolver, &mut *recorder, reading) else {
                    continue;
                };
                handle_traced(&mut sink, recorder, &mut beating, event);
                if looked.due(recorder.now()) {
                    if let Some(ended) = ended_meanwhile(&mut *recorder, exclusive, &facts) {
                        break ended;
                    }
                }
            }
            None => match due(&mut *recorder, timer) {
                Some(event) => {
                    handle_traced(&mut sink, recorder, &mut beating, event);
                    if looked.due(recorder.now()) {
                        if let Some(ended) = ended_meanwhile(&mut *recorder, exclusive, &facts) {
                            break ended;
                        }
                    }
                }
                None => {
                    if let Some(ended) = ended_meanwhile(&mut *recorder, exclusive, &facts) {
                        break ended;
                    }
                }
            },
        }
    };
    // Through the recorder, like every other call this loop makes: a release
    // written straight to the host would be reports a replay of this trace
    // never makes, so the reproduction would end holding what the run let go
    // of.
    sink.release_everything(recorder);
    (ended, sink.latency.with(facts.arrival))
}

/// Dispatch one resolved event through the sink, checkpointing and recording
/// around it exactly as [`convert_traced`]'s loop does whichever way the event
/// arrived — read off the stream, or the repeat's own wake-up.
fn handle_traced(
    sink: &mut Sink,
    recorder: &mut Recording<'_, '_, '_>,
    beating: &mut Beating,
    event: HostEvent,
) {
    // The checkpoint goes in *before* the event that prompted it, and the loop
    // writes the event itself for that reason. Recorded the other way round,
    // the event would sit in the segment being left behind: eviction would drop
    // it while keeping the checkpoint taken before it happened, and the replay
    // would be missing one event with nothing to say so.
    if recorder.checkpoint_is_due(event.at) {
        recorder.checkpoint(sink, Some(event.at));
    }
    recorder.record(Record::Event(event));

    sink.handle(event, recorder);
    for &(injected, rendered, code) in sink.pending_injections() {
        recorder.record(Record::Injected {
            injected,
            rendered,
            code,
        });
    }
    beating.beat(recorder);
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

    /// One of the other machine's records, whatever width it turns out to be.
    ///
    /// A frame carries exactly one record's worth, so anything else is a peer
    /// speaking a format this end does not have — dropped rather than padded,
    /// since a record made up to the right length would read as something the
    /// other machine never wrote (ADR-0009).
    fn record_from_the_source(&mut self, bytes: &[u8]) {
        let Ok(record) = <[u8; crate::trace::RECORD]>::try_from(bytes) else {
            return;
        };
        self.trace.push_from_the_source(record);
        self.since_checkpoint += 1;
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

    fn may_read_input(&mut self) -> bool {
        self.host.may_read_input()
    }

    fn request_input_permission(&mut self) -> bool {
        self.host.request_input_permission()
    }

    fn output_connected(&mut self) -> bool {
        self.host.output_connected()
    }

    fn stop_requested(&mut self) -> bool {
        self.host.stop_requested()
    }

    fn look_for_devices(
        &mut self,
        work: favjit_host::capture::SinkLoop,
    ) -> Option<Box<dyn favjit_host::sink::Capturing>> {
        self.host.look_for_devices(work)
    }
}

impl Host for Recording<'_, '_, '_> {
    fn now(&mut self) -> Instant {
        self.host.now()
    }

    /// Passed through without recording: the raw stream is not what a replay
    /// reproduces — [`convert_traced`] records what it resolves from this, which
    /// is what [`crate::trace::Trace::events`] hands back.
    fn next_event(&mut self, deadline: Instant) -> Option<favjit_host::HostEvent> {
        self.host.next_event(deadline)
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

    fn heartbeat(&mut self) -> Result<(), favjit_host::Trouble> {
        // Not recorded: it is the one outbound call that says nothing about what
        // the loop did, only that it came back round, and a replay that produced
        // heartbeats in different places would still be the same run.
        self.host.heartbeat()
    }
}

impl Entropy for Recording<'_, '_, '_> {
    fn fill(&mut self, into: &mut [u8]) -> bool {
        self.host.fill(into)
    }
}

/// Passed through and not recorded: a trace is a recording of the loop, and the
/// identity is settled before the first event arrives — a replay from it would be
/// establishing an identity for a link that is not being served.
impl IdentityStore for Recording<'_, '_, '_> {
    fn read(&mut self) -> Option<Vec<u8>> {
        self.host.read()
    }

    fn make_directory(&mut self) -> Result<(), favjit_host::Trouble> {
        self.host.make_directory()
    }

    fn open(&mut self) -> Result<Box<dyn favjit_host::Writing>, favjit_host::Trouble> {
        self.host.open()
    }
}

/// Not recorded: nothing here is read from the stream a replay drives, so a
/// trace that carried it would be describing a machine's settings rather than
/// what happened to it (ADR-0009).
impl favjit_host::PointerHost for Recording<'_, '_, '_> {
    fn wanted_pointer_feel(&mut self) -> (Option<f64>, Option<f64>) {
        self.host.wanted_pointer_feel()
    }

    fn output_vendor(&mut self) -> i64 {
        self.host.output_vendor()
    }

    fn open_event_system(&mut self) -> Option<Box<dyn favjit_host::Pointers>> {
        self.host.open_event_system()
    }

    fn open_simple_event_system(&mut self) -> Option<Box<dyn favjit_host::Pointers>> {
        self.host.open_simple_event_system()
    }
}

impl SinkHost for Recording<'_, '_, '_> {
    fn reach_the_output(&mut self) -> Result<Box<dyn favjit_host::sink::Reaching>, NoOutput> {
        self.host.reach_the_output()
    }

    fn run_output_alongside(&mut self, work: Box<dyn FnOnce() + Send>) -> bool {
        self.host.run_output_alongside(work)
    }

    fn bind_link(&mut self) -> Option<Box<dyn favjit_host::link::LinkHost + Send>> {
        self.host.bind_link()
    }

    fn run_alongside(&mut self, work: Box<dyn FnOnce() + Send>) -> bool {
        self.host.run_alongside(work)
    }

    /// Not recorded: what a trace holds is one `Injected` and whether it
    /// rendered, read off [`Sink::pending_injections`] once per event, so the
    /// raw reports a write carries are not the recording's business either way.
    fn instead_of_the_output(&mut self) -> Box<dyn favjit_host::sink::Injecting> {
        self.host.instead_of_the_output()
    }
}
