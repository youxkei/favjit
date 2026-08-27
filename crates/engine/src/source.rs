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

pub use favjit_host::source::{
    Answering, Answers, Arrived, At, Connected, Kind, Listing, SourceHost, Suppressing,
};
use favjit_noise::{Initiator, Session};

use crate::link::{Message, FRAME};
use crate::pairing::{self, Identity};
use crate::supervision::Beating;
use crate::trace::{Record, Trace};

use crate::{Buttons, DeviceId, DeviceInfo, EventKind, HostEvent, Key, PointerReport};

/// How long to look for the sink over mDNS before saying it is not there.
///
/// [`crate::supervision::BEAT_WITHIN`] and not a value of its own, because this wait is
/// taken in one piece: a lookup that outlasted what a supervisor allows would have the
/// run ended every time the other machine was slow to answer. mDNS on the network the
/// two machines share answers in milliseconds, so what this bounds is the case where
/// nothing is going to answer at all.
const LOOK: core::time::Duration = crate::supervision::BEAT_WITHIN;

/// How long to wait for the sink to answer a handshake.
///
/// Also one piece, and also bounded by what a supervisor allows. It is reached only once
/// the other machine has answered mDNS, so what it waits for is a machine that is there.
const HANDSHAKE_WAIT: core::time::Duration = crate::supervision::BEAT_WITHIN;

/// How long to wait before looking again after a failed attempt.
///
/// One beat, because the looking is itself what waits: a question the other machine has
/// two seconds to answer is already not a spin, and what this adds is the gap before the
/// next one. Longer, and a person who pressed the chord while the first question went
/// unanswered sits out both — which is the delay they see, since nothing about the chord
/// is slow.
const RETRY: core::time::Duration = crate::supervision::BEAT_WITHIN;

/// How long a keystroke may sit in the socket before the link counts as gone.
///
/// A wait the relaying loop takes between beats, so it is bounded like the two above.
const SEND_WAIT: core::time::Duration = crate::supervision::BEAT_WITHIN;

/// How often the capture started by [`SourceHost::take_input`] checks for a
/// watchdog probe.
///
/// Well inside the silence a watchdog allows, so an idle machine answers
/// several times over before its bound is reached — and slow enough that an
/// idle machine is not being woken for nothing.
const PROBE_TICK: core::time::Duration = core::time::Duration::from_millis(100);

/// The largest raw input [`SourceHost::take_input`] asks the platform for.
///
/// A mouse's is the bigger of the two at a header plus twenty-four bytes, and
/// only mice and keyboards are registered for.
const RAW_BYTES: usize = 128;

/// How long any of this run's waits lasts before it comes back round.
///
/// Every wait, and not only the ones with a bound to notice: the facts that end a
/// run — a bound passing, the capture stopping — produce no event of their own, so
/// a wait that parked until something arrived would be a run that never looks at
/// them. A value favjit picked, which every platform would have accepted another
/// of, so it is stated here where a test can pick a different one and not in the
/// hosts (ADR-0006).
pub const RUN_BOUND_POLL: core::time::Duration = core::time::Duration::from_millis(250);

/// Hand one record this recording has not sent yet to the link, and answer with
/// what the machine said — `None` where there was nothing to send.
///
/// **Not recorded itself.** A record about sending a record is one more record to
/// send, which is a run that never catches up: what this crosses is history, and
/// the run's own account of the link is the keystrokes it relayed.
fn hand_over_a_record(
    link: &mut Open,
    trace: &mut Option<&mut Trace<'_>>,
    sent: &mut Sent,
) -> Option<i32> {
    let record = trace.as_mut()?.unsent()?;
    let mut frame = [0u8; FRAME];
    Message::Recorded(record).encode(&mut frame);
    match link.session.seal(&frame) {
        Ok((_, sealed)) => {
            let code = link.over.send(&sealed);
            sent.wrote(code);
            Some(code)
        }
        Err(_) => Some(-1),
    }
}

/// One session, and the connection it is sent over.
///
/// The two together because neither is any use without the other: a record
/// sealed against a socket that has gone is one nothing will carry, and a socket
/// with no session on it has nothing to put on the wire — so a run that let go
/// of one and kept the other would have a link it could neither use nor rebuild.
struct Open {
    session: Session,
    over: Box<dyn favjit_host::source::Sending>,
}

/// Put one record on the trace, where a supervisor gave this run one to write.
///
/// A run with no trace is the ordinary one — a person running favjit by hand —
/// so this is written to cost that run a branch and nothing else, rather than
/// each site testing for a recording itself.
fn record(trace: &mut Option<&mut Trace<'_>>, record: Record) {
    if let Some(trace) = trace.as_mut() {
        trace.push(record);
    }
}

/// Wait for the next raw signal, for at most [`RUN_BOUND_POLL`].
///
/// The one instant this needs that `engine` does not already hold — what the
/// clock reads right now — is asked for here and nowhere else, so what reaches
/// [`favjit_host::Host::next_event`] is arithmetic over instants in hand
/// (ADR-0006, ADR-0010).
fn wait(host: &mut dyn SourceHost, seen: &mut Seen) -> Option<favjit_host::HostEvent> {
    let due = crate::clock::Clock::saturating_add(host.now(), RUN_BOUND_POLL);
    seen.took_in(host.next_event(due))
}

/// `RAWMOUSE.usFlags`' `MOUSE_MOVE_ABSOLUTE` bit, which says a report carries
/// where the pointer is rather than how far it moved
/// (`docs/platform/windows/hooks-and-raw-input.md`).
///
/// Read here and not behind the socket, because it is what a flag *means*: the
/// host carries the report's own `usFlags` and this is the one place that says
/// what a bit in it stands for, the way a HID usage is named here and not there
/// (ADR-0006).
const MOUSE_MOVE_ABSOLUTE: u16 = 0x0001;

/// What this run has been told that no keystroke says, and what it counted.
///
/// Held by the run and not by a host, because each of them arrives on the one
/// stream and the run is the end every event reaches: a host that kept them
/// would be keeping what a call answered (ADR-0006).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Seen {
    /// Nothing more will arrive on this stream.
    input_gone: bool,
    /// Somebody outside this process asked the run to stop.
    asked_to_stop: bool,
    /// How many keys and how many pointer reports arrived.
    ///
    /// Counted off the stream because the pair is the measurement that says the
    /// two halves of a source are working together: while the hooks are refusing
    /// input, events still have to arrive as raw input, and refusals with no
    /// arrivals beside them is input that has gone nowhere.
    pub keys: usize,
    pub pointers: usize,
    /// The pointers whose reports say where they are rather than how far they
    /// moved, which this relay cannot carry.
    pub absolute: Vec<DeviceId>,
    /// How many records were handed to the link and how many of them went.
    pub sent: Sent,
}

impl Seen {
    /// Take in what one event says, and answer with the ones that are input.
    ///
    /// The four it absorbs are not keystrokes, so a loop handed one would have
    /// to ask which kind it was before it could relay it — and a wait that
    /// carried one is a wait that carried no input, which is what `None` says.
    /// The two it counts are handed on as well, because they are input.
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
            favjit_host::EventKind::AskedToStop => {
                self.asked_to_stop = true;
                None
            }
            favjit_host::EventKind::Delay(_) => None,
            favjit_host::EventKind::HookedKey { .. } => {
                self.keys += 1;
                Some(event)
            }
            // Read off the report rather than told separately, because it is
            // one of the report's own flags: a machine that said it once per
            // device would be keeping a second record of which devices it had
            // mentioned, and what a flag means is read here either way
            // (ADR-0006).
            favjit_host::EventKind::MouseReport { device, flags, .. } => {
                let (device, flags) = (*device, *flags);
                self.pointers += 1;
                let absolute = (flags & MOUSE_MOVE_ABSOLUTE != 0).then_some(device);
                let new = absolute.filter(|device| !self.absolute.contains(device));
                self.absolute.extend(new);
                Some(event)
            }
            _ => Some(event),
        }
    }
}

