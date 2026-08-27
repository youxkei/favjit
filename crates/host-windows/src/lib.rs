//! The Windows side of the host boundary (ADR-0006).
//!
//! Capture comes from raw input, one message per event, because that is the
//! level where the originating device arrives and where a mouse's movement is
//! still a movement rather than a cursor position. Refusing that input is a pair
//! of low-level hooks, because that is the only Windows input hook whose return
//! value can end an event. Why each, and why not the other for both, is in
//! [`capture`] and [`suppress`].
//!
//! **Nothing is injected here.** Input flows one way (ADR-0002), so this host
//! sends and never delivers, and the conversion is the sink's alone (ADR-0003):
//! what leaves this machine is what the hardware said, key for key.
//!
//! **Suppression takes no privilege here**, unlike the seize on the macOS side,
//! and it is not the default even so. Relaying without suppressing types every
//! keystroke on both machines; suppressing without relaying takes the keyboard
//! away. Which combination a run is in is `engine`'s, and so is what each one
//! does.
//!
//! Everything here is as thin as the platform allows — the calls, and what they
//! said in the shape they said it. **What Windows means by a keyboard report is
//! not here**: `KBDLLHOOKSTRUCT`'s fields cross as they arrive, and the prefix,
//! the transition and which reports are not keys at all are
//! `favjit-hid`'s, where both machines' `engine`s and `host-sim` read them and
//! the suite drives them. This crate depends on neither, which is the form of
//! that rule a compiler holds (ADR-0005, ADR-0006).
//!
//! Whatever lives inside a host is out of reach of the end-to-end suite, which
//! is the reason to keep it small. Only the modules that call the Win32 API are
//! behind the platform gate below; the rest are compiled everywhere. [`mdns`]
//! and [`link`] hold what is read off the network, and [`pairing`] holds the
//! exchange a code is spent on. Their tests are the only check any of that
//! gets, so they build wherever the suite runs.
//!
//! **A device's own description is not read here either.** What raw input gives
//! for a keyboard is an interface path, and the vendor and product written into
//! one are read where the rule that matches on them lives.

pub mod link;
pub mod mdns;
pub mod pairing;

#[cfg(windows)]
mod capture;
#[cfg(windows)]
mod ffi;
#[cfg(windows)]
mod region;
#[cfg(windows)]
mod supervisor;
#[cfg(windows)]
mod suppress;
/// Public, because the tray item is a program of its own and this is what it talks to a
/// running favjit through (`docs/platform/windows/tray-item-as-its-own-program.md`).
#[cfg(windows)]
pub mod tray;

#[cfg(windows)]
pub use capture::{capture, what_is_attached, Captured, Chord, Clock};
#[cfg(windows)]
pub use region::{no_region, on_handle as map_trace_region, passed_down as trace_handle, Region};
#[cfg(windows)]
pub use supervisor::{end as supervisor_end, Supervisor};

#[cfg(windows)]
mod host {
    use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
    use std::time::{Duration, Instant as Wall};

    use favjit_host::source::{SourceHost, Suppressing};
    use favjit_host::{DiscoveryHost, Entropy, EventKind, Host, HostEvent, Instant};

    use crate::capture::{self, Captured, Chord, Clock};
    use crate::link::{IdentityFile, Link};
    use crate::supervisor::Supervisor;
    use crate::suppress;
    use favjit_host::IdentityStore;

    /// The real thing: this machine's keyboards and mice in, messages to the sink
    /// out.
    pub struct WindowsHost {
        /// The chord's two positions, settled before the run and asked for by
        /// `engine` at the moment it wants to read.
        chord: Chord,
        /// The way into the stream, and the stream itself.
        ///
        /// Both from the start, and the way in kept beside the reading end: a
        /// wait before the capture thread has started comes back empty rather
        /// than saying the reading has stopped, which is what a receiver with no
        /// sender left would answer — and a run that had not asked to read yet
        /// is not one whose input has gone.
        events: Sender<Captured>,
        captured: Receiver<Captured>,
        /// The clock the captured stream's own timestamps are on.
        ///
        /// There from the start rather than once reading has been asked for, so
        /// that reading the time is not a question about whether it was: what a
        /// clock nothing has stamped against answers is its own epoch, and
        /// nothing has arrived to be read against it (ADR-0006).
        clock: Clock,
        link: Link,
        /// Whether this run has anywhere to send input at all.
        ///
        /// Beside the link rather than the link's absence, so that everything
        /// asked of it is asked of something that is there: a dry run reaches
        /// none of those operations because this is what it answers, and a link
        /// that might not exist would have every one of them ask whether it does
        /// (ADR-0006).
        ///
        /// A dry run is the first thing anyone runs, and it should work on a
        /// machine where nothing has been set up yet: an identity is a file to
        /// create and a sink is one to have pinned, and neither is needed to find
        /// out which keys arrive.
        has_a_sink: bool,
        /// This machine's own identity file, read and written the same way on
        /// every mode — a relaying run, `--identity`, and nothing else.
        identity: IdentityFile,
        /// When this run was asked to stop, if it was bounded.
        ///
        /// Two fields rather than one that may be absent, so that reading the
        /// bound is arithmetic over a value that is there: an absent one would
        /// have every reading ask whether there is a bound first (ADR-0006).
        until: Wall,
        bounded: bool,
        /// The watchdog's end of things, if a watchdog started this run (ADR-0008).
        ///
        /// Two numbers and copied to the capture thread with the rest of what it
        /// is started with: the pipes are the process's, and which thread looks at
        /// the probe one is what the capture thread is for rather than a second
        /// reading of the environment.
        supervisor: Supervisor,
    }

