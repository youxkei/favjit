//! The macOS side of the host boundary (ADR-0006).
//!
//! Capture comes from the IOKit registry, one `IOHIDQueue` per keyboard, because
//! that is the level where the originating keyboard arrives with every event,
//! where keys with no `kVK_` constant still have a value, and where a seize
//! actually suppresses (see [`capture`] and
//! `docs/platform/macos/input-suppression.md`).
//!
//! Output goes back out as HID reports to a virtual device (ADR-0011), so both
//! ends of this host speak usages. That is what lets a seized keyboard's pointer
//! be relayed at all, what puts converted keystrokes below Secure Keyboard Entry,
//! and what makes `LMGetKbdType()` answer with the keyboard favjit sends through.
//!
//! **Suppression takes privilege.** Without it the seize is refused and the
//! physical keystroke reaches applications alongside the converted one, so
//! injecting produces both. [`DryRun`] converts and injects nothing, which is
//! how the layout is checked without either hazard.
//!
//! Every operation is one call into the platform and the answer it gave, and the
//! order over them is `core`'s (ADR-0006): what is here is the translation between
//! macOS's vocabulary and `core`'s, and nothing that decides. Whatever lives inside
//! a host is out of reach of the end-to-end suite, which is the reason to keep it
//! small.

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

use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::time::Duration;

use favjit_core::link::LinkHost;
use favjit_core::pairing::{Identity, IdentityStore};
use favjit_core::pointer::Wanted;
use favjit_core::sink::SinkInputHost;
use favjit_core::{Ended, EventKind, Host, HostEvent, Injected, Instant, Key};

pub use capture::{capture, release, Captured, Clock, Config, Held, Seizure, UnknownUsage};
pub use inject::{access, ax_trusted, hid_access, request_hid_access, HidAccess};
pub use region::{inherited as inherited_trace_region, Region, TRACE_BYTES, TRACE_FD_VAR};
pub use repeat::system_repeat;
pub use supervisor::{Supervisor, HEARTBEAT_FD_VAR, PROBE_FD_VAR};
pub use vhid::{Unavailable, VIRTUAL_KEYBOARD_PRODUCT, VIRTUAL_KEYBOARD_VENDOR};

/// The real thing: local keyboards in, converted keystrokes out.
pub struct MacOsHost {
    /// Which keyboards to watch, and whether to take them exclusively.
    config: Config,
    /// What the output device's pointer should be set to.
    pointer: Wanted,
    /// The file whose presence means converting is switched off.
    ///
    /// A file rather than a signal or a socket, because the thing that writes it is
    /// a menu bar item in somebody's login session and the thing that reads it is a
    /// root daemon with none: a path both can reach is the whole of what they share
    /// (ADR-0012).
    control: Option<std::path::PathBuf>,
    captured: Receiver<capture::Captured>,
    /// The way into the stream the loop reads.
    ///
    /// Made here rather than by the capture, so that the link from the other machine
    /// can put its events where local keyboards put theirs before any keyboard has
    /// been taken: one stream is what makes the order the loop sees the order things
    /// happened (ADR-0006).
    events: Sender<capture::Captured>,
    /// What has to be released to give the keyboards back, once a run has asked for
    /// them.
    ///
    /// Absent until then, because taking a keyboard is what a run decides the moment
    /// for: a host that opened the devices as it was constructed would have seized
    /// before anything could find out whether there was an output to send to
    /// (ADR-0008). It is also what says the stream is worth reading.
    held: Option<capture::Held>,
    clock: Clock,
    /// The wake-up the loop asked for, if any.
    timer: Option<Instant>,
    /// Why the stream ended, for the run to be told once it has.
    ended: Ended,
    supervisor: Supervisor,
    /// Absent until the run asks for the output and it comes up.
    ///
    /// Opening it in the constructor would create a virtual keyboard for a dry
    /// run, which exists precisely to change nothing outside this process.
    injector: Option<inject::Injector>,
    /// Whether anything was asked for with no device to send it through.
    output_missing: bool,
    /// Reported rather than swallowed, so a key the tables cannot express is
    /// visible instead of silently absent.
    pub unknown: Vec<UnknownUsage>,
    pub unsendable: Vec<Key>,
    pub seizures: Vec<Seizure>,
    /// Where favjit's own latency goes, in nanoseconds per keystroke.
    ///
    /// Kept because the suppressing configuration is the one that matters and the
    /// only thing that can see inside it: with a keyboard seized, nothing else can
    /// open the device to measure the same segment from outside.
    pub latency: Latency,
    /// The time on the event being handled, so the pipeline segment can be read
    /// off the injection without `core` having to carry it.
    handling: Instant,
    link_broken: bool,
    /// When to stop saying anything more is coming.
    ///
    /// Held here rather than checked by the caller between events, because a
    /// caller only gets to look when an event arrives: a run nobody types into
    /// would keep its keyboards seized for as long as the process lived, and the
    /// bound is the whole reason a suppressing run is safe to start.
    until: Option<std::time::Instant>,
    /// Set from outside to end the run.
    ///
    /// Ending the loop rather than exiting the process, because the way out matters
    /// here: `Drop` is what tells the virtual keyboard nothing is held any more, and
    /// that device outlives this process — a modifier left down in the last report
    /// stays down for whatever runs next.
    stop: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
}

