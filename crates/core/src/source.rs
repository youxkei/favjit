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
use crate::{Buttons, DeviceId, DeviceInfo, EventKind, Host, Key, PointerReport};

/// The boundary a source runs against.
///
/// Its own trait rather than [`crate::Host`]: a source injects nothing and a sink
/// sends nothing, and one trait covering both would give each an operation it must
/// never call. Every operation here is a call into the platform and the answer it
/// gave; what to do about the answer is [`run`]'s.
pub trait SourceHost: Host {
    /// Start reading this machine's keyboards and mice, and be ready to refuse
    /// them if `may_suppress`.
    ///
    /// `false` when they cannot be read at all, which ends the run before it opens
    /// a socket: a machine whose input this process cannot see has nothing to
    /// forward, and a link to say so with would be one the sink is sent nothing
    /// over.
    ///
    /// Being *ready* to refuse and refusing are two operations because they are two
    /// questions the platform answers differently: one may put a hook on the whole
    /// machine, and a run that will never refuse should not ask for that. When to
    /// refuse is [`SourceHost::suppress`]'s, and it is asked over and over.
    fn take_input(&mut self, may_suppress: bool) -> bool;

    /// Wait before looking for the sink again.
    ///
    /// The clock is the host's, so how long is too; that it happens between
    /// attempts and not before the first is [`run`]'s. Without it, a machine whose
    /// Mac is switched off looks as fast as the answer comes back.
    fn pause(&mut self);

    /// Look for the sink, and remember where it is.
    ///
    /// Blocks while it looks, the way the sink's accept blocks: a caller that had
    /// to poll would need a clock, and `core` has none. *Where* it is stays on this
    /// side — an address is a thing to connect with rather than anything the rules
    /// read.
    fn find_sink(&mut self) -> Connected;

    /// Open a session with the sink that was found.
    ///
    /// `false` for every way it can fail, since what to do about them is the same:
    /// the machine is not reachable, or it refused the handshake because it has not
    /// paired this one (ADR-0004), and either way the answer is to give the
    /// keyboards back and look again.
    fn open_session(&mut self) -> bool;

    /// Refuse this machine's own input, or stop refusing it.
    ///
    /// Separate from connecting because when it happens is a decision: the keys are
    /// only taken while there is a link to relay them over *and* the person has asked
    /// for the other machine ([`Driving`]).
    fn suppress(&mut self, what: Suppressing);

    /// Why the run can go no further.
    ///
    /// Asked once, after the stream has ended, rather than reported through
    /// [`SourceHost::next_event`]: what the run does about it is the same either
    /// way — it ends — and what differs is only what it says afterwards. A host
    /// that decided that would be deciding what the exit code means (ADR-0006).
    fn ended(&mut self) -> Ended;

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
}

/// What looking for the other machine produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Connected {
    /// It is there, and a session can be opened with it.
    Ready,
    /// The other machine is not there — asleep, rebooting, or not on this network.
    ///
    /// A kind of its own rather than an error, because it is the ordinary state of
    /// a machine that has not been switched on yet.
    NotFound,
    /// Nothing more will work: there is no identity, or no sink has been pinned.
    Done,
}

/// Why the run can go no further.
///
/// Its own rather than [`crate::Ended`]: the conditions a machine that forwards
/// stops under are not a converting machine's — there is no output device to lose
/// and nothing to switch converting off — and an enum carrying variants a role can
/// never return would be a host asked to pick a lie.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ended {
    /// The run was asked to stop, or the bound it was given has passed.
    AsAsked,
    /// Reading this machine's keyboards stopped.
    InputGone,
    /// There is nothing to relay to and there will not be.
    NoLink,
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

