//! The simulated machine the end-to-end suite runs against (ADR-0007).
//!
//! Not gated on `target_os`: it builds anywhere, which is what lets one process
//! stand in for a machine.
//!
//! Everything a real host would read off a platform is a value set from outside
//! here, a script rather than a device, a file or a socket. Both roles speak the
//! same raw vocabulary a real host's capture would (ADR-0006): a key is a HID
//! element value on the sink's side and a scancode on the source's, and naming
//! either one is `engine`'s to do, reached the same way it would be from real
//! hardware.
//!
//! One role runs at a time, on the suite's own thread, because there is nothing for
//! a scheduler to explore: the link delivers in order and neither role observes the
//! other except through the messages that cross (ADR-0007). A loop a run hands over
//! to be turned alongside its own is turned here to a standstill and then returned
//! from, so the same thread carries both: what its events cross into the converter's
//! stream through is the queue two loops share on a real machine, and their
//! timestamps are what put them in order there.

use core::time::Duration;
use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use favjit_discovery::{self as discovery, Found};
use favjit_hid::{page, usage, Buttons, Key, PointerReport};
use favjit_host::link::{Accepted, Incoming, LinkHost};
use favjit_host::pairing::{PairingHost, SourcePairingHost};
use favjit_host::sink::{SinkHost, SinkInputHost};
use favjit_host::source::{Answering, Answers, Arrived, Driving, Opening, SourceHost, Suppressing};
use favjit_host::watchdog::{Beat, BeatKind, Exit, WatchdogHost};
use favjit_host::{
    Datagram, DeviceId, DiscoveryHost, Entropy, EventKind, Host, HostEvent, IdentityStore, Instant,
    OutputReport, Trouble,
};
use favjit_link_wire::{Attached, Message};
use favjit_noise::{
    keypair, Identity, Initiator, Responder, Session, ANSWER, FRAME, HANDSHAKE, KEY, SEALED,
};
use favjit_pairing_exchange::{Code, Secret, Side, Started, OFFER, SEALED_KEY};

/// `at` plus `by`, on a clock [`Instant`] carries no arithmetic of its own for:
/// the type is plain data by [`favjit_host`]'s own rule, and every machine this
/// crate scripts needs the same one sum. `pub` beyond that: a test computing the
/// instant a scripted wait should land on needs the same sum, and reimplementing
/// it at each call site would be a second copy to keep in step with this one.
pub fn advance(at: Instant, by: Duration) -> Instant {
    Instant {
        nanos: at.nanos.saturating_add(by.as_nanos() as u64),
    }
}

/// The virtual key this machine reports beside a scripted position.
///
/// Two values change how a position reads, and this answers with those and
/// nothing else: right shift's, whose extended bit is a lie, and something that
/// is neither it nor the filler for every other key. Windows' own numbering for
/// the rest is not a table this project has established, and one written to fill
/// a script in would be a record of nothing — what a run does with a position
/// depends on these two values alone, so a script that carries them carries
/// everything the reading looks at
/// (`docs/platform/windows/hooks-and-raw-input.md`).
fn virtual_key(key: Key) -> u16 {
    match key {
        Key::RightShift => favjit_hid::scancode::VK_RSHIFT,
        // Not the filler, which is the one other value a reading singles out.
        _ => favjit_hid::scancode::VK_FILLER - 1,
    }
}

/// The vendor id this machine's own output device enumerates with.
///
/// A number of this crate's own, unlike the one a real machine's virtual device
/// carries: what it is for is being told apart from the other pointers a script
/// puts on the machine, and a run that read it from somewhere else would be
/// matching on a value nothing here can vary.
pub const OUTPUT_VENDOR: i64 = 0x0666;

/// One pointing device on a scripted machine, as its properties.
///
/// Properties and not a resolution and a factor: which of them a run reads, and
/// which it writes, is the run's own to decide, so a machine that held only the
/// two it happens to read now could not show a run reaching for a third.
#[derive(Debug, Clone, Default)]
pub struct SimPointer {
    fixed: BTreeMap<String, f64>,
    integer: BTreeMap<String, i64>,
    text: BTreeMap<String, String>,
    /// Every fixed-point write asked for, in order, whether or not it took.
    written: Vec<(String, f64)>,
    writable: bool,
}

impl SimPointer {
    /// A pointing device with this vendor id and nothing else on it.
    pub fn new(vendor: i64) -> Self {
        let mut pointer = Self {
            writable: true,
            ..Self::default()
        };
        pointer.integer.insert(String::from("VendorID"), vendor);
        pointer
    }

    /// Give it a 16.16 property, the way a real one carries a resolution.
    pub fn with_fixed(mut self, key: &str, value: f64) -> Self {
        self.fixed.insert(String::from(key), value);
        self
    }

    /// Give it a string property, the way one names which key its factor is in.
    pub fn with_text(mut self, key: &str, value: &str) -> Self {
        self.text.insert(String::from(key), String::from(value));
        self
    }

    /// A device that takes no writes, which is what a service favjit is not
    /// privileged for looks like.
    pub fn that_refuses_writes(mut self) -> Self {
        self.writable = false;
        self
    }

    /// What was written to it, in order.
    pub fn written(&self) -> &[(String, f64)] {
        &self.written
    }

    /// What one of its 16.16 properties says now.
    pub fn fixed(&self, key: &str) -> Option<f64> {
        self.fixed.get(key).copied()
    }
}

/// The USB HID usage page and usage a keyboard declares as its primary one.
///
/// The specification's own numbers, which is what a real keyboard reports:
/// scripting anything else would be a machine presenting a device a run has no
/// reason to read.
const KEYBOARD_PAGE: i64 = 0x01;
const KEYBOARD_USAGE: i64 = 0x06;

/// One keyboard as IOKit describes it, for the machines that speak HID.
///
/// The properties and not what a run reads off them: what a keyboard is to a run
/// is `engine`'s to work out, and a machine that answered it would be answering
/// the question the suite is here to drive (ADR-0006).
fn hid_keyboard(
    device: DeviceId,
    transport: &str,
    product: &str,
    vendor_id: Option<i64>,
    product_id: Option<i64>,
) -> EventKind {
    EventKind::HidDeviceFound {
        device,
        primary_usage_page: Some(KEYBOARD_PAGE),
        primary_usage: Some(KEYBOARD_USAGE),
        transport: Some(String::from(transport)),
        product: Some(String::from(product)),
        vendor_id,
        product_id,
    }
}

/// What the program asked this machine to do, in order.
///
/// Recorded because the order is behaviour: input must not be taken before there is
/// somewhere to send it (ADR-0008), and the only way to see that is to see what was
/// asked for when. A repeat of the same kind in a row is folded into one entry —
/// `switched_on` is asked once per idle poll while the switch stays off, and a list
/// of *that many* identical entries would say nothing a single one does not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Did {
    AskedIfSwitchedOn,
    AskedPermission,
    RequestedPermission,
    OpenedOutput,
    OpenedTheEventSystem,
    OpenedTheSimpleEventSystem,
    TunedOutput,
    BoundLink,
    StartedTheLink,
    LookedForDevices,
    TookDevice { device: DeviceId, exclusive: bool },
    ReadDevice { device: DeviceId },
    AskedForHeldDevice,
    ReleasedDevice { device: DeviceId },
    Warned,
}

/// What a control-file run asked of its host, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlCall {
    MadeDirectory,
    WroteDisabled,
}

/// What a run asked of the output device's connection, in order.
///
/// Recorded because the order is the thing under test: readiness arrives as
/// the answer to a request, so a run that served the connection before asking
/// for its devices would wait out its whole bound for an answer nobody was
/// reading when it came.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputCall {
    BoundWrites,
    BoundReads,
    AskedForTheKeyboard,
    AskedForThePointer,
    Beat,
    AnsweredHealthCheck,
    Acknowledged,
}

/// The service's own numbering, restated here rather than borrowed.
///
/// A script that took these from the code under it would assert that code
/// against itself: what makes a frame mean something is that the service and
/// the run agree on the number, and this end stands in for the service.
const HEARTBEAT: u8 = 0;
const HEALTH_CHECK: u8 = 2;
const REQUEST: u8 = 4;
const RESPONSE: u8 = 5;

/// How often the service beats at whoever is connected, and how long it will go
/// on hearing nothing back before it closes the connection.
///
/// Both are the driver's own numbers rather than favjit's
/// (`docs/platform/macos/virtual-hid-device.md`), and having them here is what
/// makes this end a service rather than a script: a simulator that only replayed
/// frames would let a run that never says anything look healthy, and being
/// closed for silence is the failure that reaches a person as their keyboard
/// stopping.
const SERVICE_BEATS_EVERY: Duration = Duration::from_secs(3);
const SERVICE_HEARS_NOTHING_FOR: Duration = Duration::from_secs(15);

/// Why the service let a connection go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceEnded {
    /// It heard nothing from the run for as long as it allows.
    HeardNothing,
    /// It was scripted to serve for a while, and that while is over.
    ///
    /// The ordinary ending, and the one an assertion about staying alive is
    /// written against: a run that held the connection up for its whole life
    /// ends this way and no other.
    ServedItsTime,
}
const DRIVER_ACTIVATED: u8 = 1;
const DRIVER_CONNECTED: u8 = 2;
const VERSION_MISMATCHED: u8 = 3;
const KEYBOARD_READY: u8 = 4;
const POINTING_READY: u8 = 5;

/// One frame's body, as the service sends it: a kind, the id it is paired by,
/// and the flag pairs behind that.
fn status_frame(pairs: &[(u8, bool)]) -> Vec<u8> {
    let mut out = vec![RESPONSE];
    out.extend_from_slice(&1u64.to_be_bytes());
    for (kind, value) in pairs {
        out.push(*kind);
        out.push(u8::from(*value));
    }
    out
}

/// The frame a service that has both devices up sends.
fn both_devices_up() -> Vec<u8> {
    status_frame(&[
        (DRIVER_ACTIVATED, true),
        (DRIVER_CONNECTED, true),
        (KEYBOARD_READY, true),
        (POINTING_READY, true),
    ])
}

/// One health check, which the service asks of whoever is connected.
pub fn a_health_check() -> Vec<u8> {
    vec![HEALTH_CHECK]
}

/// One heartbeat, which the service sends of its own accord.
pub fn a_heartbeat() -> Vec<u8> {
    vec![HEARTBEAT]
}

/// How many bytes the service spells a frame's own length in, big-endian
/// (`docs/platform/macos/virtual-hid-device.md`).
const LENGTH: usize = 4;

/// One frame as the service writes it: its body's length, then the body.
///
/// Spelled here rather than taken from the run, which is what makes a test of
/// the framing mean anything: a service that framed the way the reader expected
/// would agree with the reader by construction.
fn framed(body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(LENGTH + body.len());
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    out.extend_from_slice(body);
    out
}

/// One request, which expects an answer carrying the id it came with.
pub fn a_request() -> Vec<u8> {
    let mut out = vec![REQUEST];
    out.extend_from_slice(&9u64.to_be_bytes());
    out.extend_from_slice(&[KEYBOARD_READY, 1]);
    out
}

/// A machine standing in for the service the output device belongs to.
///
/// Scripted the way the others are: frames go in, and what the run did with
/// them comes out. The frames are the service's own, so what a script says is
/// what a real service would send rather than a flag standing in for it
/// (ADR-0006).
pub struct SimOutput {
    /// What it was scripted to say, in order, one frame's body each.
    frames: VecDeque<Vec<u8>>,
    /// The bytes it has written and the run has not read yet.
    ///
    /// A stream and not a queue of whole frames, because that is what the run
    /// reads: it asks for a length and then for that many more bytes, so a
    /// service handing over one frame per read would be a service no framing
    /// was ever read off.
    on_the_wire: VecDeque<u8>,
    /// What it was asked, shared because the loop that asks is turned
    /// somewhere this object has been given away to.
    calls: Arc<Mutex<Vec<OutputCall>>>,
    /// What the service answers a beat with, where it will not take one.
    refuses_beats: Option<i32>,
    tears_a_frame: bool,
    /// Whether the frame it tore is the one the run is still waiting on the
    /// body of.
    ///
    /// Held so that the read after the tear comes back with nothing rather than
    /// with whatever the service would have said next: a beat arriving there
    /// would be read as the torn frame's body, which is the one thing a tear is
    /// not.
    tore_one: bool,
    /// The stream the converting loop reads, which is where anything this end
    /// has to say about the connection goes — the same path
    /// [`favjit_host::link::LinkHost::deliver`] takes, because it is the same
    /// arrangement: one loop turning alongside another.
    delivering: Arc<Mutex<Observed>>,
    /// This end's clock, which only a read moves.
    ///
    /// Its own rather than the run's, because the loop serving this connection
    /// is a loop of its own: a clock shared with the converting loop would have
    /// the two advancing each other's cadences.
    now: Instant,
    /// The bound the run gave the next read.
    bound: Duration,
    /// When it last beat, and when it last heard anything at all from the run.
    beat_at: Instant,
    heard_at: Instant,
    /// When it lets go of its own accord.
    closes_at: Instant,
    ended: Arc<Mutex<Option<ServiceEnded>>>,
}

impl SimOutput {
    /// Put these bytes on the wire and hand over what this read asks for.
    fn write(&mut self, bytes: &[u8], into: &mut [u8]) -> favjit_host::output::Took {
        self.on_the_wire.extend(bytes);
        self.hand_over(into)
            .unwrap_or(favjit_host::output::Took::Nothing)
    }

    /// What this read takes off the wire, or nothing where there is nothing on
    /// it.
    ///
    /// Up to what was asked for and not all of it: a stream socket is free to
    /// answer with less, and a service that always answered with a whole frame
    /// would be one no run ever had to read twice for.
    fn hand_over(&mut self, into: &mut [u8]) -> Option<favjit_host::output::Took> {
        let taken = self.on_the_wire.len().min(into.len());
        (taken > 0).then(|| {
            for (byte, place) in self.on_the_wire.drain(..taken).zip(into.iter_mut()) {
                *place = byte;
            }
            favjit_host::output::Took::Bytes(taken)
        })
    }

    fn note(&self, call: OutputCall) {
        self.calls.lock().unwrap().push(call);
    }

    /// Note that the run said something, whatever it was.
    ///
    /// Every frame refreshes the deadline and not only a heartbeat, because
    /// that is the service's own rule: an answer to a health check is a frame
    /// it heard, so a run answering questions steadily is a run it keeps.
    fn heard(&mut self, call: OutputCall) {
        self.note(call);
        self.heard_at = self.now;
    }

    /// Why the service would have let go by `at`, if it would have.
    ///
    /// Silence first, because a connection it has already given up on is not
    /// one it would have served to the end of its time.
    fn let_go_by(&self, at: Instant) -> Option<ServiceEnded> {
        if at >= advance(self.heard_at, SERVICE_HEARS_NOTHING_FOR) {
            return Some(ServiceEnded::HeardNothing);
        }
        if at >= self.closes_at {
            return Some(ServiceEnded::ServedItsTime);
        }
        None
    }
}

impl favjit_host::output::OutputHost for SimOutput {
    fn now(&mut self) -> Instant {
        self.now
    }

    fn set_read_timeout(&mut self, timeout: Duration) -> bool {
        self.note(OutputCall::BoundReads);
        self.bound = timeout;
        true
    }

    /// Hand over what is on the wire, or wait out the bound this read was given
    /// and put there whatever the service would have said inside it.
    ///
    /// What arrives is worked out rather than scripted, because the cadence is
    /// the thing: the service beats on its own clock, so a script naming every
    /// frame would have a test choosing whether the run's read wins that race
    /// — and a run only stays connected if it holds the connection up whichever
    /// way the race goes.
    ///
    /// Where the two land at the same instant, the frame is what arrives. It is
    /// the harder of the two for a run and the one that has to work: a run
    /// reached by a read that came back empty knows a cadence came round, and a
    /// run reached by a frame has to know it from something else.
    fn read_some(&mut self, into: &mut [u8]) -> favjit_host::output::Took {
        // What is already on the wire first, and at no cost in time: those
        // bytes are there, so a read of them that waited would be a service
        // charging for what it has already written.
        if let Some(took) = self.hand_over(into) {
            return took;
        }

        // Answers next, and at no cost in time either: they are replies to
        // requests this end has already been asked, so a service that made a
        // run wait its whole bound for one would be a service answering
        // nothing.
        if let Some(body) = self.frames.pop_front() {
            return self.write(&framed(&body), into);
        }

        // Once, and after the script rather than instead of it: a run has to
        // reach the loop that serves the connection before there is anything for
        // a torn frame to happen to.
        if core::mem::take(&mut self.tears_a_frame) {
            self.tore_one = true;
            let frame = framed(&a_heartbeat());
            return self.write(&frame[..LENGTH], into);
        }

        // The wait the torn frame's body would have arrived in, come round with
        // nothing: what the service says after a tear is bytes the run cannot
        // place, and handing them over as that body is the one reading a tear
        // is not.
        if core::mem::take(&mut self.tore_one) {
            return favjit_host::output::Took::Nothing;
        }

        let beats_at = advance(self.beat_at, SERVICE_BEATS_EVERY);
        let read_ends_at = advance(self.now, self.bound);
        let beat_arrives = beats_at <= read_ends_at;
        let at = match beat_arrives {
            true => beats_at,
            false => read_ends_at,
        };

        if let Some(why) = self.let_go_by(at) {
            *self.ended.lock().unwrap() = Some(why);
            return favjit_host::output::Took::Gone;
        }

        self.now = at;
        match beat_arrives {
            true => {
                self.beat_at = at;
                let beat = framed(&a_heartbeat());
                self.write(&beat, into)
            }
            false => favjit_host::output::Took::Nothing,
        }
    }

    fn ask_for_the_keyboard(&mut self) -> bool {
        self.note(OutputCall::AskedForTheKeyboard);
        true
    }

    fn ask_for_the_pointer(&mut self) -> bool {
        self.note(OutputCall::AskedForThePointer);
        true
    }

    fn beat(&mut self) -> i32 {
        // A refused write is not heard, so the deadline is not refreshed: a
        // service that counted a frame it never took would keep a connection
        // alive on the strength of it.
        match self.refuses_beats {
            Some(code) => {
                self.note(OutputCall::Beat);
                code
            }
            None => {
                self.heard(OutputCall::Beat);
                0
            }
        }
    }

    fn answer_health_check(&mut self) -> i32 {
        self.heard(OutputCall::AnsweredHealthCheck);
        0
    }

    fn acknowledge(&mut self, _id: &[u8]) -> i32 {
        self.heard(OutputCall::Acknowledged);
        0
    }

    /// Onto the stream the converter reads, and not into what
    /// [`SimHost::delivered`] answers.
    ///
    /// That is the link's own list, and a test asking it asks about the link: an
    /// event from this loop added there would fail every assertion naming what
    /// arrived over the link exactly, for a change to a cadence that has nothing
    /// to do with one. What this loop said is in the recording, which is where a
    /// reading of the connection belongs anyway (ADR-0009).
    fn deliver(&mut self, event: EventKind) -> bool {
        let at = self.now;
        self.delivering
            .lock()
            .unwrap()
            .stream
            .push_back(favjit_host::HostEvent { at, kind: event });
        true
    }
}

/// A platform for the loop that reads this machine's keyboards to be turned
/// against.
///
/// Bounded rather than turning until the run is over, because the run has not
/// started: the loop is turned to a standstill before the converting loop reads
/// the stream, so a platform that never said it was over would be a script that
/// never finished (ADR-0007).
struct SimCapture {
    turns: usize,
    answered: Arc<Mutex<usize>>,
    /// Whether the loop asked to be able to refuse this machine's input.
    ///
    /// Read back once the loop has been turned to a standstill rather than
    /// shared: the loop runs to its end before the call that started it returns,
    /// so there is nothing to share it with.
    hooked: bool,
}

impl favjit_host::capture::Reading for SimCapture {
    fn turn_the_loop(&mut self, _poll: Duration) {
        self.turns = self.turns.saturating_sub(1);
    }

    fn take_probes(&mut self) -> usize {
        0
    }

    fn deliver(&mut self, _event: EventKind) -> bool {
        true
    }

    fn over(&mut self) -> bool {
        self.turns == 0
    }
}

/// Both roles against the one script, because a script is not a platform: what
/// it says about a keyboard is on the stream under the number it gave, so every
/// call either loop would make about finding, opening or reading one is a call
/// there is nothing here to make.
impl favjit_host::capture::SourceCapture for SimCapture {
    fn register_the_window_class(&mut self) {}

    fn make_a_window(&mut self) -> bool {
        true
    }

    fn ask_for_the_mice(&mut self) -> bool {
        true
    }

    fn send_the_keys_to_that_window(&mut self) {}

    fn state_the_chord(&mut self) {}

    fn start_the_probe_timer(&mut self) {}

    fn hook_the_keyboard(&mut self) -> bool {
        self.hooked = true;
        true
    }

    fn hook_the_pointer(&mut self) -> bool {
        true
    }

    fn next_device(&mut self, _as_this: DeviceId) -> Option<favjit_host::capture::Found> {
        None
    }

    fn take_what_the_turn_produced(&mut self) {}

    fn let_go_of_the_turn(&mut self) {}

    fn next_arrival(&mut self) -> Option<EventKind> {
        None
    }
}

impl favjit_host::capture::SinkCapture for SimCapture {
    fn open_a_notification_port(&mut self) -> bool {
        true
    }

    fn ask_to_be_told_about_devices(&mut self) -> bool {
        true
    }

    fn put_that_port_on_this_loop(&mut self) {}

    fn look_at_the_devices_here(&mut self) -> bool {
        true
    }

    /// Nothing is found through this loop at all: what a script says about a
    /// keyboard is put on the stream where the script said it, under the number
    /// the script gave it — so none of the calls that make one readable is ever
    /// reached.
    fn next_device(&mut self) -> Option<favjit_host::Handed> {
        None
    }

    fn name_of(&mut self, _found: favjit_host::Handed) -> Option<u64> {
        None
    }

    fn open_it(&mut self, _found: favjit_host::Handed) -> Option<favjit_host::Handed> {
        None
    }

    fn let_go_of_the_finding(&mut self, _found: favjit_host::Handed) {}

    fn watch_for_its_removal(&mut self, _device: favjit_host::Handed) {}

    fn put_it_where_reports_arrive(&mut self, _device: favjit_host::Handed) {}

    fn describe(&mut self, _device: favjit_host::Handed, as_this: DeviceId) -> EventKind {
        EventKind::DeviceLost(as_this)
    }

    fn keep(&mut self, _device: favjit_host::Handed, _as_this: DeviceId) {}

    fn let_go_of_the_device(&mut self, _device: favjit_host::Handed) {}

    fn forget(&mut self, _device: DeviceId) {}

    fn next_ask(&mut self) -> Option<favjit_host::capture::Ask> {
        *self.answered.lock().unwrap() += 1;
        None
    }

    fn seize(&mut self, _device: favjit_host::Handed, _exclusive: bool) -> i32 {
        -1
    }

    fn keep_what_was_seized(&mut self, _device: favjit_host::Handed, _code: i32, _exclusive: bool) {
    }

    fn every_element_of(&mut self, _device: favjit_host::Handed) -> Option<favjit_host::Handed> {
        None
    }

    fn make_a_queue_over(&mut self, _device: favjit_host::Handed) -> Option<favjit_host::Handed> {
        None
    }

    fn how_many_elements_of(&mut self, _elements: favjit_host::Handed) -> usize {
        0
    }

    fn has_an_element_at(&mut self, _elements: favjit_host::Handed, _at: usize) -> bool {
        false
    }

    fn put_one_element_on(
        &mut self,
        _queue: favjit_host::Handed,
        _elements: favjit_host::Handed,
        _at: usize,
    ) -> bool {
        false
    }

    fn let_go_of_the_elements(&mut self, _elements: favjit_host::Handed) {}

    fn watch_for_its_values(&mut self, _queue: favjit_host::Handed) {}

    fn put_it_where_values_arrive(&mut self, _queue: favjit_host::Handed) {}

    fn start_it_delivering(&mut self, _queue: favjit_host::Handed) {}

    fn keep_the_queue(&mut self, _queue: favjit_host::Handed, _device: favjit_host::Handed) {}

    fn let_go_of_the_queue(&mut self, _queue: favjit_host::Handed) {}

    fn answer_the_ask(&mut self, _code: i32) {}