/// How often the wait comes back to look at [`MacOsHost::stop_on`]'s flag.
///
/// Only while there is a flag: without one the wait has nothing to come back for,
/// and a run with neither a deadline nor a timer should park on the channel rather
/// than wake four times a second for as long as it lives.
const STOP_POLL: Duration = Duration::from_millis(250);

/// How long to wait for the output device to become ready.
///
/// Long enough for a driver that has to activate, short enough that a machine
/// without the package installed says so rather than appearing to hang. There is
/// nothing else to wait for: the service reports readiness, and a wrong protocol
/// version is silently ignored, so a timeout is the only signal it will ever give
/// (`docs/platform/macos/virtual-hid-device.md`).
const OUTPUT_WAIT: Duration = Duration::from_secs(5);

/// How often the control file is looked at.
///
/// Frequent enough that switching converting on feels immediate, rare enough to be
/// nothing at all: what is being watched is a file appearing or disappearing, which
/// nothing reports.
const CONTROL_POLL: Duration = Duration::from_millis(250);

/// Nanoseconds per keystroke, one list per segment of the path.
#[derive(Debug, Clone, Default)]
pub struct Latency {
    /// HID system's stamp to the capture thread reaching the value.
    pub arrival: Vec<u64>,
    /// That arrival to the converted keystroke being written to the device,
    /// which is the channel and the conversion together.
    pub pipeline: Vec<u64>,
    /// Writing the report to the device's socket.
    pub post: Vec<u64>,
}

impl MacOsHost {
    /// Nothing is opened and nothing is taken here.
    ///
    /// The keyboards are taken when the run asks for them, which is what lets the
    /// order be `core`'s: a host that seized as it was constructed would have done
    /// it before anything could find out whether there was an output to send to
    /// (ADR-0008).
    pub fn new(config: Config, pointer: Wanted, control: Option<std::path::PathBuf>) -> Self {
        // The channel now and the devices later: the link needs somewhere to put
        // what arrives from the other machine, and it opens before any keyboard is
        // taken.
        let (events, captured) = std::sync::mpsc::channel();
        Self {
            config,
            pointer,
            control,
            events,
            captured,
            held: None,
            clock: Clock::start(),
            timer: None,
            ended: Ended::AsAsked,
            supervisor: Supervisor::from_env(),
            injector: None,
            output_missing: false,
            unknown: Vec::new(),
            unsendable: Vec::new(),
            seizures: Vec::new(),
            latency: Latency::default(),
            handling: Instant::ZERO,
            link_broken: false,
            until: None,
            stop: None,
        }
    }

