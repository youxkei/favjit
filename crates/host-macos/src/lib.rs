//! The macOS side of the host boundary (ADR-0006).
//!
//! Capture comes from the IOKit registry, one `IOHIDQueue` per keyboard, because
//! that is the level where the originating keyboard arrives with every event,
//! where keys with no `kVK_` constant still have a value, and where a seize
//! actually suppresses (see [`capture`] and
//! `docs/platform/macos/input-suppression.md`).
//!
//! Output goes back out as HID reports to a virtual device (`docs/platform/macos/output-through-a-virtual-hid-device.md`), so both
//! ends of this host speak usages. That is what lets a seized keyboard's pointer
//! be relayed at all, what puts converted keystrokes below Secure Keyboard Entry,
//! and what makes `LMGetKbdType()` answer with the keyboard favjit sends through.
//!
//! **Suppression takes privilege.** Without it the seize is refused and the
//! physical keystroke reaches applications alongside the converted one, so
//! injecting produces both. [`DryRun`] converts and injects nothing, which is
//! how the layout is checked without either hazard.
//!
//! Every operation here is data turned into what one platform call needs, that
//! call, and its answer turned back — nothing that decides, and nothing that
//! reaches `engine` to borrow a decision from it (ADR-0005, ADR-0006). Whatever
//! lives inside a host is out of reach of the end-to-end suite, which is the
//! reason to keep it small.

#![cfg(target_os = "macos")]

pub mod acceleration;
mod capture;
mod cf;
pub mod control;
mod ffi;
mod inject;
pub mod link;
pub mod pairing;
mod region;
mod repeat;
mod supervisor;
mod vhid;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;
use std::time::Duration;

use favjit_host::sink::{SinkHost, SinkInputHost};
use favjit_host::{Entropy, Host, HostEvent, Instant};

pub use capture::Captured;
pub use inject::{ax_trusted, ax_trusted_asking, hid_access, request_hid_access, HidAccess};
pub use region::{no_region, passed_down as trace_descriptor, Region};
pub use repeat::{hid_systems, Parameters, System, Systems};
pub use supervisor::{end as supervisor_end, Supervisor};
pub use vhid::{VIRTUAL_KEYBOARD_PRODUCT, VIRTUAL_KEYBOARD_VENDOR};

/// This machine's own entropy, for the places `engine` asks a host for some.
///
/// One call, through the crate that wraps whichever the platform has: opening a
/// device and reading it would be two, and a generator seeded in this process
/// would be one more thing to be wrong about — the code protects an exchange for
/// exactly as long as it is unpredictable (ADR-0012).
pub(crate) fn urandom(into: &mut [u8]) -> bool {
    getrandom::getrandom(into).is_ok()
}