/// Wait this long, in pieces, saying between them that the loop is still turning.
///
/// **A wait taken in one piece is a silence a supervisor reads as a wedge.** Its bound
/// is about a person's patience with a keyboard that has stopped (ADR-0008), and it is
/// shorter than the waits this path is made of — looking for the other machine, and the
/// pause between attempts. Nothing is refused while any of them is going, but a watchdog
/// cannot tell that from outside a process, so what the run owes it is the same as
/// everywhere else: a beat as often as the loop otherwise comes round.
/// Counted in pieces rather than measured against a clock, because what a wait leaves
/// behind is a wait and not a reading: a loop that asked the machine for the time would
/// be asking whether the wait it just made had happened, and a machine whose clock says
/// otherwise leaves it going round for ever.
fn waiting(how_long: core::time::Duration, beating: &mut Beating, host: &mut dyn SourceHost) {
    let piece = crate::supervision::BEAT_WITHIN;
    let pieces = (how_long.as_millis() / piece.as_millis().max(1)).max(1);
    for _ in 0..pieces {
        beating.beat(host);
        host.pause(piece);
    }
    beating.beat(host);
}

/// Whether a wait that came back empty is a run that is over.
///
/// Three things and neither folded into the wait: nothing arriving is the
/// ordinary state of a keyboard nobody is touching, and only these say it is
/// anything else (ADR-0006). Two of them arrived on the stream, and the bound
/// this run was given is asked for because a run looking for a sink is not
/// waiting on that stream at all.
fn nothing_more_is_coming(host: &mut dyn SourceHost, seen: &Seen) -> bool {
    seen.input_gone || seen.asked_to_stop || host.asked_to_stop()
}

/// How a forwarding run ended.
///
/// One more than [`Ended`] has, because a run can also fail before it reads
/// anything, and whatever started favjit acts on the difference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ending {
    /// It relayed until there was nothing more, or until it was asked to stop.
    Relayed,
    /// The keyboards could not be read, so there was nothing to forward.
    NoInput,
    /// Reading the keyboards stopped while the run was going.
    InputGone,
    /// There is nothing to relay to and there will not be.
    NoLink,
}

/// Why the stream that ended went no further, asked one fact at a time.
///
/// The order is this end's: a machine with nowhere to relay to is that whatever
/// its bound says, and a stream that ran out with time still on the clock is a
/// capture that stopped rather than a run that finished. No host operation
/// answers two of these, and none of them is an ending — what an ended stream
/// means for the exit code is decided here, where the suite reads it
/// (ADR-0006).
///
/// `nowhere_to_look` is what this run found out in its own loop rather than a
/// question put to the machine again: a dry run never asks where its sink is,
/// and a machine answering that on its own would report one as never having
/// been paired.
fn why_it_stopped(host: &mut dyn SourceHost, nowhere_to_look: bool, seen: &Seen) -> Ending {
    if nowhere_to_look {
        return Ending::NoLink;
    }
    match seen.asked_to_stop || host.asked_to_stop() {
        true => Ending::Relayed,
        false => Ending::InputGone,
    }
}

/// This machine's own identity, and the sink it will present it to — asked of
/// the host once, before a relaying run does anything else with it.
///
/// `None` for either reads as the same ending either way ([`Ending::NoLink`]),
/// but each has its own sentence: a machine with no identity has nothing to
/// present, and one with nothing pinned has nobody to present it to.
fn identity_and_sink(host: &mut dyn SourceHost) -> Option<(Identity, Vec<u8>)> {
    let identity = match pairing::identity(host) {
        Ok(identity) => identity,
        Err(why) => {
            host.warn(format_args!("no identity, so no link: {why}"));
            return None;
        }
    };
    let Some(text) = host.pinned_sink() else {
        host.warn(format_args!(
            "no sink is pinned, so no link: run favjit --pair on the Mac to put a code on its \
             screen, then favjit --pair <those digits> here. The Mac has to be listening as \
             well, which is what sudo favjit --install sets up there"
        ));
        return None;
    };
    let Some(sink) = pairing::pinned_sink(&text) else {
        host.warn(format_args!(
            "the pinned-sink file names no key, so no link: pair again with favjit --pair \
             <digits>"
        ));
        return None;
    };
    Some((identity, sink))
}

/// One device on the machine the source is reading.
struct Device {
    id: DeviceId,
    /// What the host said this device is, for saying it again over a later link.
    ///
    /// Absent for a device that has only ever produced pointer reports: a mouse
    /// is not in the sink's device list, because its reports carry no key for a
    /// rule to be about.
    info: Option<DeviceInfo>,
    /// Whether the sink has been told about it *over the link that is up now*.
    announced: bool,
    /// The keys it is holding, as the sink believes them.
    held: Vec<Key>,
    /// The buttons its last relayed report said were down.
    buttons: Buttons,
}

impl Device {
    fn new(id: DeviceId) -> Self {
        Self {
            id,
            info: None,
            announced: false,
            held: Vec::new(),
            buttons: Buttons::NONE,
        }
    }
}

/// What the source knows about the machine it is reading, between events.
///
/// All of it exists because relaying is not repeating: the platform describes a
/// held key as a stream of presses and a mouse button as the transition it just
/// made, and what crosses the link is the input itself. Keeping that here rather
/// than in a host is what puts it inside the end-to-end suite's reach (ADR-0006)
/// — and it is one answer rather than one per platform.
#[derive(Default)]
struct Forwarding {
    devices: Vec<Device>,
    /// What the last event turned into, reused rather than allocated per
    /// keystroke: this sits in the interactive path.
    out: Vec<Message>,
}

impl Forwarding {
    /// This device, remembering it if it is new.
    fn device(&mut self, id: DeviceId) -> &mut Device {
        match self.devices.iter().position(|device| device.id == id) {
            Some(at) => &mut self.devices[at],
            None => {
                self.devices.push(Device::new(id));
                self.devices.last_mut().expect("the device just pushed")
            }
        }
    }

    /// A link has just come up, so nothing has been said over it.
    ///
    /// Held keys are forgotten, because a session that ended is one the sink
    /// released every held key at the end of: the finger may still be on the key,
    /// and Windows goes on delivering its auto-repeat, so the next of those
    /// presses is the key going down as far as the new link is concerned.
    ///
    /// Pointer buttons are *not* forgotten, and the asymmetry is the sink's: it
    /// releases keys when a device goes away and has nothing to say about a
    /// button. Forgetting a held button would make the report that releases it
    /// look like no change at all, and the sink would hold it for ever.
    fn on_a_new_link(&mut self) {
        for device in &mut self.devices {
            device.announced = false;
            device.held.clear();
        }
    }

    /// Follow what this machine has attached to it, and say nothing about it.
    ///
    /// For the state where the keyboard is this machine's: nothing is relayed and there
    /// is no link to relay over, and a device that arrived during it is still a device
    /// the sink has to be told about once the keyboard goes over. Dropped instead, the
    /// source would have no name for it — and a key from a device the sink has no record
    /// of is passed through unconverted, so the layout would silently stop applying to
    /// the machine the input is coming from (ADR-0003).
    ///
    /// Not [`Forwarding::relay`] with its messages thrown away, because the two differ in
    /// what they leave behind: a device noted here is *not* announced, so the link that
    /// comes up next announces it.
    fn note(&mut self, kind: EventKind) {
        match kind {
            EventKind::DeviceAttached(info) => {
                let device = self.device(info.id);
                device.info = Some(info);
                device.held.clear();
            }
            // Forgotten outright. One that came and went while nothing was being
            // relayed is one the sink has no reason to hear about, either way round.
            EventKind::DeviceDetached(id) => self.devices.retain(|device| device.id != id),
            _ => {}
        }
    }