    /// End the run when this flag is set.
    ///
    /// A flag rather than a signal handler or an `exit` from another thread,
    /// because the way out matters: the loop returning is what runs `Drop`, and
    /// `Drop` is what tells the virtual keyboard nothing is held and gives the
    /// keyboards back.
    pub fn stop_on(&mut self, flag: std::sync::Arc<std::sync::atomic::AtomicBool>) {
        self.stop = Some(flag);
    }

    /// Stop the stream at this wall-clock moment, whether or not anything is
    /// typed.
    pub fn until(&mut self, deadline: Option<std::time::Instant>) {
        self.until = deadline;
    }

    /// Whether the connection to the device is still up.
    ///
    /// Asked rather than waited on: the device belongs to a daemon that can go away,
    /// and it going away is indistinguishable from favjit working until something is
    /// typed.
    fn output_is_live(&self) -> bool {
        self.injector
            .as_ref()
            .is_some_and(inject::Injector::is_connected)
    }

    /// Keys the layout names that no HID usage is yet known for.
    pub fn unmapped_keys() -> &'static [Key] {
        capture::unmapped_keys()
    }

    /// Note the event on its way out, so an injection made while handling it can
    /// be timed against it.
    ///
    /// Every event goes through here, a timer's wake-up included: one that did
    /// not would leave the previous event's time standing, and a repeat injected
    /// under it would read as however long the key had been held.
    fn handle(&mut self, event: HostEvent) -> HostEvent {
        self.handling = event.at;
        event
    }

    /// Whether the machine still has events to give, and why not when it has not.
    ///
    /// Asked around the wait rather than reported through an event, because a
    /// keyboard nobody is typing on produces nothing to report it on: what notices
    /// any of these is whatever is waiting, and that is this.
    fn still_going(&mut self) -> Option<Ended> {
        if self.injector.is_some() && !self.output_is_live() {
            return Some(Ended::OutputGone);
        }
        if self
            .stop
            .as_ref()
            .is_some_and(|flag| flag.load(std::sync::atomic::Ordering::SeqCst))
        {
            return Some(Ended::AsAsked);
        }
        if !self.switched_on() {
            return Some(Ended::SwitchedOff);
        }
        if self
            .until
            .is_some_and(|deadline| std::time::Instant::now() >= deadline)
        {
            return Some(Ended::AsAsked);
        }
        None
    }

    /// How long the one wait may last.
    ///
    /// One wait bounded by whichever comes first, rather than a thread per deadline:
    /// a second producer is the one thing that could reorder the stream ADR-0006
    /// keeps single, and it would have to hand its event over through this channel
    /// anyway. A wait that outlasted the run's bound would hold the keyboards past
    /// it, one that outlasted the repeat's timer would drop a repeat, and one that
    /// outlasted the poll would not notice being asked to stop.
    fn wait_for(&self) -> Option<Duration> {
        let left = self
            .until
            .map(|deadline| deadline.saturating_duration_since(std::time::Instant::now()));
        let timer = self.timer.map(|due| {
            Duration::from_nanos(due.as_nanos().saturating_sub(self.clock.now().as_nanos()))
        });
        let poll = self.stop.as_ref().map(|_| STOP_POLL);
        timer.into_iter().chain(left).chain(poll).min()
    }

    fn recv(&mut self) -> Option<HostEvent> {
        loop {
            if let Some(ended) = self.still_going() {
                self.ended = ended;
                return None;
            }

            let wait = self.wait_for();
            let received = match wait {
                Some(wait) => match self.captured.recv_timeout(wait) {
                    Ok(captured) => Ok(captured),
                    Err(RecvTimeoutError::Timeout) => {
                        // The wake-up the loop asked for, if that is what the wait
                        // was for. Its deadline is left set, because the loop
                        // cancels or replaces it and clearing it here would make a
                        // repeat that arrives while the loop is busy the last one.
                        if self.timer.is_some_and(|due| self.clock.now() >= due) {
                            return Some(
                                self.handle(HostEvent::new(self.clock.now(), EventKind::Timer)),
                            );
                        }
                        continue;
                    }
                    Err(RecvTimeoutError::Disconnected) => Err(()),
                },
                None => self.captured.recv().map_err(|_| ()),
            };
            match received {
                Ok(capture::Captured::Event(event)) => return Some(self.handle(event)),
                Ok(capture::Captured::AlongsideStopped) => {
                    self.ended = Ended::AlongsideStopped;
                    return None;
                }
                // The three the capture thread sends up for the report rather than
                // for the loop: what they are for is `report`, and a run that never
                // types produces none of them.
                Ok(capture::Captured::Delay(nanos)) => self.latency.arrival.push(nanos),
                Ok(capture::Captured::Unknown(unknown)) => {
                    if !self.unknown.contains(&unknown) {
                        self.unknown.push(unknown);
                    }
                }
                Ok(capture::Captured::Seized(seizure)) => self.seizures.push(seizure),
                Err(()) => {
                    self.ended = Ended::AsAsked;
                    return None;
                }
            }
        }
    }
}

