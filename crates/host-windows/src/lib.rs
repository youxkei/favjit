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
//! away. Which combination a run is in is [`favjit_core::source::Request`]'s, and
//! so is what each one does.
//!
//! Everything here is as thin as the platform allows — the translation between
//! Windows' vocabulary and `core`'s, and nothing else. Whatever lives inside a
//! host is out of reach of the end-to-end suite, which is the reason to keep it
//! small. Only the three modules that call the Win32 API are behind the platform
//! gate below; the rest are compiled everywhere. [`scancode`], [`device`] and
//! [`pointer`] hold the tables and the arithmetic that turn what Windows says into
//! what `core` reads, [`mdns`] and [`link`] hold what is read off the network, and
//! [`pairing`] holds the exchange a code is spent on. Their tests are the only
//! check any of that gets, so they build wherever the suite runs.

pub mod device;
pub mod link;
pub mod mdns;
pub mod pairing;
pub mod pointer;
pub mod scancode;

#[cfg(windows)]
mod capture;
#[cfg(windows)]
mod ffi;
#[cfg(windows)]
mod supervisor;
#[cfg(windows)]
mod suppress;

#[cfg(windows)]
pub use capture::{announced_as, attached, capture, Captured, Clock, Config, UnknownScanCode};

#[cfg(windows)]
mod host {
    use std::sync::mpsc::{Receiver, RecvTimeoutError};
    use std::time::{Duration, Instant as Wall};

    use favjit_core::link::Message;
    use favjit_core::source::{Connected, Ended, SourceHost, Suppressing};
    use favjit_core::{DeviceId, Host, HostEvent};

    use crate::capture::{self, Captured, Config, UnknownScanCode};
    use crate::link::Link;
    use crate::supervisor::Supervisor;
    use crate::suppress;

    /// The real thing: this machine's keyboards and mice in, messages to the sink
    /// out.
    pub struct WindowsHost {
        /// What to read, and whether the hooks may go on the machine — both settled
        /// before the run and asked for by `core` at the moment it wants them.
        config: Config,
        /// Absent until reading has been asked for.
        captured: Option<Receiver<Captured>>,
        /// Whether `connect` gave up because there is nothing to relay to, which is
        /// what tells that ending apart from a run that reached its bound.
        no_link: bool,
        /// Absent for a dry run, which connects to nothing.
        ///
        /// A dry run is the first thing anyone runs, and it should work on a
        /// machine where nothing has been set up yet: an identity is a file to
        /// create and a sink is one to have pinned, and neither is needed to find
        /// out which keys arrive.
        link: Option<Link>,
        /// Reported rather than swallowed, so a key no rule can name is visible
        /// instead of a key that silently stopped working.
        pub unknown: Vec<UnknownScanCode>,
        /// Pointers whose reports say where they are rather than how far they
        /// moved, which this relay cannot carry.
        pub absolute: Vec<DeviceId>,
        /// What arrived, and what went out.
        ///
        /// Kept because the pair is the measurement that says the two halves of
        /// this host are working together: while the hooks are refusing input,
        /// events still have to arrive as raw input, and refusals with no
        /// arrivals beside them is input that has gone nowhere.
        pub keys: usize,
        pub pointers: usize,
        pub sent: usize,
        /// How many times the keyboards were taken.
        pub suppressions: usize,
        until: Option<Wall>,
        /// The watchdog's end of things, if a watchdog started this run (ADR-0008).
        supervisor: Supervisor,
        /// Whether the heartbeat's failure has been reported already.
        link_broken: bool,
    }

    /// How often the wait comes back to look at the deadline.
    ///
    /// Only while there is one: without a deadline the wait should park on the
    /// channel rather than wake four times a second for as long as it lives.
    const POLL: Duration = Duration::from_millis(250);

    impl WindowsHost {
        /// Nothing is read yet: reading starts at [`SourceHost::take_input`], which
        /// is where `core` decides it should (ADR-0006).
        pub fn start(config: Config, link: Option<Link>) -> Self {
            Self {
                config,
                captured: None,
                no_link: false,
                link,
                unknown: Vec::new(),
                absolute: Vec::new(),
                keys: 0,
                pointers: 0,
                sent: 0,
                suppressions: 0,
                until: None,
                supervisor: Supervisor::from_env(),
                link_broken: false,
            }
        }

        /// Stop the run at this moment, whether or not anything is typed.
        pub fn until(&mut self, deadline: Option<Wall>) {
            self.until = deadline;
        }

        /// Whether the run's own deadline has come.
        ///
        /// Asked before waiting for a connection as well as before waiting for an
        /// event: a source whose Mac is switched off spends the whole run inside
        /// `connect`, and a deadline only the event loop honoured would never be
        /// reached.
        pub fn finished(&self) -> bool {
            self.until.is_some_and(|deadline| Wall::now() >= deadline)
        }

