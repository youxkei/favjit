//! The simulated machine the end-to-end suite runs against (ADR-0007).
//!
//! Not gated on `target_os`: it builds anywhere, which is what lets one process
//! stand in for a machine.
//!
//! All three surfaces ADR-0007 asks for: what arrives, what the run asked the
//! machine to do, and what the machine answers — including the answers that are
//! failures, since a run that cannot bring the output up is the one ADR-0008 is
//! about. Everything is a value set from outside rather than a real device, a real
//! file or a real socket.
//!
//! One role runs at a time, on the suite's own thread, because there is nothing for
//! a scheduler to explore: the link delivers in order and neither role observes the
//! other except through the messages that cross (ADR-0007). A loop a run hands over
//! to be turned alongside its own is turned here to a standstill and then returned
//! from, so the same thread carries both: what its events cross into the converter's
//! stream through is the queue two loops share on a real machine, and their
//! timestamps are what put them in order there.

use core::time::Duration;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use favjit_core::link::{Accepted, Incoming, LinkHost, Message, ANSWER, FRAME, HANDSHAKE, SEALED};
use favjit_core::pairing::KEY;
use favjit_core::pairing::{
    self, Authorized, Code, Entropy, Identity, IdentityStore, PairingHost, Secret, Side,
    SourcePairingHost, Started, OFFER, SEALED_KEY,
};
use favjit_core::sink::{SinkHost, SinkInputHost};
use favjit_core::source::{Connected, SourceHost};
use favjit_core::watchdog::{Beat, BeatKind, Exit, WatchdogHost};
use favjit_core::{
    DeviceId, DeviceInfo, Ended, EventKind, Host, HostEvent, Injected, Instant, Key, PointerReport,
};

/// One outbound call, with the time the sink made it.
///
/// The timestamp is part of the expected result rather than incidental to it:
/// for a layout converter, *when* a key was injected is behaviour (ADR-0007).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Record {
    pub at: Instant,
    pub injected: Injected,
}

/// What the program asked this machine to do, in order.
///
/// Recorded because the order is behaviour: input must not be taken before there is
/// somewhere to send it (ADR-0008), and the only way to see that is to see what was
/// asked for when.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Did {
    AskedIfSwitchedOn,
    WaitedForOn,
    AskedPermission,
    OpenedOutput,
    TunedOutput,
    BoundLink,
    StartedTheLink,
    TookInput { suppressing: bool },
    Warned,
}

/// A machine that does exactly what the script says, on a clock the script
/// owns.
#[derive(Debug, Clone)]
pub struct SimHost {
    /// Where the script is: the timestamp the next scripted event gets.
    cursor: Instant,
    /// The time of the event currently being handled, which is what outbound
    /// calls are stamped with.
    now: Instant,
    inbound: VecDeque<HostEvent>,
    /// The outstanding wake-up, if the sink asked for one.
    timer: Option<Instant>,
    records: Vec<Record>,
    heartbeats: Vec<Instant>,
    /// What the program asked for, and what this machine answers.
    did: Vec<Did>,
    switched_on: bool,
    /// What ends the stream, and after how many events.
    ends_after: Option<(usize, Ended)>,
    /// How many events have been handed over.
    handed_over: usize,
    permits_input: bool,
    output_comes_up: bool,
    input_can_be_taken: bool,
    supervised: bool,
    /// The lines the run put into this machine's log, in order.
    warnings: Vec<String>,
    /// The identity file, with no disk under it: what it holds, whether the
    /// machine's entropy will produce a keypair, and whether a write succeeds are
    /// the whole of what the sequence in `core` reads.
    identity_file: Option<Vec<u8>>,
    makes: Option<Identity>,
    keeps: bool,
    kept: Option<Vec<u8>>,
    /// The identity the run bound the link with, which is the key a peer would have
    /// to have pinned.
    listened_with: Option<Identity>,
    /// The link this machine will hand over when a run binds one.
    ///
    /// Taken rather than borrowed, because the loop that serves it owns everything
    /// it touches — what a test asks about afterwards it asks this machine, through
    /// the handle below.
    link: Option<SimLink>,
    observed: Arc<Mutex<Observed>>,
    /// What the link has put into the stream and the loop has not read yet.
    from_the_link: VecDeque<HostEvent>,
    binds_link: bool,
    starts_alongside: bool,
    /// Whether the loop turning alongside the converter's has stopped serving.
    alongside_stopped: bool,
}

impl Default for SimHost {
    /// A machine where everything the program asks for works, because a test about
    /// something else should not have to say so.
    fn default() -> Self {
        Self {
            cursor: Instant::ZERO,
            now: Instant::ZERO,
            inbound: VecDeque::new(),
            timer: None,
            records: Vec::new(),
            heartbeats: Vec::new(),
            did: Vec::new(),
            switched_on: true,
            ends_after: None,
            handed_over: 0,
            permits_input: true,
            output_comes_up: true,
            input_can_be_taken: true,
            supervised: true,
            warnings: Vec::new(),
            identity_file: None,
            makes: Identity::new(vec![0xaa; KEY], vec![0xbb; KEY]),
            keeps: true,
            kept: None,
            listened_with: None,
            link: None,
            observed: Arc::new(Mutex::new(Observed::default())),
            from_the_link: VecDeque::new(),
            binds_link: true,
            starts_alongside: true,
            alongside_stopped: false,
        }
    }
}

impl SimHost {
    pub fn new() -> Self {
        Self::default()
    }

    /// A machine where converting has been switched off.
    pub fn with_converting_off(mut self) -> Self {
        self.switched_on = false;
        self
    }

    /// A machine where converting is switched off after this many events, which is
    /// somebody choosing the menu item while favjit is running.
    pub fn with_converting_off_after(mut self, events: usize) -> Self {
        self.ends_after = Some((events, Ended::SwitchedOff));
        self
    }

    /// A machine whose output device goes away after this many events — the driver's
    /// daemon stopping under a running converter.
    pub fn with_output_lost_after(mut self, events: usize) -> Self {
        self.ends_after = Some((events, Ended::OutputGone));
        self
    }

    /// A machine that will not let this process read the keyboards.
    pub fn with_no_permission(mut self) -> Self {
        self.permits_input = false;
        self
    }

    /// A machine where the output device does not come up — the driver is not
    /// installed, or its daemon is not running.
    pub fn with_no_output(mut self) -> Self {
        self.output_comes_up = false;
        self
    }

    /// A machine that refuses to hand over its keyboards.
    pub fn with_no_input(mut self) -> Self {
        self.input_can_be_taken = false;
        self
    }

    /// A machine where nothing is watching this process, so a wedge would keep the
    /// keyboards (ADR-0008).
    pub fn with_no_watchdog(mut self) -> Self {
        self.supervised = false;
        self
    }

    /// The other machine, on the other end of this machine's link.
    ///
    /// Handed to the machine rather than served by the test, so that what puts the
    /// other machine's keystrokes into the stream the converter reads is the same
    /// code that does it on real hardware.
    pub fn with_link(mut self, link: SimLink) -> Self {
        self.observed = Arc::clone(&link.observed);
        self.link = Some(link);
        self
    }

