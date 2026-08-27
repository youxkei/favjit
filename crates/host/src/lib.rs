//! The boundary between a role's loop and the machine it runs on (ADR-0006).
//!
//! Definitions and nothing else — no function, no method, no `impl` written by
//! hand. Every host crate depends on this one and none of them depends on
//! `engine`, which is what makes ADR-0006's rule a thing a compiler holds rather
//! than a thing a reader has to: an operation here is one call into a platform,
//! and a decision has nowhere to be borrowed from ([ADR-0005](../../../docs/adr/0005-crate-layout.md)).
//!
//! So the types below carry what a platform said, in the shape it said it. A
//! usage number is a usage number and a return code is a return code; what either
//! one *means* is named in `engine`, where the end-to-end suite drives the naming.

/// A point on the host's monotonic clock, in nanoseconds from an origin the host
/// picked.
///
/// The origin is nobody's business but the machine's: what reads these compares
/// two of them, and a value that meant something on its own would be a clock
/// `engine` could read without the event stream saying so (ADR-0009).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Instant {
    pub nanos: u64,
}

/// What one datagram receive call reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Datagram {
    /// One whole datagram was copied into the supplied buffer.
    Received(usize),
    /// The socket's read timeout elapsed.
    TimedOut,
    /// The call failed for another reason.
    Failed,
}

/// Where an mDNS answer said a service can be reached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Discovered {
    /// The machine's own name, as its service record gives it.
    pub host: String,
    pub port: u16,
    /// Its IPv4 address, when the responder included it in the answer.
    pub address: Option<[u8; 4]>,
}

/// Where the one machine a run has settled on reaching can be reached.
///
/// Carried by the run from the moment it was resolved to the moment a
/// connection is asked for, rather than kept by the host in between: a host
/// that held it would have to be asked whether it has one before every use of
/// it, and asking is a turning beside the call it makes (ADR-0006).
///
/// The address itself and not a shape of this crate's own, because there is
/// nothing here to convert: what a machine is reached at is the same value on
/// both platforms, and a type in between would be two conversions per run for
/// no fact either host reads.
pub type Sink = std::net::SocketAddr;

/// What the machine said about a call that did not work.
///
/// The platform's own words about its own failure, carried rather than acted
/// on: which failure it was decides nothing in a host, and the sentence a
/// person reads is composed where the suite can read it too — so this is the
/// call's return value reshaped and nothing besides (ADR-0006).
///
/// A string because that is the only shape every one of these has in common: a
/// `std::io::Error`, a `GetLastError` code and an `IOReturn` are three
/// different things, and what they share is being the machine's account of
/// itself. Naming which of them it was would be the decision this exists to
/// keep out.
/// The account itself is the whole of the type, and reading it is `engine`'s:
/// a `Display` written here would be code in the crate that holds none
/// (ADR-0006), and it would be this crate deciding how a machine's own words
/// read in a sentence favjit composed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trouble(pub String);

/// A keyboard, as the host chose to number it.
///
/// Opaque on purpose: nothing outside a host may derive meaning from the value, so
/// a host is free to hand out whatever its capture API gives it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DeviceId(pub u64);

/// Something a platform call handed over, carried back to the next one.
///
/// Opaque in use and not in the type, which is the most a crate holding no code
/// can be: what it stands for is one machine's own — a registry entry, a queue
/// over a device, a window — and a run has nothing it could do with the number
/// but hand it back. That is the whole of what it is for. A sequence of calls
/// over one thing can then be driven from `engine`, where a test reaches it,
/// while every one of those calls stays behind the boundary (ADR-0006).
///
/// A number and not a trait object per handle: a handle answers no question of
/// its own, so there is nothing to ask it — every question about one is a call
/// the host makes, and those are the operations already on these traits.
///
/// It is not [`DeviceId`], and the two are never mixed: that one is the number
/// the *run* gave a keyboard and is what everything above the boundary names it
/// by, while this is the machine's own and means nothing outside the host that
/// handed it over.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Handed(pub u64);

/// Something that happened outside, and when.
///
/// One stream, not one per concern: ADR-0006 makes each role a single loop, and
/// that is what keeps the end-to-end suite deterministic.
///
/// The timestamp rides on the event rather than being read from a clock when
/// wanted. That is what lets a rule distinguish a tap from a hold while a role
/// stays a function of its event stream alone.
#[derive(Debug, Clone, PartialEq)]
pub struct HostEvent {
    pub at: Instant,
    pub kind: EventKind,
}

/// What happened, as the platform reported it.
///
/// Nothing here is named: a page and a usage are the numbers IOKit handed over, a
/// scancode is what raw input handed over, and a record is the bytes that came off
/// a socket. Naming any of them is a table, and a table beside the API that
/// produced its input is a decision no test can drive (ADR-0006).
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum EventKind {
    /// A HID device the host has found, described by the properties it carries.
    ///
    /// The properties in the shape IOKit gives them, and nothing read off them:
    /// whether this is a keyboard at all, whether it is the machine's own, and
    /// what USB identity it has are each a table, and a table beside the API
    /// that produced its input is a decision no test can drive (ADR-0006). So
    /// every device found is reported, including the ones a run will decline to
    /// take.
    ///
    /// Each is absent where the device carries no such property, which is a
    /// device that cannot be singled out by whatever reads it rather than one to
    /// leave out.
    HidDeviceFound {
        device: DeviceId,
        /// `PrimaryUsagePage` and `PrimaryUsage`, which name a device class.
        primary_usage_page: Option<i64>,
        primary_usage: Option<i64>,
        /// `Transport`, the bus the device sits on.
        transport: Option<String>,
        /// `Product`, the name the device gives itself.
        product: Option<String>,
        /// `VendorID` and `ProductID`, as the numbers IOKit holds them in.
        vendor_id: Option<i64>,
        product_id: Option<i64>,
    },
    /// A device the host has found, named by the interface path it gave.
    ///
    /// The path and nothing read out of it: what a vendor and a product look
    /// like written into one is a format, and the path is the only place raw
    /// input puts them for a keyboard (ADR-0006).
    PathDeviceFound { device: DeviceId, path: String },
    /// A keyboard that has gone away.
    DeviceLost(DeviceId),
    /// One HID element value, as the device's queue handed it over.
    ///
    /// `value` is the integer the element carries, whatever its range: a key
    /// reports 0 or 1 and a pointer axis reports a delta, and which of those this
    /// is depends on the page and usage.
    HidValue {
        device: DeviceId,
        page: u32,
        usage: u32,
        /// The device's own timestamp, which is what says where one report ends:
        /// every value of one HID report carries the same one.
        stamp: u64,
        value: i64,
    },
    /// That device's queue has run dry.
    ///
    /// Its own event because a queue running dry is as certain a report boundary
    /// as the stamp changing, and it is the only one the last report of a burst
    /// gets.
    HidValuesDone { device: DeviceId },
    /// One `KBDLLHOOKSTRUCT`, as the hook that refused it reported it.
    ///
    /// The structure's own fields, undecoded. Which prefix a make code sits
    /// behind, whether the transition is a release, and whether the report is a
    /// key at all are a table — the pause key reports left control's make code,
    /// right shift reports an extended bit it does not have, and a keyboard
    /// sends a filler event beside an extended key — and a table beside the API
    /// that produced its input is a decision no test can drive (ADR-0006).
    HookedKey {
        device: DeviceId,
        /// `scanCode`, the position on the keyboard.
        make_code: u16,
        /// `vkCode`, which is part of how the position reads.
        vkey: u16,
        /// `flags`, carrying the prefix and the transition.
        flags: u32,
    },
    /// One `RAWMOUSE`, as raw input reported it.
    ///
    /// The fields Windows' own structure carries, undecoded: which of
    /// `button_flags`'s bits are set is a button transition, and `button_data`
    /// is a wheel's turn only when the matching bit is — deciding which is
    /// naming this report the same way a page and a usage are (ADR-0006).
    MouseReport {
        device: DeviceId,
        flags: u16,
        button_flags: u16,
        button_data: u16,
        dx: i32,
        dy: i32,
    },
    /// One plaintext frame that arrived over the link, at the number the session
    /// it came in on had it under.
    ///
    /// The number rides on the event because the loop that opened the record is
    /// not the loop that handles it — one turns alongside the other (ADR-0006) —
    /// and a count read again on the far side of that stream would be the count
    /// of a later record. It is the transport's own, so the sending end names the
    /// same record by the same number, which is what lets the two machines'
    /// recordings be merged (ADR-0009).
    Record { at: u64, frame: Vec<u8> },
    /// One of the other machine's own trace records, as it wrote it.
    ///
    /// On the stream because the loop that took it off the link is not the loop
    /// that keeps the recording — one turns alongside the other (ADR-0006) — and
    /// the bytes cross unread: what a record means belongs to the recording, and
    /// re-encoding one could only differ from what the machine that made it said
    /// (ADR-0009). No host makes one of these.
    Recorded(Vec<u8>),
    /// The loop serving the link has let the connection go, for the reason this
    /// number stands for.
    ///
    /// On the stream because the loop that decided it is not the loop that ends
    /// the run — one turns alongside the other (ADR-0006) — and because a
    /// recording is made of the stream: a reason only written to a log is one the
    /// other machine's recording cannot be read beside (ADR-0009). What the
    /// number means is `engine`'s, named where its other words are; no host
    /// makes one of these.
    LinkEnded { why: u32 },
    /// One frame the loop serving the output device put on its connection, and
    /// what the machine said about the write — `0` for the one that went.
    ///
    /// On the stream for [`EventKind::LinkEnded`]'s reason: the loop that wrote
    /// it is not the loop that keeps the recording. What `frame` stands for is
    /// `engine`'s ([`favjit_engine::output::Served`], where its other words
    /// are); no host makes one of these.
    OutputFrame { frame: u32, code: i32 },
    /// The loop serving the output device has come back, for the reason this
    /// number stands for.
    ///
    /// Likewise `engine`'s ([`favjit_engine::output::Lost`]). A recording that
    /// held only the injections would show every one of those reasons the same
    /// way — reports written successfully until they stop — so this is what
    /// separates them (ADR-0009).
    OutputEnded { why: u32 },
    /// The supervising watchdog asking whether the loop is still turning
    /// (ADR-0008).
    ///
    /// A kind of its own rather than a reserved key, so that nothing arriving on
    /// this path can become an injected keystroke however the tables are written.
    Probe,
    /// Somebody has asked for the keyboard to be driving this machine or the other
    /// one, by some means other than the chord (ADR-0013).
    ///
    /// On this stream and not a call a run makes, because a run's one wait is
    /// [`Host::next_event`]: an ask that had to be polled for would be seen on the
    /// next keystroke, and having no usable keyboard is when it is asked.
    ///
    /// It names the state asked for rather than saying "the other one", so that a
    /// stale menu drawn before the chord moved the keyboard cannot move it back by
    /// being clicked.
    Asked(source::Driving),
    /// Somebody outside this process has asked the run to stop.
    ///
    /// On the stream for [`EventKind::Asked`]'s reason: a run's one wait is
    /// [`Host::next_event`], and an ask that had to be polled for would be seen
    /// on the next keystroke. Behind everything that arrived before it, which is
    /// what makes those keystrokes the run's to finish with rather than a
    /// backlog a flag read at the top of a wait would drop.
    AskedToStop,
    /// The loop the run handed over to turn alongside its own has come back.
    ///
    /// On the stream rather than a fact to ask for afterwards, so that it lands
    /// behind everything that loop delivered: the run is the end that knows
    /// whether it has one, and a flag read at the top of a wait would end the run
    /// with those events still in the channel.
    AlongsideStopped,
    /// Nothing more will arrive on this stream, ever.
    ///
    /// The reading having stopped for good, which is not the same as a wait
    /// coming back with nothing: that is the ordinary state of a keyboard nobody
    /// is touching, and only this says it is anything else.
    InputGone,
    /// How long ago, in nanoseconds, the platform stamped a value the capture has
    /// just reached.
    ///
    /// Its own event rather than a field on the value: the stamp is on a clock
    /// only that thread can read, so both ends of the measurement are there, and
    /// what crosses is the one number it came to. Sent per value rather than
    /// summarised, because the summary is arithmetic and belongs where nothing
    /// is waiting on it.
    Delay(u64),
}

