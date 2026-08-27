//! A run recorded on one machine, replayed as a script on another (ADR-0009).
//!
//! What makes this possible is that `core` is a function of its event stream, and
//! that the simulated host drives it through the same stream — so a recording is
//! the same shape as a script. These pin the property the whole design rests on:
//! **the same trace produces the same keystrokes.** A trace that did not would be
//! a debugging tool that lies about what happened.
//!
//! The buffer is plain process memory here, handed in by the caller, because
//! ADR-0006 has the host provide it and `host-sim`'s host is this process.

use core::time::Duration;

use favjit_core::sink::{self, Request};
use favjit_core::{
    trace::Trace, Buttons, DeviceId, DeviceInfo, Injected, Key, Layout, PointerReport,
};
use favjit_host_sim::SimHost;

const BUILT_IN: DeviceId = DeviceId(1);
const TRACKPOINT: DeviceId = DeviceId(2);

fn converting() -> Request {
    Request::Injecting { listen: false }
}

/// Enough of everything to be worth replaying: a layer, a tap-hold decided both
/// ways, a shifted character, a pointer, and a keyboard torn away while it holds
/// something.
fn script(host: &mut SimHost) {
    host.attach(DeviceInfo::built_in(BUILT_IN));
    host.attach(DeviceInfo::external(TRACKPOINT, 6127, 24801));

    host.tap(BUILT_IN, Key::S);
    host.tap(TRACKPOINT, Key::Q);

    // Space held with another key: shift. Then space alone: a space.
    host.press(BUILT_IN, Key::Spacebar).advance(ms(20));
    host.tap(BUILT_IN, Key::A);
    host.advance(ms(20)).release(BUILT_IN, Key::Spacebar);
    host.hold(BUILT_IN, Key::Spacebar, ms(60));

    // The Henkan layer.
    host.press(BUILT_IN, Key::RightCommand);
    host.tap(BUILT_IN, Key::Q);
    host.release(BUILT_IN, Key::RightCommand);

    host.pointer(TRACKPOINT, PointerReport::moved(4, -2));
    host.pointer(
        TRACKPOINT,
        PointerReport {
            buttons: Buttons::NONE.with(1),
            ..PointerReport::default()
        },
    );
    host.pointer(TRACKPOINT, PointerReport::default());

    // Torn away holding a modifier, which is the case that strands one.
    host.press(TRACKPOINT, Key::LeftShift);
    host.detach(TRACKPOINT);

    host.probe();
}

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

/// Run the script, recording into a buffer of this size, and return what reached
/// applications alongside the recording.
fn record(bytes: usize) -> (Vec<Injected>, Vec<u8>) {
    let mut memory = vec![0u8; bytes];
    let mut host = SimHost::new();
    script(&mut host);
    {
        sink::run(
            &converting(),
            Layout::dudrack(),
            None,
            &mut host,
            Some(&mut memory),
        );
    }
    (host.injected(), memory)
}

/// Replay a recording through a fresh simulated host, from the state its
/// checkpoint recorded.
///
/// From the checkpoint and not from nothing: a bounded trace has lost its start,
/// so the events that survive describe changes to a state, and starting empty
/// would replay them against a sink that had never seen the keyboards.
fn replay(memory: &[u8]) -> Vec<Injected> {
    let trace = Trace::read(memory);
    let mut host = SimHost::from_trace(&trace);
    match trace.checkpoint() {
        // The loop directly, because a recording is not a machine to bring up: what
        // a replay reconstructs is the conversion, and everything a run does before
        // it happened once, on the machine the trace came from.
        Some(checkpoint) => sink::convert_from(Layout::dudrack(), None, &checkpoint, &mut host),
        None => sink::convert(Layout::dudrack(), None, &mut host),
    }
    host.injected()
}

#[test]
fn a_trace_replays_to_the_same_keystrokes() {
    let (typed, memory) = record(64 * 1024);
    assert!(!typed.is_empty(), "the script has to produce something");
    assert_eq!(replay(&memory), typed);
}

#[test]
fn a_trace_records_what_was_sent_as_well_as_what_arrived() {
    // Outbound calls with their results, not only the inbound events: a rejected
    // injection changes what `core` does next, so a trace without them is one
    // replay can diverge from.
    let (typed, memory) = record(64 * 1024);
    let trace = Trace::read(&memory);

    assert_eq!(
        trace.injected(),
        typed,
        "the recording of what was sent has to match what was sent"
    );
}

#[test]
fn what_survives_eviction_still_begins_at_a_checkpoint() {
    // Small enough that the run cannot fit, so the oldest segments go. Whatever
    // is left has to start at a checkpoint, or the events that remain describe
    // changes to a state nothing knows.
    // Room for fifteen records, against a script that writes far more.
    let (_, memory) = record(512);
    let trace = Trace::read(&memory);

    assert!(trace.evicted() > 0, "the buffer has to have overflowed");
    assert!(
        trace.begins_at_a_checkpoint(),
        "a trace that begins mid-stream cannot be replayed from"
    );
}