    /// What to send for one event, in order.
    ///
    /// More than one message where the sink has to be told what a device is
    /// before an event from it means anything: a key from a device the sink has
    /// no record of is passed through unconverted, so the layout would silently
    /// stop applying to the machine the input is coming from.
    fn relay(&mut self, kind: EventKind) -> &[Message] {
        self.out.clear();
        let worth = match kind {
            EventKind::DeviceAttached(info) => {
                let device = self.device(info.id);
                device.info = Some(info);
                device.announced = true;
                // Announced again, so the sink has just been handed a device with
                // nothing held on it.
                device.held.clear();
                true
            }
            EventKind::DeviceDetached(id) => {
                // Only for a device the sink was told about: it is what releases
                // that device's keys there, and one for a device it has no record
                // of is an event no hardware made.
                let announced = self
                    .devices
                    .iter()
                    .any(|device| device.id == id && device.announced);
                self.devices.retain(|device| device.id != id);
                announced
            }
            EventKind::KeyDown { device, key } => {
                let device = self.device(device);
                match device.held.contains(&key) {
                    // The platform's own auto-repeat. macOS repeats whatever it
                    // is holding (`docs/platform/macos/let-the-machine-produce-the-key-repeats.md`), so relaying these would put two
                    // repeat sources on one key.
                    true => false,
                    false => {
                        device.held.push(key);
                        true
                    }
                }
            }
            EventKind::KeyUp { device, key } => {
                let device = self.device(device);
                match device.held.iter().position(|held| *held == key) {
                    Some(at) => {
                        device.held.remove(at);
                        true
                    }
                    // A key that was already down before this end was looking.
                    // The sink never saw it go down, so there is nothing there to
                    // release.
                    None => false,
                }
            }
            EventKind::Pointer { device, report } => {
                let device = self.device(device);
                // "Holding nothing" is only silence next to a report that also
                // held nothing, which is why the comparison is against this
                // device's own last one: a click is a press and a release with no
                // movement in either, and the release is the half that would
                // leave a button stuck down.
                let worth = !report.is_still() || report.buttons != device.buttons;
                device.buttons = report.buttons;
                worth
            }
            // The watchdog's question, this process's own wake-up, and an ask for
            // the keyboard are about the machine they happened on. What the last of
            // them does is move the keyboard, which the loop does rather than the
            // link.
            EventKind::Timer | EventKind::Probe | EventKind::Asked(_) => false,
        };

        if worth {
            // In front of the message and only when there is one: an announcement
            // that went out on its own would be a message the link carried for
            // nothing, and there is a whole class of events — a held key's
            // auto-repeat, a release the sink has already made — that produce no
            // message at all.
            if let Some(id) = subject(kind) {
                self.announce(id);
            }
            if let Some(message) = crate::link::message_of(kind) {
                self.out.push(message);
            }
        }
        &self.out
    }

    /// Let go of everything the sink believes is held, in order.
    ///
    /// What this is for is the keyboard coming back to this machine while the link
    /// stays up: the chord is made with a modifier that went across, and a sink that
    /// was never told it came up holds it down for ever — every later keystroke on the
    /// Mac's own keyboard chorded with it. The session ending covers the same failure
    /// (ADR-0002) and this one does not end the session, so it is said here.
    ///
    /// Buttons as well as keys. A sink has nothing to say about a button of its own
    /// accord, so a pointer that went away holding one is the same failure in the
    /// pointer's vocabulary.
    fn let_go(&mut self) -> &[Message] {
        self.out.clear();
        for device in &mut self.devices {
            // Last pressed, first let go of, so a key is never released while the
            // modifier it was pressed under is still held there: what the sink injects
            // for a release is read against the layer it is holding.
            while let Some(key) = device.held.pop() {
                self.out.push(Message::KeyUp {
                    device: device.id,
                    key,
                });
            }
            if device.buttons != Buttons::NONE {
                device.buttons = Buttons::NONE;
                self.out.push(Message::Pointer {
                    device: device.id,
                    report: PointerReport::default(),
                });
            }
        }
        &self.out
    }

    /// Put this device's announcement in front of what is about to be relayed, if
    /// the link that is up has not had it yet.
    fn announce(&mut self, id: DeviceId) {
        let device = self.device(id);
        if device.announced {
            return;
        }
        let Some(info) = device.info else {
            return;
        };
        device.announced = true;
        self.out
            .push(Message::DeviceAttached(crate::link::Attached {
                device: info.id,
                is_built_in: info.is_built_in,
                vendor_id: info.vendor_id,
                product_id: info.product_id,
            }));
    }
}

/// The device an event is *from*, where the sink has to know what it is before the
/// event means anything.
///
/// Neither of the two device events is one of those. An attach is the announcement,
/// so putting another in front of it would say the same thing twice; a detach is
/// the sink being told to forget a device, which is not a moment to introduce one.
/// Refuse this from now on, and show it where a person can see it.
///
/// The two together and in this order: what the procedures refuse and what a
/// person is shown are one state, and showing it first would put a machine on
/// screen as having taken the keyboard a moment before it had.
fn refuse(host: &mut dyn SourceHost, what: Suppressing) {
    host.suppress(what);
    if host.there_is_somewhere_to_show_it() {
        host.show_what_is_refused(what);
    }
}

/// What to answer one call into a procedure the platform makes.
///
/// The whole of what a hook does, decided here: reported first and refused
/// second, which is the only order in which both can happen where what refuses a
/// key is also where keys are read from. The loop that decides what a key means
/// has to see every one of them, including the ones this machine is not going to
/// get.
///
/// A bare function so that a procedure reached through a function pointer can
/// call it, which is why everything it reads comes through [`Answers`] rather
/// than from anything held.
pub fn answer_a_procedure(arrived: Arrived, answers: &dyn Answers) -> isize {
    match arrived {
        Arrived::Pointer => answer_a_pointer(answers),
        Arrived::Key { at, packed, flags } => answer_a_key(at, packed, flags, answers),
        // Nothing of ours, and a kind this crate does not yet name with it: an
        // event favjit cannot say anything about is one it must not refuse, since
        // refusing what it cannot relay is taking input away with nothing to show
        // for it (ADR-0008).
        _ => answers.let_it_carry_on(),
    }
}

/// A pointer event refused only while everything is, and counted where it is.
///
/// Counted because a run that reports none of them while it was relaying is one
/// whose mouse reached neither machine, and the number is the only thing that
/// says which.
fn answer_a_pointer(answers: &dyn Answers) -> isize {
    match answers.refusing() {
        Suppressing::Everything => {
            answers.one_pointer_refused();
            answers.end_it_here()
        }
        _ => answers.let_it_carry_on(),
    }
}

/// One key handed over, and then refused or not.
fn answer_a_key(at: At, packed: usize, flags: i64, answers: &dyn Answers) -> isize {
    // Handed over before anything is decided about it: a key that reached
    // nothing is one the run cannot have relayed, and the count of those is what
    // the run reports.
    if answers.keys_go_somewhere() {
        answers.hand_the_key_over(packed, flags);
    }
    let released = flags as u32 & favjit_hid::scancode::LLKHF_UP != 0;
    let refuse = match answers.refusing() {
        // Everything but the release of a key this machine still holds. The
        // machine believes a key is down from the press it was let see until the
        // release it is let see, and a refused release leaves it down for as long
        // as nobody presses that key again — through a whole trip of the keyboard,
        // for a modifier held at the moment the link came up. Refusing the release
        // outright would be the sink's stuck-modifier hazard (ADR-0013) on this
        // side; injecting a release instead would be an event favjit made, which
        // the procedures ignore, and a second mechanism where letting one event
        // through does.
        Suppressing::Everything => {
            !(released && held_bit(at).is_some_and(|bit| answers.held_here(bit)))
        }
        // The chord alone, so pressing it moves the keyboard rather than also
        // reaching whatever has the foreground.
        Suppressing::TheSwitch => is_the_switch(at, answers),
        _ => false,
    };
    if refuse {
        return answers.end_it_here();
    }
    // Only what this machine is let see, so what it is believed to hold is
    // exactly what it was shown: a refused press is one it never saw go down, and
    // its release has nothing to let go of.
    if let Some(bit) = held_bit(at) {
        match released {
            true => answers.let_go_of_here(bit),
            false => answers.now_held_here(bit),
        }
    }
    answers.let_it_carry_on()
}