/// Why a machine has no more events to give.
///
/// Facts the platform observed, not what to do about them: every one of these ends
/// a run, and which it was decides only what is said about it.
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

/// Which of the output device's reports some bytes go on.
///
/// A name for the report rather than the number the platform posts it under,
/// because the number is the platform's and the choice of report is not: which
/// page a control belongs on is what a report is *made of*, and it is settled
/// before there are any bytes to write.
///
/// Closed, unlike the enums a host reads: a host turns each of these into the
/// number its own device posts it under, and left open it would need an answer
/// for a report it has no number for as well — which is a host deciding what to
/// do about bytes it cannot send. Adding one here is meant to stop a host
/// compiling until it says where the report goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputReport {
    Keyboard,
    Consumer,
    AppleVendorTopCase,
    AppleVendorKeyboard,
    GenericDesktop,
    Pointing,
}

/// What every role needs of the machine it runs on, whatever it does with the
/// input.
///
/// The stream events arrive on, and the two ways a process reports outwards. Each
/// role's own boundary builds on this one, and a run given one of those cannot
/// reach past it — which is how it is said, in the types, that a run which only
/// watches produces no keystroke.
///
/// Not `take_input`, which both roles have and which is not one operation: the
/// sink seizes the keyboards as it asks, and the source only becomes *able* to
/// refuse them, later and per event. Not `ended` either — the two roles end for
/// different reasons, and the enums naming them share one variant out of five.
pub trait Host {
    /// What this machine's clock reads right now.
    ///
    /// The one thing `engine` cannot compute for itself before calling
    /// [`Host::next_event`]: a deadline is a comparison of instants, and
    /// comparing is `engine`'s, but the instant *now* is only ever this
    /// machine's to report.
    fn now(&mut self) -> Instant;

    /// Block until the next thing happens outside, or until `deadline`, or return
    /// `None` if nothing has by then.
    ///
    /// `deadline` is `engine`'s, already the one instant there is by the time it
    /// reaches here — whichever wake-up, run bound, or [`Host::now`]-derived
    /// polling instant was soonest is a comparison `engine` made itself, before
    /// this was called, not a second thing this operation decides. Converting it
    /// into whatever unit the platform's own wait takes, against this machine's
    /// own clock, is this call's only job. `None` here says nothing about
    /// whether more will ever arrive — only that none has by the deadline given.
    ///
    /// **One instant and never the absence of one.** A wait with no end is one a
    /// host has to reach for a second platform call to express, and which of the
    /// two it reaches for is then this operation's choice rather than the run's.
    /// So a run that wants to wait indefinitely says so the way it says
    /// everything else about time: a bound it sets, and a loop of its own that
    /// comes back round. Whether anything is ever coming is said by
    /// [`EventKind::InputGone`] arriving on it, and not asked for: `None` says
    /// only that nothing arrived by the deadline, which is the ordinary state of
    /// a keyboard nobody is touching, and the reading having stopped for good is
    /// one more thing that happened.
    fn next_event(&mut self, deadline: Instant) -> Option<HostEvent>;

    /// Whether something is watching this process (ADR-0008).
    ///
    /// Asked rather than assumed, because it decides what a run that holds the
    /// keyboards is: supervised, a wedge ends the process and gives them back;
    /// unsupervised, nothing does.
    fn is_supervised(&mut self) -> bool;

    /// Put a line where this machine's log goes.
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
    /// Must not block. A supervisor that has gone away must not be able to stop
    /// the process that is holding the keyboard.
    ///
    /// Reports whether it got through, and what the machine said when it did
    /// not: a beat that fails silently looks like a healthy loop from in here
    /// and like a wedged one from the watchdog, so the process holding the
    /// keyboards would be ended for a broken pipe rather than for a fault of
    /// its own. Whether that is worth saying, and whether it is worth saying
    /// more than once, is the run's (ADR-0006: how often it is said).
    fn heartbeat(&mut self) -> Result<(), crate::Trouble>;
}

/// Bytes nobody can predict, from wherever this machine keeps them.
///
/// A supertrait of the boundaries that need it rather than an operation on each,
/// so that a construction driven step by step can be handed the machine it is
/// being driven against and take its randomness from there (ADR-0012).
pub trait Entropy {
    /// Fill `into` completely, or report that this machine would not.
    fn fill(&mut self, into: &mut [u8]) -> bool;
}

/// Where this machine's own key pair is kept.
///
/// Bytes both ways: what a key pair *is* — the halves, their lengths, whether two
/// of them belong together — is settled where the suite can drive it, and a store
/// that read a file and answered "no identity" would be settling it in a host.
pub trait ControlStore {
    /// Make the directory the control file belongs in.
    fn make_directory(&mut self) -> std::io::Result<()>;

    /// Write the file whose presence means converting is disabled.
    fn write_disabled(&mut self) -> std::io::Result<()>;
}

pub trait IdentityStore {
    /// What is in the file, or nothing when there is no file to read.
    fn read(&mut self) -> Option<Vec<u8>>;

    /// Make the directory the identity file belongs in.
    ///
    /// Reports what the machine said about a failure rather than only that
    /// there was one: a person whose identity cannot be kept needs to know
    /// whether it is a permission or a full disk, and saying so is `engine`'s
    /// (ADR-0006).
    fn make_directory(&mut self) -> Result<(), crate::Trouble>;

    /// Open the identity file for a private, replacing write, and hand it over.
    ///
    /// The open file rather than nothing, so that writing is asked of something
    /// that is open: an operation on a file that may not have been opened has to
    /// ask whether it was, and asking is a turning beside the call it makes
    /// (ADR-0006).
    fn open(&mut self) -> Result<Box<dyn Writing>, crate::Trouble>;
}

/// One open file, for as long as there is something to put in it.
pub trait Writing {
    /// Write these bytes to it.
    fn write(&mut self, bytes: &[u8]) -> Result<(), crate::Trouble>;
}

/// Why the device converted keystrokes go out through did not come up
/// (`docs/platform/macos/output-through-a-virtual-hid-device.md`).
///
/// Facts the service reported, and nothing about what to do with them: the
/// socket refusing and the service never reporting a ready keyboard need
/// different things done about them, and the sentence a person reads is written
/// where the suite can read it too (ADR-0006).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum NoOutput {
    /// Nothing answered where the service listens.
    ///
    /// No error code with it: the socket is root-only by directory permission,
    /// so the answer is the privilege or the package, whatever the code says.
    NoService,
    /// It answered, and no keyboard arrived inside the wait it was given.
    ///
    /// The three things the service ever says about itself, because a wrong
    /// protocol version is silently ignored and a timeout is the only other
    /// signal it gives.
    NotReady {
        driver_activated: bool,
        driver_connected: bool,
        version_mismatched: bool,
    },
}

/// The machine's pointing devices, for setting the feel of the one converted
/// input comes out through (ADR-0011).
///
/// Its own boundary rather than part of the sink's, for the reason
/// [`DiscoveryHost`] is: trying a number costs nothing where it can be done
/// without bringing a whole run up, so the same sequence is driven from a
/// diagnostic mode that has no keyboards and no output.
pub trait PointerHost {
    /// What this machine was told the output pointer should feel like, as counts
    /// per inch and an acceleration factor.
    ///
    /// Read off the machine rather than handed to it: they are a property of a
    /// device on a machine, settled once by feel, so a host reads them from
    /// wherever it was told to. What is done with them — which device they are
    /// written to, and in what order — is `engine`'s, the way entropy supplied
    /// at a step is (ADR-0006).
    fn wanted_pointer_feel(&mut self) -> (Option<f64>, Option<f64>);

    /// The vendor id the device this run's output goes through enumerates with.
    ///
    /// Reported rather than compared here: which of the machine's pointing
    /// devices is favjit's own is the comparison that decides what gets tuned,
    /// and a host that made it would be tuning wherever nothing could drive it.
    fn output_vendor(&mut self) -> i64;