#[test]
fn replaying_what_survived_eviction_reproduces_the_end_of_the_run() {
    // The checkpoint is what makes this possible: replay starts from the state
    // `core` was in when the surviving segment began, so the keystrokes from that
    // point on come out the same.
    let (typed, memory) = record(512);
    let tail = replay(&memory);

    assert!(
        typed.ends_with(&tail),
        "replaying from the checkpoint should reproduce the end of the run;\n  \
         whole: {typed:?}\n  tail: {tail:?}"
    );
    assert!(
        !tail.is_empty(),
        "something has to survive, or the buffer is too small to be useful"
    );
    assert!(
        tail.len() < typed.len(),
        "this test is only meaningful while the trace really did lose the start"
    );
}

#[test]
fn the_trace_holds_the_event_the_run_stopped_on() {
    // What a wedged converter's recording has to contain. A loop that stops after
    // handling an event — because a supervisor is about to kill it, or because it
    // wedged there on purpose — leaves a trace that has to say what favjit was
    // given last: the key that was down when it stopped is the whole of why a
    // stuck key happened, and a recording missing it explains nothing.
    //
    // Pinned here rather than around the wedge itself, which lives in the binary
    // and wraps a real host: this suite cannot reach one without becoming
    // platform-specific (ADR-0005), so what it holds is the half `core` owns —
    // that the event is recorded before it is handled, so anything stopping after
    // that keeps it.
    struct StopsAfterAKey<'a> {
        inner: &'a mut SimHost,
        handled_a_key: bool,
        stop: bool,
    }

    impl favjit_core::sink::SinkInputHost for StopsAfterAKey<'_> {
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

        fn ended(&mut self) -> favjit_core::Ended {
            self.inner.ended()
        }
    }

    impl favjit_core::Host for StopsAfterAKey<'_> {
        fn next_event(&mut self) -> Option<favjit_core::HostEvent> {
            if self.stop {
                return None;
            }
            let event = self.inner.next_event()?;
            if matches!(event.kind, favjit_core::EventKind::KeyDown { .. }) {
                self.handled_a_key = true;
            }
            Some(event)
        }

        fn is_supervised(&mut self) -> bool {
            self.inner.is_supervised()
        }

        fn warn(&mut self, message: core::fmt::Arguments) {
            self.inner.warn(message);
        }

        /// Where the loop gives up, which is where a wedge stops it: after the
        /// event is through.
        fn heartbeat(&mut self) {
            self.inner.heartbeat();
            if self.handled_a_key {
                self.stop = true;
            }
        }
    }

    impl favjit_core::pairing::IdentityStore for StopsAfterAKey<'_> {
        fn read(&mut self) -> Option<Vec<u8>> {
            self.inner.read()
        }

        fn make(&mut self) -> Option<favjit_core::pairing::Identity> {
            self.inner.make()
        }

        fn keep(&mut self, bytes: &[u8]) -> bool {
            self.inner.keep(bytes)
        }
    }

    impl favjit_core::sink::SinkHost for StopsAfterAKey<'_> {
        fn set_timer(&mut self, at: Option<favjit_core::Instant>) {
            self.inner.set_timer(at);
        }

        fn open_output(&mut self) -> bool {
            self.inner.open_output()
        }

        fn tune_output(&mut self) {
            self.inner.tune_output();
        }

        fn bind_link(
            &mut self,
            identity: &favjit_core::pairing::Identity,
        ) -> Option<Box<dyn favjit_core::link::LinkHost + Send>> {
            self.inner.bind_link(identity)
        }

        fn run_alongside(&mut self, work: Box<dyn FnOnce() + Send>) -> bool {
            self.inner.run_alongside(work)
        }

        fn inject(&mut self, injected: Injected) {
            self.inner.inject(injected);
        }
    }

    let mut memory = vec![0u8; 64 * 1024];
    let mut sim = SimHost::new();
    sim.attach(DeviceInfo::built_in(BUILT_IN));
    sim.press(BUILT_IN, Key::S);
    sim.release(BUILT_IN, Key::S);
    {
        let mut host = StopsAfterAKey {
            inner: &mut sim,
            handled_a_key: false,
            stop: false,
        };
        sink::run(
            &converting(),
            Layout::dudrack(),
            None,
            &mut host,
            Some(&mut memory),
        );
    }

    let trace = Trace::read(&memory);
    let events = trace.events();
    let last = events.last().expect("the run handled something");
    assert!(
        matches!(
            last.kind,
            favjit_core::EventKind::KeyDown { key: Key::S, .. }
        ),
        "the last event recorded should be the one it stopped on; got {:?}",
        last.kind
    );
    assert!(
        !trace.injected().is_empty(),
        "and what that event produced has to be there too, or the recording says \
         a key arrived and nothing came of it"
    );
}

#[test]
fn a_trace_of_a_quiet_run_is_replayable_too() {
    // Nothing typed, one keyboard attached. The empty case is where an
    // off-by-one in the ring shows up as a trace that cannot be read at all.
    let mut memory = vec![0u8; 4 * 1024];
    let mut host = SimHost::new();
    host.attach(DeviceInfo::built_in(BUILT_IN));
    {
        sink::run(
            &converting(),
            Layout::dudrack(),
            None,
            &mut host,
            Some(&mut memory),
        );
    }

    assert_eq!(replay(&memory), Vec::<Injected>::new());
}