    /// Nothing is put down for the run to drain: what a script says arrives on
    /// the stream whole.
    fn next_thing_with_values(&mut self) -> Option<favjit_host::Handed> {
        None
    }

    fn next_value_of(&mut self, _from: favjit_host::Handed) -> Option<EventKind> {
        None
    }

    fn how_long_ago(&mut self, _stamp: u64) -> u64 {
        0
    }

    fn nothing_more_of(&mut self, _from: favjit_host::Handed) -> EventKind {
        EventKind::HidValuesDone {
            device: DeviceId(0),
        }
    }

    fn done_with(&mut self, _from: favjit_host::Handed) {}

    fn next_gone(&mut self) -> Option<favjit_host::Handed> {
        None
    }

    fn its_queue(&mut self, _device: favjit_host::Handed) -> Option<favjit_host::Handed> {
        None
    }

    fn stop_it_delivering(&mut self, _queue: favjit_host::Handed) {}

    fn give_it_back(&mut self, _device: favjit_host::Handed) -> i32 {
        -1
    }

    fn forget_it(&mut self, _device: favjit_host::Handed) -> Option<DeviceId> {
        None
    }
}

/// A control file whose operations can fail independently.
#[derive(Default)]
pub struct SimControl {
    calls: Vec<ControlCall>,
    fails_at: Option<ControlCall>,
}

impl SimControl {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn that_fails_at(mut self, call: ControlCall) -> Self {
        self.fails_at = Some(call);
        self
    }

    pub fn calls(&self) -> &[ControlCall] {
        &self.calls
    }
}

impl favjit_host::ControlStore for SimControl {
    fn make_directory(&mut self) -> std::io::Result<()> {
        self.calls.push(ControlCall::MadeDirectory);
        match self.fails_at == Some(ControlCall::MadeDirectory) {
            true => Err(std::io::Error::other("directory")),
            false => Ok(()),
        }
    }

    fn write_disabled(&mut self) -> std::io::Result<()> {
        self.calls.push(ControlCall::WroteDisabled);
        match self.fails_at == Some(ControlCall::WroteDisabled) {
            true => Err(std::io::Error::other("write")),
            false => Ok(()),
        }
    }
}

/// What a machine that was scripted to refuse this call says about refusing.
///
/// A sentence naming the call, because that is what a real machine's own error
/// would carry: a script that answered with nothing would let a run that lost
/// the reason pass (ADR-0006).
fn refused_at(fails_at: Option<IdentityCall>, call: IdentityCall) -> Result<(), Trouble> {
    match fails_at == Some(call) {
        true => Err(Trouble(format!("this machine refuses {call:?}"))),
        false => Ok(()),
    }
}

/// What an identity run asked of its file, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentityCall {
    Read,
    MadeDirectory,
    Opened,
    Wrote,
}

/// Why a scripted ending was reached, before the run itself said so.
///
/// The two host-visible facts a run's own `idle_ended` reads apart from the
/// stream: the switch and the output device. Kept here rather than mutated onto
/// those flags directly, so that "off" and "gone" are answered from one fact —
/// how many events have been handed over — the same way a real machine's own
/// state changes on its own clock rather than on a counter this double keeps
/// twice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum End {
    SwitchedOff,
    OutputGone,
}

/// Everything one of those machines holds.
struct Machine {
    /// Where the script is: the timestamp the next scripted event gets.
    cursor: Instant,
    /// The time of the event currently being handled, or the deadline a wait was
    /// given when nothing arrived by it — `engine` compares [`Host::now`] against
    /// a wake-up it computed itself, and a clock that never advanced past an
    /// empty wait would leave that wake-up perpetually still due.
    now: Instant,
    inbound: VecDeque<HostEvent>,
    /// The next report's stamp, so two calls to [`SimHost::pointer`] are never
    /// mistaken for values of the same report.
    next_stamp: u64,
    /// The buttons the last [`SimHost::pointer`] call for a device reported, so
    /// only the ones that changed are scripted — a real queue reports a
    /// transition, not the whole set, every time.
    buttons: Vec<(DeviceId, Buttons)>,
    did: Vec<Did>,
    switched_on: bool,
    /// What ends the stream, and after how many events.
    ends_after: Option<(usize, End)>,
    /// How many events have been handed over.
    handed_over: usize,
    permits_input: bool,
    output_comes_up: bool,
    /// Whether this machine's supervisor is still reading the beats.
    ///
    /// True by default, since a watchdog that has stopped reading is the
    /// exception a test asks for by name.
    beats_arrive: bool,
    /// Which of the machine's answers a run gets when nothing answers where
    /// the service listens.
    no_output: favjit_host::NoOutput,
    /// What the service says about itself once a run is connected to it, in
    /// order, one frame's body each.
    ///
    /// What it says *of its own accord* is not in here: its heartbeats come out
    /// of the cadence it keeps, so a script holds only the answers a test is
    /// about and cannot put the service out of step with itself.
    output_frames: VecDeque<Vec<u8>>,
    /// What the service answers a beat with, where it will not take one.
    output_refuses_beats: Option<i32>,
    /// Whether the next read after the script runs out comes back with part of
    /// a frame rather than a whole one.
    output_tears_a_frame: bool,
    /// How long the service stays before letting the connection go on its own.
    ///
    /// Zero is a service that closes as soon as its script runs out, which is
    /// what a test about bringing the output up wants: the loop serving it is
    /// turned to a standstill before a run carries on, so a service that stayed
    /// would be a test that never finished.
    output_serves_for: Duration,
    /// Why the service let go, once it has — shared, because by then the end of
    /// the connection that decided it has been given away.
    output_ended: Arc<Mutex<Option<ServiceEnded>>>,
    /// What the run asked of that connection, shared with the end of it the
    /// run was given.
    output_calls: Arc<Mutex<Vec<OutputCall>>>,
    looks_for_devices: bool,
    /// How many turns the loop that reads this machine's keyboards is given.
    capture_turns: usize,
    /// How many of those turns answered what the run asked of it — shared,
    /// because by then the platform it was turned against has been given away.
    capture_answered: Arc<Mutex<usize>>,
    /// Whether [`SinkInputHost::look_for_devices`] was ever asked, and what it
    /// answered.
    looked_for_devices: Option<bool>,
    /// Keyboards this machine answers for one at a time: the code taking one
    /// returns, and whether reading it starts.
    ///
    /// Per keyboard rather than one flag for the machine, because that is the
    /// shape of the thing: another process holds *this* keyboard, and what a run
    /// does about it is carry on with the others.
    refuses_to_take: Vec<(DeviceId, i32)>,
    refuses_to_read: Vec<DeviceId>,
    refuses_to_release: Vec<(DeviceId, i32)>,
    /// The keyboards this machine holds exclusively.
    held: Vec<DeviceId>,
    /// The keyboards this machine is reading, which is what it delivers input
    /// from.
    ///
    /// Kept because a machine only hands over what it opened: a keyboard the run
    /// left alone or could not have goes on typing into whatever else is
    /// listening, and a script whose keys arrived from one anyway would let a run
    /// convert from a device it never took and look right doing it.
    reading: Vec<DeviceId>,
    supervised: bool,
    /// The lines the run put into this machine's log, in order.
    warnings: Vec<String>,
    /// The identity file, with no disk under it: what it holds, whether the
    /// machine's entropy will produce a keypair, and whether a write succeeds are
    /// the whole of what the sequence in `engine` reads.
    ///
    /// Shared with the open file [`IdentityStore::open`] hands over, because the
    /// run holds that while it goes on asking this machine other things: the
    /// file the run writes through and the machine that records what reached it
    /// are two ends of one thing.
    written: Rc<RefCell<Written>>,
    identity_fails_at: Option<IdentityCall>,
    /// The private half this machine's entropy hands over when asked for one —
    /// the whole of what it can offer now that a keypair is derived from it for
    /// real rather than handed over whole ([`favjit_noise::keypair`]).
    makes: Option<Vec<u8>>,
    /// The identity the run bound the link with, which is the key a peer would have
    /// to have pinned.
    listened_with: Option<Identity>,
    /// The link this machine will hand over when a run binds one.
    ///
    /// Taken rather than borrowed, because the loop that serves it owns everything
    /// it touches — what a test asks about afterwards it asks this machine, through
    /// the handle below.
    link: Option<SimLink>,
    binds_link: bool,
    starts_alongside: bool,
    observed: Arc<Mutex<Observed>>,
    /// What the link has put into the stream and the loop has not read yet.
    from_the_link: VecDeque<HostEvent>,
    /// What every [`SinkHost::send_report`] this run made actually wrote, in the
    /// bytes it wrote them and when — the same clock [`SimHost::heartbeats`] is
    /// on, so a test can read the two against each other.
    /// The pointing devices this machine has, in the order it lists them,
    /// shared with the view a run holds for the reason [`SimPointers`] is.
    pointers: Rc<RefCell<Vec<SimPointer>>>,
    /// Which of the event system's two views this machine will open.
    ///
    /// Both by default, so a run that asks for the writable one gets it: what
    /// a script says about them is which one a run had to settle for.
    event_system_opens: bool,
    simple_event_system_opens: bool,
    /// What this machine was told the output pointer should feel like.
    pointer_feel: (Option<f64>, Option<f64>),
    reports: Vec<(Instant, OutputReport, Vec<u8>)>,
    /// What a write to the output device answers with, `0` for one that went.
    ///
    /// Scripted rather than derived from whether an output was opened, so a test
    /// can drive one particular error number: which one a machine gave is what a
    /// trace is read for, and a double that only ever answered "it failed" could
    /// not put a number there to read (ADR-0009).
    report_code: i32,
    /// How many reports had been written the first time this machine had
    /// nothing more to hand over.
    ///
    /// Recorded because a run goes on writing after that: it lets go of
    /// whatever it still holds on the way out, and those reports are the run's
    /// rather than the script's. Noted once — a wait that comes back empty is
    /// asked again, and a count taken on the second would already include what
    /// the first one set off.
    reports_when_the_stream_ran_out: Option<usize>,
    heartbeats: Vec<Instant>,
}

impl Default for Machine {
    /// A machine where everything the program asks for works, because a test about
    /// something else should not have to say so.
    fn default() -> Self {
        Self {
            report_code: 0,
            cursor: Instant::default(),
            now: Instant::default(),
            inbound: VecDeque::new(),
            next_stamp: 0,
            buttons: Vec::new(),
            did: Vec::new(),
            switched_on: true,
            ends_after: None,
            handed_over: 0,
            permits_input: true,
            output_comes_up: true,
            beats_arrive: true,
            no_output: favjit_host::NoOutput::NoService,
            output_frames: VecDeque::from([both_devices_up()]),
            output_refuses_beats: None,
            output_tears_a_frame: false,
            output_serves_for: Duration::ZERO,
            output_ended: Arc::new(Mutex::new(None)),
            output_calls: Arc::new(Mutex::new(Vec::new())),
            looks_for_devices: true,
            capture_turns: 1,
            capture_answered: Arc::new(Mutex::new(0)),
            looked_for_devices: None,
            refuses_to_take: Vec::new(),
            refuses_to_read: Vec::new(),
            refuses_to_release: Vec::new(),
            held: Vec::new(),
            reading: Vec::new(),
            supervised: true,
            warnings: Vec::new(),
            written: Rc::default(),
            identity_fails_at: None,
            makes: Some(identity(SINK_SEED).private().to_vec()),
            listened_with: None,
            link: None,
            binds_link: true,
            starts_alongside: true,
            observed: Arc::new(Mutex::new(Observed::default())),
            from_the_link: VecDeque::new(),
            pointers: Rc::default(),
            event_system_opens: true,
            simple_event_system_opens: true,
            pointer_feel: (None, None),
            reports: Vec::new(),
            reports_when_the_stream_ran_out: None,
            heartbeats: Vec::new(),
        }
    }
}

/// A machine that does exactly what the script says, on a clock the script
/// owns.
///
/// A share of its state rather than the state itself, for the reason
/// [`SimSource`] is one: a run holds the loop reading this machine's keyboards
/// while it goes on asking the machine everything else, and both ends reach the
/// same list of calls in the run's own order.
#[derive(Default)]
pub struct SimHost(Rc<RefCell<Machine>>);

impl SimHost {
    pub fn new() -> Self {
        Self::default()
    }

    /// Put the state through one of the machine's own builders.
    ///
    /// Taken out and put back rather than each builder being written twice, so
    /// that what a script says about a machine is stated in one place: a second
    /// spelling of every one of them is a second place for one to be wrong.
    fn scripted(&self, change: impl FnOnce(Machine) -> Machine) {
        let mut held = self.0.borrow_mut();
        let machine = core::mem::take(&mut *held);
        *held = change(machine);
    }

    /// A machine where converting has been switched off.
    pub fn with_converting_off(self) -> Self {
        self.scripted(Machine::with_converting_off);
        self
    }

    pub fn with_converting_off_after(self, events: usize) -> Self {
        self.scripted(|machine| machine.with_converting_off_after(events));
        self
    }

    pub fn with_output_lost_after(self, events: usize) -> Self {
        self.scripted(|machine| machine.with_output_lost_after(events));
        self
    }

    pub fn with_no_permission(self) -> Self {
        self.scripted(Machine::with_no_permission);
        self
    }

    pub fn with_no_output(self) -> Self {
        self.scripted(Machine::with_no_output);
        self
    }

    pub fn whose_output_is_never_ready(
        self,
        driver_activated: bool,
        driver_connected: bool,
        version_mismatched: bool,
    ) -> Self {
        self.scripted(|machine| {
            machine.whose_output_is_never_ready(
                driver_activated,
                driver_connected,
                version_mismatched,
            )
        });
        self
    }

    pub fn whose_output_says_nothing(self) -> Self {
        self.scripted(Machine::whose_output_says_nothing);
        self
    }

    pub fn whose_output_serves_for(self, how_long: Duration) -> Self {
        self.scripted(|machine| machine.whose_output_serves_for(how_long));
        self
    }

    pub fn whose_output_will_not_beat(self, code: i32) -> Self {
        self.scripted(|machine| machine.whose_output_will_not_beat(code));
        self
    }

    pub fn whose_output_tears_a_frame(self) -> Self {
        self.scripted(Machine::whose_output_tears_a_frame);
        self
    }

    pub fn output_ended(&self) -> Option<ServiceEnded> {
        self.0.borrow().output_ended()
    }

    pub fn output_calls(&self) -> Vec<OutputCall> {
        self.0.borrow().output_calls()
    }

    pub fn with_no_input(self) -> Self {
        self.scripted(Machine::with_no_input);
        self
    }

    pub fn with_a_keyboard_it_cannot_take(self, device: DeviceId, code: i32) -> Self {
        self.scripted(|machine| machine.with_a_keyboard_it_cannot_take(device, code));
        self
    }

    pub fn with_a_keyboard_it_cannot_read(self, device: DeviceId) -> Self {
        self.scripted(|machine| machine.with_a_keyboard_it_cannot_read(device));
        self
    }

    pub fn with_a_keyboard_it_cannot_release(self, device: DeviceId, code: i32) -> Self {
        self.scripted(|machine| machine.with_a_keyboard_it_cannot_release(device, code));
        self
    }

    pub fn with_no_watchdog(self) -> Self {
        self.scripted(Machine::with_no_watchdog);
        self
    }

    pub fn with_link(self, link: SimLink) -> Self {
        self.scripted(|machine| machine.with_link(link));
        self
    }

    pub fn with_no_link_socket(self) -> Self {
        self.scripted(Machine::with_no_link_socket);
        self
    }

    pub fn with_nothing_to_run_alongside(self) -> Self {
        self.scripted(Machine::with_nothing_to_run_alongside);
        self
    }

    pub fn with_identity_file(self, bytes: &[u8]) -> Self {
        self.scripted(|machine| machine.with_identity_file(bytes));
        self
    }

    pub fn that_can_make(self, private: &[u8]) -> Self {
        self.scripted(|machine| machine.that_can_make(private));
        self
    }

    pub fn with_no_keypair(self) -> Self {
        self.scripted(Machine::with_no_keypair);
        self
    }

    pub fn with_an_unwritable_identity_file(self) -> Self {
        self.scripted(Machine::with_an_unwritable_identity_file);
        self
    }

    pub fn whose_identity_fails_at(self, call: IdentityCall) -> Self {
        self.scripted(|machine| machine.whose_identity_fails_at(call));
        self
    }

    pub fn identity_calls(&self) -> Vec<IdentityCall> {
        self.0.borrow().identity_calls()
    }

    pub fn kept(&self) -> Option<Vec<u8>> {
        self.0.borrow().kept()
    }

    pub fn identity_file(&self) -> Option<Vec<u8>> {
        self.0.borrow().identity_file()
    }

    pub fn listened_with(&self) -> Option<Identity> {
        self.0.borrow().listened_with()
    }

    pub fn link_calls(&self) -> Vec<Call> {
        self.0.borrow().link_calls()
    }

    pub fn delivered(&self) -> Vec<EventKind> {
        self.0.borrow().delivered()
    }

    pub fn refused(&self) -> Vec<Vec<u8>> {
        self.0.borrow().refused()
    }

    pub fn link_closed(&self) -> Vec<String> {
        self.0.borrow().link_closed()
    }

    pub fn frames_read(&self) -> usize {
        self.0.borrow().frames_read()
    }

    pub fn advertisements(&self) -> usize {
        self.0.borrow().advertisements()
    }

    pub fn did(&self) -> Vec<Did> {
        self.0.borrow().did()
    }

    pub fn took_input(&self) -> Option<bool> {
        self.0.borrow().took_input()
    }

    /// Copied out rather than borrowed, for the reason [`SimSource`]'s answers
    /// are: a borrow held across a run's own next call would refuse it.
    pub fn reports(&self) -> Vec<(Instant, OutputReport, Vec<u8>)> {
        self.0.borrow().reports().to_vec()
    }

    pub fn with_pointers(self, pointers: Vec<SimPointer>) -> Self {
        self.scripted(|machine| machine.with_pointers(pointers));
        self
    }

    pub fn with_no_writable_event_system(self) -> Self {
        self.scripted(Machine::with_no_writable_event_system);
        self
    }

    pub fn with_no_event_system(self) -> Self {
        self.scripted(Machine::with_no_event_system);
        self
    }

    pub fn told_to_tune(self, dpi: Option<f64>, acceleration: Option<f64>) -> Self {
        self.scripted(|machine| machine.told_to_tune(dpi, acceleration));
        self
    }

    pub fn pointers(&self) -> Vec<SimPointer> {
        self.0.borrow().pointers()
    }

    pub fn reports_while_reading(&self) -> Vec<(Instant, OutputReport, Vec<u8>)> {
        self.0.borrow().reports_while_reading().to_vec()
    }

    pub fn advance(&mut self, by: Duration) -> &mut Self {
        self.0.borrow_mut().advance(by);
        self
    }

    pub fn attach_built_in(&mut self, id: DeviceId) -> &mut Self {
        self.0.borrow_mut().attach_built_in(id);
        self
    }

    pub fn attach_external(&mut self, id: DeviceId, vendor_id: u16, product_id: u16) -> &mut Self {
        self.0
            .borrow_mut()
            .attach_external(id, vendor_id, product_id);
        self
    }

    pub fn attach_anonymous(&mut self, id: DeviceId) -> &mut Self {
        self.0.borrow_mut().attach_anonymous(id);
        self
    }

    pub fn detach(&mut self, id: DeviceId) -> &mut Self {
        self.0.borrow_mut().detach(id);
        self
    }

    pub fn press(&mut self, device: DeviceId, key: Key) -> &mut Self {
        self.0.borrow_mut().press(device, key);
        self
    }

    pub fn release(&mut self, device: DeviceId, key: Key) -> &mut Self {
        self.0.borrow_mut().release(device, key);
        self
    }

    pub fn tap(&mut self, device: DeviceId, key: Key) -> &mut Self {
        self.0.borrow_mut().tap(device, key);
        self
    }

    pub fn hold(&mut self, device: DeviceId, key: Key, held: Duration) -> &mut Self {
        self.0.borrow_mut().hold(device, key, held);
        self
    }

    pub fn pointer(&mut self, device: DeviceId, report: PointerReport) -> &mut Self {
        self.0.borrow_mut().pointer(device, report);
        self
    }

    pub fn probe(&mut self) -> &mut Self {
        self.0.borrow_mut().probe();
        self
    }

    pub fn script(&mut self, kind: EventKind) -> &mut Self {
        self.0.borrow_mut().script(kind);
        self
    }

    pub fn heartbeats(&self) -> Vec<Instant> {
        self.0.borrow().heartbeats().to_vec()
    }

    pub fn warnings(&self) -> Vec<String> {
        self.0.borrow().warnings().to_vec()
    }
}

impl Machine {
    /// A machine where converting has been switched off.
    pub fn with_converting_off(mut self) -> Self {
        self.switched_on = false;
        self
    }

    /// A machine where converting is switched off after this many events, which is
    /// somebody choosing the menu item while favjit is running.
    pub fn with_converting_off_after(mut self, events: usize) -> Self {
        self.ends_after = Some((events, End::SwitchedOff));
        self
    }

    /// A machine whose output device goes away after this many events — the driver's
    /// daemon stopping under a running converter.
    pub fn with_output_lost_after(mut self, events: usize) -> Self {
        self.ends_after = Some((events, End::OutputGone));
        self
    }

    /// A machine that will not let this process read the keyboards.
    pub fn with_no_permission(mut self) -> Self {
        self.permits_input = false;
        self
    }

    /// A machine where nothing answers where the service listens — the driver is
    /// not installed, or this process is not privileged for it.
    pub fn with_no_output(mut self) -> Self {
        self.output_comes_up = false;
        self
    }

    /// A machine where the service answers and no keyboard ever arrives, with
    /// these three things last said about itself.
    ///
    /// Scripted as the frames it sends rather than as the answer a run gives
    /// for them: which flags leave a device short of ready, and what a run that
    /// gave up waiting reports, are the run's own reading of what arrived
    /// (ADR-0006) — and a script that handed the answer over would assert that
    /// reading against itself.
    pub fn whose_output_is_never_ready(
        mut self,
        driver_activated: bool,
        driver_connected: bool,
        version_mismatched: bool,
    ) -> Self {
        self.output_frames = VecDeque::from([status_frame(&[
            (DRIVER_ACTIVATED, driver_activated),
            (DRIVER_CONNECTED, driver_connected),
            (VERSION_MISMATCHED, version_mismatched),
        ])]);
        self
    }

    /// A machine whose output device says nothing at all about itself.
    ///
    /// The other half of never being ready: a service that answers the socket
    /// and then goes quiet, which a run tells apart from one that said which of
    /// its own parts is missing.
    pub fn whose_output_says_nothing(mut self) -> Self {
        self.output_frames = VecDeque::new();
        self
    }

    /// A machine whose output device comes up and stays there this long,
    /// keeping the service's own cadence the whole time.
    ///
    /// How a test about staying connected is written: the service beats,
    /// expects to be answered, and closes a connection it has heard nothing on
    /// — so what a run has to do to reach the end of this is the whole of what
    /// it has to do on a real machine.
    pub fn whose_output_serves_for(mut self, how_long: Duration) -> Self {
        self.output_serves_for = how_long;
        self
    }

    /// A machine whose service answers a beat with this number.
    ///
    /// The number and not a refusal, because that is what the write answers
    /// with: a connection the service has closed and a write that ran out of
    /// time are the same failure and different problems, and which it was is
    /// what a recording is read for (ADR-0009).
    pub fn whose_output_will_not_beat(mut self, code: i32) -> Self {
        self.output_refuses_beats = Some(code);
        self
    }

    /// A machine where part of one frame arrives and the rest does not.
    ///
    /// A read and not a script of bytes: what a stream socket does with a frame
    /// that straddles the bound one read was given is the host's own to answer,
    /// and there is nothing above the boundary that could tell a torn frame
    /// from a whole one by looking at bytes (ADR-0006). What is here is the
    /// answer, so that what a run does about it is driven.
    pub fn whose_output_tears_a_frame(mut self) -> Self {
        self.output_tears_a_frame = true;
        self
    }

    /// Why the service let the connection go, once it has.
    pub fn output_ended(&self) -> Option<ServiceEnded> {
        *self.output_ended.lock().unwrap()
    }

    /// What the output device's connection was asked, in order.
    pub fn output_calls(&self) -> Vec<OutputCall> {
        self.output_calls.lock().unwrap().clone()
    }