    /// Open the event system's own view of this machine, and hand it over.
    ///
    /// The view rather than a `bool`, so that reading the devices is asked of
    /// something that is there: a read against a view that may not have opened
    /// has to ask whether it did, and asking is a turning beside the call it
    /// makes (ADR-0006).
    fn open_event_system(&mut self) -> Option<Box<dyn Pointers>>;

    /// Open the simpler view of it, for a machine where the one above will not
    /// open.
    ///
    /// Its own operation rather than a fallback inside the one above, because
    /// which of the two a run settles for is a decision: the simpler client
    /// cannot write these properties, and a client that cannot write is
    /// indistinguishable from a write that did nothing (ADR-0006, ADR-0011).
    fn open_simple_event_system(&mut self) -> Option<Box<dyn Pointers>>;
}

/// One open view of this machine's pointing devices.
pub trait Pointers {
    /// Take the pointing devices it holds, each as the one thing every property
    /// is read off.
    ///
    /// Which of them is a pointing device at all, and which is favjit's own, are
    /// read off their properties by the run (ADR-0006).
    fn look(&mut self) -> Vec<Box<dyn Pointing>>;
}

/// One of this machine's pointing devices.
pub trait Pointing {
    /// Read a 16.16 fixed-point property off it.
    fn fixed(&mut self, key: &str) -> Option<f64>;

    /// Read a plain integer property off it.
    fn integer(&mut self, key: &str) -> Option<i64>;

    /// Read a string property off it.
    fn text(&mut self, key: &str) -> Option<String>;

    /// Write a 16.16 fixed-point property to it.
    fn set_fixed(&mut self, key: &str, value: f64) -> bool;
}

/// The socket and clock calls that carry one mDNS lookup.
///
/// Separate from the source boundary because pairing uses the same lookup before
/// it has a source host. What the datagrams mean and when to stop reading them
/// stay in `engine`; this surface only carries each platform call (ADR-0006).
pub trait DiscoveryHost {
    /// Open the socket one lookup is made over, and hand it back.
    ///
    /// The socket rather than a `bool`, so that everything asked of it is asked
    /// of something that is there: an operation on a socket that may not have
    /// been opened yet has to ask whether it was, and asking is a turning beside
    /// the call it makes (ADR-0006). What comes back is a boundary of its own and
    /// not a platform handle, the same as [`link::LinkHost`] is.
    /// Borrowed from this machine for the length of one lookup, so that a
    /// platform whose socket is part of its own state can hand it over without
    /// copying anything: what the borrow costs is that nothing else can be asked
    /// of the machine while a search is open, which is the truth about a lookup
    /// anyway.
    fn bind_discovery(&mut self) -> Option<Box<dyn Searching + '_>>;

    /// Resolve the host and port from an answer, and hand back the machine it
    /// names.
    ///
    /// A literal IPv4 address is passed separately so this operation can avoid a
    /// resolver call when the responder supplied one.
    ///
    /// Not on [`Searching`]: it reaches a resolver and not that socket, and it is
    /// asked once a lookup has already answered.
    fn resolve_discovered(
        &mut self,
        host: &str,
        port: u16,
        address: Option<[u8; 4]>,
    ) -> Option<Sink>;
}

/// One open socket a lookup is made over.
pub trait Searching {
    /// Set the multicast hop limit on it.
    fn set_ttl(&mut self, ttl: u32) -> bool;

    /// Start the lookup's monotonic clock and return its origin.
    fn start_clock(&mut self) -> Instant;

    /// Read that monotonic clock.
    fn now(&mut self) -> Instant;

    /// Put one mDNS question on the local network.
    fn ask(&mut self, question: &[u8]) -> bool;

    /// Bound how long the next datagram receive may wait.
    fn set_timeout(&mut self, timeout: core::time::Duration) -> bool;

    /// Receive one datagram into `into`.
    fn receive(&mut self, into: &mut [u8]) -> Datagram;
}

/// Taking the keyboards, reading them, and telling the OS what to do instead.
pub mod sink {
    use crate::{DeviceId, Entropy, Host, IdentityStore, OutputReport};

    /// Reading this machine's keyboards, and nothing that produces a keystroke.
    pub trait SinkInputHost: Host {
        /// Whether converting is switched on right now (`docs/platform/macos/install-as-a-daemon-and-turn-off-with-a-file.md`).
        fn switched_on(&mut self) -> bool;

        /// Whether this process is allowed to read input at all.
        fn may_read_input(&mut self) -> bool;

        /// Ask the OS for permission to read input, having found
        /// [`SinkInputHost::may_read_input`] false.
        fn request_input_permission(&mut self) -> bool;

        /// Whether the device converted keystrokes go out through is still live.
        fn output_connected(&mut self) -> bool;

        /// Whether something outside asked this run to stop.
        fn stop_requested(&mut self) -> bool;

        /// Start the loop that reads this machine's keyboards, and turn `work`
        /// on it.
        ///
        /// A loop of its own because the platform ties reading to a thread: a
        /// device's queue delivers onto that thread's run loop, and a low-level
        /// hook is called on the thread that installed it. So a run's own loop
        /// cannot wait on it, and a host is handed this one to turn — the same
        /// arrangement [`SinkHost::run_alongside`] is, and for the same reason
        /// (ADR-0006).
        ///
        /// `work` is `engine`'s loop, and what it is handed is the end of that
        /// loop this host answers on. Starting the thread is the call; the order
        /// of everything on it, and how often anything is looked at, are
        /// `engine`'s.
        ///
        /// The capture rather than a `bool`, so that everything asked of a
        /// keyboard is asked of something reading them: an operation against a
        /// loop that may not have started has to ask whether it did, and asking
        /// is a turning beside the call it makes (ADR-0006).
        fn look_for_devices(
            &mut self,
            work: crate::capture::SinkLoop,
        ) -> Option<Box<dyn Capturing>>;
    }

    /// The loop reading this machine's keyboards, as the run holds it.
    ///
    /// Owned rather than borrowed from the machine, because a run goes on asking
    /// this machine other things for as long as it is reading: a beat, the
    /// output, the events themselves.
    pub trait Capturing {
        /// Open one keyboard, exclusively or not.
        ///
        /// Returns the platform's own code rather than whether it worked, because
        /// which failure it is decides what to do about it: one code says to run
        /// with privilege and another says something else already holds the
        /// keyboard.
        fn take_device(&mut self, device: DeviceId, exclusive: bool) -> i32;

        /// Start reading every element one keyboard has into the stream.
        ///
        /// Every element and not only the ones a table currently names, because
        /// which ones are worth reading is that table's question to answer, and
        /// a table is `engine`'s (ADR-0006): a page nothing names yet is where a
        /// key that reports somewhere unexpected is found.
        fn read_device(&mut self, device: DeviceId) -> bool;

        /// The next keyboard this host still holds exclusively, under both the
        /// number the run gave it and the machine's own name for it.
        ///
        /// Both, because the two calls below are made on the machine's own name
        /// while what a run says about a keyboard is the number: a call that
        /// had to look the handle up from the number would be a turning beside
        /// it, and the keyboard may have gone in between (ADR-0006).
        fn next_held_device(&mut self) -> Option<Holding>;

        /// Give it back, and answer with the platform's own code.
        fn give_it_back(&mut self, device: crate::Handed) -> i32;

        /// Let go of it as one this run holds, where the close says it went.
        ///
        /// Only then: a device this still holds is one a later run has to be
        /// able to try again for.
        fn forget_what_was_given_back(&mut self, device: crate::Handed, code: i32);
    }

    /// One keyboard a run is holding exclusively.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Holding {
        /// The number the run gave it, which is what it says about it.
        pub device: DeviceId,
        /// The machine's own name for it, which is what the calls take.
        pub at: crate::Handed,
    }

    /// Everything the converting run needs, which is the reading plus the output.
    pub trait SinkHost: SinkInputHost + IdentityStore + Entropy + crate::PointerHost {
        /// Reach the service the device converted keystrokes go out through,
        /// and hand over the connection before anything has been asked of it.
        ///
        /// Nothing is asked of the device here and nothing is waited for: what
        /// to ask it for, what its answers mean, and how long to allow are the
        /// run's, driven through [`crate::output::OutputHost`] one call at a
        /// time (ADR-0006, `docs/platform/macos/output-through-a-virtual-hid-device.md`).
        ///
        /// Reports why it could not rather than whether it did, for the reason
        /// [`Capturing::take_device`] reports a code: which failure it is
        /// decides what a person has to do about it, and saying that is
        /// `engine`'s (ADR-0006).
        fn reach_the_output(&mut self) -> Result<Box<dyn Reaching>, crate::NoOutput>;

        /// Start listening for the other machine, and hand over what it will be
        /// spoken to through.
        fn bind_link(&mut self) -> Option<Box<dyn crate::link::LinkHost + Send>>;

        /// Turn this loop alongside the role's own, on a thread of the machine's.
        ///
        /// The loop is the run's, so what it does stays where the suite can drive
        /// it; that it needs a thread at all is the machine's.
        fn run_alongside(&mut self, work: Box<dyn FnOnce() + Send>) -> bool;

        /// Turn the loop serving the output device's connection, likewise.
        ///
        /// Its own operation rather than a second [`SinkHost::run_alongside`]:
        /// this loop coming back means the output device is gone, and the one
        /// above coming back means the link is — two facts a run acts on
        /// differently, so one call each rather than one signal for both
        /// (ADR-0006).
        fn run_output_alongside(&mut self, work: Box<dyn FnOnce() + Send>) -> bool;

        /// Where a run that injects nothing writes what it converted.
        ///
        /// Its own operation rather than something [`SinkHost::open_output`]
        /// could answer with, because a run in that mode must not open a device
        /// at all: what the mode is for is being safe to run, and a virtual
        /// keyboard left behind is the change it exists to avoid. Nothing is
        /// asked of the machine here, and what a host does with the reports —
        /// prints them, counts them, drops them — is its own.
        fn instead_of_the_output(&mut self) -> Box<dyn Injecting>;
    }

    /// The connection to that service, as the run holds it before it is spoken
    /// on.
    ///
    /// Handed over rather than set up inside the open, so that each step is one
    /// call on something that is there: bounding the writes and taking a second
    /// handle on it are two more calls into the machine, and the order over them
    /// is the run's (ADR-0006).
    pub trait Reaching {
        /// Refuse to let a single write to it block longer than this.
        fn do_not_block_writes(&mut self, write_timeout: core::time::Duration) -> bool;

        /// Hand over its two ends, or nothing where a second handle on it could
        /// not be had.
        ///
        /// Two handles and not one behind a lock the run reaches through: the
        /// loop serving this connection is turned on a thread of its own, so
        /// what it waits on a read for would be what a report the run is posting
        /// waits on.
        fn both_ends(self: Box<Self>) -> Option<Opened>;
    }

    /// The two ends of the output device's connection, as the run holds them.
    ///
    /// Both handed over rather than the writing one kept here, so that writing
    /// a report is asked of a device that is there: a write against one that may
    /// not have opened has to ask whether it did, and asking is a turning beside
    /// the call it makes (ADR-0006).
    pub struct Opened {
        /// The connection's own loop, for the run to turn alongside its own.
        pub serving: Box<dyn crate::output::OutputHost>,
        /// What converted input is written to.
        pub writing: Box<dyn Injecting>,
    }

    /// The device converted input goes out through.
    pub trait Injecting {
        /// Write these bytes on that report of the output device, answering `0`
        /// for the write that went and this machine's own error number
        /// otherwise.
        ///
        /// The number and not whether it worked, for [`crate::source::Sending::send`]'s
        /// reason: a device that has gone and a write that ran out of time are
        /// the same `false`, and the trace a run is read from afterwards holds
        /// this number (ADR-0009).
        fn send_report(&mut self, report: OutputReport, bytes: &[u8]) -> i32;
    }
}