/// The real thing: local keyboards in, converted keystrokes out.
pub struct MacOsHost {
    /// What the output device's pointer should be set to, already clamped to
    /// what the device accepts — the clamping is [`favjit_hid::Wanted`]'s, so
    /// what this crate holds is the two numbers and nothing that decided them.
    pointer: (Option<f64>, Option<f64>),
    /// The file whose presence means converting is switched off.
    ///
    /// A file rather than a signal or a socket, because the thing that writes it is
    /// a menu bar item in somebody's login session and the thing that reads it is a
    /// root daemon with none: a path both can reach is the whole of what they share
    /// (`docs/platform/macos/install-as-a-daemon-and-turn-off-with-a-file.md`).
    ///
    /// An empty path where the run was given none to watch, so that asking is
    /// one call rather than a question about whether there is a file to ask
    /// about: what the file's absence means is that converting is on, and a path
    /// nothing sits at is absent (ADR-0006).
    control: std::path::PathBuf,
    captured: Receiver<capture::Captured>,
    /// The way into the stream the loop reads.
    ///
    /// Made here rather than by the capture, so that the link from the other machine
    /// can put its events where local keyboards put theirs before any keyboard has
    /// been taken: one stream is what makes the order the loop sees the order things
    /// happened (ADR-0006).
    events: Sender<capture::Captured>,
    clock: capture::Clock,
    supervisor: Supervisor,
    identity: link::IdentityFile,
    /// Whether the loop serving the output device's connection is still
    /// turning, which is the whole of what says the device is still there:
    /// shared with the thread that turns it, and cleared where that thread
    /// comes back.
    output_serving: Arc<AtomicBool>,
    /// Set from outside to end the run.
    ///
    /// Ending the loop rather than exiting the process, because the way out matters
    /// here: `Drop` is what tells the virtual keyboard nothing is held any more, and
    /// that device outlives this process — a modifier left down in the last report
    /// stays down for whatever runs next.
    ///
    /// One nobody outside holds where a run was given none, for the reason the
    /// control file's path is what it is: reading it is then one call rather
    /// than a question about whether there is a flag to read.
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl MacOsHost {
    /// Nothing is opened and nothing is taken here.
    ///
    /// The keyboards are taken when the run asks for them, which is what lets the
    /// order be `engine`'s: a host that seized as it was constructed would have done
    /// it before anything could find out whether there was an output to send to
    /// (ADR-0008).
    pub fn new(
        pointer: (Option<f64>, Option<f64>),
        control: std::path::PathBuf,
        supervisor: Supervisor,
    ) -> Self {
        // The channel now and the devices later: the link needs somewhere to put
        // what arrives from the other machine, and it opens before any keyboard is
        // taken.
        let (events, captured) = std::sync::mpsc::channel();
        Self {
            pointer,
            control,
            events,
            captured,
            clock: capture::Clock::start(),
            supervisor,
            identity: link::IdentityFile::default(),
            output_serving: Arc::new(AtomicBool::new(false)),
            stop: Arc::new(AtomicBool::new(false)),
        }
    }

    /// End the run when this flag is set.
    ///
    /// A flag rather than a signal handler or an `exit` from another thread,
    /// because the way out matters: the loop returning is what runs `Drop`, and
    /// `Drop` is what tells the virtual keyboard nothing is held and gives the
    /// keyboards back.
    pub fn stop_on(&mut self, flag: std::sync::Arc<std::sync::atomic::AtomicBool>) {
        self.stop = flag;
    }
}

/// Whatever one wait on that queue answered, as the event it stands for.
///
/// One wait says one thing, so this is a conversion and not a record of facts to
/// apply: a loop the run handed over having come back, the reading having
/// stopped, and how long ago a value was stamped are each their own kind on the
/// stream, and the run is the end every one of them reaches (ADR-0006).
///
/// `at` is what a synthesised one is stamped with, because none of the three
/// carries a time of its own and the run compares what it is handed against the
/// clock it already read (ADR-0010).
fn arrived(
    received: Result<capture::Captured, std::sync::mpsc::RecvTimeoutError>,
    at: Instant,
) -> Option<HostEvent> {
    match received {
        Ok(capture::Captured::Event(event)) => Some(event),
        Ok(capture::Captured::AlongsideStopped) => Some(HostEvent {
            at,
            kind: favjit_host::EventKind::AlongsideStopped,
        }),
        Ok(capture::Captured::Delay(nanos)) => Some(HostEvent {
            at,
            kind: favjit_host::EventKind::Delay(nanos),
        }),
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => Some(HostEvent {
            at,
            kind: favjit_host::EventKind::InputGone,
        }),
        // The ordinary state of a keyboard nobody is touching.
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => None,
    }
}

impl SinkInputHost for MacOsHost {
    fn switched_on(&mut self) -> bool {
        control::is_converting(&self.control)
    }

    fn may_read_input(&mut self) -> bool {
        inject::hid_access() == HidAccess::Granted
    }

    fn request_input_permission(&mut self) -> bool {
        inject::request_hid_access()
    }

    fn output_connected(&mut self) -> bool {
        self.output_serving.load(Ordering::SeqCst)
    }

    fn stop_requested(&mut self) -> bool {
        self.stop.load(Ordering::SeqCst)
    }

    fn look_for_devices(
        &mut self,
        work: favjit_host::capture::SinkLoop,
    ) -> Option<Box<dyn favjit_host::sink::Capturing>> {
        let (reading, clock) = capture::capture(self.events.clone(), self.supervisor, work);
        // The clock is taken beside the capture because the stamps on what
        // arrives are that thread's: one started this side would put every
        // reading on a different origin from them.
        self.clock = clock;
        Some(Box::new(reading))
    }
}

impl Host for MacOsHost {
    fn now(&mut self) -> Instant {
        self.clock.now()
    }

    /// One read from the channel, whatever the capture thread sent up.
    ///
    /// One wait and one turn of the conversion over what it answered — not a
    /// loop around them. A loop here waits again for the whole of the bound
    /// after absorbing something, so a stream of the messages the run does not
    /// name makes the bound it set unbounded; and the run is the end that holds
    /// the clock and can subtract what has already gone (ADR-0010).
    fn next_event(&mut self, deadline: Instant) -> Option<HostEvent> {
        let now = self.clock.now();
        arrived(
            self.captured.recv_timeout(Duration::from_nanos(
                deadline.nanos.saturating_sub(now.nanos),
            )),
            now,
        )
    }

    fn is_supervised(&mut self) -> bool {
        self.supervisor.is_supervised()
    }

    fn warn(&mut self, message: core::fmt::Arguments) {
        log::warn!("{message}");
    }