    /// A machine where the socket the other machine connects to cannot be opened —
    /// the port taken, or a network this process may not listen on.
    pub fn with_no_link_socket(mut self) -> Self {
        self.binds_link = false;
        self
    }

    /// A machine that cannot turn a second loop at all.
    pub fn with_nothing_to_run_alongside(mut self) -> Self {
        self.starts_alongside = false;
        self
    }

    /// A machine whose identity file already holds these bytes, whatever they are.
    pub fn with_identity_file(mut self, bytes: &[u8]) -> Self {
        self.identity_file = Some(bytes.to_vec());
        self
    }

    /// The keypair this machine's entropy will produce.
    pub fn that_can_make(mut self, identity: Identity) -> Self {
        self.makes = Some(identity);
        self
    }

    /// A machine that can produce no keypair at all.
    pub fn with_no_keypair(mut self) -> Self {
        self.makes = None;
        self
    }

    /// A machine whose identity file cannot be written — a full disk, or a
    /// directory that is not there.
    pub fn with_an_unwritable_identity_file(mut self) -> Self {
        self.keeps = false;
        self
    }

    /// What was written to the identity file, if anything. `None` is the assertion
    /// that nothing was.
    pub fn kept(&self) -> Option<Vec<u8>> {
        self.kept.clone()
    }

    /// What the identity file holds now.
    pub fn identity_file(&self) -> Option<Vec<u8>> {
        self.identity_file.clone()
    }

    /// The identity the run listened with, if it listened.
    pub fn listened_with(&self) -> Option<Identity> {
        self.listened_with.clone()
    }

    /// Every call the link's loop made into this machine, in order.
    pub fn link_calls(&self) -> Vec<Call> {
        self.observed().calls.clone()
    }

    /// Every event the link put into the stream, in order.
    pub fn delivered(&self) -> Vec<EventKind> {
        self.observed().delivered.clone()
    }

    /// Every peer the link sent away.
    pub fn refused(&self) -> Vec<Vec<u8>> {
        self.observed().refused.clone()
    }

    /// Why the link let each connection go, in order.
    pub fn link_closed(&self) -> Vec<String> {
        self.observed().closed.clone()
    }

    /// How many frames the link took from peers at all.
    pub fn frames_read(&self) -> usize {
        self.observed().frames_read
    }

    /// How many times the link said on the network that this machine is here.
    pub fn advertisements(&self) -> usize {
        self.observed().advertised
    }

    fn observed(&self) -> std::sync::MutexGuard<'_, Observed> {
        self.observed.lock().expect("nothing panics holding this")
    }

    /// What the program asked this machine to do, in order.
    pub fn did(&self) -> Vec<Did> {
        self.did.clone()
    }

    /// Whether any keyboard was taken at all, and whether exclusively.
    pub fn took_input(&self) -> Option<bool> {
        self.did.iter().find_map(|did| match did {
            Did::TookInput { suppressing } => Some(*suppressing),
            _ => None,
        })
    }

    /// Load a recording as a script (ADR-0009).
    ///
    /// The whole point of the trace using the same vocabulary as this host: a run
    /// captured on real hardware is played back by the same code the suite is
    /// written against, so a field incident becomes a test rather than a story.
    ///
    /// The events carry their own timestamps, so the script's clock is set from
    /// them rather than advanced by hand — replaying a trace against a clock this
    /// host invented would answer a tap-versus-hold question differently from the
    /// machine it came from.
    pub fn from_trace(trace: &favjit_core::trace::Reader<'_>) -> Self {
        let mut host = Self::default();
        for event in trace.events() {
            host.inbound.push_back(event);
        }
        if let Some(last) = host.inbound.back() {
            host.cursor = last.at;
        }
        host
    }

    /// Move the script's clock forward. Nothing else happens — no time passes
    /// anywhere real, which is the point.
    pub fn advance(&mut self, by: Duration) -> &mut Self {
        self.cursor = self.cursor.saturating_add(by);
        self
    }

    /// Announce a keyboard.
    pub fn attach(&mut self, info: DeviceInfo) -> &mut Self {
        self.script(EventKind::DeviceAttached(info))
    }

    /// Take a keyboard away without releasing anything first — the shape of an
    /// unplugged cable, and of the stuck-modifier case ADR-0002 puts on the
    /// sink.
    pub fn detach(&mut self, id: DeviceId) -> &mut Self {
        self.script(EventKind::DeviceDetached(id))
    }

    pub fn press(&mut self, device: DeviceId, key: Key) -> &mut Self {
        self.script(EventKind::KeyDown { device, key })
    }

    pub fn release(&mut self, device: DeviceId, key: Key) -> &mut Self {
        self.script(EventKind::KeyUp { device, key })
    }

    pub fn tap(&mut self, device: DeviceId, key: Key) -> &mut Self {
        self.press(device, key).release(device, key)
    }

    /// One pointer report from a device, as the hardware described it.
    ///
    /// Takes a whole report rather than a movement and a button separately,
    /// because that is the unit the OS accelerates: a script that could only
    /// nudge one axis at a time could not express a diagonal.
    pub fn pointer(&mut self, device: DeviceId, report: PointerReport) -> &mut Self {
        self.script(EventKind::Pointer { device, report })
    }

    /// Ask, as a watchdog would, whether the loop is still turning.
    pub fn probe(&mut self) -> &mut Self {
        self.script(EventKind::Probe)
    }

    /// Press, hold for `held`, release.
    pub fn hold(&mut self, device: DeviceId, key: Key, held: Duration) -> &mut Self {
        self.press(device, key).advance(held).release(device, key)
    }

    pub fn script(&mut self, kind: EventKind) -> &mut Self {
        self.inbound.push_back(HostEvent::new(self.cursor, kind));
        self
    }

    /// Every outbound call, in order, with its timestamp.
    pub fn records(&self) -> &[Record] {
        &self.records
    }

    /// Every outbound call, in order, without the timestamps — for assertions
    /// about what was injected rather than when.
    pub fn injected(&self) -> Vec<Injected> {
        self.records.iter().map(|r| r.injected).collect()
    }

    /// When the loop reported coming back round, in order.
    pub fn heartbeats(&self) -> &[Instant] {
        &self.heartbeats
    }

    /// What the run said about itself, in order.
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }
}

impl SinkInputHost for SimHost {
    fn switched_on(&mut self) -> bool {
        self.did.push(Did::AskedIfSwitchedOn);
        self.switched_on
    }

    fn wait_until_on(&mut self) {
        // Comes back at once and nothing has passed: what a test about the switch
        // asks is what was and was not done, and a wait that really waited would
        // only make the suite slow.
        self.did.push(Did::WaitedForOn);
    }

    fn may_read_input(&mut self) -> bool {
        self.did.push(Did::AskedPermission);
        self.permits_input
    }

    fn take_input(&mut self, suppress: bool) -> bool {
        self.did.push(Did::TookInput {
            suppressing: suppress,
        });
        self.input_can_be_taken
    }

    /// What the script says first, since that is what a test about the switch or the
    /// output device asked for; the link is what is left when nothing else ended it.
    fn ended(&mut self) -> Ended {
        match self.ends_after {
            Some((after, why)) if self.handed_over >= after => why,
            _ if self.alongside_stopped => Ended::AlongsideStopped,
            _ => Ended::AsAsked,
        }
    }
}