/// The connection to the device converted keystrokes go out through
/// (`docs/platform/macos/output-through-a-virtual-hid-device.md`).
pub mod output {
    use core::time::Duration;

    /// How much of what one read asked for arrived, and why no more did.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    #[non_exhaustive]
    pub enum Took {
        /// This many bytes, which a stream socket is free to make fewer than
        /// were asked for.
        ///
        /// The count and not what it means: none at all is the far end having
        /// closed, and reading it that way is the run's the same way the length
        /// in front of a frame is.
        Bytes(usize),
        /// Nothing, because the bound this read was given came round first.
        ///
        /// A kind of its own rather than no bytes, because it is what says a
        /// cadence has come round with nothing to answer — and a run that read
        /// it as the connection going would tear down a device that is merely
        /// quiet.
        Nothing,
        /// Nothing yet: the read was cut short rather than answered, so the
        /// same read is worth making again.
        ///
        /// Its own kind rather than either of the two above, because it is
        /// neither: no bytes came off the stream and no bound came round, so a
        /// run counting it as a quiet cadence would beat on a signal instead of
        /// on a clock.
        Interrupted,
        /// Nothing more will arrive on it, whatever the socket said.
        Gone,
    }

    /// One end of that connection, served on a loop of its own.
    ///
    /// Asymmetric on purpose, and both halves are ADR-0006's: what arrives
    /// crosses as the bytes it arrived as, because where a frame ends and which
    /// of them is a health check are the transport's own numbering and a
    /// numbering beside the socket is a decision no test can drive — while what
    /// goes out is named for what it asks, because turning that into the
    /// service's own framing, version and request id is the argument conversion
    /// a host operation is allowed.
    pub trait OutputHost: Send {
        /// What this machine's clock reads right now.
        ///
        /// Its own and not [`crate::Host::now`]'s, because the loop serving
        /// this connection is turned on a thread of its own: a run reaching
        /// for the converting loop's clock would be two loops on one, which
        /// is what handing this end over separately avoids.
        fn now(&mut self) -> crate::Instant;

        /// Bound how long the next read may wait.
        fn set_read_timeout(&mut self, timeout: Duration) -> bool;

        /// Take up to `into.len()` bytes off it, and say how far that got.
        ///
        /// Bytes and not a frame: a frame is a length and then that many more
        /// bytes, so reading one is two of these — and which two is the
        /// transport's numbering rather than anything the socket does, so it is
        /// read where a test can drive it (ADR-0006).
        fn read_some(&mut self, into: &mut [u8]) -> Took;

        /// Ask for the keyboard converted keystrokes go out through.
        fn ask_for_the_keyboard(&mut self) -> bool;

        /// Ask for the pointing device beside it.
        fn ask_for_the_pointer(&mut self) -> bool;

        /// Say this end is still here, answering `0` for the write that went
        /// and this machine's own error number otherwise.
        ///
        /// The number and not whether it went, for
        /// [`crate::sink::SinkHost::send_report`]'s reason: a connection the
        /// service has closed and a write that ran out of time are the same
        /// `false` and different problems, and the recording a dropped
        /// connection is read from afterwards holds this number (ADR-0009).
        fn beat(&mut self) -> i32;

        /// Answer a health check, likewise.
        fn answer_health_check(&mut self) -> i32;

        /// Answer the request carrying this id, likewise.
        ///
        /// The id and not the frame: the transport pairs a request with its
        /// answer by it, and what the answer carries besides is nothing.
        fn acknowledge(&mut self, id: &[u8]) -> i32;

        /// Put this on the run's own stream, stamped with this machine's clock.
        ///
        /// The same operation [`crate::link::LinkHost::deliver`] is, and for the
        /// same reason: the loop serving this connection is not the loop that
        /// keeps the recording, so what it has to say reaches the recording the
        /// way anything else outside does (ADR-0009).
        fn deliver(&mut self, event: crate::EventKind) -> bool;
    }
}

/// Reading the other machine's keyboards and relaying them.
pub mod source {
    use core::time::Duration;

    use crate::{DiscoveryHost, Entropy, Host};

    /// What one of the devices a machine has attached is.
    ///
    /// Named rather than carried as the number this machine lists it under: what
    /// a run shows is a keyboard or a mouse, and reading which is which off a
    /// number would be a table beside the enumeration (ADR-0006).
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    #[non_exhaustive]
    pub enum Kind {
        Keyboard,
        Pointer,
        /// Something else this machine lists among its input devices.
        Something,
    }

    /// The input devices a machine has attached, as the run reads them.
    ///
    /// Handed over rather than answered as a finished list, so that each step is
    /// one call: a machine that says how many there are before it will fill any
    /// in, and how much room a device's path needs before it will write one, is
    /// two calls each — and the order over them is the run's (ADR-0006).
    ///
    /// For reporting rather than for capture: what a person needs in order to
    /// write a vendor and product into a configuration is the list, and the list
    /// is a separate question from what is being typed on.
    pub trait Listing {
        /// How many there are to look at.
        fn how_many(&mut self) -> usize;

        /// Look at that many, and say how many it could.
        fn look(&mut self, how_many: usize) -> usize;

        /// What the one in this place is, and nothing for a place nothing was
        /// looked at in.
        fn at(&mut self, place: usize) -> Option<Kind>;

        /// How many characters that one's path needs.
        fn how_long_its_path_is(&mut self, place: usize) -> usize;

        /// Its path, written into that much room, and nothing where it would not
        /// go.
        fn its_path(&mut self, place: usize, room: usize) -> Option<String>;
    }

    /// What looking for the other machine produced.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    #[non_exhaustive]
    pub enum Connected {
        /// It is there, and a session can be opened with the machine named.
        Ready(crate::Sink),
        /// The other machine is not there — asleep, rebooting, or not on this
        /// network.
        ///
        /// A kind of its own rather than an error, because it is the ordinary
        /// state of a machine that has not been switched on yet.
        NotFound,
        /// Nothing more will work — the run has reached whatever bound it was
        /// given, or asking will not change: identity and the pinned sink are
        /// asked for once, before this is ever reached, since neither is a
        /// question worth asking again on the strength of a machine answering
        /// mDNS.
        Done,
    }

    /// What this machine's own input is refused for.
    ///
    /// Three states, because there is a middle one: while the keyboard is this
    /// machine's, the chord that moves it has to be refused or pressing it also
    /// reaches whatever has the foreground. Refusing exactly that costs nothing
    /// where what refuses a key is also what reports it — the chord is already
    /// on its way to the run's own loop by the time it is turned down.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    #[non_exhaustive]
    pub enum Suppressing {
        /// Nothing at all: while there is no link, and where a run ends.
        Nothing,
        /// The chord that moves the keyboard, and nothing else.
        TheSwitch,
        /// Everything, because the input is going to the other machine.
        Everything,
    }

    /// What one call into a procedure the platform makes carries.
    ///
    /// The platform's own numbers already read as what they are, so that the run
    /// answering the call is handed an event and not a pointer: a procedure is
    /// called on the thread the event arrived on, and nothing above this boundary
    /// can be reached from there except through what arrives with it.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    #[non_exhaustive]
    pub enum Arrived {
        /// A call carrying no event of ours: none at all, or one favjit injected.
        ///
        /// Injected events are neither reported nor refused. There is no device
        /// behind one, so nothing can relay it, and refusing what cannot be
        /// relayed is taking input away with nothing to show for it — the shape
        /// of failure ADR-0008 rules out, in miniature.
        NothingOfOurs,
        /// One key, as a procedure reports it.
        Key {
            /// The position it was typed at, which is what a chord is compared
            /// against.
            at: At,
            /// The two halves the capture loop is handed.
            packed: usize,
            /// The flags that came with it.
            flags: i64,
        },
        /// One pointer event, which carries nothing the run reads: a movement is
        /// still a movement where raw input reports it, and not here (ADR-0011).
        Pointer,
    }

    /// The calls one answer to such a procedure is made of.
    ///
    /// Handed over on the call rather than held anywhere, because a procedure the
    /// platform calls is reached through a bare function pointer: there is
    /// nowhere to hang a host off, so what may be called arrives with the event.
    ///
    /// Every one of these is one call into the machine or none, the same as any
    /// other operation (ADR-0006). What order they go in, and which of them
    /// happen at all, is the run's.
    pub trait Answers {
        /// What is being refused as things stand, as the run last published it.
        fn refusing(&self) -> Suppressing;