/// The bit this machine keeps a position under, and none for a make code past
/// the byte a scan code is: such a key is never held here, so its release is
/// refused like any other — the answer for a key nobody can have seen go down.
///
/// Decided here and not where the bits are stored, so the machine stores a
/// number and reads nothing (ADR-0006).
fn held_bit(at: At) -> Option<usize> {
    let (code, extended) = at;
    let bit = usize::from(code) | (usize::from(extended) << 8);
    (bit < favjit_host::source::HELD_HERE_BITS).then_some(bit)
}

/// Whether this position with the modifier down is the chord.
///
/// **Both halves, because the chord's keys are two ordinary letters.** Refusing
/// them on the position alone is those two letters typing nothing for as long as
/// the keyboard is this machine's, which is from the moment a run comes up
/// (ADR-0013) — and while it is, there is no link for them to have gone over
/// instead, so they reach nothing at all.
fn is_the_switch(at: At, answers: &dyn Answers) -> bool {
    let (to_the_sink, back_here) = answers.the_chord();
    let one_of_them = to_the_sink == Some(at) || back_here == Some(at);
    one_of_them && answers.the_modifier_is_down()
}

fn subject(kind: EventKind) -> Option<DeviceId> {
    match kind {
        EventKind::KeyDown { device, .. }
        | EventKind::KeyUp { device, .. }
        | EventKind::Pointer { device, .. } => Some(device),
        EventKind::DeviceAttached(_)
        | EventKind::DeviceDetached(_)
        | EventKind::Timer
        | EventKind::Probe
        | EventKind::Asked(_) => None,
    }
}

/// Where it is defined is `favjit_host::source`, because an event carries it: an ask
/// from outside the keyboard arrives on the same stream as the keystrokes, and the
/// program that sends one links the boundary rather than this crate (`docs/platform/windows/tray-item-as-its-own-program.md`).
pub use favjit_host::source::Driving;

/// The keys that move the keyboard, each chorded with an option key.
///
/// **The position rather than what it converts to.** While the keyboard is driving
/// this machine nothing is converting anything — the keystrokes are this machine's own
/// — so a chord named in the layout's vocabulary would be a chord this end could not
/// recognise. The conversion is the sink's alone (ADR-0003), and the source reading a
/// layout in order to know its own chord would be a second place for the layout to
/// live.
///
/// Chorded with option because it is the modifier this end never sends alone and the
/// one a person is holding least often: a bare key would move the keyboard mid-word.
pub const SWITCH_TO_THE_SINK: Key = Key::N;
pub const SWITCH_BACK: Key = Key::S;

/// Whether an option key is down, so the chord is recognised whatever is being relayed.
///
/// Kept here rather than read off [`Forwarding`]: what that holds is what the *sink*
/// believes, and while the keyboard is driving this machine the sink believes nothing.
#[derive(Debug, Default)]
struct Chording {
    /// The option keys physically down, each with the keyboard holding it. A person
    /// chords on one keyboard, and two keyboards holding option between them is not a
    /// case worth a rule of its own — but which keyboard it is matters once the
    /// keyboard goes over, because the sink reads a key against the device it came from.
    option: Vec<(DeviceId, Key)>,
}

impl Chording {
    /// The option keys down right now, as key-down events on the keyboards holding them.
    ///
    /// Kept here and not read off [`Forwarding`], for the reason the struct is: the
    /// chord that sent the keyboard over was made with option, pressed while nothing
    /// crossed, so the sink holds no record of it — and a key typed under that same
    /// press would arrive there bare, `a` for a person who meant option-`a`.
    fn still_holding(&self) -> impl Iterator<Item = EventKind> + '_ {
        self.option
            .iter()
            .map(|&(device, key)| EventKind::KeyDown { device, key })
    }

    /// Follow this event, and say which machine it asks the keyboard to drive.
    ///
    /// `None` for everything else, which is nearly everything: the chord is two keys
    /// out of the whole stream, and what happens to the rest is [`run`]'s.
    fn asked_for(&mut self, kind: EventKind) -> Option<Driving> {
        match kind {
            EventKind::KeyDown { device, key } if is_option(key) => {
                if !self.option.contains(&(device, key)) {
                    self.option.push((device, key));
                }
                None
            }
            EventKind::KeyUp { device, key } if is_option(key) => {
                self.option.retain(|held| *held != (device, key));
                None
            }
            // The press and not the release, so the keyboard moves as the chord is
            // made rather than as it is let go of.
            EventKind::KeyDown { key, .. } if !self.option.is_empty() => match key {
                SWITCH_TO_THE_SINK => Some(Driving::TheSink),
                SWITCH_BACK => Some(Driving::ThisMachine),
                _ => None,
            },
            _ => None,
        }
    }
}

fn is_option(key: Key) -> bool {
    matches!(key, Key::LeftOption | Key::RightOption)
}

/// Which machine this event asks the keyboard to drive, by either route.
///
/// Two routes because the chord needs the keyboard to be working, and the case a
/// person needs it in most is the one where it is not: a chord that is not getting
/// through leaves killing the process. So an ask can also arrive from outside the
/// keyboard — the tray item on Windows (`docs/platform/windows/tray-item-as-its-own-program.md`) — and the two meet here, where what
/// follows cannot tell them apart.
fn asked_for(kind: EventKind, chording: &mut Chording) -> Option<Driving> {
    match kind {
        EventKind::Asked(driving) => Some(driving),
        kind => chording.asked_for(kind),
    }
}

/// What a run was asked to do.
///
/// **Two modes, because refusing and relaying are one thing.** Refusing this
/// machine's input while sending it nowhere is a keyboard that has stopped, which is
/// the one outcome ADR-0008 rules out — and it would be *asked for* rather than
/// failed into, so nothing downstream would notice. Relaying without refusing is
/// the other half: the keystroke stays on this machine as well, so every key lands
/// on both screens. Neither is expressible, because there is no flag between them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request {
    /// Read the keyboards, refuse nothing, send nothing.
    ///
    /// What makes it safe to be what a bare command does: no socket is opened, so a
    /// machine with nothing paired still runs, and nothing it reads leaves it.
    DryRun,
    /// Open a link to the sink and send this machine's input over it.
    ///
    /// Always refusing this machine's own input, which is why there is no flag for
    /// it: Windows delivers the keystroke locally as well, so a run that relayed
    /// without refusing would type every key on both screens. Refusing without
    /// relaying is the other half of the same, and takes the keyboard away.
    Relaying,
}

/// Say what this run costs, if it costs anything.
///
/// One cost, and it is not the request's doing: what [`Request`] can express is safe
/// by construction, so what is left to warn about is the machine. A relaying run
/// refuses this machine's input through a wedge, which leaves it with none at all,
/// and whether anything is watching to end that is something only the machine knows
/// (ADR-0008).
fn warn_about(request: &Request, host: &mut dyn SourceHost) {
    if matches!(request, Request::Relaying) && !host.is_supervised() {
        host.warn(format_args!(
            "no watchdog: a wedge while the keyboards are refused has nothing to end it, and \
             this machine has no input left"
        ));
    }
}