/// What an ended stream means for the run.
fn ending(ended: Ended) -> Ending {
    match ended {
        Ended::AsAsked => Ending::Relayed,
        Ended::InputGone => Ending::InputGone,
        Ended::NoLink => Ending::NoLink,
    }
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
struct Relaying {
    devices: Vec<Device>,
    /// What the last event turned into, reused rather than allocated per
    /// keystroke: this sits in the interactive path.
    out: Vec<Message>,
}

impl Relaying {
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
                    // is holding (ADR-0013), so relaying these would put two
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
            // The watchdog's question and this process's own wake-up are about
            // the machine they happened on.
            EventKind::Timer | EventKind::Probe => false,
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
            if let Some(message) = Message::of(kind) {
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
        self.out.push(Message::DeviceAttached(info));
    }
}

/// The device an event is *from*, where the sink has to know what it is before the
/// event means anything.
///
/// Neither of the two device events is one of those. An attach is the announcement,
/// so putting another in front of it would say the same thing twice; a detach is
/// the sink being told to forget a device, which is not a moment to introduce one.
fn subject(kind: EventKind) -> Option<DeviceId> {
    match kind {
        EventKind::KeyDown { device, .. }
        | EventKind::KeyUp { device, .. }
        | EventKind::Pointer { device, .. } => Some(device),
        EventKind::DeviceAttached(_)
        | EventKind::DeviceDetached(_)
        | EventKind::Timer
        | EventKind::Probe => None,
    }
}

/// Which machine the keyboard in front of the person is driving.
///
/// One keyboard cannot drive both: what is relayed has to be refused here, or every
/// keystroke lands on both screens. So this is a mode with two states rather than two
/// things happening at once, and the chords below are how a person moves between them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Driving {
    /// This machine. Nothing is relayed, and nothing is refused but the chord.
    ThisMachine,
    /// The machine at the other end of the link. Everything is refused here and sent
    /// there.
    TheSink,
}

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

/// What this machine's own input is refused for.
///
/// Three states, because there is a middle one: while the keyboard is this machine's,
/// the chord that moves it has to be refused or pressing it also reaches whatever has
/// the foreground. Refusing exactly that costs nothing where what refuses a key is also
/// what reports it — the chord is already on its way to [`run`] by the time it is
/// turned down.
///
/// Refusing one chord is not "the keyboard stopped working" — everything else arrives —
/// so it does not reach the outcome ADR-0008 rules out. It is not something a run can
/// be *asked* for either: [`Request`] still has two variants and this is a state inside
/// one of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Suppressing {
    /// Nothing at all: while there is no link, and where a run ends.
    Nothing,
    /// The chord that moves the keyboard, and nothing else.
    TheSwitch,
    /// Everything, because the input is going to the other machine.
    Everything,
}

/// Whether an option key is down, so the chord is recognised whatever is being relayed.
///
/// Kept here rather than read off [`Relaying`]: what that holds is what the *sink*
/// believes, and while the keyboard is driving this machine the sink believes nothing.
#[derive(Debug, Default)]
struct Chording {
    /// The option keys physically down, on any of this machine's keyboards. A person
    /// chords on one, and two keyboards holding option between them is not a case
    /// worth a rule of its own.
    option: Vec<Key>,
}