    fn heartbeat(&mut self) -> Result<(), favjit_host::Trouble> {
        beaten(self.supervisor.beat())
    }
}

/// A beat that went, or the machine's own words about the one that did not.
fn beaten(answered: std::io::Result<()>) -> Result<(), favjit_host::Trouble> {
    answered.map_err(|error| favjit_host::Trouble(error.to_string()))
}

/// The link a bind opened, and nothing where it opened none.
fn bound(
    opened: std::io::Result<link::Link>,
) -> Option<Box<dyn favjit_host::link::LinkHost + Send>> {
    opened
        .ok()
        .map(|link| Box::new(link) as Box<dyn favjit_host::link::LinkHost + Send>)
}

impl Entropy for MacOsHost {
    fn fill(&mut self, into: &mut [u8]) -> bool {
        urandom(into)
    }
}

/// The identity file, one call at a time.
impl favjit_host::IdentityStore for MacOsHost {
    fn read(&mut self) -> Option<Vec<u8>> {
        self.identity.read()
    }

    fn make_directory(&mut self) -> Result<(), favjit_host::Trouble> {
        self.identity.make_directory()
    }

    fn open(&mut self) -> Result<Box<dyn favjit_host::Writing>, favjit_host::Trouble> {
        self.identity.open()
    }
}

impl favjit_host::PointerHost for MacOsHost {
    fn wanted_pointer_feel(&mut self) -> (Option<f64>, Option<f64>) {
        self.pointer
    }

    fn output_vendor(&mut self) -> i64 {
        VIRTUAL_KEYBOARD_VENDOR as i64
    }

    fn open_event_system(&mut self) -> Option<Box<dyn favjit_host::Pointers>> {
        acceleration::viewing(acceleration::event_system())
    }

    fn open_simple_event_system(&mut self) -> Option<Box<dyn favjit_host::Pointers>> {
        acceleration::viewing(acceleration::simple_event_system())
    }
}

impl SinkHost for MacOsHost {
    fn reach_the_output(
        &mut self,
    ) -> Result<Box<dyn favjit_host::sink::Reaching>, favjit_host::NoOutput> {
        inject::Reached::reach(self.clock, self.events.clone())
    }

    fn instead_of_the_output(&mut self) -> Box<dyn favjit_host::sink::Injecting> {
        Box::new(inject::Nowhere)
    }

    /// Turned on a thread of its own, and its coming back is what says the
    /// output device has gone.
    ///
    /// Not [`MacOsHost::run_alongside`]: that loop coming back means the link
    /// is no longer served, which a run acts on differently — one signal for
    /// both would report a dead output device as a machine nobody can reach.
    fn run_output_alongside(&mut self, work: Box<dyn FnOnce() + Send>) -> bool {
        let theirs = Arc::clone(&self.output_serving);
        self.output_serving.store(true, Ordering::SeqCst);
        // Named, for the reason the link's thread is: a run holding the
        // keyboards has more than one loop, and which of them stopped is the
        // first thing worth knowing off a sample of a wedged favjit
        // (ADR-0008).
        // Reported and not said: what a run does about an output device nothing
        // is answering for is its own, and so are the words (ADR-0006).
        // Cleared by that thread and by nothing else. A thread that never
        // started leaves it set, and nothing reads it then: what a run does
        // about a loop that could not be turned is decided off the answer this
        // gives, before there is anything to ask about the connection.
        std::thread::Builder::new()
            .name("favjit-output".into())
            .spawn(move || {
                work();
                theirs.store(false, Ordering::SeqCst);
            })
            .is_ok()
    }

    fn bind_link(&mut self) -> Option<Box<dyn favjit_host::link::LinkHost + Send>> {
        bound(link::Link::bind(self.clock, self.events.clone()))
    }

    fn run_alongside(&mut self, work: Box<dyn FnOnce() + Send>) -> bool {
        let events = self.events.clone();
        // Named, because this thread is the one a sample of a wedged favjit has to
        // be read against: a run holding the keyboards has two loops, and which of
        // them stopped is the first thing worth knowing (ADR-0008).
        std::thread::Builder::new()
            .name("favjit-link".into())
            .spawn(move || {
                work();
                // Into the stream rather than nowhere: the run decides what the end
                // of this loop means, and a thread that returned in silence would
                // leave a converter that looks well while the other machine cannot
                // reach it at all.
                let _ = events.send(capture::Captured::AlongsideStopped);
            })
            // Reported and not said, for the reason nothing else here says
            // anything: what a run does about a link it cannot serve, and
            // what it says about it, are its own (ADR-0006).
            .is_ok()
    }
}

/// Converts for real, injects nothing.
///
/// The point of running this way is that it is safe to run at all: with nothing
/// suppressed, a host that injects gives every keystroke twice. This one reports
/// what it would have sent, which is enough to check the layout against real
/// hardware and to find out which usage a key `engine`'s tables have no name for
/// reports.
pub struct DryRun {
    inner: MacOsHost,
}

impl DryRun {
    pub fn new(control: std::path::PathBuf, supervisor: Supervisor) -> Self {
        Self {
            inner: MacOsHost::new((None, None), control, supervisor),
        }
    }