impl Host for SimHost {
    /// Returns `None` once the script is exhausted, which ends the role's loop.
    /// A real host would park here instead; the suite has nothing to wait for.
    ///
    /// A wake-up that comes due before the next scripted event is delivered
    /// first, so the stream stays in time order — a timer fired as soon as it was
    /// asked for would let a repeat overtake the keystroke that cancels it. Once
    /// the script runs out the run ends even with a wake-up outstanding: a test
    /// that leaves a key held would otherwise repeat until the clock overflowed.
    fn next_event(&mut self) -> Option<HostEvent> {
        // A machine whose switch goes off, or whose output device stops answering,
        // has no more events to give: this simulated one stops giving them at the
        // point the script says, which is what a real one does the moment it
        // notices.
        if self
            .ends_after
            .is_some_and(|(after, _)| self.handed_over >= after)
        {
            return None;
        }
        // Whatever the link has put there since the last one, taken out of the
        // shared queue rather than read from it in place: past this point the two
        // sources of events are one stream, which is what makes the order the loop
        // sees the order things happened (ADR-0006).
        let arrived: Vec<HostEvent> = self.observed().stream.drain(..).collect();
        self.from_the_link.extend(arrived);

        // The keyboards in front of the person first where two arrive at once. Not
        // because it matters to the rules — they read the timestamp, and it is the
        // same one — but because a tie has to break the same way every run for the
        // suite to be worth anything.
        let next_local = self.inbound.front().map(|event| event.at);
        let next_remote = self.from_the_link.front().map(|event| event.at);
        let soonest = match (next_local, next_remote) {
            (Some(local), Some(remote)) => Some(local.min(remote)),
            (local, remote) => local.or(remote),
        };
        let event = match (self.timer, soonest) {
            (Some(due), Some(next)) if due <= next => {
                self.timer = None;
                HostEvent::new(due, EventKind::Timer)
            }
            _ => {
                let from_the_link = match (next_local, next_remote) {
                    (Some(local), Some(remote)) => remote < local,
                    (None, Some(_)) => true,
                    _ => false,
                };
                if from_the_link {
                    self.from_the_link.pop_front()?
                } else {
                    self.inbound.pop_front()?
                }
            }
        };
        self.now = event.at;
        self.handed_over += 1;
        Some(event)
    }

    fn is_supervised(&mut self) -> bool {
        self.supervised
    }

    fn warn(&mut self, message: core::fmt::Arguments) {
        // Formatted here rather than kept as it was passed: `Arguments` borrows what
        // it will print, so a machine that stored it could not answer afterwards —
        // which is the whole of what this machine is for.
        self.warnings.push(message.to_string());
        self.did.push(Did::Warned);
    }

    fn heartbeat(&mut self) {
        self.heartbeats.push(self.now);
    }
}

impl SinkHost for SimHost {
    fn set_timer(&mut self, at: Option<Instant>) {
        self.timer = at;
    }

    fn open_output(&mut self) -> bool {
        self.did.push(Did::OpenedOutput);
        self.output_comes_up
    }

    fn tune_output(&mut self) {
        self.did.push(Did::TunedOutput);
    }

    fn bind_link(&mut self, identity: &Identity) -> Option<Box<dyn LinkHost + Send>> {
        self.did.push(Did::BoundLink);
        if !self.binds_link {
            return None;
        }
        self.listened_with = Some(identity.clone());
        // A machine whose script mentioned no other machine still opens the socket,
        // because that is what a sink with nobody typing on the other end is.
        let link = self.link.take().unwrap_or_else(|| SimLink {
            observed: Arc::clone(&self.observed),
            ..SimLink::default()
        });
        Some(Box::new(link))
    }

    /// Turned to a standstill before returning, on this thread.
    ///
    /// The alternative is a thread, and then what the suite checked would depend on
    /// which loop a scheduler picked: the link's events go into the stream before
    /// the converter reads it either way, and their timestamps are what put them in
    /// order once it does (ADR-0007).
    fn run_alongside(&mut self, work: Box<dyn FnOnce() + Send>) -> bool {
        self.did.push(Did::StartedTheLink);
        if !self.starts_alongside {
            return false;
        }
        work();
        // Coming back is the socket no longer being served, unless it came back
        // because the script had nothing more in it. Recorded rather than reported
        // now: the events that loop delivered are still ahead of it in the stream,
        // and a run told here would end with them unread.
        let ran_out = self.observed().script_ran_out;
        self.alongside_stopped = !ran_out;
        true
    }

    fn inject(&mut self, injected: Injected) {
        self.records.push(Record {
            at: self.now,
            injected,
        });
    }
}

impl IdentityStore for SimHost {
    fn read(&mut self) -> Option<Vec<u8>> {
        self.identity_file.clone()
    }

    fn make(&mut self) -> Option<Identity> {
        self.makes.clone()
    }

    fn keep(&mut self, bytes: &[u8]) -> bool {
        if !self.keeps {
            return false;
        }
        self.identity_file = Some(bytes.to_vec());
        self.kept = Some(bytes.to_vec());
        true
    }
}

/// One message the source handed to the link, with the time it saw the input.
///
/// The time is kept because the sink stamps arrival: a suite that threw it away
/// could not deliver the same script to both ends and compare, which is the whole
/// test of the relay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sent {
    pub at: Instant,
    pub message: Message,
}

/// A machine standing in for the one input comes from.
///
/// Separate from [`SimHost`] because the two roles have separate boundaries: a
/// source sends and never injects. Sharing one type would give each role a
/// surface it must never touch.
#[derive(Debug, Default, Clone)]
pub struct SimSource {
    cursor: Instant,
    now: Instant,
    inbound: VecDeque<HostEvent>,
    sent: Vec<Sent>,
    heartbeats: Vec<Instant>,
    /// How many events had been scripted when the link went away.
    gone_after: Option<usize>,
    /// How many have been handed over so far, which is what that is compared to.
    taken: usize,
    /// How many times the other machine is not there yet.
    missing: usize,
    connects: usize,
    /// Whether the last answer was a link.
    linked: bool,
    /// Whether the keyboards are taken, and how many messages had been sent the
    /// first time they were — which is what says nothing was taken too early.
    suppressing: bool,
    suppressions: usize,
    suppressed_before_connecting: usize,
}

impl SimSource {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn advance(&mut self, by: Duration) -> &mut Self {
        self.cursor = self.cursor.saturating_add(by);
        self
    }

    pub fn attach(&mut self, info: DeviceInfo) -> &mut Self {
        self.script(EventKind::DeviceAttached(info))
    }

    pub fn detach(&mut self, id: DeviceId) -> &mut Self {
        self.script(EventKind::DeviceDetached(id))
    }

    pub fn press(&mut self, device: DeviceId, key: Key) -> &mut Self {
        self.script(EventKind::KeyDown { device, key })
    }

    pub fn release(&mut self, device: DeviceId, key: Key) -> &mut Self {
        self.script(EventKind::KeyUp { device, key })
    }

    pub fn tap(&mut self, device: DeviceId, key: Key) -> &mut Self {
        self.press(device, key).release(device, key)
    }

    pub fn pointer(&mut self, device: DeviceId, report: PointerReport) -> &mut Self {
        self.script(EventKind::Pointer { device, report })
    }