/// Say that the link has gone.
///
/// One wording for the two places a send can fail — a keystroke on the way over, and
/// the frame that keeps a link while the keyboard is here — because to the person they
/// are the same thing: the Mac has stopped hearing this machine. Two wordings would be
/// a log that distinguishes which of the run's sends found out.
fn say_the_link_has_gone(host: &mut dyn SourceHost, answered: i32) {
    // The number in the sentence as well as on the trace: a link that drops over
    // and over is read from the log first, and the two failures worth telling
    // apart there — a write that ran out of time and a connection the other end
    // reset — are the same sentence without it (ADR-0009).
    host.warn(format_args!(
        "the link to the other machine has gone (error {answered}); looking for it again, \
         and the keyboard is this machine's until it is back"
    ));
}

/// Drives the initiator's whole half of the handshake, so a host answers by
/// reading and writing bytes alone: the construction is `engine`'s to hold
/// (ADR-0006), and this is the one place it is held from the source's side.
fn shake_hands(
    identity: &Identity,
    sink: &[u8],
    host: &mut dyn SourceHost,
    open: &mut dyn favjit_host::source::Opening,
) -> Option<Session> {
    let mut initiator = Initiator::new(identity, sink, host).ok()?;
    let first = initiator.first().ok()?;
    if !open.send_first_message(&first) || !open.flush_first_message() {
        return None;
    }
    let mut answer = [0u8; favjit_noise::ANSWER];
    if !open.take_answer(&mut answer) {
        return None;
    }
    initiator.take_answer(&answer).ok()?;
    initiator.done().ok()
}

/// Seal one message under the session, and send it, answering with what the
/// machine said — `0` for the record that went.
///
/// A seal that failed answers `-1`, the same as a call that could not be made: a
/// cipher state that will not seal is one no later message can be sealed with
/// either, so there is nothing more `run` can do with this session, and nothing
/// the machine was asked about.
fn seal_and_send(link: &mut Open, message: Message, sent: &mut Sent) -> Handed {
    let mut frame = [0u8; FRAME];
    message.encode(&mut frame);
    match link.session.seal(&frame) {
        Ok((at, sealed)) => {
            let code = link.over.send(&sealed);
            sent.wrote(code);
            Handed { at: Some(at), code }
        }
        Err(_) => Handed { at: None, code: -1 },
    }
}

/// What became of one message handed to the link.
struct Handed {
    /// The number the session sent it under, and `None` for a record that would
    /// not seal.
    ///
    /// A record that did not seal never crossed, so the other end has no
    /// counterpart for it to be merged against — which is what the number is
    /// for, and why there is not one to record here (ADR-0009).
    at: Option<u64>,
    /// `0` for the write that went, and the machine's own number otherwise.
    code: i32,
}

/// One device's pointer, between the raw reports that describe it.
///
/// Neither [`favjit_hid::rawmouse::Pointing`]'s state nor a `HashMap`'s
/// allocation belongs to one event, so it lives here, across the whole run —
/// the same reason [`crate::sink::Resolver`] keeps one per device on the
/// sink's side.
struct Resolver {
    /// Whether this machine's keyboards are the ANSI shape.
    ///
    /// One position depends on it, and the make code alone cannot say which
    /// keyboard sent it (`favjit_hid::scancode::named`).
    ansi: bool,
    pointers: Vec<(DeviceId, favjit_hid::rawmouse::Pointing)>,
}

impl Resolver {
    fn new(ansi: bool) -> Self {
        Self {
            ansi,
            pointers: Vec::new(),
        }
    }

    fn pointing(&mut self, device: DeviceId) -> &mut favjit_hid::rawmouse::Pointing {
        if let Some(at) = self.pointers.iter().position(|(id, _)| *id == device) {
            return &mut self.pointers[at].1;
        }
        self.pointers
            .push((device, favjit_hid::rawmouse::Pointing::default()));
        &mut self.pointers.last_mut().expect("just pushed").1
    }
}

/// Turn one raw signal into what it means, or nothing when it means nothing on
/// its own — a make code no key is named for.
///
/// The one place a [`favjit_host::EventKind`] becomes an [`EventKind`], the same
/// reason [`crate::sink::resolve`] is: naming a make code is a table, and a
/// table beside the API that produced its input is a decision no test can
/// drive (ADR-0006).
fn resolve(
    raw: favjit_host::HostEvent,
    resolver: &mut Resolver,
    host: &mut dyn SourceHost,
) -> Option<HostEvent> {
    let kind = match raw.kind {
        favjit_host::EventKind::PathDeviceFound { device, path } => {
            EventKind::DeviceAttached(crate::device::from_a_path(device, &path))
        }
        // A source's own host names a device by its interface path: HID
        // properties are how the machine being typed *into* describes one, and
        // nothing on this side of the link reads them.
        favjit_host::EventKind::HidDeviceFound { .. } => return None,
        favjit_host::EventKind::DeviceLost(id) => EventKind::DeviceDetached(id),
        favjit_host::EventKind::HookedKey {
            device,
            make_code,
            vkey,
            flags,
        } => {
            use favjit_hid::scancode::Pressed;

            // Read through the hook's own spelling of the two facts a make code
            // sits behind, and then through the one table that says which of
            // them are keys at all: three things arrive on this stream that are
            // not a key, and every one of them would otherwise be named as
            // whatever its make code stands for.
            let flags = favjit_hid::scancode::from_a_hook(flags, vkey);
            let (extended, code, up) = match favjit_hid::scancode::pressed(flags, make_code, vkey) {
                Pressed::Down { extended, code } => (extended, code, false),
                Pressed::Up { extended, code } => (extended, code, true),
                // Said nothing about, unlike a position with no name: this is
                // not a key that failed to convert, it is a report that was
                // never a key.
                Pressed::NotAKey => return None,
            };
            let Some(key) = favjit_hid::scancode::named(extended, code, resolver.ansi) else {
                host.warn(format_args!(
                    "no key at make code {code:#04x} (extended: {extended})"
                ));
                return None;
            };
            match up {
                false => EventKind::KeyDown { device, key },
                true => EventKind::KeyUp { device, key },
            }
        }
        favjit_host::EventKind::MouseReport {
            device,
            flags,
            button_flags,
            button_data,
            dx,
            dy,
        } => {
            let raw = favjit_hid::rawmouse::Raw {
                flags,
                button_flags,
                button_data,
                dx,
                dy,
            };
            let report = resolver.pointing(device).report(&raw);
            EventKind::Pointer { device, report }
        }
        favjit_host::EventKind::Probe => EventKind::Probe,
        favjit_host::EventKind::Asked(driving) => EventKind::Asked(driving),
        // `favjit_host::EventKind` is `#[non_exhaustive]`: a kind this crate
        // does not yet name resolves to nothing rather than failing the whole
        // stream.
        _ => return None,
    };
    Some(HostEvent::new(raw.at, kind))
}

/// A whole run of the forwarding machine.
///
/// One loop over one event stream, the shape ADR-0006 asks of every role — with
/// the link either up or being waited for, and the keyboards taken only in the
/// first of those.
///
/// A run that was not asked to relay opens nothing and refuses nothing: it reads
/// the keyboards and stops there. That is the default, and what makes it safe to
/// be one — a machine with nothing paired still runs, and nothing it reads leaves
/// it. It is here rather than in the binary because it is the same loop with two
/// decisions taken differently, and both decisions are checkable.
pub fn run(
    request: &Request,
    ansi: bool,
    host: &mut dyn SourceHost,
    trace: Option<&mut [u8]>,
) -> (Ending, Seen) {
    // Recorded when a supervisor provided the memory, and not at all otherwise: a
    // trace this process allocated for itself would be lost in exactly the failures
    // it exists for, and it would be a keylog nobody asked for (ADR-0009).
    let mut trace = trace.map(Trace::new);
    let mut seen = Seen::default();
    let ending = relay(request, ansi, host, trace.as_mut(), &mut seen);
    (ending, seen)
}