    /// Note one call made on the connection before either end of it was taken.
    ///
    /// The same list the ends themselves are noted on, so that what a script
    /// reads is the whole order over the connection rather than the part of it
    /// that happened after the run had somewhere to write.
    fn note_output(&self, call: OutputCall) {
        self.output_calls.lock().unwrap().push(call);
    }

    /// A machine that cannot start reading its keyboards at all.
    pub fn with_no_input(mut self) -> Self {
        self.looks_for_devices = false;
        self
    }

    /// A machine where taking this one keyboard fails with that code — something
    /// else holds it, or this process is not privileged to.
    pub fn with_a_keyboard_it_cannot_take(mut self, device: DeviceId, code: i32) -> Self {
        self.refuses_to_take.push((device, code));
        self
    }

    /// A machine where this one keyboard is taken and then will not be read: the
    /// device went away between the two calls.
    pub fn with_a_keyboard_it_cannot_read(mut self, device: DeviceId) -> Self {
        self.refuses_to_read.push(device);
        self
    }

    /// A machine where giving back this one keyboard fails with that code.
    pub fn with_a_keyboard_it_cannot_release(mut self, device: DeviceId, code: i32) -> Self {
        self.refuses_to_release.push((device, code));
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
        self.observed = link.observed();
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
    pub fn with_identity_file(self, bytes: &[u8]) -> Self {
        self.written.borrow_mut().file = Some(bytes.to_vec());
        self
    }

    /// The private key this machine's entropy will hand over when a keypair is
    /// made from it.
    ///
    /// The private half and not the whole [`Identity`]: what this machine can
    /// actually offer is bytes nobody can predict, and the public half is
    /// derived from them for real ([`favjit_noise::keypair`]) rather than handed
    /// over whole — [`derived_from`] computes the same derivation for a test to
    /// assert against.
    pub fn that_can_make(mut self, private: &[u8]) -> Self {
        self.makes = Some(private.to_vec());
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
        self.identity_fails_at = Some(IdentityCall::Wrote);
        self
    }

    pub fn whose_identity_fails_at(mut self, call: IdentityCall) -> Self {
        self.identity_fails_at = Some(call);
        self
    }

    pub fn identity_calls(&self) -> Vec<IdentityCall> {
        self.written.borrow().calls.clone()
    }

    /// What was written to the identity file, if anything. `None` is the assertion
    /// that nothing was.
    pub fn kept(&self) -> Option<Vec<u8>> {
        self.written.borrow().kept.clone()
    }

    /// What the identity file holds now.
    pub fn identity_file(&self) -> Option<Vec<u8>> {
        self.written.borrow().file.clone()
    }

    /// The identity the run listened with, if it bound a link at all.
    pub fn listened_with(&self) -> Option<Identity> {
        self.listened_with.clone()
    }

    /// Every call the link's loop made into this machine, in order.
    pub fn link_calls(&self) -> Vec<Call> {
        self.observed().calls.clone()
    }

    /// Every event the link put into the stream, in order — raw, the way it
    /// reached `engine`'s own resolver rather than what that resolver made of it.
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

    /// Note one more thing done, folding it into the last entry when it is the
    /// same: a call asked over and over while nothing else happens is one thing
    /// having happened, not that many.
    fn note(&mut self, did: Did) {
        if self.did.last() != Some(&did) {
            self.did.push(did);
        }
    }

    /// Whether this machine ever started reading its keyboards, and whether that
    /// worked.
    pub fn took_input(&self) -> Option<bool> {
        self.looked_for_devices
    }

    /// Every raw report a [`SinkHost::send_report`] call actually wrote, in the
    /// bytes it wrote and when — what reached the output device, one call at a
    /// time.
    pub fn reports(&self) -> &[(Instant, OutputReport, Vec<u8>)] {
        &self.reports
    }

    /// Put these pointing devices on the machine, in this order (ADR-0011).
    pub fn with_pointers(self, pointers: Vec<SimPointer>) -> Self {
        *self.pointers.borrow_mut() = pointers;
        self
    }

    /// A machine whose writable view of the event system will not open, so a
    /// run has only the simpler one to settle for.
    pub fn with_no_writable_event_system(mut self) -> Self {
        self.event_system_opens = false;
        self
    }

    /// A machine where neither view opens, which is a machine with no pointing
    /// devices a run can reach at all.
    pub fn with_no_event_system(mut self) -> Self {
        self.event_system_opens = false;
        self.simple_event_system_opens = false;
        self
    }

    /// What this machine was told the output pointer should feel like, as
    /// `--pointers` would.
    pub fn told_to_tune(mut self, dpi: Option<f64>, acceleration: Option<f64>) -> Self {
        self.pointer_feel = (dpi, acceleration);
        self
    }

    /// The pointing devices, to read back what was written to each.
    ///
    /// Copied out rather than borrowed, for the reason [`SimPointers`] shares
    /// them: a borrow held across a run's own write to one would refuse it.
    pub fn pointers(&self) -> Vec<SimPointer> {
        self.pointers.borrow().clone()
    }

    /// The reports written while this machine still had events to hand over.
    ///
    /// Everything a script's own presses and releases produced, and nothing the
    /// run wrote once the stream was over — it lets go of what it is still
    /// holding then, so the last of [`SimHost::reports`] says that nothing is
    /// held whatever the script was about. A test reading a sequence of
    /// keystrokes wants this one; a test about the letting-go itself wants the
    /// whole of the other.
    pub fn reports_while_reading(&self) -> &[(Instant, OutputReport, Vec<u8>)] {
        match self.reports_when_the_stream_ran_out {
            Some(at) => &self.reports[..at],
            None => &self.reports,
        }
    }

    /// Move the script's clock forward. Nothing else happens — no time passes
    /// anywhere real, which is the point.
    pub fn advance(&mut self, by: Duration) -> &mut Self {
        self.cursor = advance(self.cursor, by);
        self
    }

    /// Announce the Mac's own keyboard: on the SPI bus, naming itself the way an
    /// internal one does, and with no USB identity.
    ///
    /// A method rather than properties a script writes for itself: what the
    /// machine's own keyboard reports is this crate's to say, and a script that
    /// spelled it out would be stating the platform's answer in the same place
    /// it asserts about the run's. A script that wants a device none of these
    /// three describes — a mouse, a keyboard on some other bus — writes the
    /// properties through [`SimHost::script`], where it is plain that it is
    /// saying what the platform said.
    pub fn attach_built_in(&mut self, id: DeviceId) -> &mut Self {
        self.script(hid_keyboard(
            id,
            "FIFO",
            "Apple Internal Keyboard / Trackpad",
            None,
            None,
        ))
    }

    /// Announce a keyboard on the USB bus, named by USB identity.
    pub fn attach_external(&mut self, id: DeviceId, vendor_id: u16, product_id: u16) -> &mut Self {
        self.script(hid_keyboard(
            id,
            "USB",
            "a keyboard",
            Some(i64::from(vendor_id)),
            Some(i64::from(product_id)),
        ))
    }

    /// Announce a keyboard on the USB bus that reports no identity at all.
    ///
    /// Its own method because it is a shape neither of the others covers: a
    /// keyboard that is not the machine's own and that no rule can single out.
    pub fn attach_anonymous(&mut self, id: DeviceId) -> &mut Self {
        self.script(hid_keyboard(id, "USB", "a keyboard", None, None))
    }

    /// Take a keyboard away without releasing anything first — the shape of an
    /// unplugged cable, and of the stuck-modifier case ADR-0002 puts on the
    /// sink.
    pub fn detach(&mut self, id: DeviceId) -> &mut Self {
        self.script(EventKind::DeviceLost(id))
    }

    pub fn press(&mut self, device: DeviceId, key: Key) -> &mut Self {
        self.key(device, key, true)
    }

    pub fn release(&mut self, device: DeviceId, key: Key) -> &mut Self {
        self.key(device, key, false)
    }

    /// One HID element value for `key`, on the page and usage its own table
    /// names — the level a real `IOHIDQueue` would report it at, so `engine`'s
    /// own resolver is what names the key rather than this script assuming it.
    fn key(&mut self, device: DeviceId, key: Key, down: bool) -> &mut Self {
        let (page, usage) = usage::read_at(key)
            .unwrap_or_else(|| panic!("{key:?} has no HID usage a script could report it at"));
        self.script(EventKind::HidValue {
            device,
            page,
            usage: u32::from(usage),
            stamp: 0,
            value: i64::from(down),
        })
    }

    pub fn tap(&mut self, device: DeviceId, key: Key) -> &mut Self {
        self.press(device, key).release(device, key)
    }

    /// Press, hold for `held`, release.
    pub fn hold(&mut self, device: DeviceId, key: Key, held: Duration) -> &mut Self {
        self.press(device, key).advance(held).release(device, key)
    }

    /// One pointer report from a device, as the hardware described it.
    ///
    /// Scripted as the HID element values a real queue would report for it —
    /// the axes and wheel always, and a button only where it changed since the
    /// last report this device sent, which is what makes a report the transition
    /// a real device's queue carries rather than the whole state repeated.
    /// [`EventKind::HidValuesDone`] closes it: without it, this report would
    /// only resolve once *another* value for this device arrived, which is one
    /// report too late for the last one a script ever sends.
    pub fn pointer(&mut self, device: DeviceId, report: PointerReport) -> &mut Self {
        self.next_stamp += 1;
        let stamp = self.next_stamp;
        self.hid_value(
            device,
            page::GENERIC_DESKTOP,
            usage::POINTER_X,
            stamp,
            i64::from(report.dx),
        );
        self.hid_value(
            device,
            page::GENERIC_DESKTOP,
            usage::POINTER_Y,
            stamp,
            i64::from(report.dy),
        );
        if report.vertical_wheel != 0 {
            self.hid_value(
                device,
                page::GENERIC_DESKTOP,
                usage::POINTER_WHEEL,
                stamp,
                i64::from(report.vertical_wheel),
            );
        }
        let held = self.buttons(device);
        for button in 1..=32u8 {
            if held.holds(button) != report.buttons.holds(button) {
                self.hid_value(
                    device,
                    page::BUTTON,
                    u32::from(button),
                    stamp,
                    i64::from(report.buttons.holds(button)),
                );
            }
        }
        self.set_buttons(device, report.buttons);
        self.script(EventKind::HidValuesDone { device })
    }

    fn hid_value(
        &mut self,
        device: DeviceId,
        page: u32,
        usage: u32,
        stamp: u64,
        value: i64,
    ) -> &mut Self {
        self.script(EventKind::HidValue {
            device,
            page,
            usage,
            stamp,
            value,
        })
    }

    fn buttons(&self, device: DeviceId) -> Buttons {
        self.buttons
            .iter()
            .find(|(id, _)| *id == device)
            .map_or(Buttons::NONE, |(_, buttons)| *buttons)
    }

    fn set_buttons(&mut self, device: DeviceId, buttons: Buttons) {
        match self.buttons.iter_mut().find(|(id, _)| *id == device) {
            Some((_, held)) => *held = buttons,
            None => self.buttons.push((device, buttons)),
        }
    }

    /// Ask, as a watchdog would, whether the loop is still turning.
    pub fn probe(&mut self) -> &mut Self {
        self.script(EventKind::Probe)
    }

    pub fn script(&mut self, kind: EventKind) -> &mut Self {
        self.inbound.push_back(HostEvent {
            at: self.cursor,
            kind,
        });
        self
    }

    /// When the loop reported coming back round, in order.
    pub fn heartbeats(&self) -> &[Instant] {
        &self.heartbeats
    }

    /// What the run said about itself, in order.
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    /// Nothing more is coming, noting where the script's own reports end.
    ///
    /// Not the wait that comes back empty with something still scripted for
    /// later: that one is a real machine's wait timing out, and a repeat
    /// becoming due after it writes a report the script asked for as much as a
    /// press does.
    fn nothing_more(&mut self) -> Option<HostEvent> {
        let first = self.reports_when_the_stream_ran_out.is_none();
        self.reports_when_the_stream_ran_out
            .get_or_insert(self.reports.len());
        // Said once, and only where the script did not end the run for a reason
        // of its own: a bound reached and a queue emptied are two facts a run
        // reads apart, so a machine that said both would let a test pass on the
        // wrong one.
        let ended = first && !self.effectively_off() && !self.output_is_gone();
        ended.then_some(HostEvent {
            at: self.now,
            kind: EventKind::InputGone,
        })
    }

    /// Whether this run's own scripted bound on the switch has been reached.
    fn effectively_off(&self) -> bool {
        matches!(self.ends_after, Some((after, End::SwitchedOff)) if self.handed_over >= after)
    }

    /// Whether this run's own scripted bound on the output has been reached.
    fn output_is_gone(&self) -> bool {
        matches!(self.ends_after, Some((after, End::OutputGone)) if self.handed_over >= after)
    }

    /// Whether this is input from a keyboard nothing here is reading.
    ///
    /// Only the kinds that come off a keyboard's own queue: an announcement, a
    /// frame from the link and the watchdog's probe are the machine's to deliver
    /// whatever it has open.
    fn unread(&self, kind: &EventKind) -> bool {
        match kind {
            EventKind::HidValue { device, .. }
            | EventKind::HidValuesDone { device }
            | EventKind::HookedKey { device, .. }
            | EventKind::MouseReport { device, .. } => !self.reading.contains(device),
            _ => false,
        }
    }
}

/// The bodies of [`SinkInputHost`], on the state itself, for the reason
/// [`SimSource`]'s are: the one operation that hands the run the loop reading
/// this machine's keyboards has to hand it a share of this.
impl Machine {
    fn switched_on(&mut self) -> bool {
        self.note(Did::AskedIfSwitchedOn);
        self.switched_on && !self.effectively_off()
    }

    fn may_read_input(&mut self) -> bool {
        self.note(Did::AskedPermission);
        self.permits_input
    }

    fn request_input_permission(&mut self) -> bool {
        self.note(Did::RequestedPermission);
        self.permits_input
    }

    fn output_connected(&mut self) -> bool {
        self.output_comes_up && !self.output_is_gone()
    }

    fn stop_requested(&mut self) -> bool {
        // The script's own end, standing in for whatever a real run would be
        // asked to stop for: nothing else here ever will be, so a run with
        // nothing further to give has finished the same way one that was asked
        // to stop has. Not where the script ended the run for a reason of its
        // own: a bound reached and a queue emptied are two facts a run reads
        // apart, and a machine that answered both would let a test pass on the
        // wrong one.
        let asked = self.inbound.is_empty()
            && self.from_the_link.is_empty()
            && !self.effectively_off()
            && !self.output_is_gone();
        if asked {
            // Where the script's own reports end is noted here as well as where
            // the stream runs out, because a run may read this before it makes
            // the wait that would run out: what it writes afterward is the
            // letting-go, whichever of the two it learned the end from.
            self.reports_when_the_stream_ran_out
                .get_or_insert(self.reports.len());
        }
        asked
    }

    /// Turned to a standstill before returning, on this thread, for the reason
    /// [`SimHost::run_alongside`] is: what the loop puts on the stream goes in
    /// before the converting loop reads it either way (ADR-0007).
    ///
    /// The devices are not served through it. What a script says about a
    /// keyboard is put on the stream where the script said it, under the number
    /// the script gave it — so the loop finds none here, and what it is driven
    /// for is the turn, the probes and what the run asks of it. Serving them
    /// through it means the numbers being the loop's rather than the script's,
    /// which is every assertion about a device in the suite.
    fn look_for_devices(&mut self, work: favjit_host::capture::SinkLoop) -> bool {
        self.note(Did::LookedForDevices);
        self.looked_for_devices = Some(self.looks_for_devices);
        let _ = self.looks_for_devices.then(|| {
            work(&mut SimCapture {
                turns: self.capture_turns,
                answered: Arc::clone(&self.capture_answered),
                hooked: false,
            })
        });
        self.looks_for_devices
    }

    fn take_device(&mut self, device: DeviceId, exclusive: bool) -> i32 {
        self.note(Did::TookDevice { device, exclusive });
        let code = self
            .refuses_to_take
            .iter()
            .find(|(refused, _)| *refused == device)
            .map_or(0, |(_, code)| *code);
        if code == 0 && exclusive {
            self.held.push(device);
        }
        code
    }
}

/// The loop reading a scripted machine's keyboards, as the run holds it.
struct SimReading(Rc<RefCell<Machine>>);

impl favjit_host::sink::Capturing for SimReading {
    fn take_device(&mut self, device: DeviceId, exclusive: bool) -> i32 {
        self.0.borrow_mut().take_device(device, exclusive)
    }

    fn read_device(&mut self, device: DeviceId) -> bool {
        self.0.borrow_mut().read_device(device)
    }

    fn next_held_device(&mut self) -> Option<favjit_host::sink::Holding> {
        self.0.borrow_mut().next_held_device()
    }

    fn give_it_back(&mut self, device: favjit_host::Handed) -> i32 {
        self.0.borrow_mut().give_it_back(device)
    }

    fn forget_what_was_given_back(&mut self, device: favjit_host::Handed, code: i32) {
        self.0.borrow_mut().forget_what_was_given_back(device, code)
    }
}

impl Machine {
    fn read_device(&mut self, device: DeviceId) -> bool {
        self.note(Did::ReadDevice { device });
        if self.refuses_to_read.contains(&device) {
            return false;
        }
        self.reading.push(device);
        true
    }

    /// The machine's own name for a keyboard is the number the run gave it,
    /// because a script has no handles: what a real host carries the two of is
    /// one thing here.
    fn next_held_device(&mut self) -> Option<favjit_host::sink::Holding> {
        self.note(Did::AskedForHeldDevice);
        self.held
            .last()
            .copied()
            .map(|device| favjit_host::sink::Holding {
                device,
                at: favjit_host::Handed(device.0),
            })
    }

    fn give_it_back(&mut self, device: favjit_host::Handed) -> i32 {
        let device = DeviceId(device.0);
        self.note(Did::ReleasedDevice { device });
        match self
            .refuses_to_release
            .iter()
            .find(|(refused, _)| *refused == device)
        {
            Some((_, code)) => *code,
            None => 0,
        }
    }

    fn forget_what_was_given_back(&mut self, device: favjit_host::Handed, code: i32) {
        let device = DeviceId(device.0);
        let at = (code == 0)
            .then(|| self.held.iter().position(|&held| held == device))
            .flatten();
        let _ = at.map(|at| self.held.remove(at));
    }
}

impl SinkInputHost for SimHost {
    fn switched_on(&mut self) -> bool {
        self.0.borrow_mut().switched_on()
    }

    fn may_read_input(&mut self) -> bool {
        self.0.borrow_mut().may_read_input()
    }

    fn request_input_permission(&mut self) -> bool {
        self.0.borrow_mut().request_input_permission()
    }

    fn output_connected(&mut self) -> bool {
        self.0.borrow_mut().output_connected()
    }

    fn stop_requested(&mut self) -> bool {
        self.0.borrow_mut().stop_requested()
    }

    fn look_for_devices(
        &mut self,
        work: favjit_host::capture::SinkLoop,
    ) -> Option<Box<dyn favjit_host::sink::Capturing>> {
        let looked = self.0.borrow_mut().look_for_devices(work);
        match looked {
            true => Some(Box::new(SimReading(Rc::clone(&self.0)))),
            false => None,
        }
    }
}

impl Host for SimHost {
    fn now(&mut self) -> Instant {
        self.0.borrow_mut().now()
    }

    fn next_event(&mut self, deadline: Instant) -> Option<HostEvent> {
        self.0.borrow_mut().next_event(deadline)
    }

    fn is_supervised(&mut self) -> bool {
        self.0.borrow_mut().is_supervised()
    }

    fn warn(&mut self, message: core::fmt::Arguments) {
        self.0.borrow_mut().warn(message);
    }

    fn heartbeat(&mut self) -> Result<(), Trouble> {
        self.0.borrow_mut().heartbeat()
    }
}

impl favjit_host::PointerHost for SimHost {
    fn wanted_pointer_feel(&mut self) -> (Option<f64>, Option<f64>) {
        self.0.borrow_mut().wanted_pointer_feel()
    }

    fn output_vendor(&mut self) -> i64 {
        self.0.borrow_mut().output_vendor()
    }

    fn open_event_system(&mut self) -> Option<Box<dyn favjit_host::Pointers>> {
        self.0.borrow_mut().open_event_system()
    }

    fn open_simple_event_system(&mut self) -> Option<Box<dyn favjit_host::Pointers>> {
        self.0.borrow_mut().open_simple_event_system()
    }
}

impl SinkHost for SimHost {
    fn reach_the_output(
        &mut self,
    ) -> Result<Box<dyn favjit_host::sink::Reaching>, favjit_host::NoOutput> {
        let serving = self.0.borrow_mut().reach_the_output()?;
        Ok(Box::new(SimReaching {
            machine: Rc::clone(&self.0),
            serving,
        }))
    }

    /// The same place a device's reports would go, because what a script asserts
    /// about a run that injects nothing is what it would have written: a writer
    /// that dropped them would leave the mode with nothing to read.
    fn instead_of_the_output(&mut self) -> Box<dyn favjit_host::sink::Injecting> {
        Box::new(SimInjecting(Rc::clone(&self.0)))
    }

    fn bind_link(&mut self) -> Option<Box<dyn favjit_host::link::LinkHost + Send>> {
        self.0.borrow_mut().bind_link()
    }

    fn run_alongside(&mut self, work: Box<dyn FnOnce() + Send>) -> bool {
        self.0.borrow_mut().run_alongside(work)
    }

    fn run_output_alongside(&mut self, work: Box<dyn FnOnce() + Send>) -> bool {
        self.0.borrow_mut().run_output_alongside(work)
    }
}

/// The connection a scripted machine answered a reach with, before its two ends
/// have been taken.
struct SimReaching {
    machine: Rc<RefCell<Machine>>,
    serving: Box<dyn favjit_host::output::OutputHost>,
}

impl favjit_host::sink::Reaching for SimReaching {
    /// Noted rather than kept, because there is nothing here a bound would
    /// change: what a scripted service does with a write is what the script
    /// says, and the order a run bounds it in is what an assertion reads.
    fn do_not_block_writes(&mut self, _write_timeout: Duration) -> bool {
        self.machine
            .borrow_mut()
            .note_output(OutputCall::BoundWrites);
        true
    }

    fn both_ends(self: Box<Self>) -> Option<favjit_host::sink::Opened> {
        Some(favjit_host::sink::Opened {
            serving: self.serving,
            writing: Box::new(SimInjecting(Rc::clone(&self.machine))),
        })
    }
}

/// The device a scripted machine's converted input goes out through, as the run
/// holds it.
struct SimInjecting(Rc<RefCell<Machine>>);

impl favjit_host::sink::Injecting for SimInjecting {
    fn send_report(&mut self, report: OutputReport, bytes: &[u8]) -> i32 {
        self.0.borrow_mut().send_report(report, bytes)
    }
}

impl IdentityStore for SimHost {
    fn read(&mut self) -> Option<Vec<u8>> {
        self.0.borrow_mut().read()
    }

    fn make_directory(&mut self) -> Result<(), Trouble> {
        self.0.borrow_mut().make_directory()
    }

    fn open(&mut self) -> Result<Box<dyn favjit_host::Writing>, Trouble> {
        self.0.borrow_mut().open()
    }
}

impl Entropy for SimHost {
    fn fill(&mut self, into: &mut [u8]) -> bool {
        self.0.borrow_mut().fill(into)
    }
}

impl Host for Machine {
    fn now(&mut self) -> Instant {
        self.now
    }