    /// Ask, as this machine's own watchdog would, whether the loop is turning.
    pub fn probe(&mut self) -> &mut Self {
        self.script(EventKind::Probe)
    }

    /// The other machine is not there for this many attempts.
    pub fn sink_missing(&mut self, times: usize) -> &mut Self {
        self.missing = times;
        self
    }

    /// How many times a link was asked for.
    pub fn connects(&self) -> usize {
        self.connects
    }

    /// Whether the keyboards are taken as things stand.
    pub fn suppressing(&self) -> bool {
        self.suppressing
    }

    /// How many times the keyboards were taken.
    pub fn suppressions(&self) -> usize {
        self.suppressions
    }

    /// How many times the keyboards were taken while there was no link.
    pub fn suppressed_before_connecting(&self) -> usize {
        self.suppressed_before_connecting
    }

    /// The link to the other machine has gone, for everything scripted after this.
    ///
    /// A point in the script rather than a flag set before the run, because when it
    /// happens is the thing worth testing: a link that was already gone could not
    /// show that what was relayed before it stayed relayed.
    pub fn link_gone(&mut self) -> &mut Self {
        self.gone_after = Some(self.inbound.len());
        self
    }

    pub fn script(&mut self, kind: EventKind) -> &mut Self {
        self.inbound.push_back(HostEvent::new(self.cursor, kind));
        self
    }

    /// Everything that went to the link, in order.
    pub fn sent(&self) -> &[Sent] {
        &self.sent
    }

    pub fn heartbeats(&self) -> &[Instant] {
        &self.heartbeats
    }
}

impl SourceHost for SimSource {
    fn connect(&mut self) -> Connected {
        self.connects += 1;
        if self.missing > 0 {
            self.missing -= 1;
            self.linked = false;
            return Connected::NotFound;
        }
        self.linked = true;
        Connected::Ready
    }

    fn suppress(&mut self, taking: bool) {
        if taking {
            self.suppressions += 1;
            if !self.linked {
                self.suppressed_before_connecting += 1;
            }
        }
        self.suppressing = taking;
    }

    fn next_event(&mut self) -> Option<HostEvent> {
        let event = self.inbound.pop_front()?;
        self.now = event.at;
        self.taken += 1;
        Some(event)
    }

    fn send(&mut self, message: Message) -> bool {
        if self.gone_after.is_some_and(|mark| self.taken > mark) {
            return false;
        }
        // Through the bytes rather than handing the value over: what the sink will
        // read is a frame, and a suite that passed the message straight across
        // would leave the encoding untested by every test that uses this.
        let mut frame = [0u8; FRAME];
        message.encode(&mut frame);
        let arrived = Message::decode(&frame).expect("a frame this end wrote is one it can read");
        self.sent.push(Sent {
            at: self.now,
            message: arrived,
        });
        true
    }

    fn heartbeat(&mut self) {
        self.heartbeats.push(self.now);
    }
}

/// A machine standing in for the network, for the sink's end of the link.
///
/// Scripted the same way as the others: connections and the frames inside them go
/// in, and what the sequence did with them comes out. What it stands in for is a
/// socket and a handshake, so nothing here is encrypted — the question these
/// answer is who was let in and what was read, not whether ChaCha works.
#[derive(Debug, Default, Clone)]
pub struct SimLink {
    /// What the authorised list says, as its text — the form a host reads it in.
    authorized: String,
    /// What is still to happen, in order.
    ///
    /// Pairing is in here with the connections rather than applied as the script is
    /// written, because when it happens is the thing worth testing: a list that was
    /// already complete before the run started could not show that authorising a
    /// source takes effect on the connection after it.
    script: VecDeque<Step>,
    /// The connection being served, if any.
    open: Option<Session>,
    /// Where the script is, and the time the record being handled arrived.
    cursor: Instant,
    now: Instant,
    /// Whether each record taken will open, in the order they were taken.
    opens: VecDeque<bool>,
    converter_gone: bool,
    /// What this end did, and what it put into the converter's stream.
    ///
    /// Shared rather than held, because the loop serving the link is turned
    /// somewhere this object has been given away to: a test asks the machine what
    /// its link did, the way it would ask a machine anything else.
    observed: Arc<Mutex<Observed>>,
}

/// What the link's loop did, as the machine it runs on can be asked about it.
#[derive(Debug, Default)]
struct Observed {
    advertised: usize,
    delivered: Vec<EventKind>,
    refused: Vec<Vec<u8>>,
    closed: Vec<String>,
    calls: Vec<Call>,
    frames_read: usize,
    /// What the link put into the stream the converter reads, waiting to be read
    /// out of it — the queue the two loops share on a real machine.
    stream: VecDeque<HostEvent>,
    /// Whether the link's loop came back because this test had nothing more to
    /// say, rather than because the socket stopped being served.
    script_ran_out: bool,
}

/// What the sink asked this end to do, in order.
///
/// Recorded because the order is the thing under test: every one of these is a
/// single call into a platform, so what can be wrong is which one happens when, and
/// whether one happens at all after the one before it failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Call {
    Accepted,
    TookHandshake,
    Answered,
    SentAnswer,
    Peer,
    Authorized,
    TookRecord,
    Opened,
}

#[derive(Debug, Clone)]
enum Step {
    Connect(Session),
    Pair(Vec<u8>),
    /// Something connected and never became a session.
    Rejected,
    /// The socket itself is unusable from here on.
    ListenerGone,
}

#[derive(Debug, Clone)]
struct Session {
    peer: Vec<u8>,
    /// Whether it sends a first message at all, whether that message opens, and
    /// whether the answer can still be sent back to it.
    speaks: bool,
    opens: bool,
    reachable: bool,
    records: VecDeque<Arrival>,
}

/// One record on its way in.
#[derive(Debug, Clone)]
struct Arrival {
    record: [u8; SEALED],
    /// Whether this end can open it.
    opens: bool,
    /// When the source saw the input inside it. Carried rather than stamped on
    /// arrival, because a link with no latency is what makes input from the other
    /// machine comparable against input from this one (ADR-0003).
    at: Instant,
}

impl Default for Session {
    fn default() -> Self {
        Self {
            peer: Vec::new(),
            speaks: true,
            opens: true,
            reachable: true,
            records: VecDeque::new(),
        }
    }
}

impl SimLink {
    /// Start with this list of authorised keys, as text.
    pub fn new(authorized: String) -> Self {
        Self {
            authorized,
            ..Self::default()
        }
    }

    /// A peer connects. Whatever is scripted after this belongs to it, until the
    /// next `connect` or a `hang_up`.
    pub fn connect(&mut self, peer: Vec<u8>) -> &mut Self {
        self.script.push_back(Step::Connect(Session {
            peer,
            ..Session::default()
        }));
        self
    }

    /// Something connects and never sends its first message.
    pub fn connects_and_says_nothing(&mut self) -> &mut Self {
        self.script.push_back(Step::Connect(Session {
            speaks: false,
            ..Session::default()
        }));
        self
    }