/// How many records one run handed to the link and how many of them went.
///
/// Counted here rather than by the host, because each of the two is what a call
/// answered: a host that added them up would be keeping what it was told
/// (ADR-0006), and the number a write answers with is the run's to read.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Sent {
    /// Records handed over, whether or not the write went.
    pub handed: usize,
    /// Of those, the ones the machine said nothing about — the writes that went.
    pub went: usize,
}

impl Sent {
    /// Take in what one write answered with.
    fn wrote(&mut self, code: i32) {
        self.handed += 1;
        self.went += usize::from(code == 0);
    }
}

/// The run itself, with the recording threaded through rather than wrapped round
/// the host.
///
/// The sink wraps its host, because what it records includes checkpoints of its
/// own state and every report a render produced — facts that exist only at the
/// boundary. What a source records is the event it resolved and the number its
/// one send answered with, and both are in hand at the call site: threading the
/// trace keeps one loop rather than a second copy of it that could drift from
/// this one, which is the failure a recording is least able to survive.
fn relay(
    request: &Request,
    ansi: bool,
    host: &mut dyn SourceHost,
    mut trace: Option<&mut Trace<'_>>,
    seen: &mut Seen,
) -> Ending {
    // Before anything is taken, because a cost named after the keyboards are refused
    // is one the person can no longer decide about.
    warn_about(request, host);

    // One for the whole run, whichever mode it is in: what it holds is that a
    // beat which is not arriving has been said once already, and a run that
    // started a fresh one per loop would say it again on every way round.
    let mut beating = Beating::default();

    // The shape of the request is what says whether refusing is ever going to be
    // wanted: only the variant that relays refuses anything, so a dry run does not
    // put a hook on the machine.
    if *request == Request::DryRun {
        // Before the reading starts, because a procedure the platform calls has
        // to have an answer by the time the platform can call it.
        host.answer_procedures_with(answer_a_procedure);
        // Neither the link nor suppression is reached, so a dry run is the one mode
        // that touches nothing outside this process: the loop it turns installs
        // no hook, which is what would take the keyboard away.
        if !host.take_input(
            PROBE_TICK,
            RAW_BYTES,
            Box::new(|turning| crate::capture::relay(turning, PROBE_TICK, false)),
        ) {
            return Ending::NoInput;
        }
        loop {
            let arrived = wait(host, seen).is_some();
            // Before the beat and not after it: a loop that is leaving is not one
            // coming back round, and a heartbeat is what says it did (ADR-0008).
            if !arrived && nothing_more_is_coming(host, seen) {
                break;
            }
            // Answered whether or not anything came, because what the watchdog is
            // owed is that round and not a keystroke.
            beating.beat(host);
        }
        // Nowhere to look is not one of the answers a dry run can reach: it
        // never looks, so its stream ending is the bound or the capture and
        // nothing else.
        return why_it_stopped(host, false, seen);
    }

    // Asked for up front rather than found missing partway round the loop below:
    // a machine with neither is not going to open a link on any later attempt
    // either, and there is no reading to do until one might. Established here
    // rather than left to a binary, so that the sequence deciding what this
    // machine presents is the same one the suite drives (ADR-0006), the way
    // `sink::bring_up` establishes this machine's identity for the same reason.
    let Some((identity, sink)) = identity_and_sink(host) else {
        return Ending::NoLink;
    };

    // Before the reading starts, for the reason the dry run's is.
    host.answer_procedures_with(answer_a_procedure);
    // Before anything is connected to, because a machine whose input cannot be read
    // has nothing to forward. Refusing, always, for a run that relays: the
    // keystroke stays on this machine otherwise ([`Request::Relaying`]).
    if !host.take_input(
        PROBE_TICK,
        RAW_BYTES,
        Box::new(|turning| crate::capture::relay(turning, PROBE_TICK, true)),
    ) {
        return Ending::NoInput;
    }

    let mut resolver = Resolver::new(ansi);
    let mut relaying = Forwarding::default();
    let mut chording = Chording::default();
    // This machine's from the start, so that a run coming up moves nothing: nothing is
    // sent and nothing is refused but [`SWITCH_TO_THE_SINK`], which is how a person asks
    // for the keyboard to go over. This is the state's starting value and not a second
    // mode — `--dry-run` still decides whether a run will ever take the keyboards and
    // open a link, and this decides only where the keyboard is once one will.
    //
    // What settles it is that a run is started by a logon rather than by whoever typed
    // the flag: the flag is typed once, and a keyboard that went to the Mac at
    // every logon after that would be going somewhere nobody asked for it to be that
    // time. It is also the same state a run reaches after [`SWITCH_BACK`], so startup is
    // not a case of its own — there is one waiting state and both ways in lead to it.
    let mut driving = Driving::ThisMachine;
    // Whether the last attempt at a link failed, so the wait goes between attempts and
    // not in front of one: a source started beside a Mac that is already up should not
    // sit out a pause before it says anything, and neither should the keyboard the
    // person has just asked to move.
    let mut failed = false;
    // Whether this machine turned out to have nowhere to look for a sink, which
    // is remembered rather than asked again on the way out: it is the one reason
    // for stopping that no later question can distinguish from a run that simply
    // reached its bound.
    let mut nowhere_to_look = false;
    // Whether the handshake advice has been given, so it is given once and not on
    // every round: the loop rounds again for as long as the Mac stays unpaired, and
    // a line per round would bury the one thing the person has to do.
    let mut said_how_to_pair = false;
    // Whether the run has said that nothing is answering, for the same reason and once
    // for the same length of time.
    let mut said_nothing_answered = false;
    // The session, kept across a trip of the keyboard rather than made per trip.
    //
    // **What makes the trip back immediate.** Rebuilt each time, going over again costs
    // a question asked of the network, a connection, a handshake and every keyboard
    // announced a second time — seconds in which the keyboard has stopped answering the
    // machine in front of the person and has not started answering the other one. Kept,
    // the chord is the whole of it.
    //
    // What it costs is a frame every so often while the keyboard is here, because the
    // sink ends a session it has heard nothing on: that is what
    // [`crate::link::Message::StillHere`] is for.
    let mut session: Option<Open> = None;
    loop {
        // The keyboard is this machine's, so nothing is refused but the chord that
        // moves it — which is refused so that pressing it does not also reach whatever
        // has the foreground (ADR-0013).
        if driving == Driving::ThisMachine {
            refuse(host, Suppressing::TheSwitch);
            loop {
                let Some(raw) = wait(host, seen) else {
                    if !nothing_more_is_coming(host, seen) {
                        beating.beat(host);
                        // Nothing is crossing while the keyboard is here, and a session
                        // the sink has heard nothing on is one it ends — so what keeps
                        // the link is saying that this end is still there. On the wait
                        // coming back empty, which is the only moment a run has spare.
                        if let Some(open) = session.as_mut() {
                            let handed = seal_and_send(open, Message::StillHere, &mut seen.sent);
                            if let Some(at) = handed.at {
                                record(
                                    &mut trace,
                                    Record::Sent {
                                        at,
                                        code: handed.code,
                                    },
                                );
                            }
                            let answered = handed.code;
                            if answered != 0 {
                                // The link has gone. Said here rather than found out on
                                // the way over, because what the person does next is
                                // press the chord and what should happen then is a
                                // fresh link rather than a send into a closed socket.
                                session = None;
                                say_the_link_has_gone(host, answered);
                            }
                        }
                        continue;
                    }
                    // Given back as the run stops, so nothing it refused outlives it —
                    // not even the one chord (ADR-0008).
                    refuse(host, Suppressing::Nothing);
                    return why_it_stopped(host, nowhere_to_look, seen);
                };
                let asked = resolve(raw, &mut resolver, host).and_then(|event| {
                    record(&mut trace, Record::Event(event));
                    // What this machine has attached is followed here too, because it is
                    // where a run comes up and so where every device it starts with
                    // arrives.
                    relaying.note(event.kind);
                    asked_for(event.kind, &mut chording)
                });
                // Answered before anything is decided about it, for the reason every
                // other event is: the loop came back round (ADR-0008).
                beating.beat(host);
                if asked == Some(Driving::TheSink) {
                    driving = Driving::TheSink;
                    break;
                }
            }
        }

        // Nothing is refused while there is no link, so the machine in front of the
        // person keeps working: this is the whole of what makes a source safe to leave
        // running when the other machine is off. Said on **every** way into this and
        // not only the first, because a link that dropped leaves everything refused
        // until something says otherwise — and what the person needs at that moment is
        // the keyboard they are sitting at (ADR-0008).
        //
        // Not even the chord: what happens next blocks, so refusing a key this loop is
        // not there to read would eat it for nothing.
        //
        // **All of it skipped where the session is still open**, which is the ordinary
        // way over once a link has been made: there is nothing to look for, nothing to
        // connect, nothing to shake hands over, and the devices the sink was told about
        // are still the devices it knows. Nor is anything given back and taken again,
        // since what was refused is what is about to be refused.
        if session.is_none() {
            refuse(host, Suppressing::Nothing);
            if failed {
                waiting(RETRY, &mut beating, host);
            }

            // Two facts, asked apart and prioritised here rather than answered
            // together: a run whose bound has passed is over whether or not it was
            // ever paired, so it is asked first and the second is not asked at all
            // (ADR-0006).
            let stop = host.asked_to_stop();
            if !stop && !host.has_a_sink_to_look_for() {
                nowhere_to_look = true;
            }
            let found = if stop || nowhere_to_look {
                Connected::Done
            } else {
                match host
                    .use_fixed_sink()
                    .or_else(|| crate::discovery::find(crate::link::SERVICE, LOOK, host))
                {
                    Some(sink) => Connected::Ready(sink),
                    None => Connected::NotFound,
                }
            };
            let sink_to_reach = match found {
                Connected::Ready(sink) => {
                    failed = false;
                    sink
                }
                // Round again. Looking is what waits, so this is not a spin — and the
                // other machine coming back is the ordinary case, not an error.
                Connected::NotFound => {
                    // Said once, because it is the difference between the chord doing
                    // nothing and the chord having been heard: a person who pressed it and
                    // is still typing on this machine has no other way to tell those apart,
                    // and a line per round would bury it. Once and not per round for the
                    // reason the pairing advice below is.
                    if !said_nothing_answered {
                        said_nothing_answered = true;
                        host.warn(format_args!(
                            "nothing answered as the other machine; still asking, and the \
                         keyboard stays here until something does"
                        ));
                    }
                    failed = true;
                    continue;
                }
                // The host saying no attempt will work, which the keyboards are given
                // back before asking about: whatever the reason is, waiting for it to
                // change is waiting for a person.
                Connected::Done => return why_it_stopped(host, nowhere_to_look, seen),
                // `Connected` is `#[non_exhaustive]`: a kind this crate does not yet
                // name is nothing usable this time, the same as `NotFound`.
                _ => {
                    failed = true;
                    continue;
                }
            };
            // Said before the connection and the handshake, which are the two waits on this
            // path that cannot be taken in pieces: what is left of a bound after them is
            // what a supervisor has to allow (ADR-0008). Both are reached only once the
            // other machine has answered, so what they wait for is a machine that is there.
            beating.beat(host);

            // Found and not reachable is the same round again as not found: a machine
            // that answered mDNS and then refused the handshake is one to look for
            // afresh, since the sink may not have paired this machine yet.
            let Some(mut connection) = host.connect_socket(sink_to_reach, HANDSHAKE_WAIT) else {
                failed = true;
                continue;
            };
            let bounded = connection.set_nodelay(true)
                && connection.set_read_timeout(HANDSHAKE_WAIT)
                && connection.set_write_timeout(SEND_WAIT);
            if !bounded {
                failed = true;
                continue;
            }
            beating.beat(host);
            let opened = shake_hands(&identity, &sink, host, connection.as_mut());
            let Some(session_now) = opened else {
                // The refusal ADR-0004 asks for looks exactly like this: a sink that has
                // not pinned this machine ends the handshake rather than answering a
                // question about it, so there is nothing here to tell it apart from a
                // Mac that went away mid-handshake, and the advice is worth more than
                // the distinction.
                if !said_how_to_pair {
                    said_how_to_pair = true;
                    host.warn(format_args!(
                        "the sink did not answer the handshake; it may not have paired this \
                     machine — pair again, with favjit --pair on the Mac and favjit --pair \
                     <those digits> here"
                    ));
                }
                failed = true;
                continue;
            };
            // Kept beside the session it belongs to rather than given up here: the
            // handshake is over, and what carries the records that follow is the
            // same socket it was made on.
            session = Some(Open {
                session: session_now,
                over: connection.keep(),
            });
            // A link that has just come up has been told nothing, so every device is
            // announced over it again — and only here, because a link kept across a trip of
            // the keyboard has been told already.
            relaying.on_a_new_link();
        }

        refuse(host, Suppressing::Everything);
        let Some(open) = session.as_mut() else {
            // Unreachable: the block above either left a session or went round. Said as
            // a round again rather than an `expect`, because ending a run over a
            // condition that cannot arise is a keyboard taken away for a bug.
            continue;
        };

        // Which of the three ways out of the loop below happened, because they are not
        // the same thing: a link that dropped is one to wait for again, a keyboard
        // asked to come back is this machine's again, and a stream that ended is this
        // process finishing.
        let mut linked = true;
        // What the machine said about the write that ended the link, kept for the
        // sentence and the trace: the moment it is discovered is inside the loop
        // and what reads it is the round after.
        let mut answered = 0;
        loop {
            let Some(raw) = wait(host, seen) else {
                // The whole backlog on a wait that came back empty, which is the
                // only moment a run has spare: one record per keystroke would
                // take as many keystrokes as the link was down records, so a
                // session that came up with history to hand over would never
                // finish handing it over (ADR-0009).
                //
                // Before the run is asked whether it is over, because the way out
                // is the last chance to hand any of it over: a recording is what
                // outlives the run, and the records this one has not sent are the
                // ones about how it ended.
                while let Some(code) = hand_over_a_record(open, &mut trace, &mut seen.sent) {
                    if code != 0 {
                        linked = false;
                        answered = code;
                        break;
                    }
                }
                if !linked || nothing_more_is_coming(host, seen) {
                    break;
                }
                beating.beat(host);
                continue;
            };
            let Some(event) = resolve(raw, &mut resolver, host) else {
                beating.beat(host);
                continue;
            };
            record(&mut trace, Record::Event(event));
            let asked = asked_for(event.kind, &mut chording);
            // Not handed to `relay` like any other key: the chord asking for where the
            // keyboard already is does nothing (ADR-0013), and relayed it would type
            // option-`n` on the Mac — a dead key there. Only the press is caught,
            // because a release for a key the sink never saw go down is one
            // [`Forwarding::relay`] drops already.
            if asked == Some(Driving::TheSink) {
                beating.beat(host);
                continue;
            }
            if asked == Some(Driving::ThisMachine) {
                // Before the keyboard comes back, and whether or not each one gets
                // through: a link that has already gone holds nothing down either.
                for message in relaying.let_go() {
                    let handed = seal_and_send(open, *message, &mut seen.sent);
                    if let Some(at) = handed.at {
                        record(
                            &mut trace,
                            Record::Sent {
                                at,
                                code: handed.code,
                            },
                        );
                    }
                }
                driving = Driving::ThisMachine;
                beating.beat(host);
                break;
            }
            // An option the sink has not been told about goes over in front of the
            // key pressed under it, and not as the keyboard comes over: the chord
            // that sent it is made with option, so at that moment option is always
            // down, and sending it then puts a bare option press-and-release on the
            // Mac for every trip. Waiting for a key under it sends nothing for a
            // chord let go of cleanly and option-`a` for a finger that stayed. Through
            // `relay` so the sink records it as held, which is what lets its release
            // cross and what `let_go` reads on the way back; `relay` drops a press the
            // sink already holds, so this costs nothing once it has gone.
            let under_option =
                matches!(event.kind, EventKind::KeyDown { key, .. } if !is_option(key));
            let held: Vec<EventKind> = if under_option {
                chording.still_holding().collect()
            } else {
                Vec::new()
            };
            for kind in held.into_iter().chain(std::iter::once(event.kind)) {
                for message in relaying.relay(kind) {
                    let handed = seal_and_send(open, *message, &mut seen.sent);
                    let code = handed.code;
                    if let Some(at) = handed.at {
                        record(&mut trace, Record::Sent { at, code });
                    }
                    if code != 0 {
                        // Back to waiting rather than stopping: the other machine
                        // rebooting is a link that comes back, and giving the keys up
                        // in the meantime is what the person needs.
                        linked = false;
                        answered = code;
                        break;
                    }
                }
                if !linked {
                    break;
                }
            }
            if !linked {
                break;
            }
            // Behind the keystroke and not in front of it, one per way round: the
            // recording this machine keeps is what the other one has to hold too
            // (ADR-0009), and a session that came up with a backlog would spend a
            // whole wait writing history while the person types into nothing.
            if let Some(code) = hand_over_a_record(open, &mut trace, &mut seen.sent) {
                if code != 0 {
                    linked = false;
                    answered = code;
                    break;
                }
            }
            // After handling, not before: a heartbeat sent on the way in would
            // vouch for a loop that is about to wedge inside `send`.
            beating.beat(host);
        }

        // Thrown away only where it has gone, so the way over next time is the chord and
        // nothing else. A link that dropped is one to look for again from the start,
        // since the sink has released everything it was holding for this end.
        //
        // **`failed` is deliberately not set.** The pause it turns on is for a machine
        // that is not answering, and a link that was carrying keystrokes a moment ago is
        // the opposite of that: the Mac is there, something interrupted one send, and the
        // person is sitting at a keyboard that has just stopped reaching the screen they
        // were typing at. Sitting out a wait before the first attempt is the delay they
        // see. The attempt that follows sets it if it fails, so a machine that has
        // actually gone is still only asked for at that pace — and a link that comes up
        // and drops again pays a handshake per round rather than spinning, which is
        // already a slower cycle than the pause.
        if !linked {
            session = None;
            // Every drop and not once per run: a link going is an event, and a person
            // whose keyboard came back to this machine with nothing said has no way to
            // tell it from a chord that went unheard.
            say_the_link_has_gone(host, answered);
        }

        // Round again, to wait for the chord that asks for the other machine.
        if driving == Driving::ThisMachine {
            continue;
        }
        if linked {
            refuse(host, Suppressing::Nothing);
            return why_it_stopped(host, nowhere_to_look, seen);
        }
    }
}