        /// The two positions the chord is made of, as a procedure reports them.
        ///
        /// Read back rather than held by the run: the run answering a procedure
        /// is reached through a function pointer and holds nothing, so what it
        /// compares against comes from where it published it.
        fn the_chord(&self) -> Chord;

        /// Whether there is anywhere for a key to be handed to yet.
        fn keys_go_somewhere(&self) -> bool;

        /// Put the key where the capture loop will read it.
        fn hand_the_key_over(&self, packed: usize, flags: i64) -> bool;

        /// Whether either modifier the chord needs is down, as the machine has
        /// it this instant.
        ///
        /// Asked of the machine rather than counted from the events: a count of
        /// the run's own would be wrong about a key already held when the
        /// procedures were installed, and this is the only answer available to
        /// one that has to return before the next event.
        fn the_modifier_is_down(&self) -> bool;

        /// Whether the bit the run numbered a key with is set: what this machine
        /// was let see go down and not yet let see go up, as the run keeps it.
        ///
        /// Kept on the machine rather than counted in the run's loop, for the
        /// reason the modifier is asked of the machine: the answer is wanted
        /// inside a call that has to return before the next event, and the loop
        /// reads the key only after that call has answered. A bit and not a
        /// position, so that what the machine stores is a number the run chose
        /// and nothing it had to read: which bit a position is, and that a
        /// position past the range has none, is the run's (ADR-0006).
        ///
        /// `bit` is below [`HELD_HERE_BITS`].
        fn held_here(&self, bit: usize) -> bool;

        /// Set that bit.
        fn now_held_here(&self, bit: usize);

        /// Clear that bit.
        fn let_go_of_here(&self, bit: usize);

        /// Count one refused pointer event.
        fn one_pointer_refused(&self);

        /// Let the event carry on to whatever is downstream, and answer with
        /// what the platform is told.
        fn let_it_carry_on(&self) -> isize;

        /// End the event here, and answer with what the platform is told.
        ///
        /// Everything after this — whatever has the foreground, the shell's own
        /// hotkeys — never hears it.
        fn end_it_here(&self) -> isize;
    }

    /// Where a key sits on a keyboard, as a procedure reports it: the make code
    /// and whether the prefix that says which half of the keyboard came with it.
    ///
    /// A make code says which position only with that prefix beside it — the
    /// arrow cluster shares numbers with the keypad — so the two travel together
    /// or neither says anything.
    pub type At = (u16, bool);

    /// The two positions a chord is made of, either of which a keyboard may have
    /// none for.
    pub type Chord = (Option<At>, Option<At>);

    /// How many bits [`Answers::held_here`] and its two writers address: one per
    /// make code a scan code can be, on each half of the keyboard.
    pub const HELD_HERE_BITS: usize = 512;

    /// How a run answers a procedure the platform calls.
    ///
    /// A bare function and not a closure: it is reached from a procedure the
    /// platform calls through a function pointer, which has nowhere to keep
    /// captures — so everything it reads comes through [`Answers`].
    pub type Answering = fn(Arrived, &dyn Answers) -> isize;

    /// Which machine the keyboard in front of the person is driving.
    ///
    /// One keyboard cannot drive both: what is relayed has to be refused where it
    /// was typed, or every keystroke lands on both screens. So it is a mode with
    /// two states rather than two things happening at once, and the chords in
    /// `engine`'s source loop are how a person moves between them (ADR-0013).
    ///
    /// Here rather than beside those chords, because [`crate::EventKind`] carries
    /// it: an ask from outside the keyboard arrives on the same stream as the
    /// keystrokes, and the program that sends one links this crate and not the
    /// engine.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Driving {
        /// This machine. Nothing is relayed, and nothing is refused but the chord.
        ThisMachine,
        /// The machine at the other end of the link. Everything is refused here
        /// and sent there.
        TheSink,
    }

    /// The boundary a source runs against.
    ///
    /// A supertrait of [`Host`], [`Entropy`] and [`crate::IdentityStore`] rather
    /// than further objects, for the reason [`crate::link::LinkHost`] is one: the
    /// run's own loop reads this machine's events through [`Host`], the
    /// handshake it drives needs an ephemeral key from somewhere, and both this
    /// machine's own identity and the sink it is pinned to are asked of this same
    /// platform, once, before either is needed (ADR-0006).
    pub trait SourceHost: Host + Entropy + DiscoveryHost + crate::IdentityStore {
        /// What is in the file naming the one machine this one will relay to.
        ///
        /// The text and not the key, the way [`crate::link::LinkHost::authorized`]
        /// answers with the text of the sink's own list: which line is the key,
        /// and what a key looks like written down, are questions about the
        /// file's content and not the platform's to answer (ADR-0006).
        fn pinned_sink(&mut self) -> Option<String>;

        /// Answer the procedures this platform calls with this from now on.
        ///
        /// Its own operation and asked for before [`SourceHost::take_input`],
        /// because the order is the run's: a procedure the platform calls is
        /// reached through a function pointer, so what answers it has to be
        /// there before whatever the platform calls it for exists.
        ///
        /// A platform that calls nothing back answers nothing, and this is the
        /// one operation that may do nothing at all.
        fn answer_procedures_with(&mut self, answering: Answering);

        /// Start the thread this machine's keyboards and mice are read on, and
        /// turn `work` on it. Checks for a watchdog probe every `probe_tick`,
        /// and reads each raw input into a buffer of `raw_bytes`.
        ///
        /// `false` only when that thread cannot be started at all, which ends
        /// the run before it opens a socket: a machine whose input this process
        /// cannot see has nothing to forward, and a link to say so with would be
        /// one the sink is sent nothing over.
        ///
        /// Whether this run may refuse anything is not asked here. Becoming able
        /// to refuse is one of the calls `work` makes
        /// ([`crate::capture::SourceCapture::hook_the_keyboard`]), so a run that
        /// will never refuse simply does not make it — which is one fewer thing
        /// a host is told and one more the suite can drive. When to refuse is
        /// [`SourceHost::suppress`]'s, and it is asked over and over.
        ///
        /// `probe_tick` and `raw_bytes` are `engine`'s: how often, and how much,
        /// are favjit's own judgement and not the platform's (ADR-0006). So is
        /// `work` — the loop this starts a thread for, turned on it, the same
        /// arrangement [`crate::sink::SinkInputHost::look_for_devices`] is.
        fn take_input(
            &mut self,
            probe_tick: core::time::Duration,
            raw_bytes: usize,
            work: crate::capture::SourceLoop,
        ) -> bool;

        /// Wait this long before looking for the sink again.
        ///
        /// That it happens between attempts and not before the first is the run's
        /// own loop's. Without it, a machine whose sink is switched off looks as
        /// fast as the answer comes back.
        fn pause(&mut self, how_long: Duration);

        /// Whether something outside asked this run to stop, or the bound it
        /// was given has passed.
        ///
        /// Asked before looking for a sink as well as after the stream has
        /// ended: a machine whose sink is switched off spends a whole run
        /// waiting for one, and a bound only the event loop honoured would
        /// never be reached there.
        fn asked_to_stop(&mut self) -> bool;

        /// Whether this machine has anywhere to look for a sink at all.
        ///
        /// One fact on its own, never folded together with
        /// [`SourceHost::asked_to_stop`]: which of the two explains a run that
        /// is going no further decides what it says on the way out, and a
        /// host that answered them together would be deciding that
        /// (ADR-0006).
        fn has_a_sink_to_look_for(&mut self) -> bool;

        /// Use a configured sink address without mDNS, when one was supplied.
        fn use_fixed_sink(&mut self) -> Option<crate::Sink>;

        /// Connect to `sink`, waiting at most `timeout`, and hand the
        /// connection over.
        ///
        /// The connection rather than a `bool`, so that everything asked of it
        /// is asked of something that is there: an operation on a socket that
        /// may not have connected has to ask whether it did, and asking is a
        /// turning beside the call it makes (ADR-0006).
        ///
        /// Owned rather than borrowed from the machine, because a run goes on
        /// asking this machine other things while it holds one: a beat, its own
        /// input, and the keystrokes it is about to seal.
        fn connect_socket(
            &mut self,
            sink: crate::Sink,
            timeout: Duration,
        ) -> Option<Box<dyn Opening>>;

        /// Refuse this machine's own input, or stop refusing it.
        ///
        /// Separate from connecting because when it happens is a decision: the
        /// keys are only taken while there is a link to relay them over *and*
        /// the person has asked for the other machine.
        fn suppress(&mut self, what: Suppressing);

        /// Whether this machine has anywhere to show what is being refused.
        ///
        /// A platform that shows it nowhere answers `false` and is never asked to
        /// (`docs/platform/windows/tray-item-as-its-own-program.md`); one that
        /// does may not have opened the place yet, which is the state a run comes
        /// up in.
        fn there_is_somewhere_to_show_it(&mut self) -> bool;

        /// Show what is being refused where a person can see it.
        ///
        /// Its own operation beside [`SourceHost::suppress`] because they are one
        /// state seen from two places, and setting both from one call would make
        /// the order between them the platform's rather than the run's
        /// (ADR-0006).
        fn show_what_is_refused(&mut self, what: Suppressing);
    }

    /// One connection to the sink, while a session is being opened over it.
    ///
    /// Everything here is asked of a socket that is connected, because that is
    /// what makes one of these exist at all.
    pub trait Opening {
        /// Disable Nagle's algorithm on it.
        fn set_nodelay(&mut self, enabled: bool) -> bool;

        /// Bound how long a read may wait.
        fn set_read_timeout(&mut self, timeout: Duration) -> bool;

        /// Bound how long a write may wait.
        fn set_write_timeout(&mut self, timeout: Duration) -> bool;

        /// Send the first handshake message.
        fn send_first_message(&mut self, first: &[u8]) -> bool;

        /// Flush the first handshake message.
        fn flush_first_message(&mut self) -> bool;

        /// Read the sink's answer to it, or that the connection did not survive
        /// waiting for one.
        fn take_answer(&mut self, into: &mut [u8]) -> bool;

        /// Keep it open, for the records the session that came of the handshake
        /// carries.
        ///
        /// Taken by value, so a run holds one of the two and never both: a
        /// handshake is over by the time there is a session, and everything
        /// asked of the connection afterwards is a write.
        fn keep(self: Box<Self>) -> Box<dyn Sending>;
    }

    /// One connection to the sink, with a session going over it.
    pub trait Sending {
        /// Send one sealed record, answering `0` for the write that went and
        /// this machine's own error number otherwise.
        ///
        /// The number and not whether it worked: which failure it was is the
        /// difference between the other machine having gone and this one's write
        /// having run out of time, and those are the same `false` and different
        /// problems. Reported rather than acted on — what to do about it is a
        /// decision, and this one is the difference between stopping and reading
        /// keyboards nobody will hear — and recorded as the number, since a trace
        /// is read after the run and a sentence composed here would be a host
        /// choosing the words (ADR-0006, ADR-0009).
        ///
        /// Sealing it is `engine`'s, through the session its own Noise
        /// construction produced: what crosses here is bytes, the way the
        /// sink's own sealed records are (ADR-0006, ADR-0012).
        ///
        /// Must not block indefinitely, for the reason the sink's injection
        /// must not: a single loop has no other thread to make progress on, and
        /// a source wedged on a socket is a keyboard that has stopped
        /// (ADR-0008).
        fn send(&mut self, sealed: &[u8]) -> i32;
    }
}