    /// Something connects and sends a first message this end cannot open — a
    /// machine pinned to another key, or one not speaking this protocol at all.
    pub fn connects_with_nonsense(&mut self) -> &mut Self {
        self.script.push_back(Step::Connect(Session {
            opens: false,
            ..Session::default()
        }));
        self
    }

    /// Something connects, and is gone by the time the answer is written.
    pub fn connects_and_drops_before_the_answer(&mut self) -> &mut Self {
        self.script.push_back(Step::Connect(Session {
            reachable: false,
            ..Session::default()
        }));
        self
    }

    /// A record that will not open, whatever is nominally inside it.
    pub fn sends_a_record_that_will_not_open(&mut self) -> &mut Self {
        self.record([0u8; SEALED], false)
    }

    /// Authorise a key, as `favjit --pair` would between two connections.
    pub fn pair(&mut self, key: Vec<u8>) -> &mut Self {
        self.script.push_back(Step::Pair(key));
        self
    }

    pub fn attach(&mut self, info: DeviceInfo) -> &mut Self {
        self.send(Message::DeviceAttached(info))
    }

    pub fn detach(&mut self, id: DeviceId) -> &mut Self {
        self.send(Message::DeviceDetached(id))
    }

    pub fn press(&mut self, device: DeviceId, key: Key) -> &mut Self {
        self.send(Message::KeyDown { device, key })
    }

    pub fn release(&mut self, device: DeviceId, key: Key) -> &mut Self {
        self.send(Message::KeyUp { device, key })
    }

    /// Something connects and the handshake does not complete.
    pub fn rejected(&mut self) -> &mut Self {
        self.script.push_back(Step::Rejected);
        self
    }

    /// Something connects and cannot be taken, this many times in a row — a machine
    /// on the same desk being switched off, or something scanning the port.
    pub fn rejected_times(&mut self, times: usize) -> &mut Self {
        for _ in 0..times {
            self.rejected();
        }
        self
    }

    /// The socket stops being usable — the descriptors are gone, or the interface
    /// it was bound to is.
    pub fn listener_gone(&mut self) -> &mut Self {
        self.script.push_back(Step::ListenerGone);
        self
    }

    /// The converter has stopped, so nothing more can be delivered to it.
    pub fn converter_stopped(&mut self) -> &mut Self {
        self.converter_gone = true;
        self
    }

    /// A frame no version of this end can read.
    pub fn nonsense(&mut self) -> &mut Self {
        let mut frame = [0u8; FRAME];
        frame[0] = 0xff;
        self.frame(frame)
    }

    /// The peer goes away, ending its session.
    pub fn hang_up(&mut self) -> &mut Self {
        self
    }

    /// Move the peer's clock forward, so a hold at the other end is a hold.
    pub fn advance(&mut self, by: Duration) -> &mut Self {
        self.cursor = self.cursor.saturating_add(by);
        self
    }

    /// Everything a source relayed, as it comes off the wire.
    ///
    /// Each message keeps the time the source saw the input rather than being
    /// restamped here: whether a key was tapped or held is what those times decide,
    /// and a run of the source is where they were made.
    pub fn relay(&mut self, sent: &[Sent]) -> &mut Self {
        for message in sent {
            self.cursor = message.at;
            self.send(message.message);
        }
        self
    }

    fn send(&mut self, message: Message) -> &mut Self {
        let mut frame = [0u8; FRAME];
        message.encode(&mut frame);
        self.frame(frame)
    }

    /// A frame, sealed the way this end seals one: the bytes as they are, since
    /// there is no cipher here and what a test is about is the frame.
    fn frame(&mut self, frame: [u8; FRAME]) -> &mut Self {
        let mut record = [0u8; SEALED];
        record[..FRAME].copy_from_slice(&frame);
        self.record(record, true)
    }

    fn record(&mut self, record: [u8; SEALED], opens: bool) -> &mut Self {
        let at = self.cursor;
        // Onto the last connection scripted, which is the one a reader of the
        // script would take it to belong to.
        if let Some(Step::Connect(session)) = self
            .script
            .iter_mut()
            .rev()
            .find(|step| matches!(step, Step::Connect(_)))
        {
            session.records.push_back(Arrival { record, opens, at });
        }
        self
    }

    /// Every event that reached the converter, in order.
    pub fn delivered(&self) -> Vec<EventKind> {
        self.observed().delivered.clone()
    }

    /// Every peer that was sent away.
    pub fn refused(&self) -> Vec<Vec<u8>> {
        self.observed().refused.clone()
    }

    /// Why each connection was let go, in order.
    pub fn closed(&self) -> Vec<String> {
        self.observed().closed.clone()
    }

    pub fn disconnects(&self) -> usize {
        self.observed().closed.len()
    }

    /// How many frames were taken from peers at all — which is zero for a
    /// connection that was refused.
    pub fn frames_read(&self) -> usize {
        self.observed().frames_read
    }

    /// Every call the sink made, in order.
    pub fn calls(&self) -> Vec<Call> {
        self.observed().calls.clone()
    }

    /// How many times this end said on the network that it is here.
    pub fn advertisements(&self) -> usize {
        self.observed().advertised
    }

    /// The lock is never held across a call into this object, so nothing can be
    /// waiting on it.
    fn observed(&self) -> std::sync::MutexGuard<'_, Observed> {
        self.observed.lock().expect("nothing panics holding this")
    }

    fn called(&mut self, call: Call) {
        self.observed().calls.push(call);
    }
}

impl LinkHost for SimLink {
    fn advertise(&mut self) -> bool {
        self.observed().advertised += 1;
        true
    }

    fn accept(&mut self) -> Accepted {
        loop {
            match self.script.pop_front() {
                // A real listener with nobody connecting parks in this call, and a
                // simulated one cannot: running out of script is this test having
                // nothing further to say, not the socket going away. What that
                // would be is scripted.
                None => {
                    self.observed().script_ran_out = true;
                    return Accepted::Done;
                }
                Some(Step::Pair(key)) => {
                    self.authorized = Authorized::added(&self.authorized, &key)
                }
                Some(Step::Rejected) => return Accepted::Failed,
                Some(Step::ListenerGone) => return Accepted::Done,
                Some(Step::Connect(session)) => {
                    self.open = Some(session);
                    self.called(Call::Accepted);
                    return Accepted::Connected;
                }
            }
        }
    }

    fn take_handshake(&mut self) -> Option<[u8; HANDSHAKE]> {
        self.called(Call::TookHandshake);
        self.open
            .as_ref()
            .filter(|session| session.speaks)
            .map(|_| [0u8; HANDSHAKE])
    }

    fn answer(&mut self, _first: &[u8; HANDSHAKE]) -> Option<[u8; ANSWER]> {
        self.called(Call::Answered);
        self.open
            .as_ref()
            .filter(|session| session.opens)
            .map(|_| [0u8; ANSWER])
    }

    fn send_answer(&mut self, _answer: &[u8; ANSWER]) -> bool {
        self.called(Call::SentAnswer);
        self.open.as_ref().is_some_and(|session| session.reachable)
    }

    fn peer(&mut self) -> Option<Vec<u8>> {
        self.called(Call::Peer);
        self.open.as_ref().map(|session| session.peer.clone())
    }

    fn authorized(&mut self) -> Authorized {
        self.called(Call::Authorized);
        Authorized::parse(&self.authorized)
    }