/// The messages a recorded run's events cross the link as, in order.
///
/// What a source's replay reproduces is which messages went, and not the bytes:
/// sealing them is the transport, and a session is a handshake with a machine
/// that is not there when a trace is being read. So this takes the events a
/// recording holds and answers with what the sink was told — which is the
/// sequence a case built from that recording asserts against
/// ([`crate::trace::Record::Sent`] is what each one answered with, beside it).
///
/// The chord is honoured, because a recording that crossed it holds the events
/// either side and a replay that relayed straight through them would report
/// keystrokes the sink never heard.
pub fn replay(events: &[HostEvent]) -> Vec<Message> {
    let mut relaying = Forwarding::default();
    let mut chording = Chording::default();
    let mut driving = Driving::ThisMachine;
    let mut crossed = Vec::new();
    for event in events {
        let asked = asked_for(event.kind, &mut chording);
        if driving == Driving::ThisMachine {
            relaying.note(event.kind);
            if asked == Some(Driving::TheSink) {
                driving = Driving::TheSink;
                relaying.on_a_new_link();
            }
            continue;
        }
        if asked == Some(Driving::ThisMachine) {
            crossed.extend(relaying.let_go().iter().copied());
            driving = Driving::ThisMachine;
            continue;
        }
        crossed.extend(relaying.relay(event.kind).iter().copied());
    }
    crossed
}