    /// Returns `None` once nothing scripted arrives by `deadline`, the same
    /// answer a real host gives when its wait times out — which is what lets
    /// `engine`'s own repeat timer fire between two scripted events far enough
    /// apart, rather than this double handing the later one over regardless of
    /// how much time it says passed first.
    ///
    /// What is scripted keeps arriving after this run's own scripted bound has
    /// been reached. Not `None` from there on: the switch is a file and the
    /// output is a socket, and neither stops the keyboards in front of the
    /// person producing events — so a machine that fell silent at the bound
    /// would hide a run that never looks at either while something is arriving.
    /// Scripting more afterward is how a test proves how much of it is read.
    fn next_event(&mut self, deadline: Instant) -> Option<HostEvent> {
        let arrived: Vec<HostEvent> = self.observed().stream.drain(..).collect();
        self.from_the_link.extend(arrived);

        // Input from a keyboard this machine is not reading goes wherever it
        // would have gone anyway, and not to this process. Dropped at the front
        // rather than filtered out of the script when it was written, because
        // which keyboards a run reads is the run's own answer and it has not
        // been given yet when the script is laid down.
        while self
            .inbound
            .front()
            .is_some_and(|event| self.unread(&event.kind))
        {
            self.inbound.pop_front();
        }

        let from_the_link = match (self.inbound.front(), self.from_the_link.front()) {
            (Some(local), Some(remote)) => remote.at < local.at,
            (None, Some(_)) => true,
            _ => false,
        };
        let next = if from_the_link {
            self.from_the_link.front()
        } else {
            self.inbound.front()
        };

        match next {
            // Strictly before the deadline, not at it: a repeat due at the same
            // instant as this event is already ticking, and a scripted press or
            // release tied with it is read as arriving one moment later, the same
            // order a real queue would resolve two things enqueued that close.
            Some(event) if event.at < deadline => {
                let event = if from_the_link {
                    self.from_the_link.pop_front()
                } else {
                    self.inbound.pop_front()
                }
                .expect("just peeked at the front of this same queue");
                self.now = event.at;
                self.handed_over += 1;
                Some(event)
            }
            // Something is still scripted, only later than `deadline`: a real
            // wait timing out here is what lets a repeat between now and that
            // later event become due, so the clock has to reach the deadline
            // for that comparison to mean anything on the next call.
            Some(_) => {
                self.now = self.now.max(deadline);
                None
            }
            // Nothing is scripted any more. A repeat with nothing left to
            // compare it against would otherwise re-arm itself past every
            // deadline this returns, chasing a key a script never releases —
            // so the clock stays where the last real event left it rather
            // than advancing to meet a wake-up that exists only to ask again.
            None => self.nothing_more(),
        }
    }

    fn is_supervised(&mut self) -> bool {
        self.supervised
    }

    fn warn(&mut self, message: core::fmt::Arguments) {
        // Formatted here rather than kept as it was passed: `Arguments` borrows what
        // it will print, so a machine that stored it could not answer afterwards —
        // which is the whole of what this machine is for.
        self.warnings.push(message.to_string());
        self.note(Did::Warned);
    }

    /// Arriving, unless a script says this machine's supervisor has stopped
    /// reading: which of the two it is decides whether a run says anything,
    /// and saying it is the run's (ADR-0006).
    fn heartbeat(&mut self) -> Result<(), Trouble> {
        self.heartbeats.push(self.now);
        match self.beats_arrive {
            true => Ok(()),
            false => Err(Trouble(String::from("nothing is reading the beats"))),
        }
    }
}

impl favjit_host::PointerHost for Machine {
    fn wanted_pointer_feel(&mut self) -> (Option<f64>, Option<f64>) {
        self.note(Did::TunedOutput);
        self.pointer_feel
    }

    fn output_vendor(&mut self) -> i64 {
        OUTPUT_VENDOR
    }

    /// Whichever view a script says will open, so a run that settles for the
    /// simpler one can be driven: the properties this machine answers with are
    /// the same either way, and which client asked for them is what the run
    /// decides.
    fn open_event_system(&mut self) -> Option<Box<dyn favjit_host::Pointers>> {
        self.note(Did::OpenedTheEventSystem);
        viewing(self.event_system_opens, &self.pointers)
    }

    fn open_simple_event_system(&mut self) -> Option<Box<dyn favjit_host::Pointers>> {
        self.note(Did::OpenedTheSimpleEventSystem);
        viewing(self.simple_event_system_opens, &self.pointers)
    }
}

/// Either view of a scripted machine's pointing devices, and nothing where the
/// script says it will not open.
fn viewing(
    opens: bool,
    pointers: &Rc<RefCell<Vec<SimPointer>>>,
) -> Option<Box<dyn favjit_host::Pointers>> {
    opens.then(|| Box::new(SimPointers(Rc::clone(pointers))) as Box<dyn favjit_host::Pointers>)
}

/// One open view of them, as the run holds it.
///
/// Shared with [`SimHost`] rather than taken out of it, because what a test
/// reads afterwards is what was written to each device, and a view that carried
/// the devices away would leave the machine with none.
struct SimPointers(Rc<RefCell<Vec<SimPointer>>>);

impl favjit_host::Pointers for SimPointers {
    fn look(&mut self) -> Vec<Box<dyn favjit_host::Pointing>> {
        (0..self.0.borrow().len())
            .map(|at| {
                Box::new(SimPointing {
                    at,
                    all: Rc::clone(&self.0),
                }) as Box<dyn favjit_host::Pointing>
            })
            .collect()
    }
}

/// One of them, named by its place in that list the way a real one is named by
/// the service behind it.
struct SimPointing {
    at: usize,
    all: Rc<RefCell<Vec<SimPointer>>>,
}

impl favjit_host::Pointing for SimPointing {
    fn fixed(&mut self, key: &str) -> Option<f64> {
        self.all.borrow().get(self.at)?.fixed.get(key).copied()
    }

    fn integer(&mut self, key: &str) -> Option<i64> {
        self.all.borrow().get(self.at)?.integer.get(key).copied()
    }

    fn text(&mut self, key: &str) -> Option<String> {
        self.all.borrow().get(self.at)?.text.get(key).cloned()
    }

    /// Every write asked for is recorded, whether or not it took: a device that
    /// accepts nothing is what a service favjit is not privileged for looks
    /// like, and a script that could not see the attempt could not tell that
    /// apart from a run that never tried.
    fn set_fixed(&mut self, key: &str, value: f64) -> bool {
        let mut all = self.all.borrow_mut();
        let Some(pointer) = all.get_mut(self.at) else {
            return false;
        };
        pointer.written.push((String::from(key), value));
        if !pointer.writable {
            return false;
        }
        pointer.fixed.insert(String::from(key), value);
        true
    }
}

/// The bodies of [`SinkHost`], on the state itself, for the reason
/// [`SinkInputHost`]'s are.
impl Machine {
    fn reach_the_output(
        &mut self,
    ) -> Result<Box<dyn favjit_host::output::OutputHost>, favjit_host::NoOutput> {
        self.note(Did::OpenedOutput);
        if !self.output_comes_up {
            return Err(self.no_output);
        }
        Ok(Box::new(SimOutput {
            frames: core::mem::take(&mut self.output_frames),
            on_the_wire: VecDeque::new(),
            calls: Arc::clone(&self.output_calls),
            refuses_beats: self.output_refuses_beats,
            tears_a_frame: self.output_tears_a_frame,
            tore_one: false,
            delivering: Arc::clone(&self.observed),
            now: Instant::default(),
            bound: Duration::ZERO,
            // Both from the moment it opened: the service starts counting a
            // client's silence when it accepts it, so a run that said nothing
            // from the start is one it lets go of without ever having heard it.
            beat_at: Instant::default(),
            heard_at: Instant::default(),
            closes_at: advance(Instant::default(), self.output_serves_for),
            ended: Arc::clone(&self.output_ended),
        }))
    }

    /// Turned to a standstill before returning, on this thread, for the reason
    /// [`SimHost::run_alongside`] is: the frames that loop delivers go into the
    /// stream before the converting loop reads it either way, and their
    /// timestamps are what put them in order once it does (ADR-0007).
    fn run_output_alongside(&mut self, work: Box<dyn FnOnce() + Send>) -> bool {
        work();
        true
    }

    fn bind_link(&mut self) -> Option<Box<dyn LinkHost + Send>> {
        self.note(Did::BoundLink);
        // Read here rather than handed in: `engine` settles which identity a run
        // listens with before this is called, and by then this machine's own
        // `read`/`write` calls already say which one, so asking again would be a
        // second place that could disagree with the first.
        self.listened_with = self
            .identity_file()
            .as_deref()
            .and_then(Identity::from_bytes);
        if !self.binds_link {
            return None;
        }
        // A machine whose script mentioned no other machine still opens the socket,
        // because that is what a sink with nobody typing on the other end is.
        let link = self
            .link
            .take()
            .unwrap_or_else(|| SimLink::observing(Arc::clone(&self.observed)));
        Some(Box::new(link))
    }

    /// Turned to a standstill before returning, on this thread.
    ///
    /// The alternative is a thread, and then what the suite checked would depend on
    /// which loop a scheduler picked: the link's events go into the stream before
    /// the converter reads it either way, and their timestamps are what put them in
    /// order once it does (ADR-0007).
    fn run_alongside(&mut self, work: Box<dyn FnOnce() + Send>) -> bool {
        self.note(Did::StartedTheLink);
        if !self.starts_alongside {
            return false;
        }
        work();
        // Coming back is the socket no longer being served, unless it came back
        // because the script had nothing more in it. Put on the stream behind
        // what that loop delivered rather than answered now: those events are
        // still ahead of it, and a run told here would end with them unread.
        let ran_out = self.observed().script_ran_out;
        let at = self.cursor;
        let _ = (!ran_out).then(|| {
            self.observed().stream.push_back(HostEvent {
                at,
                kind: EventKind::AlongsideStopped,
            })
        });
        true
    }

    fn send_report(&mut self, report: OutputReport, bytes: &[u8]) -> i32 {
        self.reports.push((self.now, report, bytes.to_vec()));
        self.report_code
    }
}

/// The identity file with no disk under it, and every call that reached it.
///
/// One value rather than fields on each machine, because the open file a run
/// holds writes into the same place the machine answers `read` from: the order
/// of the calls is what a script asserts, and two lists would put the write out
/// of it.
#[derive(Default)]
struct Written {
    calls: Vec<IdentityCall>,
    file: Option<Vec<u8>>,
    /// What was written and stayed, which is what a script asks of the machine
    /// that keeps an identity of its own.
    kept: Option<Vec<u8>>,
}

/// One open identity file, as the run holds it.
struct SimWriting {
    written: Rc<RefCell<Written>>,
    fails_at: Option<IdentityCall>,
    /// Whether this machine is one that keeps what it wrote.
    keeps: bool,
}

impl favjit_host::Writing for SimWriting {
    fn write(&mut self, bytes: &[u8]) -> Result<(), Trouble> {
        let mut written = self.written.borrow_mut();
        written.calls.push(IdentityCall::Wrote);
        refused_at(self.fails_at, IdentityCall::Wrote)?;
        written.file = Some(bytes.to_vec());
        written.kept = self.keeps.then(|| bytes.to_vec());
        Ok(())
    }
}

impl IdentityStore for Machine {
    fn read(&mut self) -> Option<Vec<u8>> {
        let mut written = self.written.borrow_mut();
        written.calls.push(IdentityCall::Read);
        written.file.clone()
    }

    fn make_directory(&mut self) -> Result<(), Trouble> {
        self.written
            .borrow_mut()
            .calls
            .push(IdentityCall::MadeDirectory);
        refused_at(self.identity_fails_at, IdentityCall::MadeDirectory)
    }

    fn open(&mut self) -> Result<Box<dyn favjit_host::Writing>, Trouble> {
        self.written.borrow_mut().calls.push(IdentityCall::Opened);
        refused_at(self.identity_fails_at, IdentityCall::Opened)?;
        Ok(Box::new(SimWriting {
            written: Rc::clone(&self.written),
            fails_at: self.identity_fails_at,
            keeps: true,
        }))
    }
}

impl Entropy for Machine {
    fn fill(&mut self, into: &mut [u8]) -> bool {
        match &self.makes {
            Some(private) if private.len() == into.len() => {
                into.copy_from_slice(private);
                true
            }
            _ => false,
        }
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

/// What a source run asked of its link host, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceCall {
    AskedIfItWasAskedToStop,
    CheckedForASinkToLookFor,
    TriedFixedAddress,
    BoundDiscovery,
    SetDiscoveryTtl,
    StartedDiscoveryClock,
    SentDiscoveryQuestion,
    ReadDiscoveryClock,
    SetDiscoveryTimeout,
    ReceivedDiscoveryAnswer,
    ResolvedDiscoveryAnswer,
    Connected,
    SetNoDelay,
    SetReadTimeout,
    SetWriteTimeout,
    SentFirstMessage,
    FlushedFirstMessage,
    TookAnswer,
}

/// A machine standing in for the one input comes from.
///
/// Separate from [`SimHost`] because the two roles have separate boundaries: a
/// source sends and never injects. Sharing one type would give each role a
/// surface it must never touch.
///
/// A share of its state rather than the state itself, because a run holds boxes
/// this machine handed it — a connection, a search — at the same time as it goes
/// on asking the machine other things: a beat, its own input. Each of those
/// boxes reaches the same state, so every call lands on one list in the run's
/// own order, which is what a test of an order reads. What a test pays for it is
/// that everything read back off a run comes out copied.
#[derive(Default)]
pub struct SimSource(Rc<RefCell<Source>>);

impl SimSource {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn advance(&mut self, by: Duration) -> &mut Self {
        self.0.borrow_mut().advance(by);
        self
    }

    pub fn run_for(&mut self, duration: Duration) -> &mut Self {
        self.0.borrow_mut().run_for(duration);
        self
    }

    pub fn attach_at(&mut self, device: DeviceId, path: &str) -> &mut Self {
        self.0.borrow_mut().attach_at(device, path);
        self
    }

    pub fn attach_external(&mut self, id: DeviceId, vendor_id: u16, product_id: u16) -> &mut Self {
        self.0
            .borrow_mut()
            .attach_external(id, vendor_id, product_id);
        self
    }

    pub fn attach_anonymous(&mut self, id: DeviceId) -> &mut Self {
        self.0.borrow_mut().attach_anonymous(id);
        self
    }

    pub fn detach(&mut self, id: DeviceId) -> &mut Self {
        self.0.borrow_mut().detach(id);
        self
    }

    pub fn press(&mut self, device: DeviceId, key: Key) -> &mut Self {
        self.0.borrow_mut().press(device, key);
        self
    }

    pub fn release(&mut self, device: DeviceId, key: Key) -> &mut Self {
        self.0.borrow_mut().release(device, key);
        self
    }

    pub fn tap(&mut self, device: DeviceId, key: Key) -> &mut Self {
        self.0.borrow_mut().tap(device, key);
        self
    }

    pub fn pointer(&mut self, device: DeviceId, report: PointerReport) -> &mut Self {
        self.0.borrow_mut().pointer(device, report);
        self
    }

    pub fn probe(&mut self) -> &mut Self {
        self.0.borrow_mut().probe();
        self
    }

    pub fn asked_for(&mut self, driving: Driving) -> &mut Self {
        self.0.borrow_mut().asked_for(driving);
        self
    }

    pub fn sink_missing(&mut self, times: usize) -> &mut Self {
        self.0.borrow_mut().sink_missing(times);
        self
    }

    pub fn socket_refused(&mut self, times: usize) -> &mut Self {
        self.0.borrow_mut().socket_refused(times);
        self
    }

    pub fn handshake_dropped(&mut self, times: usize) -> &mut Self {
        self.0.borrow_mut().handshake_dropped(times);
        self
    }

    pub fn handshake_flush_dropped(&mut self, times: usize) -> &mut Self {
        self.0.borrow_mut().handshake_flush_dropped(times);
        self
    }

    pub fn answer_dropped(&mut self, times: usize) -> &mut Self {
        self.0.borrow_mut().answer_dropped(times);
        self
    }

    pub fn whose_link_fails_at(&mut self, call: SourceCall) -> &mut Self {
        self.0.borrow_mut().whose_link_fails_at(call);
        self
    }

    pub fn source_calls(&self) -> Vec<SourceCall> {
        self.0.borrow().source_calls()
    }

    pub fn on_a_network(&mut self, network: SimDiscovery) -> &mut Self {
        self.0.borrow_mut().on_a_network(network);
        self
    }

    pub fn found(&self) -> Option<Found> {
        self.0.borrow().found().cloned()
    }

    pub fn network(&self) -> SimDiscovery {
        self.0.borrow().network().clone()
    }

    pub fn pauses(&self) -> usize {
        self.0.borrow().pauses()
    }

    pub fn cannot_read_input(&mut self) -> &mut Self {
        self.0.borrow_mut().cannot_read_input();
        self
    }

    pub fn stopped_reading(&mut self) -> &mut Self {
        self.0.borrow_mut().stopped_reading();
        self
    }

    pub fn no_sink(&mut self) -> &mut Self {
        self.0.borrow_mut().no_sink();
        self
    }

    pub fn pinned_to(&mut self, sink: Identity) -> &mut Self {
        self.0.borrow_mut().pinned_to(sink);
        self
    }

    pub fn with_identity_file(&mut self, bytes: &[u8]) -> &mut Self {
        self.0.borrow_mut().with_identity_file(bytes);
        self
    }

    pub fn whose_identity_fails_at(&mut self, call: IdentityCall) -> &mut Self {
        self.0.borrow_mut().whose_identity_fails_at(call);
        self
    }

    pub fn identity_calls(&self) -> Vec<IdentityCall> {
        self.0.borrow().identity_calls()
    }

    pub fn with_pinned_sink_text(&mut self, text: &str) -> &mut Self {
        self.0.borrow_mut().with_pinned_sink_text(text);
        self
    }

    pub fn may_suppress(&self) -> bool {
        self.0.borrow().may_suppress()
    }

    pub fn with_chord_at(&mut self, to_the_sink: (u16, bool), back_here: (u16, bool)) -> &mut Self {
        self.0.borrow_mut().with_chord_at(to_the_sink, back_here);
        self
    }

    pub fn with_the_modifier_down(&mut self, down: bool) -> &mut Self {
        self.0.borrow_mut().with_the_modifier_down(down);
        self
    }

    pub fn refuse(&mut self, what: Suppressing) -> &mut Self {
        self.0.borrow_mut().refuse(what);
        self
    }

    pub fn with_nowhere_for_keys(&mut self) -> &mut Self {
        self.0.borrow_mut().with_nowhere_for_keys();
        self
    }

    pub fn present(&self, arrived: Arrived) -> Option<Answered> {
        self.0.borrow().present(arrived)
    }

    pub fn connects(&self) -> usize {
        self.0.borrow().connects()
    }

    pub fn suppressing(&self) -> Suppressing {
        self.0.borrow().suppressing()
    }

    pub fn keyboards_taken(&self) -> bool {
        self.0.borrow().keyboards_taken()
    }

    pub fn refusals(&self) -> Vec<Suppressing> {
        self.0.borrow().refusals().to_vec()
    }

    pub fn shown(&self) -> Vec<Suppressing> {
        self.0.borrow().shown().to_vec()
    }

    pub fn with_nowhere_to_show_it(&mut self) -> &mut Self {
        self.0.borrow_mut().with_nowhere_to_show_it();
        self
    }

    pub fn suppressions(&self) -> usize {
        self.0.borrow().suppressions()
    }

    pub fn suppressed_before_connecting(&self) -> usize {
        self.0.borrow().suppressed_before_connecting()
    }

    pub fn link_gone(&mut self) -> &mut Self {
        self.0.borrow_mut().link_gone();
        self
    }

    pub fn answers_a_send_with(&mut self, code: i32) -> &mut Self {
        self.0.borrow_mut().answers_a_send_with(code);
        self
    }

    pub fn link_back(&mut self) -> &mut Self {
        self.0.borrow_mut().link_back();
        self
    }

    pub fn script(&mut self, kind: EventKind) -> &mut Self {
        self.0.borrow_mut().script(kind);
        self
    }

    pub fn sent(&self) -> Vec<Sent> {
        self.0.borrow().sent().to_vec()
    }

    pub fn heartbeats(&self) -> Vec<Instant> {
        self.0.borrow().heartbeats().to_vec()
    }

    pub fn with_no_watchdog(self) -> Self {
        self.0.borrow_mut().no_watchdog();
        self
    }

    pub fn warnings(&self) -> Vec<String> {
        self.0.borrow().warnings().to_vec()
    }

    pub fn warned_before_taking_input(&self) -> bool {
        self.0.borrow().warned_before_taking_input()
    }
}

impl SourceHost for SimSource {
    fn pinned_sink(&mut self) -> Option<String> {
        self.0.borrow_mut().pinned_sink()
    }

    fn answer_procedures_with(&mut self, answering: Answering) {
        self.0.borrow_mut().answer_procedures_with(answering);
    }

    fn there_is_somewhere_to_show_it(&mut self) -> bool {
        self.0.borrow_mut().there_is_somewhere_to_show_it()
    }

    fn show_what_is_refused(&mut self, what: Suppressing) {
        self.0.borrow_mut().show_what_is_refused(what);
    }

    fn take_input(
        &mut self,
        probe_tick: Duration,
        raw_bytes: usize,
        work: favjit_host::capture::SourceLoop,
    ) -> bool {
        self.0.borrow_mut().take_input(probe_tick, raw_bytes, work)
    }

    fn pause(&mut self, how_long: Duration) {
        self.0.borrow_mut().pause(how_long);
    }

    fn asked_to_stop(&mut self) -> bool {
        self.0.borrow_mut().asked_to_stop()
    }

    fn has_a_sink_to_look_for(&mut self) -> bool {
        self.0.borrow_mut().has_a_sink_to_look_for()
    }

    fn use_fixed_sink(&mut self) -> Option<favjit_host::Sink> {
        self.0.borrow_mut().use_fixed_sink()
    }

    fn connect_socket(
        &mut self,
        _sink: favjit_host::Sink,
        _timeout: Duration,
    ) -> Option<Box<dyn Opening>> {
        let connected = self.0.borrow_mut().connect();
        match connected {
            true => Some(Box::new(SimOpen(Rc::clone(&self.0)))),
            false => None,
        }
    }

    fn suppress(&mut self, what: Suppressing) {
        self.0.borrow_mut().suppress(what);
    }
}

impl IdentityStore for SimSource {
    fn read(&mut self) -> Option<Vec<u8>> {
        self.0.borrow_mut().read()
    }

    fn make_directory(&mut self) -> Result<(), Trouble> {
        self.0.borrow_mut().make_directory()
    }

    fn open(&mut self) -> Result<Box<dyn favjit_host::Writing>, Trouble> {
        self.0.borrow_mut().open()
    }
}

impl Entropy for SimSource {
    fn fill(&mut self, into: &mut [u8]) -> bool {
        self.0.borrow_mut().fill(into)
    }
}

impl Host for SimSource {
    fn now(&mut self) -> Instant {
        self.0.borrow_mut().now()
    }

    fn next_event(&mut self, deadline: Instant) -> Option<HostEvent> {
        self.0.borrow_mut().next_event(deadline)
    }

    fn is_supervised(&mut self) -> bool {
        self.0.borrow_mut().is_supervised()
    }

    fn warn(&mut self, message: core::fmt::Arguments) {
        self.0.borrow_mut().warn(message);
    }