    fn take_record(&mut self) -> Incoming {
        self.called(Call::TookRecord);
        match self.open.as_mut().and_then(|open| open.records.pop_front()) {
            Some(arrival) => {
                self.now = arrival.at;
                self.observed().frames_read += 1;
                self.opens.push_back(arrival.opens);
                Incoming::Record(arrival.record)
            }
            None => Incoming::Ended,
        }
    }

    fn open(&mut self, record: &[u8; SEALED]) -> Option<[u8; FRAME]> {
        self.called(Call::Opened);
        if !self.opens.pop_front().unwrap_or(false) {
            return None;
        }
        let mut frame = [0u8; FRAME];
        frame.copy_from_slice(&record[..FRAME]);
        Some(frame)
    }

    fn deliver(&mut self, kind: EventKind) -> bool {
        if self.converter_gone {
            return false;
        }
        let at = self.now;
        let mut observed = self.observed();
        observed.delivered.push(kind);
        observed.stream.push_back(HostEvent::new(at, kind));
        true
    }

    fn close(&mut self, reason: &str) {
        self.observed().closed.push(String::from(reason));
        if let Some(session) = self.open.take() {
            // Kept, because who was sent away is what a test about authorisation
            // asks — and after the handshake there is nobody else it could be.
            self.observed().refused.push(session.peer);
        }
    }
}

/// Bytes a run can be repeated with, standing in for the machine's own source.
///
/// A stream from a fixed seed rather than the platform's, because a run given
/// different bytes each time could not fail the same way twice (ADR-0007). Nothing
/// here is a judgement about randomness: what a real host answers with is the
/// machine's source, and what the suite pins is what the exchange does with whatever
/// arrives.
#[derive(Debug, Clone)]
struct Trickle {
    state: u64,
    /// A machine that cannot produce bytes at all, which is what a code that could
    /// not be made looks like from here.
    dry: bool,
}

impl Trickle {
    fn new(seed: u64) -> Self {
        Self {
            state: seed,
            dry: false,
        }
    }

    fn dry() -> Self {
        Self {
            state: 1,
            dry: true,
        }
    }

    fn fill(&mut self, into: &mut [u8]) -> bool {
        if self.dry {
            return false;
        }
        for byte in into.iter_mut() {
            self.state ^= self.state << 13;
            self.state ^= self.state >> 7;
            self.state ^= self.state << 17;
            *byte = (self.state >> 24) as u8;
        }
        true
    }
}

/// Digits that are not the ones shown, for a source that was given the code wrong.
///
/// Derived from the code rather than written down, so that a change to how a code is
/// produced cannot turn these into the right ones by accident.
fn other_digits(code: Code) -> Code {
    let mut wrong = code;
    wrong[0] = match code[0] {
        b'0' => b'1',
        _ => b'0',
    };
    wrong
}

/// A machine standing in for the one a code is entered on, with a source in front of
/// it.
///
/// **The exchange itself really runs.** The source this scripts is played with the
/// same functions the forwarding machine's own run calls, so a source that was given
/// other digits ends up holding nothing because the arithmetic says so — not because
/// this compared two codes. What is stood in for is the machine: a screen, a socket,
/// a file, and bytes from a stream a run can be repeated with (ADR-0007).
pub struct SimPairing {
    entropy: Trickle,
    shown: Option<Code>,
    /// The sources waiting to be served, in order. Each carries its key and whether
    /// it entered the code correctly.
    sources: VecDeque<Source>,
    /// How many of them were actually served.
    attempts: usize,
    /// The source's half of the exchange, while it is in flight.
    started: Option<Started>,
    /// What the source agreed on, once it has.
    agreed: Option<Secret>,
    /// What was sent back to the source, as the source could read it.
    gave: Option<Vec<u8>>,
    authorized: String,
    keeps: bool,
    /// Whether the code was shown before a source was waited for, and whether the
    /// source's key had arrived before anything was written down. Both are order,
    /// which is behaviour.
    showed_before_waiting: bool,
    took_the_key_before_authorizing: bool,
    took_the_key: bool,
}

/// One source waiting to be served, as the script describes it.
#[derive(Debug, Clone)]
struct Source {
    key: Vec<u8>,
    knows_the_code: bool,
}

impl Default for SimPairing {
    /// A machine that can produce a code and has nobody answering it.
    fn default() -> Self {
        Self {
            entropy: Trickle::new(0x5111_ca11),
            shown: None,
            sources: VecDeque::new(),
            attempts: 0,
            started: None,
            agreed: None,
            gave: None,
            authorized: String::new(),
            keeps: true,
            showed_before_waiting: false,
            took_the_key_before_authorizing: false,
            took_the_key: false,
        }
    }
}

impl SimPairing {
    pub fn new() -> Self {
        Self::default()
    }

    /// A source that entered the code correctly, presenting this key.
    pub fn with_a_source_that_knows_the_code(mut self, key: &[u8]) -> Self {
        self.sources.push_back(Source {
            key: key.to_vec(),
            knows_the_code: true,
        });
        self
    }

    /// A source that entered something else.
    pub fn with_a_source_that_has_the_code_wrong(mut self, key: &[u8]) -> Self {
        self.sources.push_back(Source {
            key: key.to_vec(),
            knows_the_code: false,
        });
        self
    }

    /// One more behind it, for a test about the code being spent.
    pub fn and_then_a_source_that_knows_it(self, key: &[u8]) -> Self {
        self.with_a_source_that_knows_the_code(key)
    }

    /// A machine whose entropy will not produce a code.
    pub fn with_no_code(mut self) -> Self {
        self.entropy = Trickle::dry();
        self
    }

    /// A machine that cannot write the list down.
    pub fn that_cannot_keep_the_list(mut self) -> Self {
        self.keeps = false;
        self
    }

    /// The code that was put in front of the person, if one was.
    pub fn shown(&self) -> Option<Code> {
        self.shown
    }

    /// How many sources were served. One at most, because the code is spent.
    pub fn attempts(&self) -> usize {
        self.attempts
    }

    /// The list as it stands now.
    pub fn authorized(&self) -> Authorized {
        Authorized::parse(&self.authorized)
    }

    /// What the source ended up holding, if it could open what it was sent.
    pub fn gave_the_source(&self) -> Option<Vec<u8>> {
        self.gave.clone()
    }

    pub fn showed_before_waiting(&self) -> bool {
        self.showed_before_waiting
    }

    /// Whether the source's key had arrived before anything was written down.
    pub fn took_the_key_before_authorizing(&self) -> bool {
        self.took_the_key_before_authorizing
    }

    /// The source currently being served, if one is.
    fn serving(&self) -> Option<&Source> {
        self.sources.front()
    }
}

impl Entropy for SimPairing {
    fn fill(&mut self, into: &mut [u8]) -> bool {
        self.entropy.fill(into)
    }
}

impl PairingHost for SimPairing {
    fn show(&mut self, code: Code) {
        self.shown = Some(code);
    }

    fn wait_for_a_source(&mut self) -> bool {
        self.showed_before_waiting = self.shown.is_some();
        if self.sources.is_empty() {
            return false;
        }
        self.attempts += 1;
        true
    }