impl Drop for MacOsHost {
    /// Give the keyboards back.
    ///
    /// The one path that reliably runs, and the only one there is: the capture
    /// thread's run loop never returns, so nothing on that side can release
    /// anything. A kill or a panic in the loop skips this, which is why
    /// ADR-0008 wants a watchdog rather than trusting a destructor.
    fn drop(&mut self) {
        if let Some(held) = self.held.as_ref() {
            capture::release(held);
        }
    }
}

impl SinkInputHost for MacOsHost {
    fn switched_on(&mut self) -> bool {
        self.control.as_deref().is_none_or(control::is_converting)
    }

    fn wait_until_on(&mut self) {
        let Some(control) = self.control.as_deref() else {
            return;
        };
        log::info!("converting is off ({}); waiting", control.display());
        // Polled rather than watched, because what has to be noticed is a file
        // being removed and a quarter of a second is not a delay a person typing
        // would see.
        while control.exists()
            && !self
                .stop
                .as_ref()
                .is_some_and(|flag| flag.load(std::sync::atomic::Ordering::SeqCst))
        {
            std::thread::sleep(CONTROL_POLL);
        }
    }

    fn may_read_input(&mut self) -> bool {
        // Both answers are logged because they disagree by design. The window
        // server's is what an event tap would need and is always no for a daemon,
        // which has no session — capturing goes through `IOHIDDevice`, so the HID
        // answer is the one that decides
        // (`docs/platform/macos/input-permissions.md`).
        let (listen, post) = inject::access();
        let hid = inject::hid_access();
        log::info!("input monitoring: hid={hid:?} (window server: listen={listen} post={post})");
        if hid == HidAccess::Granted {
            return true;
        }

        // Asked here rather than left to the first device open, which asks on the
        // process's behalf: at startup the answer can be logged beside the question.
        let granted = inject::request_hid_access();
        log::info!("asked for input monitoring: granted={granted}");
        granted
    }

    fn take_input(&mut self, suppress: bool) -> bool {
        let mut config = self.config.clone();
        config.suppress = suppress;
        let (held, clock) = capture::capture(config, self.events.clone());
        self.held = Some(held);
        // The clock the events are stamped with, so what this end reads and what
        // arrives on an event are the same clock.
        self.clock = clock;
        true
    }

    fn ended(&mut self) -> Ended {
        self.ended
    }
}

impl Host for MacOsHost {
    fn next_event(&mut self) -> Option<HostEvent> {
        self.recv()
    }

    fn is_supervised(&mut self) -> bool {
        self.supervisor.is_supervised()
    }

    fn warn(&mut self, message: core::fmt::Arguments) {
        log::warn!("{message}");
    }

    fn heartbeat(&mut self) {
        if let Err(error) = self.supervisor.beat() {
            // Once, not per event: the link does not come back, and a warning per
            // keystroke would be the loudest thing in the log for the rest of the
            // run.
            if !self.link_broken {
                self.link_broken = true;
                log::error!(
                    "the heartbeat is not reaching the watchdog ({error}); it will kill this \
                     process for a broken link rather than for a fault"
                );
            }
        }
    }
}