/// This machine's own key, and which sink it will present it to — the answer
/// `favjit --identity` exists to produce, on the forwarding machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identification {
    pub fingerprint: String,
    /// The digits of the sink a relaying run would present it to, or `None` when
    /// nothing is pinned.
    pub sink: Option<String>,
}

/// This machine's identity and its pinned sink, read the same way a relaying run
/// reads them, and put into the shape `favjit --identity` prints.
///
/// Its own way in rather than two steps left to a binary: reading the pinned
/// file and turning what it names into digits are `pairing`'s
/// ([`pairing::pinned_sink`], [`pairing::written_as`]), and a binary calling
/// both directly would hold the order between them itself, where the suite
/// cannot drive it (ADR-0006).
pub fn identify(host: &mut dyn SourceHost) -> Result<Identification, pairing::NoIdentity> {
    let identity = pairing::identity(host)?;
    let sink = host
        .pinned_sink()
        .and_then(|text| pairing::pinned_sink(&text))
        .map(|key| pairing::written_as(&key));
    Ok(Identification {
        fingerprint: identity.fingerprint(),
        sink,
    })
}

/// One device this machine found attached, described the way a rule matches it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attached {
    pub keyboard: bool,
    pub path: String,
    /// The vendor and product a rule would match it on, or `None` for a
    /// keyboard with neither — the machine's own, behind no USB bus at all.
    pub identity: Option<(u16, u16)>,
}

/// Every device this machine found attached, described the way `favjit
/// --devices` shows it.
///
/// Takes what was found rather than finding it itself: enumerating this
/// machine's keyboards is a platform call with no decision in it, and singling
/// each one out by the vendor and product its path names is
/// [`crate::device::identity`]'s — reached here, once, rather than from a
/// binary that would hold the order between the two itself (ADR-0006).
pub fn describe_devices(attached: &[(bool, String)]) -> Vec<Attached> {
    attached
        .iter()
        .map(|(keyboard, path)| Attached {
            keyboard: *keyboard,
            path: path.clone(),
            identity: crate::device::identity(path),
        })
        .collect()
}

/// Every keyboard and mouse this machine has attached, as `(is a keyboard,
/// path)`.
///
/// The count before the list and the room before the path, which is the order
/// this kind of enumeration is written in: a machine says how much it needs
/// before it will fill any of it in. Here rather than behind the calls, because
/// it is a sequence — and a sequence written where no test can reach it is a
/// sequence held by whoever last read it (ADR-0006).
///
/// A device this machine lists and will not name is passed over rather than
/// ending the listing: what is being asked for is the devices a rule could
/// single out, and one with no path is not one of them.
pub fn what_is_attached(listing: &mut dyn Listing) -> Vec<(bool, String)> {
    let wanted = listing.how_many();
    let looked = listing.look(wanted);
    let mut found = Vec::new();
    for place in 0..looked {
        let keyboard = match listing.at(place) {
            Some(Kind::Keyboard) => true,
            Some(Kind::Pointer) => false,
            // Neither, and `Kind` is `#[non_exhaustive]`: what `--devices` shows
            // is what a rule can match, and a rule matches a keyboard or a
            // pointer.
            _ => continue,
        };
        let room = listing.how_long_its_path_is(place);
        found.extend(listing.its_path(place, room).map(|path| (keyboard, path)));
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_attached_device_is_described_by_its_own_path() {
        let attached = [
            (true, String::from(r"\\?\HID#VID_17EF&PID_60E1#foo")),
            (false, String::from(r"\\?\ACPI#PNP0303#bar")),
        ];

        let described = describe_devices(&attached);

        assert_eq!(described[0].identity, Some((0x17ef, 0x60e1)));
        assert_eq!(described[1].identity, None);
    }
}