    fn heartbeat(&mut self) -> Result<(), Trouble> {
        self.0.borrow_mut().heartbeat()
    }
}

/// Everything one of those machines holds.
struct Source {
    cursor: Instant,
    now: Instant,
    inbound: VecDeque<HostEvent>,
    /// The buttons the last [`SimSource::pointer`] call for a device reported,
    /// for the same reason [`SimHost::buttons`] keeps one: a raw report carries
    /// a transition, not the state.
    buttons: Vec<(DeviceId, Buttons)>,
    sent: Vec<Sent>,
    heartbeats: Vec<Instant>,
    /// How many events had been scripted at each point the link went away or came
    /// back, in the order they were scripted.
    ///
    /// A list and not one mark each, because a link that drops twice in a run is an
    /// ordinary thing for one to do and the second drop is where a run that says
    /// something once per run rather than once per drop shows itself. They alternate,
    /// so how many are behind the events handed over is what says which state the link
    /// is in.
    link_changes: Vec<usize>,
    /// What a send answers with once the link has gone, for the same reason
    /// [`SimHost::report_code`] is scripted: the number is what a trace is read
    /// for, and the two failures a person has to tell apart — a write that ran
    /// out of time and a connection the other end reset — differ only in it.
    send_code: i32,
    /// How many have been handed over so far, which is what that is compared to.
    taken: usize,
    /// How many times the other machine is not there yet.
    missing: usize,
    /// How many attempts at the connection and at the handshake's two halves the
    /// other machine drops before it starts answering.
    socket_refused: usize,
    handshake_dropped: usize,
    handshake_flush_dropped: usize,
    answer_dropped: usize,
    source_calls: Vec<SourceCall>,
    source_fails_at: Option<SourceCall>,
    /// Whether the last answer was a link.
    linked: bool,
    /// The sink this machine is pinned to. Standing in for it is this end's own
    /// job during the handshake, the way a real one only exists on the other
    /// machine: without a matching identity there is nothing for the handshake
    /// `engine` drives to actually open.
    sink_identity: Option<Identity>,
    /// The answer the fake sink produced, waiting to be taken.
    pending_answer: Option<[u8; ANSWER]>,
    /// The session once the handshake completes, for reading what arrives.
    session: Option<Session>,
    /// A different value handed out each time `Entropy::fill` is asked, so
    /// repeated ephemeral keys within one run are never identical.
    entropy_calls: u8,
    connects: usize,
    /// The clock the run's own bound is on, apart from the input stream's
    /// timestamps: waiting for the network moves this even when no input arrives.
    run_now: Instant,
    /// When this run was asked to stop, if it was bounded.
    until: Option<Instant>,
    /// Whether a configured address bypasses discovery.
    fixed_sink: bool,
    /// What this machine's own input is refused for, and how many times it was ever
    /// refused outright — which, with the count below, is what says nothing was taken
    /// too early.
    suppressing: Suppressing,
    suppressions: usize,
    suppressed_before_connecting: usize,
    /// Everything the run asked for, in order.
    ///
    /// Kept because the order is the behaviour and the final state is not: a run
    /// releases everything as it ends, so what it did while it was going is only
    /// visible as a sequence.
    refused: Vec<Suppressing>,
    /// Whether this machine's keyboards can be read at all.
    ///
    /// True by default, since a machine that cannot be read is the exception a
    /// test asks for by name.
    reads_input: bool,
    /// What the run was told it may do, so a test can check that a run which will
    /// never refuse did not ask to be able to.
    may_suppress: bool,
    /// What the run answers a procedure the platform calls with.
    ///
    /// Kept so a test can present one event to it and read the answer: on a real
    /// platform this is reached from a bare function pointer, and the whole of
    /// what a hook does is in the answer rather than in the procedure.
    answering: Option<Answering>,
    /// What a person on this machine has been shown is refused, in order.
    ///
    /// The order and not the last of them, because what a person sees while the
    /// keyboard moves is the sequence: a machine shown as refusing everything a
    /// moment before it did is what this catches.
    shown: Vec<Suppressing>,
    /// Whether this machine has anywhere to show it at all.
    shows_it_somewhere: bool,
    /// The chord's two positions as the run published them, which is what it
    /// compares an arriving key against.
    chord: favjit_host::source::Chord,
    /// Whether there is anywhere for a key to be handed to, as a platform that
    /// has not opened one yet would answer.
    keys_go_somewhere: bool,
    /// Whether the modifier the chord needs is down, as the machine would answer.
    modifier_is_down: bool,
    /// The bits the run keeps on this machine for what it was let see go down and
    /// not yet go up, one per bit the run may name.
    ///
    /// Shared across the calls rather than made afresh for each: what one answer
    /// sets is what a later one reads, and a call handed clear bits would never
    /// find a key held.
    held_here: Rc<core::cell::RefCell<Vec<bool>>>,
    supervised: bool,
    /// The lines the run put into this machine's log, in order.
    warnings: Vec<String>,
    /// Whether anything had been said by the time the keyboards were asked for,
    /// which is what says a cost was named while the person could still act on it.
    warned_before_taking_input: bool,
    /// Whether reading this machine's keyboards stopped part way, rather than
    /// the run having been asked to end.
    reading_stopped: bool,
    /// Whether the run has read this machine's stream dry.
    stream_ran_out: bool,
    /// Whether there is a sink to relay to at all.
    no_sink: bool,
    /// How many times the run waited before looking again.
    pauses: usize,
    /// The network to look for the sink over, when a test scripted one.
    network: Option<SimDiscovery>,
    /// What the last look found, so a test can read it off a whole run.
    found: Option<Found>,
    /// Whether this machine's supervisor is still reading the beats, the way
    /// [`SimHost`]'s answers for the other machine's.
    beats_arrive: bool,
    /// This machine's own identity file, the way [`SimHost`]'s stands in for the
    /// sink's.
    ///
    /// Shared with the open file [`IdentityStore::open`] hands over, for the
    /// reason [`SimHost::written`] is.
    written: Rc<RefCell<Written>>,
    identity_fails_at: Option<IdentityCall>,
    /// The text of the file naming the one machine this one will relay to, read
    /// by [`SourceHost::pinned_sink`]. [`SimSource::pinned_to`] keeps it in
    /// step with [`Self::sink_identity`], since a real pairing writes down the
    /// same key it will later answer the handshake with.
    pinned_sink_text: Option<String>,
}

impl Default for Source {
    /// A machine whose keyboards can be read, with a sink to relay to, ending
    /// because it was asked to — the ordinary case, so that a test names only the
    /// thing it is about.
    fn default() -> Self {
        Self {
            // `-1` and not a platform's number: no machine answered a call this
            // double never made, which is what that stands for everywhere
            // (ADR-0009). A test after one particular failure scripts it.
            send_code: -1,
            cursor: Instant::default(),
            now: Instant::default(),
            inbound: VecDeque::new(),
            buttons: Vec::new(),
            sent: Vec::new(),
            heartbeats: Vec::new(),
            link_changes: Vec::new(),
            taken: 0,
            missing: 0,
            socket_refused: 0,
            handshake_dropped: 0,
            handshake_flush_dropped: 0,
            answer_dropped: 0,
            source_calls: Vec::new(),
            source_fails_at: None,
            linked: false,
            sink_identity: None,
            pending_answer: None,
            session: None,
            entropy_calls: 0,
            connects: 0,
            run_now: Instant::default(),
            until: None,
            fixed_sink: true,
            suppressing: Suppressing::Nothing,
            refused: Vec::new(),
            suppressions: 0,
            suppressed_before_connecting: 0,
            reads_input: true,
            may_suppress: false,
            answering: None,
            shown: Vec::new(),
            shows_it_somewhere: true,
            chord: (None, None),
            keys_go_somewhere: true,
            modifier_is_down: false,
            held_here: Rc::new(core::cell::RefCell::new(vec![
                false;
                favjit_host::source::HELD_HERE_BITS
            ])),
            supervised: true,
            warnings: Vec::new(),
            warned_before_taking_input: false,
            reading_stopped: false,
            stream_ran_out: false,
            no_sink: false,
            pauses: 0,
            network: None,
            found: None,
            beats_arrive: true,
            written: Rc::default(),
            identity_fails_at: None,
            pinned_sink_text: None,
        }
    }
}

impl Source {
    pub fn advance(&mut self, by: Duration) -> &mut Self {
        self.cursor = advance(self.cursor, by);
        self
    }

    /// Stop this run once this much of its own monotonic time has passed.
    ///
    /// This is the simulated form of the deadline supplied by `--seconds`: it is
    /// independent of how much input or how many network answers a test scripts.
    pub fn run_for(&mut self, duration: Duration) -> &mut Self {
        self.until = Some(advance(self.run_now, duration));
        self
    }

    /// Announce a device at this interface path, the way raw input names one.
    ///
    /// The path and not what a run reads out of it: what a vendor and a product
    /// look like written into one is a format, and reading it is the run's
    /// (ADR-0006).
    pub fn attach_at(&mut self, device: DeviceId, path: &str) -> &mut Self {
        self.script(EventKind::PathDeviceFound {
            device,
            path: String::from(path),
        })
    }

    /// Announce a keyboard named by USB identity, at the path Windows writes one
    /// into.
    ///
    /// **There is no built-in counterpart**, unlike [`SimHost`]'s: nothing this
    /// machine reports claims to be the Mac's own keyboard, whatever hardware it
    /// is, because that flag selects the layers Dudrack puts on the machine
    /// being typed *into*.
    pub fn attach_external(&mut self, id: DeviceId, vendor_id: u16, product_id: u16) -> &mut Self {
        self.attach_at(
            id,
            &format!(
                r"\\?\HID#VID_{vendor_id:04X}&PID_{product_id:04X}&MI_01#8&1e0b8ad9&0&0000#{{884b96c3-56ef-11d1-bc8c-00a0c91405dd}}"
            ),
        )
    }

    /// Announce a keyboard that is not on a USB bus, so no rule can single it
    /// out — a laptop's own, which raw input puts behind `ACPI#PNP0303`.
    pub fn attach_anonymous(&mut self, id: DeviceId) -> &mut Self {
        self.attach_at(
            id,
            r"\\?\ACPI#PNP0303#4&1cf8b0e6&0#{884b96c3-56ef-11d1-bc8c-00a0c91405dd}",
        )
    }

    pub fn detach(&mut self, id: DeviceId) -> &mut Self {
        self.script(EventKind::DeviceLost(id))
    }

    pub fn press(&mut self, device: DeviceId, key: Key) -> &mut Self {
        self.key(device, key, false)
    }

    pub fn release(&mut self, device: DeviceId, key: Key) -> &mut Self {
        self.key(device, key, true)
    }

    pub fn tap(&mut self, device: DeviceId, key: Key) -> &mut Self {
        self.press(device, key).release(device, key)
    }

    /// One `KBDLLHOOKSTRUCT` for `key`, the level a low-level hook reports a
    /// Windows keyboard at — `engine`'s own resolver is what reads it, the same
    /// as a real capture's.
    fn key(&mut self, device: DeviceId, key: Key, up: bool) -> &mut Self {
        let (make_code, extended) =
            favjit_hid::scancode::as_a_hook_reports(key).unwrap_or_else(|| {
                panic!("{key:?} has no Windows position a script could report it at")
            });
        let mut flags = 0;
        if extended {
            flags |= favjit_hid::scancode::LLKHF_EXTENDED;
        }
        if up {
            flags |= favjit_hid::scancode::LLKHF_UP;
        }
        self.script(EventKind::HookedKey {
            device,
            make_code,
            vkey: virtual_key(key),
            flags,
        })
    }

    /// One pointer report from a device, as `RAWMOUSE` would carry it —
    /// [`favjit_hid::rawmouse::encode`] runs the same arithmetic a real capture's
    /// decoder is the inverse of, so what a script asserts about a relayed
    /// report is read by the resolver a real one would be.
    pub fn pointer(&mut self, device: DeviceId, report: PointerReport) -> &mut Self {
        let held = self.buttons(device);
        for raw in favjit_hid::rawmouse::encode(held, report) {
            self.script(EventKind::MouseReport {
                device,
                flags: raw.flags,
                button_flags: raw.button_flags,
                button_data: raw.button_data,
                dx: raw.dx,
                dy: raw.dy,
            });
        }
        self.set_buttons(device, report.buttons);
        self
    }

    fn buttons(&self, device: DeviceId) -> Buttons {
        self.buttons
            .iter()
            .find(|(id, _)| *id == device)
            .map_or(Buttons::NONE, |(_, buttons)| *buttons)
    }

    fn set_buttons(&mut self, device: DeviceId, buttons: Buttons) {
        match self.buttons.iter_mut().find(|(id, _)| *id == device) {
            Some((_, held)) => *held = buttons,
            None => self.buttons.push((device, buttons)),
        }
    }

    /// Ask, as this machine's own watchdog would, whether the loop is turning.
    pub fn probe(&mut self) -> &mut Self {
        self.script(EventKind::Probe)
    }

    /// Ask for the keyboard to be driving this machine or the other one, from here on.
    ///
    /// The way the tray item asks and the way a terminal does (`docs/platform/windows/tray-item-as-its-own-program.md`), which is one
    /// event: what those two have in common is the ask, and a run comes up with the
    /// keyboard on the machine it is running on, so a script about what crosses the link
    /// starts here. Which of the two routes was used — this, or the chord as its four key
    /// events — is `switching.rs`'s subject and nothing else's.
    pub fn asked_for(&mut self, driving: Driving) -> &mut Self {
        self.script(EventKind::Asked(driving))
    }

    /// The other machine is not there for this many attempts.
    pub fn sink_missing(&mut self, times: usize) -> &mut Self {
        self.missing = times;
        self
    }

    /// A sink that answers mDNS and then will not take the connection, this many
    /// attempts running.
    ///
    /// Counted down rather than set for good, because what a run does about it is
    /// look again: a machine that refused for ever would be a script the run can
    /// only spin against, where a real one is either coming back or gone.
    pub fn socket_refused(&mut self, times: usize) -> &mut Self {
        self.socket_refused = times;
        self
    }

    /// A sink that takes the connection and then goes before the handshake's own
    /// first message reaches it.
    pub fn handshake_dropped(&mut self, times: usize) -> &mut Self {
        self.handshake_dropped = times;
        self
    }

    /// The write completed and flushing it failed before an answer was read.
    pub fn handshake_flush_dropped(&mut self, times: usize) -> &mut Self {
        self.handshake_flush_dropped = times;
        self
    }

    /// The same one message later: the first message got there and no answer came
    /// back.
    pub fn answer_dropped(&mut self, times: usize) -> &mut Self {
        self.answer_dropped = times;
        self
    }

    pub fn whose_link_fails_at(&mut self, call: SourceCall) -> &mut Self {
        self.source_fails_at = Some(call);
        self
    }

    pub fn source_calls(&self) -> Vec<SourceCall> {
        self.source_calls.clone()
    }

    fn source_call(&mut self, call: SourceCall) -> bool {
        self.source_calls.push(call);
        match self.source_fails_at == Some(call) {
            true => {
                self.source_fails_at = None;
                false
            }
            false => true,
        }
    }

    /// Beside [`Self::source_call`] and not folded into it, because a call that
    /// answers something other than whether it worked has no way to report the
    /// scripted failure, and taking the script's one failure while answering
    /// anyway would spend it on a call that cannot fail.
    fn record(&mut self, call: SourceCall) {
        self.source_calls.push(call);
    }

    /// The local network this machine looks for the sink over.
    ///
    /// Scripting one removes the configured-address shortcut, so a whole run has to
    /// drive every discovery operation before it can connect.
    pub fn on_a_network(&mut self, network: SimDiscovery) -> &mut Self {
        self.fixed_sink = false;
        self.network = Some(network);
        self
    }

    /// What the discovery sequence passed to the resolver, if any.
    pub fn found(&self) -> Option<&Found> {
        self.found.as_ref()
    }

    /// The network as the run left it, for reading what was asked of it.
    pub fn network(&self) -> &SimDiscovery {
        self.network.as_ref().expect("a network was scripted")
    }

    /// How many times the run waited before looking again.
    pub fn pauses(&self) -> usize {
        self.pauses
    }

    /// This machine's keyboards cannot be read at all.
    pub fn cannot_read_input(&mut self) -> &mut Self {
        self.reads_input = false;
        self
    }

    /// Reading the keyboards stopped, rather than the run being asked to end.
    ///
    /// Said rather than inferred from a script that ran out: a capture that died
    /// and a run that reached its bound both leave a stream with nothing on it,
    /// and which of the two happened is only ever this machine's to answer.
    pub fn stopped_reading(&mut self) -> &mut Self {
        self.reading_stopped = true;
        self
    }

    /// There is nothing to relay to and there will not be.
    pub fn no_sink(&mut self) -> &mut Self {
        self.no_sink = true;
        self
    }

    /// Pin this machine to the sink whose identity this is, as `favjit --pair`
    /// would. Its private half is what this end answers the handshake with —
    /// standing in for the sink is this machine's own job, so without a
    /// matching identity there is nothing for the handshake to open.
    ///
    /// Also what [`SourceHost::pinned_sink`] reads back, in the same digits a
    /// real pairing would have written down: the two are one fact, and setting
    /// them apart would let a script hand the handshake a key it never claimed
    /// to have pinned.
    pub fn pinned_to(&mut self, sink: Identity) -> &mut Self {
        self.pinned_sink_text = Some(format!("{}\n", sink.fingerprint()));
        self.sink_identity = Some(sink);
        self
    }

    /// This machine's own identity file holds these bytes already, the way
    /// [`SimHost::with_identity_file`] stands in for the sink's.
    pub fn with_identity_file(&mut self, bytes: &[u8]) -> &mut Self {
        self.written.borrow_mut().file = Some(bytes.to_vec());
        self
    }

    /// The platform call named fails when [`SourceHost::pinned_sink`] or
    /// [`favjit_host::IdentityStore`]'s own reach it, the way
    /// [`SimHost::whose_identity_fails_at`] scripts the sink's.
    pub fn whose_identity_fails_at(&mut self, call: IdentityCall) -> &mut Self {
        self.identity_fails_at = Some(call);
        self
    }

    /// What this machine's identity store was asked, in order.
    pub fn identity_calls(&self) -> Vec<IdentityCall> {
        self.written.borrow().calls.clone()
    }

    /// The pinned-sink file names this text, in place of whatever
    /// [`SimSource::pinned_to`] would otherwise have written — for a script
    /// about the file's content rather than about a pairing that produced it.
    pub fn with_pinned_sink_text(&mut self, text: &str) -> &mut Self {
        self.pinned_sink_text = Some(text.to_string());
        self
    }

    /// Whether the run said it might want to refuse input.
    ///
    /// Kept because a run that will never refuse must not ask to be able to: on a
    /// real machine being able means a hook on the whole of it.
    pub fn may_suppress(&self) -> bool {
        self.may_suppress
    }

    /// The positions the chord's two keys are at on this machine's keyboard.
    pub fn with_chord_at(&mut self, to_the_sink: (u16, bool), back_here: (u16, bool)) -> &mut Self {
        self.chord = (Some(to_the_sink), Some(back_here));
        self
    }

    /// Whether the modifier the chord needs is down, as this machine answers.
    pub fn with_the_modifier_down(&mut self, down: bool) -> &mut Self {
        self.modifier_is_down = down;
        self
    }

    /// What this machine refuses as things stand, as the run would have
    /// published it.
    ///
    /// Set rather than reached through a run, because what the run answers a
    /// procedure with reads this and the run only publishes it at the moments its
    /// own loop decides to: a script driving every state through those moments
    /// would be a script about the loop and not about the answer.
    pub fn refuse(&mut self, what: Suppressing) -> &mut Self {
        self.suppressing = what;
        self
    }

    /// Whether there is anywhere on this machine for a key to be handed to.
    pub fn with_nowhere_for_keys(&mut self) -> &mut Self {
        self.keys_go_somewhere = false;
        self
    }

    /// Hand one event to whatever the run answers a procedure with, and say what
    /// it did.
    ///
    /// The only way this can be driven at all: on a real platform the procedure
    /// is called by the OS on the thread the event arrived on, so nothing but the
    /// answer is reachable from a test (ADR-0006, ADR-0007).
    ///
    /// Nothing where no run has installed an answer, which is a machine that was
    /// never asked to read its keyboards.
    pub fn present(&self, arrived: Arrived) -> Option<Answered> {
        let answering = self.answering?;
        let asked = Asked {
            refusing: self.suppressing,
            chord: self.chord,
            keys_go_somewhere: self.keys_go_somewhere,
            modifier_is_down: self.modifier_is_down,
            held_here: Rc::clone(&self.held_here),
            answered: core::cell::RefCell::new(Answered::default()),
        };
        let told = answering(arrived, &asked);
        let mut answered = asked.answered.into_inner();
        answered.told = told;
        Some(answered)
    }

    /// How many times a link was asked for.
    pub fn connects(&self) -> usize {
        self.connects
    }

    /// What this machine's own input is refused for as things stand.
    pub fn suppressing(&self) -> Suppressing {
        self.suppressing
    }

    /// Whether the keyboards are taken outright as things stand.
    pub fn keyboards_taken(&self) -> bool {
        self.suppressing == Suppressing::Everything
    }

    /// What the run asked this machine to refuse, in order.
    pub fn refusals(&self) -> &[Suppressing] {
        &self.refused
    }

    /// What a person on this machine was shown is refused, in order.
    pub fn shown(&self) -> &[Suppressing] {
        &self.shown
    }

    /// This machine, with nowhere to show what is refused.
    pub fn with_nowhere_to_show_it(&mut self) -> &mut Self {
        self.shows_it_somewhere = false;
        self
    }

    /// How many times the keyboards were taken outright.
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
        self.link_changes.push(self.inbound.len());
        self
    }

    /// What a send answers with while the link is gone.
    ///
    /// Set for the run rather than per drop, because what a test built on it says
    /// is what a run does with one particular number: two runs of the same script
    /// under two numbers is how the difference between them is pinned, and a
    /// script that changed the number partway would be one test asserting both.
    pub fn answers_a_send_with(&mut self, code: i32) -> &mut Self {
        self.send_code = code;
        self
    }

    /// The link is back, for everything scripted after this.
    ///
    /// A separate point in the script rather than the drop lasting to the end of
    /// it, because a source is expected to carry on afterwards: what it says over
    /// the new link is where the difference between remembering and forgetting the
    /// old one shows up, and a drop with nothing after it cannot show that.
    pub fn link_back(&mut self) -> &mut Self {
        self.link_changes.push(self.inbound.len());
        self
    }

    pub fn script(&mut self, kind: EventKind) -> &mut Self {
        self.inbound.push_back(HostEvent {
            at: self.cursor,
            kind,
        });
        self
    }

    /// Everything that went to the link, in order.
    pub fn sent(&self) -> &[Sent] {
        &self.sent
    }

    pub fn heartbeats(&self) -> &[Instant] {
        &self.heartbeats
    }

    /// A machine where nothing is watching this process, so a wedge would keep the
    /// keyboards refused (ADR-0008).
    fn no_watchdog(&mut self) {
        self.supervised = false;
    }

    /// What the run said about itself, in order.
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    /// Whether anything had been said by the time the keyboards were asked for.
    pub fn warned_before_taking_input(&self) -> bool {
        self.warned_before_taking_input
    }
}

/// One search, as the run holds it.
///
/// A share of the machine it was opened on rather than a copy of its state: what
/// a script asserts is the order the whole run's calls came in, and a search
/// keeping its own list would put its own out of that order.
struct SimSearch(Rc<RefCell<Source>>);

impl favjit_host::Searching for SimSearch {
    fn set_ttl(&mut self, _ttl: u32) -> bool {
        self.0.borrow_mut().source_call(SourceCall::SetDiscoveryTtl)
    }

    fn start_clock(&mut self) -> Instant {
        let mut source = self.0.borrow_mut();
        source.record(SourceCall::StartedDiscoveryClock);
        let Some(network) = source.network.as_mut() else {
            return Instant::default();
        };
        network.start_clock()
    }

    fn now(&mut self) -> Instant {
        let mut source = self.0.borrow_mut();
        source.record(SourceCall::ReadDiscoveryClock);
        source
            .network
            .as_mut()
            .map_or(Instant::default(), SimDiscovery::now)
    }

    fn ask(&mut self, question: &[u8]) -> bool {
        let mut source = self.0.borrow_mut();
        if !source.source_call(SourceCall::SentDiscoveryQuestion) {
            return false;
        }
        source
            .network
            .as_mut()
            .is_some_and(|network| network.ask(question))
    }

    fn set_timeout(&mut self, timeout: Duration) -> bool {
        let mut source = self.0.borrow_mut();
        if !source.source_call(SourceCall::SetDiscoveryTimeout) {
            return false;
        }
        source
            .network
            .as_mut()
            .is_some_and(|network| network.set_timeout(timeout))
    }

    /// The wait a receive took is what moves this machine's clock, so a script
    /// that answers late is one whose run has spent that long looking.
    fn receive(&mut self, into: &mut [u8]) -> Datagram {
        let mut source = self.0.borrow_mut();
        if !source.source_call(SourceCall::ReceivedDiscoveryAnswer) {
            return Datagram::Failed;
        }
        let Some(network) = source.network.as_mut() else {
            return Datagram::Failed;
        };
        let before = network.now();
        let received = network.receive(into);
        let after = network.now();
        source.run_now = advance(
            source.run_now,
            Duration::from_nanos(after.nanos.saturating_sub(before.nanos)),
        );
        received
    }
}

impl DiscoveryHost for SimSource {
    fn bind_discovery(&mut self) -> Option<Box<dyn favjit_host::Searching + '_>> {
        let bound = self.0.borrow_mut().source_call(SourceCall::BoundDiscovery);
        match bound {
            true => Some(Box::new(SimSearch(Rc::clone(&self.0)))),
            false => None,
        }
    }

    fn resolve_discovered(
        &mut self,
        host: &str,
        port: u16,
        address: Option<[u8; 4]>,
    ) -> Option<favjit_host::Sink> {
        self.0.borrow_mut().resolve_discovered(host, port, address)
    }
}

impl Source {
    fn resolve_discovered(
        &mut self,
        host: &str,
        port: u16,
        address: Option<[u8; 4]>,
    ) -> Option<favjit_host::Sink> {
        if !self.source_call(SourceCall::ResolvedDiscoveryAnswer) {
            return None;
        }
        self.found = Some(Found {
            host: host.to_string(),
            port,
            address,
        });
        Some(SOMEWHERE)
    }
}