/// The socket the two machines talk over.
pub mod link {
    use core::time::Duration;

    use crate::Entropy;

    /// What came of waiting for a machine to connect.
    ///
    /// Neither copied nor compared, because one of its kinds carries the
    /// connection: a socket is one thing and not a value there can be two of.
    #[non_exhaustive]
    pub enum Accepted {
        /// Something connected, and this is the connection.
        ///
        /// The connection rather than a `Yes`, so that everything asked of it is
        /// asked of something that is there: an operation on a socket nothing
        /// has connected to has to ask whether anything did, and asking is a
        /// turning beside the call it makes (ADR-0006). Owned by the run, which
        /// holds it for a whole session and goes on asking the machine other
        /// things all the while — and letting it go is what closes it.
        Yes(Box<dyn Talking>),
        /// Nothing did, and this is what the socket said about it.
        ///
        /// The kind and not whether waiting again is worth it: which kinds are
        /// a connection that stopped existing and which are the socket that has
        /// stopped working is a table, and a table beside the socket is a
        /// decision no test can drive (ADR-0006). The kind is `std`'s, so it
        /// crosses as it is.
        No(std::io::ErrorKind),
    }

    /// One open connection to a source, for as long as a session lasts.
    pub trait Talking {
        /// Bound how long a read may wait.
        fn set_read_timeout(&mut self, timeout: Duration) -> bool;

        /// Disable Nagle's algorithm on it.
        fn set_nodelay(&mut self, enabled: bool) -> bool;

        /// Read the source's first handshake message.
        fn take_handshake(&mut self, into: &mut [u8]) -> bool;

        /// Write this end's answer to it.
        fn send_answer(&mut self, answer: &[u8]) -> bool;

        /// Flush that answer.
        fn flush_answer(&mut self) -> bool;

        /// Read one sealed record, or say the session is over.
        fn take_record(&mut self, into: &mut [u8]) -> Incoming;

        /// Say this end is sending the peer away rather than letting an ordinary
        /// session end.
        ///
        /// Its own operation because letting the connection go is the same either
        /// way — the socket closes when the run drops it — and which of the two
        /// it was is the run's to say, not something a close could be read as
        /// (ADR-0006). A platform with nothing to do about it does nothing.
        fn sending_it_away(&mut self);
    }

    /// What came of reading one record.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    #[non_exhaustive]
    pub enum Incoming {
        /// `into` now holds a whole record.
        Record,
        /// The session is over.
        Gone,
    }

    /// The sink's end of the socket, served on a loop of its own.
    ///
    /// Bytes throughout: the handshake and the sealing are one construction the two
    /// machines share, so they are driven where the suite drives them and a host
    /// carries what comes out (ADR-0012).
    pub trait LinkHost: Entropy {
        /// Read the port the listening socket was assigned.
        fn listener_port(&mut self) -> Option<u16>;

        /// Say on the network that this machine is here as `service` at `port`.
        ///
        /// Both values are `engine`'s to provide: the port comes from the preceding
        /// socket operation, while the name is an agreement with the other machine
        /// and is therefore stated once, in `engine` (ADR-0006).
        fn advertise(&mut self, service: &str, port: u16) -> bool;

        /// Take a connection, if one is waiting.
        fn accept(&mut self) -> Accepted;

        /// What is in the file naming the machines this one will talk to.
        fn authorized(&mut self) -> Option<String>;

        /// Put this on the run's own stream, stamped with this machine's clock —
        /// the peer's is not comparable to it.
        ///
        /// [`crate::EventKind`] rather than the plaintext bytes alone, so that a
        /// session ending can say a device is gone the same way losing it locally
        /// would, with nothing on the wire behind that fact to carry.
        fn deliver(&mut self, event: crate::EventKind) -> bool;

        /// Put a line where this machine's log goes.
        ///
        /// Its own operation for the reason [`crate::Host::warn`] is one: the
        /// loop serving this socket is turned on a thread of the machine's,
        /// with no [`crate::Host`] of its own to say anything through, and a
        /// line it could not say at all is a source refused for a reason
        /// nobody can see.
        fn warn(&mut self, message: core::fmt::Arguments);
    }
}

/// Reading a code off one machine and typing it into the other.
pub mod pairing {
    use core::time::Duration;

    use crate::Entropy;

    /// The sink's end of pairing: it shows the code and waits.
    pub trait PairingHost: Entropy {
        /// Open the socket this attempt takes a source on, and hand it over.
        ///
        /// The socket rather than a `bool`, so that everything asked of it is
        /// asked of something that is there: an operation on a listener that may
        /// not have been opened has to ask whether it was, and asking is a
        /// turning beside the call it makes (ADR-0006). Owned by the run and not
        /// borrowed from the machine, because the run shows the code and puts the
        /// port on the network while the socket is open.
        fn bind_listener(&mut self) -> Option<Box<dyn Listening>>;

        /// Advertise `service` at `port` for as long as this attempt lasts.
        fn advertise(&mut self, service: &str, port: u16) -> bool;

        /// Write one complete line where the person can read it.
        ///
        /// The complete text rather than digits for the host to format: what to say,
        /// and the order of the lines, are pairing policy and stay in `engine`.
        fn show(&mut self, line: &str);

        /// What is in the file naming the machines this one will talk to.
        fn authorized(&mut self) -> Option<String>;

        /// Make the directory the authorised-list file belongs in.
        ///
        /// Reports what the machine said about a failure rather than only that
        /// there was one, for the reason [`crate::IdentityStore`]'s writes do:
        /// a pairing that cannot be written down is one a person has to fix,
        /// and which of the two it was decides how (ADR-0006).
        fn make_authorized_directory(&mut self) -> Result<(), crate::Trouble>;

        /// Replace the file naming the machines this one will talk to.
        ///
        /// The whole text to store, not the key just pinned: whether that key is
        /// already in the file, and where a new line joins what a person put there,
        /// is a question about the file's content, answered where the content is
        /// read (ADR-0006).
        fn authorize(&mut self, text: &str) -> Result<(), crate::Trouble>;
    }

    /// The socket one pairing attempt takes a source on.
    pub trait Listening {
        /// The port it was assigned.
        fn port(&mut self) -> Option<u16>;

        /// Make it blocking, so waiting for a source is a wait.
        fn set_blocking(&mut self) -> bool;

        /// Wait for a machine to connect, and hand the connection over.
        fn accept(&mut self) -> Option<Box<dyn Exchanging>>;
    }

    /// The one connection a pairing attempt runs over.
    ///
    /// An [`Entropy`] as well, because the arithmetic the exchange is made of
    /// needs bytes nothing can predict and this is what a run holds while it is
    /// running one (ADR-0012).
    pub trait Exchanging: Entropy {
        /// Bound how long a read may wait.
        fn set_read_timeout(&mut self, timeout: Duration) -> bool;

        /// Bound how long a write may wait.
        fn set_write_timeout(&mut self, timeout: Duration) -> bool;

        /// Read what it offered.
        fn take_offer(&mut self, into: &mut [u8]) -> bool;

        /// Write this machine's side of the exchange.
        fn send_answer(&mut self, answer: &[u8]) -> bool;

        /// Flush this machine's side of the exchange.
        fn flush_answer(&mut self) -> bool;

        /// Read the key it sealed for this machine.
        fn take_sealed_key(&mut self, into: &mut [u8]) -> bool;

        /// Write this machine's key, sealed for it.
        fn send_sealed_key(&mut self, sealed: &[u8]) -> bool;

        /// Flush this machine's sealed key.
        fn flush_sealed_key(&mut self) -> bool;
    }

    /// The source's end of pairing: the code was typed on it.
    pub trait SourcePairingHost: Entropy {
        /// Open a socket to the machine offering to pair, waiting at most `timeout`.
        /// Connect to the machine showing the code, and hand the connection
        /// over.
        ///
        /// The connection rather than a `bool`, so that everything asked of it
        /// is asked of something that is there: an operation on a socket that
        /// may not have connected has to ask whether it did, and asking is a
        /// turning beside the call it makes (ADR-0006).
        fn connect(&mut self, timeout: Duration) -> Option<Box<dyn Offering>>;

        /// Make the directory the pinned-sink file belongs in.
        ///
        /// Reports what the machine said about a failure, for the reason the
        /// other end's own writes do (ADR-0006).
        fn make_sink_directory(&mut self) -> Result<(), crate::Trouble>;

        /// Replace the file naming the one machine this one will relay to.
        ///
        /// The whole text to store, not the key just paired with: how a key is
        /// written down, and that the file holds one rather than a list, are
        /// questions about the file's content — and the machine at the other
        /// end reads its own with the same answers, so they are given once, in
        /// `engine` (ADR-0004, ADR-0006).
        fn pin_sink(&mut self, text: &str) -> Result<(), crate::Trouble>;
    }