    pub fn host(&self) -> &MacOsHost {
        &self.inner
    }

    pub fn stop_on(&mut self, flag: std::sync::Arc<std::sync::atomic::AtomicBool>) {
        self.inner.stop_on(flag);
    }
}

impl SinkInputHost for DryRun {
    fn switched_on(&mut self) -> bool {
        self.inner.switched_on()
    }

    fn may_read_input(&mut self) -> bool {
        self.inner.may_read_input()
    }

    fn request_input_permission(&mut self) -> bool {
        self.inner.request_input_permission()
    }

    fn output_connected(&mut self) -> bool {
        self.inner.output_connected()
    }

    fn stop_requested(&mut self) -> bool {
        self.inner.stop_requested()
    }

    fn look_for_devices(
        &mut self,
        work: favjit_host::capture::SinkLoop,
    ) -> Option<Box<dyn favjit_host::sink::Capturing>> {
        self.inner.look_for_devices(work)
    }
}

impl Host for DryRun {
    fn now(&mut self) -> Instant {
        self.inner.now()
    }

    fn next_event(&mut self, deadline: Instant) -> Option<HostEvent> {
        self.inner.next_event(deadline)
    }

    fn is_supervised(&mut self) -> bool {
        self.inner.is_supervised()
    }

    fn warn(&mut self, message: core::fmt::Arguments) {
        self.inner.warn(message);
    }

    fn heartbeat(&mut self) -> Result<(), favjit_host::Trouble> {
        self.inner.heartbeat()
    }
}

impl Entropy for DryRun {
    fn fill(&mut self, into: &mut [u8]) -> bool {
        self.inner.fill(into)
    }
}

/// Nothing is read and nothing is written, so a dry run leaves no identity behind.
///
/// Not delegated to the file the converter uses: a run that opens no socket has
/// nothing to present a key to, and making one would write a file — which is a
/// change outside a process that exists to make none.
impl favjit_host::IdentityStore for DryRun {
    fn read(&mut self) -> Option<Vec<u8>> {
        None
    }

    fn make_directory(&mut self) -> Result<(), favjit_host::Trouble> {
        Err(refuses())
    }

    fn open(&mut self) -> Result<Box<dyn favjit_host::Writing>, favjit_host::Trouble> {
        Err(refuses())
    }
}

/// What a dry run answers about a file it will not touch.
fn refuses() -> favjit_host::Trouble {
    favjit_host::Trouble(String::from(
        "a dry run writes no identity, because it opens no socket to present one to",
    ))
}

/// Nothing is asked for and nothing is found, so nothing is written: a dry run
/// leaves the machine's pointers as they were, the same as it leaves its
/// keyboards.
impl favjit_host::PointerHost for DryRun {
    fn wanted_pointer_feel(&mut self) -> (Option<f64>, Option<f64>) {
        (None, None)
    }

    fn output_vendor(&mut self) -> i64 {
        VIRTUAL_KEYBOARD_VENDOR as i64
    }

    /// Neither view is opened, for the reason nothing else here is: a property
    /// written on a device outlives the process that wrote it, and a dry run
    /// changes nothing outside its own.
    fn open_event_system(&mut self) -> Option<Box<dyn favjit_host::Pointers>> {
        None
    }

    fn open_simple_event_system(&mut self) -> Option<Box<dyn favjit_host::Pointers>> {
        None
    }
}

impl SinkHost for DryRun {
    /// There is no output here, which is the whole of what a dry run is: a
    /// virtual keyboard or an open socket left behind would be a change
    /// outside a process that exists to make none.
    ///
    /// Refused rather than answered with a connection nothing is on the other
    /// end of: a run given one would ask it for the two devices and wait out
    /// its whole bound for an answer no socket is there to send.
    fn reach_the_output(
        &mut self,
    ) -> Result<Box<dyn favjit_host::sink::Reaching>, favjit_host::NoOutput> {
        Err(favjit_host::NoOutput::NoService)
    }

    fn instead_of_the_output(&mut self) -> Box<dyn favjit_host::sink::Injecting> {
        self.inner.instead_of_the_output()
    }

    fn bind_link(&mut self) -> Option<Box<dyn favjit_host::link::LinkHost + Send>> {
        None
    }

    /// Nothing is started, because nothing was bound for it to serve.
    fn run_alongside(&mut self, _work: Box<dyn FnOnce() + Send>) -> bool {
        false
    }

    /// Nothing is started, because nothing was opened for it to serve.
    fn run_output_alongside(&mut self, _work: Box<dyn FnOnce() + Send>) -> bool {
        false
    }
}