/// Where a scripted machine says the sink is.
///
/// One address for every run, because nothing here opens a socket to it: what a
/// test reads is [`SimSource::found`], and an address that varied would be a
/// second fact saying the same thing.
const SOMEWHERE: favjit_host::Sink = favjit_host::Sink::new(
    std::net::IpAddr::V4(std::net::Ipv4Addr::new(127, 0, 0, 1)),
    9000,
);

/// What one event handed to the run's answer produced.
///
/// Every call the answer could have made, whether or not it made it: a test
/// asserting on what a hook did needs "handed over and refused" told apart from
/// "refused without being handed over", and a record of only what happened would
/// leave the second reading as the first with the key missing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Answered {
    /// The key the answer handed to the capture loop, where it handed one.
    pub handed_over: Option<(usize, i64)>,
    /// Whether the answer ended the event here rather than letting it carry on.
    pub ended_here: bool,
    /// Whether the answer let it carry on to whatever is downstream.
    pub carried_on: bool,
    /// How many pointer events the answer counted as refused.
    pub pointers_refused: usize,
    /// The number the platform is told, as this machine's own numbering has it.
    pub told: isize,
}

/// This machine, as one call into a procedure sees it.
struct Asked {
    refusing: Suppressing,
    chord: favjit_host::source::Chord,
    keys_go_somewhere: bool,
    modifier_is_down: bool,
    held_here: Rc<core::cell::RefCell<Vec<bool>>>,
    answered: core::cell::RefCell<Answered>,
}

impl Answers for Asked {
    fn refusing(&self) -> Suppressing {
        self.refusing
    }

    fn the_chord(&self) -> favjit_host::source::Chord {
        self.chord
    }

    fn keys_go_somewhere(&self) -> bool {
        self.keys_go_somewhere
    }

    fn hand_the_key_over(&self, packed: usize, flags: i64) -> bool {
        self.answered.borrow_mut().handed_over = Some((packed, flags));
        true
    }

    fn the_modifier_is_down(&self) -> bool {
        self.modifier_is_down
    }

    fn held_here(&self, bit: usize) -> bool {
        self.held_here.borrow()[bit]
    }

    fn now_held_here(&self, bit: usize) {
        self.held_here.borrow_mut()[bit] = true;
    }

    fn let_go_of_here(&self, bit: usize) {
        self.held_here.borrow_mut()[bit] = false;
    }

    fn one_pointer_refused(&self) {
        self.answered.borrow_mut().pointers_refused += 1;
    }

    /// Zero, which is this machine's own way of saying an event was let through.
    fn let_it_carry_on(&self) -> isize {
        self.answered.borrow_mut().carried_on = true;
        0
    }

    /// One, the way every platform favjit runs on says an event ends here.
    fn end_it_here(&self) -> isize {
        self.answered.borrow_mut().ended_here = true;
        1
    }
}

/// The bodies of [`SourceHost`], on the state itself.
///
/// Inherent rather than the trait impl, because the one operation that hands the
/// run a connection has to hand it a share of this and there is nothing here to
/// take a share of: [`SimSource`] is where the trait is implemented, and every
/// operation but that one is this.
impl Source {
    fn pinned_sink(&mut self) -> Option<String> {
        self.pinned_sink_text.clone()
    }

    fn answer_procedures_with(&mut self, answering: Answering) {
        self.answering = Some(answering);
    }

    fn there_is_somewhere_to_show_it(&mut self) -> bool {
        self.shows_it_somewhere
    }

    fn show_what_is_refused(&mut self, what: Suppressing) {
        self.shown.push(what);
    }

    /// Turned to a standstill before returning, for the reason
    /// [`SimHost::look_for_devices`] is, and against a platform with no devices
    /// on it: what a script says about a keyboard is on the stream where the
    /// script said it, under the number the script gave it.
    fn take_input(
        &mut self,
        _probe_tick: Duration,
        _raw_bytes: usize,
        work: favjit_host::capture::SourceLoop,
    ) -> bool {
        self.warned_before_taking_input = !self.warnings.is_empty();
        if self.reads_input {
            let mut capture = SimCapture {
                turns: 1,
                answered: Arc::new(Mutex::new(0)),
                hooked: false,
            };
            work(&mut capture);
            // Read off what the loop asked for rather than off what this call
            // was told: whether a run may refuse anything is the hooks it
            // installs, and those are calls the loop makes.
            self.may_suppress = capture.hooked;
        }
        self.reads_input
    }

    fn pause(&mut self, how_long: Duration) {
        self.pauses += 1;
        self.run_now = advance(self.run_now, how_long);
    }

    fn asked_to_stop(&mut self) -> bool {
        self.record(SourceCall::AskedIfItWasAskedToStop);
        // A bound this machine was given and has passed. Where a script gave
        // none, a stream this run has already read dry is the same answer —
        // read dry, not merely empty: a script with nothing in it yet is a
        // machine nobody has typed on, and one read as a bound would end the run
        // before it ever looked for a sink.
        //
        // Reading having stopped is the other answer, and it is scripted rather
        // than inferred: the two look the same from a stream with nothing left
        // on it, which is the whole reason the run has to ask.
        match self.until {
            Some(deadline) => self.run_now >= deadline,
            None => self.stream_ran_out && !self.reading_stopped,
        }
    }

    fn has_a_sink_to_look_for(&mut self) -> bool {
        self.record(SourceCall::CheckedForASinkToLookFor);
        self.connects += 1;
        !self.no_sink
    }

    fn use_fixed_sink(&mut self) -> Option<favjit_host::Sink> {
        self.record(SourceCall::TriedFixedAddress);
        if !self.fixed_sink {
            return None;
        }
        if self.missing > 0 {
            self.missing -= 1;
            self.linked = false;
            return None;
        }
        Some(SOMEWHERE)
    }

    fn suppress(&mut self, what: Suppressing) {
        self.refused.push(what);
        if what == Suppressing::Everything {
            self.suppressions += 1;
            if !self.linked {
                self.suppressed_before_connecting += 1;
            }
        }
        self.suppressing = what;
    }

    fn send(&mut self, sealed: &[u8]) -> i32 {
        // The changes alternate away from a link that is there, so an odd number of
        // them behind the events handed over is a link that has gone.
        let behind = self
            .link_changes
            .iter()
            .filter(|mark| self.taken > **mark)
            .count();
        if behind % 2 == 1 {
            return self.send_code;
        }
        let Ok(sealed): Result<[u8; SEALED], _> = sealed.try_into() else {
            return self.send_code;
        };
        // Opened rather than read straight off the call: what the sink receives
        // is a sealed record, and a suite that took the frame before it was
        // sealed would leave the sealing untested by every test that uses this.
        let Some(frame) = self
            .session
            .as_mut()
            .and_then(|session| session.open(&sealed))
            .map(|(_, frame)| frame)
        else {
            return self.send_code;
        };
        let arrived = Message::decode(&frame).expect("a frame this end wrote is one it can read");
        self.sent.push(Sent {
            at: self.now,
            message: arrived,
        });
        0
    }
}

impl IdentityStore for Source {
    fn read(&mut self) -> Option<Vec<u8>> {
        let mut written = self.written.borrow_mut();
        written.calls.push(IdentityCall::Read);
        written.file.clone()
    }

    fn make_directory(&mut self) -> Result<(), Trouble> {
        self.written
            .borrow_mut()
            .calls
            .push(IdentityCall::MadeDirectory);
        refused_at(self.identity_fails_at, IdentityCall::MadeDirectory)
    }

    fn open(&mut self) -> Result<Box<dyn favjit_host::Writing>, Trouble> {
        self.written.borrow_mut().calls.push(IdentityCall::Opened);
        refused_at(self.identity_fails_at, IdentityCall::Opened)?;
        Ok(Box::new(SimWriting {
            written: Rc::clone(&self.written),
            fails_at: self.identity_fails_at,
            keeps: false,
        }))
    }
}

/// The bodies of [`Opening`], on the state itself, for the reason the source
/// boundary's are.
impl Source {
    fn connect(&mut self) -> bool {
        if !self.source_call(SourceCall::Connected) || self.socket_refused > 0 {
            self.socket_refused = self.socket_refused.saturating_sub(1);
            self.linked = false;
            return false;
        }
        self.linked = true;
        true
    }

    /// Answers as the sink standing in for it would, using the identity
    /// [`SimSource::pinned_to`] gave it: `engine::source::run` drives the same
    /// [`Initiator`] against whichever machine is on the other end of these
    /// calls, real or this one, so a genuine [`Responder`] here is what makes
    /// the two agree rather than a script standing in for the agreeing.
    fn send_first_message(&mut self, first: &[u8]) -> bool {
        if !self.source_call(SourceCall::SentFirstMessage) || self.handshake_dropped > 0 {
            self.handshake_dropped = self.handshake_dropped.saturating_sub(1);
            return false;
        }
        let Some(sink_identity) = self.sink_identity.clone() else {
            return false;
        };
        let Ok(first): Result<[u8; HANDSHAKE], _> = first.try_into() else {
            return false;
        };
        let Ok(mut responder) = Responder::new(&sink_identity, self) else {
            return false;
        };
        let Ok(answer) = responder.answer(&first) else {
            return false;
        };
        let Ok((_, session)) = responder.done() else {
            return false;
        };
        self.session = Some(session);
        self.pending_answer = Some(answer);
        true
    }

    fn flush_first_message(&mut self) -> bool {
        if !self.source_call(SourceCall::FlushedFirstMessage) || self.handshake_flush_dropped > 0 {
            self.handshake_flush_dropped = self.handshake_flush_dropped.saturating_sub(1);
            self.pending_answer = None;
            return false;
        }
        true
    }

    fn take_answer(&mut self, into: &mut [u8]) -> bool {
        if !self.source_call(SourceCall::TookAnswer) || self.answer_dropped > 0 {
            self.answer_dropped = self.answer_dropped.saturating_sub(1);
            self.pending_answer = None;
            return false;
        }
        match self.pending_answer.take() {
            Some(answer) => {
                into.copy_from_slice(&answer);
                true
            }
            None => false,
        }
    }
}

impl Entropy for Source {
    /// A different value each call, scripted rather than the platform's, so a
    /// run given the same script twice cannot shake hands differently
    /// (ADR-0007).
    fn fill(&mut self, into: &mut [u8]) -> bool {
        self.entropy_calls = self.entropy_calls.wrapping_add(1);
        into.fill(self.entropy_calls);
        true
    }
}

/// The connection while it is still being opened, as the run holds it.
struct SimOpen(Rc<RefCell<Source>>);

impl Opening for SimOpen {
    fn set_nodelay(&mut self, _enabled: bool) -> bool {
        self.0.borrow_mut().source_call(SourceCall::SetNoDelay)
    }

    fn set_read_timeout(&mut self, _timeout: Duration) -> bool {
        self.0.borrow_mut().source_call(SourceCall::SetReadTimeout)
    }

    fn set_write_timeout(&mut self, _timeout: Duration) -> bool {
        self.0.borrow_mut().source_call(SourceCall::SetWriteTimeout)
    }

    fn send_first_message(&mut self, first: &[u8]) -> bool {
        self.0.borrow_mut().send_first_message(first)
    }

    fn flush_first_message(&mut self) -> bool {
        self.0.borrow_mut().flush_first_message()
    }

    fn take_answer(&mut self, into: &mut [u8]) -> bool {
        self.0.borrow_mut().take_answer(into)
    }

    fn keep(self: Box<Self>) -> Box<dyn favjit_host::source::Sending> {
        Box::new(SimSending(self.0))
    }
}

/// The same connection with a session going over it, as the run holds it.
struct SimSending(Rc<RefCell<Source>>);

impl favjit_host::source::Sending for SimSending {
    fn send(&mut self, sealed: &[u8]) -> i32 {
        self.0.borrow_mut().send(sealed)
    }
}

impl Host for Source {
    fn now(&mut self) -> Instant {
        self.now
    }

    /// `deadline` is what the clock moves to when the script has nothing before
    /// it, which is what a real wait timing out leaves behind: this side has no
    /// repeat timer, so nothing else advances it.
    fn next_event(&mut self, deadline: Instant) -> Option<HostEvent> {
        let Some(event) = self.inbound.pop_front() else {
            // Said once, and only where the script itself ran dry: a bound this
            // run was given is the other reason a wait comes back empty, and a
            // machine that said both would let a test pass on the wrong one.
            let first = !self.stream_ran_out;
            self.stream_ran_out = true;
            self.now = self.now.max(deadline);
            return first.then_some(HostEvent {
                at: self.now,
                kind: EventKind::InputGone,
            });
        };
        self.now = event.at;
        self.taken += 1;
        Some(event)
    }

    fn is_supervised(&mut self) -> bool {
        self.supervised
    }

    fn warn(&mut self, message: core::fmt::Arguments) {
        self.warnings.push(message.to_string());
    }

    /// Arriving, unless a script says this machine's supervisor has stopped
    /// reading: which of the two it is decides whether a run says anything,
    /// and saying it is the run's (ADR-0006).
    fn heartbeat(&mut self) -> Result<(), Trouble> {
        self.heartbeats.push(self.now);
        match self.beats_arrive {
            true => Ok(()),
            false => Err(Trouble(String::from("nothing is reading the beats"))),
        }
    }
}

/// A machine standing in for the network, for the sink's end of the link.
///
/// Scripted the same way as the others: connections and the frames inside them go
/// in, and what the sequence did with them comes out. This end runs the source's
/// half of the real handshake and seals every record for real, the construction
/// `engine::link::serve` runs the sink's half of — so what a script decides is
/// what a real machine would send, not a flag standing in for it (ADR-0006).
#[derive(Default)]
pub struct SimLink(Arc<Mutex<Serving>>);

/// What this end is made of, shared with the connection it hands over.
///
/// Shared rather than held, because the run owns the connection for a whole
/// session and goes on asking this machine other things meanwhile — the list it
/// reads per connection among them. Behind a `Mutex` and not a `RefCell` because
/// the machine hands this end away to a thread (`bind_link`), so it has to be
/// `Send`.
#[derive(Default)]
struct Serving {
    /// What the authorised list says, as its text — the form a host reads it in.
    authorized: String,
    /// What is still to happen, in order.
    ///
    /// Pairing is in here with the connections rather than applied as the script is
    /// written, because when it happens is the thing worth testing: a list that was
    /// already complete before the run started could not show that authorising a
    /// source takes effect on the connection after it.
    script: VecDeque<Step>,
    /// The connection being served, if any, for its whole life — whether or not
    /// the handshake that would authenticate it ever completes.
    open: Option<ConnectingPeer>,
    /// The source's half of the handshake, from the moment its first message is
    /// asked for to the moment an answer either opens it or does not. Held here
    /// because this end stands in for the source's own machine and cryptography
    /// (ADR-0006).
    initiator: Option<Initiator>,
    /// The session once the handshake completes, for sealing what `records`
    /// queues next the way a real source's would.
    session: Option<Session>,
    /// A different value handed out each time `Entropy::fill` is asked, so the
    /// two ephemeral keys one handshake needs are never the one the other side
    /// just drew.
    entropy_calls: u8,
    /// Where the script is, and the time the record being handled arrived.
    cursor: Instant,
    now: Instant,
    converter_gone: bool,
    answer_flush_fails: bool,
    fails_at: Option<Call>,
    /// Whether the last refusal was one more wait rather than the socket, which
    /// the machine is asked separately because the answer that carries a
    /// connection cannot carry the reason there is none.
    refused_because: Option<bool>,
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
    /// Raw, the way [`LinkHost::deliver`] hands it over — a decoded relay is
    /// `engine`'s own resolver's to produce, the same as any other raw signal.
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
/// whether one happens at all after the one before it failed. Answering a
/// handshake and learning its peer are `engine::link::serve`'s own calls into the
/// construction it drives now, not this end's, so neither is a step to record here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Call {
    ReadListenerPort,
    Advertised,
    Accepted,
    SetReadTimeout,
    SetNoDelay,
    TookHandshake,
    SentAnswer,
    FlushedAnswer,
    Authorized,
    TookRecord,
}

#[derive(Debug)]
enum Step {
    Connect(ConnectingPeer),
    Pair(Vec<u8>),
    /// Something connected and never became a session.
    Rejected,
    /// The socket itself is unusable from here on.
    ListenerGone,
}

/// A peer, before anything about its handshake is known.
#[derive(Debug, Default)]
struct ConnectingPeer {
    /// `None` for one that never sends a first message.
    identity: Option<Identity>,
    /// The first message is nonsense instead of whatever `identity` would
    /// produce — a machine pinned to another key, or one not speaking this
    /// protocol at all.
    garbled: bool,
    /// Gone by the time the answer would reach it.
    drops: bool,
    records: VecDeque<PendingRecord>,
}

/// One record on its way in, sealed only once the session that will carry it
/// exists — the way a real source seals as it goes rather than in advance.
#[derive(Debug, Clone, Copy)]
enum PendingRecord {
    Frame([u8; FRAME], Instant),
    /// Sealed wrong, whatever is nominally inside it.
    Garbled(Instant),
}

/// `SimHost::default`'s own identity. A chosen byte pattern names no key any
/// private half actually derives, and a source standing in for the network now
/// has to shake hands with a real one to get in. Public so a `SimSource`
/// pinned to the Mac in the same test can be pinned to this same seed.
pub const SINK_SEED: u8 = 0x50;

/// Bytes this process's own source of randomness supplies, for the one identity
/// [`identity`] and [`SimHost::default`] need without a test asking for one in
/// particular.
struct RealEntropy;

impl Entropy for RealEntropy {
    fn fill(&mut self, into: &mut [u8]) -> bool {
        getrandom::getrandom(into).is_ok()
    }
}

/// The same identity every time this `seed` is asked for, within one test binary.
///
/// A real keypair rather than a chosen pattern: a source proves which key it
/// holds by shaking hands with it, so what a test pins to an authorised list has
/// to be one the handshake can actually produce. Real entropy and not a scripted
/// fill, because the property under test never depends on which bytes an
/// identity happens to have — only on whether one seed's key is the other's.
pub fn identity(seed: u8) -> Identity {
    use std::collections::HashMap;
    use std::sync::OnceLock;

    static CACHE: OnceLock<Mutex<HashMap<u8, Identity>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    cache
        .lock()
        .expect("nothing panics holding this")
        .entry(seed)
        .or_insert_with(|| keypair(&mut RealEntropy).expect("this machine's entropy"))
        .clone()
}

/// The identity a machine's entropy handing over exactly `private` would make.
///
/// A test's own way of computing what [`SimHost::that_can_make`] commits this
/// machine to presenting: the public half is derived from the private one for
/// real ([`favjit_noise::keypair`]), not chosen alongside it, so asserting
/// against an arbitrary pair would be asserting against a keypair nothing
/// derives.
pub fn derived_from(private: &[u8]) -> Identity {
    struct Fixed<'a>(&'a [u8]);
    impl Entropy for Fixed<'_> {
        fn fill(&mut self, into: &mut [u8]) -> bool {
            if into.len() != self.0.len() {
                return false;
            }
            into.copy_from_slice(self.0);
            true
        }
    }
    keypair(&mut Fixed(private)).unwrap_or_else(|| panic!("{} bytes is a private key", KEY))
}

impl SimLink {
    /// Start with this list of authorised keys, as text.
    pub fn new(authorized: String) -> Self {
        Self(Arc::new(Mutex::new(Serving {
            authorized,
            ..Serving::default()
        })))
    }

    /// This end, reporting into a record the machine it belongs to already has.
    ///
    /// One record for the machine and its link, because a test asks the machine
    /// what its link did rather than asking the link.
    pub(crate) fn observing(observed: Arc<Mutex<Observed>>) -> Self {
        Self(Arc::new(Mutex::new(Serving {
            observed,
            ..Serving::default()
        })))
    }

    /// The record this end reports into, for the machine to share.
    pub(crate) fn observed(&self) -> Arc<Mutex<Observed>> {
        Arc::clone(&self.serving().observed)
    }

    /// This end, as the connection it hands over reaches it.
    ///
    /// The lock is never held across a call into this object, so nothing can be
    /// waiting on it.
    fn serving(&self) -> std::sync::MutexGuard<'_, Serving> {
        self.0.lock().expect("nothing panics holding this")
    }

    /// A peer connects, presenting this identity. Whatever is scripted after this
    /// belongs to it, until the next `connect` or a `hang_up`.
    pub fn connect(&mut self, identity: Identity) -> &mut Self {
        self.serving().connect(identity);
        self
    }

    /// Something connects and never sends its first message.
    pub fn connects_and_says_nothing(&mut self) -> &mut Self {
        self.serving().connects_and_says_nothing();
        self
    }

    /// Something connects and sends a first message this end cannot open — a
    /// machine pinned to another key, or one not speaking this protocol at all.
    pub fn connects_with_nonsense(&mut self) -> &mut Self {
        self.serving().connects_with_nonsense();
        self
    }

    /// Something connects, presenting this identity, and is gone by the time the
    /// answer is written.
    pub fn connects_and_drops_before_the_answer(&mut self, identity: Identity) -> &mut Self {
        self.serving()
            .connects_and_drops_before_the_answer(identity);
        self
    }

    /// An answer whose write succeeds but whose flush does not.
    pub fn answer_cannot_be_flushed(&mut self) -> &mut Self {
        self.serving().answer_cannot_be_flushed();
        self
    }

    pub fn fails_at(&mut self, call: Call) -> &mut Self {
        self.serving().fails_at(call);
        self
    }

    /// A record that will not open, whatever is nominally inside it.
    pub fn sends_a_record_that_will_not_open(&mut self) -> &mut Self {
        self.serving().sends_a_record_that_will_not_open();
        self
    }

    /// Authorise a key, as `favjit --pair` would between two connections.
    pub fn pair(&mut self, key: Vec<u8>) -> &mut Self {
        self.serving().pair(key);
        self
    }

    /// Announce a keyboard the way the peer at the other end of the link does.
    pub fn attach(&mut self, info: Attached) -> &mut Self {
        self.serving().attach(info);
        self
    }

    /// A keyboard the peer announces as named by USB identity.
    pub fn attach_external(&mut self, id: DeviceId, vendor_id: u16, product_id: u16) -> &mut Self {
        self.serving().attach_external(id, vendor_id, product_id);
        self
    }

    pub fn detach(&mut self, id: DeviceId) -> &mut Self {
        self.serving().detach(id);
        self
    }

    pub fn press(&mut self, device: DeviceId, key: Key) -> &mut Self {
        self.serving().press(device, key);
        self
    }

    pub fn release(&mut self, device: DeviceId, key: Key) -> &mut Self {
        self.serving().release(device, key);
        self
    }

    /// Something connects and the handshake does not complete.
    pub fn rejected(&mut self) -> &mut Self {
        self.serving().rejected();
        self
    }

    /// Something connects and cannot be taken, this many times in a row — a machine
    /// on the same desk being switched off, or something scanning the port.
    pub fn rejected_times(&mut self, times: usize) -> &mut Self {
        self.serving().rejected_times(times);
        self
    }

    /// The socket stops being usable — the descriptors are gone, or the interface
    /// it was bound to is.
    pub fn listener_gone(&mut self) -> &mut Self {
        self.serving().listener_gone();
        self
    }

    /// The converter has stopped, so nothing more can be delivered to it.
    pub fn converter_stopped(&mut self) -> &mut Self {
        self.serving().converter_stopped();
        self
    }

    /// A frame no version of this end can read.
    pub fn nonsense(&mut self) -> &mut Self {
        self.serving().nonsense();
        self
    }

    /// The peer goes away, ending its session.
    pub fn hang_up(&mut self) -> &mut Self {
        self
    }

    /// Move the peer's clock forward, so a hold at the other end is a hold.
    pub fn advance(&mut self, by: Duration) -> &mut Self {
        self.serving().advance(by);
        self
    }

    /// Everything a source relayed, as it comes off the wire.
    pub fn relay(&mut self, sent: &[Sent]) -> &mut Self {
        self.serving().relay(sent);
        self
    }

    /// Every event that reached the converter, in order — raw, the way
    /// [`LinkHost::deliver`] handed it over.
    pub fn delivered(&self) -> Vec<EventKind> {
        self.serving().delivered()
    }

    /// Every peer that was sent away.
    pub fn refused(&self) -> Vec<Vec<u8>> {
        self.serving().refused()
    }

    /// Why each connection was let go, in order.
    pub fn closed(&self) -> Vec<String> {
        self.serving().closed()
    }

    pub fn disconnects(&self) -> usize {
        self.serving().disconnects()
    }

    /// How many frames were taken from peers at all — which is zero for a
    /// connection that was refused.
    pub fn frames_read(&self) -> usize {
        self.serving().frames_read()
    }

    /// Every call the sink made, in order.
    pub fn calls(&self) -> Vec<Call> {
        self.serving().calls()
    }

    /// How many times this end said on the network that it is here.
    pub fn advertisements(&self) -> usize {
        self.serving().advertisements()
    }
}