    /// The one connection this end of a pairing attempt runs over.
    ///
    /// An [`Entropy`] for the reason [`Exchanging`] is: the arithmetic needs
    /// bytes nothing can predict, and this is what a run holds while it is
    /// running one (ADR-0012).
    pub trait Offering: Entropy {
        /// Bound how long a read may wait.
        fn set_read_timeout(&mut self, timeout: Duration) -> bool;

        /// Bound how long a write may wait.
        fn set_write_timeout(&mut self, timeout: Duration) -> bool;

        /// Write this end's half of the code.
        fn send_offer(&mut self, offer: &[u8]) -> bool;

        /// Flush it.
        fn flush_offer(&mut self) -> bool;

        /// Read the other end's half.
        fn take_answer(&mut self, into: &mut [u8]) -> bool;

        /// Write this machine's key, sealed for it.
        fn send_sealed_key(&mut self, sealed: &[u8]) -> bool;

        /// Flush it.
        fn flush_sealed_key(&mut self) -> bool;

        /// Read the key it sealed for this machine.
        fn take_sealed_key(&mut self, into: &mut [u8]) -> bool;
    }
}

/// Watching the process that holds the keyboards (ADR-0008).
pub mod watchdog {
    use core::time::Duration;

    use crate::Instant;

    /// Why the supervised process is no longer running.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    #[non_exhaustive]
    pub enum Exit {
        /// It stopped of its own accord, with this code.
        Code(i32),
        /// A signal ended it.
        Signal(i32),
        /// It is gone and the machine cannot say how.
        Unknown,
    }

    /// What came of waiting for a heartbeat.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    #[non_exhaustive]
    pub enum BeatKind {
        /// One arrived.
        Beat,
        /// None arrived inside the patience it was given.
        Silent,
        /// The pipe it would have arrived on is gone.
        Gone,
    }

    /// A heartbeat, and when it arrived.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Beat {
        pub kind: BeatKind,
        pub at: Instant,
    }

    /// What the supervisor needs of the machine it supervises on.
    pub trait WatchdogHost {
        /// Start the process being watched, and say when it started.
        fn start(&mut self) -> Option<Instant>;

        /// Whether it has stopped, and how.
        fn ended(&mut self) -> Option<Exit>;

        /// Wait for a heartbeat, for no longer than this.
        fn wait_for_a_heartbeat(&mut self, patience: Duration) -> Beat;

        /// Ask it whether its loop is still turning.
        fn probe(&mut self) -> bool;

        /// Ask it to stop, the way a process is asked.
        fn ask_it_to_stop(&mut self) -> bool;

        /// Do nothing for this long.
        fn pause(&mut self, how_long: Duration);

        /// End it, the way a process that will not stop is ended.
        fn end_it(&mut self);

        /// Keep whatever it recorded of what it was doing (ADR-0009).
        fn keep_the_trace(&mut self);

        /// Whether somebody outside has asked for the recording.
        ///
        /// Its own fact rather than folded into the wait, for the reason every
        /// other condition is (ADR-0006): what a supervisor does about it is not
        /// what it does about a silence, and a wait that answered both would be
        /// choosing between them.
        ///
        /// Asked over and over while a run is going, because the reading a person
        /// wants is of a run that has not ended: a link that keeps dropping wedges
        /// nothing, so the ending never comes and the recording would never be
        /// read (ADR-0009).
        fn asked_for_the_trace(&mut self) -> bool;

        /// Hand the recording to whoever asked, and go on supervising.
        ///
        /// Handed over rather than written where the asker said: this process is
        /// the one running as root, and a path it took from somebody else would be
        /// a way to have root write anywhere. What the asker does with the bytes
        /// is its own business, at its own privilege.
        fn hand_the_trace_over(&mut self);

        /// Put a line where this machine's log goes.
        fn warn(&mut self, message: core::fmt::Arguments);
    }
}

/// The loop that reads this machine's own keyboards.
///
/// Its own loop because the platform ties reading to a thread: on the Mac a
/// device's queue delivers onto that thread's run loop, and on Windows a
/// low-level hook is called on the thread that installed it. So a role's own
/// loop cannot wait on it, and the host is handed this one to turn alongside
/// (ADR-0006).
///
/// What runs on it is `engine`'s. What the host answers here is one call each: a
/// device the registry has, a turn of the loop, the probes the supervisor sent.
/// Which device is worth taking, what number it gets, and in what order any of
/// it happens are decisions, and a loop that held them would be a loop no test
/// can drive.
///
/// **One boundary per role and not one both roles answer.** Which machine has
/// which role is settled — the forwarding machine is the source and the
/// converting one is the sink — so a loop reading the source's keyboards and a
/// loop reading the sink's are two loops, and nothing is gained by naming the
/// operations so that both could answer them. What that would cost is what
/// matters: the calls of one machine spelled in words invented to fit the other,
/// and each host answering the other's operations with nothing. So each step
/// here is named for the call it makes (ADR-0006), even where that is one
/// machine's own vocabulary and means nothing on the other.
pub mod capture {
    use core::time::Duration;

    use crate::{DeviceId, EventKind};

    /// The loop `engine` hands the forwarding machine's host to turn on the
    /// thread it starts, and the one it hands the converting machine's.
    ///
    /// Named because each is written out at every place that loop is handed
    /// over, and a shape written out is a shape that can drift.
    pub type SourceLoop = Box<dyn FnOnce(&mut dyn SourceCapture) + Send>;
    pub type SinkLoop = Box<dyn FnOnce(&mut dyn SinkCapture) + Send>;

    /// What the run has asked of the loop reading this machine's keyboards.
    ///
    /// The device it names is the machine's own handle for one this loop has
    /// already handed over ([`crate::Handed`]): the run knows it by the number
    /// it gave it, and turning that back into the handle is the host's — so what
    /// crosses here is the handle, and the run only hands it to the calls below.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    #[non_exhaustive]
    pub enum Ask {
        /// Open one, and take it away from everything else where `exclusive`
        /// says to.
        Take {
            device: crate::Handed,
            exclusive: bool,
        },
        /// Start reading everything it reports into the stream.
        Read { device: crate::Handed },
    }

    /// One device the platform has, as the properties it carries.
    #[derive(Debug, Clone, PartialEq)]
    pub struct Found {
        /// The platform's own id for it, which is how the same device is
        /// recognised whichever way it was come across.
        ///
        /// A number and not a handle: it is what the registry calls the device,
        /// so it crosses, while the handle it names stays behind (ADR-0006).
        pub named: u64,
        /// What the device says about itself, already under the number it was
        /// asked for.
        pub found: EventKind,
    }

    /// One end of that loop, turned by `engine` on the thread the host started.
    ///
    /// Not `Send`, unlike the ends of the other loops: this one is made on the
    /// thread it is turned on and never leaves it, and what a platform ties to
    /// that thread — a window, a hook, a message — is exactly what cannot be
    /// sent. What crosses to the thread is [`Loop`], and that is `Send`.
    pub trait Reading {
        /// Wait on this loop until it has delivered something, or for `poll` if
        /// nothing arrives.
        ///
        /// Back at the first thing delivered and not at the end of the poll,
        /// because what the platform's callbacks deliver is put down for the
        /// drains that follow this call to read: a wait that ran on to the end
        /// of the poll would have every keystroke and every pointer report wait
        /// with it, and a pointer that reports a dozen movements at once is one
        /// that moves in steps (`docs/platform/macos/input-latency.md`).
        fn turn_the_loop(&mut self, poll: Duration);

        /// How many probes the supervisor has sent since this was last asked
        /// (ADR-0008).
        fn take_probes(&mut self) -> usize;

        /// Put this on the run's own stream, stamped with this machine's clock.
        ///
        /// Answers whether anything is reading that stream, which is what says
        /// the run this loop belongs to has gone: the loop is the writer and the
        /// run is the reader, so the writer finding nobody there is the one way
        /// round this is ever learned.
        fn deliver(&mut self, event: EventKind) -> bool;

        /// Whether the run this loop belongs to has ended.
        ///
        /// Asked rather than assumed: the loop turns until the run is over, and
        /// what ends a run is decided where the run is.
        fn over(&mut self) -> bool;
    }

    /// The forwarding machine's own keyboards, read the way Windows offers
    /// them: a window for raw input, a hook for the keys, and a message a turn
    /// hands over.
    ///
    /// Not `Send`, for [`SinkCapture`]'s reason.
    pub trait SourceCapture: Reading {
        /// Register the class the window below is made of.
        ///
        /// Answers nothing, because the call says nothing worth reading: a class
        /// already registered is the ordinary case on a second run, and whether
        /// the class exists is what making the window answers.
        fn register_the_window_class(&mut self);

        /// Make a window with no screen presence for raw input to arrive at.
        fn make_a_window(&mut self) -> bool;

        /// Ask for the mice, delivered whether or not that window has the
        /// foreground.
        ///
        /// The keyboards are not asked for: a key that is refused never reaches
        /// raw input, so the only place one can be both read and refused is the
        /// hook that refuses it
        /// (`docs/platform/windows/hooks-and-raw-input.md`).
        fn ask_for_the_mice(&mut self) -> bool;

        /// Say where the keyboard hook is to hand its keys, which is this
        /// thread.
        fn send_the_keys_to_that_window(&mut self);

        /// State the two positions a hook compares a key against.
        ///
        /// Which keys the chord is made of, and their positions on this
        /// keyboard, are settled before this is reached (ADR-0013).
        fn state_the_chord(&mut self);

        /// Start the timer that wakes this loop often enough to look for a
        /// probe.
        ///
        /// Answers nothing, because a run carries on without it: the probes then
        /// go unanswered and the watchdog ends this process, which is the safe
        /// direction (ADR-0008).
        fn start_the_probe_timer(&mut self);

        /// Put the keyboard's procedure on this thread, which is what refusing a
        /// key here is.
        fn hook_the_keyboard(&mut self) -> bool;

        /// Put the pointer's procedure on the same thread.
        fn hook_the_pointer(&mut self) -> bool;