/// The identity file, one call at a time.
///
/// The wrapper is made per call rather than held in the host, because all it holds
/// is the path: keeping the bytes would answer a later read with an identity the
/// file no longer has.
impl IdentityStore for MacOsHost {
    fn read(&mut self) -> Option<Vec<u8>> {
        link::IdentityFile::default().read()
    }

    fn make(&mut self) -> Option<Identity> {
        link::IdentityFile::default().make()
    }

    fn keep(&mut self, bytes: &[u8]) -> bool {
        link::IdentityFile::default().keep(bytes)
    }
}

impl favjit_core::sink::SinkHost for MacOsHost {
    fn set_timer(&mut self, at: Option<Instant>) {
        self.timer = at;
    }

    fn open_output(&mut self) -> bool {
        match inject::Injector::open(OUTPUT_WAIT) {
            Ok(injector) => {
                self.injector = Some(injector);
                true
            }
            Err(unavailable) => {
                log::error!("{unavailable}");
                false
            }
        }
    }

    fn bind_link(&mut self, identity: &Identity) -> Option<Box<dyn LinkHost + Send>> {
        if link::authorized().is_empty() {
            // Said before anything connects, because a refusal looks from the other
            // end exactly like a machine that cannot be reached.
            log::warn!("nothing is paired: the link will refuse every source. Run favjit --pair");
        }

        let link = match link::Link::bind(identity.clone(), self.clock, self.events.clone()) {
            Ok(link) => link,
            Err(error) => {
                log::error!("cannot listen: {error}");
                return None;
            }
        };
        match link.port() {
            Ok(port) => log::info!(
                "the link is listening on port {port}; this machine's key is {}",
                link.fingerprint()
            ),
            Err(error) => log::error!("cannot tell which port the link got: {error}"),
        }
        Some(Box::new(link))
    }

    fn run_alongside(&mut self, work: Box<dyn FnOnce() + Send>) -> bool {
        let events = self.events.clone();
        // Named, because this thread is the one a sample of a wedged favjit has to
        // be read against: a run holding the keyboards has two loops, and which of
        // them stopped is the first thing worth knowing (ADR-0008).
        match std::thread::Builder::new()
            .name("favjit-link".into())
            .spawn(move || {
                work();
                log::error!("the link is no longer being served");
                // Into the stream rather than nowhere: the run decides what the end
                // of this loop means, and a thread that returned in silence would
                // leave a converter that looks well while the other machine cannot
                // reach it at all.
                let _ = events.send(capture::Captured::AlongsideStopped);
            }) {
            Ok(_) => true,
            Err(error) => {
                log::error!("cannot start the link: {error}");
                false
            }
        }
    }

    fn tune_output(&mut self) {
        if self.pointer.is_empty() {
            return;
        }
        for pointer in acceleration::pointers() {
            if pointer.vendor != Some(VIRTUAL_KEYBOARD_VENDOR as i64) {
                continue;
            }
            let before = (pointer.resolution(), pointer.acceleration());
            let done = pointer.tune(self.pointer.resolution(), self.pointer.acceleration());
            log::info!(
                "output pointer {:?}: resolution {:?} -> {:?}, acceleration {:?} -> {:?}{}",
                pointer.name.as_deref().unwrap_or("(unnamed)"),
                before.0,
                pointer.resolution(),
                before.1,
                pointer.acceleration(),
                if done { "" } else { " (refused)" }
            );
        }
    }

    fn inject(&mut self, injected: Injected) {
        let Some(injector) = self.injector.as_mut() else {
            // Once, not per keystroke: nothing is going to open the device from
            // here, so the rest of the run would be this line.
            if !self.output_missing {
                self.output_missing = true;
                log::error!(
                    "converted input has nowhere to go: no virtual HID device was opened, so \
                     nothing is reaching applications"
                );
            }
            return;
        };

        // Read before the post and again after, so the two segments are separated:
        // one number covering both would not say whether the time went into the
        // conversion or into the call, and those have different remedies.
        let before = self.clock.now();
        let result = injector.post(injected);
        let after = self.clock.now();
        self.latency
            .pipeline
            .push(before.as_nanos().saturating_sub(self.handling.as_nanos()));
        self.latency
            .post
            .push(after.as_nanos().saturating_sub(before.as_nanos()));
        if let Err(key) = result {
            if !self.unsendable.contains(&key) {
                self.unsendable.push(key);
            }
        }
    }
}