    fn take_offer(&mut self) -> Option<[u8; OFFER]> {
        let knows_the_code = self.serving()?.knows_the_code;
        let shown = self.shown?;
        // The digits the person at the source typed are the whole of what that end
        // can have wrong: a source given others produces an offer as well formed as
        // this one, and the difference only shows where a key will not open.
        let code = match knows_the_code {
            true => shown,
            false => other_digits(shown),
        };
        let (started, offer) = pairing::offer(code, self)?;
        self.started = Some(started);
        Some(offer)
    }

    fn send_answer(&mut self, answer: &[u8; OFFER]) -> bool {
        self.agreed = self
            .started
            .take()
            .and_then(|started| started.finish(answer));
        // The send itself is what this reports. A source that could not finish the
        // exchange is one that then sends nothing, which is not the same failure.
        true
    }

    fn take_sealed_key(&mut self) -> Option<[u8; SEALED_KEY]> {
        let key = self.serving()?.key.clone();
        let sealed = self.agreed.as_ref()?.seal(&key, Side::Source)?;
        self.took_the_key = true;
        Some(sealed)
    }

    fn send_sealed_key(&mut self, sealed: &[u8; SEALED_KEY]) -> bool {
        // Opened rather than recorded as sent: what a source holds at the end is what
        // it could read, and a key sealed under a secret it did not agree on is
        // nothing to it.
        self.gave = self
            .agreed
            .as_ref()
            .and_then(|agreed| agreed.open(sealed, Side::Sink));
        true
    }

    fn authorize(&mut self, key: &[u8]) -> bool {
        self.took_the_key_before_authorizing = self.took_the_key;
        if !self.keeps {
            return false;
        }
        self.authorized = Authorized::added(&self.authorized, key);
        true
    }
}

/// What a watchdog told the machine to do, in order.
///
/// Recorded because the order is the behaviour: asking a process to stop before
/// insisting is what lets it put the keyboards back itself, and taking the trace
/// before the process is gone would take it while it is still being written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Told {
    AskedItToStop,
    Paused(Duration),
    EndedIt,
    KeptTheTrace,
}

/// A machine standing in for the one a watchdog supervises (ADR-0008).
///
/// It owns the clock, which is what lets a silence of any length be produced without
/// waiting through it: a wait consumes exactly the patience it was given, and a
/// scripted arrival inside that window cuts it short. So a bound of two seconds is
/// reached in the time it takes to call a method.
///
/// The process it stands in for is scripted by how it answers probes rather than by a
/// timeline, because that is the relationship ADR-0008 describes: a probe went in and
/// a heartbeat came out, or it did not.
#[derive(Debug, Clone)]
pub struct SimWatchdog {
    now: Instant,
    /// When the process started, and `None` for a machine that cannot start one.
    starts: Option<Instant>,
    /// How long a heartbeat takes to come back after the probe that asked for it.
    round_trip: Duration,
    /// How many more probes are answered. `None` for a process that answers every
    /// one of them.
    answers: Option<usize>,
    /// The exit of a process that stops on its own: after this many probes, with this
    /// ending.
    exits: Option<(usize, Exit)>,
    /// The exit of a process whose scripted heartbeats have run out, which is what a
    /// run that finished looks like from out here.
    ends_when_quiet: Option<Exit>,
    /// Whether there is a way to ask the process to stop.
    asking_works: bool,
    /// What was asked of the process, in order, so a test can read the order rather
    /// than infer it.
    did: Vec<Told>,
    /// Whether a probe can be sent at all.
    probes_arrive: bool,
    /// Heartbeats owed, each at the moment it is due.
    owed: VecDeque<Instant>,
    probes: Vec<Instant>,
    warnings: Vec<String>,
    killed: Option<Instant>,
    kept_the_trace: bool,
    /// Whether the process had been ended by the time the trace was kept, which is
    /// the order ADR-0009 turns on.
    killed_before_keeping: bool,
}

impl Default for SimWatchdog {
    /// A machine that starts the process and answers every probe — the arrangement
    /// with nothing wrong, so a test names only the thing it is about.
    fn default() -> Self {
        Self {
            now: Instant::ZERO,
            starts: Some(Instant::ZERO),
            round_trip: Duration::from_millis(1),
            answers: None,
            exits: None,
            ends_when_quiet: None,
            asking_works: true,
            did: Vec::new(),
            probes_arrive: true,
            owed: VecDeque::new(),
            probes: Vec::new(),
            warnings: Vec::new(),
            killed: None,
            kept_the_trace: false,
            killed_before_keeping: false,
        }
    }
}

impl SimWatchdog {
    pub fn new() -> Self {
        Self::default()
    }

    /// A machine standing in for a run that beat at these moments and then finished.
    ///
    /// What the moments are for is a run that actually happened — a role's own
    /// heartbeats, taken off the machine it ran against and handed to a watchdog. That
    /// is what checks the two halves of ADR-0008's promise against each other rather
    /// than each against an idea of the other: the role answers every event, and the
    /// watchdog is satisfied by exactly those answers.
    pub fn supervising(beats: &[Instant], exit: Exit) -> Self {
        Self {
            owed: beats.iter().copied().collect(),
            ends_when_quiet: Some(exit),
            ..Self::default()
        }
    }

    /// A machine where the process cannot be started at all.
    pub fn that_cannot_start(mut self) -> Self {
        self.starts = None;
        self
    }

    /// A process that answers this many probes and then stops answering, which is
    /// what a wedge looks like from out here.
    pub fn that_answers(mut self, probes: usize) -> Self {
        self.answers = Some(probes);
        self
    }

    /// A process that never answers, which is a wedge that was there from the start.
    pub fn that_never_answers(self) -> Self {
        self.that_answers(0)
    }

    /// A process that ends on its own after this many probes.
    pub fn that_exits_after(mut self, probes: usize, exit: Exit) -> Self {
        self.exits = Some((probes, exit));
        self
    }

    /// A machine with no way to ask a process to stop, which is what a platform with
    /// no signal to send looks like.
    pub fn where_asking_does_not_work(mut self) -> Self {
        self.asking_works = false;
        self
    }

    /// What the watchdog told this machine to do, in order.
    pub fn told(&self) -> &[Told] {
        &self.did
    }

    /// A machine where a probe cannot be put in — the link to the process is broken
    /// rather than the process being slow.
    pub fn where_probes_cannot_be_sent(mut self) -> Self {
        self.probes_arrive = false;
        self
    }

    /// When each probe went out.
    pub fn probes(&self) -> &[Instant] {
        &self.probes
    }

    /// What the supervision said about itself, in order.
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    /// When the process was ended, if it was.
    pub fn killed(&self) -> Option<Instant> {
        self.killed
    }

    pub fn kept_the_trace(&self) -> bool {
        self.kept_the_trace
    }

    /// Whether the process was ended before the trace was kept.
    pub fn killed_before_keeping(&self) -> bool {
        self.killed_before_keeping
    }

    /// The moment the process started, for measuring the rest against.
    pub fn started(&self) -> Instant {
        self.starts.expect("a machine that starts the process")
    }
}

impl WatchdogHost for SimWatchdog {
    fn start(&mut self) -> Option<Instant> {
        self.starts
    }