impl Serving {
    /// A peer connects, presenting this identity. Whatever is scripted after this
    /// belongs to it, until the next `connect` or a `hang_up`.
    pub fn connect(&mut self, identity: Identity) -> &mut Self {
        self.script.push_back(Step::Connect(ConnectingPeer {
            identity: Some(identity),
            ..ConnectingPeer::default()
        }));
        self
    }

    /// Something connects and never sends its first message.
    pub fn connects_and_says_nothing(&mut self) -> &mut Self {
        self.script
            .push_back(Step::Connect(ConnectingPeer::default()));
        self
    }

    /// Something connects and sends a first message this end cannot open — a
    /// machine pinned to another key, or one not speaking this protocol at all.
    pub fn connects_with_nonsense(&mut self) -> &mut Self {
        self.script.push_back(Step::Connect(ConnectingPeer {
            garbled: true,
            ..ConnectingPeer::default()
        }));
        self
    }

    /// Something connects, presenting this identity, and is gone by the time the
    /// answer is written.
    pub fn connects_and_drops_before_the_answer(&mut self, identity: Identity) -> &mut Self {
        self.script.push_back(Step::Connect(ConnectingPeer {
            identity: Some(identity),
            drops: true,
            ..ConnectingPeer::default()
        }));
        self
    }

    /// An answer whose write succeeds but whose flush does not.
    pub fn answer_cannot_be_flushed(&mut self) -> &mut Self {
        self.answer_flush_fails = true;
        self
    }

    pub fn fails_at(&mut self, call: Call) -> &mut Self {
        self.fails_at = Some(call);
        self
    }

    /// A record that will not open, whatever is nominally inside it.
    pub fn sends_a_record_that_will_not_open(&mut self) -> &mut Self {
        self.enqueue(PendingRecord::Garbled(self.cursor))
    }

    /// Authorise a key, as `favjit --pair` would between two connections.
    pub fn pair(&mut self, key: Vec<u8>) -> &mut Self {
        self.script.push_back(Step::Pair(key));
        self
    }

    /// Announce a keyboard the way the peer at the other end of the link does.
    ///
    /// Already read, unlike the two machines' own attachments: what crosses the
    /// link is what the source's run made of its own machine's words, and this
    /// end stands in for that run rather than for its machine.
    pub fn attach(&mut self, info: Attached) -> &mut Self {
        self.send(Message::DeviceAttached(info))
    }

    /// A keyboard the peer announces as named by USB identity.
    pub fn attach_external(&mut self, id: DeviceId, vendor_id: u16, product_id: u16) -> &mut Self {
        self.attach(Attached {
            device: id,
            is_built_in: false,
            vendor_id: Some(vendor_id),
            product_id: Some(product_id),
        })
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

    /// Move the peer's clock forward, so a hold at the other end is a hold.
    pub fn advance(&mut self, by: Duration) -> &mut Self {
        self.cursor = advance(self.cursor, by);
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

    fn frame(&mut self, frame: [u8; FRAME]) -> &mut Self {
        self.enqueue(PendingRecord::Frame(frame, self.cursor))
    }

    fn enqueue(&mut self, record: PendingRecord) -> &mut Self {
        // Onto the last connection scripted, which is the one a reader of the
        // script would take it to belong to.
        if let Some(Step::Connect(peer)) = self
            .script
            .iter_mut()
            .rev()
            .find(|step| matches!(step, Step::Connect(_)))
        {
            peer.records.push_back(record);
        }
        self
    }

    /// Every event that reached the converter, in order — raw, the way
    /// [`LinkHost::deliver`] handed it over.
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
    fn listener_port(&mut self) -> Option<u16> {
        self.serving().listener_port()
    }

    fn advertise(&mut self, service: &str, port: u16) -> bool {
        self.serving().advertise(service, port)
    }

    fn accept(&mut self) -> Accepted {
        // The answer is taken out of the lock before it is read, because the arms
        // below reach this machine again: a guard held across them is a wait on a
        // lock this thread is the one holding.
        let taken = self.serving().accept();
        match taken {
            true => Accepted::Yes(Box::new(SimTalk(Arc::clone(&self.0)))),
            false => self.serving().why_not(),
        }
    }

    fn authorized(&mut self) -> Option<String> {
        self.serving().authorized()
    }

    fn deliver(&mut self, event: EventKind) -> bool {
        self.serving().deliver(event)
    }

    fn warn(&mut self, message: core::fmt::Arguments) {
        self.serving().warn(message);
    }
}

/// The one connection this end serves, as the run holds it.
struct SimTalk(Arc<Mutex<Serving>>);

impl SimTalk {
    fn serving(&self) -> std::sync::MutexGuard<'_, Serving> {
        self.0.lock().expect("nothing panics holding this")
    }
}

impl favjit_host::link::Talking for SimTalk {
    fn set_read_timeout(&mut self, timeout: Duration) -> bool {
        self.serving().set_read_timeout(timeout)
    }

    fn set_nodelay(&mut self, enabled: bool) -> bool {
        self.serving().set_nodelay(enabled)
    }

    fn take_handshake(&mut self, into: &mut [u8]) -> bool {
        self.serving().take_handshake(into)
    }

    fn send_answer(&mut self, answer: &[u8]) -> bool {
        self.serving().send_answer(answer)
    }

    fn flush_answer(&mut self) -> bool {
        self.serving().flush_answer()
    }

    fn take_record(&mut self, into: &mut [u8]) -> Incoming {
        self.serving().take_record(into)
    }

    fn sending_it_away(&mut self) {
        self.serving().sending_it_away();
    }
}

impl Drop for SimTalk {
    /// Letting it go is what closes it.
    fn drop(&mut self) {
        self.serving().let_go();
    }
}

impl Serving {
    fn listener_port(&mut self) -> Option<u16> {
        self.called(Call::ReadListenerPort);
        match self.fails_at == Some(Call::ReadListenerPort) {
            true => None,
            false => Some(51763),
        }
    }

    fn advertise(&mut self, _service: &str, _port: u16) -> bool {
        self.called(Call::Advertised);
        self.observed().advertised += 1;
        self.fails_at != Some(Call::Advertised)
    }

    /// Whether something connected, with [`Serving::why_not`] saying what it was
    /// where nothing did.
    ///
    /// Two calls rather than one answer, because the answer carries the
    /// connection and only the machine that made one can build it.
    fn accept(&mut self) -> bool {
        self.refused_because = None;
        loop {
            match self.script.pop_front() {
                // A real listener with nobody connecting parks in this call, and a
                // simulated one cannot: running out of script is this test having
                // nothing further to say, not the socket going away. What that
                // would be is scripted.
                None => {
                    self.observed().script_ran_out = true;
                    self.refused_because = Some(false);
                    return false;
                }
                Some(Step::Pair(key)) => self.authorized = added(&self.authorized, &key),
                // Nothing usable this time, the same as an empty poll: `engine`'s
                // own retry counts either the same way.
                Some(Step::Rejected) => {
                    self.refused_because = Some(true);
                    return false;
                }
                Some(Step::ListenerGone) => {
                    self.refused_because = Some(false);
                    return false;
                }
                Some(Step::Connect(peer)) => {
                    self.open = Some(peer);
                    self.called(Call::Accepted);
                    return true;
                }
            }
        }
    }

    /// What the socket said about the last accept, where it refused.
    ///
    /// One kind for each thing a script can say, chosen from what a real socket
    /// answers with: a connection that went away before it was taken is a reset,
    /// and a socket that is unusable answers with something no run has a name
    /// for. Which of those means one more wait is read off the kind in `engine`,
    /// which is what a script exercising that reading needs a kind for.
    fn why_not(&mut self) -> Accepted {
        match self.refused_because {
            Some(true) => Accepted::No(std::io::ErrorKind::ConnectionReset),
            _ => Accepted::No(std::io::ErrorKind::Other),
        }
    }

    fn set_read_timeout(&mut self, _timeout: Duration) -> bool {
        self.called(Call::SetReadTimeout);
        self.fails_at != Some(Call::SetReadTimeout)
    }

    fn set_nodelay(&mut self, _enabled: bool) -> bool {
        self.called(Call::SetNoDelay);
        self.fails_at != Some(Call::SetNoDelay)
    }

    fn take_handshake(&mut self, into: &mut [u8]) -> bool {
        self.called(Call::TookHandshake);
        let Some(peer) = self.open.as_ref() else {
            return false;
        };
        if peer.garbled {
            into.fill(0xff);
            return true;
        }
        let Some(source) = peer.identity.clone() else {
            return false;
        };
        let Ok(mut initiator) = Initiator::new(&source, identity(SINK_SEED).public(), self) else {
            return false;
        };
        let Ok(first) = initiator.first() else {
            return false;
        };
        into.copy_from_slice(&first);
        self.initiator = Some(initiator);
        true
    }

    fn send_answer(&mut self, answer: &[u8]) -> bool {
        self.called(Call::SentAnswer);
        let Some(mut initiator) = self.initiator.take() else {
            return false;
        };
        if self.open.as_ref().is_some_and(|peer| peer.drops) {
            return false;
        }
        let Ok(answer): Result<[u8; ANSWER], _> = answer.try_into() else {
            return false;
        };
        if initiator.take_answer(&answer).is_err() {
            return false;
        }
        match initiator.done() {
            Ok(session) => {
                self.session = Some(session);
                true
            }
            Err(_) => false,
        }
    }

    fn flush_answer(&mut self) -> bool {
        self.called(Call::FlushedAnswer);
        !self.answer_flush_fails
    }

    fn authorized(&mut self) -> Option<String> {
        self.called(Call::Authorized);
        Some(self.authorized.clone())
    }

    fn take_record(&mut self, into: &mut [u8]) -> Incoming {
        self.called(Call::TookRecord);
        let Some(pending) = self.open.as_mut().and_then(|peer| peer.records.pop_front()) else {
            return Incoming::Gone;
        };
        let (at, sealed) = match pending {
            PendingRecord::Frame(frame, at) => {
                let Some(sealed) = self
                    .session
                    .as_mut()
                    .and_then(|s| s.seal(&frame).ok())
                    .map(|(_, sealed)| sealed)
                else {
                    return Incoming::Gone;
                };
                (at, sealed)
            }
            PendingRecord::Garbled(at) => (at, [0xffu8; SEALED]),
        };
        self.now = at;
        self.observed().frames_read += 1;
        into.copy_from_slice(&sealed);
        Incoming::Record
    }

    fn deliver(&mut self, event: EventKind) -> bool {
        if self.converter_gone {
            return false;
        }
        let at = self.now;
        let mut observed = self.observed();
        observed.delivered.push(event.clone());
        observed.stream.push_back(HostEvent { at, kind: event });
        true
    }

    /// Who was sent away, which is what a test about authorisation asks — and
    /// after the handshake there is nobody else it could be.
    fn sending_it_away(&mut self) {
        let refused = self.open.as_ref().and_then(|peer| peer.identity.clone());
        let key = refused.map_or_else(Vec::new, |identity| identity.public().to_vec());
        self.observed().refused.push(key);
    }

    fn let_go(&mut self) {
        self.initiator = None;
        self.session = None;
        self.open = None;
    }

    /// Kept as the sentence a run composed, which is what
    /// [`SimLink::closed`] answers with: why a connection was let go is the
    /// run's own words about it, and a script asserting on them is asserting
    /// on the decision behind them.
    fn warn(&mut self, message: core::fmt::Arguments) {
        self.observed().closed.push(message.to_string());
    }
}

impl Entropy for SimLink {
    fn fill(&mut self, into: &mut [u8]) -> bool {
        self.serving().fill(into)
    }
}

impl Entropy for Serving {
    /// A different value each call, scripted rather than the platform's, so a
    /// run given the same script twice cannot shake hands differently
    /// (ADR-0007).
    fn fill(&mut self, into: &mut [u8]) -> bool {
        self.entropy_calls = self.entropy_calls.wrapping_add(1);
        into.fill(self.entropy_calls);
        true
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

/// The text `favjit --pair` would leave a file in, with one more key added.
///
/// `engine::pairing::Authorized::added`'s own arithmetic, run here rather than
/// depended on: what a text form should look like is a decision this crate
/// cannot borrow (ADR-0005), so a script that simulates the file writes the
/// same bytes that function would rather than a second format that happens to
/// agree with it today.
///
/// Private, and a script stands a paired machine up by handing
/// [`SimLink::new`] the text it wants rather than reaching for this: a test that
/// checked what a run wrote against the same function this crate writes with
/// would be checking that function against itself.
fn added(text: &str, key: &[u8]) -> String {
    let mut out = String::from(text);
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    for byte in key {
        out.push_str(&format!("{byte:02x}"));
    }
    out.push('\n');
    out
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

/// What a sink pairing run asked of its host, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairingCall {
    BoundListener,
    ReadListenerPort,
    Advertised,
    ShowedCode,
    ShowedInstruction,
    SetBlocking,
    AcceptedSource,
    SetReadTimeout,
    SetWriteTimeout,
    TookOffer,
    SentAnswer,
    FlushedAnswer,
    TookSealedKey,
    SentSealedKey,
    FlushedSealedKey,
    ReadAuthorized,
    MadeAuthorizedDirectory,
    Authorized,
}

/// A machine standing in for the one a code is entered on, with a source in front of
/// it.
///
/// **The exchange itself really runs.** The source this scripts is played with the
/// same functions the forwarding machine's own run calls, so a source that was given
/// other digits ends up holding nothing because the arithmetic says so — not because
/// this compared two codes. What is stood in for is the machine: a screen, a socket,
/// a file, and bytes from a stream a run can be repeated with (ADR-0007).
#[derive(Default)]
pub struct SimPairing(Rc<RefCell<Attempt>>);

/// What one attempt is made of, shared with the sockets it hands over.
///
/// Shared rather than held, because the run owns the listener and the connection
/// for as long as it is using them and goes on asking this machine other things
/// meanwhile: what those sockets do is this attempt's, so they reach it rather
/// than carrying a copy that would put their calls out of the order a script
/// asserts.
struct Attempt {
    entropy: Trickle,
    shown: Option<Code>,
    /// The sources waiting to be served, in order. Each carries its key and whether
    /// it entered the code correctly.
    sources: VecDeque<Waiting>,
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
    /// Whether what the source offers is bytes the exchange cannot read.
    offers_nonsense: bool,
    /// How many more of the exchange's own calls this connection will carry.
    ///
    /// A budget rather than a flag per call: what goes wrong on a network is that
    /// the peer stops, and *where* it stops is the thing a test varies — a knob
    /// per step would be one more to add every time the exchange grows a step.
    carries: Option<usize>,
    /// Whether the code was shown before a source was waited for, and whether the
    /// source's key had arrived before anything was written down. Both are order,
    /// which is behaviour.
    showed_before_waiting: bool,
    took_the_key_before_authorizing: bool,
    took_the_key: bool,
    calls: Vec<PairingCall>,
    fails_at: Option<PairingCall>,
    shown_lines: Vec<String>,
}

/// One source waiting to be served, as the script describes it.
#[derive(Debug, Clone)]
struct Waiting {
    key: Vec<u8>,
    knows_the_code: bool,
}

impl Default for Attempt {
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
            offers_nonsense: false,
            carries: None,
            showed_before_waiting: false,
            took_the_key_before_authorizing: false,
            took_the_key: false,
            calls: Vec::new(),
            fails_at: None,
            shown_lines: Vec::new(),
        }
    }
}

impl SimPairing {
    pub fn new() -> Self {
        Self::default()
    }

    /// A source that entered the code correctly, presenting this key.
    pub fn with_a_source_that_knows_the_code(self, key: &[u8]) -> Self {
        self.0.borrow_mut().sources.push_back(Waiting {
            key: key.to_vec(),
            knows_the_code: true,
        });
        self
    }