        /// The next device a report named that nothing has a number for, under
        /// this number.
        ///
        /// One operation and not the sequence [`SinkCapture`] needs, because a
        /// device here is not opened at all: what says there is one is a report
        /// naming a handle nothing has numbered, and the handle is already
        /// reporting.
        fn next_device(&mut self, as_this: DeviceId) -> Option<Found>;

        /// Take out of the turn just made whatever it produced, and put it where
        /// [`SourceCapture::next_arrival`] reads it.
        ///
        /// Its own call and not part of the wait, because a message hands over a
        /// payload and reading that payload is a second call into the machine.
        fn take_what_the_turn_produced(&mut self);

        /// Give the machine back whatever the turn handed over.
        ///
        /// After everything it produced has been read off, because the payload
        /// is memory the system allocated to deliver with: reading it once it
        /// has been given back is reading freed memory, and never giving it back
        /// is that memory held for the life of the run. Not called at all for a
        /// turn that came back with nothing.
        fn let_go_of_the_turn(&mut self);

        /// The next thing the turn just taken produced and could not name yet,
        /// and none once it has none waiting.
        ///
        /// Put down rather than delivered because the number a device is called
        /// by is not known when its report arrives: the report is what says
        /// there is a device, and the number is the run's to give (ADR-0006).
        /// Asked after [`SourceCapture::next_device`] for that reason — the
        /// number arrives there, in this same turn.
        fn next_arrival(&mut self) -> Option<EventKind>;
    }

    /// The converting machine's own keyboards, read the way IOKit offers them:
    /// a registry to find them in, a seize to take one away, and a queue its
    /// values come off.
    ///
    /// Not `Send`, unlike the ends of the other loops: this one is made on the
    /// thread it is turned on and never leaves it, and what a platform ties to
    /// that thread — a run loop source, a queue — is exactly what cannot be
    /// sent. What crosses to the thread is [`SinkLoop`], and that is `Send`.
    pub trait SinkCapture: Reading {
        /// Open a port for IOKit to deliver notifications on.
        fn open_a_notification_port(&mut self) -> bool;

        /// Ask to be told about every HID device that arrives, on that port.
        fn ask_to_be_told_about_devices(&mut self) -> bool;

        /// Put that port's source on this thread's run loop, without which
        /// nothing it was asked for is ever delivered.
        ///
        /// Answers nothing: the call says nothing about it, and a source that
        /// went nowhere shows up as arrivals never coming.
        fn put_that_port_on_this_loop(&mut self);

        /// Look at the devices already attached, and say whether the registry
        /// gave a way to.
        ///
        /// Its own operation and after the asking above, because which of the
        /// two comes first is the whole of whether a keyboard can be missed —
        /// and that order is `engine`'s.
        fn look_at_the_devices_here(&mut self) -> bool;

        /// The next registry entry either listing has, as the thing the calls
        /// below are made on.
        ///
        /// `None` once it has none waiting. What it stands for is the machine's
        /// own and the run only hands it back (see [`crate::Handed`]) — which is
        /// what lets the order over the calls that make a device readable be
        /// driven from where a test reaches it, rather than written behind them
        /// (ADR-0006).
        fn next_device(&mut self) -> Option<crate::Handed>;

        /// The registry's own id for it, and none where it will not give one.
        ///
        /// Its own call because it is what says two findings are the same
        /// device, and a device it will not name cannot be told from one already
        /// being read.
        fn name_of(&mut self, found: crate::Handed) -> Option<u64>;

        /// A HID device over that entry, and none where it could not be made.
        fn open_it(&mut self, found: crate::Handed) -> Option<crate::Handed>;

        /// Give the entry back, which is a separate thing from the device made
        /// from it.
        fn let_go_of_the_finding(&mut self, found: crate::Handed);

        /// Ask to be told when it goes away.
        fn watch_for_its_removal(&mut self, device: crate::Handed);

        /// Put it on this thread's run loop, where what it reports arrives.
        fn put_it_where_reports_arrive(&mut self, device: crate::Handed);

        /// The properties it carries, under this number.
        fn describe(&mut self, device: crate::Handed, as_this: DeviceId) -> EventKind;

        /// Keep it under this number, so the run can ask for it by that later.
        fn keep(&mut self, device: crate::Handed, as_this: DeviceId);

        /// Give back one the run is not keeping.
        fn let_go_of_the_device(&mut self, device: crate::Handed);

        /// Let go of a device this loop was told about and does not want.
        ///
        /// Its own operation because the alternative is a host deciding: a loop
        /// that let go of whatever was not asked for next would be reading the
        /// absence of a call as an answer.
        fn forget(&mut self, device: DeviceId);

        /// The next thing the run has asked of this loop, and none once it has
        /// asked for nothing more.
        ///
        /// The calls IOKit ties to this thread — opening a device, scheduling
        /// its queue — cannot be made from the run's own loop, so what the run
        /// asks arrives here. What it asked for crosses as one of these rather
        /// than being acted on where it arrived, because acting on it is a
        /// sequence of those calls and the order over them is the run's
        /// (ADR-0006).
        fn next_ask(&mut self) -> Option<Ask>;

        /// Open the device this ask names, exclusively or not, and answer with
        /// the platform's own code.
        ///
        /// The number and not whether it worked, because which failure it is
        /// decides what a person has to do about it — a keyboard something else
        /// already holds and one that has gone are different problems.
        fn seize(&mut self, device: crate::Handed, exclusive: bool) -> i32;

        /// Keep it among the ones this run took away from everything else,
        /// where the open says it did.
        ///
        /// Only those: the others are open and nothing has to give them back.
        fn keep_what_was_seized(&mut self, device: crate::Handed, code: i32, exclusive: bool);

        /// Every element the device declares, as the thing a queue is filled
        /// from.
        fn every_element_of(&mut self, device: crate::Handed) -> Option<crate::Handed>;

        /// A queue over that device, deep enough not to drop what a person
        /// types.
        fn make_a_queue_over(&mut self, device: crate::Handed) -> Option<crate::Handed>;

        /// How many elements that is.
        fn how_many_elements_of(&mut self, elements: crate::Handed) -> usize;

        /// Whether there is one at that place at all.
        ///
        /// Asked rather than left to the call below to answer, because what a
        /// machine does when handed nothing where an element should be is its
        /// own — and not worth finding out.
        fn has_an_element_at(&mut self, elements: crate::Handed, at: usize) -> bool;

        /// Put the one at that place on the queue, and say whether it went on.
        ///
        /// One at a time and every one of them, not only the ones a table
        /// currently names: which pages and usages are worth reading is that
        /// table's question and a table is `engine`'s — filtering behind this
        /// call would be a second copy of it, kept or not kept in step by hand.
        fn put_one_element_on(
            &mut self,
            queue: crate::Handed,
            elements: crate::Handed,
            at: usize,
        ) -> bool;

        /// Give the elements back, which is a separate thing from the queue
        /// filled from them.
        fn let_go_of_the_elements(&mut self, elements: crate::Handed);

        /// Ask to be told when that queue has values.
        fn watch_for_its_values(&mut self, queue: crate::Handed);

        /// Put it where those tellings will arrive.
        fn put_it_where_values_arrive(&mut self, queue: crate::Handed);

        /// Start it delivering.
        fn start_it_delivering(&mut self, queue: crate::Handed);

        /// Keep the queue against the device it is over, so what arrives on it
        /// can be named.
        fn keep_the_queue(&mut self, queue: crate::Handed, device: crate::Handed);

        /// Give back a queue nothing was put on, which wakes for nothing.
        fn let_go_of_the_queue(&mut self, queue: crate::Handed);

        /// Answer the ask this loop is on, with the platform's own code — `0`
        /// for the one that went.
        ///
        /// Its own call and after the calls the ask was made of, because the run
        /// is waiting on it: an answer sent before the sequence finished would
        /// be a run reading a keyboard nothing had started delivering.
        fn answer_the_ask(&mut self, code: i32);

        /// The next queue IOKit has said has values on it, and none once it has
        /// said that of no more.
        ///
        /// Said rather than read where it was said: the callback IOKit tells
        /// runs inside the turn, so what it can do is put down which queue it
        /// was told about, and reading the values off is this same turn's drain
        /// (ADR-0006).
        fn next_thing_with_values(&mut self) -> Option<crate::Handed>;

        /// One value off it, and none once it has run dry.
        ///
        /// Drained until it does, because the telling says a value is available
        /// and not what it is — and the platform tells again only once the queue
        /// is empty.
        fn next_value_of(&mut self, from: crate::Handed) -> Option<EventKind>;

        /// How long ago, in nanoseconds, this machine stamped a value.
        ///
        /// Its own call because the stamp is on a clock only this machine can
        /// read: both ends of the measurement are here, and what crosses is the
        /// one number it came to (ADR-0010).
        fn how_long_ago(&mut self, stamp: u64) -> u64;

        /// What says that queue has run dry, which is as certain a report
        /// boundary as a pointer's stamp changing — assembling one from the
        /// values is `engine`'s.
        fn nothing_more_of(&mut self, from: crate::Handed) -> EventKind;

        /// Let go of it as one with values on it, so the platform is asked again
        /// the next time it has some.
        fn done_with(&mut self, from: crate::Handed);

        /// The next device IOKit has said has gone, and none once it has said
        /// that of no more.
        ///
        /// Put down where it was said, for
        /// [`SinkCapture::next_thing_with_values`]'s reason.
        fn next_gone(&mut self) -> Option<crate::Handed>;

        /// The queue over it, and none where nothing was reading it.
        fn its_queue(&mut self, device: crate::Handed) -> Option<crate::Handed>;

        /// Stop it delivering.
        fn stop_it_delivering(&mut self, queue: crate::Handed);

        /// Give the device back, and answer with the platform's own code.
        ///
        /// Worth making for a keyboard that is already gone: what a seize is is
        /// state held against a device that may come back, and one left behind
        /// makes a reconnect look permanently taken.
        fn give_it_back(&mut self, device: crate::Handed) -> i32;

        /// Let go of it as one this loop knows about, and say what the run
        /// called it.
        fn forget_it(&mut self, device: crate::Handed) -> Option<DeviceId>;
    }
}