    /// Ended once the scripted number of probes has gone out.
    ///
    /// Counted in probes rather than in time, because what a test says is "it answered
    /// this many and then stopped" — and a process that has ended answers nothing,
    /// which is the same shape from out here as a wedge until this is asked.
    fn ended(&mut self) -> Option<Exit> {
        if let Some((after, exit)) = self.exits {
            if self.probes.len() >= after {
                return Some(exit);
            }
        }
        // Every beat a finished run made has been read, so there is nothing left for
        // it to answer with and nothing wrong with that.
        self.ends_when_quiet.filter(|_| self.owed.is_empty())
    }

    fn wait_for_a_heartbeat(&mut self, patience: Duration) -> Beat {
        let deadline = self.now.saturating_add(patience);

        if let Some(due) = self.owed.pop_front_if(|due| *due <= deadline) {
            self.now = due.max(self.now);
            return Beat {
                at: self.now,
                kind: BeatKind::Heartbeat,
            };
        }

        // The wait consumed all of it, which is what a silence is.
        self.now = deadline;
        Beat {
            at: self.now,
            kind: BeatKind::Silence,
        }
    }

    fn probe(&mut self) -> bool {
        self.probes.push(self.now);
        if !self.probes_arrive {
            return false;
        }
        // Answered unless the script has run out of answers, and the answer is owed
        // from the moment the probe went rather than from now.
        let answering = self.answers.is_none_or(|left| self.probes.len() <= left);
        if answering {
            self.owed
                .push_back(self.now.saturating_add(self.round_trip));
        }
        true
    }

    fn ask_it_to_stop(&mut self) -> bool {
        self.did.push(Told::AskedItToStop);
        self.asking_works
    }

    /// The wait, which is what this machine owns: the moment advances and nothing
    /// takes any real time.
    fn pause(&mut self, how_long: Duration) {
        self.did.push(Told::Paused(how_long));
        self.now = self.now.saturating_add(how_long);
    }

    fn end_it(&mut self) {
        self.did.push(Told::EndedIt);
        self.killed = Some(self.now);
    }

    fn keep_the_trace(&mut self) {
        self.did.push(Told::KeptTheTrace);
        self.kept_the_trace = true;
        self.killed_before_keeping = self.killed.is_some();
    }

    fn warn(&mut self, message: core::fmt::Arguments) {
        self.warnings.push(message.to_string());
    }
}
/// A machine standing in for the one a code is read off, from this end.
///
/// The mirror of [`SimPairing`], and the exchange really runs here too: the sink is
/// scripted with the code it is showing and the key it will present, and it answers
/// with the same function its own run would call. So digits that are not the ones it
/// is showing end in nothing opening, decided by the arithmetic rather than by
/// comparing two codes (ADR-0007).
pub struct SimSourcePairing {
    entropy: Trickle,
    /// The sink to be reached, if there is one.
    sink: Option<Sink>,
    /// Whether the connection goes after the answer has been sent.
    stops_after_the_answer: bool,
    /// The sink's half of the exchange, once an offer has reached it.
    answered: Option<[u8; OFFER]>,
    /// What the sink agreed on, which for a wrong code is not what this end has.
    agreed: Option<Secret>,
    /// What was sent to the sink, as the sink could read it.
    gave: Option<Vec<u8>>,
    pinned: Option<Vec<u8>>,
    keeps: bool,
    offered_before_waiting: bool,
    took_the_key_before_pinning: bool,
    took_the_key: bool,
}

/// The sink waiting to be reached, as the script describes it.
#[derive(Debug, Clone)]
struct Sink {
    code: Code,
    key: Vec<u8>,
}

impl Default for SimSourcePairing {
    /// A machine with nothing to connect to, which is what a Mac showing no code
    /// looks like from here.
    fn default() -> Self {
        Self {
            entropy: Trickle::new(0x5011_2ce0),
            sink: None,
            stops_after_the_answer: false,
            answered: None,
            agreed: None,
            gave: None,
            pinned: None,
            keeps: true,
            offered_before_waiting: false,
            took_the_key_before_pinning: false,
            took_the_key: false,
        }
    }
}

impl SimSourcePairing {
    pub fn new() -> Self {
        Self::default()
    }

    /// A sink that is showing this code and will present this key.
    pub fn with_a_sink_that_knows(mut self, code: Code, key: &[u8]) -> Self {
        self.sink = Some(Sink {
            code,
            key: key.to_vec(),
        });
        self
    }

    /// A connection that goes after the answer, part way through the exchange.
    pub fn that_stops_after_the_answer(mut self) -> Self {
        self.stops_after_the_answer = true;
        self
    }

    /// A machine that cannot write the pinned key down.
    pub fn that_cannot_keep_the_sink(mut self) -> Self {
        self.keeps = false;
        self
    }

    /// The key this end wrote down as the sink, if it wrote one.
    pub fn pinned(&self) -> Option<Vec<u8>> {
        self.pinned.clone()
    }

    /// What the sink ended up holding, if it could open what it was sent.
    pub fn gave_the_sink(&self) -> Option<Vec<u8>> {
        self.gave.clone()
    }

    pub fn offered_before_waiting(&self) -> bool {
        self.offered_before_waiting
    }

    /// Whether the sink's key had arrived before anything was written down.
    pub fn took_the_key_before_pinning(&self) -> bool {
        self.took_the_key_before_pinning
    }
}

impl Entropy for SimSourcePairing {
    fn fill(&mut self, into: &mut [u8]) -> bool {
        self.entropy.fill(into)
    }
}

impl SourcePairingHost for SimSourcePairing {
    fn connect(&mut self) -> bool {
        self.sink.is_some()
    }

    fn send_offer(&mut self, offer: &[u8; OFFER]) -> bool {
        let Some(code) = self.sink.as_ref().map(|sink| sink.code) else {
            return false;
        };
        // The sink's half, under the digits it is showing rather than the ones this
        // end offered: that is where a wrong code stops being indistinguishable from
        // a right one.
        let Some((answer, agreed)) = pairing::answer(code, offer, self) else {
            return false;
        };
        self.answered = Some(answer);
        self.agreed = Some(agreed);
        true
    }

    fn take_answer(&mut self) -> Option<[u8; OFFER]> {
        self.offered_before_waiting = self.answered.is_some();
        self.answered
    }

    fn send_sealed_key(&mut self, sealed: &[u8; SEALED_KEY]) -> bool {
        // Opened rather than recorded as sent, for the reason it is at the other end:
        // what the sink holds is what it could read.
        self.gave = self
            .agreed
            .as_ref()
            .and_then(|agreed| agreed.open(sealed, Side::Source));
        true
    }

    fn take_sealed_key(&mut self) -> Option<[u8; SEALED_KEY]> {
        // The connection going here is not a wrong code, and the run has to be able
        // to tell them apart.
        if self.stops_after_the_answer {
            return None;
        }
        let key = self.sink.as_ref()?.key.clone();
        let sealed = self.agreed.as_ref()?.seal(&key, Side::Sink)?;
        self.took_the_key = true;
        Some(sealed)
    }

    fn pin_sink(&mut self, key: &[u8]) -> bool {
        self.took_the_key_before_pinning = self.took_the_key;
        if !self.keeps {
            return false;
        }
        self.pinned = Some(key.to_vec());
        true
    }
}