impl Chording {
    /// Follow this event, and say which machine it asks the keyboard to drive.
    ///
    /// `None` for everything else, which is nearly everything: the chord is two keys
    /// out of the whole stream, and what happens to the rest is [`run`]'s.
    fn asked_for(&mut self, kind: EventKind) -> Option<Driving> {
        match kind {
            EventKind::KeyDown { key, .. } if is_option(key) => {
                if !self.option.contains(&key) {
                    self.option.push(key);
                }
                None
            }
            EventKind::KeyUp { key, .. } if is_option(key) => {
                self.option.retain(|held| *held != key);
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
pub fn run(request: &Request, host: &mut dyn SourceHost) -> Ending {
    // Before anything is taken, because a cost named after the keyboards are refused
    // is one the person can no longer decide about.
    warn_about(request, host);

    // The shape of the request is what says whether refusing is ever going to be
    // wanted: only the variant that relays refuses anything, so a dry run does not
    // put a hook on the machine.
    if *request == Request::DryRun {
        // Neither the link nor suppression is reached, so a dry run is the one mode
        // that touches nothing outside this process.
        if !host.take_input(false) {
            return Ending::NoInput;
        }
        while host.next_event().is_some() {
            host.heartbeat();
        }
        return ending(host.ended());
    }

    // Before anything is connected to, because a machine whose input cannot be read
    // has nothing to forward. Able to refuse, always, for a run that relays: the
    // keystroke stays on this machine otherwise ([`Request::Relaying`]).
    if !host.take_input(true) {
        return Ending::NoInput;
    }

    let mut relaying = Relaying::default();
    let mut chording = Chording::default();
    // The other machine's from the start, because that is what `--dry-run false` asks
    // for and asking for it is the point: the person who typed it wants their keyboard
    // over there. [`SWITCH_BACK`] is how they get it back, and until they press it a
    // run behaves as it did before there was a chord at all.
    let mut driving = Driving::TheSink;
    // Whether the last attempt at a link failed, so the wait goes between attempts and
    // not in front of one: a source started beside a Mac that is already up should not
    // sit out a pause before it says anything, and neither should the keyboard the
    // person has just asked to move.
    let mut failed = false;
    loop {
        // The keyboard is this machine's, so nothing is refused but the chord that
        // moves it — which is refused so that pressing it does not also reach whatever
        // has the foreground (ADR-0018).
        if driving == Driving::ThisMachine {
            host.suppress(Suppressing::TheSwitch);
            loop {
                let Some(event) = host.next_event() else {
                    // Given back as the run stops, so nothing it refused outlives it —
                    // not even the one chord (ADR-0008).
                    host.suppress(Suppressing::Nothing);
                    return ending(host.ended());
                };
                let asked = chording.asked_for(event.kind);
                // Answered before anything is decided about it, for the reason every
                // other event is: the loop came back round (ADR-0008).
                host.heartbeat();
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
        host.suppress(Suppressing::Nothing);
        if failed {
            host.pause();
        }

        match host.find_sink() {
            Connected::Ready => failed = false,
            // Round again. Looking is what waits, so this is not a spin — and the
            // other machine coming back is the ordinary case, not an error.
            Connected::NotFound => {
                failed = true;
                continue;
            }
            // The host saying no attempt will work, which the keyboards are given
            // back before asking about: whatever the reason is, waiting for it to
            // change is waiting for a person.
            Connected::Done => return ending(host.ended()),
        }
        // Found and not reachable is the same round again as not found: a machine
        // that answered mDNS and then refused the handshake is one to look for
        // afresh, since the sink may not have paired this machine yet.
        if !host.open_session() {
            failed = true;
            continue;
        }
        host.suppress(Suppressing::Everything);
        relaying.on_a_new_link();

        // Which of the three ways out of the loop below happened, because they are not
        // the same thing: a link that dropped is one to wait for again, a keyboard
        // asked to come back is this machine's again, and a stream that ended is this
        // process finishing.
        let mut linked = true;
        while let Some(event) = host.next_event() {
            if chording.asked_for(event.kind) == Some(Driving::ThisMachine) {
                // Before the keyboard comes back, and whether or not each one gets
                // through: a link that has already gone holds nothing down either.
                for message in relaying.let_go() {
                    host.send(*message);
                }
                driving = Driving::ThisMachine;
                host.heartbeat();
                break;
            }
            for message in relaying.relay(event.kind) {
                if !host.send(*message) {
                    // Back to waiting rather than stopping: the other machine
                    // rebooting is a link that comes back, and giving the keys up
                    // in the meantime is what the person needs.
                    linked = false;
                    break;
                }
            }
            if !linked {
                break;
            }
            // After handling, not before: a heartbeat sent on the way in would
            // vouch for a loop that is about to wedge inside `send`.
            host.heartbeat();
        }

        // Round again, to wait for the chord that asks for the other machine.
        if driving == Driving::ThisMachine {
            continue;
        }
        if linked {
            host.suppress(Suppressing::Nothing);
            return ending(host.ended());
        }
    }
}