        /// This machine's key, for the log line that says which machine this is.
        /// Absent for a run with no link, which has no identity either.
        pub fn fingerprint(&self) -> Option<String> {
            self.link.as_ref().map(Link::fingerprint)
        }

        /// How many pointer events the hook refused.
        ///
        /// No count for keys: what refuses them is the raw input registration, which
        /// stops them reaching applications without seeing them one at a time
        /// ([`suppress`]).
        pub fn refused(&self) -> usize {
            suppress::refused()
        }

        fn recv(&mut self) -> Option<HostEvent> {
            loop {
                if self.finished() {
                    return None;
                }
                // Nothing has been asked for yet, which is a stream with nothing in
                // it rather than a wait: the order is `core`'s, and it asks to read
                // before it asks for an event.
                let captured = self.captured.as_ref()?;
                let left = self
                    .until
                    .map(|deadline| deadline.saturating_duration_since(Wall::now()).min(POLL));
                let received = match left {
                    Some(wait) => match captured.recv_timeout(wait) {
                        Ok(captured) => Ok(captured),
                        Err(RecvTimeoutError::Timeout) => continue,
                        Err(RecvTimeoutError::Disconnected) => Err(()),
                    },
                    None => captured.recv().map_err(|_| ()),
                };
                match received {
                    Ok(Captured::Event(event)) => {
                        match event.kind {
                            favjit_core::EventKind::Pointer { .. } => self.pointers += 1,
                            favjit_core::EventKind::KeyDown { .. }
                            | favjit_core::EventKind::KeyUp { .. } => self.keys += 1,
                            _ => {}
                        }
                        return Some(event);
                    }
                    Ok(Captured::Unknown(unknown)) => {
                        if !self.unknown.contains(&unknown) {
                            self.unknown.push(unknown);
                        }
                    }
                    Ok(Captured::Absolute(device)) => {
                        if !self.absolute.contains(&device) {
                            self.absolute.push(device);
                        }
                    }
                    Err(()) => return None,
                }
            }
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

    impl SourceHost for WindowsHost {
        /// Start the capture thread, with the hooks in it if refusing may be asked
        /// for.
        ///
        /// `false` only when that thread cannot be started at all. A window or a
        /// registration that fails does so on the thread itself, which closes the
        /// stream — and that arrives as [`Ended::InputGone`], which is the same
        /// thing said a moment later.
        fn take_input(&mut self, may_suppress: bool) -> bool {
            self.captured = capture::capture(self.config.clone(), may_suppress)
                .map(|(captured, _clock)| captured);
            self.captured.is_some()
        }

        fn pause(&mut self) {
            if let Some(link) = self.link.as_ref() {
                link.pause();
            }
        }

        fn find_sink(&mut self) -> Connected {
            if self.finished() {
                return Connected::Done;
            }
            match self.link.as_mut() {
                Some(link) => link.find(),
                None => {
                    // Noted rather than only answered, because `Done` is also what
                    // a run that reached its bound gets, and the two are different
                    // endings.
                    self.no_link = true;
                    Connected::Done
                }
            }
        }

        fn open_session(&mut self) -> bool {
            self.link.as_mut().is_some_and(Link::open)
        }

        /// Why the run can go no further.
        ///
        /// Reported and not acted on: which of these means a failure and which
        /// means a finished run is `favjit_core::source`'s (ADR-0006).
        fn ended(&mut self) -> Ended {
            if self.no_link {
                return Ended::NoLink;
            }
            match self.finished() {
                true => Ended::AsAsked,
                // The stream closed with time still on the clock, which is the
                // capture thread having stopped.
                false => Ended::InputGone,
            }
        }

        fn suppress(&mut self, what: Suppressing) {
            if what == Suppressing::Everything {
                self.suppressions += 1;
            }
            suppress::take(what);
        }

        fn send(&mut self, message: Message) -> bool {
            let sent = self.link.as_mut().is_some_and(|link| link.send(message));
            if sent {
                self.sent += 1;
            }
            sent
        }
    }

    impl Host for WindowsHost {
        fn next_event(&mut self) -> Option<HostEvent> {
            self.recv()
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
        fn heartbeat(&mut self) {
            if let Err(error) = self.supervisor.beat() {
                // Once, not per event: the link does not come back, and a warning per
                // keystroke would be the loudest thing in the log for the rest of the
                // run.
                if !self.link_broken {
                    self.link_broken = true;
                    log::error!(
                        "the heartbeat is not reaching the watchdog ({error}); it will end this \
                         process for a broken link rather than for a fault"
                    );
                }
            }
        }
    }
}

#[cfg(windows)]
pub use host::WindowsHost;