impl MacOsHost {}

/// Converts for real, injects nothing.
///
/// The point of running this way is that it is safe to run at all: with nothing
/// suppressed, a host that injects gives every keystroke twice. This one reports
/// what it would have sent, which is enough to check the layout against real
/// hardware and to find out which usage the keys in
/// [`MacOsHost::unmapped_keys`] report.
pub struct DryRun {
    inner: MacOsHost,
    pub would_inject: Vec<Injected>,
}

impl DryRun {
    pub fn new(config: Config, control: Option<std::path::PathBuf>) -> Self {
        Self {
            inner: MacOsHost::new(config, Wanted::default(), control),
            would_inject: Vec::new(),
        }
    }

    pub fn host(&self) -> &MacOsHost {
        &self.inner
    }

    pub fn stop_on(&mut self, flag: std::sync::Arc<std::sync::atomic::AtomicBool>) {
        self.inner.stop_on(flag);
    }

    /// Stop the stream at this wall-clock moment, whether or not anything is
    /// typed.
    pub fn until(&mut self, deadline: Option<std::time::Instant>) {
        self.inner.until(deadline);
    }
}

impl SinkInputHost for DryRun {
    fn switched_on(&mut self) -> bool {
        self.inner.switched_on()
    }

    fn wait_until_on(&mut self) {
        self.inner.wait_until_on();
    }

    fn may_read_input(&mut self) -> bool {
        self.inner.may_read_input()
    }

    fn take_input(&mut self, suppress: bool) -> bool {
        self.inner.take_input(suppress)
    }

    fn ended(&mut self) -> Ended {
        self.inner.ended()
    }
}

impl Host for DryRun {
    fn next_event(&mut self) -> Option<HostEvent> {
        self.inner.next_event()
    }

    fn is_supervised(&mut self) -> bool {
        self.inner.is_supervised()
    }

    fn warn(&mut self, message: core::fmt::Arguments) {
        self.inner.warn(message);
    }

    fn heartbeat(&mut self) {
        self.inner.heartbeat();
    }
}

/// Nothing is read and nothing is written, so a dry run leaves no identity behind.
///
/// Not delegated to the file the converter uses: a run that opens no socket has
/// nothing to present a key to, and making one would write a file — which is a
/// change outside a process that exists to make none.
impl IdentityStore for DryRun {
    fn read(&mut self) -> Option<Vec<u8>> {
        None
    }

    fn make(&mut self) -> Option<Identity> {
        None
    }

    fn keep(&mut self, _bytes: &[u8]) -> bool {
        false
    }
}

impl favjit_core::sink::SinkHost for DryRun {
    /// Passed through rather than dropped: a wake-up costs nothing outside this
    /// process, and a run that swallowed them would answer a held key differently
    /// from the run this one stands in for.
    fn set_timer(&mut self, at: Option<Instant>) {
        self.inner.set_timer(at);
    }

    /// Nothing is opened, which is the whole of what a dry run is: a virtual
    /// keyboard or an open socket left behind would be a change outside a process
    /// that exists to make none.
    fn open_output(&mut self) -> bool {
        true
    }

    fn tune_output(&mut self) {}

    fn bind_link(&mut self, _identity: &Identity) -> Option<Box<dyn LinkHost + Send>> {
        None
    }

    /// Nothing is started, because nothing was bound for it to serve.
    fn run_alongside(&mut self, _work: Box<dyn FnOnce() + Send>) -> bool {
        false
    }

    fn inject(&mut self, injected: Injected) {
        self.would_inject.push(injected);
    }
}