    /// A source that entered something else.
    pub fn with_a_source_that_has_the_code_wrong(self, key: &[u8]) -> Self {
        self.0.borrow_mut().sources.push_back(Waiting {
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
    pub fn with_no_code(self) -> Self {
        self.0.borrow_mut().entropy = Trickle::dry();
        self
    }

    /// A machine that cannot write the list down.
    pub fn that_cannot_keep_the_list(self) -> Self {
        self.0.borrow_mut().keeps = false;
        self
    }

    pub fn whose_pairing_fails_at(self, call: PairingCall) -> Self {
        self.0.borrow_mut().fails_at = Some(call);
        self
    }

    pub fn pairing_calls(&self) -> Vec<PairingCall> {
        self.0.borrow().calls.clone()
    }

    pub fn shown_lines(&self) -> Vec<String> {
        self.0.borrow().shown_lines.clone()
    }

    /// A list this machine already holds, as the file holds it.
    ///
    /// Text and not keys, because a person edits this file: what a pairing has to
    /// do with a line they typed without a newline after it is the thing worth
    /// scripting.
    pub fn holding(self, list: &str) -> Self {
        self.0.borrow_mut().authorized = list.to_string();
        self
    }

    /// A source that stops answering once it has carried this many of the
    /// exchange's calls.
    pub fn that_goes_quiet_after(self, calls: usize) -> Self {
        self.0.borrow_mut().carries = Some(calls);
        self
    }

    /// A source whose offer is not one: bytes of the right length that the
    /// exchange cannot read, which is what a machine speaking a different version
    /// of it sounds like from here.
    pub fn with_a_source_that_offers_nonsense(self) -> Self {
        self.0.borrow_mut().offers_nonsense = true;
        self
    }

    /// The code that was put in front of the person, if one was.
    pub fn shown(&self) -> Option<Code> {
        self.0.borrow().shown
    }

    /// How many sources were served. One at most, because the code is spent.
    pub fn attempts(&self) -> usize {
        self.0.borrow().attempts
    }

    /// The list as it stands now, as the text a file holds it in.
    ///
    /// Text rather than a decoded list: what a line means is `engine::pairing`'s
    /// own `Authorized::parse` to say, and a second reading of it here could
    /// disagree with the one the sequence itself trusts.
    pub fn authorized_text(&self) -> String {
        self.0.borrow().authorized.clone()
    }

    /// What the source ended up holding, if it could open what it was sent.
    pub fn gave_the_source(&self) -> Option<Vec<u8>> {
        self.0.borrow().gave.clone()
    }

    pub fn showed_before_waiting(&self) -> bool {
        self.0.borrow().showed_before_waiting
    }

    /// Whether the source's key had arrived before anything was written down.
    pub fn took_the_key_before_authorizing(&self) -> bool {
        self.0.borrow().took_the_key_before_authorizing
    }
}

impl Attempt {
    /// What a machine that will not keep this scripts back.
    ///
    /// A sentence naming the call, because a real machine's own error carries
    /// one: a script answering with nothing would let a run that lost the
    /// reason pass (ADR-0006).
    fn would_keep(&self, call: PairingCall) -> Result<(), Trouble> {
        match self.keeps && self.fails_at != Some(call) {
            true => Ok(()),
            false => Err(Trouble(format!("this machine refuses {call:?}"))),
        }
    }

    /// The source currently being served, if one is.
    fn serving(&self) -> Option<&Waiting> {
        self.sources.front()
    }

    /// Whether this connection carries one more of the exchange's calls, spending
    /// the budget if it does.
    fn carries_one_more(&mut self) -> bool {
        match &mut self.carries {
            None => true,
            Some(0) => false,
            Some(left) => {
                *left -= 1;
                true
            }
        }
    }
}

impl Entropy for SimPairing {
    fn fill(&mut self, into: &mut [u8]) -> bool {
        self.0.borrow_mut().entropy.fill(into)
    }
}

impl Entropy for Attempt {
    fn fill(&mut self, into: &mut [u8]) -> bool {
        self.entropy.fill(into)
    }
}

/// The socket this attempt takes a source on, as the run holds it.
struct SimListener(Rc<RefCell<Attempt>>);

impl favjit_host::pairing::Listening for SimListener {
    fn port(&mut self) -> Option<u16> {
        self.0.borrow_mut().listener_port()
    }

    fn set_blocking(&mut self) -> bool {
        self.0.borrow_mut().set_blocking()
    }

    fn accept(&mut self) -> Option<Box<dyn favjit_host::pairing::Exchanging>> {
        match self.0.borrow_mut().accept_source() {
            true => Some(Box::new(SimExchange(Rc::clone(&self.0)))),
            false => None,
        }
    }
}

/// The one connection this attempt runs over, as the run holds it.
struct SimExchange(Rc<RefCell<Attempt>>);

impl Entropy for SimExchange {
    fn fill(&mut self, into: &mut [u8]) -> bool {
        self.0.borrow_mut().entropy.fill(into)
    }
}

impl favjit_host::pairing::Exchanging for SimExchange {
    fn set_read_timeout(&mut self, timeout: Duration) -> bool {
        self.0.borrow_mut().set_read_timeout(timeout)
    }

    fn set_write_timeout(&mut self, timeout: Duration) -> bool {
        self.0.borrow_mut().set_write_timeout(timeout)
    }

    fn take_offer(&mut self, into: &mut [u8]) -> bool {
        self.0.borrow_mut().take_offer(into)
    }

    fn send_answer(&mut self, answer: &[u8]) -> bool {
        self.0.borrow_mut().send_answer(answer)
    }

    fn flush_answer(&mut self) -> bool {
        self.0.borrow_mut().flush_answer()
    }

    fn take_sealed_key(&mut self, into: &mut [u8]) -> bool {
        self.0.borrow_mut().take_sealed_key(into)
    }

    fn send_sealed_key(&mut self, sealed: &[u8]) -> bool {
        self.0.borrow_mut().send_sealed_key(sealed)
    }

    fn flush_sealed_key(&mut self) -> bool {
        self.0.borrow_mut().flush_sealed_key()
    }
}

impl PairingHost for SimPairing {
    fn bind_listener(&mut self) -> Option<Box<dyn favjit_host::pairing::Listening>> {
        match self.0.borrow_mut().bind_listener() {
            true => Some(Box::new(SimListener(Rc::clone(&self.0)))),
            false => None,
        }
    }

    fn advertise(&mut self, service: &str, port: u16) -> bool {
        self.0.borrow_mut().advertise(service, port)
    }

    fn show(&mut self, line: &str) {
        self.0.borrow_mut().show(line);
    }

    fn authorized(&mut self) -> Option<String> {
        self.0.borrow_mut().authorized()
    }

    fn make_authorized_directory(&mut self) -> Result<(), Trouble> {
        self.0.borrow_mut().make_authorized_directory()
    }

    fn authorize(&mut self, text: &str) -> Result<(), Trouble> {
        self.0.borrow_mut().authorize(text)
    }
}

impl Attempt {
    fn bind_listener(&mut self) -> bool {
        self.calls.push(PairingCall::BoundListener);
        self.fails_at != Some(PairingCall::BoundListener)
    }

    fn listener_port(&mut self) -> Option<u16> {
        self.calls.push(PairingCall::ReadListenerPort);
        match self.fails_at == Some(PairingCall::ReadListenerPort) {
            true => None,
            false => Some(51764),
        }
    }

    fn advertise(&mut self, _service: &str, _port: u16) -> bool {
        self.calls.push(PairingCall::Advertised);
        self.fails_at != Some(PairingCall::Advertised)
    }

    fn show(&mut self, line: &str) {
        let call = match self.shown_lines.is_empty() {
            true => PairingCall::ShowedCode,
            false => PairingCall::ShowedInstruction,
        };
        self.calls.push(call);
        self.shown_lines.push(line.to_string());
        if let Some(digits) = line.strip_prefix("pairing code: ") {
            self.shown = digits.as_bytes().try_into().ok();
        }
    }

    fn set_blocking(&mut self) -> bool {
        self.calls.push(PairingCall::SetBlocking);
        self.fails_at != Some(PairingCall::SetBlocking)
    }

    fn accept_source(&mut self) -> bool {
        self.calls.push(PairingCall::AcceptedSource);
        self.showed_before_waiting = self.shown.is_some();
        if self.fails_at == Some(PairingCall::AcceptedSource) || self.sources.is_empty() {
            return false;
        }
        self.attempts += 1;
        true
    }

    fn set_read_timeout(&mut self, _timeout: Duration) -> bool {
        self.calls.push(PairingCall::SetReadTimeout);
        self.fails_at != Some(PairingCall::SetReadTimeout)
    }

    fn set_write_timeout(&mut self, _timeout: Duration) -> bool {
        self.calls.push(PairingCall::SetWriteTimeout);
        self.fails_at != Some(PairingCall::SetWriteTimeout)
    }

    fn take_offer(&mut self, into: &mut [u8]) -> bool {
        self.calls.push(PairingCall::TookOffer);
        if self.fails_at == Some(PairingCall::TookOffer) || !self.carries_one_more() {
            return false;
        }
        let Some(knows_the_code) = self.serving().map(|source| source.knows_the_code) else {
            return false;
        };
        let Some(shown) = self.shown else {
            return false;
        };
        // The digits the person at the source typed are the whole of what that end
        // can have wrong: a source given others produces an offer as well formed as
        // this one, and the difference only shows where a key will not open.
        let code = match knows_the_code {
            true => shown,
            false => other_digits(shown),
        };
        let Some((started, offer)) = favjit_pairing_exchange::offer(code, self) else {
            return false;
        };
        self.started = Some(started);
        match self.offers_nonsense {
            true => into.fill(0xff),
            false => into.copy_from_slice(&offer),
        }
        true
    }

    fn send_answer(&mut self, answer: &[u8]) -> bool {
        self.calls.push(PairingCall::SentAnswer);
        if self.fails_at == Some(PairingCall::SentAnswer) || !self.carries_one_more() {
            return false;
        }
        let Ok(answer): Result<[u8; OFFER], _> = answer.try_into() else {
            return false;
        };
        self.agreed = self
            .started
            .take()
            .and_then(|started| started.finish(&answer));
        // The send itself is what this reports. A source that could not finish the
        // exchange is one that then sends nothing, which is not the same failure.
        true
    }

    fn flush_answer(&mut self) -> bool {
        self.calls.push(PairingCall::FlushedAnswer);
        self.fails_at != Some(PairingCall::FlushedAnswer)
    }

    fn take_sealed_key(&mut self, into: &mut [u8]) -> bool {
        self.calls.push(PairingCall::TookSealedKey);
        if self.fails_at == Some(PairingCall::TookSealedKey) || !self.carries_one_more() {
            return false;
        }
        let Some(key) = self.serving().map(|source| source.key.clone()) else {
            return false;
        };
        let Some(sealed) = self
            .agreed
            .as_ref()
            .and_then(|agreed| agreed.seal(&key, Side::Source))
        else {
            return false;
        };
        self.took_the_key = true;
        into.copy_from_slice(&sealed);
        true
    }

    fn send_sealed_key(&mut self, sealed: &[u8]) -> bool {
        self.calls.push(PairingCall::SentSealedKey);
        if self.fails_at == Some(PairingCall::SentSealedKey) || !self.carries_one_more() {
            return false;
        }
        let Ok(sealed): Result<[u8; SEALED_KEY], _> = sealed.try_into() else {
            return false;
        };
        // Opened rather than recorded as sent: what a source holds at the end is what
        // it could read, and a key sealed under a secret it did not agree on is
        // nothing to it.
        self.gave = self
            .agreed
            .as_ref()
            .and_then(|agreed| agreed.open(&sealed, Side::Sink));
        true
    }

    fn flush_sealed_key(&mut self) -> bool {
        self.calls.push(PairingCall::FlushedSealedKey);
        self.fails_at != Some(PairingCall::FlushedSealedKey)
    }

    fn authorized(&mut self) -> Option<String> {
        self.calls.push(PairingCall::ReadAuthorized);
        self.took_the_key_before_authorizing = self.took_the_key;
        Some(self.authorized.clone())
    }

    fn make_authorized_directory(&mut self) -> Result<(), Trouble> {
        self.calls.push(PairingCall::MadeAuthorizedDirectory);
        self.would_keep(PairingCall::MadeAuthorizedDirectory)
    }

    fn authorize(&mut self, text: &str) -> Result<(), Trouble> {
        self.calls.push(PairingCall::Authorized);
        self.would_keep(PairingCall::Authorized)?;
        self.authorized = text.to_string();
        Ok(())
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
    HandedTheTraceOver,
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
    /// After how many probes somebody asks for the recording.
    ///
    /// Counted in probes rather than set as a flag, because when it is asked for
    /// is the thing worth testing: a run that was asked before it started could
    /// not show that supervising carries on afterwards.
    asked_after: Vec<usize>,
    handed_over: usize,
}

impl Default for SimWatchdog {
    /// A machine that starts the process and answers every probe — the arrangement
    /// with nothing wrong, so a test names only the thing it is about.
    fn default() -> Self {
        Self {
            now: Instant::default(),
            starts: Some(Instant::default()),
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
            asked_after: Vec::new(),
            handed_over: 0,
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

    /// Somebody asks for the recording once this many probes have gone out.
    pub fn asked_for_the_trace_after(mut self, probes: usize) -> Self {
        self.asked_after.push(probes);
        self
    }

    /// How many times it was handed over.
    pub fn handed_the_trace_over(&self) -> usize {
        self.handed_over
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
        let deadline = advance(self.now, patience);

        if let Some(due) = self.owed.pop_front_if(|due| *due <= deadline) {
            self.now = due.max(self.now);
            return Beat {
                at: self.now,
                kind: BeatKind::Beat,
            };
        }

        // The wait consumed all of it, which is what a silence is.
        self.now = deadline;
        Beat {
            at: self.now,
            kind: BeatKind::Silent,
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
            self.owed.push_back(advance(self.now, self.round_trip));
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
        self.now = advance(self.now, how_long);
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

    /// True on the way round after the scripted number of probes, once each.
    ///
    /// Taken out of the script rather than left in it, because a real ask is one
    /// somebody made once: a fact that stayed true would have the run hand the
    /// recording over on every way round from then on.
    fn asked_for_the_trace(&mut self) -> bool {
        let sent = self.probes.len();
        match self.asked_after.iter().position(|after| *after == sent) {
            Some(at) => {
                self.asked_after.remove(at);
                true
            }
            None => false,
        }
    }

    fn hand_the_trace_over(&mut self) {
        self.did.push(Told::HandedTheTraceOver);
        self.handed_over += 1;
    }

    fn warn(&mut self, message: core::fmt::Arguments) {
        self.warnings.push(message.to_string());
    }
}

/// A local network that answers, or does not, when asked who is offering a
/// service.
///
/// The answers are written here as the bytes a responder would put on the wire,
/// with a writer of its own rather than by calling the reader's helpers: an encoder
/// and a decoder that share their arithmetic agree with each other whether or not
/// either agrees with the format.
#[derive(Debug, Default, Clone)]
pub struct SimDiscovery {
    asked: Vec<Vec<u8>>,
    answers: VecDeque<Vec<u8>>,
    now: Instant,
    timeout: Option<Duration>,
}

impl SimDiscovery {
    pub fn new() -> Self {
        Self::default()
    }

    /// A machine offering `service`, on `port`.
    ///
    /// `service` is the caller's to state rather than this crate's to know:
    /// `engine::link::SERVICE`/`PAIRING` are two names for what a sink offers, and
    /// a scripted network answering for the wrong one should look exactly like a
    /// machine that never offered it at all.
    pub fn advertises(
        &mut self,
        service: &str,
        instance: &str,
        port: u16,
        address: Option<[u8; 4]>,
    ) -> &mut Self {
        self.advertises_service(&discovery::service(service), instance, port, address)
    }

    /// The same, already qualified — for a network with other things on it.
    pub fn advertises_service(
        &mut self,
        service: &str,
        instance: &str,
        port: u16,
        address: Option<[u8; 4]>,
    ) -> &mut Self {
        let full = format!("{instance}.{service}");
        let mut message = Answer::new(if address.is_some() { 3 } else { 2 });
        message.record(service, 12, &name(&full));
        let mut srv = vec![0, 0, 0, 0];
        srv.extend_from_slice(&port.to_be_bytes());
        srv.extend_from_slice(&name("the-mac.local"));
        message.record(&full, 33, &srv);
        if let Some(address) = address {
            message.record("the-mac.local", 1, &address);
        }
        self.answers_with(message.done())
    }

    /// A responder that names the instance and never says which port.
    pub fn advertises_without_a_port(&mut self, service: &str, instance: &str) -> &mut Self {
        let full = format!("{instance}.{}", discovery::service(service));
        let mut message = Answer::new(1);
        message.record(&discovery::service(service), 12, &name(&full));
        self.answers_with(message.done())
    }

    /// An answer whose first name is a pointer to itself.
    pub fn answers_with_a_name_that_points_at_itself(&mut self) -> &mut Self {
        let mut message = Answer::new(1);
        message.record("anything.local", 12, &[0]);
        let mut bytes = message.done();
        bytes[12] = 0xC0;
        bytes[13] = 12;
        self.answers_with(bytes)
    }

    /// An answer as bytes, for a test about bytes.
    pub fn answers_with(&mut self, message: Vec<u8>) -> &mut Self {
        self.answers.push_back(message);
        self
    }

    /// The last answer scripted, for a test that wants to cut it short.
    pub fn advertisement(&self) -> &[u8] {
        self.answers.back().map(Vec::as_slice).unwrap_or_default()
    }

    /// Every question that went out.
    pub fn asked(&self) -> &[Vec<u8>] {
        &self.asked
    }

    fn start_clock(&mut self) -> Instant {
        self.now = Instant::default();
        self.now
    }

    fn now(&mut self) -> Instant {
        self.now
    }

    fn ask(&mut self, question: &[u8]) -> bool {
        self.asked.push(question.to_vec());
        true
    }

    fn set_timeout(&mut self, timeout: Duration) -> bool {
        self.timeout = Some(timeout);
        true
    }

    fn receive(&mut self, into: &mut [u8]) -> Datagram {
        match self.answers.pop_front() {
            Some(answer) if answer.len() <= into.len() => {
                into[..answer.len()].copy_from_slice(&answer);
                Datagram::Received(answer.len())
            }
            Some(_) => Datagram::Failed,
            None => {
                if let Some(timeout) = self.timeout {
                    self.now = advance(self.now, timeout);
                }
                Datagram::TimedOut
            }
        }
    }
}

/// A name as the labels a message carries it as.
///
/// [`discovery::write_name`] and not a copy of it: this machine is standing in for
/// a responder, and a second encoder here would be a second format to keep in step
/// with the one [`discovery::answer`] reads.
fn name(name: &str) -> Vec<u8> {
    let mut out = Vec::new();
    discovery::write_name(&mut out, name);
    out
}

/// What a source pairing run asked of its host, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourcePairingCall {
    Connected,
    SetReadTimeout,
    SetWriteTimeout,
    SentOffer,
    FlushedOffer,
    TookAnswer,
    SentSealedKey,
    FlushedSealedKey,
    TookSealedKey,
    MadeSinkDirectory,
    PinnedSink,
}

/// A machine standing in for the one a code is read off, from this end.
///
/// The mirror of [`SimPairing`], and the exchange really runs here too: the sink is
/// scripted with the code it is showing and the key it will present, and it answers
/// with the same function its own run would call. So digits that are not the ones it
/// is showing end in nothing opening, decided by the arithmetic rather than by
/// comparing two codes (ADR-0007).
#[derive(Default)]
pub struct SimSourcePairing(Rc<RefCell<Offered>>);

/// What one attempt from this end is made of, shared with the connection it
/// hands over, for the reason [`Attempt`] is.
struct Offered {
    entropy: Trickle,
    /// The sink to be reached, if there is one.
    sink: Option<Sink>,
    /// Whether the connection goes after the answer has been sent.
    stops_after_the_answer: bool,
    /// Whether what the sink answers with is bytes the exchange cannot read.
    answers_nonsense: bool,
    /// How many more of the exchange's own calls this connection will carry, for
    /// the same reason [`SimPairing`] keeps one.
    carries: Option<usize>,
    /// The sink's half of the exchange, once an offer has reached it.
    answered: Option<[u8; OFFER]>,
    /// What the sink agreed on, which for a wrong code is not what this end has.
    agreed: Option<Secret>,
    /// What was sent to the sink, as the sink could read it.
    gave: Option<Vec<u8>>,
    pinned: Option<String>,
    keeps: bool,
    offered_before_waiting: bool,
    took_the_key_before_pinning: bool,
    took_the_key: bool,
    calls: Vec<SourcePairingCall>,
    fails_at: Option<SourcePairingCall>,
}

/// The sink waiting to be reached, as the script describes it.
#[derive(Debug, Clone)]
struct Sink {
    code: Code,
    key: Vec<u8>,
}

impl Default for Offered {
    /// A machine with nothing to connect to, which is what a Mac showing no code
    /// looks like from here.
    fn default() -> Self {
        Self {
            entropy: Trickle::new(0x5011_2ce0),
            sink: None,
            stops_after_the_answer: false,
            answers_nonsense: false,
            carries: None,
            answered: None,
            agreed: None,
            gave: None,
            pinned: None,
            keeps: true,
            offered_before_waiting: false,
            took_the_key_before_pinning: false,
            took_the_key: false,
            calls: Vec::new(),
            fails_at: None,
        }
    }
}

impl SimSourcePairing {
    pub fn new() -> Self {
        Self::default()
    }

    /// A sink that is showing this code and will present this key.
    pub fn with_a_sink_that_knows(self, code: Code, key: &[u8]) -> Self {
        self.0.borrow_mut().sink = Some(Sink {
            code,
            key: key.to_vec(),
        });
        self
    }

    /// A connection that goes after the answer, part way through the exchange.
    pub fn that_stops_after_the_answer(self) -> Self {
        self.0.borrow_mut().stops_after_the_answer = true;
        self
    }

    /// A sink that stops answering once it has carried this many of the
    /// exchange's calls.
    pub fn that_goes_quiet_after(self, calls: usize) -> Self {
        self.0.borrow_mut().carries = Some(calls);
        self
    }

    /// A machine whose entropy will not produce this end's half of the exchange.
    ///
    /// The source needs bytes of its own even though the digits came from the
    /// other machine: the offer is a keypair this end makes for the one exchange.
    pub fn with_no_entropy(self) -> Self {
        self.0.borrow_mut().entropy = Trickle::dry();
        self
    }

    /// A sink whose answer is not one: bytes of the right length that the
    /// exchange cannot read, which is what a machine speaking a different version
    /// of it sounds like from here.
    pub fn with_a_sink_that_answers_with_nonsense(self) -> Self {
        self.0.borrow_mut().answers_nonsense = true;
        self
    }

    /// A machine that cannot write the pinned key down.
    pub fn that_cannot_keep_the_sink(self) -> Self {
        self.0.borrow_mut().keeps = false;
        self
    }

    pub fn whose_pairing_fails_at(self, call: SourcePairingCall) -> Self {
        self.0.borrow_mut().fails_at = Some(call);
        self
    }

    pub fn pairing_calls(&self) -> Vec<SourcePairingCall> {
        self.0.borrow().calls.clone()
    }

    /// The text this end wrote as the file naming its sink, if it wrote one.
    ///
    /// The text and not the key it names: what the file says is what the other
    /// machine's own list is written in, so a test reading it back with the
    /// parser that produced it would be checking that parser against itself.
    pub fn pinned_text(&self) -> Option<String> {
        self.0.borrow().pinned.clone()
    }

    /// What the sink ended up holding, if it could open what it was sent.
    pub fn gave_the_sink(&self) -> Option<Vec<u8>> {
        self.0.borrow().gave.clone()
    }

    pub fn offered_before_waiting(&self) -> bool {
        self.0.borrow().offered_before_waiting
    }

    /// Whether the sink's key had arrived before anything was written down.
    pub fn took_the_key_before_pinning(&self) -> bool {
        self.0.borrow().took_the_key_before_pinning
    }
}

impl Offered {
    /// What a machine that will not keep this scripts back, for the reason
    /// [`Attempt::would_keep`] answers the same way at the other end.
    fn would_keep(&self, call: SourcePairingCall) -> Result<(), Trouble> {
        match self.keeps && self.fails_at != Some(call) {
            true => Ok(()),
            false => Err(Trouble(format!("this machine refuses {call:?}"))),
        }
    }

    /// Whether this connection carries one more of the exchange's calls, spending
    /// the budget if it does.
    fn carries_one_more(&mut self) -> bool {
        match &mut self.carries {
            None => true,
            Some(0) => false,
            Some(left) => {
                *left -= 1;
                true
            }
        }
    }
}

impl Entropy for SimSourcePairing {
    fn fill(&mut self, into: &mut [u8]) -> bool {
        self.0.borrow_mut().entropy.fill(into)
    }
}

impl Entropy for Offered {
    fn fill(&mut self, into: &mut [u8]) -> bool {
        self.entropy.fill(into)
    }
}

/// The one connection this attempt runs over, as the run holds it.
struct SimOffer(Rc<RefCell<Offered>>);

impl Entropy for SimOffer {
    fn fill(&mut self, into: &mut [u8]) -> bool {
        self.0.borrow_mut().entropy.fill(into)
    }
}

impl favjit_host::pairing::Offering for SimOffer {
    fn set_read_timeout(&mut self, timeout: Duration) -> bool {
        self.0.borrow_mut().set_read_timeout(timeout)
    }

    fn set_write_timeout(&mut self, timeout: Duration) -> bool {
        self.0.borrow_mut().set_write_timeout(timeout)
    }

    fn send_offer(&mut self, offer: &[u8]) -> bool {
        self.0.borrow_mut().send_offer(offer)
    }

    fn flush_offer(&mut self) -> bool {
        self.0.borrow_mut().flush_offer()
    }

    fn take_answer(&mut self, into: &mut [u8]) -> bool {
        self.0.borrow_mut().take_answer(into)
    }

    fn send_sealed_key(&mut self, sealed: &[u8]) -> bool {
        self.0.borrow_mut().send_sealed_key(sealed)
    }

    fn flush_sealed_key(&mut self) -> bool {
        self.0.borrow_mut().flush_sealed_key()
    }

    fn take_sealed_key(&mut self, into: &mut [u8]) -> bool {
        self.0.borrow_mut().take_sealed_key(into)
    }
}

impl SourcePairingHost for SimSourcePairing {
    fn connect(&mut self, _timeout: Duration) -> Option<Box<dyn favjit_host::pairing::Offering>> {
        match self.0.borrow_mut().connect() {
            true => Some(Box::new(SimOffer(Rc::clone(&self.0)))),
            false => None,
        }
    }

    fn make_sink_directory(&mut self) -> Result<(), Trouble> {
        self.0.borrow_mut().make_sink_directory()
    }

    fn pin_sink(&mut self, text: &str) -> Result<(), Trouble> {
        self.0.borrow_mut().pin_sink(text)
    }
}

impl Offered {
    fn connect(&mut self) -> bool {
        self.calls.push(SourcePairingCall::Connected);
        self.fails_at != Some(SourcePairingCall::Connected) && self.sink.is_some()
    }

    fn set_read_timeout(&mut self, _timeout: Duration) -> bool {
        self.calls.push(SourcePairingCall::SetReadTimeout);
        self.fails_at != Some(SourcePairingCall::SetReadTimeout)
    }

    fn set_write_timeout(&mut self, _timeout: Duration) -> bool {
        self.calls.push(SourcePairingCall::SetWriteTimeout);
        self.fails_at != Some(SourcePairingCall::SetWriteTimeout)
    }

    fn send_offer(&mut self, offer: &[u8]) -> bool {
        self.calls.push(SourcePairingCall::SentOffer);
        if self.fails_at == Some(SourcePairingCall::SentOffer) || !self.carries_one_more() {
            return false;
        }
        let Ok(offer): Result<[u8; OFFER], _> = offer.try_into() else {
            return false;
        };
        let Some(code) = self.sink.as_ref().map(|sink| sink.code) else {
            return false;
        };
        // The sink's half, under the digits it is showing rather than the ones this
        // end offered: that is where a wrong code stops being indistinguishable from
        // a right one.
        let Some((answer, agreed)) = favjit_pairing_exchange::answer(code, &offer, self) else {
            return false;
        };
        self.answered = Some(answer);
        self.agreed = Some(agreed);
        true
    }

    fn flush_offer(&mut self) -> bool {
        self.calls.push(SourcePairingCall::FlushedOffer);
        self.fails_at != Some(SourcePairingCall::FlushedOffer)
    }

    fn take_answer(&mut self, into: &mut [u8]) -> bool {
        self.calls.push(SourcePairingCall::TookAnswer);
        self.offered_before_waiting = self.answered.is_some();
        if self.fails_at == Some(SourcePairingCall::TookAnswer) || !self.carries_one_more() {
            return false;
        }
        match self.answered {
            Some(answer) => {
                match self.answers_nonsense {
                    true => into.fill(0xff),
                    false => into.copy_from_slice(&answer),
                }
                true
            }
            None => false,
        }
    }

    fn send_sealed_key(&mut self, sealed: &[u8]) -> bool {
        self.calls.push(SourcePairingCall::SentSealedKey);
        if self.fails_at == Some(SourcePairingCall::SentSealedKey) || !self.carries_one_more() {
            return false;
        }
        let Ok(sealed): Result<[u8; SEALED_KEY], _> = sealed.try_into() else {
            return false;
        };
        // Opened rather than recorded as sent, for the reason it is at the other end:
        // what the sink holds is what it could read.
        self.gave = self
            .agreed
            .as_ref()
            .and_then(|agreed| agreed.open(&sealed, Side::Source));
        true
    }

    fn flush_sealed_key(&mut self) -> bool {
        self.calls.push(SourcePairingCall::FlushedSealedKey);
        self.fails_at != Some(SourcePairingCall::FlushedSealedKey)
    }

    fn take_sealed_key(&mut self, into: &mut [u8]) -> bool {
        self.calls.push(SourcePairingCall::TookSealedKey);
        // The connection going here is not a wrong code, and the run has to be able
        // to tell them apart.
        if self.fails_at == Some(SourcePairingCall::TookSealedKey)
            || self.stops_after_the_answer
            || !self.carries_one_more()
        {
            return false;
        }
        let Some(key) = self.sink.as_ref().map(|sink| sink.key.clone()) else {
            return false;
        };
        let Some(sealed) = self
            .agreed
            .as_ref()
            .and_then(|agreed| agreed.seal(&key, Side::Sink))
        else {
            return false;
        };
        self.took_the_key = true;
        into.copy_from_slice(&sealed);
        true
    }

    fn make_sink_directory(&mut self) -> Result<(), Trouble> {
        self.calls.push(SourcePairingCall::MadeSinkDirectory);
        self.would_keep(SourcePairingCall::MadeSinkDirectory)
    }

    fn pin_sink(&mut self, text: &str) -> Result<(), Trouble> {
        self.calls.push(SourcePairingCall::PinnedSink);
        self.took_the_key_before_pinning = self.took_the_key;
        self.would_keep(SourcePairingCall::PinnedSink)?;
        self.pinned = Some(String::from(text));
        Ok(())
    }
}

/// A response being written, one record at a time.
struct Answer(Vec<u8>);

impl Answer {
    fn new(records: u16) -> Self {
        let mut out = Vec::new();
        out.extend_from_slice(&0u16.to_be_bytes());
        // A response, with the bit a responder sets to say it is authoritative.
        out.extend_from_slice(&0x8400u16.to_be_bytes());
        out.extend_from_slice(&0u16.to_be_bytes());
        out.extend_from_slice(&records.to_be_bytes());
        out.extend_from_slice(&0u16.to_be_bytes());
        out.extend_from_slice(&0u16.to_be_bytes());
        Self(out)
    }

    fn record(&mut self, owner: &str, kind: u16, data: &[u8]) {
        self.0.extend_from_slice(&name(owner));
        self.0.extend_from_slice(&kind.to_be_bytes());
        self.0.extend_from_slice(&1u16.to_be_bytes());
        self.0.extend_from_slice(&10u32.to_be_bytes());
        self.0.extend_from_slice(&(data.len() as u16).to_be_bytes());
        self.0.extend_from_slice(data);
    }

    fn done(self) -> Vec<u8> {
        self.0
    }
}