    impl WindowsHost {
        /// Nothing is read yet: reading starts at [`SourceHost::take_input`], which
        /// is where `engine` decides it should (ADR-0006).
        ///
        /// The supervisor is handed over rather than read here: each of its two
        /// ends is a call into the machine and a host makes one of those per
        /// operation (ADR-0006). How long a wait lasts is nothing this takes
        /// either — every wait this host makes is bounded by the deadline the run
        /// hands [`crate::Host::next_event`], so there is no cadence to hold on
        /// to.
        pub fn start(chord: Chord, has_a_sink: bool, link: Link, supervisor: Supervisor) -> Self {
            let (events, captured) = std::sync::mpsc::channel();
            Self {
                chord,
                events,
                captured,
                clock: Clock::start(),
                link,
                has_a_sink,
                identity: IdentityFile::new(crate::link::identity_path()),
                until: Wall::now(),
                bounded: false,
                supervisor,
            }
        }

        /// Stop the run at this moment, whether or not anything is typed, and
        /// only where `bounded` says there is a moment to stop at.
        ///
        /// The two apart rather than one moment that may be absent, for the
        /// reason the two fields behind them are two: an absent one would have
        /// every reading of the bound ask whether there is one first, and asking
        /// is a turning beside the call that reads the clock (ADR-0006).
        pub fn until(&mut self, deadline: Wall, bounded: bool) {
            self.until = deadline;
            self.bounded = bounded;
        }

        /// Whether the run's own deadline has come.
        ///
        /// Asked before waiting for a connection as well as before waiting for an
        /// event: a source whose Mac is switched off spends the whole run inside
        /// `find_sink`, and a deadline only the event loop honoured would never be
        /// reached.
        pub fn finished(&self) -> bool {
            self.bounded && Wall::now() >= self.until
        }

        /// How many pointer events the hook refused.
        ///
        /// No count for keys: what refuses them is the raw input registration, which
        /// stops them reaching applications without seeing them one at a time
        /// ([`suppress`]).
        pub fn refused(&self) -> usize {
            suppress::refused()
        }
    }

    /// Whatever one wait on that queue answered, as the event it stands for.
    ///
    /// One wait says one thing, so this is a conversion and not a record of
    /// facts to apply: somebody outside asking the run to stop, the reading
    /// having stopped, and a pointer that reports where it is are each their own
    /// kind on the stream, and the run is the end every one of them reaches
    /// (ADR-0006).
    ///
    /// `at` is what the reading having stopped is stamped with, because that is
    /// the one thing here the capture thread did not stamp itself: the run
    /// compares what it is handed against the clock it already read (ADR-0010).
    fn arrived(received: Result<Captured, RecvTimeoutError>, at: Instant) -> Option<HostEvent> {
        match received {
            Ok(event) => Some(event),
            Err(RecvTimeoutError::Disconnected) => Some(HostEvent {
                at,
                kind: EventKind::InputGone,
            }),
            // The ordinary state of a keyboard nobody is touching.
            Err(RecvTimeoutError::Timeout) => None,
        }
    }

    impl Drop for WindowsHost {
        /// Give the keyboards back.
        ///
        /// The number the hooks read is what refuses input, so clearing it here is
        /// the whole of the release — there is nothing the capture thread has to
        /// do and nothing to wait for it to notice. A kill or a panic skips this
        /// path, and what gives the keyboards back then is the hook belonging to
        /// this process rather than anything favjit runs.
        fn drop(&mut self) {
            suppress::take(Suppressing::Nothing);
        }
    }

    impl Entropy for WindowsHost {
        fn fill(&mut self, into: &mut [u8]) -> bool {
            // The same source `Link` and `Pairing` read the ephemeral key from:
            // a generator seeded in this process would be one more thing to be
            // wrong about, and what the code protects it protects only while it
            // is unpredictable.
            getrandom::getrandom(into).is_ok()
        }
    }

    impl DiscoveryHost for WindowsHost {
        fn bind_discovery(&mut self) -> Option<Box<dyn favjit_host::Searching + '_>> {
            self.link.bind_discovery()
        }

        fn resolve_discovered(
            &mut self,
            host: &str,
            port: u16,
            address: Option<[u8; 4]>,
        ) -> Option<favjit_host::Sink> {
            crate::mdns::resolve(host, port, address)
        }
    }

    impl IdentityStore for WindowsHost {
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

    impl SourceHost for WindowsHost {
        fn pinned_sink(&mut self) -> Option<String> {
            crate::link::read_sink(&crate::link::sink_path())
        }

        fn answer_procedures_with(&mut self, answering: favjit_host::source::Answering) {
            crate::suppress::answer_with(answering);
        }

        /// Start the capture thread and turn the run's loop on it.
        ///
        /// `false` only when that thread cannot be started at all. A window or a
        /// registration that fails does so on the thread itself, which closes the
        /// stream — and a stream that closes with time still on the clock is
        /// reading having stopped, which is the same thing said a moment later.
        fn take_input(
            &mut self,
            probe_tick: Duration,
            raw_bytes: usize,
            work: favjit_host::capture::SourceLoop,
        ) -> bool {
            let started = capture::capture(
                self.chord,
                self.supervisor,
                probe_tick,
                raw_bytes,
                work,
                self.events.clone(),
            );
            // The clock is taken whether or not a thread started, because the
            // stamps on what arrives are that thread's: a clock this side
            // started for itself would put every reading on a different origin
            // from them.
            self.clock = started.clock;
            started.reading
        }

        fn pause(&mut self, how_long: Duration) {
            self.link.pause(how_long);
        }

        /// The bound a run was given, and nothing else: somebody outside asking —
        /// the tray item's Quit (`docs/platform/windows/tray-item-as-its-own-program.md`) — arrives on the stream, so it is the
        /// run's from the moment it is told. A run that stopped because it was
        /// asked to exits cleanly either way, which is what keeps the logon task
        /// from starting it again.
        fn asked_to_stop(&mut self) -> bool {
            self.finished()
        }

        fn has_a_sink_to_look_for(&mut self) -> bool {
            self.has_a_sink
        }

        fn use_fixed_sink(&mut self) -> Option<favjit_host::Sink> {
            self.link.use_fixed()
        }

        fn connect_socket(
            &mut self,
            sink: favjit_host::Sink,
            timeout: Duration,
        ) -> Option<Box<dyn favjit_host::source::Opening>> {
            self.link.connect(sink, timeout)
        }

        fn suppress(&mut self, what: Suppressing) {
            suppress::take(what);
        }

        fn there_is_somewhere_to_show_it(&mut self) -> bool {
            suppress::there_is_a_window()
        }

        fn show_what_is_refused(&mut self, what: Suppressing) {
            suppress::draw(what);
        }
    }

    impl Host for WindowsHost {
        /// The captured stream's own clock, once reading has started.
        ///
        /// `engine`'s `source::run` never compares this against a deadline of its
        /// own — this role has no repeat to wait for — so a clock not yet started
        /// answering its own epoch costs nothing real.
        fn now(&mut self) -> Instant {
            self.clock.now()
        }

        /// `deadline` against this host's own clock, which is the capture
        /// thread's: the run bound [`WindowsHost::until`] sets is wall-clock and
        /// asked about separately, through [`crate::host::WindowsHost::asked_to_stop`].
        /// One wait and one turn of the conversion over what it answered — not a
        /// loop around them. A loop here waits again for the whole of the bound
        /// after absorbing something, so a stream of the messages the run does
        /// not name makes the bound it set unbounded; and the run is the end
        /// that holds the clock and can subtract what has already gone
        /// (ADR-0010).
        fn next_event(&mut self, deadline: Instant) -> Option<HostEvent> {
            let now = self.clock.now();
            arrived(
                self.captured.recv_timeout(Duration::from_nanos(
                    deadline.nanos.saturating_sub(now.nanos),
                )),
                now,
            )
        }

        /// Whether `favjit-watchdog` started this run and handed it the two pipes.
        ///
        /// Absence is not an error — running favjit by hand is how it gets measured —
        /// but a relaying run that refuses this machine's input has nothing to end a
        /// wedge, so the run says so before it takes anything (ADR-0008).
        fn is_supervised(&mut self) -> bool {
            self.supervisor.is_supervised()
        }

        fn warn(&mut self, message: core::fmt::Arguments) {
            log::warn!("{message}");
        }

        /// Tell the watchdog the loop came back round.
        ///
        /// The probe that asked for it arrived as an event on this host's stream, so
        /// what answers is the loop itself rather than a thread beside it — which is
        /// the whole of what makes a wedged loop detectable (ADR-0008).
        fn heartbeat(&mut self) -> Result<(), favjit_host::Trouble> {
            beaten(self.supervisor.beat())
        }
    }

    /// A beat that went, or the machine's own words about the one that did not.
    fn beaten(answered: std::io::Result<()>) -> Result<(), favjit_host::Trouble> {
        answered.map_err(|error| favjit_host::Trouble(error.to_string()))
    }
}

#[cfg(windows)]
pub use host::WindowsHost;
